//! The real-time half of native capture (T-230): what runs in the audio
//! callback, the ring it fills, and the `SampleSource` trait that opens an
//! input (CoreAudio through `cpal`, or a fake one in tests).
//!
//! The callback never allocates, locks or logs: it averages the channels
//! of each frame to mono and pushes the samples into a lock-free SPSC ring
//! (`rtrb`). A full ring drops the newest samples and counts an overrun;
//! it never waits. Errors reported by the backend only set atomic flags,
//! read by the capture thread (worker.rs).

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
        for frame in data.chunks_exact(self.channels) {
            let mut sum = 0.0f32;
            for s in frame {
                sum += s.to_f32();
            }
            if self.producer.push(sum * self.inv_channels).is_err() {
                lost += 1;
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
    (Sink { producer, channels, inv_channels: 1.0 / channels as f32, counters, epoch }, consumer)
}

/// An audio input as listed by `GET /api/audio/devices`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputDevice {
    pub name: String,
    /// The system's default input (Réglages Système › Son › Entrée).
    pub is_default: bool,
}

/// What to open.
pub struct OpenRequest<'a> {
    /// `None` = the system default input.
    pub device: Option<&'a str>,
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
    Failed(String),
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
mod tests {
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

    fn allocations_during(f: impl FnOnce()) -> usize {
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
        let req = OpenRequest { device: None, buffer_frames: 256, counters: Arc::clone(&counters) };
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
