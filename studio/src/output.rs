//! Sends rendered frames to a real laser DAC through `laser-dac` (IDN,
//! Ether Dream or Helios USB), or to MadMapper over PONK (`ponk.rs`, T-300). The
//! ShowNET will plug in here as another `Output` once Laserworld's API is
//! available - the engine and UI don't change.

use crate::interlock::{blank_unless, EStop};
use crate::patterns::Point;
use anyhow::{Context, Result};
use laser_dac::{list_devices, open_device, Frame, FrameSession, FrameSessionConfig, LaserPoint, OutputModel};
use std::borrow::Cow;

pub trait Output: Send {
    fn name(&self) -> &str;
    /// What kind of output this is, for the UI: "dac", "ponk" or "test".
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
        if let Some(mut out) = self.output.take() {
            if let Err(e) = out.set_armed(false) {
                log::warn!("disarm at shutdown failed: {e}");
            }
            for _ in 0..3 {
                if let Err(e) = out.blank_now() {
                    log::warn!("blank frame at shutdown failed: {e}");
                }
            }
        }
        self.output_armed = false;
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
    pub fn create(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self { name: format!("fichier de test ({})", path.display()), file, lit: None })
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

pub struct DacOutput {
    name: String,
    session: FrameSession,
    /// Most points per frame this DAC takes, for frame-swap DACs (Helios
    /// USB: 4095), less room for the blanked loop-back `laser-dac` adds.
    max_points: Option<usize>,
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
        let (session, info) = dac
            .start_frame_session(FrameSessionConfig::new(pps))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to start frame session")?;
        let max_points = (info.caps.output_model == OutputModel::UsbFrameSwap)
            .then(|| info.caps.max_points_per_chunk.saturating_sub(loop_back_points(pps)).max(2));
        Ok(Self { name: format!("{} ({id})", info.name), session, max_points })
    }
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
        {
            let mut stage = OutputStage::new(Some(Box::new(FileLogOutput::create(&path).unwrap())));
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
}
