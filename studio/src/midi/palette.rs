//! The APC40 mkII's RGB pads (T-206): its fixed colour palette, the
//! nearest palette entry to a cue's colour and the dim variant shown for
//! "cue present", and which colour a cell stands for.
//!
//! The palette (velocity → sRGB) and the LED modes (channel 0 = solid,
//! 6–10 = pulsing, 11–15 = blinking) are public technical facts from
//! Akai's "APC40 mkII Communications Protocol" v1.2, pp. 18–21, as
//! summarised in docs/research/midi-apc40.md §2.3–2.4.

use crate::engine::{Content, Settings};
use crate::generators::ColorMode;
use crate::Shared;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// LED mode channels (§2.3): secondary colour pulsing at 1/4, blinking at 1/8.
pub const CH_PULSE_QUARTER: u8 = 9;
pub const CH_BLINK_EIGHTH: u8 = 13;
/// Blinking at 1/4 (a cue waiting for its quantised start, T-159).
pub const CH_BLINK_QUARTER: u8 = 14;

pub const MK2_GREY: u8 = 1;
pub const MK2_WHITE: u8 = 3;
pub const MK2_YELLOW: u8 = 13;
pub const MK2_GREEN: u8 = 21;
pub const MK2_DIM_GREEN: u8 = 22;

/// Velocity → 0xRRGGBB.
const MK2_HEX: [u32; 128] = [
    0x000000, 0x1E1E1E, 0x7F7F7F, 0xFFFFFF, 0xFF4C4C, 0xFF0000, 0x590000, 0x190000, //
    0xFFBD6C, 0xFF5400, 0x591D00, 0x271B00, 0xFFFF4C, 0xFFFF00, 0x595900, 0x191900, //
    0x88FF4C, 0x54FF00, 0x1D5900, 0x142B00, 0x4CFF4C, 0x00FF00, 0x005900, 0x001900, //
    0x4CFF5E, 0x00FF19, 0x00590D, 0x001902, 0x4CFF88, 0x00FF55, 0x00591D, 0x001F12, //
    0x4CFFB7, 0x00FF99, 0x005935, 0x001912, 0x4CC3FF, 0x00A9FF, 0x004152, 0x001019, //
    0x4C88FF, 0x0055FF, 0x001D59, 0x000819, 0x4C4CFF, 0x0000FF, 0x000059, 0x000019, //
    0x874CFF, 0x5400FF, 0x190064, 0x0F0030, 0xFF4CFF, 0xFF00FF, 0x590059, 0x190019, //
    0xFF4C87, 0xFF0054, 0x59001D, 0x220013, 0xFF1500, 0x993500, 0x795100, 0x436400, //
    0x033900, 0x005735, 0x00547F, 0x0000FF, 0x00454F, 0x2500CC, 0x7F7F7F, 0x202020, //
    0xFF0000, 0xBDFF2D, 0xAFED06, 0x64FF09, 0x108B00, 0x00FF87, 0x00A9FF, 0x002AFF, //
    0x3F00FF, 0x7A00FF, 0xB21A7D, 0x402100, 0xFF4A00, 0x88E106, 0x72FF15, 0x00FF00, //
    0x3BFF26, 0x59FF71, 0x38FFCC, 0x5B8AFF, 0x3151C6, 0x877FE9, 0xD31DFF, 0xFF005D, //
    0xFF7F00, 0xB9B000, 0x90FF00, 0x835D07, 0x392B00, 0x144C10, 0x0D5038, 0x15152A, //
    0x16205A, 0x693C1C, 0xA8000A, 0xDE513D, 0xD86A1C, 0xFFE126, 0x9EE12F, 0x67B50F, //
    0x1E1E30, 0xDCFF6B, 0x80FFBD, 0x9A99FF, 0x8E66FF, 0x404040, 0x757575, 0xE0FFFF, //
    0xA00000, 0x350000, 0x1AD000, 0x074200, 0xB9B000, 0x3F3100, 0xB35F00, 0x4B1502,
];

/// The mkII palette: velocity (colour index) → sRGB.
pub const MK2_PALETTE: [[u8; 3]; 128] = {
    let mut out = [[0u8; 3]; 128];
    let mut i = 0;
    while i < 128 {
        let h = MK2_HEX[i];
        out[i] = [(h >> 16) as u8, (h >> 8) as u8, h as u8];
        i += 1;
    }
    out
};

/// Entries this dark read as "off" on the pads: never picked for a colour.
const DARK: u8 = 0x2B;

/// Weighted RGB distance (green counts most, then blue, then red).
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    let d = |i: usize| (i32::from(a[i]) - i32::from(b[i])).pow(2) as u32;
    2 * d(0) + 4 * d(1) + 3 * d(2)
}

/// The closest palette entry to `rgb` among `candidates` (first one on a tie).
fn closest(rgb: [u8; 3], candidates: impl Iterator<Item = usize>) -> u8 {
    candidates.min_by_key(|&i| distance(rgb, MK2_PALETTE[i])).unwrap_or(0) as u8
}

static NEAREST: LazyLock<Mutex<HashMap<[u8; 3], u8>>> = LazyLock::new(Mutex::default);

/// The palette entry closest to `rgb`, dark entries left out (a dark cue
/// still lights its pad). Cached per colour: the LEDs ask 30 times a second.
pub fn nearest_velocity(rgb: [u8; 3]) -> u8 {
    let mut cache = NEAREST.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 4096 {
        cache.clear();
    }
    *cache.entry(rgb).or_insert_with(|| closest(rgb, (1..128).filter(|&i| MK2_PALETTE[i].iter().any(|&c| c > DARK))))
}

/// The dim variant of `v`, for a cue present but not playing. The colour
/// families 4–59 come in fours, bright then dim (red 4, 5, 6, 7): the dim
/// one is `4k + 2`. Elsewhere, the entry closest to `v` at a third of its
/// brightness (or `v` itself if nothing fits).
pub fn dim_velocity(v: u8) -> u8 {
    let v = v & 0x7F;
    match v {
        0 => 0,
        // Already the dim (or darker) one of its family: kept.
        4..=59 if v % 4 >= 2 => v,
        4..=59 => v / 4 * 4 + 2,
        _ => {
            let dim = MK2_PALETTE[v as usize].map(|c| (u16::from(c) * 35 / 100) as u8);
            let d = closest(dim, (1..128).filter(|&i| i != v as usize));
            if distance(dim, MK2_PALETTE[d as usize]) < distance(dim, MK2_PALETTE[v as usize]) {
                d
            } else {
                v
            }
        }
    }
}

/// Which colours the mkII's pads show (`pad_colors` in `devices.json`,
/// per port; « Couleurs des pads » in the Controller tab).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadColors {
    /// The cue's own colour (its look), live while it plays.
    #[default]
    Cue,
    /// The cell's colour on screen: the colour set on the cell (T-430),
    /// else the user cue's band, else its page's category colour.
    Cell,
    /// Yellow = cue present, green = playing, like the original APC40.
    Classic,
}

/// The category colours of the cue grid, as the UI draws them
/// (`PAGE_COLORS` in index.html, T-299), in `presets::CATEGORIES` order.
pub const CATEGORY_COLORS: [[u8; 3]; 9] = [
    [0x22, 0xe3, 0xff],
    [0x8b, 0x5c, 0xf6],
    [0xff, 0x3d, 0xc8],
    [0x3d, 0x9b, 0xff],
    [0x2e, 0xe6, 0xa6],
    [0xff, 0xb0, 0x20],
    [0xff, 0x5d, 0x6e],
    [0xc4, 0xf0, 0x3d],
    [0xff, 0x8a, 0x3d],
];

/// The one colour that stands for a look: its main colour; white for a
/// rainbow (no single hue) or a black main colour.
pub fn look_rgb(settings: &Settings) -> [u8; 3] {
    let rainbow = matches!(&settings.content, Content::Generator { params, .. } if params.color_mode == ColorMode::Rainbow);
    if rainbow || settings.color == [0, 0, 0] {
        [255, 255, 255]
    } else {
        settings.color
    }
}

/// The colour of cue `id` on the pads under `scheme` (`Classic` has none).
/// A playing cue shows its live look (a colour changed from the UI, T-140,
/// shows at the next LED update).
pub fn cue_rgb(s: &Shared, id: &str, scheme: PadColors) -> Option<[u8; 3]> {
    match scheme {
        PadColors::Classic => None,
        PadColors::Cue => {
            let newest = s.deck.active.iter().rposition(|a| a.cue == id);
            let live = match newest {
                // The newest cue's live look is `Shared::settings` (cues.rs).
                Some(i) if i + 1 == s.deck.active.len() => Some(&s.settings),
                Some(i) => Some(&s.deck.active[i].settings),
                None => None,
            };
            let settings = live.or_else(|| s.presets.iter().find(|p| p.id == id).map(|p| &p.settings))?;
            Some(look_rgb(settings))
        }
        PadColors::Cell => {
            if let Some(c) = s.deck.slots.get(id).and_then(|slot| slot.color) {
                return Some(c);
            }
            if let Some(c) = s.user_cues.get(id) {
                return Some(c.color);
            }
            let cat = s.presets.iter().find(|p| p.id == id)?.category;
            let i = crate::presets::CATEGORIES.iter().position(|&c| c == cat).unwrap_or(0);
            Some(CATEGORY_COLORS[i % CATEGORY_COLORS.len()])
        }
    }
}

/// `#rrggbb` of velocity `v`, for the UI preview.
pub fn hex(v: u8) -> String {
    let [r, g, b] = MK2_PALETTE[(v & 0x7F) as usize];
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn palette_has_128_entries_and_starts_black() {
        assert_eq!(MK2_PALETTE.len(), 128);
        assert_eq!(MK2_PALETTE[0], [0, 0, 0]);
        assert_eq!(MK2_PALETTE[MK2_WHITE as usize], [255, 255, 255]);
        assert_eq!(MK2_PALETTE[5], [255, 0, 0]);
        assert_eq!(MK2_PALETTE[127], [0x4B, 0x15, 0x02]);
        assert_eq!(hex(41), "#0055ff");
    }

    #[test]
    fn preset_colours_find_their_pad_colour() {
        assert_eq!(nearest_velocity([0, 255, 0]), 21);
        assert!(matches!(nearest_velocity([255, 0, 0]), 5 | 72));
        assert!((41..=45).contains(&nearest_velocity([0, 80, 255])), "{}", nearest_velocity([0, 80, 255]));
        assert_eq!(nearest_velocity([255, 255, 255]), MK2_WHITE);
        // Cached: same answer the second time.
        assert_eq!(nearest_velocity([0, 255, 0]), 21);
        // Dark colours still light: never an entry that reads as off.
        let v = nearest_velocity([10, 0, 0]);
        assert!(MK2_PALETTE[v as usize].iter().any(|&c| c > DARK), "{v}");
        assert_ne!(nearest_velocity([0, 0, 0]), 0);
    }

    #[test]
    fn dim_variants() {
        assert_eq!(dim_velocity(5), 6, "red → dark red");
        assert_eq!(dim_velocity(21), 22);
        assert_eq!(dim_velocity(41), 42);
        assert_eq!(dim_velocity(4), 6);
        assert_eq!(dim_velocity(0), 0);
        // Outside the families: a darker entry of the same hue.
        let white = dim_velocity(MK2_WHITE);
        assert_ne!(white, MK2_WHITE);
        let [r, g, b] = MK2_PALETTE[white as usize];
        assert!(r == g && g == b && r < 0x80, "grey: {white}");
        let red = MK2_PALETTE[dim_velocity(72) as usize];
        assert!(red[0] > 0 && red[0] < 0x80 && red[1] == 0, "{red:?}");
        for v in 0..128 {
            let d = dim_velocity(v);
            let sum = |i: u8| MK2_PALETTE[i as usize].iter().map(|&c| u32::from(c)).sum::<u32>();
            assert!(d == v || sum(d) < sum(v), "{v} → {d}");
        }
    }

    #[test]
    fn a_look_stands_for_its_main_colour_or_white() {
        let mut s = Settings { color: [0, 80, 255], ..Settings::default() };
        assert_eq!(look_rgb(&s), [0, 80, 255]);
        s.content = Content::Generator { generator: "lissajous".into(), params: crate::generators::GenParams { color_mode: ColorMode::Rainbow, ..Default::default() } };
        assert_eq!(look_rgb(&s), [255, 255, 255]);
        s.content = Content::Generator { generator: "lissajous".into(), params: crate::generators::GenParams { color_mode: ColorMode::Gradient, ..Default::default() } };
        assert_eq!(look_rgb(&s), [0, 80, 255], "gradient: the main colour");
        s.color = [0, 0, 0];
        assert_eq!(look_rgb(&s), [255, 255, 255]);
    }

    #[test]
    fn cue_colour_follows_the_scheme_and_the_live_look() {
        let mut s = test_support::shared();
        let p = s.presets.iter().find(|p| p.category == crate::presets::CATEGORIES[1]).unwrap().clone();
        assert_eq!(cue_rgb(&s, &p.id, PadColors::Cue), Some(look_rgb(&p.settings)));
        assert_eq!(cue_rgb(&s, &p.id, PadColors::Cell), Some(CATEGORY_COLORS[1]));
        assert_eq!(cue_rgb(&s, &p.id, PadColors::Classic), None);
        // A colour set on the cell (T-430) wins.
        s.deck.set_slot(&p.id, crate::cues::CueSlot { color: Some([1, 2, 3]), ..Default::default() });
        assert_eq!(cue_rgb(&s, &p.id, PadColors::Cell), Some([1, 2, 3]));
        s.deck.set_slot(&p.id, crate::cues::CueSlot::default());
        assert_eq!(cue_rgb(&s, "nope", PadColors::Cue), None);
        // Playing: the live look, changed from the UI.
        assert!(crate::controls::press_cue(&mut s, &p.id, None, true));
        s.settings.color = [255, 0, 255];
        assert_eq!(cue_rgb(&s, &p.id, PadColors::Cue), Some([255, 0, 255]));
    }
}
