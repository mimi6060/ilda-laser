//! Built-in calibration/test shapes for the web control panel. Same shapes
//! as `pc-client`'s `patterns.rs`, but working directly in this crate's
//! normalized `Point` (x/y -1.0..=1.0, r/g/b 0.0..=1.0) instead of
//! `laser_dac::LaserPoint`, since `pi-receiver` doesn't depend on that
//! crate's client-side types.

#[derive(Copy, Clone, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Point {
    fn lit(x: f32, y: f32, r: f32, g: f32, b: f32) -> Self {
        Self { x, y, r, g, b }
    }

    fn blanked(x: f32, y: f32) -> Self {
        Self { x, y, r: 0.0, g: 0.0, b: 0.0 }
    }
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
    (0..segments)
        .map(|i| {
            let a = i as f32 / segments as f32 * std::f32::consts::TAU;
            Point::lit(scale * a.cos(), scale * a.sin(), r, g, b)
        })
        .collect()
}

pub fn square(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    [(-scale, -scale), (scale, -scale), (scale, scale), (-scale, scale)]
        .iter()
        .map(|&(x, y)| Point::lit(x, y, r, g, b))
        .collect()
}

pub fn triangle(scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    [
        (0.0, scale),
        (scale * 0.866, -scale * 0.5),
        (-scale * 0.866, -scale * 0.5),
    ]
    .iter()
    .map(|&(x, y)| Point::lit(x, y, r, g, b))
    .collect()
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

/// Look up a shape by the web UI's form value.
pub fn by_name(name: &str, scale: f32, r: f32, g: f32, b: f32) -> Option<Vec<Point>> {
    match name {
        "circle" => Some(circle(scale, r, g, b)),
        "square" => Some(square(scale, r, g, b)),
        "triangle" => Some(triangle(scale, r, g, b)),
        "cross" => Some(cross(scale, r, g, b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_stays_in_bounds() {
        let pts = circle(0.8, 1.0, 0.0, 0.0);
        assert_eq!(pts.len(), 120);
        assert!(pts.iter().all(|p| p.x.abs() <= 0.8 + 1e-6 && p.y.abs() <= 0.8 + 1e-6));
    }

    #[test]
    fn by_name_rejects_unknown_shape() {
        assert!(by_name("hexagon", 0.5, 1.0, 1.0, 1.0).is_none());
    }

    #[test]
    fn by_name_dispatches_correctly() {
        assert_eq!(by_name("square", 0.5, 0.0, 1.0, 0.0).unwrap().len(), 4);
        assert_eq!(by_name("triangle", 0.5, 0.0, 1.0, 0.0).unwrap().len(), 3);
    }
}
