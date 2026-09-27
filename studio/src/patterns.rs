//! Built-in shapes, in normalized `Point`s (x/y -1.0..=1.0, r/g/b
//! 0.0..=1.0). Started as a copy of `pi-receiver`'s shapes; the difference
//! is that closed outlines here repeat their first point at the end, since
//! `laser-dac`'s frame session blanks the jump from a frame's last point
//! back to its first - without the repeat, a square's closing edge would
//! never be drawn.

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Point {
    pub fn lit(x: f32, y: f32, r: f32, g: f32, b: f32) -> Self {
        Self { x, y, r, g, b }
    }

    pub fn blanked(x: f32, y: f32) -> Self {
        Self { x, y, r: 0.0, g: 0.0, b: 0.0 }
    }

    pub fn is_lit(&self) -> bool {
        self.r > 0.0 || self.g > 0.0 || self.b > 0.0
    }
}

/// Repeat the first point at the end, so the outline's closing edge is
/// drawn instead of blanked (see the module docs).
fn closed(mut points: Vec<Point>) -> Vec<Point> {
    if let Some(&first) = points.first() {
        points.push(first);
    }
    points
}

const BLANK_SAMPLES: usize = 8;

fn push_blank_jump(points: &mut Vec<Point>, from: (f32, f32), to: (f32, f32)) {
    for i in 0..BLANK_SAMPLES {
        let t = i as f32 / (BLANK_SAMPLES - 1) as f32;
        points.push(Point::blanked(
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
        ));
    }
}

pub fn circle(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let segments = 120;
    closed((0..segments)
        .map(|i| {
            let a = i as f32 / segments as f32 * std::f32::consts::TAU;
            Point::lit(scale * a.cos(), scale * a.sin(), r, g, b)
        })
        .collect())
}

pub fn square(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    closed(
        [(-scale, -scale), (scale, -scale), (scale, scale), (-scale, scale)]
            .iter()
            .map(|&(x, y)| Point::lit(x, y, r, g, b))
            .collect(),
    )
}

pub fn triangle(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    closed(
        [
            (0.0, scale),
            (scale * 0.866, -scale * 0.5),
            (-scale * 0.866, -scale * 0.5),
        ]
        .iter()
        .map(|&(x, y)| Point::lit(x, y, r, g, b))
        .collect(),
    )
}

pub fn cross(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let mut points = Vec::new();
    points.push(Point::lit(-scale, 0.0, r, g, b));
    points.push(Point::lit(scale, 0.0, r, g, b));
    push_blank_jump(&mut points, (scale, 0.0), (0.0, -scale));
    points.push(Point::lit(0.0, -scale, r, g, b));
    points.push(Point::lit(0.0, scale, r, g, b));
    points
}

pub fn line(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    vec![Point::lit(-scale, 0.0, r, g, b), Point::lit(scale, 0.0, r, g, b)]
}

/// A five-point star outline, traced without lifting the beam (a standard
/// star polygon path: every second vertex of a 10-point circle).
pub fn star(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let outer = scale;
    let inner = scale * 0.382; // classic five-point star ratio
    closed(
        (0..10)
            .map(|i| {
                let radius = if i % 2 == 0 { outer } else { inner };
                let a = i as f32 / 10.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                Point::lit(radius * a.cos(), radius * a.sin(), r, g, b)
            })
            .collect(),
    )
}

/// An outward spiral, `turns` full rotations from center to `scale`.
pub fn spiral(scale: f32, turns: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let segments = (120.0 * turns.max(0.1)) as usize;
    (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let a = t * turns * std::f32::consts::TAU;
            let radius = t * scale;
            Point::lit(radius * a.cos(), radius * a.sin(), r, g, b)
        })
        .collect()
}

/// A grid of short dwell-points (each "dot" is a few coincident points so
/// it has visible duration/brightness), blanked jumps between them.
pub fn dots(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    const N: i32 = 4; // N x N grid
    const DWELL: usize = 6;

    let mut points = Vec::new();
    let mut last: Option<(f32, f32)> = None;

    for iy in 0..N {
        for ix in 0..N {
            let x = (ix as f32 / (N - 1) as f32 * 2.0 - 1.0) * scale;
            let y = (iy as f32 / (N - 1) as f32 * 2.0 - 1.0) * scale;
            if let Some(from) = last {
                push_blank_jump(&mut points, from, (x, y));
            }
            for _ in 0..DWELL {
                points.push(Point::lit(x, y, r, g, b));
            }
            last = Some((x, y));
        }
    }
    points
}

/// A horizontal sine wave, `amplitude` (fraction of `scale`) high, shifted
/// by `phase` radians - animate `phase` to make it travel.
pub fn wave(scale: f32, amplitude: f32, phase: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let segments = 100;
    (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let x = (t * 2.0 - 1.0) * scale;
            let y = amplitude * scale * (t * 2.0 * std::f32::consts::TAU + phase).sin();
            Point::lit(x, y, r, g, b)
        })
        .collect()
}

/// Look up a shape by the web UI's form value.
pub fn by_name(name: &str, scale: f32, r: f32, g: f32, b: f32) -> Option<Vec<Point>> {
    match name {
        "circle" => Some(circle(scale, r, g, b)),
        "square" => Some(square(scale, r, g, b)),
        "triangle" => Some(triangle(scale, r, g, b)),
        "cross" => Some(cross(scale, r, g, b)),
        "line" => Some(line(scale, r, g, b)),
        "star" => Some(star(scale, r, g, b)),
        "spiral" => Some(spiral(scale, 3.0, r, g, b)),
        "dots" => Some(dots(scale, r, g, b)),
        _ => None,
    }
}

/// Names accepted by `by_name`, in the order the web UI should list them.
pub const SHAPE_NAMES: &[&str] =
    &["circle", "square", "triangle", "cross", "line", "star", "spiral", "dots"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_stays_in_bounds() {
        let pts = circle(0.8, 1.0, 0.0, 0.0);
        assert_eq!(pts.len(), 121); // 120 segments + closing point
        assert!(pts.iter().all(|p| p.x.abs() <= 0.8 + 1e-6 && p.y.abs() <= 0.8 + 1e-6));
    }

    #[test]
    fn closed_shapes_end_where_they_start() {
        for name in ["circle", "square", "triangle", "star"] {
            let pts = by_name(name, 0.5, 1.0, 1.0, 1.0).unwrap();
            assert_eq!(pts.first(), pts.last(), "'{name}' is not closed");
        }
    }

    #[test]
    fn by_name_rejects_unknown_shape() {
        assert!(by_name("hexagon", 0.5, 1.0, 1.0, 1.0).is_none());
    }

    #[test]
    fn by_name_dispatches_correctly() {
        assert_eq!(by_name("square", 0.5, 0.0, 1.0, 0.0).unwrap().len(), 5);
        assert_eq!(by_name("triangle", 0.5, 0.0, 1.0, 0.0).unwrap().len(), 4);
    }

    #[test]
    fn star_alternates_outer_and_inner_radius() {
        let pts = star(1.0, 1.0, 1.0, 1.0);
        assert_eq!(pts.len(), 11);
        let r0 = (pts[0].x.powi(2) + pts[0].y.powi(2)).sqrt();
        let r1 = (pts[1].x.powi(2) + pts[1].y.powi(2)).sqrt();
        assert!(r0 > r1, "outer point should be farther from center than inner point");
    }

    #[test]
    fn spiral_starts_at_center_and_ends_near_scale() {
        let pts = spiral(1.0, 2.0, 1.0, 1.0, 1.0);
        let first_r = (pts[0].x.powi(2) + pts[0].y.powi(2)).sqrt();
        let last = pts.last().unwrap();
        let last_r = (last.x.powi(2) + last.y.powi(2)).sqrt();
        assert!(first_r < 0.05);
        assert!((last_r - 1.0).abs() < 0.05);
    }

    #[test]
    fn dots_produces_16_lit_points_for_a_4x4_grid() {
        let pts = dots(1.0, 1.0, 1.0, 1.0);
        let lit = pts.iter().filter(|p| p.r > 0.0 || p.g > 0.0 || p.b > 0.0).count();
        assert_eq!(lit, 16 * 6); // 16 grid cells x 6 dwell points each
    }

    #[test]
    fn all_shape_names_are_dispatchable() {
        for name in SHAPE_NAMES {
            assert!(by_name(name, 0.5, 1.0, 1.0, 1.0).is_some(), "'{name}' not handled by by_name");
        }
    }
}
