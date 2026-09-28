//! The timeline's song, played by the studio itself (T-161).
//!
//! **Where**: natively, on the Mac's default output through cpal
//! (CoreAudio), not in the browser. The show must keep going with the tab
//! hidden or closed, like the native capture (T-230), and only the native
//! side knows exactly which sample the sound card is playing.
//!
//! **Who follows whom**: the song is the clock. The transport commands
//! (play, pause, seek, loop, stop, e-stop) come from the timeline player;
//! each one becomes a `Command` (song position at a studio time). Once the
//! output callback has taken a command, it publishes every buffer which
//! song position is heard when (`ClockSnap`: the position of the buffer's
//! first sample and the time it reaches the speaker, callback time +
//! CoreAudio's reported output latency). The engine (`SongSync`, every
//! frame) pulls the timeline towards that position, so the sound card's
//! crystal, not the system clock, sets the pace: no drift, however long
//! the song. A command is stamped with the studio time it was sent, and
//! the callback starts the song where the playhead is *by the time it is
//! heard*, so neither side jumps when the audio takes over. Until it has
//! (a device opening, a file decoding, `--no-audio`, no output device),
//! the timeline runs on the system clock as before. The remaining,
//! constant latency of the laser path is the show's *Décalage* (±0.5 s).
//!
//! Threads, none waiting on another: the **callback** (`Renderer::render`:
//! no allocation, no blocking, `try_lock` only), the **song-playback**
//! thread (decodes the wanted file, opens and reopens the output), and the
//! engine (`SongSync::sync`, a few short locks). Audio never touches the
//! arm state or the output gate.

use super::decode::Decoded;
use super::media::MediaStore;
use crate::timeline::{Clock, Player};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fade in / out on start, stop and jumps, so they don't click.
const FADE_S: f32 = 0.005;
/// A clock with no callback for this long is not followed any more.
const CLOCK_STALE_S: f64 = 0.5;
/// Retry an output that failed or vanished this often.
const RETRY: Duration = Duration::from_secs(2);

/// Where the song should be: at studio time `at_s`, song position `file_s`
/// (seconds, may be negative = before the song starts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Command {
    /// 0 = nothing asked yet.
    pub gen: u64,
    pub playing: bool,
    pub file_s: f64,
    pub at_s: f64,
    pub gain: f32,
}

/// Published by the callback every buffer: song position `file_s` is heard
/// at studio time `heard_s`, for command `gen`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockSnap {
    pub gen: u64,
    pub playing: bool,
    pub file_s: f64,
    pub heard_s: f64,
}

impl ClockSnap {
    /// The song position heard at studio time `t`.
    pub fn file_s_at(&self, t: f64) -> f64 {
        if self.playing {
            self.file_s + (t - self.heard_s)
        } else {
            self.file_s
        }
    }
}

/// Shared by the engine (commands), the callback (clock) and the thread.
#[derive(Default)]
pub struct Link {
    cmd: Mutex<Command>,
    clock: Mutex<Option<ClockSnap>>,
    /// Set by the stream's error callback: the thread reopens.
    failed: AtomicBool,
}

/// The output callback's state: one per open stream.
pub struct Renderer {
    track: Arc<Decoded>,
    link: Arc<Link>,
    /// Song frames per device frame.
    step: f64,
    rate: f64,
    fade_step: f32,
    /// Song position, in song frames.
    pos: f64,
    gen: u64,
    playing: bool,
    gain: f32,
    fade: f32,
}

impl Renderer {
    pub fn new(track: Arc<Decoded>, link: Arc<Link>, device_rate: u32) -> Self {
        // Touch both locks here, so any lazy set-up of theirs never happens
        // in the callback.
        drop(link.cmd.lock());
        drop(link.clock.lock());
        let rate = track.sample_rate as f64;
        Self {
            step: rate / device_rate.max(1) as f64,
            rate,
            fade_step: 1.0 / (FADE_S * device_rate.max(1) as f32),
            track,
            link,
            pos: 0.0,
            gen: 0,
            playing: false,
            gain: 1.0,
            fade: 0.0,
        }
    }

    /// Song position (seconds) of the next sample to render.
    pub fn position_s(&self) -> f64 {
        self.pos / self.rate
    }

    /// Fill one output buffer (`channels` interleaved). `now_s` is studio
    /// time, `latency_s` how long until this buffer is heard. Real-time
    /// safe: no allocation, no lock wait, no log.
    pub fn render(&mut self, out: &mut [f32], channels: usize, now_s: f64, latency_s: f64) {
        let heard_s = now_s + latency_s.max(0.0);
        // A busy lock just means: take the command next buffer.
        let cmd = self.link.cmd.try_lock().map(|c| *c).ok();
        if let Some(c) = cmd {
            if c.gen != 0 && c.gen != self.gen {
                // Where the playhead will be when this buffer is heard.
                let file_s = if c.playing { c.file_s + (heard_s - c.at_s).max(0.0) } else { c.file_s };
                let pos = file_s * self.rate;
                if (pos - self.pos).abs() > 0.01 * self.rate {
                    self.fade = 0.0; // a jump: fade in from silence
                }
                self.pos = pos;
                self.gen = c.gen;
                self.playing = c.playing;
            }
            self.gain = if c.gain.is_finite() { c.gain.clamp(0.0, 1.0) } else { 0.0 };
        }
        if let Ok(mut clock) = self.link.clock.try_lock() {
            *clock = Some(ClockSnap { gen: self.gen, playing: self.playing, file_s: self.position_s(), heard_s });
        }

        let channels = channels.max(1);
        for frame in out.chunks_mut(channels) {
            self.fade = if self.playing { (self.fade + self.fade_step).min(1.0) } else { (self.fade - self.fade_step).max(0.0) };
            if self.fade <= 0.0 && !self.playing {
                frame.fill(0.0);
                continue;
            }
            let (l, r) = self.sample_at(self.pos);
            let g = self.gain * self.fade;
            match frame.len() {
                1 => frame[0] = 0.5 * (l + r) * g,
                _ => {
                    frame[0] = l * g;
                    frame[1] = r * g;
                    frame[2..].fill(0.0);
                }
            }
            self.pos += self.step;
        }
    }

    /// Linear interpolation between the two nearest song frames.
    #[inline]
    fn sample_at(&self, pos: f64) -> (f32, f32) {
        let i = pos.floor();
        let frac = (pos - i) as f32;
        let i = i as i64;
        let t = &self.track;
        let l = t.sample(i, 0) * (1.0 - frac) + t.sample(i + 1, 0) * frac;
        let r = t.sample(i, 1) * (1.0 - frac) + t.sample(i + 1, 1) * frac;
        (l, r)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SongState {
    /// `--no-audio`: no playback, the timeline runs on the system clock.
    Disabled,
    /// No song wanted.
    #[default]
    Idle,
    Loading,
    /// Decoded and the output is open: the song is the clock.
    Ready,
    /// The file couldn't be read or decoded.
    Error,
    /// No output device (or it failed): retried every 2 s.
    NoDevice,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SongStatus {
    pub state: SongState,
    pub file: Option<String>,
    pub message: String,
    pub device: Option<String>,
    pub sample_rate: u32,
}

/// The meeting point: the engine says which file it wants and sends
/// commands; the thread loads and plays; everyone reads the status.
pub struct SongHub {
    enabled: bool,
    epoch: Instant,
    link: Arc<Link>,
    want: Mutex<Option<String>>,
    status: Mutex<SongStatus>,
}

impl SongHub {
    pub fn new(epoch: Instant, enabled: bool) -> Self {
        let status = SongStatus { state: if enabled { SongState::Idle } else { SongState::Disabled }, ..Default::default() };
        Self { enabled, epoch, link: Arc::new(Link::default()), want: Mutex::new(None), status: Mutex::new(status) }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn status(&self) -> SongStatus {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, f: impl FnOnce(&mut SongStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    fn want(&self, file: Option<&str>) {
        let mut want = self.want.lock().unwrap();
        if want.as_deref() != file {
            *want = file.map(str::to_string);
        }
    }

    fn wanted(&self) -> Option<String> {
        self.want.lock().unwrap().clone()
    }

    fn command(&self, c: Command) {
        *self.link.cmd.lock().unwrap() = c;
    }

    fn set_gain(&self, gain: f32) {
        self.link.cmd.lock().unwrap().gain = gain;
    }

    fn clock(&self) -> Option<ClockSnap> {
        *self.link.clock.lock().unwrap()
    }

    /// The song is loaded and heard: its clock can be followed.
    fn ready_for(&self, file: &str) -> bool {
        let s = self.status.lock().unwrap();
        s.state == SongState::Ready && s.file.as_deref() == Some(file)
    }
}

/// What `SongSync` last told the song, to spot what changed.
#[derive(Clone, Debug, PartialEq)]
struct Sent {
    jumps: u64,
    playing: bool,
    file: String,
    offset_s: f64,
}

/// Which clock the timeline followed on the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    #[default]
    System,
    Audio,
}

/// Engine side, called every frame before the player advances.
#[derive(Default)]
pub struct SongSync {
    gen: u64,
    sent: Option<Sent>,
    pub clock: ClockSource,
}

impl SongSync {
    /// Tell the song what the timeline does, and make the timeline follow
    /// the song once the song plays what was asked.
    pub fn sync(&mut self, hub: &SongHub, player: &mut Player, c: &Clock) {
        self.clock = ClockSource::System;
        let song = player.show.as_ref().and_then(|s| s.song()).cloned();
        hub.want(song.as_ref().map(|a| a.file.as_str()));
        let Some(song) = song else {
            if self.sent.take().is_some() {
                self.gen += 1;
                hub.command(Command { gen: self.gen, playing: false, ..Default::default() });
            }
            return;
        };
        if !hub.enabled() {
            return;
        }
        let playing = player.is_playing();
        let now = Sent { jumps: player.jumps(), playing, file: song.file.clone(), offset_s: song.offset_s };
        if self.sent.as_ref() != Some(&now) {
            self.gen += 1;
            let file_s = player.position_at(c) - song.offset_s;
            hub.command(Command { gen: self.gen, playing, file_s, at_s: c.t, gain: song.gain });
            self.sent = Some(now);
        } else {
            hub.set_gain(song.gain);
        }
        if !playing || !hub.ready_for(&song.file) {
            return;
        }
        if let Some(snap) = hub.clock().filter(|s| s.gen == self.gen && s.playing && c.t - s.heard_s < CLOCK_STALE_S) {
            player.follow(snap.file_s_at(c.t) + song.offset_s, c);
            self.clock = ClockSource::Audio;
        }
    }
}

/// An open output stream. Dropping `stream` closes it (on the thread that
/// opened it: a cpal stream is not `Send` everywhere).
pub struct Opened {
    /// Only held: the stream plays as long as it lives.
    #[allow(dead_code)]
    pub stream: Box<dyn std::any::Any>,
    pub device: String,
    pub sample_rate: u32,
}

/// Where the song goes: CoreAudio, or a fake in tests.
pub trait AudioOut {
    /// Open the default output; `make` builds the callback's renderer for
    /// the device's rate. The stream reports failures through `link`.
    fn open(&mut self, link: &Arc<Link>, epoch: Instant, make: &mut dyn FnMut(u32) -> Renderer) -> Result<Opened, String>;
}

pub struct CpalOut {
    host: cpal::Host,
}

impl CpalOut {
    pub fn new() -> Self {
        Self { host: cpal::default_host() }
    }
}

impl AudioOut for CpalOut {
    fn open(&mut self, link: &Arc<Link>, epoch: Instant, make: &mut dyn FnMut(u32) -> Renderer) -> Result<Opened, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = self.host.default_output_device().ok_or("aucune sortie audio")?;
        let name = device.description().ok().map(|d| d.name().to_string()).unwrap_or_else(|| "?".into());
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("format de sortie non géré : {}", supported.sample_format()));
        }
        let config = supported.config();
        let channels = config.channels as usize;
        let mut renderer = make(config.sample_rate);
        let failed = Arc::clone(link);
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    let latency = ts.playback.duration_since(ts.callback).as_secs_f64();
                    renderer.render(data, channels, epoch.elapsed().as_secs_f64(), latency);
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
        Ok(Opened { stream: Box::new(stream), device: name, sample_rate: config.sample_rate })
    }
}

/// The song-playback thread's state, one `step` every 20 ms.
pub struct Worker<O: AudioOut> {
    hub: Arc<SongHub>,
    media: Arc<MediaStore>,
    out: O,
    /// The file decoded (or that failed to), and its samples.
    loaded: Option<(String, Option<Arc<Decoded>>)>,
    stream: Option<Opened>,
    retry_at: Instant,
}

impl<O: AudioOut> Worker<O> {
    pub fn new(hub: Arc<SongHub>, media: Arc<MediaStore>, out: O) -> Self {
        Self { hub, media, out, loaded: None, stream: None, retry_at: Instant::now() }
    }

    fn close(&mut self) {
        self.stream = None;
        *self.hub.link.clock.lock().unwrap() = None;
    }

    pub fn step(&mut self, now: Instant) {
        let want = self.hub.wanted();
        if want.as_deref() != self.loaded.as_ref().map(|(f, _)| f.as_str()) {
            self.close();
            self.loaded = None;
            match want {
                None => self.hub.set_status(|s| *s = SongStatus::default()),
                Some(file) => {
                    self.hub.set_status(|s| *s = SongStatus { state: SongState::Loading, file: Some(file.clone()), ..Default::default() });
                    // Decoding takes a moment (≈1 s for a long MP3): only this thread waits.
                    match self.media.decode(&file) {
                        Ok(d) => self.loaded = Some((file, Some(Arc::new(d)))),
                        Err(e) => {
                            log::warn!("song {file}: {e:#}");
                            self.hub.set_status(|s| {
                                s.state = SongState::Error;
                                s.message = format!("{e:#}");
                            });
                            self.loaded = Some((file, None));
                        }
                    }
                    self.retry_at = now;
                }
            }
        }
        if self.stream.is_some() && self.hub.link.failed.swap(false, Ordering::Relaxed) {
            log::warn!("song output lost; retrying every {RETRY:?}");
            self.close();
            self.retry_at = now + RETRY;
            self.hub.set_status(|s| {
                s.state = SongState::NoDevice;
                s.message = "sortie audio perdue".into();
            });
        }
        let Some((file, Some(track))) = &self.loaded else { return };
        if self.stream.is_some() || now < self.retry_at {
            return;
        }
        self.hub.link.failed.store(false, Ordering::Relaxed);
        let (link, epoch) = (Arc::clone(&self.hub.link), self.hub.epoch);
        let track = Arc::clone(track);
        match self.out.open(&link, epoch, &mut |rate| Renderer::new(Arc::clone(&track), Arc::clone(&link), rate)) {
            Ok(opened) => {
                let (file, device, rate) = (file.clone(), opened.device.clone(), opened.sample_rate);
                self.hub.set_status(|s| *s = SongStatus { state: SongState::Ready, file: Some(file), message: String::new(), device: Some(device), sample_rate: rate });
                self.stream = Some(opened);
            }
            Err(e) => {
                let first = self.hub.status().state != SongState::NoDevice;
                if first {
                    log::warn!("song output: {e}; retrying every {RETRY:?}");
                }
                self.hub.set_status(|s| {
                    s.state = SongState::NoDevice;
                    s.message = e;
                });
                self.retry_at = now + RETRY;
            }
        }
    }
}

/// The song-playback thread (not started with `--no-audio`). It owns the
/// stream, which is not `Send` on every platform.
pub fn spawn(hub: Arc<SongHub>, media: Arc<MediaStore>, running: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("song-playback".into())
        .spawn(move || {
            let mut worker = Worker::new(hub, media, CpalOut::new());
            while running.load(Ordering::SeqCst) {
                worker.step(Instant::now());
                std::thread::sleep(Duration::from_millis(20));
            }
            worker.close();
        })
        .expect("failed to spawn the song-playback thread")
}

#[cfg(test)]
mod tests {
    use super::super::decode::testing::{sine, wav16};
    use super::*;
    use crate::timeline::{AudioRef, Event, EventSource, Show, TimeBase, Track};
    use std::sync::atomic::AtomicU64;

    /// A song whose sample value is its own position: frame i = i / 2^15
    /// (ramps up to 1 s at 32 768 Hz), so what is heard tells where.
    fn ramp_track() -> Arc<Decoded> {
        Arc::new(Decoded { sample_rate: 32_768, channels: 1, samples: (0..32_768).map(|i| i as i16).collect() })
    }

    fn pos_of(sample: f32) -> f64 {
        sample as f64 // = i / 32768 = seconds
    }

    fn command(link: &Link, c: Command) {
        *link.cmd.lock().unwrap() = c;
    }

    #[test]
    fn the_renderer_starts_where_the_playhead_is_when_heard() {
        let link = Arc::new(Link::default());
        let mut r = Renderer::new(ramp_track(), Arc::clone(&link), 32_768);
        let mut buf = vec![0.0f32; 512];
        r.render(&mut buf, 2, 10.0, 0.01);
        assert!(buf.iter().all(|&v| v == 0.0), "nothing asked: silence");
        // Asked at t = 10.0 to be at 0.2 s; this buffer is heard at 10.05 + 0.01.
        command(&link, Command { gen: 1, playing: true, file_s: 0.2, at_s: 10.0, gain: 1.0 });
        r.render(&mut buf, 2, 10.05, 0.01);
        let snap = link.clock.lock().unwrap().unwrap();
        assert_eq!(snap.gen, 1);
        assert!((snap.file_s - 0.26).abs() < 1e-9 && (snap.heard_s - 10.06).abs() < 1e-9, "{snap:?}");
        // Left and right carry the song (mono → both); after the fade-in the
        // sample value is the position.
        assert_eq!(buf[300], buf[301]);
        assert!((pos_of(buf[400]) - (0.26 + 200.0 / 32_768.0)).abs() < 1e-4, "{}", buf[400]);
        // Pause: silence after the fade-out, the clock holds.
        command(&link, Command { gen: 2, playing: false, file_s: 0.3, at_s: 10.1, gain: 1.0 });
        r.render(&mut buf, 2, 10.1, 0.01);
        assert!(buf[400..].iter().all(|&v| v == 0.0));
        let snap = link.clock.lock().unwrap().unwrap();
        assert!(!snap.playing && (snap.file_s_at(99.0) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn gain_resampling_and_channel_layouts() {
        let link = Arc::new(Link::default());
        // 32 768 Hz song on a 65 536 Hz device: half a song frame per sample.
        let mut r = Renderer::new(ramp_track(), Arc::clone(&link), 65_536);
        command(&link, Command { gen: 1, playing: true, file_s: 0.5, at_s: 0.0, gain: 0.5 });
        let mut buf = vec![0.0f32; 4 * 2000];
        r.render(&mut buf, 4, 0.0, 0.0);
        let (a, b) = (buf[4 * 1000], buf[4 * 1001]);
        assert!(((b - a) as f64 - 0.5 * 0.5 / 32_768.0).abs() < 1e-6, "interpolated half steps at half gain");
        assert!((pos_of(a * 2.0) - (0.5 + 500.0 / 32_768.0)).abs() < 1e-4);
        assert_eq!((buf[4 * 1000 + 2], buf[4 * 1000 + 3]), (0.0, 0.0), "extra channels stay silent");
        let mut mono = vec![0.0f32; 1000];
        r.render(&mut mono, 1, 0.1, 0.0);
        assert!(mono[999] > 0.0);
        // Past the end of the song: silence, but the clock keeps counting.
        command(&link, Command { gen: 2, playing: true, file_s: 5.0, at_s: 0.2, gain: 1.0 });
        r.render(&mut mono, 1, 0.2, 0.0);
        assert!(mono.iter().all(|&v| v == 0.0));
        r.render(&mut mono, 1, 0.3, 0.0);
        assert!(r.position_s() > 5.0);
    }

    #[test]
    fn the_callback_never_allocates() {
        let link = Arc::new(Link::default());
        let mut r = Renderer::new(ramp_track(), Arc::clone(&link), 48_000);
        let mut buf = vec![0.0f32; 1024];
        command(&link, Command { gen: 1, playing: true, file_s: 0.0, at_s: 0.0, gain: 1.0 });
        let n = super::super::capture::tests::allocations_during(|| {
            for k in 0..20 {
                r.render(&mut buf, 2, k as f64 * 0.01, 0.005);
            }
            let _held = link.cmd.lock().unwrap(); // a busy lock: skipped, not waited for
            r.render(&mut buf, 2, 0.3, 0.005);
        });
        assert_eq!(n, 0);
    }

    fn song_show(len: f64, file: &str) -> Show {
        let event = Event { id: 1, start: 0.0, len, source: EventSource::Cue { id: "c".into() }, ..Default::default() };
        Show {
            name: "chanson".into(),
            time_base: TimeBase::Seconds,
            audio: Some(AudioRef { file: file.into(), duration_s: len, ..Default::default() }),
            tracks: vec![Track { events: vec![event], ..Default::default() }],
            ..Default::default()
        }
    }

    fn clock(t: f64) -> Clock {
        Clock { t, beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 }
    }

    fn ready_hub(file: &str) -> Arc<SongHub> {
        let hub = Arc::new(SongHub::new(Instant::now(), true));
        hub.set_status(|s| *s = SongStatus { state: SongState::Ready, file: Some(file.into()), ..Default::default() });
        hub
    }

    /// Acceptance: 3 minutes of playback with a sound card whose crystal
    /// runs 100 ppm fast and callbacks that jitter by up to ±1 ms. The
    /// timeline never strays 10 ms from the song actually heard. On the
    /// system clock alone it would be 18 ms off by the end.
    #[test]
    fn three_minutes_of_playback_stay_within_10_ms_of_the_song() {
        let hub = ready_hub("s.wav");
        let track = Arc::new(Decoded { sample_rate: 48_000, channels: 1, samples: vec![0; 48_000 * 200] });
        let mut r = Renderer::new(track, Arc::clone(&hub.link), 48_000);
        let mut player = Player::default();
        player.load(song_show(200.0, "s.wav"));
        let mut sync = SongSync::default();
        let (ppm, latency, buffer) = (100e-6, 0.012, 512usize);
        let t0 = 5.0;
        player.play(&clock(t0));
        let mut buf = vec![0.0f32; buffer];
        let mut next_cb = t0 + 0.004; // the device's first callback
        let mut heard: Vec<(f64, f64)> = Vec::new(); // (studio time heard, song s)
        let mut seed = 7u64;
        let mut worst: f64 = 0.0;
        let mut t = t0;
        while t < t0 + 180.0 {
            // Callbacks due before this engine frame.
            while next_cb <= t {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let jitter = ((seed >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.002;
                r.render(&mut buf, 1, next_cb + jitter, latency);
                // This buffer's first sample is heard at next_cb + latency.
                heard.push((next_cb + latency, r.position_s() - buffer as f64 / 48_000.0));
                next_cb += buffer as f64 / (48_000.0 * (1.0 + ppm));
            }
            let c = clock(t);
            sync.sync(&hub, &mut player, &c);
            player.frame(&c);
            // The song truly heard now: last buffer start + device-rate time.
            if let Some(&(at, s)) = heard.iter().rev().find(|(at, _)| *at <= t) {
                let truth = s + (t - at) * (1.0 + ppm);
                let err = (player.position_at(&c) - truth).abs();
                if t > t0 + 0.1 {
                    worst = worst.max(err);
                }
            }
            if heard.len() > 64 {
                heard.drain(..32);
            }
            t += 1.0 / 60.0;
        }
        assert_eq!(sync.clock, ClockSource::Audio);
        assert!(worst < 0.010, "worst {:.2} ms", worst * 1e3);
        assert!(worst < 0.003, "in practice ~0.5 ms: worst {:.2} ms", worst * 1e3);
        assert!(180.0 * ppm > 0.010, "the system clock alone would have drifted past 10 ms");
    }

    #[test]
    fn transport_changes_become_commands_and_an_estop_silences_the_song() {
        let hub = ready_hub("s.wav");
        let mut player = Player::default();
        let mut sync = SongSync::default();
        sync.sync(&hub, &mut player, &clock(0.0));
        assert_eq!(hub.link.cmd.lock().unwrap().gen, 0, "no show: nothing sent");
        let mut show = song_show(60.0, "s.wav");
        show.audio.as_mut().unwrap().offset_s = 0.25;
        show.audio.as_mut().unwrap().gain = 0.8;
        player.load(show);
        sync.sync(&hub, &mut player, &clock(1.0));
        assert_eq!(hub.wanted().as_deref(), Some("s.wav"));
        let c = *hub.link.cmd.lock().unwrap();
        assert_eq!((c.gen, c.playing, c.file_s, c.gain), (1, false, -0.25, 0.8), "stopped at 0, song 0.25 s later");

        player.play(&clock(2.0));
        sync.sync(&hub, &mut player, &clock(2.0));
        let c = *hub.link.cmd.lock().unwrap();
        assert_eq!((c.gen, c.playing, c.at_s), (2, true, 2.0));
        sync.sync(&hub, &mut player, &clock(2.1));
        assert_eq!(hub.link.cmd.lock().unwrap().gen, 2, "nothing changed: no new command");
        assert_eq!(sync.clock, ClockSource::System, "the song hasn't answered yet");

        player.seek(30.0, &clock(3.0));
        sync.sync(&hub, &mut player, &clock(3.0));
        let c = *hub.link.cmd.lock().unwrap();
        assert_eq!((c.gen, c.playing), (3, true));
        assert!((c.file_s - 29.75).abs() < 1e-9);

        // The e-stop halts the timeline (main.rs): the song pauses too.
        player.halt(&clock(4.0));
        sync.sync(&hub, &mut player, &clock(4.0));
        let c = *hub.link.cmd.lock().unwrap();
        assert_eq!((c.gen, c.playing), (4, false));

        // Volume: no new command, just the gain.
        player.show.as_mut().unwrap().audio.as_mut().unwrap().gain = 0.3;
        sync.sync(&hub, &mut player, &clock(5.0));
        let c = *hub.link.cmd.lock().unwrap();
        assert_eq!((c.gen, c.gain), (4, 0.3));

        // Another show without a song: stop wanting the file, silence.
        let mut other = song_show(10.0, "");
        other.audio = None;
        player.load(other);
        sync.sync(&hub, &mut player, &clock(6.0));
        assert_eq!(hub.wanted(), None);
        assert!(!hub.link.cmd.lock().unwrap().playing);
    }

    #[test]
    fn a_stale_or_foreign_clock_is_not_followed() {
        let hub = ready_hub("s.wav");
        let mut player = Player::default();
        player.load(song_show(60.0, "s.wav"));
        player.play(&clock(0.0));
        let mut sync = SongSync::default();
        sync.sync(&hub, &mut player, &clock(0.0));
        // A clock from an older command is ignored...
        *hub.link.clock.lock().unwrap() = Some(ClockSnap { gen: 0, playing: true, file_s: 40.0, heard_s: 1.0 });
        sync.sync(&hub, &mut player, &clock(1.0));
        assert_eq!(sync.clock, ClockSource::System);
        assert!((player.position_at(&clock(1.0)) - 1.0).abs() < 1e-9);
        // ...so is one that stopped ticking (device gone)...
        *hub.link.clock.lock().unwrap() = Some(ClockSnap { gen: 1, playing: true, file_s: 0.5, heard_s: 0.5 });
        sync.sync(&hub, &mut player, &clock(1.2));
        assert_eq!(sync.clock, ClockSource::System);
        // ...and one of a song that is not the one loaded.
        hub.set_status(|s| s.file = Some("autre.wav".into()));
        *hub.link.clock.lock().unwrap() = Some(ClockSnap { gen: 1, playing: true, file_s: 1.25, heard_s: 1.25 });
        sync.sync(&hub, &mut player, &clock(1.3));
        assert_eq!(sync.clock, ClockSource::System);
        // The right one is followed.
        hub.set_status(|s| s.file = Some("s.wav".into()));
        sync.sync(&hub, &mut player, &clock(1.3));
        assert_eq!(sync.clock, ClockSource::Audio);
        // Without playback (--no-audio) the system clock stays.
        let off = SongHub::new(Instant::now(), false);
        let mut sync = SongSync::default();
        sync.sync(&off, &mut player, &clock(2.0));
        assert_eq!((sync.clock, off.link.cmd.lock().unwrap().gen), (ClockSource::System, 0));
        assert_eq!(off.status().state, SongState::Disabled);
    }

    /// A fake output: records the renderer so the test can pull buffers.
    #[derive(Default, Clone)]
    struct FakeOut {
        fail: Arc<AtomicBool>,
        opens: Arc<AtomicU64>,
        renderer: Arc<Mutex<Option<Renderer>>>,
    }

    impl AudioOut for FakeOut {
        fn open(&mut self, _: &Arc<Link>, _: Instant, make: &mut dyn FnMut(u32) -> Renderer) -> Result<Opened, String> {
            if self.fail.load(Ordering::Relaxed) {
                return Err("aucune sortie audio".into());
            }
            self.opens.fetch_add(1, Ordering::Relaxed);
            *self.renderer.lock().unwrap() = Some(make(48_000));
            Ok(Opened { stream: Box::new(()), device: "Fausse sortie".into(), sample_rate: 48_000 })
        }
    }

    fn temp_media(tag: &str) -> Arc<MediaStore> {
        let dir = std::env::temp_dir().join(format!("laser-studio-song-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let media = Arc::new(MediaStore::new(&dir));
        media.import("son.wav", &wav16(48_000, 1, &sine(48_000, 1.0, 440.0, 0.5))).unwrap();
        media
    }

    #[test]
    fn the_worker_decodes_opens_retries_and_reports() {
        let media = temp_media("worker");
        std::fs::write(media.dir().join("cassé.wav"), b"RIFF\x10\0\0\0WAVEjunk").unwrap();
        let hub = Arc::new(SongHub::new(Instant::now(), true));
        let out = FakeOut::default();
        let mut w = Worker::new(Arc::clone(&hub), media, out.clone());
        let t = Instant::now();
        w.step(t);
        assert_eq!(hub.status().state, SongState::Idle);

        hub.want(Some("son.wav"));
        out.fail.store(true, Ordering::Relaxed);
        w.step(t);
        let st = hub.status();
        assert_eq!((st.state, st.message.as_str()), (SongState::NoDevice, "aucune sortie audio"));
        out.fail.store(false, Ordering::Relaxed);
        w.step(t + Duration::from_secs(1));
        assert_eq!(hub.status().state, SongState::NoDevice, "retried every 2 s, not every step");
        w.step(t + Duration::from_secs(2));
        let st = hub.status();
        assert_eq!((st.state, st.file.as_deref(), st.device.as_deref()), (SongState::Ready, Some("son.wav"), Some("Fausse sortie")));
        assert!(hub.ready_for("son.wav"));
        // The stream's renderer plays what the engine asks.
        hub.command(Command { gen: 1, playing: true, file_s: 0.0, at_s: 0.0, gain: 1.0 });
        let mut buf = vec![0.0f32; 4800];
        out.renderer.lock().unwrap().as_mut().unwrap().render(&mut buf, 1, 0.0, 0.0);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
        assert!(hub.clock().is_some());

        // Device lost: closed, clock dropped, reopened 2 s later.
        hub.link.failed.store(true, Ordering::Relaxed);
        w.step(t + Duration::from_secs(3));
        assert_eq!(hub.status().state, SongState::NoDevice);
        assert!(hub.clock().is_none());
        w.step(t + Duration::from_secs(5));
        assert_eq!(hub.status().state, SongState::Ready);
        assert_eq!(out.opens.load(Ordering::Relaxed), 2);

        // A damaged file: a clear error, no output opened, no retry loop.
        hub.want(Some("cassé.wav"));
        w.step(t + Duration::from_secs(6));
        let st = hub.status();
        assert_eq!(st.state, SongState::Error);
        assert!(st.message.contains("WAV illisible"), "{}", st.message);
        w.step(t + Duration::from_secs(9));
        assert_eq!(out.opens.load(Ordering::Relaxed), 2);
        hub.want(Some("absent.wav"));
        w.step(t + Duration::from_secs(10));
        assert!(hub.status().message.contains("introuvable"));
        hub.want(None);
        w.step(t + Duration::from_secs(11));
        assert_eq!(hub.status().state, SongState::Idle);
    }

    /// Real hardware, opt-in: opens the Mac's default output and plays a
    /// song **at volume 0** for 2 s, then checks the song clock runs at the
    /// wall clock's pace and the song is where the playhead is.
    /// `cargo test -p laser-studio real_output -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_output_default_device_is_a_steady_clock() {
        let hub = SongHub::new(Instant::now(), true);
        let track = Arc::new(Decoded { sample_rate: 44_100, channels: 1, samples: vec![0; 44_100 * 5] });
        let mut out = CpalOut::new();
        let link = Arc::clone(&hub.link);
        let opened = out.open(&link, hub.epoch, &mut |rate| Renderer::new(Arc::clone(&track), Arc::clone(&link), rate)).expect("open the default output");
        println!("opened {} at {} Hz", opened.device, opened.sample_rate);
        let t0 = hub.epoch.elapsed().as_secs_f64();
        hub.command(Command { gen: 1, playing: true, file_s: 0.0, at_s: t0, gain: 0.0 });
        std::thread::sleep(Duration::from_millis(300));
        let a = hub.clock().expect("callbacks run");
        std::thread::sleep(Duration::from_millis(1700));
        let b = hub.clock().unwrap();
        assert_eq!((a.gen, b.gen), (1, 1));
        let (song, wall) = (b.file_s - a.file_s, b.heard_s - a.heard_s);
        println!("song {song:.4} s over {wall:.4} s of callbacks");
        assert!((song - wall).abs() < 0.02, "song and wall clock agree within 20 ms over ~1.7 s");
        // Where the song is now, from the snapshot, vs where it was asked to be.
        let now = hub.epoch.elapsed().as_secs_f64();
        assert!((b.file_s_at(now) - (now - t0)).abs() < 0.03, "the song is where the playhead is");
        drop(opened);
    }
}
