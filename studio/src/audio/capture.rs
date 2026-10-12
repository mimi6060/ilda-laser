//! The real-time half of native capture (T-230): what runs in the audio
//! callback, the ring it fills, and the `SampleSource` trait that opens an
//! input (CoreAudio through `cpal`, or a fake one in tests).
//!
//! The callback never allocates, locks or logs: it averages the channels
//! of each frame to mono and pushes the samples into a lock-free SPSC ring
//! (`rtrb`). A full ring drops the newest samples and counts an overrun;
//! it never waits. With LTC in (T-168) it also copies one channel (or the
//! mix) into a second ring for the timecode decoder; that channel is then
//! left out of the mono mix. Errors reported by the backend only set atomic flags,
//! read by the capture thread (worker.rs).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// The ring holds this much audio: a stalled analysis thread has one
/// second before samples are dropped.
pub const RING_SECONDS: usize = 1;
/// Smallest buffer we ask CoreAudio for (task note: below that, CPU load).
pub const MIN_BUFFER_FRAMES: u32 = 128;
pub const MAX_BUFFER_FRAMES: u32 = 4096;
/// Sample rate we ask for when the device supports it.
pub const PREFERRED_RATE: u32 = 48_000;

/// A sample format the callback can take, converted to -1..1.
pub trait ToF32: Copy {
    fn to_f32(self) -> f32;
}

impl ToF32 for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}

impl ToF32 for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / 32_768.0
    }
}

impl ToF32 for i32 {
    fn to_f32(self) -> f32 {
        (self as f64 / 2_147_483_648.0) as f32
    }
}

impl ToF32 for u16 {
    fn to_f32(self) -> f32 {
        (self as f32 - 32_768.0) / 32_768.0
    }
}

/// Counters the callback and the backend's error handler update
/// lock-free; the capture and analysis threads read them.
#[derive(Debug)]
pub struct CaptureCounters {
    /// Callbacks received (the capture thread watches it move).
    pub callbacks: AtomicU64,
    /// Mono frames lost because the ring was full.
    pub dropped: AtomicU64,
    /// Callbacks that lost frames, plus xruns reported by the backend.
    pub overruns: AtomicU64,
    /// Nanoseconds from the studio epoch to the first callback
    /// (`u64::MAX` until then): sample `n` was captured at about
    /// `first + n / rate`.
    first_ns: AtomicU64,
    /// The backend reported the stream dead (device gone, invalidated).
    failed: AtomicBool,
}

impl Default for CaptureCounters {
    fn default() -> Self {
        Self { callbacks: AtomicU64::new(0), dropped: AtomicU64::new(0), overruns: AtomicU64::new(0), first_ns: AtomicU64::new(u64::MAX), failed: AtomicBool::new(false) }
    }
}

/// What the backend's error handler reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFault {
    /// A glitch; the stream goes on.
    Xrun,
    /// Not fatal (real-time priority refused): the stream goes on.
    Minor,
    /// The stream is gone or must be rebuilt (device unplugged, route
    /// changed, invalidated).
    Lost,
}

impl CaptureCounters {
    /// Called from the backend's error handler: atomics only.
    pub fn report(&self, fault: StreamFault) {
        match fault {
            StreamFault::Xrun => {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
            StreamFault::Minor => {}
            StreamFault::Lost => self.failed.store(true, Ordering::Release),
        }
    }

    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Seconds from the studio epoch to the first callback, once there
    /// has been one.
    pub fn first_callback_s(&self) -> Option<f64> {
        match self.first_ns.load(Ordering::Acquire) {
            u64::MAX => None,
            ns => Some(ns as f64 / 1e9),
        }
    }
}

/// The producer end, moved into the audio callback.
pub struct Sink {
    producer: rtrb::Producer<f32>,
    channels: usize,
    inv_channels: f32,
    counters: Arc<CaptureCounters>,
    epoch: Instant,
    ltc: Option<LtcTap>,
}

/// The LTC copy (T-168): `tap` says which channel (-1 none, 0 the mix, n
/// channel n), read once per callback.
struct LtcTap {
    tap: Arc<AtomicI32>,
    producer: rtrb::Producer<f32>,
}

impl Sink {
    /// The callback body: interleaved samples → mono → ring. No
    /// allocation, no lock, never blocks (see the counting-allocator test).
    pub fn push<T: ToF32>(&mut self, data: &[T]) {
        let c = &self.counters;
        if c.first_ns.load(Ordering::Relaxed) == u64::MAX {
            let ns = Instant::now().saturating_duration_since(self.epoch).as_nanos().min(u64::MAX as u128 - 1) as u64;
            c.first_ns.store(ns, Ordering::Release);
        }
        let mut lost = 0u64;
        let tap = self.ltc.as_ref().map_or(-1, |l| l.tap.load(Ordering::Relaxed));
        // A channel of its own carries the LTC: out of the music's mix.
        let skip = if tap >= 1 && tap as usize <= self.channels && self.channels > 1 { tap as usize - 1 } else { usize::MAX };
        let inv = if skip == usize::MAX { self.inv_channels } else { 1.0 / (self.channels - 1) as f32 };
        for frame in data.chunks_exact(self.channels) {
            let mut sum = 0.0f32;
            for (i, s) in frame.iter().enumerate() {
                if i != skip {
                    sum += s.to_f32();
                }
            }
            if self.producer.push(sum * inv).is_err() {
                lost += 1;
            }
            if let (Some(l), true) = (self.ltc.as_mut(), tap >= 0) {
                let v = match tap as usize {
                    0 => sum * inv,
                    n => frame.get(n - 1).map_or(0.0, |s| s.to_f32()),
                };
                // A full LTC ring loses samples, never time.
                let _ = l.producer.push(v);
            }
        }
        if lost > 0 {
            c.dropped.fetch_add(lost, Ordering::Relaxed);
            c.overruns.fetch_add(1, Ordering::Relaxed);
        }
        c.callbacks.fetch_add(1, Ordering::Release);
    }
}

/// A new ring for a stream of `channels` interleaved channels at
/// `sample_rate`: the callback's `Sink` and the analysis side's consumer.
pub fn ring(sample_rate: u32, channels: u16, counters: Arc<CaptureCounters>, epoch: Instant) -> (Sink, rtrb::Consumer<f32>) {
    ring_with_capacity(sample_rate.max(1) as usize * RING_SECONDS, channels, counters, epoch)
}

pub fn ring_with_capacity(capacity: usize, channels: u16, counters: Arc<CaptureCounters>, epoch: Instant) -> (Sink, rtrb::Consumer<f32>) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(1));
    let channels = channels.max(1) as usize;
    (Sink { producer, channels, inv_channels: 1.0 / channels as f32, counters, epoch, ltc: None }, consumer)
}

impl Sink {
    /// Adds the LTC copy (T-168): a ring of `capacity` samples, filled
    /// while `tap` is 0 or more. Returns the decoder's end.
    pub fn with_ltc(mut self, tap: Arc<AtomicI32>, capacity: usize) -> (Sink, rtrb::Consumer<f32>) {
        let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(1));
        self.ltc = Some(LtcTap { tap, producer });
        (self, consumer)
    }
}

/// An audio input as listed by `GET /api/audio/devices`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDevice {
    pub name: String,
    /// The system's default input (Réglages Système › Son › Entrée).
    pub is_default: bool,
}

/// What to open.
pub struct OpenRequest<'a> {
    /// `None` = the system default input.
    pub device: Option<&'a str>,
    /// *Son du Mac* (T-241): what the Mac plays on its default output,
    /// instead of an input (`device` is then ignored).
    pub system: bool,
    pub buffer_frames: u32,
    pub counters: Arc<CaptureCounters>,
}

/// An open, playing input stream. Dropping `stream` closes it (on the
/// thread that opened it: a `cpal` stream is not `Send` everywhere).
pub struct Opened {
    /// Only held: the stream plays as long as it lives.
    #[allow(dead_code)]
    pub stream: Box<dyn std::any::Any>,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No input device at all (no default input).
    NoDevice,
    /// The named device isn't there (unplugged?).
    NotFound(String),
    /// macOS refused the microphone (TCC). Often it doesn't say so and
    /// delivers silence instead (T-242).
    PermissionDenied(String),
    /// *Son du Mac* asked of a build or a macOS that can't do it.
    Unavailable(String),
    Failed(String),
}

/// What *Son du Mac* is called in the device line and the status.
pub const SYSTEM_AUDIO_NAME: &str = "Son du Mac";
/// Core Audio process taps record the output from macOS 14.2; `cpal`
/// needs 14.6 for its loopback stream.
pub const SYSTEM_AUDIO_MIN_MACOS: (u32, u32) = (14, 6);

/// Whether *Son du Mac* (T-241) can be offered here: compiled in (Cargo
/// feature `system-audio`, off by default) and a recent enough macOS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SystemAudioSupport {
    pub compiled: bool,
    /// `ProductVersion` of the running macOS, if read.
    pub macos: Option<String>,
    pub available: bool,
    /// Why not, in French (`None` when available).
    pub reason: Option<String>,
}

impl SystemAudioSupport {
    /// From what is known; pure, for the tests.
    pub fn from_parts(compiled: bool, macos: Option<&str>) -> Self {
        let (major, minor) = SYSTEM_AUDIO_MIN_MACOS;
        let reason = if !compiled {
            Some("non compilé dans ce studio (option system-audio) : passez par BlackHole".to_string())
        } else {
            match macos {
                None => Some("version de macOS inconnue : passez par BlackHole".to_string()),
                Some(v) if !version_at_least(v, (major, minor)) => Some(format!("macOS {v} : il faut macOS {major}.{minor} ou plus récent, sinon passez par BlackHole")),
                Some(_) => None,
            }
        };
        Self { compiled, macos: macos.map(str::to_string), available: reason.is_none(), reason }
    }

    /// This build on this Mac. Reads one small system file; called once
    /// at start-up, never from the audio callback.
    pub fn detect() -> Self {
        Self::from_parts(cfg!(all(feature = "system-audio", target_os = "macos")), macos_version().as_deref())
    }
}

/// `"14.6.1"` ≥ (14, 6)? Missing parts count as 0; garbage is too old.
pub fn version_at_least(version: &str, (major, minor): (u32, u32)) -> bool {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(a)), None) => (a, 0) >= (major, minor),
        (Some(Ok(a)), Some(Ok(b))) => (a, b) >= (major, minor),
        _ => false,
    }
}

/// The `ProductVersion` string of a `SystemVersion.plist`.
pub fn plist_product_version(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>ProductVersion</key>")? + "<key>ProductVersion</key>".len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string()).filter(|v| !v.is_empty())
}

/// The running macOS version (`None` elsewhere or if unreadable).
fn macos_version() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let text = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    plist_product_version(&text)
}

/// Where samples come from: the Mac's inputs (`CpalSource`) or, in tests,
/// a fake that is fed synthetic signals. Called only from the capture
/// thread, never under the `Shared` lock.
pub trait SampleSource {
    fn devices(&mut self) -> anyhow::Result<Vec<InputDevice>>;
    /// Opens and starts an input. `make_sink(rate, channels)` builds the
    /// ring once the stream format is known; it may be called again if a
    /// first attempt (fixed buffer size) is refused.
    fn open(&mut self, req: &OpenRequest, make_sink: &mut dyn FnMut(u32, u16) -> Sink) -> Result<Opened, OpenError>;
}

/// CoreAudio (on the Mac) through `cpal`.
pub struct CpalSource {
    host: cpal::Host,
}

impl CpalSource {
    pub fn new() -> Self {
        Self { host: cpal::default_host() }
    }
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

fn fault_of(kind: cpal::ErrorKind) -> StreamFault {
    match kind {
        cpal::ErrorKind::Xrun => StreamFault::Xrun,
        cpal::ErrorKind::RealtimeDenied => StreamFault::Minor,
        // DeviceChanged (default input rerouted): rebuild anyway, the
        // rate or channel count may have changed.
        _ => StreamFault::Lost,
    }
}

fn open_error(e: cpal::Error, device: &str) -> OpenError {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => OpenError::PermissionDenied(e.to_string()),
        cpal::ErrorKind::DeviceNotAvailable => OpenError::NotFound(device.to_string()),
        _ => OpenError::Failed(e.to_string()),
    }
}

/// The device's config at 48 kHz if it has one (float preferred), else
/// its default config.
fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, cpal::Error> {
    if let Ok(ranges) = device.supported_input_configs() {
        let mut at_48k: Vec<_> = ranges.filter(|r| r.min_sample_rate() <= PREFERRED_RATE && PREFERRED_RATE <= r.max_sample_rate()).collect();
        at_48k.sort_by_key(|r| (r.sample_format() != cpal::SampleFormat::F32, r.channels()));
        if let Some(r) = at_48k.into_iter().next() {
            return Ok(r.with_sample_rate(PREFERRED_RATE));
        }
    }
    device.default_input_config()
}

fn build<T: ToF32 + cpal::SizedSample>(device: &cpal::Device, config: cpal::StreamConfig, mut sink: Sink, counters: Arc<CaptureCounters>) -> Result<cpal::Stream, cpal::Error> {
    device.build_input_stream::<T, _, _>(config, move |data: &[T], _: &cpal::InputCallbackInfo| sink.push(data), move |e: cpal::Error| counters.report(fault_of(e.kind())), None)
}

impl SampleSource for CpalSource {
    fn devices(&mut self) -> anyhow::Result<Vec<InputDevice>> {
        let default = self.host.default_input_device().and_then(|d| device_name(&d));
        let mut list: Vec<InputDevice> = Vec::new();
        for d in self.host.input_devices()? {
            if let Some(name) = device_name(&d) {
                if !list.iter().any(|x| x.name == name) {
                    list.push(InputDevice { is_default: default.as_deref() == Some(name.as_str()), name });
                }
            }
        }
        Ok(list)
    }

    fn open(&mut self, req: &OpenRequest, make_sink: &mut dyn FnMut(u32, u16) -> Sink) -> Result<Opened, OpenError> {
        if req.system {
            return self.open_system(req, make_sink);
        }
        let device = match req.device {
            None => self.host.default_input_device().ok_or(OpenError::NoDevice)?,
            Some(want) => self
                .host
                .input_devices()
                .map_err(|e| OpenError::Failed(e.to_string()))?
                .find(|d| device_name(d).as_deref() == Some(want))
                .ok_or_else(|| OpenError::NotFound(want.to_string()))?,
        };
        let name = device_name(&device).unwrap_or_else(|| "?".into());
        let supported = pick_config(&device).map_err(|e| open_error(e, &name))?;
        let mut config = supported.config();
        let frames = req.buffer_frames.clamp(MIN_BUFFER_FRAMES, MAX_BUFFER_FRAMES);
        let fixed_ok = matches!(supported.buffer_size(), cpal::SupportedBufferSize::Range { min, max } if (*min..=*max).contains(&frames));
        let mut attempts = vec![cpal::BufferSize::Default];
        if fixed_ok {
            attempts.insert(0, cpal::BufferSize::Fixed(frames));
        }
        let mut last_err = None;
        for buffer_size in attempts {
            config.buffer_size = buffer_size;
            let sink = make_sink(config.sample_rate, config.channels);
            let counters = Arc::clone(&req.counters);
            let built = match supported.sample_format() {
                cpal::SampleFormat::F32 => build::<f32>(&device, config, sink, counters),
                cpal::SampleFormat::I16 => build::<i16>(&device, config, sink, counters),
                cpal::SampleFormat::I32 => build::<i32>(&device, config, sink, counters),
                cpal::SampleFormat::U16 => build::<u16>(&device, config, sink, counters),
                other => return Err(OpenError::Failed(format!("format d'échantillon non géré : {other}"))),
            };
            match built {
                Ok(stream) => {
                    stream.play().map_err(|e| open_error(e, &name))?;
                    return Ok(Opened { stream: Box::new(stream), device: name, sample_rate: config.sample_rate, channels: config.channels });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.map(|e| open_error(e, &name)).unwrap_or(OpenError::Failed("aucune configuration".into())))
    }
}

impl CpalSource {
    /// *Son du Mac* (T-241): `cpal` records an output device by creating a
    /// private Core Audio process tap and a private aggregate device
    /// around it (public Apple API, macOS 14.6+), both destroyed when the
    /// stream is dropped. Nothing is changed in the Mac's audio settings:
    /// the stream uses the output's own format, the tap leaves the sound
    /// going to the speakers, and nothing is recorded to disk.
    #[cfg(all(feature = "system-audio", target_os = "macos"))]
    fn open_system(&mut self, req: &OpenRequest, make_sink: &mut dyn FnMut(u32, u16) -> Sink) -> Result<Opened, OpenError> {
        let support = SystemAudioSupport::detect();
        if let Some(reason) = support.reason {
            return Err(OpenError::Unavailable(reason));
        }
        let device = self.host.default_output_device().ok_or(OpenError::NoDevice)?;
        let output = device_name(&device).unwrap_or_else(|| "?".into());
        // `cpal` opens a device's own inputs when it has some (an audio
        // interface used as the output): that would be its microphones,
        // not what the Mac plays. Say so instead of capturing the wrong thing.
        if device.supported_input_configs().map(|mut c| c.next().is_some()).unwrap_or(false) {
            return Err(OpenError::Unavailable(format!("la sortie « {output} » a aussi des entrées : le studio ne peut pas en isoler le son du Mac ; passez par BlackHole")));
        }
        let supported = device.default_output_config().map_err(|e| open_error(e, &output))?;
        let mut config = supported.config();
        config.buffer_size = cpal::BufferSize::Default;
        let sink = make_sink(config.sample_rate, config.channels);
        let counters = Arc::clone(&req.counters);
        let built = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, sink, counters),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, sink, counters),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, sink, counters),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, sink, counters),
            other => return Err(OpenError::Failed(format!("format d'échantillon non géré : {other}"))),
        };
        let stream = built.map_err(|e| open_error(e, SYSTEM_AUDIO_NAME))?;
        stream.play().map_err(|e| open_error(e, SYSTEM_AUDIO_NAME))?;
        Ok(Opened { stream: Box::new(stream), device: format!("{SYSTEM_AUDIO_NAME} ({output})"), sample_rate: config.sample_rate, channels: config.channels })
    }

    /// Without the `system-audio` feature (the default build), or off macOS.
    #[cfg(not(all(feature = "system-audio", target_os = "macos")))]
    fn open_system(&mut self, _req: &OpenRequest, _make_sink: &mut dyn FnMut(u32, u16) -> Sink) -> Result<Opened, OpenError> {
        Err(OpenError::Unavailable(SystemAudioSupport::detect().reason.unwrap_or_default()))
    }
}

#[cfg(test)]
pub mod testing {
    //! A fake `SampleSource` for tests: no audio device is ever touched.

    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeState {
        pub devices: Vec<String>,
        pub default: Option<String>,
        pub sample_rate: u32,
        pub channels: u16,
        /// Next `open` is refused with this.
        pub refuse: Option<OpenError>,
        pub opens: u32,
        /// *Son du Mac*: the default output's name; `None` refuses it as
        /// unavailable (a build without the feature).
        pub system_output: Option<String>,
        /// The open stream's sink and counters, and which device it is.
        pub live: Option<(Sink, Arc<CaptureCounters>, String)>,
    }

    /// Cloneable handle; the test keeps one, the capture worker the other.
    #[derive(Clone)]
    pub struct FakeSource(pub Arc<Mutex<FakeState>>);

    /// Dropping the fake stream closes it, like a real one.
    struct FakeStream(Arc<Mutex<FakeState>>);
    impl Drop for FakeStream {
        fn drop(&mut self) {
            if let Ok(mut s) = self.0.lock() {
                s.live = None;
            }
        }
    }

    impl FakeSource {
        pub fn new(devices: &[&str]) -> Self {
            let state = FakeState {
                devices: devices.iter().map(|d| d.to_string()).collect(),
                default: devices.first().map(|d| d.to_string()),
                sample_rate: 48_000,
                channels: 2,
                ..Default::default()
            };
            Self(Arc::new(Mutex::new(state)))
        }

        /// Feeds interleaved samples through the real callback path.
        /// Returns false when no stream is open.
        pub fn feed<T: ToF32>(&self, data: &[T]) -> bool {
            let mut s = self.0.lock().unwrap();
            match s.live.as_mut() {
                Some((sink, _, _)) => {
                    sink.push(data);
                    true
                }
                None => false,
            }
        }

        /// The device disappears; an open stream on it reports itself lost.
        pub fn unplug(&self, name: &str) {
            let mut s = self.0.lock().unwrap();
            s.devices.retain(|d| d != name);
            if s.default.as_deref() == Some(name) {
                s.default = s.devices.first().cloned();
            }
            if let Some((_, counters, dev)) = &s.live {
                if dev == name {
                    counters.report(StreamFault::Lost);
                }
            }
        }

        pub fn plug(&self, name: &str) {
            let mut s = self.0.lock().unwrap();
            s.devices.push(name.to_string());
            if s.default.is_none() {
                s.default = Some(name.to_string());
            }
        }

        pub fn is_open(&self) -> bool {
            self.0.lock().unwrap().live.is_some()
        }

        pub fn opens(&self) -> u32 {
            self.0.lock().unwrap().opens
        }
    }

    impl SampleSource for FakeSource {
        fn devices(&mut self) -> anyhow::Result<Vec<InputDevice>> {
            let s = self.0.lock().unwrap();
            Ok(s.devices.iter().map(|d| InputDevice { name: d.clone(), is_default: s.default.as_ref() == Some(d) }).collect())
        }

        fn open(&mut self, req: &OpenRequest, make_sink: &mut dyn FnMut(u32, u16) -> Sink) -> Result<Opened, OpenError> {
            let mut s = self.0.lock().unwrap();
            s.opens += 1;
            if let Some(e) = s.refuse.take() {
                return Err(e);
            }
            let name = match req.device {
                _ if req.system => match &s.system_output {
                    Some(out) => format!("{SYSTEM_AUDIO_NAME} ({out})"),
                    None => return Err(OpenError::Unavailable("non compilé".into())),
                },
                None => s.default.clone().ok_or(OpenError::NoDevice)?,
                Some(want) if s.devices.iter().any(|d| d == want) => want.to_string(),
                Some(want) => return Err(OpenError::NotFound(want.to_string())),
            };
            let (rate, channels) = (s.sample_rate, s.channels);
            let sink = make_sink(rate, channels);
            s.live = Some((sink, Arc::clone(&req.counters), name.clone()));
            drop(s);
            Ok(Opened { stream: Box::new(FakeStream(Arc::clone(&self.0))), device: name, sample_rate: rate, channels })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    /// Counts allocations made by the current thread while counting is on
    /// (other test threads are not counted).
    struct CountingAlloc;

    thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    fn note_alloc() {
        let _ = COUNTING.try_with(|on| {
            if on.get() {
                let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note_alloc();
            System.alloc(layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note_alloc();
            System.alloc_zeroed(layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            note_alloc();
            System.realloc(ptr, layout, new_size)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            note_alloc();
            System.dealloc(ptr, layout)
        }
    }

    #[global_allocator]
    static GLOBAL: CountingAlloc = CountingAlloc;

    /// Allocations made by `f` on this thread (also used by the analysis tests).
    pub(crate) fn allocations_during(f: impl FnOnce()) -> usize {
        ALLOCATIONS.with(|n| n.set(0));
        COUNTING.with(|c| c.set(true));
        f();
        COUNTING.with(|c| c.set(false));
        ALLOCATIONS.with(|n| n.get())
    }

    fn drain(consumer: &mut rtrb::Consumer<f32>) -> Vec<f32> {
        std::iter::from_fn(|| consumer.pop().ok()).collect()
    }

    #[test]
    fn stereo_is_mixed_to_mono() {
        let counters = Arc::new(CaptureCounters::default());
        let (mut sink, mut out) = ring(48_000, 2, Arc::clone(&counters), Instant::now());
        sink.push(&[1.0f32, 0.0, 0.5, 0.5, -1.0, 1.0, -0.2, -0.4]);
        let mono = drain(&mut out);
        assert_eq!(mono.len(), 4);
        for (got, want) in mono.iter().zip([0.5, 0.5, 0.0, -0.3]) {
            assert!((got - want).abs() < 1e-6, "{mono:?}");
        }
        assert_eq!(counters.callbacks.load(Ordering::Relaxed), 1);
        assert!(counters.first_callback_s().is_some(), "first callback timestamped");
    }

    #[test]
    fn a_trailing_partial_frame_is_ignored() {
        let (mut sink, mut out) = ring(48_000, 3, Arc::new(CaptureCounters::default()), Instant::now());
        sink.push(&[0.3f32, 0.3, 0.3, 0.9]);
        assert_eq!(drain(&mut out), vec![0.3]);
    }

    #[test]
    fn integer_samples_convert_to_float() {
        assert_eq!(0i16.to_f32(), 0.0);
        assert_eq!(i16::MIN.to_f32(), -1.0);
        assert!((i16::MAX.to_f32() - 1.0).abs() < 1e-4);
        assert!((16_384i16.to_f32() - 0.5).abs() < 1e-6);
        assert_eq!(i32::MIN.to_f32(), -1.0);
        assert!((i32::MAX.to_f32() - 1.0).abs() < 1e-6);
        assert_eq!(32_768u16.to_f32(), 0.0);
        assert_eq!(0u16.to_f32(), -1.0);
        let (mut sink, mut out) = ring(48_000, 1, Arc::new(CaptureCounters::default()), Instant::now());
        sink.push(&[i16::MIN, 0, 16_384]);
        assert_eq!(drain(&mut out), vec![-1.0, 0.0, 0.5]);
    }

    #[test]
    fn a_full_ring_counts_overruns_and_never_blocks() {
        let counters = Arc::new(CaptureCounters::default());
        let (mut sink, mut out) = ring_with_capacity(100, 1, Arc::clone(&counters), Instant::now());
        let block = [0.25f32; 64];
        sink.push(&block);
        assert_eq!(counters.overruns.load(Ordering::Relaxed), 0);
        let t = Instant::now();
        sink.push(&block); // 28 of these don't fit
        sink.push(&block); // none fit
        assert!(t.elapsed().as_millis() < 50, "the callback never waits for the reader");
        assert_eq!(counters.overruns.load(Ordering::Relaxed), 2);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 28 + 64);
        assert_eq!(drain(&mut out).len(), 100, "the oldest samples are kept");
        sink.push(&block);
        assert_eq!(drain(&mut out).len(), 64, "room again once read");
        assert_eq!(counters.overruns.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_callback_does_not_allocate() {
        let counters = Arc::new(CaptureCounters::default());
        let (mut sink, mut out) = ring_with_capacity(1_024, 2, Arc::clone(&counters), Instant::now());
        let f: Vec<f32> = (0..1_024).map(|i| (i as f32 * 0.01).sin()).collect();
        let i: Vec<i16> = (0..512).map(|i| (i * 37) as i16).collect();
        // Sanity check that the counter works on this thread.
        assert!(allocations_during(|| drop(std::hint::black_box(vec![1u8; 16]))) > 0);
        let n = allocations_during(|| {
            sink.push(&f); // first callback: timestamp
            sink.push(&i);
            sink.push(&f); // ring full: overrun path
            sink.push(&f);
        });
        assert_eq!(n, 0, "allocations in the audio callback");
        assert!(counters.overruns.load(Ordering::Relaxed) > 0, "the overrun path ran");
        assert_eq!(drain(&mut out).len(), 1_024);
    }

    #[test]
    fn the_ltc_channel_is_copied_and_left_out_of_the_mix() {
        let tap = Arc::new(AtomicI32::new(-1));
        let (sink, mut music) = ring(48_000, 2, Arc::new(CaptureCounters::default()), Instant::now());
        let (mut sink, mut ltc) = sink.with_ltc(Arc::clone(&tap), 48_000);
        // Off: the usual mix, nothing copied.
        sink.push(&[0.25f32, 0.75]);
        assert_eq!(drain(&mut music), vec![0.5]);
        assert!(drain(&mut ltc).is_empty());
        // Channel 2: copied, and the music is channel 1 alone.
        tap.store(2, Ordering::Relaxed);
        sink.push(&[0.25f32, 0.75, 0.125, -0.75]);
        assert_eq!(drain(&mut music), vec![0.25, 0.125]);
        assert_eq!(drain(&mut ltc), vec![0.75, -0.75]);
        // The mix: both get it.
        tap.store(0, Ordering::Relaxed);
        sink.push(&[0.25f32, 0.75]);
        assert_eq!(drain(&mut music), vec![0.5]);
        assert_eq!(drain(&mut ltc), vec![0.5]);
        // A channel the device doesn't have: silence, the mix untouched.
        tap.store(5, Ordering::Relaxed);
        sink.push(&[0.25f32, 0.75]);
        assert_eq!(drain(&mut music), vec![0.5]);
        assert_eq!(drain(&mut ltc), vec![0.0]);
        // Still no allocation in the callback.
        tap.store(2, Ordering::Relaxed);
        let data = vec![0.1f32; 1024];
        assert_eq!(allocations_during(|| sink.push(&data)), 0);
    }

    #[test]
    fn backend_faults_are_flags_only() {
        let c = CaptureCounters::default();
        c.report(StreamFault::Xrun);
        c.report(StreamFault::Minor);
        assert!(!c.is_failed());
        assert_eq!(c.overruns.load(Ordering::Relaxed), 1);
        c.report(StreamFault::Lost);
        assert!(c.is_failed());
        assert_eq!(fault_of(cpal::ErrorKind::DeviceNotAvailable), StreamFault::Lost);
        assert_eq!(fault_of(cpal::ErrorKind::Xrun), StreamFault::Xrun);
    }

    #[test]
    fn macos_versions_compare_by_major_then_minor() {
        for (v, ok) in [("14.6", true), ("14.6.1", true), ("15", true), ("26.0", true), ("14.5", false), ("14.2.1", false), ("13.7", false), ("14", false), ("", false), ("abc", false), ("14.x", false)] {
            assert_eq!(version_at_least(v, SYSTEM_AUDIO_MIN_MACOS), ok, "{v:?}");
        }
    }

    #[test]
    fn the_product_version_is_read_from_the_system_plist() {
        let plist = "<dict>\n\t<key>BuildID</key>\n\t<string>X</string>\n\t<key>ProductVersion</key>\n\t<string>15.3.1</string>\n</dict>";
        assert_eq!(plist_product_version(plist).as_deref(), Some("15.3.1"));
        assert_eq!(plist_product_version("<dict></dict>"), None);
        assert_eq!(plist_product_version("<key>ProductVersion</key><string> </string>"), None);
    }

    #[test]
    fn son_du_mac_needs_the_feature_and_a_recent_macos() {
        let off = SystemAudioSupport::from_parts(false, Some("15.0"));
        assert!(!off.available && off.reason.as_deref().unwrap().contains("BlackHole"), "{off:?}");
        let old = SystemAudioSupport::from_parts(true, Some("14.5"));
        assert!(!old.available && old.reason.as_deref().unwrap().contains("14.6"), "{old:?}");
        assert!(!SystemAudioSupport::from_parts(true, None).available);
        let ok = SystemAudioSupport::from_parts(true, Some("14.6"));
        assert!(ok.available && ok.reason.is_none(), "{ok:?}");
        // This build: the default one never offers it.
        let here = SystemAudioSupport::detect();
        assert_eq!(here.compiled, cfg!(all(feature = "system-audio", target_os = "macos")));
        if !here.compiled {
            assert!(!here.available);
        }
    }

    #[test]
    fn without_the_feature_opening_son_du_mac_is_refused_before_any_device() {
        if cfg!(all(feature = "system-audio", target_os = "macos")) {
            return; // would open the real output
        }
        let counters = Arc::new(CaptureCounters::default());
        let req = OpenRequest { device: None, system: true, buffer_frames: 256, counters };
        let mut sinks = 0;
        let r = CpalSource::new().open(&req, &mut |rate, ch| {
            sinks += 1;
            ring(rate, ch, Arc::new(CaptureCounters::default()), Instant::now()).0
        });
        assert!(matches!(r, Err(OpenError::Unavailable(ref m)) if m.contains("BlackHole")), "{:?}", r.err());
        assert_eq!(sinks, 0, "no stream was built");
    }

    /// Real hardware (T-241): half a second of what the Mac plays. Play
    /// music first; macOS may ask for the system audio permission.
    /// `cargo test -p laser-studio --features system-audio real_system -- --ignored --nocapture`
    #[cfg(all(feature = "system-audio", target_os = "macos"))]
    #[test]
    #[ignore]
    fn real_system_audio() {
        let counters = Arc::new(CaptureCounters::default());
        let mut consumer = None;
        let req = OpenRequest { device: None, system: true, buffer_frames: 256, counters: Arc::clone(&counters) };
        let opened = CpalSource::new()
            .open(&req, &mut |rate, ch| {
                let (sink, c) = ring(rate, ch, Arc::clone(&counters), Instant::now());
                consumer = Some(c);
                sink
            })
            .expect("open the Mac's sound");
        println!("opened {} at {} Hz, {} ch", opened.device, opened.sample_rate, opened.channels);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let samples = drain(consumer.as_mut().unwrap());
        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!("{} samples, peak {peak}", samples.len());
        assert!(!samples.is_empty());
    }

    /// Real hardware: lists the Mac's inputs and records half a second
    /// from the default one. May trigger the microphone permission prompt.
    /// `cargo test -p laser-studio real_input -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_input_default_device() {
        let mut src = CpalSource::new();
        println!("inputs: {:?}", src.devices().unwrap());
        let counters = Arc::new(CaptureCounters::default());
        let mut consumer = None;
        let req = OpenRequest { device: None, system: false, buffer_frames: 256, counters: Arc::clone(&counters) };
        let opened = src
            .open(&req, &mut |rate, ch| {
                let (sink, c) = ring(rate, ch, Arc::clone(&counters), Instant::now());
                consumer = Some(c);
                sink
            })
            .expect("open the default input");
        println!("opened {} at {} Hz, {} ch", opened.device, opened.sample_rate, opened.channels);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let got = consumer.unwrap().slots();
        println!("{got} samples, {} callbacks", counters.callbacks.load(Ordering::Relaxed));
        assert!(got > 0);
    }
}
