//! Sends rendered frames to a real laser DAC through `laser-dac` (IDN or
//! Ether Dream today), or to MadMapper over PONK (`ponk.rs`, T-300). The
//! ShowNET will plug in here as another `Output` once Laserworld's API is
//! available - the engine and UI don't change.

use crate::interlock::{blank_unless, EStop};
use crate::patterns::Point;
use anyhow::{Context, Result};
use laser_dac::{list_devices, open_device, Frame, FrameSession, FrameSessionConfig, LaserPoint};

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

    /// `hold_ok` false (hold-to-run released) sends a dark frame but keeps
    /// the output armed, so pressing again resumes at once.
    pub fn emit(&mut self, frame: &[Point], gate_armed: bool, hold_ok: bool, estop: &EStop) -> Emitted {
        let armed = gate_armed && !estop.is_latched();
        let mut sent = frame.to_vec();
        blank_unless(armed && hold_ok, &mut sent);
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
        Ok(Self { name: format!("{} ({id})", info.name), session })
    }
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

    fn send(&mut self, points: &[Point]) {
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
    fn unlit_points_are_blanked() {
        let lp = to_laser_point(&Point::blanked(0.1, 0.2));
        assert_eq!((lp.r, lp.g, lp.b), (0, 0, 0));
    }
}
