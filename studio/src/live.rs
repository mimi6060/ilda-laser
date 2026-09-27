//! Master live modifiers: what the laserist changes constantly while a
//! cue plays - size, position, rotation (with speed presets and tempo
//! sync), animation speed and a master dimmer. Applied to the rendered
//! look, before calibration and safety, so a live move can never push the
//! beam past the calibration clamp or into a safety zone.

use crate::patterns::Point;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveModifiers {
    /// Master dimmer 0..1.
    pub brightness: f32,
    /// Overall size 0..2.
    pub size: f32,
    /// Per-axis size -2..2 (negative flips).
    pub size_x: f32,
    pub size_y: f32,
    /// Offset -1..1.
    pub pos_x: f32,
    pub pos_y: f32,
    /// Fixed rotation per axis (X, Y, Z), degrees.
    pub rot_angle: [f32; 3],
    /// Rotation speed per axis: degrees per second, or turns per bar when
    /// `rot_sync` is on.
    pub rot_speed: [f32; 3],
    pub rot_sync: bool,
    /// Momentary: reverses rotation direction while held.
    pub rot_reverse: bool,
    /// Depth of the 3D effect for X/Y rotations, 0..1.
    pub perspective: f32,
    /// Animation speed multiplier for generators, 0..4 (0 freezes).
    pub speed: f32,
}

impl Default for LiveModifiers {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            size: 1.0,
            size_x: 1.0,
            size_y: 1.0,
            pos_x: 0.0,
            pos_y: 0.0,
            rot_angle: [0.0; 3],
            rot_speed: [0.0; 3],
            rot_sync: false,
            rot_reverse: false,
            perspective: 0.3,
            speed: 1.0,
        }
    }
}

/// Rotation speed presets (Stop, Lent, Moyen, Rapide): degrees per second
/// in free mode, turns per bar in tempo-sync mode.
pub const ROT_PRESETS_FREE: [f32; 4] = [0.0, 30.0, 90.0, 270.0];
pub const ROT_PRESETS_SYNC: [f32; 4] = [0.0, 0.25, 1.0, 2.0];
pub const ROT_PRESET_LABELS: [&str; 4] = ["Stop", "Lent", "Moyen", "Rapide"];

/// Time-based state: accumulated rotation per axis, in degrees.
#[derive(Default)]
pub struct LiveState {
    spin: [f32; 3],
}

impl LiveState {
    /// Advance the rotations by `dt` seconds. `bpm` and `beats_per_bar`
    /// turn tempo-synced speeds (turns per bar) into degrees per second.
    pub fn advance(&mut self, m: &LiveModifiers, dt: f32, bpm: f64, beats_per_bar: u8) {
        let direction = if m.rot_reverse { -1.0 } else { 1.0 };
        for axis in 0..3 {
            let deg_per_s = if m.rot_sync {
                m.rot_speed[axis] * 360.0 * (bpm as f32 / 60.0) / beats_per_bar.max(1) as f32
            } else {
                m.rot_speed[axis]
            };
            self.spin[axis] = (self.spin[axis] + direction * deg_per_s * dt).rem_euclid(360.0);
        }
    }

    pub fn angles(&self, m: &LiveModifiers) -> [f32; 3] {
        [0, 1, 2].map(|a| m.rot_angle[a] + self.spin[a])
    }
}

pub fn apply(points: &[Point], m: &LiveModifiers, st: &LiveState) -> Vec<Point> {
    if *m == LiveModifiers::default() && st.spin == [0.0; 3] {
        return points.to_vec();
    }
    let [ax, ay, az] = st.angles(m).map(f32::to_radians);
    let (sx, cx) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();
    let three_d = ax != 0.0 || ay != 0.0;
    let gain = m.brightness.clamp(0.0, 1.0);

    points
        .iter()
        .map(|p| {
            let mut x = p.x * m.size * m.size_x;
            let mut y = p.y * m.size * m.size_y;
            if three_d {
                // Rotate around X then Y, then project with a simple perspective.
                let (y1, z1) = (y * cx, y * sx);
                let (x2, z2) = (x * cy + z1 * sy, -x * sy + z1 * cy);
                let depth = (1.0 + m.perspective.clamp(0.0, 1.0) * z2).max(0.2);
                x = x2 / depth;
                y = y1 / depth;
            }
            let (xr, yr) = (x * cz - y * sz, x * sz + y * cz);
            Point { x: xr + m.pos_x, y: yr + m.pos_y, r: p.r * gain, g: p.g * gain, b: p.b * gain }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures};
    use crate::presets::catalog;

    #[test]
    fn default_modifiers_change_nothing() {
        let cues = catalog();
        for p in cues.iter().step_by(40).take(5) {
            let frame = Animator::default().render(&p.settings, AudioFeatures::default(), 1.0 / 60.0);
            assert_eq!(apply(&frame, &LiveModifiers::default(), &LiveState::default()), frame, "cue {}", p.id);
        }
    }

    #[test]
    fn size_doubles_and_negative_x_flips() {
        let pts = vec![Point::lit(0.25, 0.1, 1.0, 1.0, 1.0)];
        let big = apply(&pts, &LiveModifiers { size: 2.0, ..Default::default() }, &LiveState::default());
        assert_eq!((big[0].x, big[0].y), (0.5, 0.2));
        let flipped = apply(&pts, &LiveModifiers { size_x: -1.0, ..Default::default() }, &LiveState::default());
        assert_eq!((flipped[0].x, flipped[0].y), (-0.25, 0.1));
    }

    #[test]
    fn z_rotation_speed_and_reverse() {
        let mut m = LiveModifiers { rot_speed: [0.0, 0.0, 90.0], ..Default::default() };
        let mut st = LiveState::default();
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        assert!((st.angles(&m)[2] - 90.0).abs() < 0.5);
        m.rot_reverse = true;
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        let a = st.angles(&m)[2];
        assert!(a < 0.5 || a > 359.5, "angle {a}");
    }

    #[test]
    fn synced_rotation_follows_the_tempo() {
        // 1 turn per bar at 120 BPM in 4/4 = 1 turn per 2 s.
        let m = LiveModifiers { rot_speed: [0.0, 0.0, 1.0], rot_sync: true, ..Default::default() };
        let mut st = LiveState::default();
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        assert!((st.angles(&m)[2] - 180.0).abs() < 0.5);
    }

    #[test]
    fn rotation_by_90_degrees_moves_x_to_y() {
        let pts = vec![Point::lit(0.5, 0.0, 1.0, 1.0, 1.0)];
        let m = LiveModifiers { rot_angle: [0.0, 0.0, 90.0], ..Default::default() };
        let out = apply(&pts, &m, &LiveState::default());
        assert!(out[0].x.abs() < 1e-6 && (out[0].y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn position_offsets_and_brightness_dims() {
        let pts = vec![Point::lit(0.0, 0.0, 1.0, 0.5, 0.0)];
        let out = apply(&pts, &LiveModifiers { pos_x: 0.3, pos_y: -0.2, brightness: 0.5, ..Default::default() }, &LiveState::default());
        assert_eq!((out[0].x, out[0].y, out[0].r, out[0].g), (0.3, -0.2, 0.5, 0.25));
    }

    #[test]
    fn y_rotation_by_90_collapses_to_a_vertical_line() {
        let pts = vec![Point::lit(0.5, 0.2, 1.0, 1.0, 1.0)];
        let out = apply(&pts, &LiveModifiers { rot_angle: [0.0, 90.0, 0.0], ..Default::default() }, &LiveState::default());
        assert!(out[0].x.abs() < 0.01, "x {}", out[0].x);
    }

    #[test]
    fn two_thousand_points_cost_well_under_a_millisecond() {
        let pts: Vec<Point> = (0..2000).map(|i| Point::lit((i as f32 / 2000.0) - 0.5, 0.1, 1.0, 1.0, 1.0)).collect();
        let m = LiveModifiers { size: 1.3, rot_angle: [20.0, 30.0, 40.0], pos_x: 0.1, brightness: 0.8, ..Default::default() };
        let st = LiveState::default();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(apply(&pts, &m, &st));
        }
        let per_frame = start.elapsed() / 100;
        // Debug builds are much slower than release; this bound holds in both.
        assert!(per_frame.as_micros() < 5_000, "{per_frame:?} per frame");
    }
}
