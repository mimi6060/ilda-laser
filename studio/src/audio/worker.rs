//! The two audio threads.
//!
//! **capture**: owns the `SampleSource` and the open stream. Every 50 ms
//! it applies config changes, and it watches the stream: a fault reported
//! by the backend, no callback for a second, or its device missing from
//! the list (scanned every 2 s) all close it; it is reopened after 2 s,
//! again and again until the device is back. Opening can take a while
//! (CoreAudio), which is why this is not the analysis thread.
//!
//! **analysis**: gets each new stream's ring through a channel, reads it
//! by hops and publishes the result in the hub. It never touches a device.
//! It also decodes the LTC channel's ring (T-168, timecode/ltc.rs) and
//! publishes each timecode frame in the hub.
//!
//! Neither ever takes the `Shared` lock: a stuck device can't stall the
//! engine or the UI.

use super::analysis::{Analyzer, FLOOR_DB, HOP};
use super::spectrum::SPECTRUM_BANDS;
use super::bpm::TempoEstimate;
use super::downbeat::DownbeatDetection;
use super::onsets::Onsets;
use super::sections::SectionState;
use super::health::{HealthMeter, InputHealth};
use super::capture::{self, CaptureCounters, CpalSource, OpenError, OpenRequest, SampleSource};
use super::{AudioHub, AudioInputSource, CaptureState, CaptureStatus, NativeSnapshot};
use crate::timecode::ltc::LtcDecoder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Retry delay after a failed open or a lost device (acceptance: back in
/// under 5 s after replugging).
pub const RETRY_EVERY: Duration = Duration::from_secs(2);
pub const SCAN_EVERY: Duration = Duration::from_secs(2);
/// A running stream with no callback for this long is considered dead.
pub const STALL_AFTER: Duration = Duration::from_secs(1);
/// A new stream gets longer to deliver its first callback.
pub const FIRST_CALLBACK_GRACE: Duration = Duration::from_secs(5);
const CAPTURE_TICK: Duration = Duration::from_millis(50);
/// Well under a hop (5.3 ms at 48 kHz).
const ANALYSIS_TICK: Duration = Duration::from_millis(2);

/// A new stream for the analysis thread.
pub struct Feed {
    pub consumer: rtrb::Consumer<f32>,
    pub sample_rate: u32,
    pub counters: Arc<CaptureCounters>,
    /// The LTC channel's copy (T-168): empty while no LTC is asked for.
    pub ltc: Option<rtrb::Consumer<f32>>,
}

struct Live {
    opened: capture::Opened,
    counters: Arc<CaptureCounters>,
    device: String,
    /// A device chosen by name (false: the system default input).
    named: bool,
    callbacks: u64,
    progress_at: Instant,
}

pub struct Capture<S: SampleSource> {
    source: S,
    hub: Arc<AudioHub>,
    feed: Sender<Feed>,
    live: Option<Live>,
    applied: Option<u64>,
    next_open: Instant,
    next_scan: Instant,
}

impl<S: SampleSource> Capture<S> {
    pub fn new(source: S, hub: Arc<AudioHub>, feed: Sender<Feed>) -> Self {
        let now = Instant::now();
        Self { source, hub, feed, live: None, applied: None, next_open: now, next_scan: now }
    }

    fn status(&self, state: CaptureState, device: Option<String>, message: String) {
        let (sample_rate, channels) = match (&self.live, state) {
            (Some(l), CaptureState::Running) => (l.opened.sample_rate, l.opened.channels),
            _ => (0, 0),
        };
        let overruns = self.live.as_ref().map_or(0, |l| l.counters.overruns.load(Ordering::Relaxed));
        let status = CaptureStatus { state, device, message: message.clone(), sample_rate, channels, overruns };
        if self.hub.set_status(status) {
            match state {
                CaptureState::Running | CaptureState::Off => log::info!("audio : {message}"),
                _ => log::warn!("audio : {message}"),
            }
        }
    }

    fn close(&mut self) {
        // Dropping the stream stops CoreAudio's callback; the analysis
        // thread sees the ring abandoned and lets it go.
        self.live = None;
    }

    fn lost(&mut self, now: Instant, why: &str) {
        let device = self.live.as_ref().map(|l| l.device.clone());
        self.close();
        self.next_open = now + RETRY_EVERY;
        let name = device.clone().unwrap_or_default();
        self.status(CaptureState::DeviceLost, device, format!("Entrée « {name} » perdue ({why}) : nouvel essai toutes les 2 s"));
    }

    /// One pass of the capture thread's loop.
    pub fn step(&mut self, now: Instant) {
        let (config, generation) = self.hub.config();
        if self.applied != Some(generation) {
            self.applied = Some(generation);
            self.close();
            self.next_open = now;
        }

        if now >= self.next_scan {
            self.next_scan = now + SCAN_EVERY;
            match self.source.devices() {
                Ok(devices) => {
                    let gone = self.live.as_ref().is_some_and(|l| l.named && !devices.iter().any(|d| d.name == l.device));
                    self.hub.set_devices(devices);
                    if gone {
                        self.lost(now, "débranchée");
                    }
                }
                Err(e) => log::debug!("audio : liste des entrées impossible : {e}"),
            }
        }

        if let Some(live) = self.live.as_mut() {
            let callbacks = live.counters.callbacks.load(Ordering::Acquire);
            if callbacks != live.callbacks {
                live.callbacks = callbacks;
                live.progress_at = now;
            }
            let overruns = live.counters.overruns.load(Ordering::Relaxed);
            let patience = if live.callbacks == 0 { FIRST_CALLBACK_GRACE } else { STALL_AFTER };
            if live.counters.is_failed() {
                self.lost(now, "erreur du périphérique");
            } else if now.saturating_duration_since(live.progress_at) > patience {
                self.lost(now, "plus de son reçu");
            } else {
                self.hub.set_overruns(overruns);
            }
        }

        if !config.source.captures() {
            self.close();
            self.status(CaptureState::Off, None, "Capture native arrêtée".into());
            return;
        }
        if self.live.is_some() || now < self.next_open {
            return;
        }
        // *Son du Mac* (T-241) has no device name: the Mac's default output.
        let system = config.source == AudioInputSource::System;
        let device = if system { None } else { config.device };
        self.open(&device, system, config.buffer_frames, now);
    }

    fn open(&mut self, device: &Option<String>, system: bool, buffer_frames: u32, now: Instant) {
        let counters = Arc::new(CaptureCounters::default());
        let req = OpenRequest { device: device.as_deref(), system, buffer_frames, counters: Arc::clone(&counters) };
        let epoch = self.hub.epoch();
        let mut consumer = None;
        let mut ltc = None;
        let tap = self.hub.ltc_tap();
        let result = self.source.open(&req, &mut |rate, channels| {
            let (sink, c) = capture::ring(rate, channels, Arc::clone(&counters), epoch);
            consumer = Some(c);
            let (sink, l) = sink.with_ltc(Arc::clone(&tap), rate.max(1) as usize * capture::RING_SECONDS);
            ltc = Some(l);
            sink
        });
        let wanted = if system { capture::SYSTEM_AUDIO_NAME.to_string() } else { device.clone().unwrap_or_else(|| "entrée par défaut".into()) };
        match (result, consumer) {
            (Ok(opened), Some(consumer)) => {
                let message = format!("Écoute : {} ({} Hz, {} canaux)", opened.device, opened.sample_rate, opened.channels);
                let feed = Feed { consumer, sample_rate: opened.sample_rate, counters: Arc::clone(&counters), ltc };
                let name = opened.device.clone();
                self.live = Some(Live { opened, counters, device: name.clone(), named: device.is_some(), callbacks: 0, progress_at: now });
                if self.feed.send(feed).is_err() {
                    log::warn!("audio : fil d'analyse arrêté");
                }
                self.status(CaptureState::Running, Some(name), message);
            }
            (Ok(_), None) => {
                self.next_open = now + RETRY_EVERY;
                self.status(CaptureState::Error, Some(wanted), "Erreur : flux ouvert sans tampon".into());
            }
            (Err(e), _) => {
                self.next_open = now + RETRY_EVERY;
                let (state, message) = match e {
                    OpenError::NoDevice => (CaptureState::NoDevice, "Pas d'entrée audio".to_string()),
                    OpenError::NotFound(name) => (CaptureState::DeviceLost, format!("Entrée « {name} » introuvable : nouvel essai toutes les 2 s")),
                    OpenError::PermissionDenied(_) if system => (
                        CaptureState::PermissionDenied,
                        "Autorisation refusée ? Réglages Système › Confidentialité et sécurité › Enregistrement de l'écran et de l'audio du système : autoriser le terminal".to_string(),
                    ),
                    OpenError::Unavailable(why) => (CaptureState::Error, format!("Son du Mac indisponible : {why}")),
                    OpenError::PermissionDenied(_) => (
                        CaptureState::PermissionDenied,
                        "Autorisation refusée ? Réglages Système › Confidentialité et sécurité › Microphone : autoriser le terminal".to_string(),
                    ),
                    OpenError::Failed(msg) => (CaptureState::Error, format!("Erreur : {msg}")),
                };
                self.status(state, Some(wanted), message);
            }
        }
    }

    pub fn run(mut self, running: &AtomicBool) {
        while running.load(Ordering::SeqCst) {
            self.step(Instant::now());
            std::thread::sleep(CAPTURE_TICK);
        }
        self.close();
    }
}

struct Current {
    feed: Feed,
    analyzer: Analyzer,
    consumed: u64,
    ltc: LtcDecoder,
    /// The signal's health (T-242), per stream: a new stream starts afresh.
    health: HealthMeter,
    verdict: InputHealth,
}

/// LTC samples read from the ring at once.
const LTC_CHUNK: usize = 512;

pub struct Analysis {
    hub: Arc<AudioHub>,
    feeds: Receiver<Feed>,
    current: Option<Current>,
    /// Onset counters (the legacy beat is the kick one), carried from one
    /// stream to the next (a reopened stream must not look like a new beat
    /// to the engine).
    onsets: Onsets,
    /// The last tempo estimate: a reopened stream shows its BPM until it
    /// has its own.
    tempo: TempoEstimate,
    /// Drop counter and history, carried like the onsets.
    sections: SectionState,
    /// Phrase counter, carried like the drops.
    downbeat: DownbeatDetection,
    /// *Nouveau morceau* requests already applied (`AudioHub::new_track`).
    new_tracks: u64,
    /// Generation of the *Guider* tempo already applied (`AudioHub::set_guide`).
    guides: u64,
    buf: [f32; HOP],
    ltc_buf: [f32; LTC_CHUNK],
}

impl Analysis {
    pub fn new(hub: Arc<AudioHub>, feeds: Receiver<Feed>) -> Self {
        let new_tracks = hub.new_track_requests();
        Self { hub, feeds, current: None, onsets: Onsets::default(), tempo: TempoEstimate::default(), sections: SectionState::default(), downbeat: DownbeatDetection::default(), new_tracks, guides: 0, buf: [0.0; HOP], ltc_buf: [0.0; LTC_CHUNK] }
    }

    /// Reads every complete hop waiting in the ring and publishes the
    /// latest result. Returns the number of hops analysed.
    pub fn poll(&mut self) -> usize {
        while let Ok(feed) = self.feeds.try_recv() {
            let mut analyzer = Analyzer::with_config(feed.sample_rate, self.onsets, self.hub.analysis_config());
            analyzer.carry_bpm(self.tempo.bpm);
            analyzer.set_guide(self.hub.guide().0);
            analyzer.carry_sections(&self.sections);
            analyzer.carry_downbeat(&self.downbeat);
            let ltc = LtcDecoder::new(feed.sample_rate);
            self.current = Some(Current { analyzer, feed, consumed: 0, ltc, health: HealthMeter::default(), verdict: InputHealth::Ok });
        }
        let Some(cur) = self.current.as_mut() else { return 0 };
        Self::decode_ltc(&self.hub, cur, &mut self.ltc_buf);
        let new_tracks = self.hub.new_track_requests();
        if new_tracks != self.new_tracks {
            self.new_tracks = new_tracks;
            cur.analyzer.new_track();
        }
        let (guide, guides) = self.hub.guide();
        if guides != self.guides {
            self.guides = guides;
            cur.analyzer.set_guide(guide);
        }
        let mut silence_db = None;
        if cur.feed.consumer.slots() >= HOP {
            // A copy under a leaf lock, at most once per poll.
            let config = self.hub.analysis_config();
            silence_db = Some(config.silence_db);
            cur.analyzer.set_config(config);
        }
        let hop_s = HOP as f64 / cur.feed.sample_rate.max(1) as f64;
        let mut hops = 0;
        let mut last = None;
        while cur.feed.consumer.slots() >= HOP && cur.feed.consumer.pop_entire_slice(&mut self.buf).is_ok() {
            cur.consumed += HOP as u64;
            let first = cur.feed.counters.first_callback_s().unwrap_or(0.0);
            // Dropped samples still took their time on the audio clock.
            let dropped = cur.feed.counters.dropped.load(Ordering::Relaxed);
            let t = first + (cur.consumed + dropped) as f64 / cur.feed.sample_rate.max(1) as f64;
            last = Some((cur.analyzer.process(&self.buf, t), t));
            let health = cur.health.hop(&self.buf, hop_s, silence_db.unwrap_or(super::spectrum::AnalysisConfig::default().silence_db));
            if health != cur.verdict {
                cur.verdict = health;
                Self::note_health(&self.hub, health);
            }
            hops += 1;
        }
        let health = cur.verdict;
        self.onsets = cur.analyzer.onsets();
        self.tempo = cur.analyzer.tempo();
        self.sections = cur.analyzer.sections();
        self.downbeat = cur.analyzer.downbeat();
        if let Some((m, t)) = last {
            // The display spectrum of the last hop only (64 maxima over 513
            // bins, once per poll).
            let mut spectrum = [FLOOR_DB; SPECTRUM_BANDS];
            cur.analyzer.log_spectrum(&mut spectrum);
            self.hub.publish(NativeSnapshot { features: m.features, rms_db: m.rms_db, peak_db: m.peak_db, spectral: m.spectral, onsets: m.onsets, tempo: m.tempo, spectrum, sections: m.sections, downbeat: m.downbeat, health, t, at: Instant::now() });
        }
        if cur.feed.consumer.is_abandoned() && cur.feed.consumer.slots() < HOP {
            // The stream was closed and its ring is drained.
            self.current = None;
        }
        hops
    }

    /// A new verdict on the signal: logged (application log) when it is a
    /// problem, kept as the last error when it is a likely refusal.
    fn note_health(hub: &AudioHub, health: InputHealth) {
        match health {
            InputHealth::PermissionSuspected => {
                let message = "Entrée exactement muette depuis plus de 2 s : autorisation micro refusée par macOS ?";
                log::warn!("audio : {message}");
                hub.note_error(message);
            }
            // Loud music can go in and out of the clip every second: not
            // worth the application log.
            _ => log::debug!("audio : {}", health.label()),
        }
    }

    /// Everything waiting in the LTC ring, through the decoder; the newest
    /// frame goes to the hub. Its time is the capture time of the sample
    /// that ended it, on the same clock as the analysis.
    fn decode_ltc(hub: &AudioHub, cur: &mut Current, buf: &mut [f32; LTC_CHUNK]) {
        let Some(ring) = cur.feed.ltc.as_mut() else { return };
        let mut newest = None;
        loop {
            let n = ring.slots().min(LTC_CHUNK);
            if n == 0 || ring.pop_entire_slice(&mut buf[..n]).is_err() {
                break;
            }
            cur.ltc.feed(&buf[..n], |f| newest = Some(f));
        }
        if let Some(f) = newest {
            let first = cur.feed.counters.first_callback_s().unwrap_or(0.0);
            hub.publish_ltc(f, first + f.end_sample as f64 / cur.feed.sample_rate.max(1) as f64);
        }
    }

    pub fn run(mut self, running: &AtomicBool) {
        while running.load(Ordering::SeqCst) {
            if self.poll() == 0 {
                std::thread::sleep(ANALYSIS_TICK);
            }
        }
    }
}

/// Starts the capture (CoreAudio) and analysis threads. Returns their
/// handles; a thread that can't be created is logged and skipped, and the
/// studio simply runs without native audio.
pub fn spawn(hub: Arc<AudioHub>, running: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    spawn_with(CpalSource::new, hub, running)
}

/// Same with another source, built on the capture thread (a CoreAudio
/// stream stays on the thread that opened it).
pub fn spawn_with<S: SampleSource + 'static>(make: impl FnOnce() -> S + Send + 'static, hub: Arc<AudioHub>, running: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    let (tx, rx) = channel();
    let analysis = std::thread::Builder::new().name("audio-analysis".into()).spawn({
        let (hub, running) = (Arc::clone(&hub), Arc::clone(&running));
        move || Analysis::new(hub, rx).run(&running)
    });
    let capture = std::thread::Builder::new().name("audio-capture".into()).spawn(move || Capture::new(make(), hub, tx).run(&running));
    [analysis, capture]
        .into_iter()
        .filter_map(|t| t.map_err(|e| log::warn!("audio : impossible de démarrer le thread : {e}")).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::analysis::testsig::sine;
    use super::super::bpm::DetectState;
    use super::super::{Active, AudioConfig, AudioInputSource};
    use super::capture::testing::FakeSource;
    use super::*;
    use crate::engine::AudioFeatures;

    struct Rig {
        fake: FakeSource,
        hub: Arc<AudioHub>,
        capture: Capture<FakeSource>,
        analysis: Analysis,
        now: Instant,
    }

    fn rig(devices: &[&str], config: AudioConfig) -> Rig {
        let fake = FakeSource::new(devices);
        let hub = Arc::new(AudioHub::new(Instant::now(), true, None, config));
        let (tx, rx) = channel();
        let capture = Capture::new(fake.clone(), Arc::clone(&hub), tx);
        let analysis = Analysis::new(Arc::clone(&hub), rx);
        Rig { fake, hub, capture, analysis, now: Instant::now() }
    }

    impl Rig {
        /// Advances simulated time by `ms`, stepping both threads every
        /// 10 ms and feeding 10 ms of a stereo 1 kHz tone while a stream
        /// is open.
        fn run(&mut self, ms: u64, amp: f32) {
            let mono = sine(1_000.0, amp, 48_000, 480, 0);
            let stereo: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
            for _ in 0..ms / 10 {
                self.now += Duration::from_millis(10);
                self.capture.step(self.now);
                self.fake.feed(&stereo);
                self.analysis.poll();
            }
        }

        fn state(&self) -> CaptureState {
            self.hub.status().state
        }

        /// Like `run`, with `signal(n)` (n: sample index since the start
        /// of this call) on both channels.
        fn run_with(&mut self, ms: u64, mut signal: impl FnMut(usize) -> f32) {
            let mut n = 0;
            for _ in 0..ms / 10 {
                self.now += Duration::from_millis(10);
                self.capture.step(self.now);
                let stereo: Vec<f32> = (0..480).flat_map(|i| [signal(n + i); 2]).collect();
                n += 480;
                self.fake.feed(&stereo);
                self.analysis.poll();
            }
        }

        fn diagnostics(&self) -> serde_json::Value {
            self.hub.diagnostics(Instant::now())
        }

        fn health(&self) -> String {
            self.diagnostics()["health"]["state"].as_str().unwrap_or("none").to_string()
        }
    }

    /// T-242 on the whole chain (fake input → ring → analysis → hub): what
    /// macOS delivers when it refuses the microphone (exact zeros), a
    /// real silence (background noise), a full-scale sine. No microphone.
    #[test]
    fn the_input_health_reads_zeros_noise_and_clipping_from_the_samples() {
        let mut r = rig(&["Mic"], AudioConfig::native());
        let mut waited = 0;
        while r.health() != "permission_suspected" {
            r.run(100, 0.0);
            waited += 100;
            assert!(waited <= 3_000, "not suspected after {waited} ms: {}", r.health());
        }
        assert!(waited >= 2_000, "not before 2 s of zeros: {waited} ms");
        let d = r.diagnostics();
        assert_eq!((d["state"].as_str(), d["signal_present"].as_bool()), (Some("running"), Some(false)));
        assert_eq!((d["sample_rate"].as_u64(), d["channels"].as_u64()), (Some(48_000), Some(2)));
        assert_eq!(d["devices"][0]["name"], "Mic");
        assert!(d["last_error"]["message"].as_str().unwrap().contains("autorisation micro"), "{d}");
        assert!(d["health"]["help"].as_str().unwrap().contains("Confidentialité et sécurité › Microphone"));
        assert_eq!(d["health"]["tone"], "bad");
        // The capture keeps running: zeros are not a lost device.
        assert_eq!(r.fake.opens(), 1);

        // Background noise at −70 dBFS: *Silence*, never the refusal.
        let mut seed = 1u32;
        let mut noise = move |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            // Uniform ±a, RMS a / √3 = −70 dBFS.
            10f32.powf(-70.0 / 20.0) * 3f32.sqrt() * ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        };
        for _ in 0..40 {
            r.run_with(100, &mut noise);
            assert_ne!(r.health(), "permission_suspected");
        }
        assert_eq!(r.health(), "silent");
        assert_eq!(r.diagnostics()["signal_present"], false);

        // A 0 dBFS sine: *Entrée trop forte*, and music is there.
        r.run(300, 1.0);
        assert_eq!(r.health(), "clipping");
        assert_eq!(r.diagnostics()["signal_present"], true);
        assert_eq!(r.hub.view(AudioFeatures::default(), Instant::now() - Duration::from_secs(60), Instant::now())["health"]["label"], "Entrée trop forte");
        // A normal level: Ok once the clip has been shown for 1 s.
        r.run(1_200, 0.3);
        assert_eq!(r.health(), "ok");
    }

    #[test]
    fn no_device_and_refusal_have_their_own_health_and_last_error() {
        let mut r = rig(&[], AudioConfig::native());
        r.run(100, 0.5);
        assert_eq!(r.health(), "no_device");
        assert_eq!(r.diagnostics()["last_error"]["message"], "Pas d'entrée audio");
        r.fake.plug("Mic");
        r.fake.0.lock().unwrap().refuse = Some(OpenError::PermissionDenied("TCC".into()));
        r.run(2_100, 0.5);
        assert_eq!(r.health(), "permission_suspected");
        // Another source: nothing to judge.
        r.hub.set_config(AudioConfig { source: AudioInputSource::Browser, ..AudioConfig::native() }).unwrap();
        r.run(20, 0.5);
        assert!(r.diagnostics()["health"].is_null());
        // The last error stays readable after it is over.
        assert!(r.diagnostics()["last_error"]["message"].as_str().unwrap().contains("Microphone"));
    }

    /// LTC on channel 2 (T-168), music on channel 1: the analysis thread
    /// decodes it and publishes each frame with its capture time, and the
    /// music analysis doesn't hear the timecode. Our own generated LTC:
    /// no microphone is ever opened.
    #[test]
    fn ltc_on_its_own_channel_is_decoded_and_kept_out_of_the_music() {
        use crate::timecode::{ltc, Rate, Tc};
        let mut r = rig(&["Scarlett 2i2 USB"], AudioConfig::native());
        r.hub.set_ltc_tap(2);
        let code = ltc::encode(Tc { h: 10, m: 0, s: 0, f: 0 }, Rate::F25, 50, 48_000, 0.5);
        let tone = sine(1_000.0, 0.5, 48_000, code.len(), 0);
        let stereo: Vec<f32> = tone.iter().zip(&code).flat_map(|(&a, &b)| [a, b]).collect();
        r.capture.step(r.now);
        for chunk in stereo.chunks(960) {
            r.now += Duration::from_millis(10);
            r.capture.step(r.now);
            assert!(r.fake.feed(chunk));
            r.analysis.poll();
        }
        let (gen, f, t) = r.hub.ltc_since(0).expect("decoded");
        // The last frame's closing flip is the next frame's start: the one
        // before is the newest complete one.
        assert_eq!(f.tc, Tc { h: 10, m: 0, s: 1, f: 23 });
        assert_eq!(Rate::from_ltc(f.fps, f.drop_frame), Rate::F25);
        // It ended 49 frames (1.96 s) of audio after the first callback.
        assert!((-0.04..0.5).contains(&(t - 1.96)), "{t}");
        let snap = r.hub.snapshot().expect("published");
        assert!((snap.rms_db + 9.0).abs() < 0.5, "the tone alone: {}", snap.rms_db);
        // Off: nothing more is copied or decoded.
        r.hub.set_ltc_tap(-1);
        r.run(200, 0.5);
        assert!(r.hub.ltc_since(gen).is_none());
    }

    #[test]
    fn native_capture_publishes_a_meter_without_any_browser() {
        let mut r = rig(&["Built-in Microphone", "Scarlett 2i2 USB"], AudioConfig::native());
        r.run(200, 0.5);
        assert_eq!(r.state(), CaptureState::Running);
        let status = r.hub.status();
        assert_eq!((status.device.as_deref(), status.sample_rate, status.channels), (Some("Built-in Microphone"), 48_000, 2));
        let names: Vec<String> = r.hub.devices().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Built-in Microphone", "Scarlett 2i2 USB"]);
        let snap = r.hub.snapshot().expect("published");
        assert!((snap.rms_db + 9.0).abs() < 0.5, "{snap:?}");
        assert!(snap.t > 0.0);
        // The bands come with it: a 1 kHz tone is `mid`.
        assert_eq!(snap.spectral.t, snap.t);
        assert!(snap.spectral.bands.mid > 0.9 && snap.spectral.bands.high < 0.1, "{:?}", snap.spectral);
        assert!(!snap.spectral.silent);
        // An analysis setting reaches the running analyser without a reopen.
        r.hub.set_config(AudioConfig { analysis: super::super::spectrum::AnalysisConfig { auto_gain: false, manual_gain_db: -40.0, ..Default::default() }, ..AudioConfig::native() }).unwrap();
        r.run(50, 0.5);
        assert_eq!(r.fake.opens(), 1);
        let mid = r.hub.snapshot().unwrap().spectral.bands.mid;
        assert!((mid - 0.22).abs() < 0.03, "manual: (-9 - 40 + 60) / 50 = {mid}");
        r.hub.set_config(AudioConfig::native()).unwrap();
        let never = Instant::now() - Duration::from_secs(60);
        let (f, active) = r.hub.effective(AudioFeatures::default(), never, Instant::now());
        assert_eq!(active, Active::Native);
        assert!(f.level > 0.9);
        // Quieter input: the meter follows.
        r.run(300, 0.05);
        assert!((r.hub.snapshot().unwrap().rms_db + 29.0).abs() < 0.5);
    }

    #[test]
    fn unplug_then_replug_resumes_by_itself_within_5_s() {
        let config = AudioConfig { device: Some("Scarlett 2i2 USB".into()), ..AudioConfig::native() };
        let mut r = rig(&["Built-in Microphone", "Scarlett 2i2 USB"], config);
        r.run(100, 0.5);
        assert_eq!(r.hub.status().device.as_deref(), Some("Scarlett 2i2 USB"));
        r.fake.unplug("Scarlett 2i2 USB");
        r.run(100, 0.5);
        assert_eq!(r.state(), CaptureState::DeviceLost);
        assert!(!r.fake.is_open(), "the dead stream is closed");
        // Stays lost (no silent switch to another input) while unplugged.
        r.run(5_000, 0.5);
        assert_eq!(r.state(), CaptureState::DeviceLost);
        assert!(r.fake.opens() <= 5, "retries every 2 s, not in a loop: {}", r.fake.opens());
        r.fake.plug("Scarlett 2i2 USB");
        let mut waited = 0;
        while r.state() != CaptureState::Running {
            r.run(100, 0.5);
            waited += 100;
            assert!(waited < 5_000, "not back after {waited} ms");
        }
        let before = r.hub.snapshot().unwrap().at;
        std::thread::sleep(Duration::from_millis(2));
        r.run(100, 0.5);
        assert!(r.hub.snapshot().unwrap().at > before, "analysis resumed");
    }

    #[test]
    fn a_device_that_vanishes_from_the_list_is_closed_even_without_an_error() {
        let config = AudioConfig { device: Some("USB".into()), ..AudioConfig::native() };
        let mut r = rig(&["USB"], config);
        r.run(100, 0.5);
        r.fake.0.lock().unwrap().devices.clear(); // no error reported
        r.run(2_100, 0.5);
        assert_eq!(r.state(), CaptureState::DeviceLost);
    }

    #[test]
    fn a_stream_without_callbacks_is_reopened() {
        let mut r = rig(&["Mic"], AudioConfig::native());
        r.run(100, 0.5);
        assert_eq!(r.fake.opens(), 1);
        // No more callbacks at all.
        for _ in 0..150 {
            r.now += Duration::from_millis(10);
            r.capture.step(r.now);
        }
        assert_eq!(r.state(), CaptureState::DeviceLost);
        r.run(2_100, 0.5);
        assert_eq!((r.state(), r.fake.opens()), (CaptureState::Running, 2));
    }

    /// *Son du Mac* (T-241) with a fake output: the same chain as an
    /// input (meter, features, health), the device name ignored.
    #[test]
    fn son_du_mac_captures_the_output_through_the_same_chain() {
        let config = AudioConfig { source: AudioInputSource::System, device: Some("Scarlett 2i2 USB".into()), ..AudioConfig::native() };
        let mut r = rig(&["Built-in Microphone", "Scarlett 2i2 USB"], config);
        r.fake.0.lock().unwrap().system_output = Some("Haut-parleurs MacBook Pro".into());
        r.run(300, 0.5);
        assert_eq!(r.state(), CaptureState::Running);
        assert_eq!(r.hub.status().device.as_deref(), Some("Son du Mac (Haut-parleurs MacBook Pro)"));
        assert!((r.hub.snapshot().expect("published").rms_db + 9.0).abs() < 0.5);
        let never = Instant::now() - Duration::from_secs(60);
        let (f, active) = r.hub.effective(AudioFeatures::default(), never, Instant::now());
        assert_eq!(active, Active::Native, "the studio's own analysis");
        assert!(f.level > 0.9);
        assert_eq!(r.health(), "ok");
        assert!(r.hub.fresh_tempo(Instant::now()).is_some());
        // An input unplugged meanwhile doesn't concern it.
        r.fake.unplug("Scarlett 2i2 USB");
        r.run(1_100, 0.5);
        assert_eq!((r.state(), r.fake.opens()), (CaptureState::Running, 1));
        // Exact zeros: nothing plays, or the permission is missing; its own help.
        r.run(3_000, 0.0);
        let d = r.diagnostics();
        assert_eq!(d["health"]["state"], "permission_suspected");
        assert!(d["health"]["help"].as_str().unwrap().contains("audio du système"), "{d}");
        // Back to Browser: closed.
        r.hub.set_config(AudioConfig { source: AudioInputSource::Browser, ..AudioConfig::native() }).unwrap();
        r.run(20, 0.5);
        assert!(!r.fake.is_open());
    }

    #[test]
    fn son_du_mac_unavailable_says_why_and_never_opens_an_input() {
        let config = AudioConfig { source: AudioInputSource::System, ..AudioConfig::native() };
        let mut r = rig(&["Mic"], config);
        r.run(100, 0.5);
        let status = r.hub.status();
        assert_eq!(status.state, CaptureState::Error);
        assert!(status.message.starts_with("Son du Mac indisponible"), "{}", status.message);
        assert!(!r.fake.is_open(), "the microphone is not opened instead");
        assert!(r.hub.snapshot().is_none());
        let never = Instant::now() - Duration::from_secs(60);
        assert_eq!(r.hub.effective(AudioFeatures::default(), never, Instant::now()).1, Active::None);
        // Retried calmly, logged once.
        r.run(2_100, 0.5);
        assert_eq!(r.fake.opens(), 2);
        assert!(!r.hub.set_status(r.hub.status()));
    }

    #[test]
    fn source_none_opens_nothing_and_says_so_once() {
        let mut r = rig(&["Mic"], AudioConfig::native().with_cli_device("none"));
        r.run(5_000, 0.5);
        assert_eq!(r.fake.opens(), 0);
        assert_eq!(r.state(), CaptureState::Off);
        assert!(!r.hub.set_status(r.hub.status()), "no repeated message");
        // Switching to Native from the UI starts it at once.
        r.hub.set_config(AudioConfig::native()).unwrap();
        r.run(20, 0.5);
        assert_eq!((r.state(), r.fake.opens()), (CaptureState::Running, 1));
        // And Browser closes it.
        r.hub.set_config(AudioConfig { source: AudioInputSource::Browser, ..AudioConfig::native() }).unwrap();
        r.run(20, 0.5);
        assert!(!r.fake.is_open());
        assert_eq!(r.state(), CaptureState::Off);
    }

    #[test]
    fn no_device_and_permission_refusal_are_reported_and_retried() {
        let mut r = rig(&[], AudioConfig::native());
        r.run(100, 0.5);
        assert_eq!(r.state(), CaptureState::NoDevice);
        r.fake.plug("Mic");
        r.fake.0.lock().unwrap().refuse = Some(OpenError::PermissionDenied("TCC".into()));
        r.run(2_000, 0.5);
        assert_eq!(r.state(), CaptureState::PermissionDenied);
        assert!(r.hub.status().message.contains("Microphone"));
        r.run(2_100, 0.5);
        assert_eq!(r.state(), CaptureState::Running);
    }

    #[test]
    fn the_beat_counter_survives_a_reopen() {
        let mut r = rig(&["Mic"], AudioConfig::native());
        r.run(50, 0.0);
        r.analysis.onsets = Onsets { onset: 50, kick: 41, snare: 20, hat: 90, ..Default::default() };
        r.hub.set_config(AudioConfig { buffer_frames: 512, ..AudioConfig::native() }).unwrap();
        r.run(100, 0.0);
        assert_eq!(r.fake.opens(), 2, "a config change reopens");
        let snap = r.hub.snapshot().unwrap();
        assert_eq!(snap.features.beat, 41);
        assert_eq!((snap.onsets.onset, snap.onsets.kick, snap.onsets.snare, snap.onsets.hat), (50, 41, 20, 90));
    }

    #[test]
    fn the_tempo_estimate_is_published_carried_and_reset_on_request() {
        let mut r = rig(&["Mic"], AudioConfig::native());
        r.run(50, 0.5);
        let snap = r.hub.snapshot().unwrap();
        assert_eq!((snap.tempo.bpm, snap.tempo.state), (0.0, DetectState::Checking), "a steady tone: listening, nothing yet");
        // A reopened stream shows the last BPM until it has its own.
        r.analysis.tempo = TempoEstimate { bpm: 126.0, ..Default::default() };
        r.hub.set_config(AudioConfig { buffer_frames: 512, ..AudioConfig::native() }).unwrap();
        r.run(100, 0.5);
        assert_eq!(r.fake.opens(), 2);
        assert_eq!(r.hub.snapshot().unwrap().tempo.bpm, 126.0);
        // *Nouveau morceau* reaches the analyser without a reopen.
        assert_eq!(r.analysis.new_tracks, 0);
        r.hub.new_track();
        r.run(20, 0.5);
        assert_eq!((r.analysis.new_tracks, r.fake.opens()), (1, 2));
        assert_eq!(r.hub.snapshot().unwrap().tempo.state, DetectState::Checking);
        // *Guider* (T-234) reaches it the same way, and survives a reopen.
        r.hub.set_guide(Some(128.0));
        r.run(20, 0.5);
        assert_eq!(r.hub.snapshot().unwrap().tempo.state, DetectState::Guided);
        r.hub.set_config(AudioConfig { buffer_frames: 256, ..AudioConfig::native() }).unwrap();
        r.run(100, 0.5);
        assert_eq!(r.fake.opens(), 3);
        assert_eq!(r.hub.snapshot().unwrap().tempo.state, DetectState::Guided);
        r.hub.set_guide(None);
        r.run(20, 0.5);
        assert_eq!(r.hub.snapshot().unwrap().tempo.state, DetectState::Checking);
    }

    #[test]
    fn the_threads_start_and_stop_with_a_fake_source() {
        let fake = FakeSource::new(&["Mic"]);
        let hub = Arc::new(AudioHub::new(Instant::now(), true, None, AudioConfig::native()));
        let running = Arc::new(AtomicBool::new(true));
        let source = fake.clone();
        let threads = spawn_with(move || source, Arc::clone(&hub), Arc::clone(&running));
        assert_eq!(threads.len(), 2);
        let tone = sine(440.0, 0.3, 48_000, 1_024, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while hub.snapshot().is_none() {
            fake.feed(&tone);
            std::thread::sleep(Duration::from_millis(5));
            assert!(Instant::now() < deadline, "no snapshot from the analysis thread");
        }
        running.store(false, Ordering::SeqCst);
        for t in threads {
            t.join().unwrap();
        }
        assert!(!fake.is_open(), "the stream is closed on shutdown");
    }
}

