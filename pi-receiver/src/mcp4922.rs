//! Minimal driver for the Microchip MCP4922 dual 12-bit SPI DAC.
//!
//! Protocol: one 16-bit big-endian word per channel update.
//!
//! ```text
//! bit15    channel select (0 = DAC A, 1 = DAC B)
//! bit14    buffered input (always 1 here)
//! bit13    gain (1 = 1x, 0 = 2x)         - always 1x here
//! bit12    SHDN (1 = active, 0 = shutdown)
//! bits11-0 12-bit value, MSB first
//! ```
//!
//! The chip latches its output on the rising edge of Chip Select, so the two
//! channels on one chip are necessarily two separate SPI transactions - they
//! cannot be updated in the exact same instant from software alone. For an
//! X/Y pair this shows up as a small timing skew between axes. If that turns
//! out to be visible in practice, the fix is a shared hardware LDAC line
//! across all three chips (not implemented here).

use anyhow::{Context, Result};
use rppal::spi::Spi;

#[derive(Copy, Clone)]
pub enum Channel {
    A,
    B,
}

pub struct Mcp4922 {
    spi: Spi,
}

impl Mcp4922 {
    pub fn new(spi: Spi) -> Self {
        Self { spi }
    }

    /// Write a 12-bit value (0-4095; out-of-range bits are masked off) to
    /// one channel.
    pub fn write(&mut self, channel: Channel, value: u16) -> Result<()> {
        let word = encode_word(channel, value);
        self.spi
            .write(&word.to_be_bytes())
            .context("SPI write to MCP4922 failed")?;
        Ok(())
    }
}

fn encode_word(channel: Channel, value: u16) -> u16 {
    let mut word: u16 = 0b0111_0000_0000_0000; // BUF=1, GA=1x, SHDN=active
    if matches!(channel, Channel::B) {
        word |= 1 << 15;
    }
    word |= value & 0x0FFF;
    word
}

/// Map a value in `min..=max` linearly onto the DAC's 12-bit range
/// (0..=4095), clamping out-of-range input.
pub fn normalized_to_12bit(v: f32, min: f32, max: f32) -> u16 {
    let t = ((v - min) / (max - min)).clamp(0.0, 1.0);
    (t * 4095.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_word_sets_channel_select_bit() {
        let a = encode_word(Channel::A, 0);
        let b = encode_word(Channel::B, 0);
        assert_eq!(a & (1 << 15), 0);
        assert_eq!(b & (1 << 15), 1 << 15);
    }

    #[test]
    fn encode_word_masks_value_to_12_bits() {
        let word = encode_word(Channel::A, 0xFFFF);
        assert_eq!(word & 0x0FFF, 0x0FFF);
    }

    #[test]
    fn encode_word_always_sets_buf_gain_shdn_bits() {
        let word = encode_word(Channel::A, 0);
        assert_eq!(word & 0b0111_0000_0000_0000, 0b0111_0000_0000_0000);
    }

    #[test]
    fn normalized_maps_endpoints_and_center() {
        assert_eq!(normalized_to_12bit(-1.0, -1.0, 1.0), 0);
        assert_eq!(normalized_to_12bit(1.0, -1.0, 1.0), 4095);
        assert_eq!(normalized_to_12bit(0.0, -1.0, 1.0), 2048); // rounds 2047.5
    }

    #[test]
    fn normalized_clamps_out_of_range_input() {
        assert_eq!(normalized_to_12bit(-5.0, -1.0, 1.0), 0);
        assert_eq!(normalized_to_12bit(5.0, -1.0, 1.0), 4095);
    }

    #[test]
    fn normalized_maps_unit_range() {
        assert_eq!(normalized_to_12bit(0.0, 0.0, 1.0), 0);
        assert_eq!(normalized_to_12bit(1.0, 0.0, 1.0), 4095);
    }
}
