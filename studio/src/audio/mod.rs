//! Native audio input (T-230): the studio listens to a Mac input itself,
//! so the laser keeps reacting to the music with the browser tab hidden or
//! closed.
//!
//! Three threads, none of which ever waits on another:
//! - the **audio callback** (CoreAudio's real-time thread, capture.rs):
//!   mono mix into a lock-free SPSC ring, nothing else;
//! - the **capture** thread (worker.rs): opens/closes the input, lists
//!   devices, retries every 2 s after an unplug or a refusal;
//! - the **analysis** thread (worker.rs): reads the ring by hops of 256
//!   samples (analysis.rs, spectrum.rs, onsets.rs, bpm.rs: meter, bands,
//!   auto-gain, silence, onsets, kick / snare / hat, BPM and beats)
//!   and publishes a snapshot in `AudioHub`, which the 60 fps engine reads
//!   at the top of each frame (a mutex held for a copy).
//!
//! The browser source (`POST /api/audio`) is kept: it is the *Navigateur*
//! source, and the fallback when the native one has nothing fresh.
//!
//! Audio *output* lives here too: the timeline's song (T-161) is decoded
//! (decode.rs, in a child process: isolate.rs), kept in
//! `studio-data/media/audio/` (media.rs) and played on the Mac's output by
//! its own thread, its clock driving the timeline (playback.rs).

pub mod analysis;
pub mod bpm;
pub mod capture;
pub mod decode;
pub mod isolate;
pub mod media;
pub mod onsets;
pub mod playback;
pub mod spectrum;
pub mod worker;

use crate::engine::{AudioFeatures, Section, AUDIO_EVENTS, AUDIO_VALUES};
use anyhow::{Context, Result};
use bpm::TempoEstimate;
use capture::{InputDevice, MAX_BUFFER_FRAMES, MIN_BUFFER_FRAMES};
use onsets::Onsets;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use spectrum::{AnalysisConfig, SpectralFrame, SPECTRUM_BANDS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Features older than this are treated as silence (tab closed, mic
/// stopped, device unplugged).
pub const STALE: Duration = Duration::from_millis(500);
/// Once stale, the engine's continuous values fall to neutral with this
/// time constant (T-237): after `STALE` + 5 τ = 1 s they are < 1 % of
/// where they were, and they never jump.
pub const RELEASE_TAU_S: f32 = 0.1;
/// `/api/state.audio` shows the engine's decaying frame while it is this
/// recent (else it computes the features itself).
const FRAME_RECENT: Duration = Duration::from_millis(250);

/// Features from `POST /api/audio` (the *Navigateur* source). The old
/// body `{level, bass, beat}` still works; what it doesn't send is filled
/// from it where that means something (`bands.sub`/`bands.bass` = `bass`,
/// `kick` = `onset` = `beat`, `level_db` from `level`, `silent` under
/// −60 dBFS, `t` = when it arrived), the rest stays neutral. Everything is
/// then sanitised (0..1, finite).
pub fn browser_features(body: &Value, received_t: f64) -> Result<AudioFeatures> {
    body.as_object().context("objet JSON attendu")?;
    let mut f: AudioFeatures = serde_json::from_value(body.clone()).context("level, bass (0..1), beat (entier) attendus")?;
    let sent = |k: &str| body.get(k).is_some();
    if !sent("bands") {
        f.bands.sub = f.bass;
        f.bands.bass = f.bass;
    }
    if !sent("kick") {
        f.kick = f.beat;
    }
    if !sent("onset") {
        f.onset = f.beat;
    }
    if !sent("level_db") {
        // The page's level is RMS × 6.
        let rms = (f.level / 6.0).max(0.0);
        f.level_db = analysis::to_db(rms * rms);
    }
    if !sent("silent") {
        f.silent = f.level_db < -60.0;
    }
    if !sent("section") {
        f.section = if f.silent { Section::Silence } else { Section::Normal };
    }
    if !sent("t") {
        f.t = received_t;
    }
    Ok(f.sanitized())
}

/// The engine's features as they fall back to neutral after the source
/// went stale or away.
#[derive(Clone, Copy, Debug)]
struct Release {
    out: AudioFeatures,
    active: Active,
    at: Option<Instant>,
}

/// One step of `y += (x − y)(1 − e^(−dt/τ))` on every continuous value of
/// `from` towards `to`; counters, flags, section, tempo and time are `to`'s.
fn release(from: &AudioFeatures, to: &AudioFeatures, dt: f32) -> AudioFeatures {
    let k = 1.0 - (-dt.max(0.0) / RELEASE_TAU_S).exp();
    let f = |a: f32, b: f32| a + (b - a) * k;
    let (fb, tb) = (from.bands.to_array(), to.bands.to_array());
    AudioFeatures {
        level: f(from.level, to.level),
        bass: f(from.bass, to.bass),
        level_db: f(from.level_db, to.level_db),
        bands: spectrum::Bands::from_array(std::array::from_fn(|i| f(fb[i], tb[i]))),
        kick_strength: f(from.kick_strength, to.kick_strength),
        snare_strength: f(from.snare_strength, to.snare_strength),
        hat_strength: f(from.hat_strength, to.hat_strength),
        centroid_hz: f(from.centroid_hz, to.centroid_hz),
        bpm_confidence: f(from.bpm_confidence, to.bpm_confidence),
        buildup: f(from.buildup, to.buildup),
        ..*to
    }
}

/// Where the engine's audio features come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioInputSource {
    /// The studio's own capture (this module); the browser's features
    /// stand in while it has nothing fresh.
    Native,
    /// Only `POST /api/audio` (the page's analysis), as before T-230. The
    /// default: the studio never opens the Mac's microphone until the
    /// operator chooses the native source.
    #[default]
    Browser,
    /// No audio: the looks don't react.
    None,
}

/// Saved in `<data-dir>/audio.json`. Machine settings, not part of a
/// project (a device name means nothing on another Mac).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub source: AudioInputSource,
    /// Input device name; `None` = the system default input.
    pub device: Option<String>,
    /// Buffer asked of CoreAudio, in frames (128..=4096; the device's
    /// default if it refuses).
    pub buffer_frames: u32,
    /// Bands: auto-gain, manual gain, silence threshold (T-231). Changing
    /// it never reopens the input.
    pub analysis: AnalysisConfig,
}

#[cfg(test)]
impl AudioConfig {
    /// Tests exercise the native capture explicitly (the default is Browser).
    pub fn native() -> Self {
        Self { source: AudioInputSource::Native, ..Default::default() }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { source: AudioInputSource::Browser, device: None, buffer_frames: 256, analysis: AnalysisConfig::default() }
    }
}

impl AudioConfig {
    pub fn sanitized(mut self) -> Self {
        self.buffer_frames = self.buffer_frames.clamp(MIN_BUFFER_FRAMES, MAX_BUFFER_FRAMES);
        self.device = self.device.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        self.analysis = self.analysis.sanitized();
        self
    }

    /// The fields that need the input reopened when they change.
    fn capture_part(&self) -> (AudioInputSource, Option<&str>, u32) {
        (self.source, self.device.as_deref(), self.buffer_frames)
    }

    /// `--audio-device`: `none` disables capture, `default` is the system
    /// input, anything else a device name.
    pub fn with_cli_device(mut self, arg: &str) -> Self {
        match arg.trim() {
            a if a.eq_ignore_ascii_case("none") => self.source = AudioInputSource::None,
            a if a.eq_ignore_ascii_case("default") || a.is_empty() => {
                self.source = AudioInputSource::Native;
                self.device = None;
            }
            name => {
                self.source = AudioInputSource::Native;
                self.device = Some(name.to_string());
            }
        }
        self
    }

    /// Applies the fields present in a `POST /api/audio/config` body.
    pub fn patched(&self, patch: &Value) -> Result<Self> {
        let mut c = self.clone();
        let obj = patch.as_object().context("objet JSON attendu")?;
        if let Some(v) = obj.get("source") {
            c.source = serde_json::from_value(v.clone()).context("source : native, browser ou none")?;
        }
        if let Some(v) = obj.get("device") {
            c.device = match v {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                _ => anyhow::bail!("device : nom (texte) ou null"),
            };
        }
        if let Some(v) = obj.get("buffer_frames") {
            c.buffer_frames = v.as_u64().context("buffer_frames : entier")?.min(u32::MAX as u64) as u32;
        }
        if let Some(v) = obj.get("analysis") {
            // Field by field too: `{"analysis": {"auto_gain": false}}`,
            // `{"analysis": {"onsets": {"delta": 0.2}}}`.
            let mut merged = serde_json::to_value(c.analysis)?;
            merge_fields(&mut merged, v, "analysis")?;
            c.analysis = serde_json::from_value(merged)
                .context("analysis : auto_gain (booléen), manual_gain_db, silence_db (nombres), onsets { delta, lookahead_hops (0..2), kick_refractory_ms }")?;
        }
        Ok(c.sanitized())
    }
}

/// Copies the fields of `patch` into `into`, recursing into objects; an
/// unknown field is an error (a typo must not be silently ignored).
fn merge_fields(into: &mut Value, patch: &Value, path: &str) -> Result<()> {
    let fields = patch.as_object().with_context(|| format!("{path} : objet attendu"))?;
    for (k, v) in fields {
        let Some(slot) = into.get_mut(k) else { anyhow::bail!("{path} : champ inconnu « {k} »") };
        if slot.is_object() {
            merge_fields(slot, v, &format!("{path}.{k}"))?;
        } else {
            *slot = v.clone();
        }
    }
    Ok(())
}

/// Stream figures for the meter and the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct CaptureStats {
    pub sample_rate: u32,
    pub channels: u16,
    pub overruns: u64,
    pub rms_db: f32,
    pub peak_db: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    /// Source is not *Native*: nothing open.
    #[default]
    Off,
    /// `--no-audio`: no capture thread at all.
    Disabled,
    Running,
    /// No input device on the Mac.
    NoDevice,
    /// The chosen device is missing or was unplugged: retried every 2 s.
    DeviceLost,
    PermissionDenied,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CaptureStatus {
    pub state: CaptureState,
    /// The device being captured (or last tried).
    pub device: Option<String>,
    /// For the UI, in French.
    pub message: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub overruns: u64,
}

/// What the analysis thread publishes after each batch of hops.
#[derive(Clone, Copy, Debug)]
pub struct NativeSnapshot {
    pub features: AudioFeatures,
    pub rms_db: f32,
    pub peak_db: f32,
    /// Bands, centroid, flatness, silence of the last hop.
    pub spectral: SpectralFrame,
    /// Onset / kick / snare / hat counters, strengths and times (T-232).
    pub onsets: Onsets,
    /// BPM, confidence, last / next beat, detector state (T-233). A
    /// proposal only: the tempo clock is not touched here.
    pub tempo: TempoEstimate,
    /// Display spectrum of the last hop: 64 log bands 20 Hz–20 kHz, dBFS
    /// (`GET /api/audio/spectrum`).
    pub spectrum: [f32; SPECTRUM_BANDS],
    /// Audio time of the end of the last hop, seconds since the studio
    /// epoch (the tempo clock's time base).
    pub t: f64,
    pub at: Instant,
}

/// Which source fed the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Active {
    Native,
    Browser,
    None,
}

#[derive(Default)]
struct Info {
    status: CaptureStatus,
    devices: Vec<InputDevice>,
}

/// The meeting point of the audio threads, the engine and the HTTP API.
/// Its locks are leaves: nothing else is ever locked while one is held,
/// and none is held for more than a copy.
pub struct AudioHub {
    epoch: Instant,
    capture_enabled: bool,
    path: Option<PathBuf>,
    /// The config and a generation bumped on every change.
    config: Mutex<(AudioConfig, u64)>,
    info: Mutex<Info>,
    snapshot: Mutex<Option<NativeSnapshot>>,
    /// *Nouveau morceau* requests, counted (the analysis thread compares).
    new_tracks: AtomicU64,
    /// The engine's last frame of features (`frame`).
    release: Mutex<Release>,
    /// The *Guider* tempo for the estimator (T-234) and a generation
    /// bumped on each change (the analysis thread compares).
    guide: Mutex<(Option<f32>, u64)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking audio thread must not take the engine down with it.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AudioHub {
    /// `epoch` is the studio's (`Shared::epoch`), so audio timestamps are
    /// on the tempo clock's time base.
    pub fn new(epoch: Instant, capture_enabled: bool, path: Option<PathBuf>, config: AudioConfig) -> Self {
        let status = if capture_enabled {
            CaptureStatus { message: "Capture native arrêtée".into(), ..Default::default() }
        } else {
            CaptureStatus { state: CaptureState::Disabled, message: "Capture audio désactivée (--no-audio)".into(), ..Default::default() }
        };
        Self { epoch, capture_enabled, path, config: Mutex::new((config.sanitized(), 0)), info: Mutex::new(Info { status, devices: Vec::new() }), snapshot: Mutex::new(None), new_tracks: AtomicU64::new(0), release: Mutex::new(Release { out: AudioFeatures::default(), active: Active::None, at: None }), guide: Mutex::new((None, 0)) }
    }

    /// From `<data_dir>/audio.json`, then `--audio-device` (for this run).
    pub fn load(data_dir: &Path, epoch: Instant, capture_enabled: bool, cli_device: Option<&str>) -> Self {
        let path = data_dir.join("audio.json");
        let mut config: AudioConfig = crate::load_json(&path);
        if let Some(arg) = cli_device {
            config = config.with_cli_device(arg);
        }
        Self::new(epoch, capture_enabled, Some(path), config)
    }

    #[cfg(test)]
    pub fn in_memory(capture_enabled: bool) -> Self {
        Self::new(Instant::now(), capture_enabled, None, AudioConfig::native())
    }

    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    pub fn capture_enabled(&self) -> bool {
        self.capture_enabled
    }

    pub fn config(&self) -> (AudioConfig, u64) {
        lock(&self.config).clone()
    }

    /// Saves and applies a new config; the capture thread picks it up
    /// within 50 ms (the generation only moves when the input must be
    /// reopened), the analysis thread at its next hop.
    pub fn set_config(&self, config: AudioConfig) -> Result<AudioConfig> {
        let config = config.sanitized();
        if let Some(path) = &self.path {
            let text = serde_json::to_string_pretty(&config)?;
            std::fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
        }
        let mut c = lock(&self.config);
        let generation = if c.0.capture_part() != config.capture_part() { c.1 + 1 } else { c.1 };
        *c = (config.clone(), generation);
        Ok(config)
    }

    /// For the analysis thread: a copy, no allocation.
    pub fn analysis_config(&self) -> AnalysisConfig {
        lock(&self.config).0.analysis
    }

    pub fn status(&self) -> CaptureStatus {
        lock(&self.info).status.clone()
    }

    /// Returns whether the state or message changed (the caller logs only
    /// then, so a retry loop doesn't flood the log).
    pub fn set_status(&self, status: CaptureStatus) -> bool {
        let mut info = lock(&self.info);
        let changed = info.status.state != status.state || info.status.message != status.message;
        info.status = status;
        changed
    }

    pub fn set_overruns(&self, overruns: u64) {
        lock(&self.info).status.overruns = overruns;
    }

    pub fn devices(&self) -> Vec<InputDevice> {
        lock(&self.info).devices.clone()
    }

    pub fn set_devices(&self, devices: Vec<InputDevice>) {
        lock(&self.info).devices = devices;
    }

    /// *Nouveau morceau*: the tempo estimator forgets its history at its
    /// next hop. Never blocks.
    pub fn new_track(&self) {
        self.new_tracks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn new_track_requests(&self) -> u64 {
        self.new_tracks.load(Ordering::Relaxed)
    }

    /// *Guider*: a tempo the estimator takes as a strong prior, or none.
    /// Applied by the analysis thread at its next poll.
    pub fn set_guide(&self, bpm: Option<f64>) {
        let bpm = bpm.filter(|b| b.is_finite() && *b > 0.0).map(|b| b as f32);
        let mut g = lock(&self.guide);
        if g.0 != bpm {
            *g = (bpm, g.1 + 1);
        }
    }

    /// The guide and its generation.
    pub fn guide(&self) -> (Option<f32>, u64) {
        *lock(&self.guide)
    }

    /// The latest native tempo estimate, if fresh and the native input is
    /// the audio source (what *Tempo auto* follows).
    pub fn fresh_tempo(&self, now: Instant) -> Option<TempoEstimate> {
        if lock(&self.config).0.source != AudioInputSource::Native {
            return None;
        }
        self.snapshot().filter(|n| now.saturating_duration_since(n.at) < STALE).map(|n| n.tempo)
    }

    pub fn publish(&self, snapshot: NativeSnapshot) {
        *lock(&self.snapshot) = Some(snapshot);
    }

    /// The latest native analysis, if any.
    pub fn snapshot(&self) -> Option<NativeSnapshot> {
        *lock(&self.snapshot)
    }

    /// The features the engine uses this frame, and where they came from.
    /// `browser`/`browser_at`: the last `POST /api/audio`. Stale features
    /// become silence but keep their beat counter, so going stale is never
    /// a beat.
    pub fn effective(&self, browser: AudioFeatures, browser_at: Instant, now: Instant) -> (AudioFeatures, Active) {
        let fresh = |at: Instant| now.saturating_duration_since(at) < STALE;
        let native = self.snapshot();
        let source = lock(&self.config).0.source;
        let browser_ok = fresh(browser_at);
        match source {
            AudioInputSource::Native => match native {
                Some(n) if fresh(n.at) => (n.features, Active::Native),
                _ if browser_ok => (browser, Active::Browser),
                Some(n) => (n.features.neutral(), Active::None),
                None => (browser.neutral(), Active::None),
            },
            AudioInputSource::Browser if browser_ok => (browser, Active::Browser),
            AudioInputSource::Browser | AudioInputSource::None => (browser.neutral(), Active::None),
        }
    }

    /// What the engine uses this frame (called once per frame, at its top).
    /// Fresh features pass through untouched; stale or absent ones make
    /// the continuous values *fall* to neutral (τ = `RELEASE_TAU_S`)
    /// instead of freezing or jumping, while the counters stay put.
    pub fn frame(&self, browser: AudioFeatures, browser_at: Instant, now: Instant) -> (AudioFeatures, Active) {
        let (target, active) = self.effective(browser, browser_at, now);
        let mut r = lock(&self.release);
        r.out = match (active, r.at) {
            (Active::None, Some(at)) => release(&r.out, &target, now.saturating_duration_since(at).as_secs_f32().min(0.1)),
            _ => target,
        };
        r.active = active;
        r.at = Some(now);
        (r.out, active)
    }

    /// For `/api/state`: the features as the engine sees them. Fresh ones
    /// are computed here (the next frame will use exactly these); while
    /// falling back to neutral, the engine's last frame.
    fn features_view(&self, browser: AudioFeatures, browser_at: Instant, now: Instant) -> (AudioFeatures, Active) {
        let (target, active) = self.effective(browser, browser_at, now);
        if active != Active::None {
            return (target, active);
        }
        let r = *lock(&self.release);
        match r.at {
            Some(at) if now.saturating_duration_since(at) < FRAME_RECENT => (r.out, Active::None),
            _ => (target, active),
        }
    }

    /// `GET /api/audio/spectrum`: the native capture's 64-band display
    /// spectrum (`db` in dBFS, `values` 0..1 over −90..0 dBFS), or nulls
    /// when nothing fresh is captured (the *Navigateur* source has none).
    pub fn spectrum_view(&self, now: Instant) -> Value {
        let source = lock(&self.config).0.source;
        let native = self.snapshot().filter(|n| now.saturating_duration_since(n.at) < STALE && source == AudioInputSource::Native);
        json!({
            "bands": SPECTRUM_BANDS,
            "lo_hz": 20.0,
            "hi_hz": 20_000.0,
            "t": native.map(|n| n.t),
            "db": native.map(|n| n.spectrum.to_vec()),
            "values": native.map(|n| n.spectrum.iter().map(|db| ((db + 90.0) / 90.0).clamp(0.0, 1.0)).collect::<Vec<f32>>()),
        })
    }

    /// `/api/state.audio`.
    pub fn view(&self, browser: AudioFeatures, browser_at: Instant, now: Instant) -> Value {
        let (features, active) = self.features_view(browser, browser_at, now);
        let (config, _) = self.config();
        let status = self.status();
        let native = self.snapshot().filter(|n| now.saturating_duration_since(n.at) < STALE && config.source == AudioInputSource::Native);
        let stats = CaptureStats {
            sample_rate: status.sample_rate,
            channels: status.channels,
            overruns: status.overruns,
            rms_db: native.map_or(analysis::FLOOR_DB, |n| n.rms_db),
            peak_db: native.map_or(analysis::FLOOR_DB, |n| n.peak_db),
        };
        json!({
            "source": config.source,
            "device": config.device,
            "buffer_frames": config.buffer_frames,
            "capture": self.capture_enabled,
            "active": active,
            "state": status.state,
            "message": status.message,
            "capturing": status.device.as_ref().filter(|_| status.state == CaptureState::Running),
            // The native meter (null when nothing fresh is captured).
            "level_db": native.map(|n| n.rms_db),
            "peak_db": native.map(|n| n.peak_db),
            "t": native.map(|n| n.t),
            // Bands (0..1 and dBFS), centroid, flatness, silence (T-231);
            // null when nothing fresh is captured.
            "spectral": native.map(|n| n.spectral),
            // Onset / kick / snare / hat counters, 0..1 strengths, audio
            // times (T-232); null when nothing fresh is captured.
            "onsets": native.map(|n| n.onsets),
            // BPM, confidence, beat_time / next_beat (audio times), state
            // (T-233); null when nothing fresh is captured.
            "tempo": native.map(|n| n.tempo),
            "stats": stats,
            // What the engine uses (T-237), whichever the source: the legacy
            // three, then the whole snapshot. `bands` always present, 0..1.
            "level": features.level,
            "bass": features.bass,
            "beat": features.beat,
            "bands": features.bands,
            "silent": features.silent,
            "section": features.section,
            "buildup": features.buildup,
            // By the ids routes and modulators use (`engine::AUDIO_EVENTS`,
            // `engine::AUDIO_VALUES`).
            "counters": AUDIO_EVENTS.iter().map(|&id| (id.to_string(), json!(features.counter(id)))).collect::<serde_json::Map<_, _>>(),
            "signals": AUDIO_VALUES.iter().map(|&id| (id.to_string(), json!(features.value(id)))).collect::<serde_json::Map<_, _>>(),
            "detected_bpm": features.bpm,
            "detected_confidence": features.bpm_confidence,
            "features": features,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(level: f32, beat: u64) -> AudioFeatures {
        AudioFeatures { level, bass: level, beat, ..Default::default() }
    }

    fn snap(level: f32, beat: u64, at: Instant) -> NativeSnapshot {
        let onsets = Onsets { kick: beat, onset: beat + 2, ..Default::default() };
        let tempo = TempoEstimate { bpm: 128.0, confidence: 0.8, beat_time: 0.9, next_beat: 1.37, state: bpm::DetectState::Locked };
        NativeSnapshot { features: feat(level, beat), rms_db: -20.0, peak_db: -10.0, spectral: SpectralFrame::default(), onsets, tempo, spectrum: [-30.0; SPECTRUM_BANDS], t: 1.0, at }
    }

    fn set_source(hub: &AudioHub, source: AudioInputSource) {
        hub.set_config(AudioConfig { source, ..hub.config().0 }).unwrap();
    }

    #[test]
    fn tempo_auto_reads_only_a_fresh_native_estimate_and_guides_by_generation() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        let mut n = snap(0.5, 1, now);
        n.tempo = TempoEstimate { bpm: 128.0, confidence: 0.9, state: bpm::DetectState::Locked, ..Default::default() };
        hub.publish(n);
        assert_eq!(hub.fresh_tempo(now).map(|t| t.bpm), Some(128.0));
        assert_eq!(hub.fresh_tempo(now + STALE), None, "stale");
        set_source(&hub, AudioInputSource::Browser);
        assert_eq!(hub.fresh_tempo(now), None, "native input not selected");

        assert_eq!(hub.guide(), (None, 0));
        hub.set_guide(Some(127.5));
        hub.set_guide(Some(127.5));
        assert_eq!(hub.guide(), (Some(127.5), 1), "same guide: no new generation");
        hub.set_guide(Some(f64::NAN));
        assert_eq!(hub.guide(), (None, 2));
    }

    #[test]
    fn config_defaults_and_old_files_load() {
        let c: AudioConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, AudioConfig { source: AudioInputSource::Browser, device: None, buffer_frames: 256, analysis: AnalysisConfig::default() });
        let c: AudioConfig = serde_json::from_str(r#"{"source":"browser","device":"Scarlett 2i2","extra":1}"#).unwrap();
        assert_eq!((c.source, c.device.as_deref(), c.buffer_frames), (AudioInputSource::Browser, Some("Scarlett 2i2"), 256));
        assert_eq!(AudioConfig { buffer_frames: 16, ..Default::default() }.sanitized().buffer_frames, 128, "never under 128 frames");
        assert_eq!(AudioConfig { buffer_frames: 1 << 20, device: Some("  ".into()), ..Default::default() }.sanitized(), AudioConfig { buffer_frames: 4096, ..Default::default() });
    }

    #[test]
    fn the_cli_device_overrides_the_file() {
        let saved = AudioConfig { source: AudioInputSource::Browser, device: Some("Old".into()), buffer_frames: 512, ..Default::default() };
        assert_eq!(saved.clone().with_cli_device("none").source, AudioInputSource::None);
        let c = saved.clone().with_cli_device("Scarlett 2i2 USB");
        assert_eq!((c.source, c.device.as_deref(), c.buffer_frames), (AudioInputSource::Native, Some("Scarlett 2i2 USB"), 512));
        let c = saved.with_cli_device("default");
        assert_eq!((c.source, c.device), (AudioInputSource::Native, None));
    }

    #[test]
    fn a_patch_changes_only_what_it_names() {
        let base = AudioConfig { device: Some("Mic".into()), ..Default::default() };
        let c = base.patched(&json!({ "source": "browser" })).unwrap();
        assert_eq!((c.source, c.device.as_deref()), (AudioInputSource::Browser, Some("Mic")));
        assert_eq!(base.patched(&json!({ "device": null })).unwrap().device, None);
        assert_eq!(base.patched(&json!({ "buffer_frames": 64 })).unwrap().buffer_frames, 128);
        assert!(base.patched(&json!({ "source": "spotify" })).is_err());
        assert!(base.patched(&json!({ "device": 3 })).is_err());
        assert!(base.patched(&json!([1])).is_err());
    }

    #[test]
    fn the_analysis_settings_are_patched_field_by_field() {
        let base = AudioConfig::default();
        let c = base.patched(&json!({ "analysis": { "auto_gain": false } })).unwrap();
        assert_eq!(c.analysis, AnalysisConfig { auto_gain: false, ..Default::default() });
        let c = c.patched(&json!({ "analysis": { "manual_gain_db": 99, "silence_db": -50 } })).unwrap();
        assert_eq!(c.analysis, AnalysisConfig { auto_gain: false, manual_gain_db: 40.0, silence_db: -50.0, ..Default::default() });
        assert!(base.patched(&json!({ "analysis": { "auto_gain": "oui" } })).is_err());
        assert!(base.patched(&json!({ "analysis": { "gain": 3 } })).is_err());
        assert!(base.patched(&json!({ "analysis": 3 })).is_err());
        // The onset settings, field by field one level down.
        let c = base.patched(&json!({ "analysis": { "onsets": { "delta": 0.3 } } })).unwrap();
        assert_eq!(c.analysis.onsets, onsets::OnsetConfig { delta: 0.3, ..Default::default() });
        let c = c.patched(&json!({ "analysis": { "onsets": { "lookahead_hops": 7, "kick_refractory_ms": 150 } } })).unwrap();
        assert_eq!(c.analysis.onsets, onsets::OnsetConfig { delta: 0.3, lookahead_hops: 2, kick_refractory_ms: 150.0 });
        assert!(base.patched(&json!({ "analysis": { "onsets": { "sensitivity": 1 } } })).is_err());
        assert!(base.patched(&json!({ "analysis": { "onsets": 3 } })).is_err());
        assert!(base.patched(&json!({ "analysis": { "onsets": { "delta": "x" } } })).is_err());
        // Old audio.json files without it load with the defaults.
        let c: AudioConfig = serde_json::from_str(r#"{"source":"native","analysis":{"silence_db":-70}}"#).unwrap();
        assert_eq!(c.analysis, AnalysisConfig { silence_db: -70.0, ..Default::default() });
    }

    #[test]
    fn an_analysis_change_does_not_reopen_the_input() {
        let hub = AudioHub::in_memory(true);
        let (c, g) = hub.config();
        hub.set_config(AudioConfig { analysis: AnalysisConfig { auto_gain: false, ..Default::default() }, ..c.clone() }).unwrap();
        assert_eq!(hub.config().1, g, "same generation: the capture thread leaves the stream alone");
        assert!(!hub.analysis_config().auto_gain);
        hub.set_config(AudioConfig { buffer_frames: 512, ..hub.config().0 }).unwrap();
        assert_eq!(hub.config().1, g + 1);
        assert!(!hub.analysis_config().auto_gain, "kept");
    }

    #[test]
    fn a_config_change_bumps_the_generation_and_is_saved() {
        let dir = std::env::temp_dir().join(format!("laser-studio-audio-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hub = AudioHub::load(&dir, Instant::now(), true, None);
        let (_, g0) = hub.config();
        hub.set_config(AudioConfig { source: AudioInputSource::Native, ..Default::default() }).unwrap();
        let (c, g1) = hub.config();
        assert_eq!((c.source, g1), (AudioInputSource::Native, g0 + 1));
        hub.set_config(c.clone()).unwrap();
        assert_eq!(hub.config().1, g1, "same config: no reopen");
        let reloaded = AudioHub::load(&dir, Instant::now(), true, None);
        assert_eq!(reloaded.config().0.source, AudioInputSource::Native);
        assert_eq!(AudioHub::load(&dir, Instant::now(), true, Some("none")).config().0.source, AudioInputSource::None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn native_wins_when_fresh_and_the_browser_stands_in() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        let old = now - Duration::from_secs(2);
        // Nothing at all: silence.
        assert_eq!(hub.effective(feat(0.7, 3), old, now).1, Active::None);
        // Only the browser: it stands in for the native capture.
        let (f, a) = hub.effective(feat(0.7, 3), now, now);
        assert_eq!((a, f.level), (Active::Browser, 0.7));
        hub.publish(snap(0.4, 9, now));
        let (f, a) = hub.effective(feat(0.7, 3), now, now);
        assert_eq!((a, f.level, f.beat), (Active::Native, 0.4, 9));
        // Native gone stale, browser too: silence, but the beat counter stays.
        let later = now + Duration::from_secs(1);
        let (f, a) = hub.effective(feat(0.7, 3), now, later);
        assert_eq!((a, f.level, f.bass, f.beat), (Active::None, 0.0, 0.0, 9));
    }

    #[test]
    fn the_browser_sends_the_old_format_or_the_new_one() {
        // Before T-237: three fields. What can be derived is.
        let f = browser_features(&json!({ "level": 0.6, "bass": 0.4, "beat": 7 }), 12.5).unwrap();
        assert_eq!((f.level, f.bass, f.beat), (0.6, 0.4, 7));
        assert_eq!((f.bands.sub, f.bands.bass, f.bands.mid), (0.4, 0.4, 0.0));
        assert_eq!((f.kick, f.onset, f.snare, f.drop), (7, 7, 0, 0));
        assert!((f.level_db - -20.0).abs() < 0.01, "level 0.6 = RMS 0.1 = −20 dBFS: {}", f.level_db);
        assert_eq!((f.silent, f.section, f.t), (false, Section::Normal, 12.5));
        let f = browser_features(&json!({ "level": 0, "bass": 0, "beat": 0 }), 1.0).unwrap();
        assert_eq!((f.level_db, f.silent, f.section), (analysis::FLOOR_DB, true, Section::Silence));
        // The whole snapshot: kept as sent, bounded.
        let v = json!({
            "level": 0.5, "bass": 0.3, "beat": 4, "level_db": -18, "bands": { "sub": 0.1, "bass": 0.2, "low_mid": 0.3, "mid": 0.4, "high": 7 },
            "onset": 9, "kick": 4, "snare": 2, "hat": 11, "kick_strength": 0.8, "silent": false, "bpm": 128, "bpm_confidence": 0.7,
            "section": "buildup", "buildup": 0.6, "drop": 1, "t": 3.25,
        });
        let f = browser_features(&v, 99.0).unwrap();
        assert_eq!(f.bands, spectrum::Bands { sub: 0.1, bass: 0.2, low_mid: 0.3, mid: 0.4, high: 1.0 });
        assert_eq!((f.onset, f.kick, f.snare, f.hat, f.drop), (9, 4, 2, 11, 1));
        assert_eq!((f.level_db, f.bpm, f.section, f.buildup, f.t), (-18.0, 128.0, Section::Buildup, 0.6, 3.25));
        // Nonsense is clamped, wrong types refused.
        let f = browser_features(&json!({ "level": 9, "bass": -3, "bpm_confidence": 2, "level_db": 400 }), 0.0).unwrap();
        assert_eq!((f.level, f.bass, f.bpm_confidence, f.level_db), (1.0, 0.0, 1.0, 12.0));
        assert!(browser_features(&json!({ "level": "fort" }), 0.0).is_err());
        assert!(browser_features(&json!({ "section": "chorus" }), 0.0).is_err());
        assert!(browser_features(&json!([1]), 0.0).is_err());
    }

    /// 60 fps frames from `from` for `seconds`; returns (time since `from`, features).
    fn frames(hub: &AudioHub, from: Instant, seconds: f32) -> Vec<(f32, AudioFeatures)> {
        (0..=(seconds * 60.0) as u32)
            .map(|i| {
                let d = Duration::from_secs_f32(i as f32 / 60.0);
                (d.as_secs_f32(), hub.frame(AudioFeatures::default(), from - Duration::from_secs(10), from + d).0)
            })
            .collect()
    }

    #[test]
    fn stale_features_fall_to_neutral_within_a_second_without_a_jump() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        let mut loud = snap(0.8, 5, now);
        loud.features = AudioFeatures { level: 0.8, bass: 0.9, beat: 5, kick: 5, hat: 3, bands: spectrum::Bands { sub: 0.9, bass: 0.9, low_mid: 0.7, mid: 0.6, high: 0.5 }, buildup: 0.4, ..Default::default() };
        hub.publish(loud);
        // The analysis thread stops publishing here.
        let run = frames(&hub, now, 1.2);
        for (t, f) in &run {
            if *t < 0.49 {
                assert_eq!((f.level, f.bands.sub), (0.8, 0.9), "fresh until 500 ms: {t}");
            }
            if *t >= 1.0 {
                assert!(f.level < 0.01 && f.bass < 0.01 && f.bands.to_array().iter().all(|&b| b < 0.01) && f.buildup < 0.01, "neutral by 1 s: {t} {f:?}");
                assert!(f.silent && f.section == Section::Silence);
            }
            assert_eq!((f.beat, f.kick, f.hat), (5, 5, 3), "going stale is never an event");
        }
        for w in run.windows(2) {
            let (a, b) = (w[0].1, w[1].1);
            assert!(b.level <= a.level, "falls monotonically");
            assert!(a.level - b.level < 0.8 * 0.2, "no step of more than 20 % in a frame: {} → {}", a.level, b.level);
        }
        // /api/state shows the falling values while the engine runs.
        let hub2 = AudioHub::in_memory(true);
        hub2.publish(loud);
        frames(&hub2, now, 0.6);
        let mid = now + Duration::from_millis(600);
        let shown = hub2.view(AudioFeatures::default(), now - Duration::from_secs(10), mid + Duration::from_millis(5))["level"].as_f64().unwrap();
        assert!(shown > 0.05 && shown < 0.8, "{shown}");
        // New audio: straight back, untouched.
        hub.publish(snap(0.3, 6, now + Duration::from_millis(1300)));
        let (f, a) = hub.frame(AudioFeatures::default(), now - Duration::from_secs(10), now + Duration::from_millis(1310));
        assert_eq!((a, f.level, f.beat), (Active::Native, 0.3, 6));
    }

    #[test]
    fn a_stopped_browser_also_falls_back_gently() {
        let hub = AudioHub::in_memory(true);
        set_source(&hub, AudioInputSource::Browser);
        let now = Instant::now();
        let posted = browser_features(&json!({ "level": 1, "bass": 1, "beat": 3 }), 0.0).unwrap();
        let at = |ms: u64| hub.frame(posted, now, now + Duration::from_millis(ms)).0;
        assert_eq!(at(0).bass, 1.0);
        assert_eq!(at(490).bass, 1.0);
        let just_stale = at(510).bass;
        assert!(just_stale > 0.8 && just_stale < 1.0, "{just_stale}");
        let mut last = at(520);
        for ms in (530..=1000).step_by(16) {
            last = at(ms);
        }
        assert!(last.bass < 0.01 && last.beat == 3, "{last:?}");
    }

    #[test]
    fn the_frame_path_does_not_allocate() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        hub.publish(snap(0.4, 9, now));
        hub.frame(AudioFeatures::default(), now, now);
        let n = capture::tests::allocations_during(|| {
            for i in 0..120 {
                hub.frame(AudioFeatures::default(), now, now + Duration::from_millis(i * 16));
            }
        });
        assert_eq!(n, 0);
    }

    #[test]
    fn the_spectrum_is_served_only_from_a_fresh_native_capture() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        let v = hub.spectrum_view(now);
        assert_eq!((v["bands"].as_u64(), &v["db"], &v["values"]), (Some(64), &Value::Null, &Value::Null));
        hub.publish(snap(0.4, 9, now));
        let v = hub.spectrum_view(now);
        assert_eq!(v["db"].as_array().unwrap().len(), 64);
        let values = v["values"].as_array().unwrap();
        assert_eq!(values.len(), 64);
        assert!((values[0].as_f64().unwrap() - 60.0 / 90.0).abs() < 1e-4, "−30 dBFS over −90..0");
        assert_eq!(hub.spectrum_view(now + Duration::from_secs(1))["db"], Value::Null, "stale");
    }

    #[test]
    fn browser_and_none_sources_ignore_the_native_capture() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        hub.publish(snap(0.4, 9, now));
        set_source(&hub, AudioInputSource::Browser);
        let (f, a) = hub.effective(feat(0.7, 3), now, now);
        assert_eq!((a, f.level), (Active::Browser, 0.7), "as before T-230");
        let (f, a) = hub.effective(feat(0.7, 3), now - Duration::from_secs(1), now);
        assert_eq!((a, f.level, f.beat), (Active::None, 0.0, 3));
        set_source(&hub, AudioInputSource::None);
        let (f, a) = hub.effective(feat(0.7, 3), now, now);
        assert_eq!((a, f.level, f.beat), (Active::None, 0.0, 3));
    }

    #[test]
    fn the_state_view_shows_the_meter_only_when_fresh() {
        let hub = AudioHub::in_memory(true);
        let now = Instant::now();
        let v = hub.view(AudioFeatures::default(), now - Duration::from_secs(5), now);
        assert_eq!(v["level_db"], Value::Null);
        assert_eq!(v["spectral"], Value::Null);
        assert_eq!(v["onsets"], Value::Null);
        assert_eq!(v["tempo"], Value::Null);
        assert_eq!(v["source"], "native");
        assert_eq!(v["active"], "none");
        assert_eq!(v["stats"]["rms_db"], analysis::FLOOR_DB as f64);
        assert_eq!(v["bands"]["sub"], 0.0, "bands always present");
        assert_eq!(v["silent"], true);
        hub.publish(snap(0.4, 9, now));
        let v = hub.view(AudioFeatures::default(), now - Duration::from_secs(5), now);
        assert_eq!(v["level_db"], -20.0);
        assert_eq!(v["active"], "native");
        assert_eq!(v["spectral"]["bands"]["low_mid"], 0.0);
        assert_eq!(v["spectral"]["bands_db"].as_array().unwrap().len(), 5);
        assert_eq!(v["spectral"]["silent"], true);
        assert_eq!((v["onsets"]["kick"].as_u64(), v["onsets"]["onset"].as_u64()), (Some(9), Some(11)));
        assert_eq!(v["beat"], 9, "the legacy beat is the kick counter");
        assert_eq!((v["tempo"]["bpm"].as_f64(), v["tempo"]["state"].as_str()), (Some(128.0), Some("locked")));
        assert_eq!(v["tempo"]["next_beat"], 1.37);
        // The engine's whole snapshot (T-237).
        assert_eq!(v["features"]["beat"], 9);
        assert_eq!(v["counters"]["beat"], 9);
        assert_eq!(v["counters"].as_object().unwrap().len(), AUDIO_EVENTS.len());
        assert_eq!(v["signals"].as_object().unwrap().len(), AUDIO_VALUES.len());
        assert_eq!(v["section"], "normal");
        let bands = v["bands"].as_object().unwrap();
        assert_eq!(bands.len(), 5);
        assert!(bands.values().all(|b| (0.0..=1.0).contains(&b.as_f64().unwrap())));
        set_source(&hub, AudioInputSource::Browser);
        assert_eq!(hub.view(AudioFeatures::default(), now, now)["spectral"], Value::Null, "native only");
        let off = AudioHub::in_memory(false);
        assert_eq!(off.view(AudioFeatures::default(), now, now)["state"], "disabled");
        assert_eq!(off.view(AudioFeatures::default(), now, now)["capture"], false);
    }
}
