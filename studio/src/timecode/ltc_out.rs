//! LTC out (T-501): the timeline's position as linear timecode (SMPTE 12M)
//! on one channel of an audio output, for the consoles, video servers and
//! recorders that only take LTC on an audio / XLR input.
//!
//! The third route of the timecode output (out.rs): same rules as MTC and
//! Art-Net (off by default, per machine, the timeline only read, the
//! self-drive trap refused, nothing sent while paused, stopped, blocked or
//! after an e-stop). It never arms the laser and never touches the output.
//!
//! The signal: bi-phase mark, 80 bits a frame sent over the frame's
//! duration, the bits of ltc.rs (`frame_bits`, the decoder's own layout, so
//! the decoder is the tests' reference), at the level the operator set
//! (-18 dBFS by default), edges softened to a few tens of µs like a real
//! LTC generator. Frames are always sent whole: a seek or a loop wrap
//! starts a new frame at the next frame boundary, never in the middle of
//! one.
//!
//! Threads: the "tc-out" thread opens the device (it owns the stream) and
//! publishes a `Target` (where the timecode is at which studio time) into
//! the `LtcLink`; the audio callback (`LtcGen::render`) reads it with
//! `try_lock` only, allocates nothing and never waits. It follows the
//! timeline: each frame's length is nudged (±1 %) so the sound card's
//! crystal does not drift away from the playhead; more than a frame and a
//! half off (a seek), it jumps.

use super::ltc::frame_bits;
use super::{Rate, Tc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const DEFAULT_LEVEL_DB: f32 = -18.0;
pub const MIN_LEVEL_DB: f32 = -40.0;
pub const MAX_LEVEL_DB: f32 = 0.0;
pub const MAX_CHANNEL: u16 = 64;
/// Below this the half bits of 30 im/s are under two samples.
pub const MIN_SAMPLE_RATE: u32 = 32_000;
/// Edge rise time constant (SMPTE 12M asks for 25 to 50 µs, 10–90 %).
const RISE_S: f64 = 15e-6;
/// Largest correction of a frame's length to follow the playhead.
const MAX_PULL: f64 = 0.01;
/// Further off than this (in frames), the next frame jumps to the playhead.
const JUMP_FRAMES: f64 = 1.5;

pub const SELF_DRIVE_LTC: &str = "Entrée et sortie de timecode sur la même voie audio : le studio se piloterait lui-même.";
/// The simulated device of `--test-ltc-out` (no real audio device).
pub const SIM_DEVICE: &str = "Test LTC";

/// `timecode.json`, key `out.ltc`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LtcOut {
    /// The audio output, by name.
    pub device: String,
    /// The channel of that output (1-based).
    pub channel: u16,
    /// Peak level, dBFS.
    pub level_db: f32,
}

impl Default for LtcOut {
    fn default() -> Self {
        LtcOut { device: String::new(), channel: 1, level_db: DEFAULT_LEVEL_DB }
    }
}

impl LtcOut {
    pub fn validate(&self) -> Result<(), String> {
        let d = self.device.trim();
        if d.is_empty() || d.len() > 128 {
            return Err("sortie audio invalide".into());
        }
        if !(1..=MAX_CHANNEL).contains(&self.channel) {
            return Err(format!("voie LTC : de 1 à {MAX_CHANNEL}"));
        }
        if !self.level_db.is_finite() || !(MIN_LEVEL_DB..=MAX_LEVEL_DB).contains(&self.level_db) {
            return Err(format!("niveau LTC : de {MIN_LEVEL_DB} à {MAX_LEVEL_DB} dBFS"));
        }
        Ok(())
    }

    /// The peak amplitude (0..1).
    pub fn amplitude(&self) -> f32 {
        10f32.powf(self.level_db.clamp(MIN_LEVEL_DB, MAX_LEVEL_DB) / 20.0)
    }
}

/// Where the timecode is: `secs` (timecode seconds, offset included) at
/// studio time `at_s` (seconds since the link's epoch), running on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// Changes on every seek, start or loop wrap (`Player::jumps`).
    pub gen: u64,
    pub secs: f64,
    pub at_s: f64,
    pub rate: Rate,
    pub amp: f32,
}

impl Target {
    /// Timecode seconds heard at studio time `t`.
    pub fn secs_at(&self, t: f64) -> f64 {
        self.secs + (t - self.at_s)
    }
}

/// Shared by the "tc-out" thread (writes the target) and the callback.
pub struct LtcLink {
    epoch: Instant,
    /// None: silence.
    target: Mutex<Option<Target>>,
    /// Frames started by the callback.
    pub frames: AtomicU64,
    /// The stream reported an error: reopened by the thread.
    pub failed: AtomicBool,
    /// What the simulated device decoded last (`--test-ltc-out` only).
    pub heard: Mutex<Option<Tc>>,
}

impl LtcLink {
    pub fn new(epoch: Instant) -> Self {
        LtcLink { epoch, target: Mutex::new(None), frames: AtomicU64::new(0), failed: AtomicBool::new(false), heard: Mutex::new(None) }
    }

    /// Studio seconds of `at` on this link's clock (negative before it).
    pub fn secs_of(&self, at: Instant) -> f64 {
        match at.checked_duration_since(self.epoch) {
            Some(d) => d.as_secs_f64(),
            None => -self.epoch.duration_since(at).as_secs_f64(),
        }
    }

    pub fn now_s(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    pub fn set_target(&self, t: Option<Target>) {
        *self.target.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }
}

/// The LTC generator of one output stream (runs in the audio callback).
pub struct LtcGen {
    sample_rate: f64,
    channels: usize,
    /// 0-based.
    channel: usize,
    alpha: f32,
    target: Option<Target>,
    running: bool,
    gen: u64,
    rate: Rate,
    frame: i64,
    bits: [bool; 80],
    /// This frame's length and the position in it, in samples.
    frame_len: f64,
    pos: f64,
    /// The last half-bit cell (0..160) drawn; -1 at a frame's start.
    cell: i32,
    level: f32,
    smooth: f32,
}

impl LtcGen {
    pub fn new(sample_rate: u32, channels: u16, channel: u16) -> Self {
        let sr = sample_rate.max(1) as f64;
        LtcGen {
            sample_rate: sr,
            channels: channels.max(1) as usize,
            channel: (channel as usize).min(channels.max(1) as usize - 1),
            alpha: (1.0 - (-1.0 / (sr * RISE_S)).exp()) as f32,
            target: None,
            running: false,
            gen: 0,
            rate: Rate::F25,
            frame: 0,
            bits: [false; 80],
            frame_len: sr / 25.0,
            pos: 0.0,
            cell: -1,
            level: 1.0,
            smooth: 0.0,
        }
    }

    /// The frame being sent (None: silent).
    #[cfg(test)]
    pub fn frame(&self) -> Option<(i64, Rate)> {
        self.running.then_some((self.frame, self.rate))
    }

    /// Fills `out` (interleaved): LTC on the chosen channel, silence on the
    /// others. `now_s` is the studio time of the callback, `latency_s` until
    /// its first sample is heard. Never allocates, never waits.
    pub fn render(&mut self, out: &mut [f32], now_s: f64, latency_s: f64, link: &LtcLink) {
        if let Ok(t) = link.target.try_lock() {
            self.target = *t;
        }
        if self.target.is_none() {
            // Stopped, paused, disabled, e-stop: silent at once.
            self.running = false;
        }
        let start = now_s + latency_s;
        for (i, slot) in out.chunks_mut(self.channels).enumerate() {
            if self.target.is_some() && (!self.running || self.pos >= self.frame_len) {
                self.next_frame(start + i as f64 / self.sample_rate, link);
            }
            let v = if self.running { self.sample() } else { 0.0 };
            self.smooth += self.alpha * (v - self.smooth);
            for (c, x) in slot.iter_mut().enumerate() {
                *x = if c == self.channel { self.smooth } else { 0.0 };
            }
        }
    }

    /// One sample of the current frame: a flip at every bit start and in
    /// the middle of a `1`.
    fn sample(&mut self) -> f32 {
        let cell = ((self.pos * 160.0 / self.frame_len) as i32).min(159);
        while self.cell < cell {
            self.cell += 1;
            let c = self.cell as usize;
            if c.is_multiple_of(2) || self.bits[c / 2] {
                self.level = -self.level;
            }
        }
        self.pos += 1.0;
        self.level * self.target.map_or(0.0, |t| t.amp)
    }

    /// Starts the frame heard at studio time `heard`.
    fn next_frame(&mut self, heard: f64, link: &LtcLink) {
        let Some(t) = self.target else { return };
        let fps = t.rate.fps();
        let want = t.secs_at(heard) * fps;
        let follow = self.running && self.gen == t.gen && self.rate == t.rate;
        let next = if follow && (want - (self.frame + 1) as f64).abs() <= JUMP_FRAMES { self.frame + 1 } else { want.round() as i64 };
        // Ahead of us (error > 0): a shorter frame catches up.
        let err = want - next as f64;
        let len = self.sample_rate / fps * (1.0 - (0.1 * err).clamp(-MAX_PULL, MAX_PULL));
        self.pos = if follow { (self.pos - self.frame_len).max(0.0) } else { 0.0 };
        (self.frame, self.rate, self.gen, self.frame_len, self.cell, self.running) = (next, t.rate, t.gen, len, -1, true);
        self.bits = frame_bits(Tc::from_frames(next, t.rate), t.rate);
        link.frames.fetch_add(1, Ordering::Relaxed);
    }
}

/// An audio output the LTC can go to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LtcDevice {
    pub name: String,
    pub channels: u16,
}

/// An open stream: it plays as long as `stream` lives (dropped on the
/// thread that opened it: a cpal stream is not `Send` everywhere).
pub struct OpenedLtc {
    #[allow(dead_code)]
    pub stream: Box<dyn std::any::Any>,
    pub device: String,
    pub sample_rate: u32,
}

/// Where LTC goes: Core Audio, or the simulated device (tests, e2e).
pub trait LtcBackend {
    fn outputs(&mut self) -> Result<Vec<LtcDevice>, String>;
    fn open(&mut self, cfg: &LtcOut, link: &Arc<LtcLink>) -> Result<OpenedLtc, String>;
}

/// Builds the backend on the "tc-out" thread.
pub type LtcFactory = Box<dyn FnOnce() -> Box<dyn LtcBackend> + Send>;

fn check_channel(cfg: &LtcOut, channels: u16, sample_rate: u32) -> Result<(), String> {
    if cfg.channel == 0 || cfg.channel > channels {
        return Err(format!("voie {} absente : « {} » a {channels} voie(s)", cfg.channel, cfg.device));
    }
    if sample_rate < MIN_SAMPLE_RATE {
        return Err(format!("fréquence trop basse pour le LTC : {sample_rate} Hz"));
    }
    Ok(())
}

/// Core Audio through cpal: the device chosen by name, never the default
/// one implicitly.
pub struct CpalLtc {
    host: cpal::Host,
}

impl CpalLtc {
    pub fn new() -> Self {
        CpalLtc { host: cpal::default_host() }
    }

    pub fn factory() -> LtcFactory {
        Box::new(|| Box::new(CpalLtc::new()))
    }
}

impl LtcBackend for CpalLtc {
    fn outputs(&mut self) -> Result<Vec<LtcDevice>, String> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let mut list: Vec<LtcDevice> = Vec::new();
        for d in self.host.output_devices().map_err(|e| e.to_string())? {
            let Some(name) = d.description().ok().map(|x| x.name().to_string()) else { continue };
            let Ok(cfg) = d.default_output_config() else { continue };
            if !list.iter().any(|x| x.name == name) {
                list.push(LtcDevice { name, channels: cfg.channels() });
            }
        }
        Ok(list)
    }

    fn open(&mut self, cfg: &LtcOut, link: &Arc<LtcLink>) -> Result<OpenedLtc, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = self
            .host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.description().ok().is_some_and(|x| x.name() == cfg.device))
            .ok_or_else(|| format!("sortie audio introuvable : {}", cfg.device))?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("format de sortie non géré : {}", supported.sample_format()));
        }
        let config = supported.config();
        check_channel(cfg, config.channels, config.sample_rate)?;
        let mut gen = LtcGen::new(config.sample_rate, config.channels, cfg.channel - 1);
        let (l, failed) = (Arc::clone(link), Arc::clone(link));
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    let latency = ts.playback.duration_since(ts.callback).as_secs_f64();
                    gen.render(data, l.now_s(), latency, &l);
                },
                move |e: cpal::Error| {
                    if !matches!(e.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
                        failed.failed.store(true, Ordering::Relaxed);
                    }
                },
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(OpenedLtc { stream: Box::new(stream), device: cfg.device.clone(), sample_rate: config.sample_rate })
    }
}

/// The simulated output of `--test-ltc-out` and the tests: one stereo
/// device « Test LTC » at 48 kHz whose "sound card" is a thread rendering
/// 10 ms blocks on the system clock into our own decoder (`LtcLink::heard`).
/// No audio device is ever touched.
pub struct SimLtc;

impl SimLtc {
    pub fn factory() -> LtcFactory {
        Box::new(|| Box::new(SimLtc))
    }
}

/// Stops the simulated card's thread when dropped (the stream closes).
struct SimStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SimStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl LtcBackend for SimLtc {
    fn outputs(&mut self) -> Result<Vec<LtcDevice>, String> {
        Ok(vec![LtcDevice { name: SIM_DEVICE.into(), channels: 2 }])
    }

    fn open(&mut self, cfg: &LtcOut, link: &Arc<LtcLink>) -> Result<OpenedLtc, String> {
        if cfg.device != SIM_DEVICE {
            return Err(format!("sortie audio introuvable : {}", cfg.device));
        }
        const RATE: u32 = 48_000;
        const BLOCK: usize = 480;
        check_channel(cfg, 2, RATE)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (link, halt, ch) = (Arc::clone(link), Arc::clone(&stop), (cfg.channel - 1) as usize);
        let thread = std::thread::Builder::new()
            .name("ltc-sim".into())
            .spawn(move || {
                let mut gen = LtcGen::new(RATE, 2, ch as u16);
                let mut dec = super::ltc::LtcDecoder::new(RATE);
                let (mut buf, mut mono) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK]);
                let t0 = Instant::now();
                let mut blocks = 0u64;
                while !halt.load(Ordering::SeqCst) {
                    gen.render(&mut buf, link.now_s(), 0.0, &link);
                    for (m, s) in mono.iter_mut().zip(buf.chunks(2)) {
                        *m = s[ch];
                    }
                    dec.feed(&mono, |f| *link.heard.lock().unwrap_or_else(|e| e.into_inner()) = Some(f.tc));
                    blocks += 1;
                    let due = t0 + std::time::Duration::from_millis(10 * blocks);
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(OpenedLtc { stream: Box::new(SimStream { stop, thread: Some(thread) }), device: SIM_DEVICE.into(), sample_rate: RATE })
    }
}

#[cfg(test)]
mod tests {
    use super::super::ltc::{LtcDecoder, LtcFrame};
    use super::*;

    const SR: u32 = 48_000;

    fn target(secs: f64, rate: Rate) -> Target {
        Target { gen: 1, secs, at_s: 0.0, rate, amp: LtcOut::default().amplitude() }
    }

    /// Renders `secs` of the output (mono, 512-sample callbacks, no latency)
    /// and decodes it with the input's decoder.
    fn round_trip(gen: &mut LtcGen, link: &LtcLink, from_s: f64, secs: f64, dec: &mut LtcDecoder, got: &mut Vec<LtcFrame>) {
        let mut buf = vec![0.0f32; 512];
        let blocks = (secs * SR as f64 / 512.0) as u64;
        let first = (from_s * SR as f64 / 512.0).round() as u64;
        for b in first..first + blocks {
            gen.render(&mut buf, (b * 512) as f64 / SR as f64, 0.0, link);
            dec.feed(&buf, |f| got.push(f));
        }
    }

    /// Ten simulated minutes at every rate through our encoder then the
    /// existing decoder: every frame read back, in order, none wrong or
    /// missing, each one where the playhead was when it was sent.
    #[test]
    fn ten_minutes_round_trip_at_every_rate() {
        for rate in Rate::ALL {
            let start = 3_000.0 + 0.37; // crosses 00:50:00 → 01:00:00 → …
            let link = LtcLink::new(Instant::now());
            link.set_target(Some(target(start, rate)));
            let mut gen = LtcGen::new(SR, 1, 0);
            let (mut dec, mut got) = (LtcDecoder::new(SR), Vec::new());
            round_trip(&mut gen, &link, 0.0, 600.0, &mut dec, &mut got);
            let frames = 600.0 * rate.fps();
            assert!(got.len() as f64 >= frames - 3.0, "{rate:?}: {} of {frames}", got.len());
            assert!(got.windows(2).all(|w| w[1].tc.frames(rate) == w[0].tc.frames(rate) + 1), "{rate:?}: a frame lost or wrong");
            for f in &got {
                assert!(f.tc.valid(rate), "{rate:?} {}", f.tc);
                assert_eq!(f.drop_frame, rate.drop_frame());
                // The frame ends one frame after its own time.
                let heard = (start + f.end_sample as f64 / SR as f64) * rate.fps();
                assert!((heard - (f.tc.frames(rate) + 1) as f64).abs() <= 1.0, "{rate:?} {} at {heard}", f.tc);
            }
            let last = got.last().unwrap();
            assert_eq!(Rate::from_ltc(last.fps, last.drop_frame), rate, "{rate:?} measured {}", last.fps);
            assert!(link.frames.load(Ordering::Relaxed) as f64 >= frames);
        }
    }

    /// 29,97 drop-frame: frames 0 and 1 are skipped at every minute but the
    /// tenth ones, and nothing else.
    #[test]
    fn drop_frame_skips_the_right_labels() {
        let rate = Rate::F2997Df;
        let link = LtcLink::new(Instant::now());
        link.set_target(Some(target(Tc { h: 0, m: 8, s: 58, f: 0 }.seconds(rate), rate)));
        let mut gen = LtcGen::new(SR, 1, 0);
        let (mut dec, mut got) = (LtcDecoder::new(SR), Vec::new());
        round_trip(&mut gen, &link, 0.0, 130.0, &mut dec, &mut got);
        let labels: Vec<Tc> = got.iter().map(|f| f.tc).collect();
        let at = |m, s, f| labels.iter().position(|t| *t == Tc { h: 0, m, s, f });
        // 00:08:59;29 → 00:09:00;02, 00:09:59;29 → 00:10:00;00 → 00:10:00;01,
        // 00:10:59;29 → 00:11:00;02.
        for (before, after) in [((8, 59, 29), (9, 0, 2)), ((9, 59, 29), (10, 0, 0)), ((10, 0, 0), (10, 0, 1)), ((10, 59, 29), (11, 0, 2))] {
            let i = at(before.0, before.1, before.2).unwrap_or_else(|| panic!("{before:?} missing"));
            assert_eq!(labels[i + 1], Tc { h: 0, m: after.0, s: after.1, f: after.2 }, "after {before:?}");
        }
        assert!(at(9, 0, 0).is_none() && at(9, 0, 1).is_none() && at(11, 0, 0).is_none());
        assert!(labels.iter().all(|t| t.valid(rate)));
    }

    #[test]
    fn every_frame_has_an_even_number_of_ones() {
        for rate in Rate::ALL {
            for n in [0i64, 1, 1_799, 17_982, 107_892, 2_000_000] {
                let bits = frame_bits(Tc::from_frames(n, rate), rate);
                assert_eq!(bits.iter().filter(|&&b| b).count() % 2, 0, "{rate:?} {n}");
            }
        }
    }

    /// A seek starts a whole new frame at the new time; a stop is silence
    /// at once; the level and the channel are the ones chosen.
    #[test]
    fn seek_stop_level_and_channel() {
        let link = LtcLink::new(Instant::now());
        let mut t = target(10.0, Rate::F25);
        link.set_target(Some(t));
        let mut gen = LtcGen::new(SR, 4, 2);
        let (mut dec, mut got) = (LtcDecoder::new(SR), Vec::new());
        let mut buf = vec![0.0f32; 512 * 4];
        let mut mono = vec![0.0f32; 512];
        let mut now = 0.0;
        let mut run = |gen: &mut LtcGen, secs: f64, now: &mut f64, got: &mut Vec<LtcFrame>, peak: &mut f32| {
            for _ in 0..(secs * SR as f64 / 512.0) as usize {
                gen.render(&mut buf, *now, 0.0, &link);
                for (m, s) in mono.iter_mut().zip(buf.chunks(4)) {
                    assert_eq!((s[0], s[1], s[3]), (0.0, 0.0, 0.0), "other channels stay silent");
                    *m = s[2];
                    *peak = peak.max(s[2].abs());
                }
                dec.feed(&mono, |f| got.push(f));
                *now += 512.0 / SR as f64;
            }
        };
        let mut peak = 0.0f32;
        run(&mut gen, 2.0, &mut now, &mut got, &mut peak);
        // -18 dBFS peak.
        assert!((peak - 0.1259).abs() < 0.005, "{peak}");
        assert!(got.iter().any(|f| f.tc == Tc { h: 0, m: 0, s: 11, f: 0 }));
        // Seek to 01:00:00:00 (a new gen).
        t = Target { gen: 2, secs: 3600.0, at_s: now, ..t };
        link.set_target(Some(t));
        got.clear();
        run(&mut gen, 1.0, &mut now, &mut got, &mut peak);
        // At most the frame under way finishes, then whole frames from 01:00:00:00.
        let before = got.iter().take_while(|f| f.tc.h == 0).count();
        assert!(before <= 1 && got[before..].iter().all(|f| f.tc.h == 1), "{:?}", got.iter().map(|f| f.tc.to_string()).collect::<Vec<_>>());
        let ones: Vec<_> = got.iter().filter(|f| f.tc.h == 1).collect();
        assert!(ones.len() >= 23 && ones[0].tc.frames(Rate::F25) <= Tc { h: 1, m: 0, s: 0, f: 1 }.frames(Rate::F25));
        // Stop: silence within a block.
        link.set_target(None);
        let mut quiet = 0.0f32;
        gen.render(&mut buf, now, 0.0, &link);
        gen.render(&mut buf, now, 0.0, &link);
        for s in buf.chunks(4) {
            quiet = quiet.max(s[2].abs());
        }
        assert!(quiet < 1e-6, "{quiet}");
        assert!(gen.frame().is_none());
    }

    /// The sound card runs 0.05 % fast: the frames stay on the playhead.
    #[test]
    fn a_fast_sound_card_stays_on_the_playhead() {
        let rate = Rate::F30;
        let link = LtcLink::new(Instant::now());
        link.set_target(Some(target(0.0, rate)));
        let mut gen = LtcGen::new(SR, 1, 0);
        let mut buf = vec![0.0f32; 512];
        let mut worst = 0.0f64;
        for b in 0..(120.0 * SR as f64 / 512.0) as u64 {
            // The card plays 1.0005 s of samples per studio second.
            let now = (b * 512) as f64 / (SR as f64 * 1.0005);
            gen.render(&mut buf, now, 0.0, &link);
            if b > 100 {
                let (f, _) = gen.frame().unwrap();
                worst = worst.max((f as f64 - now * rate.fps()).abs());
            }
        }
        assert!(worst <= 1.5, "{worst}");
    }

    #[test]
    fn render_does_not_allocate() {
        let link = LtcLink::new(Instant::now());
        link.set_target(Some(target(5.0, Rate::F2997Df)));
        let mut gen = LtcGen::new(SR, 2, 1);
        let mut buf = vec![0.0f32; 1024];
        let allocs = crate::audio::capture::tests::allocations_during(|| {
            for k in 0..200 {
                gen.render(&mut buf, k as f64 * 512.0 / SR as f64, 0.003, &link);
            }
        });
        assert_eq!(allocs, 0);
        assert!(link.frames.load(Ordering::Relaxed) > 50);
    }

    #[test]
    fn settings_are_checked() {
        let ok = LtcOut { device: "Carte".into(), ..Default::default() };
        assert!(ok.validate().is_ok());
        assert!((ok.amplitude() - 0.1259).abs() < 1e-3);
        for bad in [
            LtcOut { device: " ".into(), ..ok.clone() },
            LtcOut { channel: 0, ..ok.clone() },
            LtcOut { channel: MAX_CHANNEL + 1, ..ok.clone() },
            LtcOut { level_db: 3.0, ..ok.clone() },
            LtcOut { level_db: f32::NAN, ..ok.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(check_channel(&LtcOut { channel: 3, ..ok.clone() }, 2, 48_000).is_err());
        assert!(check_channel(&ok, 2, 22_050).is_err());
    }

    /// The simulated card plays and decodes in real time; refuses another name.
    #[test]
    fn the_simulated_device_decodes_what_it_plays() {
        let link = Arc::new(LtcLink::new(Instant::now()));
        let cfg = LtcOut { device: SIM_DEVICE.into(), channel: 2, ..Default::default() };
        assert!(SimLtc.open(&LtcOut { device: "Autre".into(), ..cfg.clone() }, &link).is_err());
        link.set_target(Some(Target { gen: 1, secs: 7200.0, at_s: link.now_s(), rate: Rate::F25, amp: 0.2 }));
        let opened = SimLtc.open(&cfg, &link).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let heard = *link.heard.lock().unwrap();
        assert!(heard.is_some_and(|t| t.h == 2 && t.m == 0), "{heard:?}");
        drop(opened);
    }
}
