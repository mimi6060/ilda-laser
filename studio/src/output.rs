//! Sends rendered frames to a real laser DAC through `laser-dac` (IDN,
//! Ether Dream or Helios USB), or to MadMapper over PONK (`ponk.rs`, T-300). The
//! ShowNET will plug in here as another `Output` once Laserworld's API is
//! available - the engine and UI don't change.

use crate::interlock::{blank_unless, EStop};
use crate::patterns::Point;
use anyhow::{Context, Result};
use laser_dac::{default_transition, list_devices, open_device, Frame, FrameSession, FrameSessionConfig, LaserPoint, OutputModel, TransitionPlan};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub trait Output: Send {
    fn name(&self) -> &str;
    /// What kind of output this is, for the UI: "dac", "ponk", "test",
    /// "artnet" (the ShowNET SD card, `artnet.rs`) or "shownet"
    /// (`shownet.rs`, behind the `shownet` feature).
    fn kind(&self) -> &'static str {
        "dac"
    }
    /// Laser emission on/off. Frames keep flowing while disarmed, so
    /// re-arming shows the current look immediately.
    fn set_armed(&mut self, armed: bool) -> Result<()>;
    fn send(&mut self, points: &[Point]);
    /// Sends a dark frame right away, for shutdown and failures (T-253).
    /// The default is one blanked point at the centre; a new output (the
    /// ShowNET later) should override it if it has a faster way.
    fn blank_now(&mut self) -> Result<()> {
        self.send(&[Point::blanked(0.0, 0.0)]);
        Ok(())
    }
    /// A thread-safe way to disarm the output without going through the
    /// engine thread, for the emergency stop. `None` if it has none.
    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        None
    }
    /// Most points one frame may have (frame-swap DACs: Helios USB), or
    /// `None` for no limit. The engine thins the frame to fit *before*
    /// the safety stage (T-353), so zones, horizon and caps see the frame
    /// that really goes out.
    fn max_points(&self) -> Option<usize> {
        None
    }
    /// `Some(why)` once the device is gone (unplugged, session ended): the
    /// engine then disarms with « Sortie perdue », closes it and lets the
    /// output manager try to reconnect (T-401). Checked every frame, so it
    /// must be cheap and never block.
    fn fault(&self) -> Option<String> {
        None
    }
    /// For an output that plays its own content (the ShowNET SD card over
    /// Art-Net, `artnet.rs`, T-301): the cue on show, its bank number and
    /// the master × cap, once per frame before `send`. Ignored by outputs
    /// that draw the points.
    fn cue_signal(&mut self, _signal: &CueSignal) {}
    /// What the output reports to the UI beside its name (the values the
    /// Art-Net output sends, its warning), or `None`. Cheap, never blocks.
    fn status(&self) -> Option<serde_json::Value> {
        None
    }
    /// Fixed frame rate (T-463): `on`, the output inserts the same number
    /// of (dark) points between any two frames, and returns it, so the
    /// engine sends frames of `N - seam` points and every frame lasts N.
    /// Off, it goes back to its own seam. Called every frame: cheap.
    /// Outputs that add nothing between frames (the preview, the test
    /// outputs) keep the default, 0.
    fn fixed_seam(&mut self, _on: bool) -> usize {
        0
    }
}

/// What a cue-playing output needs beside the points (T-301), read by the
/// engine under the lock each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CueSignal {
    /// Name of the cue on show (the newest one played by hand, else the
    /// timeline's), None for a hand-made look, a scene or a test pattern.
    pub cue: Option<String>,
    /// Its number in the SD bank (`sdbank.rs`), if it has one.
    pub pattern: Option<u8>,
    /// Master dimmer after its automatic gains (audio silence, DMX loss), 0..1.
    pub master: f32,
    /// The output's power cap as one factor, 0..1: global × the lowest
    /// colour cap (the card's colours can't be changed live).
    pub cap: f32,
    /// Master size, 0..2 (1 = as drawn).
    pub size: f32,
}

/// The last pipeline stage: what actually leaves for the laser. The arm
/// state is re-checked here against the e-stop latch (lock-free), so a
/// stop that arrives while a frame renders still blanks that frame.
#[derive(Default)]
pub struct OutputStage {
    output: Option<Box<dyn Output>>,
    output_armed: bool,
}

/// What `OutputStage::emit` did.
pub struct Emitted {
    /// Lit points sent (0 whenever disarmed).
    pub lit: usize,
    /// `Some` when the output's arm state changed: the error, if any.
    pub arm_change: Option<Option<String>>,
}

impl OutputStage {
    pub fn new(output: Option<Box<dyn Output>>) -> Self {
        Self { output, output_armed: false }
    }

    /// The active output's frame size limit (`Output::max_points`).
    pub fn max_points(&self) -> Option<usize> {
        self.output.as_ref().and_then(|o| o.max_points())
    }

    /// Points the installed output adds between frames at a fixed rate
    /// (`Output::fixed_seam`), 0 without an output.
    pub fn fixed_seam(&mut self, on: bool) -> usize {
        self.output.as_mut().map_or(0, |o| o.fixed_seam(on))
    }

    /// `hold_ok` false (hold-to-run released) sends a dark frame but keeps
    /// the output armed, so pressing again resumes at once.
    pub fn emit(&mut self, frame: &[Point], gate_armed: bool, hold_ok: bool, estop: &EStop) -> Emitted {
        let armed = gate_armed && !estop.is_latched();
        let mut sent = frame.to_vec();
        blank_unless(armed && hold_ok, &mut sent);
        // Last resort: the engine already fitted the frame before the
        // safety stage, but the zone mask may have added a few points.
        // This thinning only ever removes light (T-353).
        if let Some(max) = self.max_points() {
            if sent.len() > max {
                sent = fit_frame_dark(&sent, max).into_owned();
            }
        }
        let mut arm_change = None;
        if let Some(out) = self.output.as_mut() {
            if armed != self.output_armed {
                arm_change = Some(out.set_armed(armed).err().map(|e| e.to_string()));
                self.output_armed = armed;
            }
            out.send(&sent);
        }
        Emitted { lit: sent.iter().filter(|p| p.is_lit()).count(), arm_change }
    }

    /// Clean shutdown (Ctrl+C, SIGTERM): disarm, three dark frames, then
    /// close the output (drop it).
    pub fn shutdown(&mut self) {
        drop(self.replace(None));
    }

    /// Swaps the output (T-401): the old one, if any, is disarmed and gets
    /// three dark frames before the new one is installed, then is handed
    /// back to be closed (dropping a DAC session waits for its thread, so
    /// the engine does that aside). The new one starts disarmed: only the
    /// gate arms it, on a later frame.
    #[must_use = "the old output closes when dropped"]
    pub fn replace(&mut self, next: Option<Box<dyn Output>>) -> Option<Box<dyn Output>> {
        let old = self.output.take().map(|mut out| {
            if let Err(e) = out.set_armed(false) {
                log::warn!("disarm before closing the output failed: {e}");
            }
            for _ in 0..3 {
                if let Err(e) = out.blank_now() {
                    log::warn!("dark frame before closing the output failed: {e}");
                }
            }
            out
        });
        self.output = next;
        self.output_armed = false;
        old
    }

    /// Passes this frame's cue signal to the output (`Output::cue_signal`).
    pub fn cue_signal(&mut self, signal: &CueSignal) {
        if let Some(out) = self.output.as_mut() {
            out.cue_signal(signal);
        }
    }

    /// The installed output's report for the UI (`Output::status`).
    pub fn status(&self) -> Option<serde_json::Value> {
        self.output.as_ref()?.status()
    }

    /// The installed output's fault, if it has one (see `Output::fault`).
    pub fn fault(&self) -> Option<String> {
        self.output.as_ref()?.fault()
    }

    /// The installed output's kill switch, for `EStop` (None in preview).
    pub fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        self.output.as_ref()?.kill_switch()
    }

    /// Name and kind of the installed output.
    pub fn describe(&self) -> Option<(String, &'static str)> {
        self.output.as_ref().map(|o| (o.name().to_string(), o.kind()))
    }
}

impl Drop for OutputStage {
    /// Reached without `shutdown` only when the engine thread unwinds from a
    /// panic: send a dark frame, then disarm, before the output closes.
    fn drop(&mut self) {
        if let Some(out) = self.output.as_mut() {
            let _ = out.blank_now();
            let _ = out.set_armed(false);
        }
    }
}

/// What the fake outputs are called in the UI (never a laser).
pub const TEST_OUTPUT_NAME: &str = "Sortie de test";

/// Testing only (`--test-output <file>`): no laser at all. Appends what the
/// studio asks of an output to a text file, one call per line (`arm`,
/// `disarm`, `blank`, `close`, and `lit`/`dark` when frames change between
/// lit and dark), so a subprocess test can check the shutdown sequence.
pub struct FileLogOutput {
    name: String,
    file: std::fs::File,
    lit: Option<bool>,
}

impl FileLogOutput {
    /// Opens the log, keeping what it holds (a reconnection, T-401, goes on
    /// in the same file).
    pub fn append(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new().append(true).create(true).open(path).with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Self { name: TEST_OUTPUT_NAME.into(), file, lit: None })
    }

    fn log(&mut self, line: &str) {
        use std::io::Write;
        let _ = writeln!(self.file, "{line}");
        let _ = self.file.flush();
    }
}

impl Output for FileLogOutput {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> &'static str {
        "test"
    }
    fn set_armed(&mut self, armed: bool) -> Result<()> {
        self.log(if armed { "arm" } else { "disarm" });
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        let lit = points.iter().any(|p| p.is_lit());
        if self.lit != Some(lit) {
            self.lit = Some(lit);
            self.log(if lit { "lit" } else { "dark" });
        }
    }
    fn blank_now(&mut self) -> Result<()> {
        self.lit = Some(false);
        self.log("blank");
        Ok(())
    }
}

impl Drop for FileLogOutput {
    fn drop(&mut self) {
        self.log("close");
    }
}

/// The colour delay `laser-dac` applies by default to a frame session
/// (150 µs at `pps`, in points). We keep it as it is: the output's own
/// colour delay (T-414, `colorcal.rs`) adds to it, inside each stroke,
/// and the UI shows both, so there is no hidden double compensation.
pub fn library_color_delay_points(pps: u32) -> usize {
    FrameSessionConfig::new(pps).color_delay_points
}

pub struct DacOutput {
    name: String,
    session: FrameSession,
    /// Most points per frame this DAC takes, for frame-swap DACs (Helios
    /// USB: 4095), less room for the blanked loop-back `laser-dac` adds.
    max_points: Option<usize>,
    /// Fixed frame rate on (T-463): the session's seam has a constant
    /// length (`fixed_transition`), read by its scheduler thread.
    fixed: Arc<AtomicBool>,
    pps: u32,
}

impl DacOutput {
    /// `device` is an id from `ilda-laser discover`, or "auto" for the
    /// first DAC found.
    pub fn open(device: &str, pps: u32) -> Result<Self> {
        let id = if device == "auto" {
            let devices = list_devices().map_err(|e| anyhow::anyhow!("{e}")).context("failed to scan for DACs")?;
            devices.first().context("no DAC found on the network")?.id.clone()
        } else {
            device.to_string()
        };
        let dac = open_device(&id).map_err(|e| anyhow::anyhow!("{e}")).context("failed to open device")?;
        let fixed = Arc::new(AtomicBool::new(false));
        let (session, info) = dac
            .start_frame_session(FrameSessionConfig::new(pps).with_transition_fn(seam_fn(pps, Arc::clone(&fixed))))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to start frame session")?;
        let max_points = (info.caps.output_model == OutputModel::UsbFrameSwap)
            .then(|| info.caps.max_points_per_chunk.saturating_sub(loop_back_points(pps)).max(2));
        Ok(Self { name: format!("{} ({id})", info.name), session, max_points, fixed, pps })
    }
}

/// Fixed-rate transit between two frames: always this many points,
/// `laser-dac`'s longest (its own transit takes 0..64 points by distance),
/// so the galvos never move faster between frames than without the mode.
const FIXED_TRANSIT: usize = 64;

/// `laser-dac`'s dwells around a frame change (100 us at the end of a
/// frame, 400 us before the next), in points at `pps`.
fn seam_dwells(pps: u32) -> (usize, usize) {
    let pts = |us: f64| (us * pps as f64 / 1_000_000.0).round() as usize;
    (pts(100.0), pts(400.0))
}

/// Points the fixed-rate seam inserts between two frames at `pps`.
pub fn fixed_seam_points(pps: u32) -> usize {
    let (end, start) = seam_dwells(pps);
    end + FIXED_TRANSIT + start
}

/// The seam between two frames at a fixed rate (T-463): `laser-dac`
/// promotes a new frame only once the current one has played to its end
/// (latest wins, a frame is never cut), and inserts a transition there,
/// whose length depends on the distance; here it is always
/// `fixed_seam_points`, all dark: a dwell where the frame ends, an eased
/// move, a dwell where the next starts. Even when the same frame loops
/// (no new one yet) the seam is the same length, so every frame lasts
/// the same time.
pub fn fixed_transition(pps: u32, from: &LaserPoint, to: &LaserPoint) -> Vec<LaserPoint> {
    let (end, start) = seam_dwells(pps);
    let mut out = Vec::with_capacity(end + FIXED_TRANSIT + start);
    out.extend((0..end).map(|_| LaserPoint::blanked(from.x, from.y)));
    for i in 0..FIXED_TRANSIT {
        // Smoothstep ease: slow off the end point, slow onto the next.
        let t = (i + 1) as f32 / (FIXED_TRANSIT + 1) as f32;
        let e = t * t * (3.0 - 2.0 * t);
        out.push(LaserPoint::blanked(from.x + (to.x - from.x) * e, from.y + (to.y - from.y) * e));
    }
    out.extend((0..start).map(|_| LaserPoint::blanked(to.x, to.y)));
    out
}

/// The session's transition: `laser-dac`'s own, or the fixed-rate one
/// while `fixed` is set.
fn seam_fn(pps: u32, fixed: Arc<AtomicBool>) -> laser_dac::TransitionFn {
    let default = default_transition(pps);
    Box::new(move |from: &LaserPoint, to: &LaserPoint| if fixed.load(Ordering::Relaxed) { TransitionPlan::Transition(fixed_transition(pps, from, to)) } else { default(from, to) })
}

/// Points `laser-dac` adds to loop a frame-swap frame back to its start
/// (blanked dwell, travel, dwell: 100 us + up to 64 points + 400 us),
/// plus a small margin. Leaving room for them means the DAC never has to
/// drop them, which would draw the jump back to the start lit.
fn loop_back_points(pps: u32) -> usize {
    (500.0 * pps as f64 / 1_000_000.0).ceil() as usize + 64 + 4
}

/// Room left for the points the safety stage adds (the zone mask inserts
/// up to four samples per edge crossing) when the engine fits a frame to
/// an output's `max_points` before that stage.
pub fn content_budget(max: usize) -> usize {
    (max - max / 8).max(2)
}

/// Points per frame an output takes (T-171, `power::OutputScan`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameLimits {
    /// Fewer points are completed (`pad_frame`); 0 = off.
    pub min: usize,
    /// More points are thinned (`fit_frame`); None = no limit of its own.
    pub max: Option<usize>,
}

/// A frame within `limits` and within `dac_max` (the DAC's frame size,
/// `Output::max_points`), on content, **before** the safety stage: thinned
/// with `fit_frame` (never lights travel), then completed with `pad_frame`
/// (never adds a lit segment that wasn't there). The minimum gives way to
/// the maxima, so the DAC fit that follows has room for the zone mask.
pub fn fit_frame_limits(points: Vec<Point>, limits: FrameLimits, dac_max: Option<usize>) -> Vec<Point> {
    let max = match (limits.max, dac_max.map(content_budget)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let points = match max {
        Some(max) if points.len() > max => fit_frame(&points, max).into_owned(),
        _ => points,
    };
    pad_frame(points, max.map_or(limits.min, |m| limits.min.min(m)))
}

/// How close two points must be to count as the same position.
const SAME_POSITION: f32 = 1e-6;

/// Completes a frame shorter than `min` points (T-171): too few points
/// make the DAC loop the frame too fast for the galvos. Never adds a lit
/// segment the frame didn't already draw:
///
/// - a closed path (it ends where it starts) is drawn again, whole, as
///   many times as fits: the same lit segments, retraced;
/// - an open path waits on its last point, **dark** (holding a lit point
///   would make a hot spot);
/// - a frame that doesn't move (empty, or a beam held on one spot) is
///   left as it is: the galvos are still, there is nothing to protect,
///   and the static-beam guard sees it exactly as before.
///
/// Whatever is left after the last whole lap is a dark hold too, so the
/// frame always ends where it ended. Runs before the safety stage, which
/// still has the last word on every point.
pub fn pad_frame(mut points: Vec<Point>, min: usize) -> Vec<Point> {
    if points.len() >= min {
        return points;
    }
    let Some(&last) = points.last() else {
        return points;
    };
    let same = |a: &Point, b: &Point| (a.x - b.x).abs() <= SAME_POSITION && (a.y - b.y).abs() <= SAME_POSITION;
    let first = points[0];
    if points.iter().all(|p| same(p, &first)) {
        return points;
    }
    if same(&first, &last) {
        // One lap = every point after the first: its first segment, from
        // the end (same place as the start), is the frame's first one.
        let lap = points.len() - 1;
        while points.len() + lap <= min {
            points.extend_from_within(1..=lap);
        }
    }
    points.resize(min, Point::blanked(last.x, last.y));
    points
}

/// Thins a frame down to `max` points for a DAC with a frame size limit
/// (`laser-dac` would cut the tail, leaving a lit end point to jump back
/// from). Evenly spaced points are kept, first and last included, and a
/// kept point goes dark whenever a dark point was skipped just before
/// it: thinning never lights a blanked travel. A lit chord replaces the
/// skipped points, though, so this runs on content, *before* the safety
/// stage (T-353); after it, only `fit_frame_dark` may thin.
pub fn fit_frame(points: &[Point], max: usize) -> Cow<'_, [Point]> {
    let n = points.len();
    if n <= max || max < 2 {
        return Cow::Borrowed(points);
    }
    let mut out = Vec::with_capacity(max);
    let mut prev = 0;
    for k in 0..max {
        let i = k * (n - 1) / (max - 1);
        let mut p = points[i];
        if k > 0 && points[prev + 1..i].iter().any(|q| !q.is_lit()) {
            p = Point::blanked(p.x, p.y);
        }
        out.push(p);
        prev = i;
    }
    Cow::Owned(out)
}

/// How far (normalized units) a skipped point may lie from the chord that
/// replaces it in `fit_frame_dark`: below one 16-bit DAC step (2 / 65536),
/// well above the float noise of a densified straight line.
const CHORD_TOLERANCE: f32 = 1e-5;

/// Distance from `q` to the segment a-b.
fn distance_to_segment(q: &Point, a: &Point, b: &Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((q.x - a.x) * dx + (q.y - a.y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((q.x - a.x - t * dx).powi(2) + (q.y - a.y - t * dy).powi(2)).sqrt()
}

/// Thins a frame that already went through the safety stage down to `max`
/// points, removing light only (T-353): the last-resort guard of the
/// output stage. Evenly spaced points are kept, first and last included.
/// The chord between two kept points stays lit only if every point it
/// skips was lit and lies on it (`CHORD_TOLERANCE`), so it draws exactly
/// the path the safety stage approved, at the darkest colour of that
/// stretch. Otherwise both ends of the chord go dark: it can't be a lit
/// shortcut through a Blank zone or below the horizon.
pub fn fit_frame_dark(points: &[Point], max: usize) -> Cow<'_, [Point]> {
    let n = points.len();
    if n <= max || max < 2 {
        return Cow::Borrowed(points);
    }
    let mut out: Vec<Point> = Vec::with_capacity(max);
    let mut prev = 0;
    for k in 0..max {
        let i = k * (n - 1) / (max - 1);
        let mut p = points[i];
        if k > 0 && i > prev + 1 {
            let (a, skipped) = (&points[prev], &points[prev + 1..i]);
            if skipped.iter().all(|q| q.is_lit() && distance_to_segment(q, a, &p) <= CHORD_TOLERANCE) {
                for q in skipped {
                    (p.r, p.g, p.b) = (p.r.min(q.r), p.g.min(q.g), p.b.min(q.b));
                }
            } else {
                p = Point::blanked(p.x, p.y);
                let last = out.last_mut().expect("k > 0");
                *last = Point::blanked(last.x, last.y);
            }
        }
        out.push(p);
        prev = i;
    }
    Cow::Owned(out)
}

impl Output for DacOutput {
    fn name(&self) -> &str {
        &self.name
    }

    fn set_armed(&mut self, armed: bool) -> Result<()> {
        let control = self.session.control();
        let result = if armed { control.arm() } else { control.disarm() };
        result.map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn max_points(&self) -> Option<usize> {
        self.max_points
    }

    fn fixed_seam(&mut self, on: bool) -> usize {
        self.fixed.store(on, Ordering::Relaxed);
        if on {
            fixed_seam_points(self.pps)
        } else {
            0
        }
    }

    fn send(&mut self, points: &[Point]) {
        // The output stage already fitted the frame; this only guards a
        // caller that bypassed it, and only ever removes light.
        let points = match self.max_points {
            Some(max) => fit_frame_dark(points, max),
            None => Cow::Borrowed(points),
        };
        self.session.send_frame(Frame::new(points.iter().map(to_laser_point).collect()));
    }

    fn blank_now(&mut self) -> Result<()> {
        // A single blanked point replaces whatever the DAC would repeat.
        self.session.send_frame(Frame::new(vec![LaserPoint::blanked(0.0, 0.0)]));
        Ok(())
    }

    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let control = self.session.control();
        Some(Box::new(move || {
            if let Err(e) = control.disarm() {
                log::warn!("emergency disarm of the DAC failed: {e}");
            }
        }))
    }

    /// The session runs without `laser-dac`'s own reconnection, so its
    /// thread ends when the DAC goes away: the studio's output manager
    /// reconnects instead, always disarmed (T-401).
    fn fault(&self) -> Option<String> {
        (self.session.is_finished() || !self.session.metrics().connected()).then(|| format!("{} ne répond plus", self.name))
    }
}

fn to_laser_point(p: &Point) -> LaserPoint {
    let c = |v: f32| (v.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    if p.is_lit() {
        LaserPoint::new(p.x, p.y, c(p.r), c(p.g), c(p.b), u16::MAX)
    } else {
        LaserPoint::blanked(p.x, p.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::ArmSource;
    use std::sync::{Arc, Mutex};

    /// Records what the stage does to the output.
    #[derive(Clone, Default)]
    struct Probe {
        armed: Arc<Mutex<Vec<bool>>>,
        frames: Arc<Mutex<Vec<Vec<Point>>>>,
        /// Every call in order: "arm", "disarm", "send", "blank".
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Output for Probe {
        fn name(&self) -> &str {
            "probe"
        }
        fn set_armed(&mut self, armed: bool) -> Result<()> {
            self.armed.lock().unwrap().push(armed);
            self.calls.lock().unwrap().push(if armed { "arm" } else { "disarm" });
            Ok(())
        }
        fn send(&mut self, points: &[Point]) {
            self.frames.lock().unwrap().push(points.to_vec());
            self.calls.lock().unwrap().push("send");
        }
        fn blank_now(&mut self) -> Result<()> {
            self.calls.lock().unwrap().push("blank");
            Ok(())
        }
    }

    /// Only the trait's default `blank_now`, as a new output would have.
    struct Plain(Arc<Mutex<Vec<Vec<Point>>>>);

    impl Output for Plain {
        fn name(&self) -> &str {
            "plain"
        }
        fn set_armed(&mut self, _: bool) -> Result<()> {
            Ok(())
        }
        fn send(&mut self, points: &[Point]) {
            self.0.lock().unwrap().push(points.to_vec());
        }
    }

    fn lit() -> Vec<Point> {
        vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), Point::lit(0.2, 0.2, 0.0, 1.0, 0.0)]
    }

    /// T-463: at a fixed rate the DAC session's seam is the same length
    /// whatever the two frames (a jump across the field, the same frame
    /// looping), all dark, from where one ends to where the next starts,
    /// never steeper than `laser-dac`'s own; off, it is `laser-dac`'s.
    #[test]
    fn the_fixed_rate_seam_is_constant_and_dark() {
        let pps = 30_000;
        assert_eq!(fixed_seam_points(pps), 3 + 64 + 12);
        let fixed = Arc::new(AtomicBool::new(true));
        let seam = seam_fn(pps, Arc::clone(&fixed));
        let default = default_transition(pps);
        let pts = |plan: TransitionPlan| match plan {
            TransitionPlan::Transition(p) => p,
            TransitionPlan::Coalesce => Vec::new(),
        };
        let a = LaserPoint::new(-1.0, -1.0, 65_535, 0, 0, 65_535);
        for b in [LaserPoint::blanked(1.0, 1.0), LaserPoint::blanked(-1.0, -1.0), LaserPoint::new(0.01, -0.99, 0, 65_535, 0, 65_535)] {
            let ours = pts(seam(&a, &b));
            assert_eq!(ours.len(), fixed_seam_points(pps));
            assert!(ours.iter().all(|p| p.r == 0 && p.g == 0 && p.b == 0 && p.intensity == 0));
            assert_eq!((ours[0].x, ours[0].y), (a.x, a.y));
            assert_eq!((ours.last().unwrap().x, ours.last().unwrap().y), (b.x, b.y));
            let steepest = |p: &[LaserPoint]| p.windows(2).map(|w| (w[1].x - w[0].x).abs().max((w[1].y - w[0].y).abs())).fold(0.0f32, f32::max);
            let theirs = pts(default(&a, &b));
            assert!(steepest(&ours) <= steepest(&theirs).max(1e-6) * 1.6, "{} vs {}", steepest(&ours), steepest(&theirs));
        }
        fixed.store(false, Ordering::Relaxed);
        let b = LaserPoint::blanked(0.3, 0.1);
        assert_eq!(pts(seam(&a, &b)).len(), pts(default(&a, &b)).len());
    }

    #[test]
    fn disarmed_stage_sends_only_blank_points() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let e = stage.emit(&lit(), false, true, &EStop::default());
        assert_eq!(e.lit, 0);
        assert!(e.arm_change.is_none(), "starts disarmed: nothing to change");
        assert!(probe.frames.lock().unwrap()[0].iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn armed_stage_sends_the_frame_and_arms_once() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let estop = EStop::default();
        assert_eq!(stage.emit(&lit(), true, true, &estop).lit, 2);
        assert_eq!(stage.emit(&lit(), true, true, &estop).lit, 2);
        assert_eq!(*probe.armed.lock().unwrap(), vec![true]);
    }

    /// An e-stop tripped mid-frame (after the gate state was read under the
    /// lock, gate still "armed") blanks the very next emitted frame.
    #[test]
    fn estop_blanks_the_next_frame_even_before_the_gate_syncs() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let estop = EStop::default();
        stage.emit(&lit(), true, true, &estop);
        estop.trip(ArmSource::Keyboard);
        let e = stage.emit(&lit(), true, true, &estop);
        assert_eq!(e.lit, 0);
        assert!(probe.frames.lock().unwrap().last().unwrap().iter().all(|p| !p.is_lit()));
        assert_eq!(*probe.armed.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn hold_released_sends_dark_frames_but_keeps_the_output_armed() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let estop = EStop::default();
        assert_eq!(stage.emit(&lit(), true, true, &estop).lit, 2);
        let e = stage.emit(&lit(), true, false, &estop);
        assert_eq!(e.lit, 0);
        assert!(e.arm_change.is_none(), "not disarmed");
        assert!(probe.frames.lock().unwrap().last().unwrap().iter().all(|p| !p.is_lit()));
        assert_eq!(stage.emit(&lit(), true, true, &estop).lit, 2, "pressed again");
        assert_eq!(*probe.armed.lock().unwrap(), vec![true]);
    }

    #[test]
    fn shutdown_disarms_sends_three_dark_frames_then_closes() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        stage.emit(&lit(), true, true, &EStop::default());
        stage.shutdown();
        assert_eq!(*probe.calls.lock().unwrap(), vec!["arm", "send", "disarm", "blank", "blank", "blank"]);
        drop(stage);
        assert_eq!(probe.calls.lock().unwrap().len(), 6, "closed: nothing more after shutdown");
    }

    /// The engine thread panics mid-show: unwinding drops the stage, which
    /// blanks then disarms the output.
    #[test]
    fn a_panicking_engine_thread_blanks_and_disarms_the_output() {
        let probe = Probe::default();
        let p = probe.clone();
        let engine = std::thread::spawn(move || {
            let mut stage = OutputStage::new(Some(Box::new(p)));
            stage.emit(&lit(), true, true, &EStop::default());
            panic!("simulated engine panic");
        });
        assert!(engine.join().is_err());
        assert_eq!(*probe.calls.lock().unwrap(), vec!["arm", "send", "blank", "disarm"]);
    }

    #[test]
    fn default_blank_now_sends_one_dark_point() {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let mut out = Plain(Arc::clone(&frames));
        out.blank_now().unwrap();
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 1);
        assert!(!frames[0][0].is_lit());
    }

    #[test]
    fn file_log_output_records_the_calls() {
        let path = std::env::temp_dir().join(format!("laser-studio-filelog-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut stage = OutputStage::new(Some(Box::new(FileLogOutput::append(&path).unwrap())));
            stage.emit(&lit(), false, true, &EStop::default());
            stage.emit(&lit(), true, true, &EStop::default());
            stage.emit(&lit(), true, true, &EStop::default());
            stage.shutdown();
        }
        let log = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(log.lines().collect::<Vec<_>>(), ["dark", "arm", "lit", "disarm", "blank", "blank", "blank", "close"]);
    }

    #[test]
    fn preview_only_stage_still_reports_gated_points() {
        let mut stage = OutputStage::new(None);
        assert_eq!(stage.emit(&lit(), false, true, &EStop::default()).lit, 0);
        assert_eq!(stage.emit(&lit(), true, true, &EStop::default()).lit, 2);
    }

    #[test]
    fn full_color_maps_to_full_scale() {
        let lp = to_laser_point(&Point::lit(0.5, -0.5, 1.0, 0.0, 0.5));
        assert_eq!((lp.r, lp.g), (u16::MAX, 0));
        assert!((lp.b as i32 - 32768).abs() <= 1);
    }

    #[test]
    fn frames_that_fit_are_sent_as_is() {
        let f = lit();
        assert!(matches!(fit_frame(&f, 4095), Cow::Borrowed(_)));
        assert!(matches!(fit_frame(&f, 2), Cow::Borrowed(_)));
    }

    #[test]
    fn big_frames_are_thinned_to_the_limit_keeping_both_ends() {
        let f: Vec<Point> = (0..10_000).map(|i| Point::lit(i as f32 / 10_000.0, 0.0, 1.0, 0.0, 0.0)).collect();
        let out = fit_frame(&f, 4000);
        assert_eq!(out.len(), 4000);
        assert_eq!(out[0], f[0]);
        assert_eq!(out[3999], f[9999]);
        assert!(out.iter().all(|p| p.is_lit()), "an all-lit path stays lit");
        assert!(out.windows(2).all(|w| w[1].x > w[0].x), "in order");
    }

    /// Two lit shapes joined by a blanked travel: however the points fall,
    /// the thinned frame never draws a lit line across the gap.
    #[test]
    fn thinning_never_lights_a_blanked_travel() {
        let mut f = Vec::new();
        for i in 0..3000 {
            f.push(Point::lit(-1.0 + i as f32 / 3000.0, 0.0, 0.0, 1.0, 0.0));
        }
        f.push(Point::blanked(0.0, 0.5));
        f.push(Point::blanked(0.5, 0.5));
        for i in 0..3000 {
            f.push(Point::lit(0.5, 0.5 - i as f32 / 3000.0, 0.0, 1.0, 0.0));
        }
        for max in [100, 1000, 2047, 4000, 5999] {
            let out = fit_frame(&f, max);
            assert_eq!(out.len(), max);
            let first_right = out.iter().position(|p| p.x >= 0.0).unwrap();
            assert!(!out[first_right].is_lit(), "max {max}: the jump to the second shape is dark");
            assert!(out.iter().filter(|p| p.is_lit()).count() <= f.iter().filter(|p| p.is_lit()).count());
        }
    }

    /// An L-shaped lit path that goes around a Blank zone in its corner:
    /// `fit_frame` would cut the corner with a lit chord through the zone,
    /// `fit_frame_dark` never does (T-353).
    #[test]
    fn dark_thinning_never_cuts_a_corner_lit() {
        use crate::zones::{blank_violations, Zone, ZoneKind};
        let zone = Zone { points: vec![[0.3, -0.9], [0.9, -0.9], [0.9, -0.3], [0.3, -0.3]], kind: ZoneKind::Blank, ..Default::default() };
        let mut f: Vec<Point> = (0..=100).map(|i| Point::lit(-1.0 + i as f32 / 50.0, -1.0, 1.0, 0.0, 0.0)).collect();
        f.extend((1..=100).map(|i| Point::lit(1.0, -1.0 + i as f32 / 50.0, 1.0, 0.0, 0.0)));
        assert_eq!(blank_violations(&f, std::slice::from_ref(&zone)), 0);
        assert!(blank_violations(&fit_frame(&f, 6), std::slice::from_ref(&zone)) > 0, "the content thinning does cut it");
        for max in 2..f.len() {
            let out = fit_frame_dark(&f, max);
            assert_eq!(out.len(), max);
            assert_eq!(blank_violations(&out, std::slice::from_ref(&zone)), 0, "max {max}");
            assert!(out.iter().filter(|p| p.is_lit()).count() <= f.iter().filter(|p| p.is_lit()).count());
        }
    }

    /// Straight lit lines survive the dark thinning lit, at the darkest
    /// colour of what each chord replaced.
    #[test]
    fn dark_thinning_keeps_straight_lines_lit_and_never_brighter() {
        let f: Vec<Point> = (0..1000).map(|i| Point::lit(-1.0 + i as f32 / 500.0, 0.25, if i == 500 { 0.2 } else { 1.0 }, 0.5, 0.0)).collect();
        let out = fit_frame_dark(&f, 100);
        assert_eq!(out.len(), 100);
        assert!(out.iter().all(|p| p.is_lit()));
        assert!(out.iter().all(|p| p.r <= 1.0 && p.g <= 0.5 && p.b == 0.0));
        assert_eq!(out.iter().filter(|p| p.r < 1.0).count(), 1, "the dim point darkens the chord over it");
        assert!(matches!(fit_frame_dark(&f, 1000), Cow::Borrowed(_)));
    }

    /// An output with a frame size limit: the stage's last-resort thinning
    /// sends at most that many points and only removes light.
    #[test]
    fn stage_fits_frames_to_the_output_limit_without_adding_light() {
        struct Small(Arc<Mutex<Vec<Vec<Point>>>>);
        impl Output for Small {
            fn name(&self) -> &str {
                "small"
            }
            fn set_armed(&mut self, _: bool) -> Result<()> {
                Ok(())
            }
            fn send(&mut self, points: &[Point]) {
                self.0.lock().unwrap().push(points.to_vec());
            }
            fn max_points(&self) -> Option<usize> {
                Some(50)
            }
        }
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut stage = OutputStage::new(Some(Box::new(Small(Arc::clone(&sent)))));
        assert_eq!(stage.max_points(), Some(50));
        assert_eq!(OutputStage::default().max_points(), None);
        let f: Vec<Point> = (0..400).map(|i| Point::lit((i as f32 * 0.1).sin(), (i as f32 * 0.13).cos(), 1.0, 1.0, 1.0)).collect();
        let e = stage.emit(&f, true, true, &EStop::default());
        let out = sent.lock().unwrap().last().unwrap().clone();
        assert_eq!(out.len(), 50);
        assert_eq!(e.lit, out.iter().filter(|p| p.is_lit()).count());
        // A curve: no chord lies on it, so nothing stays lit.
        assert_eq!(e.lit, 0);
        assert_eq!(stage.emit(&f[..30], true, true, &EStop::default()).lit, 30, "frames that fit pass as is");
    }

    #[test]
    fn the_content_budget_leaves_room_for_the_safety_stage() {
        assert_eq!(content_budget(4000), 3500);
        assert_eq!(content_budget(2), 2);
        assert!((2..10_000).all(|m| content_budget(m) <= m && content_budget(m) >= 2));
    }

    #[test]
    fn the_helios_limit_leaves_room_for_the_loop_back() {
        // 4095 points per frame on Helios USB; laser-dac's loop-back is at
        // most 7 + 64 + 26 points at its fastest rate.
        let room = 4095 - loop_back_points(65_535);
        assert!(room > 3900 && room <= 4095 - 97, "{room}");
        assert!(loop_back_points(30_000) >= 3 + 64 + 12);
    }

    #[test]
    fn unlit_points_are_blanked() {
        let lp = to_laser_point(&Point::blanked(0.1, 0.2));
        assert_eq!((lp.r, lp.g, lp.b), (0, 0, 0));
    }

    fn lit_segments(f: &[Point]) -> Vec<[(f32, f32); 2]> {
        f.windows(2).filter(|w| w[1].is_lit() && (w[0].x, w[0].y) != (w[1].x, w[1].y)).map(|w| [(w[0].x, w[0].y), (w[1].x, w[1].y)]).collect()
    }

    #[test]
    fn a_50_point_frame_is_completed_to_200() {
        // An open line: its 50 points as they were, then a dark hold on its end.
        let line: Vec<Point> = (0..50).map(|i| Point::lit(-0.5 + i as f32 / 49.0, 0.1, 1.0, 0.5, 0.0)).collect();
        let out = pad_frame(line.clone(), 200);
        assert_eq!(out.len(), 200);
        assert_eq!(out[..50], line[..]);
        assert!(out[50..].iter().all(|p| !p.is_lit() && (p.x, p.y) == (line[49].x, line[49].y)), "dark hold on the last point");
        // A closed outline of 50 points: drawn again, whole laps, ending where it starts.
        let ring: Vec<Point> = (0..=49)
            .map(|i| {
                let a = std::f32::consts::TAU * (i % 49) as f32 / 49.0;
                Point::lit(0.4 * a.cos(), 0.4 * a.sin(), 0.0, 1.0, 0.0)
            })
            .collect();
        let out = pad_frame(ring.clone(), 200);
        assert_eq!(out.len(), 200);
        assert_eq!(out.iter().filter(|p| p.is_lit()).count(), 50 + 3 * 49, "three more whole laps");
        assert!(out[50 + 3 * 49..].iter().all(|p| !p.is_lit()));
        let (first, last) = (out[0], out[199]);
        assert_eq!((first.x, first.y), (last.x, last.y));
        // Never a lit segment the frame didn't draw.
        let before = lit_segments(&ring);
        assert!(lit_segments(&out).iter().all(|s| before.contains(s)));
    }

    #[test]
    fn padding_never_holds_a_lit_spot_and_leaves_long_frames_alone() {
        // A static beam (one spot, lit) or an empty frame: the galvos are
        // still, left as it is (the static-beam guard sees the same frame).
        let beam = vec![Point::lit(0.2, 0.3, 1.0, 1.0, 1.0); 6];
        assert_eq!(pad_frame(beam.clone(), 200), beam);
        assert_eq!(pad_frame(Vec::new(), 200), Vec::new());
        // An open line ending lit: the hold is dark, never a lit spot.
        let line = vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), Point::lit(0.5, 0.0, 1.0, 1.0, 1.0)];
        let out = pad_frame(line, 200);
        assert_eq!((out.len(), out.iter().filter(|p| p.is_lit()).count()), (200, 2));
        // Long enough, or off: untouched.
        let sq = crate::patterns::square(0.5, 1.0, 1.0, 1.0);
        assert_eq!(pad_frame(sq.clone(), 3), sq);
        assert_eq!(pad_frame(sq.clone(), 0), sq);
        assert_eq!(pad_frame(Vec::new(), 0), Vec::new());
    }

    #[test]
    fn frame_limits_thin_then_complete_within_the_dac() {
        let long: Vec<Point> = (0..1000).map(|i| Point::lit((i as f32 / 999.0) * 2.0 - 1.0, 0.0, 1.0, 1.0, 1.0)).collect();
        let lim = FrameLimits { min: 200, max: Some(600) };
        assert_eq!(fit_frame_limits(long.clone(), lim, None).len(), 600);
        // The DAC's frame size wins when smaller (room left for the zone mask).
        assert_eq!(fit_frame_limits(long.clone(), lim, Some(300)).len(), content_budget(300));
        assert_eq!(fit_frame_limits(long.clone(), FrameLimits::default(), None).len(), 1000);
        // The minimum gives way to a smaller DAC.
        let short = vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), Point::lit(0.1, 0.0, 1.0, 1.0, 1.0)];
        assert_eq!(fit_frame_limits(short.clone(), lim, Some(100)).len(), content_budget(100));
        assert_eq!(fit_frame_limits(short, lim, None).len(), 200);
    }
}
