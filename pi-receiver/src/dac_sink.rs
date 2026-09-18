//! Shared analog output sink: writes one laser point (normalized
//! coordinates/colors) out to the three MCP4922 SPI DACs.
//!
//! Both the IDN network receiver and the local web control panel need to
//! push points to the same physical DACs, so this lives behind a mutex
//! (see `Arc<Mutex<DacSink>>` in `main.rs`) rather than being owned
//! exclusively by either one.

use crate::mcp4922::{normalized_to_12bit, Channel, Mcp4922};
use anyhow::Result;
use log::warn;
use serde::{Deserialize, Serialize};

/// Global output alignment/correction, applied to every point regardless
/// of source (network content or the web UI's own patterns) - the laser
/// hardware equivalent of a projector's keystone/position/size settings.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Calibration {
    pub offset_x: f32,
    pub offset_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_deg: f32,
}

impl Default for Calibration {
    fn default() -> Self {
        Self { offset_x: 0.0, offset_y: 0.0, scale_x: 1.0, scale_y: 1.0, rotation_deg: 0.0 }
    }
}

impl Calibration {
    /// Rotate around the origin, scale, then offset - in that order, so
    /// rotation always happens around the pattern's own center rather than
    /// an already-shifted point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let rad = self.rotation_deg.to_radians();
        let (sin, cos) = rad.sin_cos();
        let xr = x * cos - y * sin;
        let yr = x * sin + y * cos;
        (
            (xr * self.scale_x + self.offset_x).clamp(-1.0, 1.0),
            (yr * self.scale_y + self.offset_y).clamp(-1.0, 1.0),
        )
    }
}

pub struct DacSink {
    xy: Mcp4922,
    rg: Mcp4922,
    b: Mcp4922,
    calibration: Calibration,
}

impl DacSink {
    pub fn new(xy: Mcp4922, rg: Mcp4922, b: Mcp4922, calibration: Calibration) -> Self {
        Self { xy, rg, b, calibration }
    }

    pub fn set_calibration(&mut self, calibration: Calibration) {
        self.calibration = calibration;
    }

    pub fn calibration(&self) -> Calibration {
        self.calibration
    }

    /// `x`, `y` in -1.0..=1.0 (pre-calibration); `r`, `g`, `b` in 0.0..=1.0.
    /// Errors on individual channel writes are logged and otherwise
    /// ignored, so one bad SPI transaction doesn't stop the rest of the
    /// point from being written.
    pub fn write_point(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32) {
        let (x, y) = self.calibration.apply(x, y);

        let x = normalized_to_12bit(x, -1.0, 1.0);
        let y = normalized_to_12bit(y, -1.0, 1.0);
        let r = normalized_to_12bit(r, 0.0, 1.0);
        let g = normalized_to_12bit(g, 0.0, 1.0);
        let b = normalized_to_12bit(b, 0.0, 1.0);

        log_write(self.xy.write(Channel::A, x), "xy/x");
        log_write(self.xy.write(Channel::B, y), "xy/y");
        log_write(self.rg.write(Channel::A, r), "rg/r");
        log_write(self.rg.write(Channel::B, g), "rg/g");
        log_write(self.b.write(Channel::A, b), "b/b");
    }

    /// Turn the beam off (zero color, centered position) - used when
    /// stopping playback or on shutdown.
    pub fn blank(&mut self) {
        self.write_point(0.0, 0.0, 0.0, 0.0, 0.0);
    }
}

fn log_write(result: Result<()>, label: &str) {
    if let Err(e) = result {
        warn!("DAC write failed ({label}): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_calibration_is_identity() {
        let c = Calibration::default();
        let (x, y) = c.apply(0.3, -0.4);
        assert!((x - 0.3).abs() < 1e-6);
        assert!((y - (-0.4)).abs() < 1e-6);
    }

    #[test]
    fn calibration_offset_shifts_center() {
        let c = Calibration { offset_x: 0.2, offset_y: -0.1, ..Default::default() };
        let (x, y) = c.apply(0.0, 0.0);
        assert!((x - 0.2).abs() < 1e-6);
        assert!((y + 0.1).abs() < 1e-6);
    }

    #[test]
    fn calibration_clamps_output_to_valid_range() {
        let c = Calibration { scale_x: 3.0, scale_y: 3.0, ..Default::default() };
        let (x, y) = c.apply(1.0, 1.0);
        assert_eq!(x, 1.0);
        assert_eq!(y, 1.0);
    }

    #[test]
    fn calibration_rotation_by_90_degrees_swaps_axes() {
        let c = Calibration { rotation_deg: 90.0, ..Default::default() };
        let (x, y) = c.apply(1.0, 0.0);
        assert!(x.abs() < 1e-5);
        assert!((y - 1.0).abs() < 1e-5);
    }
}
