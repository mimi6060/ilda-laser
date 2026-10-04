//! Festival palettes and perceived colour balance (T-130).
//!
//! Pro shows use few colours, chosen well, and change them on musical
//! phrases. A festival palette is two colours (colour 1, colour 2) that
//! stand in for the colours of every look on show - the manual look, the
//! cues, the timeline - while it is active; the looks' colour modes and
//! geometry stay theirs. A palette picked from the UI lands on the next
//! bar's « one » (or the next 8-bar phrase, or at once).
//!
//! Perceived balance is a *content* choice: green reads much brighter
//! than red or blue in haze, so a two-colour look can scale green down
//! to look even. It is not the output's colour calibration (colorcal.rs,
//! T-414), which corrects one projector's hardware; this one is applied
//! to the drawn content with the master live stage, before the zones,
//! the calibration, the colour calibration and the whole safety stage.
//! Its gains are clamped to 0..1, so it can only ever dim a channel.
//!
//! The palettes are ours: names and colours picked from the colour
//! practice notes (docs/research/festival-looks.md, section 3), no file
//! copied from any other software.

use crate::patterns::Point;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A named pair of colours.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FestivalPalette {
    pub name: &'static str,
    pub c1: [u8; 3],
    pub c2: [u8; 3],
}

const fn pal(name: &'static str, c1: [u8; 3], c2: [u8; 3]) -> FestivalPalette {
    FestivalPalette { name, c1, c2 }
}

const GREEN: [u8; 3] = [0, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const CYAN: [u8; 3] = [0, 255, 255];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 40, 255];
const MAGENTA: [u8; 3] = [255, 0, 200];
const AMBER: [u8; 3] = [255, 150, 0];

/// The built-in festival palettes, in UI order. Append-only: the UI and
/// controls refer to them by index.
pub const PALETTES: [FestivalPalette; 8] = [
    pal("Vert pur", GREEN, GREEN),
    pal("Blanc", WHITE, WHITE),
    pal("Cyan glacier", CYAN, WHITE),
    pal("Rouge hardstyle", RED, WHITE),
    pal("Afterlife", BLUE, MAGENTA),
    pal("Vert/bleu", GREEN, BLUE),
    pal("Coucher de soleil", AMBER, WHITE),
    pal("Pop", MAGENTA, CYAN),
];

/// When a palette change lands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteQuantize {
    /// At once.
    Now,
    /// On the next bar's « one ».
    #[default]
    Bar,
    /// On the « one » of the next 8-bar phrase.
    Phrase,
}

impl PaletteQuantize {
    /// The tempo-clock beat a change asked at `beat` lands on.
    pub fn boundary(self, beat: f64, beats_per_bar: u8) -> f64 {
        let bar = beats_per_bar.max(1) as f64;
        let grid = match self {
            PaletteQuantize::Now => return beat,
            PaletteQuantize::Bar => bar,
            PaletteQuantize::Phrase => 8.0 * bar,
        };
        crate::tempo::quantize(beat, grid, crate::cues::CATCH_UP_BEATS)
    }
}

/// The « Équilibre perçu » preset: green down, red and blue as they are.
pub const PERCEIVED_PRESET: [f32; 3] = [1.0, 0.6, 1.0];

/// Per-channel gains on the drawn content (R, G, B), 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Balance {
    pub enabled: bool,
    pub gains: [f32; 3],
}

impl Default for Balance {
    fn default() -> Self {
        Self { enabled: false, gains: PERCEIVED_PRESET }
    }
}

impl Balance {
    /// Gains in 0..1 (anything else, NaN included, is pulled into range).
    fn clamped(&self) -> [f32; 3] {
        self.gains.map(|g| if g.is_finite() { g.clamp(0.0, 1.0) } else { 1.0 })
    }

    /// Scale each channel of the lit points. Off, or all gains at 1: the
    /// points are untouched. Never brightens; a blanked point stays dark.
    pub fn apply(&self, points: &mut [Point]) {
        let [r, g, b] = self.clamped();
        if !self.enabled || [r, g, b] == [1.0; 3] {
            return;
        }
        for p in points {
            p.r *= r;
            p.g *= g;
            p.b *= b;
        }
    }
}

/// A palette change waiting for its boundary.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    /// `None` = back to the looks' own colours.
    pub palette: Option<usize>,
    pub at_beat: f64,
}

/// What is saved to `festival.json`, plus the palette on show (memory
/// only: every start shows the looks' own colours).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Festival {
    pub quantize: PaletteQuantize,
    pub balance: Balance,
    #[serde(skip_deserializing)]
    pub active: Option<usize>,
    #[serde(skip_deserializing)]
    pub pending: Option<Pending>,
    /// Where the configuration is saved (`None` in tests).
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// The configuration part, as saved.
#[derive(Serialize)]
struct Saved<'a> {
    quantize: PaletteQuantize,
    balance: &'a Balance,
}

impl Festival {
    /// Loads `festival.json` (missing or damaged: the defaults).
    pub fn load(path: PathBuf) -> Self {
        let mut f: Festival = crate::load_json(&path);
        f.balance.gains = f.balance.clamped();
        f.path = Some(path);
        f
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &Saved { quantize: self.quantize, balance: &self.balance });
        }
    }

    /// New quantisation and balance from the UI (gains pulled into 0..1), saved.
    pub fn configure(&mut self, quantize: PaletteQuantize, balance: Balance) {
        self.quantize = quantize;
        self.balance = Balance { gains: balance.clamped(), ..balance };
        self.save();
    }

    /// Ask for `palette` (`None` = the looks' own colours) at tempo-clock
    /// beat `beat`: applied now or queued for the boundary. A new request
    /// replaces a waiting one.
    pub fn request(&mut self, palette: Option<usize>, beat: f64, beats_per_bar: u8) -> anyhow::Result<()> {
        if let Some(i) = palette {
            anyhow::ensure!(i < PALETTES.len(), "palette inconnue : {i}");
        }
        let at_beat = self.quantize.boundary(beat, beats_per_bar);
        self.pending = Some(Pending { palette, at_beat });
        self.fire_due(beat);
        Ok(())
    }

    /// A waiting change whose boundary is reached lands. Every frame.
    pub fn fire_due(&mut self, beat: f64) {
        if let Some(p) = self.pending.filter(|p| p.at_beat <= beat) {
            self.active = p.palette;
            self.pending = None;
        }
    }

    /// The colours on show, for `Animator::set_palette`.
    pub fn colors(&self) -> Option<([u8; 3], [u8; 3])> {
        self.active.and_then(|i| PALETTES.get(i)).map(|p| (p.c1, p.c2))
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("festival.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_palettes_with_french_names() {
        let names: Vec<&str> = PALETTES.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["Vert pur", "Blanc", "Cyan glacier", "Rouge hardstyle", "Afterlife", "Vert/bleu", "Coucher de soleil", "Pop"]
        );
        assert_eq!(PALETTES[3].c1, RED);
        assert_eq!(PALETTES[3].c2, WHITE);
    }

    #[test]
    fn bar_quantize_lands_on_the_next_one() {
        let mut f = Festival::default();
        f.request(Some(2), 5.5, 4).unwrap();
        assert_eq!(f.active, None);
        assert_eq!(f.pending.unwrap().at_beat, 8.0);
        f.fire_due(7.99);
        assert_eq!(f.active, None, "not before the one");
        f.fire_due(8.0);
        assert_eq!(f.active, Some(2));
        assert_eq!(f.pending, None);
        assert_eq!(f.colors(), Some((CYAN, WHITE)));
    }

    #[test]
    fn phrase_quantize_waits_for_eight_bars() {
        let mut f = Festival { quantize: PaletteQuantize::Phrase, ..Default::default() };
        f.request(Some(0), 33.0, 4).unwrap();
        assert_eq!(f.pending.unwrap().at_beat, 64.0);
        // In 3/4 a phrase is 24 beats.
        f.request(Some(1), 25.0, 3).unwrap();
        assert_eq!(f.pending.unwrap().at_beat, 48.0);
    }

    #[test]
    fn now_and_a_hair_late_apply_at_once() {
        let mut f = Festival { quantize: PaletteQuantize::Now, ..Default::default() };
        f.request(Some(4), 5.3, 4).unwrap();
        assert_eq!(f.active, Some(4));
        let mut f = Festival::default();
        f.request(Some(5), 8.05, 4).unwrap();
        assert_eq!(f.active, Some(5), "just after the one: on it");
        f.request(None, 12.0, 4).unwrap();
        assert_eq!((f.active, f.colors()), (None, None));
    }

    #[test]
    fn unknown_palette_is_refused() {
        let mut f = Festival::default();
        assert!(f.request(Some(PALETTES.len()), 0.0, 4).is_err());
        assert_eq!(f.pending, None);
    }

    #[test]
    fn balance_only_dims() {
        let lit = Point::lit(0.0, 0.0, 1.0, 1.0, 1.0);
        let mut pts = vec![lit, Point::blanked(0.5, 0.5)];
        Balance { enabled: false, ..Default::default() }.apply(&mut pts);
        assert_eq!(pts[0], lit);
        Balance { enabled: true, gains: PERCEIVED_PRESET }.apply(&mut pts);
        assert_eq!((pts[0].r, pts[0].g, pts[0].b), (1.0, 0.6, 1.0));
        assert!(!pts[1].is_lit());
        let mut pts = vec![lit];
        Balance { enabled: true, gains: [3.0, f32::NAN, -1.0] }.apply(&mut pts);
        assert_eq!((pts[0].r, pts[0].g, pts[0].b), (1.0, 1.0, 0.0));
    }

    #[test]
    fn saved_config_round_trips_without_the_palette_on_show() {
        let dir = std::env::temp_dir().join(format!("festival-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Festival::load(path(&dir));
        f.active = Some(3);
        f.configure(PaletteQuantize::Phrase, Balance { enabled: true, gains: [1.0, 0.5, 2.0] });
        let back = Festival::load(path(&dir));
        assert_eq!((back.quantize, back.balance, back.active), (PaletteQuantize::Phrase, Balance { enabled: true, gains: [1.0, 0.5, 1.0] }, None));
        // Missing file: defaults.
        let fresh = Festival::load(dir.join("none.json"));
        assert_eq!((fresh.quantize, fresh.balance), (PaletteQuantize::Bar, Balance::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
