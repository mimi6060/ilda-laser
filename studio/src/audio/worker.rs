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
//!
//! Neither ever takes the `Shared` lock: a stuck device can't stall the
//! engine or the UI.

use super::analysis::{Analyzer, HOP};
use super::capture::{self, CaptureCounters, CpalSource, OpenError, OpenRequest, SampleSource};
use super::{AudioHub, AudioInputSource, CaptureState, CaptureStatus, NativeSnapshot};
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

        if config.source != AudioInputSource::Native {
            self.close();
            self.status(CaptureState::Off, None, "Capture native arrêtée".into());
            return;
        }
        if self.live.is_some() || now < self.next_open {
            return;
        }
        self.open(&config.device, config.buffer_frames, now);
    }

    fn open(&mut self, device: &Option<String>, buffer_frames: u32, now: Instant) {
        let counters = Arc::new(CaptureCounters::default());
        let req = OpenRequest { device: device.as_deref(), buffer_frames, counters: Arc::clone(&counters) };
        let epoch = self.hub.epoch();
        let mut consumer = None;
        let result = self.source.open(&req, &mut |rate, channels| {
            let (sink, c) = capture::ring(rate, channels, Arc::clone(&counters), epoch);
            consumer = Some(c);
            sink
        });
        let wanted = device.clone().unwrap_or_else(|| "entrée par défaut".into());
        match (result, consumer) {
            (Ok(opened), Some(consumer)) => {
                let message = format!("Écoute : {} ({} Hz, {} canaux)", opened.device, opened.sample_rate, opened.channels);
                let feed = Feed { consumer, sample_rate: opened.sample_rate, counters: Arc::clone(&counters) };
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
}

pub struct Analysis {
    hub: Arc<AudioHub>,
    feeds: Receiver<Feed>,
    current: Option<Current>,
    /// Beat counter, carried from one stream to the next (a reopened
    /// stream must not look like a new beat to the engine).
    beat: u64,
    buf: [f32; HOP],
}

impl Analysis {
    pub fn new(hub: Arc<AudioHub>, feeds: Receiver<Feed>) -> Self {
        Self { hub, feeds, current: None, beat: 0, buf: [0.0; HOP] }
    }

    /// Reads every complete hop waiting in the ring and publishes the
    /// latest result. Returns the number of hops analysed.
    pub fn poll(&mut self) -> usize {
        while let Ok(feed) = self.feeds.try_recv() {
            let analyzer = Analyzer::with_config(feed.sample_rate, self.beat, self.hub.analysis_config());
            self.current = Some(Current { analyzer, feed, consumed: 0 });
        }
        let Some(cur) = self.current.as_mut() else { return 0 };
        if cur.feed.consumer.slots() >= HOP {
            // A copy under a leaf lock, at most once per poll.
            cur.analyzer.set_config(self.hub.analysis_config());
        }
        let mut hops = 0;
        let mut last = None;
        while cur.feed.consumer.slots() >= HOP && cur.feed.consumer.pop_entire_slice(&mut self.buf).is_ok() {
            cur.consumed += HOP as u64;
            let first = cur.feed.counters.first_callback_s().unwrap_or(0.0);
            // Dropped samples still took their time on the audio clock.
            let dropped = cur.feed.counters.dropped.load(Ordering::Relaxed);
            let t = first + (cur.consumed + dropped) as f64 / cur.feed.sample_rate.max(1) as f64;
            last = Some((cur.analyzer.process(&self.buf, t), t));
            hops += 1;
        }
        self.beat = cur.analyzer.beat();
        if let Some((m, t)) = last {
            self.hub.publish(NativeSnapshot { features: m.features, rms_db: m.rms_db, peak_db: m.peak_db, spectral: m.spectral, t, at: Instant::now() });
        }
        if cur.feed.consumer.is_abandoned() && cur.feed.consumer.slots() < HOP {
            // The stream was closed and its ring is drained.
            self.current = None;
        }
        hops
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
    use super::super::{Active, AudioConfig};
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
        r.analysis.beat = 41;
        r.hub.set_config(AudioConfig { buffer_frames: 512, ..AudioConfig::native() }).unwrap();
        r.run(100, 0.0);
        assert_eq!(r.fake.opens(), 2, "a config change reopens");
        assert_eq!(r.hub.snapshot().unwrap().features.beat, 41);
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

