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
//!   samples and publishes a snapshot in `AudioHub`, which the 60 fps
//!   engine reads at the top of each frame (a mutex held for a copy).
//!
//! The browser source (`POST /api/audio`) is kept: it is the *Navigateur*
//! source, and the fallback when the native one has nothing fresh.

pub mod analysis;
pub mod capture;
pub mod worker;

use crate::engine::AudioFeatures;
use anyhow::{Context, Result};
use capture::{InputDevice, MAX_BUFFER_FRAMES, MIN_BUFFER_FRAMES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Features older than this are treated as silence (tab closed, mic
/// stopped, device unplugged).
pub const STALE: Duration = Duration::from_millis(500);

/// Where the engine's audio features come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioInputSource {
    /// The studio's own capture (this module); the browser's features
    /// stand in while it has nothing fresh.
    #[default]
    Native,
    /// Only `POST /api/audio` (the page's analysis), as before T-230.
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
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { source: AudioInputSource::Native, device: None, buffer_frames: 256 }
    }
}

impl AudioConfig {
    pub fn sanitized(mut self) -> Self {
        self.buffer_frames = self.buffer_frames.clamp(MIN_BUFFER_FRAMES, MAX_BUFFER_FRAMES);
        self.device = self.device.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        self
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
        Ok(c.sanitized())
    }
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
        Self { epoch, capture_enabled, path, config: Mutex::new((config.sanitized(), 0)), info: Mutex::new(Info { status, devices: Vec::new() }), snapshot: Mutex::new(None) }
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
        Self::new(Instant::now(), capture_enabled, None, AudioConfig::default())
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
    /// within 50 ms.
    pub fn set_config(&self, config: AudioConfig) -> Result<AudioConfig> {
        let config = config.sanitized();
        if let Some(path) = &self.path {
            let text = serde_json::to_string_pretty(&config)?;
            std::fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
        }
        let mut c = lock(&self.config);
        if c.0 != config {
            *c = (config.clone(), c.1 + 1);
        }
        Ok(config)
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
                Some(n) => (AudioFeatures { beat: n.features.beat, ..Default::default() }, Active::None),
                None => (AudioFeatures { beat: browser.beat, ..Default::default() }, Active::None),
            },
            AudioInputSource::Browser if browser_ok => (browser, Active::Browser),
            AudioInputSource::Browser | AudioInputSource::None => (AudioFeatures { beat: browser.beat, ..Default::default() }, Active::None),
        }
    }

    /// `/api/state.audio`.
    pub fn view(&self, browser: AudioFeatures, browser_at: Instant, now: Instant) -> Value {
        let (features, active) = self.effective(browser, browser_at, now);
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
            "stats": stats,
            "level": features.level,
            "bass": features.bass,
            "beat": features.beat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(level: f32, beat: u64) -> AudioFeatures {
        AudioFeatures { level, bass: level, beat }
    }

    fn snap(level: f32, beat: u64, at: Instant) -> NativeSnapshot {
        NativeSnapshot { features: feat(level, beat), rms_db: -20.0, peak_db: -10.0, t: 1.0, at }
    }

    fn set_source(hub: &AudioHub, source: AudioInputSource) {
        hub.set_config(AudioConfig { source, ..hub.config().0 }).unwrap();
    }

    #[test]
    fn config_defaults_and_old_files_load() {
        let c: AudioConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, AudioConfig { source: AudioInputSource::Native, device: None, buffer_frames: 256 });
        let c: AudioConfig = serde_json::from_str(r#"{"source":"browser","device":"Scarlett 2i2","extra":1}"#).unwrap();
        assert_eq!((c.source, c.device.as_deref(), c.buffer_frames), (AudioInputSource::Browser, Some("Scarlett 2i2"), 256));
        assert_eq!(AudioConfig { buffer_frames: 16, ..Default::default() }.sanitized().buffer_frames, 128, "never under 128 frames");
        assert_eq!(AudioConfig { buffer_frames: 1 << 20, device: Some("  ".into()), ..Default::default() }.sanitized(), AudioConfig { buffer_frames: 4096, ..Default::default() });
    }

    #[test]
    fn the_cli_device_overrides_the_file() {
        let saved = AudioConfig { source: AudioInputSource::Browser, device: Some("Old".into()), buffer_frames: 512 };
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
    fn a_config_change_bumps_the_generation_and_is_saved() {
        let dir = std::env::temp_dir().join(format!("laser-studio-audio-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hub = AudioHub::load(&dir, Instant::now(), true, None);
        let (_, g0) = hub.config();
        hub.set_config(AudioConfig { source: AudioInputSource::Browser, ..Default::default() }).unwrap();
        let (c, g1) = hub.config();
        assert_eq!((c.source, g1), (AudioInputSource::Browser, g0 + 1));
        hub.set_config(c.clone()).unwrap();
        assert_eq!(hub.config().1, g1, "same config: no reopen");
        let reloaded = AudioHub::load(&dir, Instant::now(), true, None);
        assert_eq!(reloaded.config().0.source, AudioInputSource::Browser);
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
        assert_eq!(v["source"], "native");
        assert_eq!(v["active"], "none");
        assert_eq!(v["stats"]["rms_db"], analysis::FLOOR_DB as f64);
        hub.publish(snap(0.4, 9, now));
        let v = hub.view(AudioFeatures::default(), now - Duration::from_secs(5), now);
        assert_eq!(v["level_db"], -20.0);
        assert_eq!(v["active"], "native");
        let off = AudioHub::in_memory(false);
        assert_eq!(off.view(AudioFeatures::default(), now, now)["state"], "disabled");
        assert_eq!(off.view(AudioFeatures::default(), now, now)["capture"], false);
    }
}
