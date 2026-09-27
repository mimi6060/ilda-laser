//! Sends rendered frames to a real laser DAC through `laser-dac` (IDN or
//! Ether Dream today). The ShowNET will plug in here as another `Output`
//! once Laserworld's API is available - the engine and UI don't change.

use crate::interlock::{blank_unless, EStop};
use crate::patterns::Point;
use anyhow::{Context, Result};
use laser_dac::{list_devices, open_device, Frame, FrameSession, FrameSessionConfig, LaserPoint};

pub trait Output: Send {
    fn name(&self) -> &str;
    /// Laser emission on/off. Frames keep flowing while disarmed, so
    /// re-arming shows the current look immediately.
    fn set_armed(&mut self, armed: bool) -> Result<()>;
    fn send(&mut self, points: &[Point]);
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

    pub fn emit(&mut self, frame: &[Point], gate_armed: bool, estop: &EStop) -> Emitted {
        let armed = gate_armed && !estop.is_latched();
        let mut sent = frame.to_vec();
        blank_unless(armed, &mut sent);
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

    pub fn shutdown(&mut self) {
        if let Some(out) = self.output.as_mut() {
            let _ = out.set_armed(false);
        }
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
    }

    impl Output for Probe {
        fn name(&self) -> &str {
            "probe"
        }
        fn set_armed(&mut self, armed: bool) -> Result<()> {
            self.armed.lock().unwrap().push(armed);
            Ok(())
        }
        fn send(&mut self, points: &[Point]) {
            self.frames.lock().unwrap().push(points.to_vec());
        }
    }

    fn lit() -> Vec<Point> {
        vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), Point::lit(0.2, 0.2, 0.0, 1.0, 0.0)]
    }

    #[test]
    fn disarmed_stage_sends_only_blank_points() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let e = stage.emit(&lit(), false, &EStop::default());
        assert_eq!(e.lit, 0);
        assert!(e.arm_change.is_none(), "starts disarmed: nothing to change");
        assert!(probe.frames.lock().unwrap()[0].iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn armed_stage_sends_the_frame_and_arms_once() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let estop = EStop::default();
        assert_eq!(stage.emit(&lit(), true, &estop).lit, 2);
        assert_eq!(stage.emit(&lit(), true, &estop).lit, 2);
        assert_eq!(*probe.armed.lock().unwrap(), vec![true]);
    }

    /// An e-stop tripped mid-frame (after the gate state was read under the
    /// lock, gate still "armed") blanks the very next emitted frame.
    #[test]
    fn estop_blanks_the_next_frame_even_before_the_gate_syncs() {
        let probe = Probe::default();
        let mut stage = OutputStage::new(Some(Box::new(probe.clone())));
        let estop = EStop::default();
        stage.emit(&lit(), true, &estop);
        estop.trip(ArmSource::Keyboard);
        let e = stage.emit(&lit(), true, &estop);
        assert_eq!(e.lit, 0);
        assert!(probe.frames.lock().unwrap().last().unwrap().iter().all(|p| !p.is_lit()));
        assert_eq!(*probe.armed.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn preview_only_stage_still_reports_gated_points() {
        let mut stage = OutputStage::new(None);
        assert_eq!(stage.emit(&lit(), false, &EStop::default()).lit, 0);
        assert_eq!(stage.emit(&lit(), true, &EStop::default()).lit, 2);
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
