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

pub struct DacSink {
    xy: Mcp4922,
    rg: Mcp4922,
    b: Mcp4922,
}

impl DacSink {
    pub fn new(xy: Mcp4922, rg: Mcp4922, b: Mcp4922) -> Self {
        Self { xy, rg, b }
    }

    /// `x`, `y` in -1.0..=1.0; `r`, `g`, `b` in 0.0..=1.0. Errors on
    /// individual channel writes are logged and otherwise ignored, so one
    /// bad SPI transaction doesn't stop the rest of the point from being
    /// written.
    pub fn write_point(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32) {
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
