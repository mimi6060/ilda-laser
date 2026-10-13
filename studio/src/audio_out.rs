//! Output through an audio interface (T-498): X, Y (and R, G, B in laser
//! mode) as audio samples on the channels the operator picks, for an
//! oscilloscope in XY mode, or a DC-coupled interface wired to a
//! projector's ILDA input.
//!
//! Our own code on `cpal` (Core Audio), the crate the audio analysis
//! already uses. `laser-dac`'s `oscilloscope` and `avb` features were not
//! kept: they pull `cpal` 0.15, which can't sit beside our `cpal` 0.18
//! (both link the same native audio library: Cargo refuses the build),
//! they have no channel assignment and no oscilloscope-only mode, and
//! they would make every sound card of the Mac show up in the DAC scan.
//!
//! Safety, as for `DacOutput`:
//! - the output opens disarmed, and every launch starts in **oscilloscope
//!   mode** (X/Y only, the colour channels always 0): the mode is never
//!   saved. Laser mode needs an explicit confirmation naming the risk
//!   (`LASER_CONFIRM_FR`), and is chosen while the output is disconnected;
//! - disarmed, e-stop (kill switch, lock-free), blackout, shutdown, panic:
//!   **every channel at 0** (centre, black) from the next audio buffer.
//!   The stream keeps running and keeps sending those zeros until closed;
//! - frames are never cut: a new frame starts once the current one has
//!   played, after a dark travel (dwell, eased move, dwell) from where it
//!   ended. Points are clamped again here (x, y in -1..1, colours 0..1);
//! - the device gone (unplugged, Core Audio error) is a fault: the engine
//!   disarms with « Sortie perdue », the manager reconnects disarmed;
//! - the Mac's default output (music, alerts, the song playback) and the
//!   audio analysis' input device are refused: nothing else may ever mix
//!   into these channels.
//!
//! What a stopped audio stream leaves on the interface's outputs is up to
//! the interface: the projector's own hardware stop stays the real one.
//!
//! The audio callback never allocates nor waits: it `try_lock`s the frame
//! slot (a busy slot means the current frame plays once more) and swaps
//! buffers with the engine, which refills the old one.

use crate::output::Output;
use crate::patterns::Point;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const KIND: &str = "audio";
/// Output manager ids: `audio:<Core Audio device name>`.
pub const ID_PREFIX: &str = "audio:";
/// Highest channel number one can assign.
pub const MAX_CHANNEL: u16 = 64;
/// The panel's permanent warning.
pub const WARNING_FR: &str = "Seules les interfaces couplées en continu conviennent à un laser. Une interface ordinaire déforme le signal : utilisez-la seulement pour un oscilloscope.";
/// What the operator confirms to enter laser mode.
pub const LASER_CONFIRM_FR: &str = "Passer en mode Laser ? Les canaux R, V, B allumeront le laser. Une interface audio ordinaire (couplée en alternatif) déforme X et Y : le faisceau peut se figer en un point ou sortir de la zone prévue, un danger pour les yeux du public. Ce mode demande une interface couplée en continu câblée vers l'entrée ILDA, et l'arrêt d'urgence matériel du projecteur à portée de main.";
/// The interfaces the fake backend lists (tests and e2e, never Core Audio).
pub const FAKE_DEVICES: [(&str, u16); 2] = [("Interface de test (8 canaux)", 8), ("Interface de test stéréo", 2)];
/// Sample rate of the fake output.
const FAKE_RATE: u32 = 48_000;

/// What the channels carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// X and Y only, the colour channels always at 0.
    #[default]
    Scope,
    /// X, Y, R, G, B: a DC-coupled interface wired to a laser.
    Laser,
}

impl Mode {
    pub fn label_fr(self) -> &'static str {
        match self {
            Mode::Scope => "Oscilloscope",
            Mode::Laser => "Laser",
        }
    }
}

/// Channel numbers (1-based, as on the interface).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMap {
    pub x: u16,
    pub y: u16,
    pub r: u16,
    pub g: u16,
    pub b: u16,
}

impl Default for ChannelMap {
    fn default() -> Self {
        Self { x: 1, y: 2, r: 3, g: 4, b: 5 }
    }
}

impl ChannelMap {
    fn all(&self) -> [u16; 5] {
        [self.x, self.y, self.r, self.g, self.b]
    }

    /// Channels a stream needs in `mode`.
    pub fn needed(&self, mode: Mode) -> u16 {
        match mode {
            Mode::Scope => self.x.max(self.y),
            Mode::Laser => self.all().into_iter().max().unwrap_or(2),
        }
    }

    /// 0-based indices of x, y, r, g, b; the colours are None in scope mode.
    fn indices(&self, mode: Mode) -> [Option<usize>; 5] {
        let i = |c: u16| Some(c as usize - 1);
        let colour = |c: u16| (mode == Mode::Laser).then(|| c as usize - 1);
        [i(self.x), i(self.y), colour(self.r), colour(self.g), colour(self.b)]
    }
}

/// `<data-dir>/audio_output.json`: machine settings, never in a project.
/// The mode is not in it: every launch starts in oscilloscope mode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioOutConfig {
    /// Core Audio device name. None: the output is off (not listed).
    pub device: Option<String>,
    pub channels: ChannelMap,
}

impl AudioOutConfig {
    pub fn validate(&self) -> Result<()> {
        let all = self.channels.all();
        if all.iter().any(|&c| !(1..=MAX_CHANNEL).contains(&c)) {
            bail!("canaux 1 à {MAX_CHANNEL}");
        }
        for (i, a) in all.iter().enumerate() {
            if all[i + 1..].contains(a) {
                bail!("le canal {a} est affecté deux fois");
            }
        }
        if let Some(d) = &self.device {
            if d.trim().is_empty() || d.len() > 200 {
                bail!("nom de périphérique audio invalide");
            }
        }
        Ok(())
    }

    pub fn id(&self) -> Option<String> {
        Some(format!("{ID_PREFIX}{}", self.device.as_deref()?))
    }
}

/// One output device for the panel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioDevice {
    pub name: String,
    /// Most channels it offers.
    pub channels: u16,
}

/// The configuration and the mode, shared by the output backend (listing
/// and opening) and the HTTP handlers.
pub struct AudioOutStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    cfg: Mutex<AudioOutConfig>,
    mode: Mutex<Mode>,
    /// Tests and e2e: `FAKE_DEVICES`, Core Audio is never asked.
    fake: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AudioOutStore {
    /// A missing or invalid file: off. Always oscilloscope mode.
    pub fn load(path: PathBuf) -> Self {
        let cfg: AudioOutConfig = crate::load_json(&path);
        let cfg = if cfg.validate().is_ok() { cfg } else { AudioOutConfig::default() };
        Self { path: Some(path), cfg: Mutex::new(cfg), mode: Mutex::new(Mode::Scope), fake: AtomicBool::new(false) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, cfg: Mutex::default(), mode: Mutex::new(Mode::Scope), fake: AtomicBool::new(true) }
    }

    /// Tests and e2e (the fake output backend): never list Core Audio.
    pub fn use_fake_devices(&self) {
        self.fake.store(true, Ordering::SeqCst);
    }

    pub fn is_fake(&self) -> bool {
        self.fake.load(Ordering::SeqCst)
    }

    pub fn get(&self) -> AudioOutConfig {
        lock(&self.cfg).clone()
    }

    pub fn mode(&self) -> Mode {
        *lock(&self.mode)
    }

    /// Checks and saves. Laser mode only with `confirm_laser`; going back
    /// to oscilloscope mode never needs anything.
    pub fn set(&self, mut cfg: AudioOutConfig, mode: Mode, confirm_laser: bool) -> Result<AudioOutConfig> {
        cfg.device = cfg.device.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        cfg.validate()?;
        if mode == Mode::Laser && !confirm_laser {
            bail!("Mode Laser : confirmation requise. {LASER_CONFIRM_FR}");
        }
        if let Some(path) = &self.path {
            let bytes = serde_json::to_vec_pretty(&cfg).context("réglages de l'interface audio illisibles")?;
            crate::datafile::replace(path, &bytes).with_context(|| format!("impossible d'écrire {}", path.display()))?;
        }
        *lock(&self.cfg) = cfg.clone();
        *lock(&self.mode) = mode;
        Ok(cfg)
    }

    /// The listed entry (none while off).
    pub fn entry(&self) -> Option<crate::devices::DeviceInfo> {
        let cfg = self.get();
        let id = cfg.id()?;
        Some(crate::devices::DeviceInfo::new(KIND.into(), id, output_name(cfg.device.as_deref()?, self.mode())))
    }

    /// The Mac's output devices (Core Audio's list: nothing is opened),
    /// or the fake ones.
    pub fn devices(&self) -> Vec<AudioDevice> {
        if self.is_fake() {
            return FAKE_DEVICES.iter().map(|(n, c)| AudioDevice { name: n.to_string(), channels: *c }).collect();
        }
        real::devices()
    }

    /// Opens `id` with the stored channels and mode: a real interface, or
    /// with the fake devices a stream-less output (no sound card at all).
    pub fn open(&self, id: &str, pps: u32) -> Result<AudioOutput> {
        let device = id.strip_prefix(ID_PREFIX).filter(|d| !d.is_empty()).ok_or_else(|| anyhow::anyhow!("sortie audio invalide : « {id} »"))?;
        let (cfg, mode) = (self.get(), self.mode());
        if self.is_fake() {
            let (_, channels) = FAKE_DEVICES.iter().find(|(n, _)| *n == device).ok_or_else(|| anyhow::anyhow!("interface audio « {device} » introuvable"))?;
            return AudioOutput::fake(device, cfg.channels, mode, pps, *channels);
        }
        AudioOutput::open(device, cfg.channels, mode, pps)
    }
}

/// « Interface audio : MOTU M4 (oscilloscope) ».
pub fn output_name(device: &str, mode: Mode) -> String {
    format!("Interface audio : {device} ({})", mode.label_fr().to_lowercase())
}

/// Whether the audio analysis' input is the same interface (same Core
/// Audio name): refused, so nothing it records could ever loop back.
pub fn clashes_with_input(output: Option<&str>, input: Option<&str>) -> bool {
    matches!((output, input), (Some(a), Some(b)) if a.trim().eq_ignore_ascii_case(b.trim()))
}

/// One point as the channels carry it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// Clamped again (calibration already clamps; this is the last word), a
/// NaN goes to 0; colours only in laser mode and only for a lit point.
pub fn to_sample(p: &Point, mode: Mode) -> Sample {
    let f = |v: f32, lo: f32| if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 };
    let lit = mode == Mode::Laser && p.is_lit();
    let c = |v: f32| if lit { f(v, 0.0) } else { 0.0 };
    Sample { x: f(p.x, -1.0), y: f(p.y, -1.0), r: c(p.r), g: c(p.g), b: c(p.b) }
}

/// What the engine and the audio callback share.
#[derive(Default)]
pub struct Feed {
    armed: AtomicBool,
    /// Bumped by `blank_now` and the kill switch: the callback drops what
    /// it plays at once.
    cut: AtomicU64,
    slot: Mutex<Slot>,
    failed: AtomicBool,
    error: Mutex<Option<String>>,
    callbacks: AtomicU64,
}

#[derive(Default)]
struct Slot {
    frame: Vec<Sample>,
    fresh: bool,
}

impl Feed {
    /// The device is gone: reported by `Output::fault`.
    fn fail(&self, why: String) {
        *lock(&self.error) = Some(why);
        self.failed.store(true, Ordering::Release);
    }

    fn kill(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.cut.fetch_add(1, Ordering::SeqCst);
    }
}

/// `laser-dac`'s dwells around a frame change: 100 µs where a frame ends,
/// 400 µs where the next starts.
const DWELL_END_S: f64 = 100e-6;
const DWELL_START_S: f64 = 400e-6;
/// Longest dark move between two frames, in points (as `laser-dac`).
const TRAVEL_POINTS: f64 = 64.0;

/// A dark travel between two frames: computed sample by sample, no buffer.
#[derive(Clone, Copy, Debug)]
struct Travel {
    from: (f32, f32),
    to: (f32, f32),
    end: u32,
    moving: u32,
    start: u32,
    i: u32,
}

impl Travel {
    fn new(from: (f32, f32), to: (f32, f32), rate: u32, step: f64) -> Self {
        let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt() as f64;
        let points = (dist / 2.0 * TRAVEL_POINTS).ceil().min(TRAVEL_POINTS);
        let samples = |s: f64| (s * rate as f64).round().max(1.0) as u32;
        Self { from, to, end: samples(DWELL_END_S), moving: (points / step).ceil() as u32, start: samples(DWELL_START_S), i: 0 }
    }

    fn next(&mut self) -> Option<Sample> {
        let i = self.i;
        self.i += 1;
        let at = |(x, y): (f32, f32)| Sample { x, y, ..Default::default() };
        if i < self.end {
            return Some(at(self.from));
        }
        let m = i - self.end;
        if m < self.moving {
            let t = (m + 1) as f32 / (self.moving + 1) as f32;
            let e = t * t * (3.0 - 2.0 * t);
            return Some(at((self.from.0 + (self.to.0 - self.from.0) * e, self.from.1 + (self.to.1 - self.from.1) * e)));
        }
        (m - self.moving < self.start).then(|| at(self.to))
    }
}

/// The audio callback's side: plays the current frame in a loop at `pps`
/// whatever the device's rate (nearest point), takes a new one at the end
/// of a frame. Never allocates, never waits.
pub struct Player {
    feed: Arc<Feed>,
    current: Vec<Sample>,
    cursor: f64,
    /// Points per sample: pps / sample rate.
    step: f64,
    rate: u32,
    travel: Option<Travel>,
    /// Where the beam was last sent.
    last: (f32, f32),
    map: [Option<usize>; 5],
    channels: usize,
    cut: u64,
}

impl Player {
    fn new(feed: Arc<Feed>, map: ChannelMap, mode: Mode, pps: u32, rate: u32, channels: u16) -> Self {
        let cut = feed.cut.load(Ordering::SeqCst);
        let channels = channels as usize;
        let map = map.indices(mode).map(|i| i.filter(|&i| i < channels));
        Self { feed, current: Vec::with_capacity(8192), cursor: 0.0, step: pps.max(1) as f64 / rate.max(1) as f64, rate: rate.max(1), travel: None, last: (0.0, 0.0), map, channels: channels.max(1), cut }
    }

    /// Swaps in the engine's newest frame, if there is one and the slot
    /// is free right now.
    fn take_fresh(&mut self) {
        if let Ok(mut slot) = self.feed.slot.try_lock() {
            if slot.fresh {
                std::mem::swap(&mut slot.frame, &mut self.current);
                slot.fresh = false;
            }
        }
    }

    /// Back to the centre, dark: the next frame starts with a travel.
    fn reset(&mut self) {
        self.travel = None;
        self.cursor = self.current.len() as f64;
        self.last = (0.0, 0.0);
    }

    fn next_sample(&mut self) -> Sample {
        let cut = self.feed.cut.load(Ordering::Acquire);
        if cut != self.cut {
            self.cut = cut;
            self.take_fresh();
            self.reset();
            return Sample::default();
        }
        if !self.feed.armed.load(Ordering::Acquire) {
            self.reset();
            return Sample::default();
        }
        if let Some(t) = self.travel.as_mut() {
            if let Some(s) = t.next() {
                self.last = (s.x, s.y);
                return s;
            }
            self.travel = None;
            self.cursor = 0.0;
        }
        if let Some(&s) = self.current.get(self.cursor as usize) {
            self.cursor += self.step;
            self.last = (s.x, s.y);
            return s;
        }
        // End of the frame: the newest one (or this one again), after a
        // dark travel from here to its first point.
        self.take_fresh();
        let Some(first) = self.current.first() else {
            self.last = (0.0, 0.0);
            return Sample::default();
        };
        let mut travel = Travel::new(self.last, (first.x, first.y), self.rate, self.step);
        self.cursor = 0.0;
        match travel.next() {
            Some(s) => {
                self.travel = Some(travel);
                s
            }
            None => self.next_sample_in_frame(),
        }
    }

    fn next_sample_in_frame(&mut self) -> Sample {
        let s = self.current.get(self.cursor as usize).copied().unwrap_or_default();
        self.cursor += self.step;
        self.last = (s.x, s.y);
        s
    }

    /// Fills one interleaved buffer; channels not assigned stay at 0.
    pub fn render(&mut self, out: &mut [f32]) {
        self.feed.callbacks.fetch_add(1, Ordering::Relaxed);
        for frame in out.chunks_mut(self.channels) {
            let s = self.next_sample();
            frame.fill(0.0);
            for (idx, v) in self.map.iter().zip([s.x, s.y, s.r, s.g, s.b]) {
                if let Some(slot) = idx.and_then(|i| frame.get_mut(i)) {
                    *slot = v;
                }
            }
        }
    }
}

pub struct AudioOutput {
    name: String,
    device: String,
    feed: Arc<Feed>,
    mode: Mode,
    map: ChannelMap,
    pps: u32,
    rate: u32,
    channels: u16,
    /// The thread that owns the Core Audio stream (none for the fake).
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioOutput {
    /// Opens the interface named `device` (never the Mac's default output),
    /// disarmed: its stream starts with zeros.
    pub fn open(device: &str, map: ChannelMap, mode: Mode, pps: u32) -> Result<Self> {
        if cfg!(test) {
            bail!("tests : aucune interface audio réelle n'est ouverte");
        }
        let feed = Arc::new(Feed::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("audio-laser-out".into())
            .spawn({
                let (feed, stop, device) = (Arc::clone(&feed), Arc::clone(&stop), device.to_string());
                move || real::run(&device, map, mode, pps, feed, stop, tx)
            })
            .context("failed to start the audio output thread")?;
        let opened = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err(format!("l'interface « {device} » ne répond pas")));
        match opened {
            Ok((rate, channels)) => Ok(Self { name: output_name(device, mode), device: device.into(), feed, mode, map, pps, rate, channels, stop, thread: Some(thread) }),
            Err(e) => {
                stop.store(true, Ordering::SeqCst);
                thread.thread().unpark();
                let _ = thread.join();
                bail!(e)
            }
        }
    }

    /// No sound card: the frames only reach `player()` (tests, e2e).
    pub fn fake(device: &str, map: ChannelMap, mode: Mode, pps: u32, channels: u16) -> Result<Self> {
        if map.needed(mode) > channels {
            bail!("« {device} » n'a que {channels} canaux (il en faut {})", map.needed(mode));
        }
        Ok(Self { name: output_name(device, mode), device: device.into(), feed: Arc::default(), mode, map, pps, rate: FAKE_RATE, channels, stop: Arc::default(), thread: None })
    }

    /// The audio callback's side, as the stream would run it (tests).
    #[cfg(test)]
    pub fn player(&self) -> Player {
        Player::new(Arc::clone(&self.feed), self.map, self.mode, self.pps, self.rate, self.channels)
    }
}

impl Output for AudioOutput {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> &'static str {
        KIND
    }
    fn set_armed(&mut self, armed: bool) -> Result<()> {
        if armed {
            self.feed.armed.store(true, Ordering::SeqCst);
        } else {
            self.feed.kill();
        }
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        let mut slot = lock(&self.feed.slot);
        slot.frame.clear();
        slot.frame.extend(points.iter().map(|p| to_sample(p, self.mode)));
        slot.fresh = true;
    }
    fn blank_now(&mut self) -> Result<()> {
        {
            let mut slot = lock(&self.feed.slot);
            slot.frame.clear();
            slot.fresh = true;
        }
        self.feed.cut.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let feed = Arc::clone(&self.feed);
        Some(Box::new(move || feed.kill()))
    }
    fn fault(&self) -> Option<String> {
        if !self.feed.failed.load(Ordering::Acquire) {
            return None;
        }
        Some(self.feed.error.try_lock().ok().and_then(|e| e.clone()).unwrap_or_else(|| format!("{} ne répond plus", self.name)))
    }
    fn status(&self) -> Option<serde_json::Value> {
        Some(json!({
            "kind": KIND,
            "device": self.device,
            "mode": self.mode,
            "pps": self.pps,
            "sample_rate": self.rate,
            "channels": self.channels,
            "map": self.map,
            "callbacks": self.feed.callbacks.load(Ordering::Relaxed),
            "warning": WARNING_FR,
        }))
    }
}

impl Drop for AudioOutput {
    /// Zeros on every channel, a few buffers' time for the interface to
    /// play them, then the stream closes.
    fn drop(&mut self) {
        self.feed.kill();
        if let Some(t) = self.thread.take() {
            std::thread::sleep(Duration::from_millis(60));
            self.stop.store(true, Ordering::SeqCst);
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Core Audio through `cpal`: only ever reached from a real (non-test)
/// output backend.
mod real {
    use super::{AudioDevice, ChannelMap, Feed, Mode, Player};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    fn name(d: &cpal::Device) -> Option<String> {
        d.description().ok().map(|desc| desc.name().to_string())
    }

    pub fn devices() -> Vec<AudioDevice> {
        let host = cpal::default_host();
        let Ok(list) = host.output_devices() else { return Vec::new() };
        let mut out: Vec<AudioDevice> = Vec::new();
        for d in list {
            let Some(n) = name(&d) else { continue };
            let channels = d.supported_output_configs().map(|c| c.map(|r| r.channels()).max().unwrap_or(0)).unwrap_or(0);
            if channels >= 2 && !out.iter().any(|x| x.name == n) {
                out.push(AudioDevice { name: n, channels });
            }
        }
        out
    }

    /// The config at `pps` if the device has it (one point per sample),
    /// else its rate nearest above (or below) `pps`, f32 preferred.
    fn pick(device: &cpal::Device, needed: u16, pps: u32) -> Result<cpal::SupportedStreamConfig, String> {
        let ranges: Vec<_> = device.supported_output_configs().map_err(|e| e.to_string())?.filter(|r| r.channels() >= needed).collect();
        let best = ranges
            .iter()
            .map(|r| {
                let rate = pps.clamp(r.min_sample_rate(), r.max_sample_rate());
                // Exact first, then above pps (no point lost), then f32.
                let key = (rate != pps, rate < pps, rate.abs_diff(pps), r.sample_format() != cpal::SampleFormat::F32, r.channels());
                (key, (*r).with_sample_rate(rate))
            })
            .min_by_key(|(k, _)| *k)
            .map(|(_, c)| c);
        best.ok_or_else(|| format!("l'interface n'a pas {needed} canaux de sortie"))
    }

    fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(device: &cpal::Device, config: cpal::StreamConfig, mut player: Player, feed: Arc<Feed>, label: String) -> Result<cpal::Stream, cpal::Error> {
        let mut buf: Vec<f32> = vec![0.0; 16_384];
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Sized once; a bigger buffer than expected goes out in parts.
                for chunk in data.chunks_mut(buf.len()) {
                    let tmp = &mut buf[..chunk.len()];
                    player.render(tmp);
                    for (o, v) in chunk.iter_mut().zip(tmp.iter()) {
                        *o = T::from_sample(*v);
                    }
                }
            },
            move |e: cpal::Error| match e.kind() {
                cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied => {}
                _ => feed.fail(format!("interface audio « {label} » perdue : {e}")),
            },
            None,
        )
    }

    type Opened = Result<(u32, u16), String>;

    /// Opens the stream, reports, then keeps it until `stop`.
    pub fn run(want: &str, map: ChannelMap, mode: Mode, pps: u32, feed: Arc<Feed>, stop: Arc<AtomicBool>, tx: Sender<Opened>) {
        let stream = match open(want, map, mode, pps, &feed) {
            Ok((stream, rate, channels)) => {
                let _ = tx.send(Ok((rate, channels)));
                stream
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        while !stop.load(Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(250));
            // A device the system removed without an error callback.
            if !cpal::default_host().output_devices().map(|mut l| l.any(|d| name(&d).as_deref() == Some(want))).unwrap_or(true) {
                feed.fail(format!("interface audio « {want} » débranchée"));
            }
        }
        drop(stream);
    }

    fn open(want: &str, map: ChannelMap, mode: Mode, pps: u32, feed: &Arc<Feed>) -> Result<(cpal::Stream, u32, u16), String> {
        let host = cpal::default_host();
        if host.default_output_device().and_then(|d| name(&d)).as_deref() == Some(want) {
            return Err(format!("« {want} » est la sortie son du Mac (musique, alertes) : choisissez une autre sortie son dans Réglages système, ou une autre interface"));
        }
        let device = host.output_devices().map_err(|e| e.to_string())?.find(|d| name(d).as_deref() == Some(want)).ok_or_else(|| format!("interface audio « {want} » introuvable"))?;
        let supported = pick(&device, map.needed(mode), pps)?;
        let mut config = supported.config();
        config.buffer_size = cpal::BufferSize::Default;
        let (rate, channels) = (config.sample_rate, config.channels);
        let player = Player::new(Arc::clone(feed), map, mode, pps, rate, channels);
        let label = want.to_string();
        let built = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, player, Arc::clone(feed), label),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, player, Arc::clone(feed), label),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, player, Arc::clone(feed), label),
            other => return Err(format!("format d'échantillon non géré : {other}")),
        };
        let stream = built.map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, rate, channels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::EStop;
    use crate::output::OutputStage;

    fn lit() -> Vec<Point> {
        vec![Point::lit(-0.5, -0.5, 1.0, 0.5, 0.25), Point::lit(0.5, -0.5, 1.0, 0.5, 0.25), Point::lit(0.5, 0.5, 1.0, 0.5, 0.25), Point::lit(-0.5, -0.5, 1.0, 0.5, 0.25)]
    }

    fn out(mode: Mode) -> AudioOutput {
        AudioOutput::fake("fake", ChannelMap::default(), mode, 48_000, 8).unwrap()
    }

    /// `n` sample frames of 8 channels.
    fn play(p: &mut Player, n: usize) -> Vec<[f32; 8]> {
        let mut buf = vec![0.5f32; n * 8];
        p.render(&mut buf);
        buf.chunks(8).map(|c| c.try_into().unwrap()).collect()
    }

    fn silent(frames: &[[f32; 8]]) -> bool {
        frames.iter().all(|f| f.iter().all(|&v| v == 0.0))
    }

    #[test]
    fn disarmed_every_channel_is_zero() {
        let mut o = out(Mode::Laser);
        let mut p = o.player();
        o.send(&lit());
        assert!(silent(&play(&mut p, 2000)));
        // Through the output stage too: it starts disarmed.
        let o = out(Mode::Laser);
        let mut p = o.player();
        let mut stage = OutputStage::new(Some(Box::new(o)));
        stage.emit(&lit(), false, true, &EStop::default());
        assert!(silent(&play(&mut p, 2000)));
    }

    #[test]
    fn armed_x_y_stay_in_range_and_the_frame_is_drawn() {
        let mut o = out(Mode::Laser);
        let mut p = o.player();
        o.set_armed(true).unwrap();
        o.send(&[Point::lit(3.0, -7.0, 2.0, -1.0, f32::NAN), Point::lit(f32::NAN, 0.25, 1.0, 1.0, 1.0)]);
        let got = play(&mut p, 4000);
        assert!(got.iter().all(|f| (-1.0..=1.0).contains(&f[0]) && (-1.0..=1.0).contains(&f[1])));
        assert!(got.iter().all(|f| (0.0..=1.0).contains(&f[2]) && (0.0..=1.0).contains(&f[3]) && (0.0..=1.0).contains(&f[4])));
        assert!(got.iter().any(|f| f[0] == 1.0 && f[1] == -1.0 && f[2] == 1.0 && f[3] == 0.0 && f[4] == 0.0), "clamped point drawn");
        assert!(got.iter().any(|f| f[0] == 0.0 && f[1] == 0.25 && f[2] == 1.0), "NaN goes to 0");
        assert!(got.iter().all(|f| f[5..].iter().all(|&v| v == 0.0)), "unassigned channels stay at 0");
    }

    #[test]
    fn scope_mode_never_sends_colour() {
        let mut o = out(Mode::Scope);
        let mut p = o.player();
        o.set_armed(true).unwrap();
        o.send(&lit());
        let got = play(&mut p, 4000);
        assert!(got.iter().any(|f| f[0] != 0.0 || f[1] != 0.0), "X/Y drawn");
        assert!(got.iter().all(|f| f[2..].iter().all(|&v| v == 0.0)));
        assert_eq!(to_sample(&Point::lit(0.1, 0.2, 1.0, 1.0, 1.0), Mode::Scope), Sample { x: 0.1, y: 0.2, ..Default::default() });
    }

    #[test]
    fn blackout_kill_and_disarm_give_zeros_from_the_next_buffer() {
        let mut o = out(Mode::Laser);
        let mut p = o.player();
        o.set_armed(true).unwrap();
        o.send(&lit());
        assert!(play(&mut p, 500).iter().any(|f| f[2] > 0.0));
        o.blank_now().unwrap();
        assert!(silent(&play(&mut p, 500)), "blank: zeros at once, mid-frame");
        o.send(&lit());
        assert!(play(&mut p, 2000).iter().any(|f| f[2] > 0.0));
        (o.kill_switch().unwrap())();
        assert!(silent(&play(&mut p, 2000)), "kill switch");
        o.set_armed(true).unwrap();
        assert!(play(&mut p, 2000).iter().any(|f| f[2] > 0.0));
        o.set_armed(false).unwrap();
        assert!(silent(&play(&mut p, 2000)), "disarm");
        // Shutdown: the stage disarms and blanks before closing.
        o.set_armed(true).unwrap();
        let feed = Arc::clone(&o.feed);
        let mut stage = OutputStage::new(Some(Box::new(o)));
        stage.shutdown();
        assert!(!feed.armed.load(Ordering::SeqCst));
        assert!(silent(&play(&mut p, 2000)));
    }

    #[test]
    fn a_new_frame_waits_for_the_end_of_the_current_one_and_travels_dark() {
        let mut o = out(Mode::Laser);
        let mut p = o.player();
        o.set_armed(true).unwrap();
        let a: Vec<Point> = (0..100).map(|i| Point::lit(-0.9 + i as f32 * 0.001, -0.9, 1.0, 0.0, 0.0)).collect();
        o.send(&a);
        let first = play(&mut p, 50);
        o.send(&[Point::lit(0.9, 0.9, 0.0, 1.0, 0.0), Point::lit(0.8, 0.9, 0.0, 1.0, 0.0)]);
        let rest = play(&mut p, 400);
        let all: Vec<_> = first.into_iter().chain(rest).collect();
        let b_at = all.iter().position(|f| f[3] > 0.0).expect("frame B drawn");
        let last_a = all[..b_at].iter().rposition(|f| f[2] > 0.0).unwrap();
        // All of A's frame played before B: its last points are at x ≈ -0.801.
        assert!((all[last_a][0] - (-0.9 + 99.0 * 0.001)).abs() < 1e-4);
        let gap = &all[last_a + 1..b_at];
        assert!(gap.len() > 10 && gap.iter().all(|f| f[2..5].iter().all(|&v| v == 0.0)), "dark travel between frames");
        let steepest = all.windows(2).map(|w| (w[1][0] - w[0][0]).abs().max((w[1][1] - w[0][1]).abs())).fold(0.0, f32::max);
        assert!(steepest < 0.1, "no jump across the field: {steepest}");
    }

    #[test]
    fn a_lost_device_is_a_fault_that_disarms_through_the_engine() {
        // The engine checks the stage's fault every frame and disarms with
        // « Sortie perdue » (T-401); the reason is the notification.
        let o = out(Mode::Scope);
        let feed = Arc::clone(&o.feed);
        let stage = OutputStage::new(Some(Box::new(o)));
        assert!(stage.fault().is_none());
        feed.fail("interface audio « fake » perdue".into());
        assert!(stage.fault().unwrap().contains("perdue"));
    }

    #[test]
    fn laser_mode_needs_the_confirmation_and_is_never_saved() {
        let dir = std::env::temp_dir().join(format!("audio-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audio_output.json");
        let store = AudioOutStore::load(path.clone());
        assert_eq!(store.mode(), Mode::Scope);
        let cfg = AudioOutConfig { device: Some(" MOTU ".into()), ..Default::default() };
        assert!(store.set(cfg.clone(), Mode::Laser, false).unwrap_err().to_string().contains("confirmation"));
        assert_eq!(store.mode(), Mode::Scope);
        store.set(cfg, Mode::Laser, true).unwrap();
        assert_eq!((store.mode(), store.get().device.as_deref()), (Mode::Laser, Some("MOTU")));
        assert!(store.entry().unwrap().name.contains("laser"));
        let again = AudioOutStore::load(path);
        assert_eq!((again.mode(), again.get().device.as_deref()), (Mode::Scope, Some("MOTU")), "every launch starts in oscilloscope mode");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn channels_are_checked() {
        let bad = |channels| AudioOutConfig { device: Some("x".into()), channels }.validate().is_err();
        assert!(bad(ChannelMap { x: 1, y: 1, ..Default::default() }));
        assert!(bad(ChannelMap { x: 0, ..Default::default() }));
        assert!(bad(ChannelMap { b: MAX_CHANNEL + 1, ..Default::default() }));
        assert!(!bad(ChannelMap { x: 7, y: 8, r: 1, g: 2, b: 3 }));
        assert!(AudioOutput::fake("s", ChannelMap::default(), Mode::Laser, 30_000, 2).is_err(), "5 channels needed");
        assert!(AudioOutput::fake("s", ChannelMap::default(), Mode::Scope, 30_000, 2).is_ok());
    }

    #[test]
    fn no_test_opens_a_real_interface() {
        assert!(AudioOutput::open("Built-in Output", ChannelMap::default(), Mode::Scope, 30_000).is_err());
        let store = AudioOutStore::in_memory();
        assert!(store.is_fake());
        assert_eq!(store.devices().len(), FAKE_DEVICES.len());
        assert!(store.open("audio:Built-in Output", 30_000).is_err(), "unknown to the fake list");
        assert!(store.open(&format!("audio:{}", FAKE_DEVICES[0].0), 30_000).is_ok());
    }

    #[test]
    fn the_analysis_input_is_refused() {
        assert!(clashes_with_input(Some("Scarlett 2i2 USB"), Some(" scarlett 2i2 usb")));
        assert!(!clashes_with_input(Some("MOTU M4"), Some("Scarlett 2i2 USB")));
        assert!(!clashes_with_input(Some("MOTU M4"), None));
    }

    #[test]
    fn a_device_slower_than_pps_steps_through_the_frame() {
        let feed = Arc::new(Feed::default());
        let mut p = Player::new(Arc::clone(&feed), ChannelMap::default(), Mode::Scope, 96_000, 48_000, 2);
        feed.armed.store(true, Ordering::SeqCst);
        {
            let mut s = lock(&feed.slot);
            s.frame = (0..10).map(|i| Sample { x: i as f32 / 10.0, ..Default::default() }).collect();
            s.fresh = true;
        }
        let mut buf = vec![0.0f32; 2 * 200];
        p.render(&mut buf);
        let xs: Vec<f32> = buf.chunks(2).map(|c| c[0]).collect();
        // Frame start reached after the travel from the centre: 0.0, 0.2, 0.4...
        let start = xs.windows(3).position(|w| w[1] == 0.2 && w[2] == 0.4).unwrap();
        assert_eq!(&xs[start..start + 5], &[0.0, 0.2, 0.4, 0.6, 0.8]);
    }
}
