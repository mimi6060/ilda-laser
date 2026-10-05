//! Reader for SVG 1.1 fonts (`<font>` / `<glyph d="…">`) whose paths
//! are open single-line strokes, as Relief SingleLine ships them (our own
//! code; the data file is in `studio/assets/fonts/relief/`, `OFL.txt`
//! next to it).
//!
//! Glyph paths are in font units with y up. Curves are flattened into
//! polylines with a tolerance, so detailed letters stay within a sane
//! point count. Only glyphs mapped to exactly one character are kept
//! (ligatures and unnamed glyphs are skipped).

use super::Glyph;
use anyhow::{anyhow, bail, Result};

/// Flattening tolerance, as a fraction of the cap height.
const FLATNESS: f64 = 0.006;
/// Most segments a cubic or quadratic is cut into.
const MAX_CURVE_STEPS: usize = 24;

/// Every single-character glyph of an SVG font, in our units: cap height
/// 2 (the `font-face` `cap-height`), baseline at 0.
pub fn parse(src: &str) -> Result<Vec<(char, Glyph)>> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(src, opts)?;
    let font = doc.descendants().find(|n| n.has_tag_name("font")).ok_or_else(|| anyhow!("pas d'élément <font>"))?;
    let default_adv: f64 = font.attribute("horiz-adv-x").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let face = font.children().find(|n| n.has_tag_name("font-face"));
    let num = |name: &str| face.and_then(|f| f.attribute(name)).and_then(|v| v.trim().parse::<f64>().ok());
    let cap = num("cap-height").or_else(|| num("units-per-em").map(|u| u * 0.7)).unwrap_or(700.0);
    if !(cap.is_finite() && cap > 0.0) {
        bail!("hauteur des capitales invalide");
    }
    let k = 2.0 / cap;
    let mut out = Vec::new();
    for g in font.children().filter(|n| n.has_tag_name("glyph")) {
        let Some(uni) = g.attribute("unicode") else { continue };
        let mut chars = uni.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else { continue };
        let adv = g.attribute("horiz-adv-x").and_then(|v| v.parse().ok()).unwrap_or(default_adv);
        let strokes = match g.attribute("d") {
            Some(d) => path(d, cap * FLATNESS)?,
            None => Vec::new(),
        };
        let strokes = strokes
            .into_iter()
            .map(|s| s.into_iter().map(|[x, y]| [(x * k) as f32, (y * k) as f32]).collect())
            .collect();
        out.push((c, Glyph { strokes, advance: (adv * k) as f32 }));
    }
    Ok(out)
}

/// Path data → polylines (absolute and relative M L H V C S Q T Z; an
/// arc is drawn as a straight line to its end). A closed subpath repeats
/// its first point.
pub fn path(d: &str, tolerance: f64) -> Result<Vec<Vec<[f64; 2]>>> {
    let mut toks = Tokens { s: d.as_bytes(), i: 0 };
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    let (mut p, mut start) = ([0.0, 0.0], [0.0, 0.0]);
    // Last control point, for S and T.
    let mut last_ctrl: Option<(u8, [f64; 2])> = None;
    let mut cmd = 0u8;
    let flush = |cur: &mut Vec<[f64; 2]>, out: &mut Vec<Vec<[f64; 2]>>| {
        if cur.len() >= 2 {
            out.push(std::mem::take(cur));
        }
        cur.clear();
    };
    loop {
        match toks.command() {
            Some(c) => cmd = c,
            None if toks.done() => break,
            // Repeated parameters: same command again (M becomes L).
            None if cmd == b'M' => cmd = b'L',
            None if cmd == b'm' => cmd = b'l',
            None if cmd == 0 || cmd.eq_ignore_ascii_case(&b'z') => bail!("tracé SVG invalide : {d:?}"),
            None => {}
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { p } else { [0.0, 0.0] };
        let pt = |t: &mut Tokens| -> Result<[f64; 2]> { Ok([base[0] + t.num()?, base[1] + t.num()?]) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                flush(&mut cur, &mut out);
                p = pt(&mut toks)?;
                start = p;
                cur.push(p);
                last_ctrl = None;
            }
            b'L' => {
                p = pt(&mut toks)?;
                cur.push(p);
                last_ctrl = None;
            }
            b'H' => {
                p = [base[0] + toks.num()?, p[1]];
                cur.push(p);
                last_ctrl = None;
            }
            b'V' => {
                p = [p[0], (if rel { p[1] } else { 0.0 }) + toks.num()?];
                cur.push(p);
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    pt(&mut toks)?
                } else {
                    match last_ctrl {
                        Some((b'C', c)) => [2.0 * p[0] - c[0], 2.0 * p[1] - c[1]],
                        _ => p,
                    }
                };
                let c2 = pt(&mut toks)?;
                let end = pt(&mut toks)?;
                if cur.is_empty() {
                    cur.push(p);
                }
                cubic(&mut cur, p, c1, c2, end, tolerance);
                last_ctrl = Some((b'C', c2));
                p = end;
            }
            b'Q' | b'T' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'Q') {
                    pt(&mut toks)?
                } else {
                    match last_ctrl {
                        Some((b'Q', c)) => [2.0 * p[0] - c[0], 2.0 * p[1] - c[1]],
                        _ => p,
                    }
                };
                let end = pt(&mut toks)?;
                if cur.is_empty() {
                    cur.push(p);
                }
                // A quadratic is the cubic with these two controls.
                let a = [p[0] + 2.0 / 3.0 * (c1[0] - p[0]), p[1] + 2.0 / 3.0 * (c1[1] - p[1])];
                let b = [end[0] + 2.0 / 3.0 * (c1[0] - end[0]), end[1] + 2.0 / 3.0 * (c1[1] - end[1])];
                cubic(&mut cur, p, a, b, end, tolerance);
                last_ctrl = Some((b'Q', c1));
                p = end;
            }
            b'A' => {
                for _ in 0..5 {
                    toks.num()?;
                }
                p = pt(&mut toks)?;
                cur.push(p);
                last_ctrl = None;
            }
            b'Z' => {
                if !cur.is_empty() {
                    cur.push(start);
                }
                flush(&mut cur, &mut out);
                p = start;
                last_ctrl = None;
            }
            _ => bail!("commande SVG inconnue : {}", cmd as char),
        }
    }
    flush(&mut cur, &mut out);
    Ok(out)
}

/// Append a cubic's points after `p0`, cut into as many straight steps as
/// its control polygon needs for `tolerance`.
fn cubic(out: &mut Vec<[f64; 2]>, p0: [f64; 2], c1: [f64; 2], c2: [f64; 2], p3: [f64; 2], tolerance: f64) {
    // Deviation bound of a cubic from its chord: 3/4 of the largest second difference.
    let dd = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| ((a[0] - 2.0 * b[0] + c[0]).powi(2) + (a[1] - 2.0 * b[1] + c[1]).powi(2)).sqrt();
    let m = dd(p0, c1, c2).max(dd(c1, c2, p3)) * 0.75;
    let n = if tolerance > 0.0 { ((m / tolerance).sqrt().ceil() as usize).clamp(1, MAX_CURVE_STEPS) } else { MAX_CURVE_STEPS };
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push([a * p0[0] + b * c1[0] + c * c2[0] + e * p3[0], a * p0[1] + b * c1[1] + c * c2[1] + e * p3[1]]);
    }
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn done(&mut self) -> bool {
        self.skip();
        self.i >= self.s.len()
    }

    /// The next command letter, if the next token is one.
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        (c.is_ascii_alphabetic() && c != b'e' && c != b'E').then(|| {
            self.i += 1;
            c
        })
    }

    fn num(&mut self) -> Result<f64> {
        self.skip();
        let from = self.i;
        let s = self.s;
        let mut j = self.i;
        if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
            j += 1;
        }
        let mut dot = false;
        while j < s.len() && (s[j].is_ascii_digit() || (s[j] == b'.' && !dot)) {
            dot |= s[j] == b'.';
            j += 1;
        }
        if j < s.len() && (s[j] == b'e' || s[j] == b'E') {
            j += 1;
            if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
                j += 1;
            }
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
        }
        let v: f64 = std::str::from_utf8(&s[from..j])?.parse().map_err(|_| anyhow!("nombre attendu dans le tracé SVG"))?;
        if !v.is_finite() {
            bail!("nombre invalide dans le tracé SVG");
        }
        self.i = j;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A hand-written SVG font (not the shipped file).
    const FONT: &str = r#"<?xml version="1.0" standalone="no"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" >
<svg xmlns="http://www.w3.org/2000/svg"><defs>
<font id="T" horiz-adv-x="300">
  <font-face units-per-em="1000" cap-height="500"/>
  <glyph glyph-name="L" unicode="L" horiz-adv-x="400" d="M100 500v-500h250"/>
  <glyph glyph-name="o" unicode="o" d="M0 0c0 100 200 100 200 0s-200 -100 -200 0z"/>
  <glyph glyph-name="space" unicode=" "/>
  <glyph glyph-name="fi" unicode="fi" d="M0 0h10"/>
  <glyph glyph-name="eacute" unicode="&#xe9;" d="M0 0h100M50 300l50 50"/>
</font></defs></svg>"#;

    #[test]
    fn parses_a_handwritten_font() {
        let font = parse(FONT).unwrap();
        let get = |c| &font.iter().find(|(k, _)| *k == c).unwrap().1;
        assert!(!font.iter().any(|(c, _)| *c == 'f'), "ligatures are skipped");
        // L: one stroke, cap height 500 → 2 units.
        let l = get('L');
        assert_eq!(l.strokes, vec![vec![[0.4, 2.0], [0.4, 0.0], [1.4, 0.0]]]);
        assert!((l.advance - 1.6).abs() < 1e-6);
        // Default advance from <font>; a closed curve repeats its first point.
        let o = get('o');
        assert!((o.advance - 1.2).abs() < 1e-6);
        assert_eq!(o.strokes.len(), 1);
        assert!(o.strokes[0].len() > 4);
        assert_eq!(o.strokes[0].first(), o.strokes[0].last());
        assert!(get(' ').strokes.is_empty());
        assert_eq!(get('é').strokes.len(), 2);
    }

    #[test]
    fn path_commands_absolute_and_relative() {
        let p = path("M10 10L20 10H30V0l5 5h-5v5Z", 1.0).unwrap();
        assert_eq!(p, vec![vec![[10.0, 10.0], [20.0, 10.0], [30.0, 10.0], [30.0, 0.0], [35.0, 5.0], [30.0, 5.0], [30.0, 10.0], [10.0, 10.0]]]);
        // Implicit repeats: extra pairs after M are line-tos.
        let p = path("m0 0 10 0 0 10", 1.0).unwrap();
        assert_eq!(p, vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]]);
        // Quadratic and its reflection end where asked.
        let p = path("M0 0Q50 50 100 0T200 0", 0.5).unwrap();
        assert_eq!(*p[0].last().unwrap(), [200.0, 0.0]);
        assert!(p[0].iter().any(|q| q[1] > 10.0) && p[0].iter().any(|q| q[1] < -10.0));
    }

    #[test]
    fn bad_paths_are_errors_not_panics() {
        for d in ["L10", "M1 2 3", "M0 0X1 1", "Mabc", "M1e999 0"] {
            assert!(path(d, 1.0).is_err(), "{d}");
        }
        assert!(parse("<svg/>").is_err());
    }
}
