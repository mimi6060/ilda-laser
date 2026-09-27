//! Sends rendered frames to a real laser DAC through `laser-dac` (IDN or
//! Ether Dream today). The ShowNET will plug in here as another `Output`
//! once Laserworld's API is available - the engine and UI don't change.

use crate::patterns::Point;
use anyhow::{Context, Result};
use laser_dac::{list_devices, open_device, Frame, FrameSession, FrameSessionConfig, LaserPoint};

pub trait Output: Send {
    fn name(&self) -> &str;
    /// Laser emission on/off. Frames keep flowing while disarmed, so
    /// re-arming shows the current look immediately.
    fn set_armed(&mut self, armed: bool) -> Result<()>;
    fn send(&mut self, points: &[Point]);
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
