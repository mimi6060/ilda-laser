//! Pushed state and binary frames (T-411), instead of the page polling
//! `/api/frame` in JSON 30 times a second.
//!
//! - The engine drops each finished frame here (`publish_frame`): a swap
//!   in a « latest » slot, never a queue, never a network write. Readers of
//!   `GET /api/frame.bin` wait (at most `FRAME_WAIT`) for a newer one.
//! - The state the page shows (`/api/frame` without the points) is built
//!   by web.rs at most 30 times a second, only while a stream is open, and
//!   published only when it changed (`publish_state`): at rest, nothing but
//!   the stream's keep-alive comment goes out.
//!
//! Every reader has its own thread (web.rs); a slow one skips frames and
//! versions (it always gets the latest), a stuck one blocks only its own
//! thread: readers take a copy (`Arc`) and write with no lock held.
//! Nothing here waits on a client.

use crate::patterns::Point;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// State streams open at once; the next one is answered 503.
pub const MAX_STREAMS: usize = 8;
/// `/api/frame.bin` requests waiting at once; the next one is answered 503.
pub const MAX_FRAME_WAITERS: usize = 16;
pub const FRAME_MAGIC: &[u8; 4] = b"LSF1";
/// `"LSF1"`, sequence `u32`, point count `u16`, flags `u16`.
pub const HEADER_LEN: usize = 12;
/// `x`, `y` as `i16`, then `r`, `g`, `b` as `u8`.
pub const POINT_LEN: usize = 7;
/// Longest wait for a newer frame before the current one is sent anyway.
pub const FRAME_WAIT: Duration = Duration::from_millis(100);
/// At most 30 state events a second.
pub const STATE_INTERVAL: Duration = Duration::from_millis(33);
/// The stream's comment when nothing changed for this long.
pub const KEEPALIVE: Duration = Duration::from_secs(15);
/// A frame is encoded only while someone asked for one this recently.
const FRAME_WANTED: Duration = Duration::from_secs(2);

/// Frame header flags, as the engine saw the frame.
pub mod flags {
    /// The gate was armed (and the stop not latched).
    pub const ARMED: u16 = 1;
    /// The emergency stop is latched.
    pub const ESTOP: u16 = 2;
    /// Something lit went to the output.
    pub const LIT: u16 = 4;
    /// The watchdog sees the engine running.
    pub const ENGINE_OK: u16 = 8;
    /// More points than a frame can carry: the frame has none.
    pub const TOO_BIG: u16 = 16;
}

/// One encoded frame.
#[derive(Clone)]
pub struct Frame {
    pub seq: u32,
    pub bytes: Arc<[u8]>,
}

/// The newest frame and state.
struct Latest {
    seq: u32,
    /// `None` while nobody reads frames (not encoded).
    frame: Option<Arc<[u8]>>,
    state_version: u64,
    state_json: Arc<str>,
    /// The state without what moves on its own (the beat), to tell a
    /// change from time passing.
    fingerprint: Value,
    closed: bool,
}

pub struct Hub {
    latest: Mutex<Latest>,
    wake: Condvar,
    /// State streams open (at most `MAX_STREAMS`).
    streams: AtomicUsize,
    /// `/api/frame.bin` requests in progress.
    frame_waiters: AtomicUsize,
    /// When a frame was last asked for, in ms since `epoch`.
    frame_wanted_ms: AtomicU64,
    epoch: Instant,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            latest: Mutex::new(Latest { seq: 0, frame: None, state_version: 0, state_json: Arc::from(""), fingerprint: Value::Null, closed: false }),
            wake: Condvar::new(),
            streams: AtomicUsize::new(0),
            frame_waiters: AtomicUsize::new(0),
            frame_wanted_ms: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }
}

/// What `wait_state` found.
#[derive(Debug, PartialEq)]
pub enum StateWait {
    State(u64, Arc<str>),
    /// Nothing new in time: send the keep-alive.
    Timeout,
    /// The server is stopping.
    Closed,
}

/// One open stream or frame request, counted until dropped.
pub struct OwnedSlot {
    hub: Arc<Hub>,
    frame: bool,
}

impl Drop for OwnedSlot {
    fn drop(&mut self) {
        let count = if self.frame { &self.hub.frame_waiters } else { &self.hub.streams };
        count.fetch_sub(1, Ordering::SeqCst);
    }
}

fn take(count: &AtomicUsize, max: usize) -> bool {
    count.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < max).then_some(n + 1)).is_ok()
}

impl Hub {
    fn lock(&self) -> std::sync::MutexGuard<'_, Latest> {
        // Plain data: a reader that panicked left nothing half-written.
        self.latest.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Milliseconds since `epoch`, from 1 (0 in `frame_wanted_ms` = never).
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    fn frame_wanted(&self) -> bool {
        let asked = self.frame_wanted_ms.load(Ordering::Relaxed);
        asked != 0 && self.now_ms().saturating_sub(asked) < FRAME_WANTED.as_millis() as u64
    }

    /// Called by the engine at the end of each frame: encodes it if someone
    /// reads frames, swaps it in and wakes the readers. Never waits on one.
    pub fn publish_frame(&self, points: &[Point], flags: u16) {
        // Only the engine publishes: the next sequence is known before the
        // lock is taken again, and the encoding is done outside it.
        let seq = self.lock().seq.wrapping_add(1);
        let encoded = self.frame_wanted().then(|| Arc::<[u8]>::from(encode_or_empty(seq, points, flags)));
        let mut l = self.lock();
        l.seq = seq;
        l.frame = encoded;
        drop(l);
        self.wake.notify_all();
    }

    /// The newest frame once its sequence differs from `after` (a newer
    /// one, or a restarted studio), or after `timeout` whatever it is.
    /// `None` only if no frame was ever encoded in time.
    pub fn wait_frame(&self, after: Option<u32>, timeout: Duration) -> Option<Frame> {
        self.frame_wanted_ms.store(self.now_ms(), Ordering::Relaxed);
        let deadline = Instant::now() + timeout;
        let mut l = self.lock();
        loop {
            if l.frame.is_some() && (after != Some(l.seq) || l.closed) {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || l.closed {
                break;
            }
            l = self.wake.wait_timeout(l, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        l.frame.as_ref().map(|bytes| Frame { seq: l.seq, bytes: Arc::clone(bytes) })
    }

    /// A new state from web.rs, as its fingerprint (the state without what
    /// moves by itself). Only if that changed is `json` called (with the
    /// fingerprint) and the result published (version + 1, readers woken):
    /// at rest nothing is serialized. Returns whether it was published.
    /// Only one thread publishes states.
    pub fn publish_state(&self, fingerprint: Value, json: impl FnOnce(&Value) -> String) -> bool {
        {
            let l = self.lock();
            if l.state_version > 0 && l.fingerprint == fingerprint {
                return false;
            }
        }
        let json = Arc::from(json(&fingerprint));
        let mut l = self.lock();
        l.state_version += 1;
        l.state_json = json;
        l.fingerprint = fingerprint;
        drop(l);
        self.wake.notify_all();
        true
    }

    /// The newest state once its version differs from `after`.
    pub fn wait_state(&self, after: u64, timeout: Duration) -> StateWait {
        let deadline = Instant::now() + timeout;
        let mut l = self.lock();
        loop {
            if l.closed {
                return StateWait::Closed;
            }
            if l.state_version != 0 && l.state_version != after {
                return StateWait::State(l.state_version, Arc::clone(&l.state_json));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return StateWait::Timeout;
            }
            l = self.wake.wait_timeout(l, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    pub fn seq(&self) -> u32 {
        self.lock().seq
    }

    pub fn state_version(&self) -> u64 {
        self.lock().state_version
    }

    /// A state stream slot, or `None` with `MAX_STREAMS` open. Wakes the
    /// state builder (`wait_for_streams`).
    pub fn open_stream(self: &Arc<Self>) -> Option<OwnedSlot> {
        let ok = take(&self.streams, MAX_STREAMS);
        if ok {
            // Taken under the lock, so a builder about to sleep sees it.
            drop(self.lock());
            self.wake.notify_all();
        }
        ok.then(|| OwnedSlot { hub: Arc::clone(self), frame: false })
    }

    /// A frame request slot, or `None` with `MAX_FRAME_WAITERS` waiting.
    pub fn frame_request(self: &Arc<Self>) -> Option<OwnedSlot> {
        take(&self.frame_waiters, MAX_FRAME_WAITERS).then(|| OwnedSlot { hub: Arc::clone(self), frame: true })
    }

    pub fn streams(&self) -> usize {
        self.streams.load(Ordering::SeqCst)
    }

    /// Waits (at most `timeout`) until a stream is open or the hub closes.
    /// Returns whether a stream is open.
    pub fn wait_for_streams(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut l = self.lock();
        while self.streams() == 0 && !l.closed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            l = self.wake.wait_timeout(l, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        !l.closed && self.streams() > 0
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// The server stops: every waiting reader returns.
    pub fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }
}

/// Points in a frame at most (the count is a `u16`).
pub const MAX_POINTS: usize = u16::MAX as usize;

/// The binary frame, little-endian: `"LSF1"`, `seq: u32`, `count: u16`,
/// `flags: u16`, then per point `x: i16`, `y: i16` (−32767..32767 for
/// −1..1) and `r`, `g`, `b: u8`. `None` beyond `MAX_POINTS` points.
pub fn encode_frame(seq: u32, points: &[Point], flags: u16) -> Option<Vec<u8>> {
    if points.len() > MAX_POINTS {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER_LEN + POINT_LEN * points.len());
    out.extend_from_slice(FRAME_MAGIC);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(points.len() as u16).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    let coord = |v: f32| ((if v.is_finite() { v } else { 0.0 }).clamp(-1.0, 1.0) * 32767.0).round() as i16;
    let level = |v: f32| ((if v.is_finite() { v } else { 0.0 }).clamp(0.0, 1.0) * 255.0).round() as u8;
    for p in points {
        out.extend_from_slice(&coord(p.x).to_le_bytes());
        out.extend_from_slice(&coord(p.y).to_le_bytes());
        out.extend_from_slice(&[level(p.r), level(p.g), level(p.b)]);
    }
    Some(out)
}

/// A frame too big to encode goes out empty, flagged `TOO_BIG`.
fn encode_or_empty(seq: u32, points: &[Point], flags: u16) -> Vec<u8> {
    encode_frame(seq, points, flags).unwrap_or_else(|| encode_frame(seq, &[], flags | flags::TOO_BIG).expect("no points fit"))
}

/// The other way (tests, tools): `(seq, flags, points)`.
#[cfg(test)]
pub fn decode_frame(bytes: &[u8]) -> Option<(u32, u16, Vec<Point>)> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != FRAME_MAGIC {
        return None;
    }
    let seq = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let count = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as usize;
    let flags = u16::from_le_bytes(bytes[10..12].try_into().ok()?);
    let body = &bytes[HEADER_LEN..];
    if body.len() != count * POINT_LEN {
        return None;
    }
    let points = body
        .as_chunks::<POINT_LEN>()
        .0
        .iter()
        .map(|c| {
            let coord = |i: usize| i16::from_le_bytes([c[i], c[i + 1]]) as f32 / 32767.0;
            Point { x: coord(0), y: coord(2), r: c[4] as f32 / 255.0, g: c[5] as f32 / 255.0, b: c[6] as f32 / 255.0 }
        })
        .collect();
    Some((seq, flags, points))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(n: usize) -> Vec<Point> {
        (0..n).map(|i| {
            let a = i as f32 * 0.37;
            Point { x: a.sin(), y: (a * 1.3).cos(), r: (i % 7) as f32 / 6.0, g: 0.5, b: if i % 3 == 0 { 0.0 } else { 1.0 } }
        }).collect()
    }

    #[test]
    fn frames_round_trip_within_a_sixteen_thousandth() {
        let pts = points(500);
        let bytes = encode_frame(42, &pts, flags::ARMED | flags::LIT).unwrap();
        let (seq, f, back) = decode_frame(&bytes).unwrap();
        assert_eq!((seq, f, back.len()), (42, flags::ARMED | flags::LIT, 500));
        for (a, b) in pts.iter().zip(&back) {
            // The field is 2 wide (−1..1).
            assert!((a.x - b.x).abs() * 2.0 < 2.0 / 16_000.0, "{a:?} {b:?}");
            assert!((a.y - b.y).abs() * 2.0 < 2.0 / 16_000.0);
            for (c, d) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                assert!((c - d).abs() <= 0.5 / 255.0 + 1e-6);
            }
            assert_eq!(a.is_lit(), b.is_lit(), "blanked stays blanked");
        }
        // Out of range and NaN are clamped, never wrapped.
        let odd = [Point { x: 3.0, y: -9.0, r: 2.0, g: -1.0, b: f32::NAN }];
        let (_, _, back) = decode_frame(&encode_frame(1, &odd, 0).unwrap()).unwrap();
        assert_eq!(back[0], Point { x: 1.0, y: -1.0, r: 1.0, g: 0.0, b: 0.0 });
    }

    #[test]
    fn empty_and_oversized_frames() {
        let bytes = encode_frame(7, &[], 0).unwrap();
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(decode_frame(&bytes).unwrap().2, vec![]);
        assert!(encode_frame(1, &vec![Point::blanked(0.0, 0.0); MAX_POINTS + 1], 0).is_none(), "65 536 points don't fit");
        let max = encode_frame(1, &vec![Point::blanked(0.0, 0.0); MAX_POINTS], 0).unwrap();
        assert_eq!(decode_frame(&max).unwrap().2.len(), MAX_POINTS);
        // Too big: published empty and flagged, never truncated silently.
        let (_, f, back) = decode_frame(&encode_or_empty(3, &vec![Point::blanked(0.0, 0.0); MAX_POINTS + 1], flags::ARMED)).unwrap();
        assert_eq!((f, back.len()), (flags::ARMED | flags::TOO_BIG, 0));
        // Damaged input is refused.
        assert!(decode_frame(&bytes[..11]).is_none());
        assert!(decode_frame(b"LSF2\0\0\0\0\0\0\0\0").is_none());
        let mut short = encode_frame(1, &points(2), 0).unwrap();
        short.pop();
        assert!(decode_frame(&short).is_none());
    }

    #[test]
    fn three_thousand_points_fit_in_forty_kilobytes() {
        let bytes = encode_frame(1, &points(3000), 0).unwrap();
        assert_eq!(bytes.len(), 21_012);
        assert!(bytes.len() < 40_000);
    }

    #[test]
    fn frames_are_encoded_only_while_someone_reads_them() {
        let hub = Hub::default();
        hub.publish_frame(&points(10), 0);
        assert_eq!(hub.seq(), 1);
        assert!(hub.wait_frame(Some(0), Duration::ZERO).is_none(), "nobody asked: nothing encoded");
        // The ask above marks frames as wanted: the next one is encoded.
        hub.publish_frame(&points(10), flags::ENGINE_OK);
        let f = hub.wait_frame(Some(0), Duration::ZERO).unwrap();
        assert_eq!(f.seq, 2);
        assert_eq!(decode_frame(&f.bytes).unwrap().0, 2, "the header carries the slot's sequence");
    }

    #[test]
    fn wait_frame_wakes_on_publish_and_times_out_at_its_limit() {
        let hub = Arc::new(Hub::default());
        hub.wait_frame(Some(0), Duration::ZERO);
        hub.publish_frame(&points(3), 0);
        // Nothing newer than 1: the current one after the timeout.
        let t0 = Instant::now();
        let f = hub.wait_frame(Some(1), FRAME_WAIT).unwrap();
        let waited = t0.elapsed();
        assert_eq!(f.seq, 1);
        assert!(waited >= FRAME_WAIT && waited < FRAME_WAIT + Duration::from_millis(80), "{waited:?}");
        // A publish wakes the waiter at once.
        let waiter = std::thread::spawn({
            let hub = Arc::clone(&hub);
            move || {
                let t0 = Instant::now();
                (hub.wait_frame(Some(1), Duration::from_secs(5)).unwrap().seq, t0.elapsed())
            }
        });
        std::thread::sleep(Duration::from_millis(20));
        hub.publish_frame(&points(3), 0);
        let (seq, waited) = waiter.join().unwrap();
        assert_eq!(seq, 2);
        assert!(waited < Duration::from_secs(1), "{waited:?}");
        // A restarted studio's numbers are lower: still "newer".
        assert_eq!(hub.wait_frame(Some(900), Duration::ZERO).unwrap().seq, 2);
    }

    #[test]
    fn publishing_never_waits_for_readers_even_a_stuck_one() {
        let hub = Arc::new(Hub::default());
        hub.wait_frame(Some(0), Duration::ZERO);
        // Eight readers, one of which never comes back to read.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut readers = Vec::new();
        for i in 0..8 {
            let (hub, stop) = (Arc::clone(&hub), Arc::clone(&stop));
            readers.push(std::thread::spawn(move || {
                let mut seq = 0;
                while !stop.load(Ordering::SeqCst) {
                    if let Some(f) = hub.wait_frame(Some(seq), Duration::from_millis(50)) {
                        seq = f.seq;
                        if i == 0 {
                            // Holds its copy and never asks again.
                            std::thread::sleep(Duration::from_millis(400));
                        }
                    }
                }
            }));
        }
        let pts = points(3000);
        let mut times = Vec::new();
        for _ in 0..100 {
            let t0 = Instant::now();
            hub.publish_frame(&pts, 0);
            times.push(t0.elapsed());
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::SeqCst);
        for r in readers {
            r.join().unwrap();
        }
        times.sort();
        // Under 1 ms (a debug build, 3 000 points encoded); the worst far
        // from the stuck reader's 400 ms, whatever else the machine runs.
        assert!(times[50] < Duration::from_millis(1), "median publish {:?}", times[50]);
        assert!(times[99] < Duration::from_millis(50), "worst publish {:?}", times[99]);
    }

    #[test]
    fn states_are_published_only_when_they_change() {
        let hub = Hub::default();
        assert_eq!(hub.wait_state(0, Duration::ZERO), StateWait::Timeout, "nothing yet");
        let json = |v: &Value| v.to_string();
        assert!(hub.publish_state(serde_json::json!({ "a": 1 }), json));
        assert_eq!(hub.wait_state(0, Duration::ZERO), StateWait::State(1, Arc::from("{\"a\":1}")));
        // Same fingerprint (only the beat moved): no new version, nothing serialized.
        assert!(!hub.publish_state(serde_json::json!({ "a": 1 }), |_| unreachable!("not serialized at rest")));
        assert_eq!(hub.wait_state(1, Duration::from_millis(20)), StateWait::Timeout);
        assert!(hub.publish_state(serde_json::json!({ "a": 2 }), |v| format!("{v}+beat")));
        assert_eq!(hub.wait_state(1, Duration::ZERO), StateWait::State(2, Arc::from("{\"a\":2}+beat")));
        assert_eq!(hub.state_version(), 2);
        hub.close();
        assert_eq!(hub.wait_state(2, Duration::from_secs(5)), StateWait::Closed);
        assert!(!hub.wait_for_streams(Duration::from_secs(5)));
    }

    #[test]
    fn streams_are_bounded_and_counted_until_dropped() {
        let hub = Arc::new(Hub::default());
        let slots: Vec<_> = (0..MAX_STREAMS).map(|_| hub.open_stream().expect("a slot")).collect();
        assert!(hub.open_stream().is_none(), "the ninth is refused");
        assert!(hub.wait_for_streams(Duration::ZERO));
        drop(slots);
        assert_eq!(hub.streams(), 0);
        let one = hub.open_stream().unwrap();
        assert_eq!(hub.streams(), 1);
        drop(one);
        assert!(!hub.wait_for_streams(Duration::from_millis(10)));
        let frames: Vec<_> = (0..MAX_FRAME_WAITERS).map(|_| hub.frame_request().unwrap()).collect();
        assert!(hub.frame_request().is_none());
        drop(frames);
        assert!(hub.frame_request().is_some());
    }
}
