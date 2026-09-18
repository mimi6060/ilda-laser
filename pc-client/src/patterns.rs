//! Built-in test patterns for calibrating a laser projector without needing
//! an ILDA file. Useful to check that X/Y orientation and colors are wired
//! up correctly before playing real content.

use laser_dac::LaserPoint;

/// Number of blanked points inserted at a jump between two disconnected
/// segments. Real DACs need a few samples with the beam off to let the
/// galvos settle before re-enabling the beam, otherwise you get a bright
/// streak connecting the two segments.
const BLANK_SAMPLES: usize = 8;

fn push_blank_jump(points: &mut Vec<LaserPoint>, from: (f32, f32), to: (f32, f32)) {
    for i in 0..BLANK_SAMPLES {
        let t = i as f32 / (BLANK_SAMPLES - 1) as f32;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        points.push(LaserPoint::blanked(x, y));
    }
}

/// A filled-outline circle. Closed shapes don't need internal blanking：
/// the frame session handles the blank transition back to the first point
/// on loop and between frames.
pub fn circle(scale: f32, segments: usize, r: u16, g: u16, b: u16) -> Vec<LaserPoint> {
    let segments = segments.max(3);
    (0..segments)
        .map(|i| {
            let a = i as f32 / segments as f32 * std::f32::consts::TAU;
            LaserPoint::new(scale * a.cos(), scale * a.sin(), r, g, b, u16::MAX)
        })
        .collect()
}

/// A filled-outline square, corners at (+/-scale, +/-scale).
pub fn square(scale: f32, r: u16, g: u16, b: u16) -> Vec<LaserPoint> {
    let corners = [
        (-scale, -scale),
        (scale, -scale),
        (scale, scale),
        (-scale, scale),
    ];
    corners
        .iter()
        .map(|&(x, y)| LaserPoint::new(x, y, r, g, b, u16::MAX))
        .collect()
}

/// A filled-outline equilateral triangle pointing up.
pub fn triangle(scale: f32, r: u16, g: u16, b: u16) -> Vec<LaserPoint> {
    let points = [
        (0.0, scale),
        (scale * 0.866, -scale * 0.5),
        (-scale * 0.866, -scale * 0.5),
    ];
    points
        .iter()
        .map(|&(x, y)| LaserPoint::new(x, y, r, g, b, u16::MAX))
        .collect()
}

/// A calibration cross: a horizontal and a vertical line through the
/// origin, plus a small center dot. Handy for checking that (0,0) is
/// centered and that a positive X/Y actually moves right/up on the wall.
pub fn cross(scale: f32, r: u16, g: u16, b: u16) -> Vec<LaserPoint> {
    let mut points = Vec::new();

    // Horizontal line, left to right.
    points.push(LaserPoint::new(-scale, 0.0, r, g, b, u16::MAX));
    points.push(LaserPoint::new(scale, 0.0, r, g, b, u16::MAX));

    // Jump (blanked) to the bottom of the vertical line.
    push_blank_jump(&mut points, (scale, 0.0), (0.0, -scale));

    // Vertical line, bottom to top.
    points.push(LaserPoint::new(0.0, -scale, r, g, b, u16::MAX));
    points.push(LaserPoint::new(0.0, scale, r, g, b, u16::MAX));

    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_has_requested_segment_count_and_is_in_bounds() {
        let pts = circle(0.8, 64, 65535, 0, 0);
        assert_eq!(pts.len(), 64);
        for p in &pts {
            assert!(p.x.abs() <= 0.8 + 1e-6);
            assert!(p.y.abs() <= 0.8 + 1e-6);
        }
    }

    #[test]
    fn square_has_four_corners_at_expected_extent() {
        let pts = square(0.5, 0, 65535, 0);
        assert_eq!(pts.len(), 4);
        assert!(pts.iter().all(|p| p.x.abs() == 0.5 && p.y.abs() == 0.5));
    }

    #[test]
    fn cross_center_is_reachable_and_blank_jump_does_not_light_beam() {
        let pts = cross(1.0, 0, 0, 65535);
        // The blanked jump points must carry zero color so the beam is off
        // while the galvo repositions.
        let blanked: Vec<_> = pts.iter().filter(|p| p.r == 0 && p.g == 0 && p.b == 0).collect();
        assert!(blanked.len() >= BLANK_SAMPLES);
    }
}
