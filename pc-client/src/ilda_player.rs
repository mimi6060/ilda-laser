//! Loads standard `.ild` (ILDA Image Data Transfer) files and converts them
//! into the DAC-agnostic `LaserPoint` frames used by `laser_dac`.

use anyhow::{Context, Result};
use ilda::animation::Animation;
use laser_dac::LaserPoint;

/// ILDA coordinates are signed 16-bit (-32768..32767). Normalize to the
/// crate's -1.0..1.0 convention.
fn coord_to_f32(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(-1.0, 1.0)
}

/// Scale an 8-bit ILDA color channel (0-255) up to the crate's 16-bit
/// convention (0-65535), mapping 0->0 and 255->65535 exactly.
fn color_to_u16(v: u8) -> u16 {
    v as u16 * 257
}

/// Load an `.ild` file and return one `Vec<LaserPoint>` per ILDA frame, in
/// file order.
pub fn load_frames(path: &str) -> Result<Vec<Vec<LaserPoint>>> {
    let animation = Animation::read_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("failed to parse ILDA file '{path}'"))?;

    let frames = animation
        .get_frames()
        .iter()
        .map(|frame| {
            frame
                .get_points()
                .iter()
                .map(|sp| {
                    if sp.is_blank {
                        LaserPoint::blanked(coord_to_f32(sp.x), coord_to_f32(sp.y))
                    } else {
                        let r = color_to_u16(sp.r);
                        let g = color_to_u16(sp.g);
                        let b = color_to_u16(sp.b);
                        // ILDA points don't carry a separate intensity
                        // channel; drive the DAC's intensity input with the
                        // brightest color component so backends that use it
                        // for modulation still light the beam.
                        let intensity = r.max(g).max(b);
                        LaserPoint::new(coord_to_f32(sp.x), coord_to_f32(sp.y), r, g, b, intensity)
                    }
                })
                .collect()
        })
        .collect();

    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_conversion_maps_extremes_to_unit_range() {
        assert_eq!(coord_to_f32(0), 0.0);
        assert!((coord_to_f32(32767) - 1.0).abs() < 1e-6);
        assert!((coord_to_f32(-32767) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn color_conversion_is_exact_at_the_boundaries() {
        assert_eq!(color_to_u16(0), 0);
        assert_eq!(color_to_u16(255), 65535);
    }

    #[test]
    fn missing_file_produces_a_readable_error() {
        let err = load_frames("/no/such/file.ild").unwrap_err();
        assert!(err.to_string().contains("failed to parse ILDA file"));
    }
}
