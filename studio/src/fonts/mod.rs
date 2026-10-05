//! Laser text (T-439): single-stroke fonts and text layout.
//!
//! - `segments`: our 7-segment-style font, the default and the fallback
//!   (capitals only, no accents). `text_to_points` / `text_strokes` keep
//!   drawing it exactly as before.
//! - `hershey`: Dr. A. V. Hershey's Roman simplex and duplex (`.jhf`).
//! - `svgfont`: Relief SingleLine (SVG font, SIL OFL 1.1).
//!
//! The third-party files are embedded from `studio/assets/fonts/` (their
//! licences next to them, listed in `SOURCES.txt`) and turned into strokes
//! the first time a font is used. Accented letters a font lacks are
//! composed by our own code: the base letter plus a drawn accent.
//!
//! Units: every font is normalised to a cap height of 2 with the baseline
//! at 0, so `scale` means the same size whatever the font.

pub mod hershey;
pub mod segments;
pub mod svgfont;

pub use segments::{text_strokes, text_to_points};

use crate::figures::RateUnit;
use crate::patterns::Point;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Longest text drawn (and accepted by the figure editor's text tool).
pub const MAX_TEXT_CHARS: usize = 128;
/// Letter spacing range, in letter widths.
pub const SPACING: std::ops::RangeInclusive<f32> = -0.5..=2.0;
/// « Ajuster à la largeur » range, in frame units (the frame is 2 wide).
pub const FIT_WIDTH: std::ops::RangeInclusive<f32> = 0.1..=2.0;
/// Scroll speed range, in frame widths per beat or per second.
pub const SCROLL_SPEED: std::ops::RangeInclusive<f32> = -4.0..=4.0;

/// One font of the list (`GET /api/fonts`).
#[derive(Clone, Debug, Serialize)]
pub struct FontInfo {
    pub id: &'static str,
    pub name_fr: &'static str,
    /// Draws lowercase letters (otherwise the text is set in capitals).
    pub lowercase: bool,
    /// Draws the French accented letters (é è ê ë à â ç î ï ô ù û ü ÿ œ
    /// and capitals), natively or composed.
    pub accents: bool,
}

const FONTS: [FontInfo; 4] = [
    FontInfo { id: "segments", name_fr: "Segments", lowercase: false, accents: false },
    FontInfo { id: "hershey-simple", name_fr: "Hershey simple", lowercase: true, accents: true },
    FontInfo { id: "hershey-double", name_fr: "Hershey double", lowercase: true, accents: true },
    FontInfo { id: "relief", name_fr: "Relief", lowercase: true, accents: true },
];

/// Every font, the default (« Segments ») first.
pub fn fonts() -> &'static [FontInfo] {
    &FONTS
}

/// The font `id` names; an unknown one is « Segments ».
pub fn info(id: &str) -> &'static FontInfo {
    FONTS.iter().find(|f| f.id == id).unwrap_or(&FONTS[0])
}

/// How a text is set (part of `Content::Text`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    /// A `fonts()` id; unknown = « segments ».
    pub font: String,
    /// Where the line sits when it doesn't scroll.
    pub align: Align,
    /// Extra space between letters, in letter widths (`SPACING`).
    pub spacing: f32,
    /// Scale the text so its width is this (`FIT_WIDTH`), whatever its
    /// length; `None` = the look's size.
    pub fit_width: Option<f32>,
    pub scroll: Scroll,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { font: "segments".into(), align: Align::Center, spacing: 0.0, fit_width: None, scroll: Scroll::default() }
    }
}

impl TextStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Drawn exactly as before T-439: the segment font, centred, no
    /// option. Saved scenes give the same points as they always did.
    fn is_legacy(&self) -> bool {
        info(&self.font).id == "segments" && self.align == Align::Center && self.spacing == 0.0 && self.fit_width.is_none() && !self.scroll.enabled
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// Horizontal scrolling, locked to the tempo clock (or to seconds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scroll {
    pub enabled: bool,
    /// Frame widths per `per`; positive = towards the left, negative =
    /// towards the right (`SCROLL_SPEED`).
    pub speed: f32,
    pub per: RateUnit,
}

impl Default for Scroll {
    fn default() -> Self {
        Self { enabled: false, speed: 0.25, per: RateUnit::Beat }
    }
}

/// One glyph in font units: cap height 2, baseline 0, x from its left side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glyph {
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub advance: f32,
}

/// A loaded font.
struct Font {
    glyphs: HashMap<char, Glyph>,
    /// Set in capitals (the segment font).
    uppercase_only: bool,
    /// Compose accented letters the font lacks.
    compose: bool,
    /// Vertical offset of the baseline, in units: the segment font keeps
    /// its historical placement, the others centre their capitals.
    base_off: f32,
    /// Advance of a character the font doesn't draw.
    missing: f32,
    /// Top of the lowercase letters (to drop the dot of i and j).
    x_height: f32,
    /// One « letter width », the unit of `TextStyle::spacing`.
    letter_w: f32,
}

impl Font {
    fn from_glyphs(glyphs: Vec<(char, Glyph)>, uppercase_only: bool, compose: bool, base_off: f32, missing: Option<f32>) -> Font {
        let glyphs: HashMap<char, Glyph> = glyphs.into_iter().collect();
        let adv = |c| glyphs.get(&c).map(|g: &Glyph| g.advance);
        let x_height = glyphs.get(&'x').and_then(|g| bounds(&g.strokes)).map_or(1.4, |b| b[3]);
        let missing = missing.or_else(|| adv(' ')).unwrap_or(0.8);
        let letter_w = adv('N').unwrap_or(1.0);
        Font { glyphs, uppercase_only, compose, base_off, missing, x_height, letter_w }
    }
}

const SEG_W: f32 = 0.7;
const SEG_ADVANCE: f32 = SEG_W * 1.3;

fn segments_font() -> Font {
    let glyphs = (' '..='~')
        .map(|c| {
            let strokes = segments::glyph(c).into_iter().filter(|s| s.len() >= 2).map(|s| s.iter().map(|&(x, y)| [x * SEG_W, y]).collect()).collect();
            (c, Glyph { strokes, advance: SEG_ADVANCE })
        })
        .collect();
    Font::from_glyphs(glyphs, true, false, -0.5, Some(SEG_ADVANCE))
}

const ROWMANS: &str = include_str!("../../assets/fonts/hershey/rowmans.jhf");
const ROWMAND: &str = include_str!("../../assets/fonts/hershey/rowmand.jhf");
const RELIEF: &str = include_str!("../../assets/fonts/relief/ReliefSingleLineSVG-Regular.svg");

fn load(id: &str) -> anyhow::Result<Font> {
    Ok(match id {
        "hershey-simple" => Font::from_glyphs(hershey::ascii_font(ROWMANS)?, false, true, -1.0, None),
        "hershey-double" => Font::from_glyphs(hershey::ascii_font(ROWMAND)?, false, true, -1.0, None),
        "relief" => Font::from_glyphs(svgfont::parse(RELIEF)?, false, true, -1.0, None),
        _ => segments_font(),
    })
}

/// The font `id` names (unknown = segments), loaded once.
fn font(id: &str) -> &'static Font {
    static CELLS: [OnceLock<Font>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let i = FONTS.iter().position(|f| f.id == id).unwrap_or(0);
    CELLS[i].get_or_init(|| {
        load(FONTS[i].id).unwrap_or_else(|e| {
            log::warn!("police {} illisible ({e:#}) : police à segments", FONTS[i].id);
            segments_font()
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Accent {
    Acute,
    Grave,
    Circumflex,
    Diaeresis,
    Cedilla,
}

/// What an accented letter is made of: a base letter and an accent, or
/// two letters (œ, æ).
enum Parts {
    Accented(char, Accent),
    Ligature(char, char),
}

fn decompose(c: char) -> Option<Parts> {
    use Accent::*;
    let (base, accent) = match c {
        'á' => ('a', Acute),
        'à' => ('a', Grave),
        'â' => ('a', Circumflex),
        'ä' => ('a', Diaeresis),
        'é' => ('e', Acute),
        'è' => ('e', Grave),
        'ê' => ('e', Circumflex),
        'ë' => ('e', Diaeresis),
        'í' => ('i', Acute),
        'ì' => ('i', Grave),
        'î' => ('i', Circumflex),
        'ï' => ('i', Diaeresis),
        'ó' => ('o', Acute),
        'ò' => ('o', Grave),
        'ô' => ('o', Circumflex),
        'ö' => ('o', Diaeresis),
        'ú' => ('u', Acute),
        'ù' => ('u', Grave),
        'û' => ('u', Circumflex),
        'ü' => ('u', Diaeresis),
        'ÿ' => ('y', Diaeresis),
        'ç' => ('c', Cedilla),
        'Á' => ('A', Acute),
        'À' => ('A', Grave),
        'Â' => ('A', Circumflex),
        'Ä' => ('A', Diaeresis),
        'É' => ('E', Acute),
        'È' => ('E', Grave),
        'Ê' => ('E', Circumflex),
        'Ë' => ('E', Diaeresis),
        'Í' => ('I', Acute),
        'Ì' => ('I', Grave),
        'Î' => ('I', Circumflex),
        'Ï' => ('I', Diaeresis),
        'Ó' => ('O', Acute),
        'Ò' => ('O', Grave),
        'Ô' => ('O', Circumflex),
        'Ö' => ('O', Diaeresis),
        'Ú' => ('U', Acute),
        'Ù' => ('U', Grave),
        'Û' => ('U', Circumflex),
        'Ü' => ('U', Diaeresis),
        'Ÿ' => ('Y', Diaeresis),
        'Ç' => ('C', Cedilla),
        'œ' => return Some(Parts::Ligature('o', 'e')),
        'Œ' => return Some(Parts::Ligature('O', 'E')),
        'æ' => return Some(Parts::Ligature('a', 'e')),
        'Æ' => return Some(Parts::Ligature('A', 'E')),
        _ => return None,
    };
    Some(Parts::Accented(base, accent))
}

/// Bounding box `[min x, max x, min y, max y]` of some strokes.
fn bounds(strokes: &[Vec<[f32; 2]>]) -> Option<[f32; 4]> {
    let mut it = strokes.iter().flatten();
    let first = it.next()?;
    Some(it.fold([first[0], first[0], first[1], first[1]], |b, p| [b[0].min(p[0]), b[1].max(p[0]), b[2].min(p[1]), b[3].max(p[1])]))
}

/// The glyph drawn for `c` in `f`: its own, or one we compose.
fn glyph(f: &Font, c: char) -> Option<std::borrow::Cow<'_, Glyph>> {
    use std::borrow::Cow;
    if let Some(g) = f.glyphs.get(&c) {
        return Some(Cow::Borrowed(g));
    }
    if !f.compose {
        return None;
    }
    match decompose(c)? {
        Parts::Ligature(a, b) => {
            let (ga, gb) = (f.glyphs.get(&a)?, f.glyphs.get(&b)?);
            let mut strokes = ga.strokes.clone();
            strokes.extend(gb.strokes.iter().map(|s| s.iter().map(|p| [p[0] + ga.advance, p[1]]).collect()));
            Some(Cow::Owned(Glyph { strokes, advance: ga.advance + gb.advance }))
        }
        Parts::Accented(base, accent) => {
            let g = f.glyphs.get(&base)?;
            let mut strokes = g.strokes.clone();
            if base == 'i' {
                // Dotless i: drop the dot above the x-height.
                strokes.retain(|s| s.iter().any(|p| p[1] <= f.x_height + 0.05));
            }
            let b = bounds(&strokes)?;
            strokes.extend(accent_strokes(accent, (b[0] + b[1]) / 2.0, b[3]));
            Some(Cow::Owned(Glyph { strokes, advance: g.advance }))
        }
    }
}

/// An accent centred on `cx`, above `top` (or under the baseline for the
/// cedilla). Our own shapes, in font units.
fn accent_strokes(accent: Accent, cx: f32, top: f32) -> Vec<Vec<[f32; 2]>> {
    let y = top + 0.25;
    match accent {
        Accent::Acute => vec![vec![[cx - 0.12, y], [cx + 0.18, y + 0.35]]],
        Accent::Grave => vec![vec![[cx + 0.12, y], [cx - 0.18, y + 0.35]]],
        Accent::Circumflex => vec![vec![[cx - 0.25, y], [cx, y + 0.3], [cx + 0.25, y]]],
        Accent::Diaeresis => vec![vec![[cx - 0.2, y], [cx - 0.2, y + 0.12]], vec![[cx + 0.2, y], [cx + 0.2, y + 0.12]]],
        Accent::Cedilla => vec![vec![[cx, 0.0], [cx, -0.18], [cx + 0.15, -0.28], [cx, -0.42], [cx - 0.12, -0.38]]],
    }
}

/// A line of text set in a font, before placement: strokes in font units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaped {
    strokes: Vec<Vec<[f32; 2]>>,
    /// `[min x, max x, min y, max y]` of the strokes; `None` = nothing lit.
    ink: Option<[f32; 4]>,
    base_off: f32,
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        d
    }
}

/// Set `text` (at most `MAX_TEXT_CHARS` characters) in `font_id`, with
/// `spacing` letter widths between letters.
pub fn shape(text: &str, font_id: &str, spacing: f32) -> Shaped {
    let f = font(font_id);
    let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
    let text = if f.uppercase_only { text.to_uppercase() } else { text };
    let extra = finite_or(spacing, 0.0).clamp(*SPACING.start(), *SPACING.end()) * f.letter_w;
    let mut strokes = Vec::new();
    let mut x = 0.0;
    for c in text.chars() {
        let advance = match glyph(f, c) {
            Some(g) => {
                strokes.extend(g.strokes.iter().filter(|s| s.len() >= 2).map(|s| s.iter().map(|p| [p[0] + x, p[1]]).collect::<Vec<_>>()));
                g.advance
            }
            None => f.missing,
        };
        x += advance + extra;
    }
    Shaped { ink: bounds(&strokes), strokes, base_off: f.base_off }
}

/// Place a shaped line in the frame: size `scale` (or fitted), aligned or
/// scrolled to `scroll_pos` (beats or seconds, as `style.scroll.per`),
/// and cut at the frame's edges: everything returned is within -1..1.
pub fn place(shaped: &Shaped, style: &TextStyle, scale: f32, scroll_pos: f64) -> Vec<Vec<[f32; 2]>> {
    let Some([x0, x1, y0, y1]) = shaped.ink else { return Vec::new() };
    let off = shaped.base_off;
    let mut s = finite_or(scale, 0.0).max(0.0);
    if let Some(w) = style.fit_width {
        let w = finite_or(w, 1.0).clamp(*FIT_WIDTH.start(), *FIT_WIDTH.end());
        if x1 - x0 > 1e-6 {
            s = w / (x1 - x0);
        }
        // Never taller than the frame.
        let tall = (y0 + off).abs().max((y1 + off).abs());
        if tall * s > 1.0 {
            s = 1.0 / tall;
        }
    }
    let width = (x1 - x0) * s;
    let left = if style.scroll.enabled {
        // Starts centred; leaves on one side, comes back on the other.
        let speed = finite_or(style.scroll.speed, 0.0).clamp(*SCROLL_SPEED.start(), *SCROLL_SPEED.end()) as f64;
        let pos = if scroll_pos.is_finite() { scroll_pos } else { 0.0 };
        let travel = speed * pos * 2.0;
        let (w, cycle) = (width as f64, width as f64 + 2.0);
        ((-w / 2.0 - travel + 1.0 + w).rem_euclid(cycle) - 1.0 - w) as f32
    } else {
        match style.align {
            Align::Left => -1.0,
            Align::Center => -width / 2.0,
            Align::Right => 1.0 - width,
        }
    };
    let tx = left - x0 * s;
    let mut out = Vec::new();
    for stroke in &shaped.strokes {
        let world: Vec<[f32; 2]> = stroke.iter().map(|p| [tx + p[0] * s, (p[1] + off) * s]).collect();
        clip(&world, &mut out);
    }
    out
}

/// The part of a polyline inside the frame (-1..1 on both axes), as one
/// or more polylines: letters leaving the frame are cut, never squashed.
fn clip(stroke: &[[f32; 2]], out: &mut Vec<Vec<[f32; 2]>>) {
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let flush = |cur: &mut Vec<[f32; 2]>, out: &mut Vec<Vec<[f32; 2]>>| {
        if cur.len() >= 2 {
            out.push(std::mem::take(cur));
        }
        cur.clear();
    };
    for w in stroke.windows(2) {
        match clip_segment(w[0], w[1]) {
            Some((p, q, cut_start, cut_end)) => {
                if cut_start || cur.is_empty() {
                    flush(&mut cur, out);
                    cur.push(p);
                }
                cur.push(q);
                if cut_end {
                    flush(&mut cur, out);
                }
            }
            None => flush(&mut cur, out),
        }
    }
    flush(&mut cur, out);
}

/// Liang-Barsky: the part of a→b inside the frame, and whether each end
/// was cut.
fn clip_segment(a: [f32; 2], b: [f32; 2]) -> Option<([f32; 2], [f32; 2], bool, bool)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, a[0] + 1.0), (dx, 1.0 - a[0]), (-dy, a[1] + 1.0), (dy, 1.0 - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    let at = |t: f32| [(a[0] + dx * t).clamp(-1.0, 1.0), (a[1] + dy * t).clamp(-1.0, 1.0)];
    Some((at(t0), at(t1), t0 > 0.0, t1 < 1.0))
}

/// `text` laid out with `style` at size `scale`, as strokes in -1..1
/// (not scrolled): what the figure editor's text tool inserts.
pub fn layout(text: &str, style: &TextStyle, scale: f32) -> Vec<Vec<[f32; 2]>> {
    place(&shape(text, &style.font, style.spacing), style, scale, 0.0)
}

/// Strokes as laser points: the first point dark, then each stroke lit,
/// with a blanked jump between strokes (as `text_to_points` does).
pub fn strokes_to_points(strokes: &[Vec<[f32; 2]>], r: f32, g: f32, b: f32) -> Vec<Point> {
    let mut points = Vec::new();
    let mut last: Option<[f32; 2]> = None;
    for s in strokes.iter().filter(|s| s.len() >= 2) {
        match last {
            Some(from) => {
                for i in 0..4 {
                    let t = i as f32 / 3.0;
                    points.push(Point::blanked(from[0] + (s[0][0] - from[0]) * t, from[1] + (s[0][1] - from[1]) * t));
                }
            }
            None => points.push(Point::blanked(s[0][0], s[0][1])),
        }
        points.extend(s.iter().map(|p| Point::lit(p[0], p[1], r, g, b)));
        last = s.last().copied();
    }
    points
}

/// The shaped line of the text on show, kept until the text, the font or
/// the spacing change (a text set by the API is laid out once, then only
/// placed on every frame).
#[derive(Default)]
pub struct TextCache {
    key: Option<(String, String, u32)>,
    shaped: Shaped,
}

impl TextCache {
    pub fn shaped(&mut self, text: &str, style: &TextStyle) -> &Shaped {
        let key = (text.to_string(), info(&style.font).id.to_string(), style.spacing.to_bits());
        if self.key.as_ref() != Some(&key) {
            self.shaped = shape(text, &style.font, style.spacing);
            self.key = Some(key);
        }
        &self.shaped
    }

    /// One frame of `text`: the look's points at size `scale`, scrolled
    /// to `scroll_pos`. The default style draws exactly what the studio
    /// drew before T-439.
    pub fn points(&mut self, text: &str, style: &TextStyle, scale: f32, scroll_pos: f64, (r, g, b): (f32, f32, f32)) -> Vec<Point> {
        if style.is_legacy() {
            return text_to_points(&text.to_uppercase(), scale, r, g, b);
        }
        let strokes = place(self.shaped(text, style), style, scale, scroll_pos);
        strokes_to_points(&strokes, r, g, b)
    }
}

/// Whether `text` has accented letters `font_id` can't draw (what the
/// panel warns about, from the `accents` flag of `fonts()`).
#[cfg(test)]
fn missing_accents(text: &str, font_id: &str) -> bool {
    let f = font(font_id);
    text.chars().filter(|c| decompose(*c).is_some()).any(|c| {
        let c = if f.uppercase_only { c.to_uppercase().next().unwrap_or(c) } else { c };
        glyph(f, c).is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 4] = ["segments", "hershey-simple", "hershey-double", "relief"];

    fn style(font: &str) -> TextStyle {
        TextStyle { font: font.into(), ..TextStyle::default() }
    }

    fn lit_xs(strokes: &[Vec<[f32; 2]>]) -> (f32, f32) {
        let b = bounds(strokes).unwrap();
        (b[0], b[1])
    }

    #[test]
    fn the_list_matches_the_loaders() {
        assert_eq!(fonts().iter().map(|f| f.id).collect::<Vec<_>>(), ALL);
        assert_eq!(info("nope").id, "segments");
        for id in ALL {
            // Each embedded font loads (no fallback to segments).
            let f = font(id);
            assert_eq!(f.uppercase_only, id == "segments", "{id}");
            assert!(f.glyphs.len() >= 90, "{id}: {}", f.glyphs.len());
        }
    }

    #[test]
    fn every_font_has_capitals_and_digits() {
        for id in ALL {
            let f = font(id);
            for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
                let g = glyph(f, c).unwrap_or_else(|| panic!("{id}: no {c}"));
                assert!(!g.strokes.is_empty() && g.advance > 0.0, "{id}: {c}");
            }
            if info(id).lowercase {
                for c in "abcdefghijklmnopqrstuvwxyz".chars() {
                    assert!(glyph(f, c).is_some_and(|g| !g.strokes.is_empty()), "{id}: no {c}");
                }
            }
        }
    }

    #[test]
    fn accented_letters_are_drawn_where_announced() {
        let accented = "ÉÈÀÇ éèàç êëâîïôùûüÿœŒÊËÂÎÏÔÙÛÜŸ";
        for f in fonts().iter().filter(|f| f.accents) {
            let font = font(f.id);
            for c in accented.chars().filter(|c| *c != ' ') {
                let g = glyph(font, c).unwrap_or_else(|| panic!("{}: no {c}", f.id));
                let base = match decompose(c).unwrap() {
                    Parts::Accented(b, _) | Parts::Ligature(b, _) => b,
                };
                // Not the plain letter: an accent above or below, or two letters.
                let plain = glyph(font, base).unwrap();
                let b = bounds(&g.strokes).unwrap();
                let top = if base.is_lowercase() { font.x_height } else { 2.0 };
                assert!(b[3] > top + 0.2 || b[2] < -0.1 || g.advance > plain.advance * 1.2, "{}: {c} looks like {base}", f.id);
            }
            assert!(!missing_accents(accented, f.id), "{}", f.id);
            assert!(!layout("ÉÈÀÇ éèàç", &style(f.id), 0.2).is_empty());
        }
        assert!(missing_accents("Été", "segments"));
        assert!(!missing_accents("ETE", "segments"));
    }

    #[test]
    fn composed_accents_sit_above_the_letter() {
        let f = font("hershey-simple");
        let e = bounds(&glyph(f, 'e').unwrap().strokes).unwrap();
        let ee = bounds(&glyph(f, 'é').unwrap().strokes).unwrap();
        assert!(ee[3] > e[3] + 0.2 && ee[2] == e[2]);
        let c = bounds(&glyph(f, 'ç').unwrap().strokes).unwrap();
        assert!(c[2] < -0.3, "cedilla under the baseline");
        // The dotless i keeps the stem only, under the accent.
        let i = glyph(f, 'î').unwrap();
        assert_eq!(i.strokes.len(), glyph(f, 'ı').map_or(1, |g| g.strokes.len()) + 1);
    }

    #[test]
    fn segment_default_style_is_the_old_drawing() {
        // Non-regression (T-439): same points as `text_to_points`.
        let mut cache = TextCache::default();
        for text in ["HELLO", "laser 42", "", "ÉTÉ!"] {
            let old = text_to_points(&text.to_uppercase(), 0.25, 1.0, 0.5, 0.0);
            assert_eq!(cache.points(text, &TextStyle::default(), 0.25, 3.0, (1.0, 0.5, 0.0)), old, "{text}");
        }
        // An unknown font is the segment font.
        assert_eq!(cache.points("HI", &style("comic"), 0.25, 0.0, (1.0, 1.0, 1.0)), text_to_points("HI", 0.25, 1.0, 1.0, 1.0));
        // The new layout of the segment font draws the same strokes as the
        // old one, only placed by its ink.
        let old = text_strokes("HI", 0.3);
        let new = layout("HI", &TextStyle { align: Align::Left, ..TextStyle::default() }, 0.3);
        assert_eq!(old.len(), new.len());
        let dx = new[0][0][0] - old[0][0][0];
        for (a, b) in old.iter().zip(&new) {
            for (p, q) in a.iter().zip(b) {
                assert!((p[0] + dx - q[0]).abs() < 1e-5 && (p[1] - q[1]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn alignment_puts_the_ink_on_the_edges() {
        for id in ALL {
            let left = layout("Hello", &TextStyle { align: Align::Left, ..style(id) }, 0.2);
            assert!((lit_xs(&left).0 + 1.0).abs() < 1e-4, "{id}");
            // The first lit point of the laser frame is on the left edge.
            let pts = strokes_to_points(&left, 1.0, 1.0, 1.0);
            let min_lit = pts.iter().filter(|p| p.r > 0.0).map(|p| p.x).fold(f32::MAX, f32::min);
            assert!((min_lit + 1.0).abs() < 1e-4, "{id}");
            let right = layout("Hello", &TextStyle { align: Align::Right, ..style(id) }, 0.2);
            assert!((lit_xs(&right).1 - 1.0).abs() < 1e-4, "{id}");
            let centre = layout("Hello", &style(id), 0.2);
            let (a, b) = lit_xs(&centre);
            assert!((a + b).abs() < 1e-4, "{id}");
        }
    }

    #[test]
    fn spacing_widens_the_line() {
        for id in ALL {
            let w = |sp: f32| {
                let (a, b) = lit_xs(&layout("ABC", &TextStyle { spacing: sp, ..style(id) }, 0.1));
                b - a
            };
            assert!(w(1.0) > w(0.0) + 0.05 && w(0.0) > w(-0.3), "{id}");
            // Out of range or NaN: clamped, no panic.
            assert_eq!(w(50.0), w(2.0));
            assert_eq!(w(f32::NAN), w(0.0));
        }
    }

    #[test]
    fn fit_width_holds_whatever_the_length() {
        for id in ALL {
            for text in ["A", "Bonjour", "Joyeux anniversaire Camille et Théo !"] {
                for w in [0.3, 1.0, 1.8] {
                    let st = TextStyle { fit_width: Some(w), ..style(id) };
                    let strokes = layout(text, &st, 0.5);
                    let b = bounds(&strokes).unwrap();
                    assert!(b[1] - b[0] <= w + 1e-4, "{id} {text} {w}: {}", b[1] - b[0]);
                    assert!(b[2] >= -1.0 && b[3] <= 1.0);
                }
            }
            // A long line is fitted to the full width, not overflowing.
            let b = bounds(&layout("Joyeux anniversaire", &TextStyle { fit_width: Some(1.6), ..style(id) }, 0.5)).unwrap();
            assert!((b[1] - b[0] - 1.6).abs() < 1e-3, "{id}");
        }
    }

    #[test]
    fn everything_stays_in_the_frame_with_blanked_moves() {
        for id in ALL {
            for st in [
                style(id),
                TextStyle { align: Align::Right, spacing: 2.0, ..style(id) },
                TextStyle { scroll: Scroll { enabled: true, speed: 0.7, per: RateUnit::Beat }, ..style(id) },
            ] {
                let strokes = place(&shape("Laser Studio Été 2026", id, st.spacing), &st, 0.6, 1.3);
                assert!(!strokes.is_empty(), "{id}");
                for p in strokes.iter().flatten() {
                    assert!(p[0].abs() <= 1.0 && p[1].abs() <= 1.0, "{id}: {p:?}");
                }
                let pts = strokes_to_points(&strokes, 1.0, 1.0, 1.0);
                assert!(pts[0].r == 0.0, "starts dark");
                // A new stroke never starts with a lit line from the last one.
                let blanks = pts.iter().filter(|p| p.r == 0.0).count();
                assert_eq!(blanks, 1 + 4 * (strokes.len() - 1));
            }
        }
    }

    #[test]
    fn scrolling_follows_the_clock_and_cuts_at_the_edges() {
        for id in ALL {
            let st = TextStyle { scroll: Scroll { enabled: true, speed: 0.25, per: RateUnit::Beat }, ..style(id) };
            let sh = shape("BONJOUR", id, 0.0);
            // Same position at the same time.
            assert_eq!(place(&sh, &st, 0.1, 2.5), place(&sh, &st, 0.1, 2.5));
            // Starts centred, then moves left: 0.25 frame width (0.5) per beat.
            let (a0, b0) = lit_xs(&place(&sh, &st, 0.1, 0.0));
            assert!((a0 + b0).abs() < 1e-3, "{id}");
            let (a1, _) = lit_xs(&place(&sh, &st, 0.1, 0.1));
            assert!((a0 - a1 - 0.05).abs() < 1e-3, "{id}: {a0} → {a1}");
            // Negative speed: towards the right.
            let rev = TextStyle { scroll: Scroll { speed: -0.25, ..st.scroll.clone() }, ..st.clone() };
            assert!(lit_xs(&place(&sh, &rev, 0.1, 0.1)).0 > a0);
            // After 2 beats the middle of the line is on the left edge:
            // the left half is cut there, nothing lit beyond.
            let w = b0 - a0;
            let (a, b) = lit_xs(&place(&sh, &st, 0.1, 2.0));
            assert!((-1.0..-0.95).contains(&a) && (b - (-1.0 + w / 2.0)).abs() < 1e-3, "{id}: {a}..{b}");
            // A cycle is (w + 2) of travel: back where it started.
            let cycle = (w + 2.0) / 0.5;
            let (a3, b3) = lit_xs(&place(&sh, &st, 0.1, cycle as f64 * 3.0));
            assert!((a3 - a0).abs() < 1e-3 && (b3 - b0).abs() < 1e-3, "{id}");
            // Just gone on the left = just coming back on the right.
            let gone = (w / 2.0 + 1.0 + 0.01) / 0.5;
            let back = place(&sh, &st, 0.1, gone as f64);
            assert!(back.iter().flatten().all(|p| p[0] >= 0.98 && p[0] <= 1.0), "{id}: {back:?}");
        }
    }

    #[test]
    fn clipping_splits_strokes_at_the_frame() {
        let mut out = Vec::new();
        clip(&[[-2.0, 0.0], [0.0, 0.0], [2.0, 0.0], [2.0, 0.5], [0.0, 0.5]], &mut out);
        assert_eq!(out, vec![vec![[-1.0, 0.0], [0.0, 0.0], [1.0, 0.0]], vec![[1.0, 0.5], [0.0, 0.5]]]);
        let mut out = Vec::new();
        clip(&[[2.0, 2.0], [3.0, 3.0]], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn style_serde_and_defaults() {
        let st: TextStyle = serde_json::from_str("{}").unwrap();
        assert_eq!(st, TextStyle::default());
        assert!(st.is_default() && st.is_legacy());
        let st: TextStyle = serde_json::from_str(r#"{"font":"relief","align":"right","scroll":{"enabled":true,"per":"second"}}"#).unwrap();
        assert_eq!((st.align, st.scroll.speed, st.scroll.per), (Align::Right, 0.25, RateUnit::Second));
        assert!(!st.is_legacy());
    }

    #[test]
    fn the_cache_lays_out_once() {
        let mut cache = TextCache::default();
        let st = style("relief");
        let a = cache.shaped("Été", &st).clone();
        assert_eq!(cache.key.as_ref().unwrap().0, "Été");
        assert_eq!(cache.shaped("Été", &st), &a);
        assert_ne!(cache.shaped("Eté", &st), &a);
        // A placement-only change keeps the cached shape.
        let key = cache.key.clone();
        cache.points("Eté", &TextStyle { align: Align::Left, ..st.clone() }, 0.2, 0.0, (1.0, 1.0, 1.0));
        assert_eq!(cache.key, key);
    }

    #[test]
    fn long_text_is_capped() {
        let long = "A".repeat(500);
        let n = |t: &str| shape(t, "hershey-simple", 0.0).strokes.len();
        assert_eq!(n(&long), n(&"A".repeat(MAX_TEXT_CHARS)));
    }

    #[test]
    fn every_asset_is_listed_with_its_source() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
        let sources = std::fs::read_to_string(dir.join("SOURCES.txt")).unwrap();
        // The private register, when this checkout has it (it is not in git).
        let register = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/CONTENT_SOURCES.md")).ok();
        let mut stack = vec![dir.clone()];
        let mut n = 0;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
                assert!(sources.contains(&rel), "{rel} not in assets/fonts/SOURCES.txt");
                if let Some(reg) = &register {
                    let name = p.file_name().unwrap().to_string_lossy();
                    assert!(reg.contains(&*name), "{name} not in docs/CONTENT_SOURCES.md");
                }
                n += 1;
            }
        }
        assert!(n >= 6);
    }
}
