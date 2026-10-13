//! Native ShowNET output (T-471): the foundation, compiled only with the
//! `shownet` Cargo feature (off by default). Without Laserworld's SDK it
//! drives nothing: the default build has no ShowNET output at all, and a
//! `shownet` build lists one only once a real binding is installed.
//!
//! Everything that does not depend on the SDK lives here, in our own terms:
//! the conversion of our normalised points at the very edge (12-bit
//! position, 8-bit colour), the points-per-second cap, a bounded queue
//! between the engine and the sending thread, a watchdog, and the same
//! arm / disarm / blank / kill-switch / fault behaviour as `DacOutput`.
//! What the SDK does is reached only through `ShowNetBackend` and
//! `ShowNetLink`, two small traits that are *ours*: they say what the
//! studio needs (list the boxes, open one, send a frame, read the link
//! state, close), not how Laserworld's API looks. Nothing here guesses
//! that API, and nothing here opens a socket: in the repository the only
//! implementation is the fake one the tests use.
//!
//! # Adding the real binding (when the SDK arrives, under NDA)
//!
//! The SDK (library, headers, documentation, keys) never enters git. The
//! binding is one Rust file that implements `ShowNetBackend` and
//! `ShowNetLink` on top of the SDK; it lives with the SDK outside the
//! repository (or in the gitignored `studio/shownet-sdk/` folder).
//! The plan, to be finalised once the header is known:
//! - a second feature, `shownet-sdk` (implies `shownet`), pulls that file
//!   in at compile time with `include!` from a directory named by an
//!   environment variable (`SHOWNET_SDK_DIR`), so no path to it is ever
//!   written in a tracked file;
//! - a build script, active only with `shownet-sdk`, reads the same
//!   variable to tell the linker where the SDK library is;
//! - at start-up `main.rs` calls `install_backend` with the binding's
//!   backend. Until then `installed_backend()` is `None`, the output
//!   manager lists no ShowNET, and opening one fails with a clear message.
//!
//! The binding then only maps our calls to the SDK's, adapting
//! `ShowNetPoint` if the box wants another layout or axis direction.
//! Keys or licence files the SDK needs are read from that same private
//! directory at run time, never from `outputs.json` or a project.
//!
//! # Safety
//! - Arming only lets light through: whether a frame leaves lit is decided
//!   on the sending thread, point by point, right before the frame goes to
//!   the link. Disarmed (or after the kill switch), every point leaves with
//!   its colours at 0, so no lit frame reaches the backend.
//! - The box's behaviour when the stream stops is not documented (it may
//!   hold its last frame): if the engine supplies nothing for `WATCHDOG`,
//!   the sending thread sends a dark frame on its own and disarms. If the
//!   output was armed it then reports a fault, so the engine disarms with
//!   « Sortie perdue » and the output manager reconnects, disarmed (T-401).
//! - A lost link (or a failed send) disarms at once and reports a fault the
//!   same way. Swapping or closing the output goes through
//!   `OutputStage::replace`, which disarms and sends dark frames first;
//!   closing here also ends with a dark frame.
//! - `kind()` is "shownet", which the pre-show checklist (T-472) counts as
//!   a ShowNET output (hardware stop reminders, `hw_stop` interlock).

use crate::devices::DeviceInfo;
use crate::output::Output;
use crate::patterns::Point;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The studio's own ceiling on the point rate, whatever the box says.
pub const MAX_PPS: u32 = 100_000;
/// The engine stalled this long: dark frame and disarm.
pub const WATCHDOG: Duration = Duration::from_millis(100);
/// `Output::kind` and `DeviceInfo::kind` of a ShowNET output.
pub const KIND: &str = "shownet";
/// Output manager ids: `shownet:<device id>`, or `shownet:auto` for the
/// first box found.
pub const ID_PREFIX: &str = "shownet:";
pub const AUTO: &str = "auto";
/// Largest 12-bit position.
const XY_MAX: f32 = 4095.0;
/// Bounds of `ShowNetConfig::queue_frames`: at least two, so a dark frame
/// is never the one evicted (see `Pump::evict`).
const QUEUE_MIN: usize = 2;
const QUEUE_MAX: usize = 16;
/// How often the idle sending thread wakes to run the watchdog.
const TICK: Duration = Duration::from_millis(5);

/// A ShowNET output's settings. The device and the rate come from the
/// output manager's `connection` (`selected`, `pps` in `outputs.json`,
/// shared with T-401). Never a key or an SDK path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowNetConfig {
    /// None: the first box found.
    pub device_id: Option<String>,
    pub pps: u32,
    /// Frames waiting between the engine and the sending thread.
    pub queue_frames: usize,
}

impl Default for ShowNetConfig {
    fn default() -> Self {
        Self { device_id: None, pps: 30_000, queue_frames: 4 }
    }
}

impl ShowNetConfig {
    /// From an output manager id (`shownet:auto`, `shownet:<id>`); None
    /// for any other output.
    pub fn from_id(id: &str, pps: u32) -> Option<Self> {
        let device = id.strip_prefix(ID_PREFIX)?;
        let device_id = (device != AUTO && !device.is_empty()).then(|| device.to_string());
        Some(Self { device_id, pps, ..Default::default() })
    }
}

/// One point as it leaves for the box: position 0..4095 on each axis,
/// colours 0..255. Our layout; the binding adapts it to the SDK's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShowNetPoint {
    pub x: u16,
    pub y: u16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[cfg(test)]
impl ShowNetPoint {
    pub fn is_lit(&self) -> bool {
        self.r > 0 || self.g > 0 || self.b > 0
    }
}

/// The edge conversion. Positions saturate to -1..1 then map to 0..4095
/// (-1 → 0, 0 → 2048, 1 → 4095); colours saturate to 0..1 then map to
/// 0..255. A blanked point, or any point while `lit_allowed` is false,
/// leaves with its colours at 0. A non-finite coordinate leaves dark at
/// the centre.
pub fn to_shownet_point(p: &Point, lit_allowed: bool) -> ShowNetPoint {
    if !p.x.is_finite() || !p.y.is_finite() {
        return ShowNetPoint { x: 2048, y: 2048, ..Default::default() };
    }
    let pos = |v: f32| ((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * XY_MAX).round() as u16;
    // NaN colours become 0 (`as` saturates NaN to 0).
    let col = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = ShowNetPoint { x: pos(p.x), y: pos(p.y), ..Default::default() };
    if lit_allowed && p.is_lit() {
        (out.r, out.g, out.b) = (col(p.r), col(p.g), col(p.b));
    }
    out
}

/// The rate actually used: the request, capped at the box's limit and at
/// `MAX_PPS` (a box that reports 0 gets `MAX_PPS`), with a warning when
/// capped.
pub fn clamp_pps(requested: u32, device_max: u32) -> u32 {
    let cap = if device_max == 0 { MAX_PPS } else { device_max.min(MAX_PPS) };
    if requested > cap {
        log::warn!("ShowNET : {requested} points par seconde demandés, ramenés à {cap}");
    }
    requested.clamp(1, cap)
}

/// One ShowNET box, as the backend lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowNetDevice {
    /// Stable id the operator selects (`shownet:<id>`).
    pub id: String,
    pub name: String,
    /// The box's point rate limit (0: unknown).
    pub max_pps: u32,
}

/// The state of an open link. Built by the backends (the SDK binding is
/// outside the repository, hence the `allow` in non-test builds).
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    Ready,
    /// Gone (unplugged, rebooted, taken by another program): why, in French.
    Lost(String),
}

/// What the studio needs from the ShowNET driver, in our terms. The real
/// implementation wraps Laserworld's SDK and lives outside git (see the
/// module docs); tests use the fake one.
pub trait ShowNetBackend: Send + Sync {
    /// The boxes that can be opened. May take a while: never called on the
    /// engine thread.
    fn enumerate(&self) -> Result<Vec<ShowNetDevice>>;
    /// Opens a box. It must emit nothing until it gets a frame.
    fn open(&self, device: &ShowNetDevice) -> Result<Box<dyn ShowNetLink>>;
}

/// An open box. Only the output's sending thread calls it.
pub trait ShowNetLink: Send {
    /// Sends one frame, already converted, to be drawn at `pps`. An error
    /// is treated as a lost link.
    fn send_frame(&mut self, points: &[ShowNetPoint], pps: u32) -> Result<()>;
    /// Cheap and non-blocking: checked after every frame and tick.
    fn status(&self) -> LinkStatus;
    /// Releases the box. Called once, after the last (dark) frame.
    fn close(&mut self);
}

/// The binding installed at start-up, if any (see the module docs).
static BACKEND: OnceLock<Arc<dyn ShowNetBackend>> = OnceLock::new();

/// Installs the real binding; false if one already is. Only the private
/// SDK binding calls this, so nothing in the repository does yet.
#[allow(dead_code)]
pub fn install_backend(backend: Arc<dyn ShowNetBackend>) -> bool {
    BACKEND.set(backend).is_ok()
}

/// The installed binding (None in every build from the repository alone).
pub fn installed_backend() -> Option<Arc<dyn ShowNetBackend>> {
    BACKEND.get().cloned()
}

/// What the output manager lists for a backend: the "first box found"
/// entry, then the boxes it enumerates.
pub fn fixed_entry() -> DeviceInfo {
    DeviceInfo::new(KIND.into(), format!("{ID_PREFIX}{AUTO}"), "ShowNET (premier trouvé)".into())
}

pub fn scan(backend: &dyn ShowNetBackend) -> Result<Vec<DeviceInfo>> {
    Ok(backend
        .enumerate()?
        .into_iter()
        .map(|d| DeviceInfo::new(KIND.into(), format!("{ID_PREFIX}{}", d.id), format!("ShowNET {}", d.name)))
        .collect())
}

/// Opens output manager id `id` (`shownet:...`) at `pps`, disarmed.
pub fn open_id(backend: Option<&dyn ShowNetBackend>, id: &str, pps: u32) -> Result<Box<dyn Output>> {
    let Some(backend) = backend else {
        bail!("ShowNET : cette version du studio n'a pas le SDK Laserworld (voir l'aide)");
    };
    let config = ShowNetConfig::from_id(id, pps).with_context(|| format!("Sortie ShowNET inconnue : « {id} »"))?;
    Ok(Box::new(ShowNetOutput::open(backend, &config)?))
}

/// A frame waiting to leave.
struct Queued {
    points: Vec<Point>,
    dark: bool,
}

/// What the sending thread does next.
#[derive(Debug, PartialEq)]
enum Next {
    Frame(Vec<Point>),
    /// The engine has been silent for `WATCHDOG`: dark frame, disarm.
    Watchdog,
    Idle,
}

/// The bounded queue between the engine and the sending thread, with the
/// watchdog's clock. Pure logic, driven with explicit instants so tests can
/// simulate time.
struct Pump {
    queue: VecDeque<Queued>,
    cap: usize,
    /// When the engine last supplied a frame (None: not yet, no watchdog).
    last_push: Option<Instant>,
    /// The watchdog fired for the current silence (fires once per silence).
    starved: bool,
}

impl Pump {
    fn new(cap: usize) -> Self {
        Self { queue: VecDeque::new(), cap: cap.clamp(QUEUE_MIN, QUEUE_MAX), last_push: None, starved: false }
    }

    /// A frame from the engine. Full: the oldest frame goes, never a dark
    /// one unless the dark frame right after it stays (two dark frames in a
    /// row draw the same nothing).
    fn push(&mut self, points: Vec<Point>, now: Instant) {
        let dark = !points.iter().any(Point::is_lit);
        while self.queue.len() >= self.cap {
            self.evict();
        }
        self.queue.push_back(Queued { points, dark });
        self.last_push = Some(now);
        self.starved = false;
    }

    /// `blank_now`: the frames still waiting are stale, the lit ones go;
    /// a dark frame goes out next.
    fn push_dark(&mut self, now: Instant) {
        self.queue.retain(|q| q.dark);
        self.push(vec![Point::blanked(0.0, 0.0)], now);
    }

    fn evict(&mut self) {
        let i = self
            .queue
            .iter()
            .position(|q| !q.dark)
            .or_else(|| (0..self.queue.len().saturating_sub(1)).find(|&i| self.queue[i].dark && self.queue[i + 1].dark))
            .unwrap_or(0);
        self.queue.remove(i);
    }

    fn next(&mut self, now: Instant) -> Next {
        if let Some(q) = self.queue.pop_front() {
            return Next::Frame(q.points);
        }
        match self.last_push {
            Some(t) if !self.starved && now.saturating_duration_since(t) >= WATCHDOG => {
                self.starved = true;
                Next::Watchdog
            }
            _ => Next::Idle,
        }
    }
}

/// State shared by the output, its kill switch and its sending thread.
struct Shared {
    pump: Mutex<Pump>,
    wake: Condvar,
    /// Read on the sending thread right before each frame is converted.
    armed: AtomicBool,
    closing: AtomicBool,
    fault: Mutex<Option<String>>,
}

impl Shared {
    /// The link is gone: disarm first, then report.
    fn lose(&self, why: String) {
        self.armed.store(false, Ordering::SeqCst);
        let mut fault = self.fault.lock().unwrap_or_else(|e| e.into_inner());
        if fault.is_none() {
            log::warn!("{why}");
            *fault = Some(why);
        }
    }
}

/// `Output` for a ShowNET box through a `ShowNetBackend`.
pub struct ShowNetOutput {
    name: String,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl ShowNetOutput {
    /// Finds and opens the box `config` names, disarmed, and starts the
    /// sending thread.
    pub fn open(backend: &dyn ShowNetBackend, config: &ShowNetConfig) -> Result<Self> {
        let devices = backend.enumerate().context("recherche des ShowNET impossible")?;
        let device = match &config.device_id {
            None => devices.first().context("Aucun ShowNET trouvé")?,
            Some(id) => devices.iter().find(|d| &d.id == id).with_context(|| format!("ShowNET « {id} » introuvable"))?,
        };
        let pps = clamp_pps(config.pps, device.max_pps);
        let link = backend.open(device).with_context(|| format!("ouverture du ShowNET {} impossible", device.name))?;
        let name = format!("ShowNET {} ({})", device.name, device.id);
        let shared = Arc::new(Shared {
            pump: Mutex::new(Pump::new(config.queue_frames)),
            wake: Condvar::new(),
            armed: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            fault: Mutex::new(None),
        });
        let thread = {
            let (shared, name) = (Arc::clone(&shared), name.clone());
            std::thread::Builder::new().name("shownet-out".into()).spawn(move || run(&shared, link, pps, &name))?
        };
        Ok(Self { name, shared, thread: Some(thread) })
    }

    fn enqueue(&self, f: impl FnOnce(&mut Pump)) {
        f(&mut self.shared.pump.lock().unwrap_or_else(|e| e.into_inner()));
        self.shared.wake.notify_one();
    }
}

/// Converts and sends one frame; lit only if armed at this very moment.
fn send(shared: &Shared, link: &mut dyn ShowNetLink, points: &[Point], pps: u32, buf: &mut Vec<ShowNetPoint>) -> Result<()> {
    let armed = shared.armed.load(Ordering::SeqCst);
    buf.clear();
    buf.extend(points.iter().map(|p| to_shownet_point(p, armed)));
    if buf.is_empty() {
        buf.push(to_shownet_point(&Point::blanked(0.0, 0.0), false));
    }
    link.send_frame(buf, pps)
}

/// The sending thread: frames in order, the watchdog when idle, the link
/// state after each step; on closing, the queue is flushed and a last dark
/// frame sent before the link closes.
fn run(shared: &Shared, mut link: Box<dyn ShowNetLink>, pps: u32, name: &str) {
    let mut buf = Vec::new();
    let dark = [Point::blanked(0.0, 0.0)];
    let mut alive = true;
    loop {
        let next = {
            let mut pump = shared.pump.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let next = pump.next(Instant::now());
                if next != Next::Idle || shared.closing.load(Ordering::SeqCst) {
                    break next;
                }
                pump = shared.wake.wait_timeout(pump, TICK).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        let sent = match next {
            Next::Frame(points) => send(shared, &mut *link, &points, pps, &mut buf),
            Next::Watchdog => {
                let was_armed = shared.armed.swap(false, Ordering::SeqCst);
                log::warn!("ShowNET : plus de trame depuis {} ms, trame noire et désarmement", WATCHDOG.as_millis());
                let sent = send(shared, &mut *link, &dark, pps, &mut buf);
                if was_armed {
                    shared.lose(format!("{name} : moteur bloqué, trame noire envoyée et laser désarmé"));
                }
                sent
            }
            Next::Idle => break, // closing, queue flushed
        };
        if let Err(e) = sent {
            shared.lose(format!("{name} ne répond plus : {e:#}"));
            alive = false;
            break;
        }
        if let LinkStatus::Lost(why) = link.status() {
            shared.lose(format!("{name} ne répond plus : {why}"));
            alive = false;
            break;
        }
    }
    if alive {
        shared.armed.store(false, Ordering::SeqCst);
        let _ = send(shared, &mut *link, &dark, pps, &mut buf);
    }
    link.close();
}

impl Output for ShowNetOutput {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> &'static str {
        KIND
    }

    fn set_armed(&mut self, armed: bool) -> Result<()> {
        if armed {
            if let Some(why) = self.fault() {
                bail!("{why}");
            }
        }
        self.shared.armed.store(armed, Ordering::SeqCst);
        Ok(())
    }

    fn send(&mut self, points: &[Point]) {
        let now = Instant::now();
        self.enqueue(|pump| pump.push(points.to_vec(), now));
    }

    /// Drops the lit frames still waiting and sends a dark one next.
    fn blank_now(&mut self) -> Result<()> {
        let now = Instant::now();
        self.enqueue(|pump| pump.push_dark(now));
        Ok(())
    }

    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let shared = Arc::clone(&self.shared);
        Some(Box::new(move || {
            shared.armed.store(false, Ordering::SeqCst);
            shared.wake.notify_one();
        }))
    }

    fn fault(&self) -> Option<String> {
        self.shared.fault.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for ShowNetOutput {
    /// Disarms, lets the thread flush the queue and send a last dark frame,
    /// then waits for it to close the link.
    fn drop(&mut self) {
        self.shared.armed.store(false, Ordering::SeqCst);
        self.shared.closing.store(true, Ordering::SeqCst);
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The fake ShowNET the tests drive: it records every frame instead of
/// sending anything anywhere.
#[cfg(test)]
pub mod fake {
    use super::*;

    /// What the fake saw, in order.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Event {
        Open(String),
        Frame { points: Vec<ShowNetPoint>, pps: u32 },
        Close,
    }

    #[derive(Clone)]
    pub struct FakeShowNet {
        pub devices: Vec<ShowNetDevice>,
        pub events: Arc<Mutex<Vec<Event>>>,
        /// Set: the open links report `Lost`.
        pub lost: Arc<AtomicBool>,
        /// While true, `send_frame` blocks (a slow box).
        pub hold: Arc<(Mutex<bool>, Condvar)>,
    }

    impl FakeShowNet {
        pub fn new(devices: &[(&str, u32)]) -> Self {
            Self {
                devices: devices.iter().map(|(id, max)| ShowNetDevice { id: id.to_string(), name: format!("Fake {id}"), max_pps: *max }).collect(),
                events: Arc::default(),
                lost: Arc::default(),
                hold: Arc::default(),
            }
        }

        pub fn set_hold(&self, on: bool) {
            *self.hold.0.lock().unwrap() = on;
            self.hold.1.notify_all();
        }

        pub fn frames(&self) -> Vec<Vec<ShowNetPoint>> {
            self.events.lock().unwrap().iter().filter_map(|e| if let Event::Frame { points, .. } = e { Some(points.clone()) } else { None }).collect()
        }

        /// Waits (up to 2 s) until `f` holds on the events.
        pub fn wait_for(&self, f: impl Fn(&[Event]) -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if f(&self.events.lock().unwrap()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            false
        }
    }

    impl ShowNetBackend for FakeShowNet {
        fn enumerate(&self) -> Result<Vec<ShowNetDevice>> {
            Ok(self.devices.clone())
        }
        fn open(&self, device: &ShowNetDevice) -> Result<Box<dyn ShowNetLink>> {
            if self.lost.load(Ordering::SeqCst) {
                bail!("{} injoignable (simulation)", device.name);
            }
            self.events.lock().unwrap().push(Event::Open(device.id.clone()));
            Ok(Box::new(FakeLink { events: Arc::clone(&self.events), lost: Arc::clone(&self.lost), hold: Arc::clone(&self.hold) }))
        }
    }

    struct FakeLink {
        events: Arc<Mutex<Vec<Event>>>,
        lost: Arc<AtomicBool>,
        hold: Arc<(Mutex<bool>, Condvar)>,
    }

    impl ShowNetLink for FakeLink {
        fn send_frame(&mut self, points: &[ShowNetPoint], pps: u32) -> Result<()> {
            let (lock, cv) = &*self.hold;
            let mut held = lock.lock().unwrap();
            while *held {
                held = cv.wait(held).unwrap();
            }
            self.events.lock().unwrap().push(Event::Frame { points: points.to_vec(), pps });
            Ok(())
        }
        fn status(&self) -> LinkStatus {
            if self.lost.load(Ordering::SeqCst) {
                LinkStatus::Lost("câble débranché (simulation)".into())
            } else {
                LinkStatus::Ready
            }
        }
        fn close(&mut self) {
            self.events.lock().unwrap().push(Event::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Event, FakeShowNet};
    use super::*;
    use crate::interlock::EStop;
    use crate::output::OutputStage;

    fn lit() -> Vec<Point> {
        vec![Point::lit(-0.5, 0.5, 1.0, 0.0, 0.0), Point::lit(0.5, -0.5, 0.0, 1.0, 1.0)]
    }

    fn open(fake: &FakeShowNet) -> ShowNetOutput {
        ShowNetOutput::open(fake, &ShowNetConfig::default()).unwrap()
    }

    #[test]
    fn positions_map_to_twelve_bits_and_saturate() {
        let at = |x: f32, y: f32| {
            let p = to_shownet_point(&Point::lit(x, y, 1.0, 1.0, 1.0), true);
            (p.x, p.y)
        };
        assert_eq!(at(-1.0, 1.0), (0, 4095));
        let (cx, _) = at(0.0, 0.0);
        assert!(cx == 2047 || cx == 2048, "{cx}");
        assert_eq!(at(-7.0, 3.5), (0, 4095), "out of range: saturated");
        assert_eq!(at(f32::NEG_INFINITY, 0.0), (2048, 2048), "non-finite: centre");
        assert!(!to_shownet_point(&Point::lit(f32::NAN, 0.0, 1.0, 1.0, 1.0), true).is_lit(), "and dark");
    }

    #[test]
    fn colours_map_to_eight_bits_and_saturate() {
        let p = to_shownet_point(&Point::lit(0.0, 0.0, 1.0, 0.5, 2.0), true);
        assert_eq!((p.r, p.g, p.b), (255, 128, 255));
        let p = to_shownet_point(&Point::lit(0.0, 0.0, -1.0, f32::NAN, 0.25), true);
        assert_eq!((p.r, p.g, p.b), (0, 0, 64));
        assert!(!to_shownet_point(&Point::blanked(0.3, 0.3), true).is_lit());
        assert!(!to_shownet_point(&Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), false).is_lit(), "disarmed: colours at 0");
    }

    #[test]
    fn the_rate_is_capped_at_the_box_and_at_100_kpps() {
        assert_eq!(clamp_pps(150_000, 0), 100_000);
        assert_eq!(clamp_pps(150_000, 200_000), 100_000);
        assert_eq!(clamp_pps(60_000, 40_000), 40_000);
        assert_eq!(clamp_pps(30_000, 100_000), 30_000);
        assert_eq!(clamp_pps(0, 100_000), 1);
    }

    #[test]
    fn config_comes_from_the_output_id() {
        assert_eq!(ShowNetConfig::from_id("shownet:auto", 40_000), Some(ShowNetConfig { device_id: None, pps: 40_000, queue_frames: 4 }));
        assert_eq!(ShowNetConfig::from_id("shownet:box-1", 30_000).unwrap().device_id.as_deref(), Some("box-1"));
        assert_eq!(ShowNetConfig::from_id("etherdream:1", 30_000), None);
        let c: ShowNetConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, ShowNetConfig::default());
    }

    #[test]
    fn a_full_queue_drops_the_oldest_lit_frames_never_the_dark_ones() {
        let t = Instant::now();
        let mut pump = Pump::new(4);
        let lit_at = |x: f32| vec![Point::lit(x, 0.0, 1.0, 1.0, 1.0)];
        pump.push(lit_at(0.1), t);
        pump.push_dark(t); // drops the lit 0.1
        pump.push(lit_at(0.2), t);
        pump.push(lit_at(0.3), t);
        pump.push(lit_at(0.4), t);
        pump.push(lit_at(0.5), t); // full: 0.2 goes
        let out: Vec<_> = std::iter::from_fn(|| match pump.next(t) {
            Next::Frame(f) => Some(f),
            _ => None,
        })
        .collect();
        assert_eq!(out.len(), 4);
        assert!(!out[0][0].is_lit(), "the dark frame stayed, first");
        assert_eq!(out[1..].iter().map(|f| f[0].x).collect::<Vec<_>>(), [0.3, 0.4, 0.5]);
        // All dark: one of two adjacent dark frames goes, a dark frame
        // still precedes the lit one.
        for _ in 0..4 {
            pump.push_dark(t);
        }
        pump.push(lit_at(0.9), t);
        let out: Vec<_> = std::iter::from_fn(|| match pump.next(t) {
            Next::Frame(f) => Some(f),
            _ => None,
        })
        .collect();
        assert_eq!(out.len(), 4);
        assert!(out[..3].iter().all(|f| !f[0].is_lit()) && out[3][0].is_lit());
    }

    #[test]
    fn the_watchdog_fires_once_after_100_ms_of_silence() {
        let t0 = Instant::now();
        let mut pump = Pump::new(4);
        assert_eq!(pump.next(t0 + Duration::from_secs(5)), Next::Idle, "no watchdog before the first frame");
        pump.push(lit(), t0);
        assert!(matches!(pump.next(t0), Next::Frame(_)));
        assert_eq!(pump.next(t0 + Duration::from_millis(99)), Next::Idle);
        assert_eq!(pump.next(t0 + Duration::from_millis(100)), Next::Watchdog);
        assert_eq!(pump.next(t0 + Duration::from_millis(500)), Next::Idle, "once per silence");
        pump.push(lit(), t0 + Duration::from_millis(600));
        assert!(matches!(pump.next(t0 + Duration::from_millis(600)), Next::Frame(_)));
        assert_eq!(pump.next(t0 + Duration::from_millis(700)), Next::Watchdog, "armed again by a new frame");
    }

    #[test]
    fn disarmed_no_lit_frame_reaches_the_backend() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut out = open(&fake);
        for _ in 0..10 {
            out.send(&lit());
        }
        drop(out);
        let frames = fake.frames();
        assert!(frames.len() >= 2);
        assert!(frames.iter().flatten().all(|p| !p.is_lit()));
        assert_eq!(fake.events.lock().unwrap().last(), Some(&Event::Close));
    }

    #[test]
    fn armed_frames_leave_lit_at_the_capped_rate_and_closing_ends_dark() {
        let fake = FakeShowNet::new(&[("a", 0)]);
        let mut out = ShowNetOutput::open(&fake, &ShowNetConfig { pps: 150_000, ..Default::default() }).unwrap();
        assert_eq!((out.kind(), out.name()), ("shownet", "ShowNET Fake a (a)"));
        out.set_armed(true).unwrap();
        out.send(&lit());
        assert!(fake.wait_for(|e| e.iter().any(|e| matches!(e, Event::Frame { points, .. } if points.iter().any(|p| p.is_lit())))));
        drop(out);
        let events = fake.events.lock().unwrap().clone();
        assert_eq!(events[0], Event::Open("a".into()));
        assert!(events.iter().all(|e| !matches!(e, Event::Frame { pps, .. } if *pps != 100_000)), "150 000 capped to 100 000");
        let Event::Frame { points, .. } = &events[events.len() - 2] else { panic!("{events:?}") };
        assert!(points.iter().all(|p| !p.is_lit()), "last frame dark");
        assert_eq!(events.last(), Some(&Event::Close));
    }

    /// The e-stop trips while a frame renders: that frame leaves dark even
    /// though it was queued before the trip.
    #[test]
    fn the_kill_switch_blanks_the_frame_in_flight() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut out = open(&fake);
        let kill = out.kill_switch().expect("has one");
        out.set_armed(true).unwrap();
        fake.set_hold(true); // the box is busy with the first frame
        out.send(&lit());
        std::thread::sleep(Duration::from_millis(20));
        out.send(&lit()); // queued while the first is in flight
        kill();
        fake.set_hold(false);
        assert!(fake.wait_for(|e| e.iter().filter(|e| matches!(e, Event::Frame { .. })).count() >= 2));
        let frames = fake.frames();
        assert!(frames[1].iter().all(|p| !p.is_lit()), "queued before the trip, sent after it: dark");
        drop(out);
    }

    #[test]
    fn a_stalled_engine_gets_a_dark_frame_and_a_disarmed_output() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut out = open(&fake);
        out.set_armed(true).unwrap();
        out.send(&lit());
        assert!(fake.wait_for(|_| out.fault().is_some()), "watchdog after 100 ms");
        assert!(out.fault().unwrap().contains("moteur bloqué"));
        let frames = fake.frames();
        assert!(frames[0].iter().any(|p| p.is_lit()));
        assert!(frames[1].iter().all(|p| !p.is_lit()), "dark frame on its own");
        assert!(out.set_armed(true).is_err(), "won't re-arm a faulted output");
        out.send(&lit());
        assert!(fake.wait_for(|e| e.iter().filter(|e| matches!(e, Event::Frame { .. })).count() >= 3));
        assert!(fake.frames()[2].iter().all(|p| !p.is_lit()), "disarmed: dark");
        drop(out);
    }

    /// A disarmed output whose engine pauses just gets a dark frame: no
    /// fault, nothing to reconnect.
    #[test]
    fn the_watchdog_reports_no_fault_when_disarmed() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut out = open(&fake);
        out.send(&lit());
        assert!(fake.wait_for(|e| e.iter().filter(|e| matches!(e, Event::Frame { .. })).count() >= 2));
        assert_eq!(out.fault(), None);
        drop(out);
    }

    #[test]
    fn a_lost_box_disarms_and_reports_a_fault() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut out = open(&fake);
        out.set_armed(true).unwrap();
        out.send(&lit());
        fake.lost.store(true, Ordering::SeqCst);
        out.send(&lit());
        assert!(fake.wait_for(|_| out.fault().is_some()));
        assert!(out.fault().unwrap().contains("câble débranché"));
        assert!(!out.shared.armed.load(Ordering::SeqCst));
        drop(out); // closes without hanging
        assert_eq!(fake.events.lock().unwrap().last(), Some(&Event::Close));
    }

    /// Through the output stage, as the engine drives it: arming and
    /// sending in order, the e-stop latch, then the swap away (disarm,
    /// dark frames, close).
    #[test]
    fn it_behaves_like_a_dac_in_the_output_stage() {
        let fake = FakeShowNet::new(&[("a", 100_000)]);
        let mut stage = OutputStage::new(Some(Box::new(open(&fake))));
        let estop = EStop::default();
        assert_eq!(stage.emit(&lit(), false, true, &estop).lit, 0);
        assert_eq!(stage.emit(&lit(), true, true, &estop).lit, 2);
        assert!(fake.wait_for(|e| e.iter().any(|e| matches!(e, Event::Frame { points, .. } if points.iter().any(|p| p.is_lit())))));
        assert_eq!(stage.describe().map(|(_, k)| k), Some("shownet"));
        let old = stage.replace(None);
        drop(old);
        let events = fake.events.lock().unwrap().clone();
        let first_lit = events.iter().position(|e| matches!(e, Event::Frame { points, .. } if points.iter().any(|p| p.is_lit()))).unwrap();
        let after: Vec<_> = events[first_lit + 1..].iter().filter(|e| matches!(e, Event::Frame { points, .. } if points.iter().any(|p| p.is_lit()))).collect();
        assert!(after.len() <= 1, "at most the second lit frame, then dark: {after:?}");
        let Event::Frame { points, .. } = &events[events.len() - 2] else { panic!() };
        assert!(points.iter().all(|p| !p.is_lit()));
        assert_eq!(events.last(), Some(&Event::Close));
    }

    #[test]
    fn opening_picks_the_named_box_or_the_first() {
        let fake = FakeShowNet::new(&[("a", 100_000), ("b", 100_000)]);
        assert_eq!(open(&fake).name(), "ShowNET Fake a (a)");
        let out = ShowNetOutput::open(&fake, &ShowNetConfig { device_id: Some("b".into()), ..Default::default() }).unwrap();
        assert_eq!(out.name(), "ShowNET Fake b (b)");
        assert!(ShowNetOutput::open(&fake, &ShowNetConfig { device_id: Some("c".into()), ..Default::default() }).is_err());
        assert!(ShowNetOutput::open(&FakeShowNet::new(&[]), &ShowNetConfig::default()).is_err());
        assert!(open_id(None, "shownet:auto", 30_000).err().unwrap().to_string().contains("SDK"));
        assert_eq!(open_id(Some(&fake), "shownet:b", 30_000).unwrap().kind(), "shownet");
        assert!(open_id(Some(&fake), "idn:x", 30_000).is_err());
        assert_eq!(scan(&fake).unwrap().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["shownet:a", "shownet:b"]);
        assert_eq!(fixed_entry().id, "shownet:auto");
    }

    #[test]
    fn no_backend_is_installed_from_the_repository() {
        assert!(installed_backend().is_none());
    }

    /// The repository holds no address or socket for the box: what reaches
    /// it is up to the private binding (T-471).
    #[test]
    fn this_file_has_no_network_address_or_socket() {
        let src = include_str!("shownet.rs");
        for needle in [concat!("std::", "net"), concat!("Socket", "Addr"), concat!("Udp", "Socket"), concat!("Tcp", "Stream"), concat!("192", ".168")] {
            assert!(!src.contains(needle), "{needle}");
        }
        // No dotted quad (a.b.c.d) anywhere.
        let bytes = src.as_bytes();
        for start in 0..bytes.len() {
            if start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
                continue;
            }
            let mut i = start;
            let mut groups = 0;
            loop {
                let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
                if digits == 0 {
                    break;
                }
                groups += 1;
                i += digits;
                if bytes.get(i) != Some(&b'.') {
                    break;
                }
                i += 1;
            }
            assert!(groups < 4, "dotted quad at byte {start}");
        }
    }
}
