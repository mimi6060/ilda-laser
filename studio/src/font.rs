//! A compact single-stroke vector font for laser text, built from
//! 7-segment-style primitives (plus a few diagonals for letters that don't
//! render legibly as pure segments). This is a simplified custom font, not
//! true Hershey/vector-font data - it prioritizes being unambiguously
//! correct to define over typographic faithfulness. Each glyph occupies a
//! 1 (wide) x 2 (tall) cell:
//!
//! ```text
//!   f--a--b
//!   |     |
//!   +--g--+
//!   |     |
//!   e--d--c
//! ```
//!
//! `glyph()` returns one or more strokes (polylines); the laser must blank
//! between strokes, since a single glyph is rarely drawable with the pen
//! never lifted.

use crate::patterns::Point;

type Stroke = Vec<(f32, f32)>;

// Segment endpoints in the cell described above.
const TOP_LEFT: (f32, f32) = (0.0, 2.0);
const TOP_RIGHT: (f32, f32) = (1.0, 2.0);
const MID_LEFT: (f32, f32) = (0.0, 1.0);
const MID_RIGHT: (f32, f32) = (1.0, 1.0);
const BOT_LEFT: (f32, f32) = (0.0, 0.0);
const BOT_RIGHT: (f32, f32) = (1.0, 0.0);

fn seg_a() -> Stroke { vec![TOP_LEFT, TOP_RIGHT] }
fn seg_b() -> Stroke { vec![TOP_RIGHT, MID_RIGHT] }
fn seg_c() -> Stroke { vec![MID_RIGHT, BOT_RIGHT] }
fn seg_d() -> Stroke { vec![BOT_LEFT, BOT_RIGHT] }
fn seg_e() -> Stroke { vec![BOT_LEFT, MID_LEFT] }
fn seg_f() -> Stroke { vec![MID_LEFT, TOP_LEFT] }
fn seg_g() -> Stroke { vec![MID_LEFT, MID_RIGHT] }

/// Look up the strokes for one glyph, in the 1x2 cell described above.
/// Unknown characters (and space) return no strokes - just advance the
/// cursor.
fn glyph(c: char) -> Vec<Stroke> {
    match c.to_ascii_uppercase() {
        '0' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_e(), seg_f()],
        '1' => vec![seg_b(), seg_c()],
        '2' => vec![seg_a(), seg_b(), seg_g(), seg_e(), seg_d()],
        '3' => vec![seg_a(), seg_b(), seg_g(), seg_c(), seg_d()],
        '4' => vec![seg_f(), seg_g(), seg_b(), seg_c()],
        '5' => vec![seg_a(), seg_f(), seg_g(), seg_c(), seg_d()],
        '6' => vec![seg_a(), seg_f(), seg_g(), seg_e(), seg_c(), seg_d()],
        '7' => vec![seg_a(), seg_b(), seg_c()],
        '8' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_e(), seg_f(), seg_g()],
        '9' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_f(), seg_g()],

        'A' => vec![seg_a(), seg_b(), seg_c(), seg_e(), seg_f(), seg_g()],
        'B' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_e(), seg_g()],
        'C' => vec![seg_a(), seg_f(), seg_e(), seg_d()],
        'D' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_g()],
        'E' => vec![seg_a(), seg_f(), seg_g(), seg_e(), seg_d()],
        'F' => vec![seg_a(), seg_f(), seg_g(), seg_e()],
        'G' => vec![seg_a(), seg_f(), seg_e(), seg_d(), seg_c(), seg_g()],
        'H' => vec![seg_f(), seg_e(), seg_g(), seg_b(), seg_c()],
        'I' => vec![vec![(0.5, 2.0), (0.5, 0.0)]],
        'J' => vec![seg_b(), seg_c(), seg_d(), seg_e()],
        // K: vertical left side plus two diagonals from mid-left.
        'K' => vec![seg_f(), seg_e(), vec![MID_LEFT, TOP_RIGHT], vec![MID_LEFT, BOT_RIGHT]],
        'L' => vec![seg_f(), seg_e(), seg_d()],
        // M: both verticals plus two diagonals meeting at top-middle.
        'M' => vec![seg_f(), seg_e(), vec![TOP_LEFT, (0.5, 1.2)], vec![(0.5, 1.2), TOP_RIGHT], seg_b(), seg_c()],
        // N: both verticals plus one diagonal corner-to-corner.
        'N' => vec![seg_f(), seg_e(), vec![TOP_LEFT, BOT_RIGHT], seg_b(), seg_c()],
        'O' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_e(), seg_f()],
        'P' => vec![seg_a(), seg_b(), seg_g(), seg_f(), seg_e()],
        // Q: like O plus a small tail diagonal at bottom-right.
        'Q' => vec![seg_a(), seg_b(), seg_c(), seg_d(), seg_e(), seg_f(), vec![MID_RIGHT, (1.15, -0.15)]],
        // R: like P plus a diagonal leg from the middle.
        'R' => vec![seg_a(), seg_b(), seg_g(), seg_f(), seg_e(), vec![MID_LEFT, BOT_RIGHT]],
        'S' => vec![seg_a(), seg_f(), seg_g(), seg_c(), seg_d()],
        'T' => vec![seg_a(), vec![(0.5, 2.0), (0.5, 0.0)]],
        'U' => vec![seg_f(), seg_e(), seg_d(), seg_c(), seg_b()],
        'V' => vec![vec![TOP_LEFT, (0.5, 0.0)], vec![(0.5, 0.0), TOP_RIGHT]],
        'W' => vec![seg_f(), seg_e(), vec![BOT_LEFT, (0.5, 0.8)], vec![(0.5, 0.8), BOT_RIGHT], seg_c(), seg_b()],
        'X' => vec![vec![TOP_LEFT, BOT_RIGHT], vec![TOP_RIGHT, BOT_LEFT]],
        'Y' => vec![vec![TOP_LEFT, (0.5, 1.0)], vec![TOP_RIGHT, (0.5, 1.0)], vec![(0.5, 1.0), (0.5, 0.0)]],
        'Z' => vec![seg_a(), seg_g(), seg_e(), seg_d(), vec![TOP_RIGHT, BOT_LEFT]],

        '-' => vec![seg_g()],
        '_' => vec![seg_d()],
        '.' => vec![vec![BOT_RIGHT, (BOT_RIGHT.0 + 0.02, BOT_RIGHT.1 + 0.02)]],
        ',' => vec![vec![BOT_RIGHT, (BOT_RIGHT.0 - 0.05, BOT_RIGHT.1 - 0.2)]],
        '!' => vec![vec![(0.5, 2.0), (0.5, 0.5)], vec![(0.5, 0.15), (0.5, 0.0)]],
        '\'' => vec![vec![TOP_LEFT, (0.0, 1.6)]],
        ' ' => vec![],
        _ => vec![],
    }
}

/// Lay out a string as laser points: each character's strokes traced in
/// order (blanked jump between strokes), with a blanked jump to the next
/// character's start. `scale` is the total cell height as a fraction of
/// the -1.0..=1.0 coordinate range; text is centered horizontally and
/// vertically around the origin.
pub fn text_to_points(text: &str, scale: f32, r: f32, g: f32, b: f32) -> Vec<Point> {
    let cell_w = scale * 0.7; // a bit narrower than tall, like real letterforms
    let cell_h = scale;
    let advance = cell_w * 1.3; // includes inter-letter spacing

    let chars: Vec<char> = text.chars().collect();
    let total_width = advance * chars.len() as f32;
    let start_x = -total_width / 2.0;
    let base_y = -cell_h / 2.0;

    let mut points = Vec::new();
    let mut cursor_x = start_x;
    let mut last_point: Option<(f32, f32)> = None;

    for c in chars {
        let strokes = glyph(c);

        for stroke in &strokes {
            if stroke.len() < 2 {
                continue;
            }
            let world: Vec<(f32, f32)> = stroke
                .iter()
                .map(|&(gx, gy)| (cursor_x + gx * cell_w, base_y + gy * cell_h))
                .collect();

            if let Some(from) = last_point {
                push_blank_jump(&mut points, from, world[0]);
            } else if let Some(&first) = world.first() {
                points.push(Point { x: first.0, y: first.1, r: 0.0, g: 0.0, b: 0.0 });
            }

            for &(x, y) in &world {
                points.push(Point { x, y, r, g, b });
            }
            last_point = world.last().copied();
        }

        cursor_x += advance;
    }

    points
}

fn push_blank_jump(points: &mut Vec<Point>, from: (f32, f32), to: (f32, f32)) {
    const SAMPLES: usize = 4;
    for i in 0..SAMPLES {
        let t = i as f32 / (SAMPLES - 1) as f32;
        points.push(Point {
            x: from.0 + (to.0 - from.0) * t,
            y: from.1 + (to.1 - from.1) * t,
            r: 0.0,
            g: 0.0,
            b: 0.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_letter_and_digit_has_strokes() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            let strokes = glyph(c);
            assert!(!strokes.is_empty(), "'{c}' has no strokes defined");
            for s in &strokes {
                assert!(s.len() >= 2, "'{c}' has a degenerate stroke with < 2 points");
            }
        }
    }

    #[test]
    fn space_and_unknown_chars_produce_no_strokes() {
        assert!(glyph(' ').is_empty());
        assert!(glyph('@').is_empty());
    }

    #[test]
    fn text_to_points_is_empty_for_empty_string() {
        assert!(text_to_points("", 1.0, 1.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn text_to_points_centers_around_origin() {
        // Two full-width glyphs (O uses the whole 0..1 cell), so the
        // rendered bounding box should come out close to symmetric. A
        // proportional-width string (e.g. one narrow glyph next to a wide
        // one) isn't expected to be perfectly symmetric - that's normal
        // for proportional fonts, not a bug.
        let pts = text_to_points("OO", 1.0, 1.0, 1.0, 1.0);
        assert!(!pts.is_empty());
        let min_x = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        assert!(((min_x + max_x) / 2.0).abs() < 0.2, "text not centered: {min_x}..{max_x}");
    }

    #[test]
    fn glyph_i_is_a_single_centered_stroke() {
        // Regression check for a real layout bug: I used to be drawn
        // hugging the left edge of its cell (x=0) instead of centered
        // (x=0.5), which threw off text centering for any string ending
        // in I.
        let strokes = glyph('I');
        assert_eq!(strokes.len(), 1);
        assert!(strokes[0].iter().all(|&(x, _)| (x - 0.5).abs() < 1e-6));
    }

    #[test]
    fn text_to_points_advances_between_letters() {
        let one = text_to_points("I", 1.0, 1.0, 1.0, 1.0);
        let two = text_to_points("II", 1.0, 1.0, 1.0, 1.0);
        assert!(two.len() > one.len());
    }
}
