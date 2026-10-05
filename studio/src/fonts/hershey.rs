//! Reader for Hershey fonts in James Hurt's `.jhf` format (our own code;
//! the data files are in `studio/assets/fonts/hershey/`, licence next to
//! them).
//!
//! One glyph per record: the glyph number in columns 0..5, the number of
//! coordinate pairs in columns 5..8, then the pairs as two characters
//! each, relative to `'R'`. The first pair is the glyph's left and right
//! side; `" R"` lifts the pen. Y grows downwards. A long record may be
//! wrapped over several lines: the pair count says where it ends.

use super::Glyph;
use anyhow::{bail, Result};

/// One record of a `.jhf` file, in the file's own coordinates.
#[derive(Debug, PartialEq)]
pub struct JhfGlyph {
    pub left: i32,
    pub right: i32,
    /// Pen-down polylines, (x, y) with y down.
    pub strokes: Vec<Vec<(i32, i32)>>,
}

/// Every record of a `.jhf` file, in file order.
pub fn parse(src: &str) -> Result<Vec<JhfGlyph>> {
    let mut out = Vec::new();
    let mut lines = src.lines().filter(|l| !l.trim().is_empty());
    while let Some(first) = lines.next() {
        let mut rec: Vec<u8> = first.as_bytes().to_vec();
        if rec.len() < 8 {
            bail!("enregistrement Hershey trop court : {first:?}");
        }
        let count: usize = std::str::from_utf8(&rec[5..8])?.trim().parse()?;
        if count == 0 {
            bail!("enregistrement Hershey sans côtés : {first:?}");
        }
        let need = 8 + 2 * count;
        while rec.len() < need {
            match lines.next() {
                Some(more) => rec.extend_from_slice(more.as_bytes()),
                None => bail!("enregistrement Hershey tronqué : {first:?}"),
            }
        }
        let coord = |b: u8| b as i32 - b'R' as i32;
        let pairs = &rec[8..need];
        let (left, right) = (coord(pairs[0]), coord(pairs[1]));
        let mut strokes: Vec<Vec<(i32, i32)>> = Vec::new();
        let mut current: Vec<(i32, i32)> = Vec::new();
        for p in pairs[2..].chunks(2) {
            if p == b" R" {
                if current.len() >= 2 {
                    strokes.push(std::mem::take(&mut current));
                }
                current.clear();
            } else {
                current.push((coord(p[0]), coord(p[1])));
            }
        }
        if current.len() >= 2 {
            strokes.push(current);
        }
        out.push(JhfGlyph { left, right, strokes });
    }
    Ok(out)
}

/// A `.jhf` file holding the printable ASCII set (space first, as the
/// Roman fonts are distributed) as glyphs in our units: cap height 2
/// (measured on `H`), baseline at 0, x from the glyph's left side.
pub fn ascii_font(src: &str) -> Result<Vec<(char, Glyph)>> {
    let raw = parse(src)?;
    let h = raw.get(usize::from(b'H' - b' ')).filter(|g| !g.strokes.is_empty());
    let Some(h) = h else { bail!("police Hershey sans « H »") };
    let ys = h.strokes.iter().flatten().map(|p| p.1);
    let (top, base) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
    if base <= top {
        bail!("police Hershey : « H » sans hauteur");
    }
    let k = 2.0 / (base - top) as f32;
    Ok(raw
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let c = char::from_u32(32 + i as u32).filter(|c| c.is_ascii() && !c.is_ascii_control())?;
            let strokes = g
                .strokes
                .iter()
                .map(|s| s.iter().map(|&(x, y)| [(x - g.left) as f32 * k, (base - y) as f32 * k]).collect())
                .collect();
            Some((c, Glyph { strokes, advance: (g.right - g.left) as f32 * k }))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hand-written records (not the shipped files): an « H » like the
    // format's own example, and a glyph wrapped over two lines.
    const H: &str = "    8  9MWOMOV RUMUV ROQUQ\n";

    #[test]
    fn parses_a_handwritten_glyph() {
        let g = parse(H).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].left, g[0].right), (-5, 5));
        assert_eq!(g[0].strokes, vec![vec![(-3, -5), (-3, 4)], vec![(3, -5), (3, 4)], vec![(-3, -1), (3, -1)]]);
    }

    #[test]
    fn a_record_may_wrap_over_lines() {
        let g = parse("    1  6MWOMOV\n RUMUV\n").unwrap();
        assert_eq!(g[0].strokes, vec![vec![(-3, -5), (-3, 4)], vec![(3, -5), (3, 4)]]);
    }

    #[test]
    fn truncated_or_garbage_records_are_errors() {
        assert!(parse("    1  9MWOM\n").is_err());
        assert!(parse("abc\n").is_err());
        assert!(parse("    1 xxMWOM\n").is_err());
    }

    #[test]
    fn ascii_font_normalises_on_the_h() {
        // Space, then 39 empty records, then « H » at index 40.
        let mut src = String::new();
        for _ in 0..40 {
            src.push_str("    1  1JZ\n");
        }
        src.push_str(H);
        let font = ascii_font(&src).unwrap();
        let (c, h) = font.iter().find(|(c, _)| *c == 'H').unwrap();
        assert_eq!(*c, 'H');
        let ys: Vec<f32> = h.strokes.iter().flatten().map(|p| p[1]).collect();
        assert!((ys.iter().cloned().fold(f32::MIN, f32::max) - 2.0).abs() < 1e-6);
        assert!(ys.iter().cloned().fold(f32::MAX, f32::min).abs() < 1e-6);
        assert!((h.advance - 10.0 * 2.0 / 9.0).abs() < 1e-6);
        assert_eq!(font[0].0, ' ');
    }
}
