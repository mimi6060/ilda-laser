//! Our published DMX personalities (T-407): what a lighting console patches
//! to drive one projection zone as one fixture.
//!
//! The tables below are the single source of truth: the receiver decodes
//! with them, `markdown()` writes the published chart
//! (`studio/fixtures/dmx-personality.md`, checked by a test) and `gdtf.rs`
//! builds the fixture file a desk imports from them. They cannot drift.
//!
//! The layout is our own design (order, grouping, ranges and names). It is
//! frozen: a change is a new personality, or channels added at the end
//! with a new `VERSION`. Rules every channel follows:
//! - Nothing arms the laser and nothing touches a safety setting, a zone's
//!   geometry or the calibration. There is no « laser on » channel, on
//!   purpose and for good.
//! - *Accès* below 128: the instance obeys nothing but *Fonction › noir*.
//! - 16-bit channels: the coarse (high) byte first, then the fine one.
//! - Centred channels: 128 (32768 in 16 bits) is exactly the neutral
//!   value (size 100 %, centred, no rotation).

use serde::{Deserialize, Serialize};

/// Bumped when channels are added at the end of a table.
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Personality {
    #[default]
    Compact,
    Full,
}

impl Personality {
    pub const ALL: [Personality; 2] = [Personality::Compact, Personality::Full];

    pub fn id(self) -> &'static str {
        match self {
            Personality::Compact => "compact",
            Personality::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == s)
    }

    /// The mode's name in the fixture file and the chart.
    pub fn name_fr(self) -> &'static str {
        match self {
            Personality::Compact => "Compact",
            Personality::Full => "Complet",
        }
    }

    pub fn label_fr(self) -> String {
        format!("{} — {} canaux", self.name_fr(), self.width())
    }

    pub fn channels(self) -> &'static [ChannelDef] {
        match self {
            Personality::Compact => COMPACT,
            Personality::Full => FULL,
        }
    }

    pub fn width(self) -> u16 {
        self.channels().len() as u16
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChanKind {
    /// A level over the whole range (8 bits).
    Level,
    /// The high byte of a 16-bit value.
    Coarse,
    /// The low byte of a 16-bit value (the channel after its coarse one).
    Fine,
    /// A number: page, cue, effect.
    Index,
    /// Ranges, each one an action taken on the way in.
    Function,
    /// ≥ 128: the instance listens.
    Access,
}

impl ChanKind {
    pub fn label_fr(self) -> &'static str {
        match self {
            ChanKind::Level => "niveau",
            ChanKind::Coarse => "16 bits (gros)",
            ChanKind::Fine => "16 bits (fin)",
            ChanKind::Index => "numéro",
            ChanKind::Function => "plages",
            ChanKind::Access => "accès",
        }
    }
}

/// What a channel drives (the decoder's switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    Intensity,
    Size,
    PosX,
    PosY,
    Rotation,
    SizeX,
    SizeY,
    RotSpeed,
    AnimSpeed,
    ColorMode,
    Hue,
    Red,
    Green,
    Blue,
    RgbMix,
    Strobe,
    Page,
    Cue,
    /// Effect slot 0..4: its number, then its amount.
    Effect(u8),
    EffectAmount(u8),
    Function,
    Access,
}

impl Param {
    /// 128 (or 32768) is exactly the middle.
    pub fn centred(self) -> bool {
        matches!(self, Param::Size | Param::PosX | Param::PosY | Param::Rotation | Param::SizeX | Param::SizeY | Param::RotSpeed)
    }
}

/// One range of an index or function channel.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Range {
    pub from: u8,
    pub to: u8,
    pub label_fr: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelDef {
    /// From the start address: 0 is the first channel.
    pub offset: u16,
    pub key: &'static str,
    pub label_fr: &'static str,
    pub kind: ChanKind,
    /// The console's home value: neutral (the image doesn't change).
    pub default: u8,
    /// Geometry: interpolated between two received frames.
    pub smoothed: bool,
    pub param: Param,
    /// What the values do, for the chart.
    pub values_fr: &'static str,
    pub ranges: &'static [Range],
}

/// Fonction: what each range does when the channel enters it.
pub const FUNCTION_RANGES: &[Range] = &[
    Range { from: 0, to: 127, label_fr: "rien" },
    Range { from: 128, to: 191, label_fr: "arrêter les cues" },
    Range { from: 192, to: 255, label_fr: "noir (arrêt d'urgence)" },
];

/// Colour modes, 32 values each (`live::COLOR_MODE_LABELS` order).
pub const COLOR_RANGES: &[Range] = &[
    Range { from: 0, to: 31, label_fr: "couleurs du maître" },
    Range { from: 32, to: 63, label_fr: "fixe" },
    Range { from: 64, to: 95, label_fr: "teinte" },
    Range { from: 96, to: 127, label_fr: "palette" },
    Range { from: 128, to: 159, label_fr: "arc-en-ciel" },
    Range { from: 160, to: 191, label_fr: "chenillard" },
    Range { from: 192, to: 255, label_fr: "couleurs du maître (réservé)" },
];

/// Our zone effects (fixture.rs), by number.
pub const EFFECTS: [&str; 4] = ["Vague", "Pulsation", "Balancement", "Torsion"];

pub const EFFECT_RANGES: &[Range] = &[
    Range { from: 0, to: 0, label_fr: "aucun" },
    Range { from: 1, to: 1, label_fr: "Vague" },
    Range { from: 2, to: 2, label_fr: "Pulsation" },
    Range { from: 3, to: 3, label_fr: "Balancement" },
    Range { from: 4, to: 4, label_fr: "Torsion" },
    Range { from: 5, to: 255, label_fr: "aucun (réservé)" },
];

pub const CUE_RANGES: &[Range] = &[Range { from: 0, to: 0, label_fr: "aucun" }, Range { from: 1, to: 40, label_fr: "case 1 à 40 de la page" }, Range { from: 41, to: 255, label_fr: "aucun" }];

pub const PAGE_RANGES: &[Range] = &[Range { from: 0, to: 0, label_fr: "page affichée" }, Range { from: 1, to: 255, label_fr: "page N" }];

/// Highest strobe rate asked for (the strobe limiter of T-101 has the last word).
pub const STROBE_MAX_HZ: f32 = 20.0;
/// Below this the strobe channel is open (no strobe).
pub const STROBE_OPEN_BELOW: u8 = 8;
/// Rotation speed at either end of its channel, degrees per second.
pub const ROT_SPEED_MAX: f32 = 360.0;

const fn ch(offset: u16, key: &'static str, label_fr: &'static str, kind: ChanKind, default: u8, param: Param, values_fr: &'static str) -> ChannelDef {
    let smoothed = matches!(param, Param::Size | Param::PosX | Param::PosY | Param::Rotation | Param::SizeX | Param::SizeY);
    ChannelDef { offset, key, label_fr, kind, default, smoothed, param, values_fr, ranges: &[] }
}

const fn ranged(offset: u16, key: &'static str, label_fr: &'static str, kind: ChanKind, param: Param, ranges: &'static [Range], values_fr: &'static str) -> ChannelDef {
    ChannelDef { offset, key, label_fr, kind, default: 0, smoothed: false, param, values_fr, ranges }
}

use ChanKind::{Access, Coarse, Fine, Function, Index, Level};

const V_INTENSITY: &str = "0 = noir, 255 = 100 % du maître";
const V_SIZE: &str = "0 = 0 %, 128 = 100 %, 255 = 200 %";
const V_POS: &str = "0 = bord gauche/bas, 128 = centre, 255 = bord droit/haut";
const V_ROT: &str = "0 = −180°, 128 = 0°, 255 = +180°";
const V_ROT_SPEED: &str = "0 = −360 °/s, 128 = arrêt, 255 = +360 °/s";
const V_ANIM: &str = "64 = vitesse normale, 0 = figé, 255 ≈ ×4 (tout le studio)";
const V_COLOR: &str = "plages de 32 : maître, fixe, teinte, palette, arc-en-ciel, chenillard";
const V_HUE: &str = "0 à 255 = 0 à 360°";
const V_RGB: &str = "0 à 255";
const V_MIX: &str = "0 = couleurs du cue, 255 = couleur RVB seule";
const V_STROBE: &str = "0–7 = ouvert, 8–255 = 1 à 20 Hz (limité par le limiteur de stroboscope)";
const V_PAGE: &str = "0 = page affichée, N = page N";
const V_CUE: &str = "0 = aucun, 1–40 = case de la page (joue le cue à l'arrivée)";
const V_FX: &str = "0 = aucun, 1 Vague, 2 Pulsation, 3 Balancement, 4 Torsion";
const V_FX_AMOUNT: &str = "0 = sans effet, 255 = plein effet";
const V_FUNCTION: &str = "0–127 rien, 128–191 arrêter les cues, 192–255 noir (arrêt d'urgence)";
const V_ACCESS: &str = "0–127 n'obéit à rien (sauf noir), 128–255 obéit";
const V_FINE: &str = "octet de poids faible";

/// « Compact »: 16 channels, 8 bits.
pub const COMPACT: &[ChannelDef] = &[
    ch(0, "intensity", "Intensité", Level, 255, Param::Intensity, V_INTENSITY),
    ch(1, "size", "Taille", Level, 128, Param::Size, V_SIZE),
    ch(2, "pos_x", "Position X", Level, 128, Param::PosX, V_POS),
    ch(3, "pos_y", "Position Y", Level, 128, Param::PosY, V_POS),
    ch(4, "rotation", "Rotation", Level, 128, Param::Rotation, V_ROT),
    ch(5, "rot_speed", "Vitesse de rotation", Level, 128, Param::RotSpeed, V_ROT_SPEED),
    ch(6, "anim_speed", "Vitesse d'animation", Level, 64, Param::AnimSpeed, V_ANIM),
    ranged(7, "color_mode", "Mode couleur", Index, Param::ColorMode, COLOR_RANGES, V_COLOR),
    ch(8, "hue", "Teinte", Level, 0, Param::Hue, V_HUE),
    ch(9, "strobe", "Stroboscope", Level, 0, Param::Strobe, V_STROBE),
    ranged(10, "page", "Page", Index, Param::Page, PAGE_RANGES, V_PAGE),
    ranged(11, "cue", "Cue", Index, Param::Cue, CUE_RANGES, V_CUE),
    ch(12, "size_x", "Taille X", Level, 128, Param::SizeX, V_SIZE),
    ch(13, "size_y", "Taille Y", Level, 128, Param::SizeY, V_SIZE),
    ranged(14, "function", "Fonction", Function, Param::Function, FUNCTION_RANGES, V_FUNCTION),
    ranged(15, "access", "Accès", Access, Param::Access, &[], V_ACCESS),
];

/// « Complet »: 32 channels, geometry in 16 bits.
pub const FULL: &[ChannelDef] = &[
    ch(0, "intensity", "Intensité", Level, 255, Param::Intensity, V_INTENSITY),
    ch(1, "size", "Taille", Coarse, 128, Param::Size, V_SIZE),
    ch(2, "size_fine", "Taille (fin)", Fine, 0, Param::Size, V_FINE),
    ch(3, "pos_x", "Position X", Coarse, 128, Param::PosX, V_POS),
    ch(4, "pos_x_fine", "Position X (fin)", Fine, 0, Param::PosX, V_FINE),
    ch(5, "pos_y", "Position Y", Coarse, 128, Param::PosY, V_POS),
    ch(6, "pos_y_fine", "Position Y (fin)", Fine, 0, Param::PosY, V_FINE),
    ch(7, "rotation", "Rotation", Coarse, 128, Param::Rotation, V_ROT),
    ch(8, "rotation_fine", "Rotation (fin)", Fine, 0, Param::Rotation, V_FINE),
    ch(9, "size_x", "Taille X", Level, 128, Param::SizeX, V_SIZE),
    ch(10, "size_y", "Taille Y", Level, 128, Param::SizeY, V_SIZE),
    ch(11, "rot_speed", "Vitesse de rotation", Level, 128, Param::RotSpeed, V_ROT_SPEED),
    ch(12, "anim_speed", "Vitesse d'animation", Level, 64, Param::AnimSpeed, V_ANIM),
    ranged(13, "color_mode", "Mode couleur", Index, Param::ColorMode, COLOR_RANGES, V_COLOR),
    ch(14, "hue", "Teinte", Level, 0, Param::Hue, V_HUE),
    ch(15, "red", "Rouge", Level, 255, Param::Red, V_RGB),
    ch(16, "green", "Vert", Level, 255, Param::Green, V_RGB),
    ch(17, "blue", "Bleu", Level, 255, Param::Blue, V_RGB),
    ch(18, "rgb_mix", "Dosage RVB", Level, 0, Param::RgbMix, V_MIX),
    ch(19, "strobe", "Stroboscope", Level, 0, Param::Strobe, V_STROBE),
    ranged(20, "page", "Page", Index, Param::Page, PAGE_RANGES, V_PAGE),
    ranged(21, "cue", "Cue", Index, Param::Cue, CUE_RANGES, V_CUE),
    ranged(22, "fx1", "Effet 1", Index, Param::Effect(0), EFFECT_RANGES, V_FX),
    ch(23, "fx1_amount", "Effet 1 : dosage", Level, 0, Param::EffectAmount(0), V_FX_AMOUNT),
    ranged(24, "fx2", "Effet 2", Index, Param::Effect(1), EFFECT_RANGES, V_FX),
    ch(25, "fx2_amount", "Effet 2 : dosage", Level, 0, Param::EffectAmount(1), V_FX_AMOUNT),
    ranged(26, "fx3", "Effet 3", Index, Param::Effect(2), EFFECT_RANGES, V_FX),
    ch(27, "fx3_amount", "Effet 3 : dosage", Level, 0, Param::EffectAmount(2), V_FX_AMOUNT),
    ranged(28, "fx4", "Effet 4", Index, Param::Effect(3), EFFECT_RANGES, V_FX),
    ch(29, "fx4_amount", "Effet 4 : dosage", Level, 0, Param::EffectAmount(3), V_FX_AMOUNT),
    ranged(30, "function", "Fonction", Function, Param::Function, FUNCTION_RANGES, V_FUNCTION),
    ranged(31, "access", "Accès", Access, Param::Access, &[], V_ACCESS),
];

/// *Fonction*'s action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Func {
    #[default]
    Nothing,
    StopCues,
    Blackout,
}

impl Func {
    pub fn of(v: u8) -> Self {
        match v {
            0..=127 => Func::Nothing,
            128..=191 => Func::StopCues,
            _ => Func::Blackout,
        }
    }
}

/// What a zone's instance asks for, in the studio's units. Neutral by
/// default: applied to a zone, it changes nothing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZoneRemote {
    /// Multiplies the master dimmer, 0..1.
    pub brightness: f32,
    /// Multiply the master sizes, 0..2.
    pub size: f32,
    pub size_x: f32,
    pub size_y: f32,
    /// Added to the master position, -1..1.
    pub pos: [f32; 2],
    /// Added to the master Z angle, degrees.
    pub rot_deg: f32,
    /// Degrees per second, accumulated by the instance.
    pub rot_speed: f32,
    /// `live::COLOR_MODE_LABELS` index; 0 keeps the master's colours.
    pub color_mode: usize,
    pub hue: f32,
    pub rgb: [u8; 3],
    /// 0..1: how much of `rgb` replaces the colours.
    pub rgb_mix: f32,
    /// 0: no strobe.
    pub strobe_hz: f32,
    /// (effect number, amount 0..1); 0 = none.
    pub effects: [(u8, f32); 4],
}

impl Default for ZoneRemote {
    fn default() -> Self {
        ZoneRemote {
            brightness: 1.0,
            size: 1.0,
            size_x: 1.0,
            size_y: 1.0,
            pos: [0.0; 2],
            rot_deg: 0.0,
            rot_speed: 0.0,
            color_mode: 0,
            hue: 0.0,
            rgb: [255; 3],
            rgb_mix: 0.0,
            strobe_hz: 0.0,
            effects: [(0, 0.0); 4],
        }
    }
}

impl ZoneRemote {
    /// The smoothed values, in a fixed order (fixture.rs interpolates them).
    pub fn geometry(&self) -> [f32; 6] {
        [self.size, self.size_x, self.size_y, self.pos[0], self.pos[1], self.rot_deg]
    }

    pub fn set_geometry(&mut self, g: [f32; 6]) {
        [self.size, self.size_x, self.size_y, self.pos[0], self.pos[1], self.rot_deg] = g;
    }
}

/// One packet decoded with a personality.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub remote: ZoneRemote,
    /// The studio's animation speed (global: looks are shared by zones).
    pub anim_speed: f32,
    /// 0: the page shown; N: page N.
    pub page: u8,
    pub cue: u8,
    pub function: Func,
    pub access: bool,
}

/// A 16-bit value as 0..1. Centred: 32768 is exactly ½.
pub fn norm16(v: u16, centred: bool) -> f32 {
    let v = v as f32;
    match centred {
        false => v / 65535.0,
        true if v <= 32768.0 => v / 65536.0,
        true => 0.5 + (v - 32768.0) / 65534.0,
    }
}

/// An 8-bit value as 0..1. Centred: 128 is exactly ½.
pub fn norm8(v: u8, centred: bool) -> f32 {
    let v = v as f32;
    match centred {
        false => v / 255.0,
        true if v <= 128.0 => v / 256.0,
        true => 0.5 + (v - 128.0) / 254.0,
    }
}

/// Decodes an instance's channels (`values[i]` is channel `i` from the
/// start address; `None` past the end of a short packet, which counts as
/// the channel's default).
pub fn decode(mode: Personality, values: &[Option<u8>]) -> Decoded {
    let table = mode.channels();
    let raw = |d: &ChannelDef| values.get(d.offset as usize).copied().flatten().unwrap_or(d.default);
    // 0..1 for a parameter: 16 bits when it has a fine channel.
    let norm = |p: Param| -> Option<f32> {
        let coarse = table.iter().find(|d| d.param == p && d.kind != ChanKind::Fine)?;
        Some(match table.iter().find(|d| d.param == p && d.kind == ChanKind::Fine) {
            Some(fine) => norm16(u16::from(raw(coarse)) << 8 | u16::from(raw(fine)), p.centred()),
            None => norm8(raw(coarse), p.centred()),
        })
    };
    let byte = |p: Param| table.iter().find(|d| d.param == p).map(raw);
    let mut r = ZoneRemote::default();
    if let Some(n) = norm(Param::Intensity) {
        r.brightness = n;
    }
    if let Some(n) = norm(Param::Size) {
        r.size = n * 2.0;
    }
    if let Some(n) = norm(Param::SizeX) {
        r.size_x = n * 2.0;
    }
    if let Some(n) = norm(Param::SizeY) {
        r.size_y = n * 2.0;
    }
    if let Some(n) = norm(Param::PosX) {
        r.pos[0] = n * 2.0 - 1.0;
    }
    if let Some(n) = norm(Param::PosY) {
        r.pos[1] = n * 2.0 - 1.0;
    }
    if let Some(n) = norm(Param::Rotation) {
        r.rot_deg = n * 360.0 - 180.0;
    }
    if let Some(n) = norm(Param::RotSpeed) {
        r.rot_speed = (n * 2.0 - 1.0) * ROT_SPEED_MAX;
    }
    if let Some(v) = byte(Param::ColorMode) {
        let m = v as usize / 32;
        r.color_mode = if m < crate::live::COLOR_MODE_LABELS.len() { m } else { 0 };
    }
    if let Some(n) = norm(Param::Hue) {
        r.hue = n * 360.0;
    }
    for (i, p) in [Param::Red, Param::Green, Param::Blue].into_iter().enumerate() {
        if let Some(v) = byte(p) {
            r.rgb[i] = v;
        }
    }
    if let Some(n) = norm(Param::RgbMix) {
        r.rgb_mix = n;
    }
    if let Some(v) = byte(Param::Strobe) {
        r.strobe_hz = strobe_hz(v);
    }
    for slot in 0..4u8 {
        let n = byte(Param::Effect(slot)).unwrap_or(0);
        let amount = norm(Param::EffectAmount(slot)).unwrap_or(0.0);
        r.effects[slot as usize] = (if (n as usize) <= EFFECTS.len() { n } else { 0 }, amount);
    }
    Decoded {
        remote: r,
        anim_speed: byte(Param::AnimSpeed).map_or(1.0, |v| v as f32 / 64.0),
        page: byte(Param::Page).unwrap_or(0),
        cue: byte(Param::Cue).unwrap_or(0),
        function: byte(Param::Function).map_or(Func::Nothing, Func::of),
        access: byte(Param::Access).is_some_and(|v| v >= 128),
    }
}

/// The strobe channel's rate: 0 while open.
pub fn strobe_hz(v: u8) -> f32 {
    if v < STROBE_OPEN_BELOW {
        return 0.0;
    }
    1.0 + (v - STROBE_OPEN_BELOW) as f32 / (255 - STROBE_OPEN_BELOW) as f32 * (STROBE_MAX_HZ - 1.0)
}

/// The defaults of a personality, as a packet would carry them.
#[cfg(test)]
pub fn defaults(mode: Personality) -> Vec<Option<u8>> {
    mode.channels().iter().map(|d| Some(d.default)).collect()
}

/// A personality as JSON (`GET /api/dmx/personality`).
pub fn json(mode: Personality) -> serde_json::Value {
    let channels: Vec<serde_json::Value> = mode
        .channels()
        .iter()
        .map(|d| {
            serde_json::json!({
                "channel": d.offset + 1,
                "key": d.key,
                "label": d.label_fr,
                "kind": d.kind,
                "kind_label": d.kind.label_fr(),
                "default": d.default,
                "smoothed": d.smoothed,
                "values": d.values_fr,
                "ranges": d.ranges,
            })
        })
        .collect();
    serde_json::json!({ "version": VERSION, "mode": mode, "name": mode.name_fr(), "label": mode.label_fr(), "width": mode.width(), "channels": channels })
}

const MD_INTRO: &str = "\
Laser Studio se présente à une console lumière comme un projecteur : une \
instance par zone de projection, chacune à son adresse de départ (RÉGLAGES › \
Entrées réseau › *Appareils DMX*). Deux personnalités, de notre conception.

Règles communes :
- Aucun canal n'arme le laser, ne change un réglage de sécurité, une zone ou \
la calibration. Il n'y a pas de canal « allumer le laser », volontairement.
- Rien n'est appliqué tant que les *Entrées externes* sont fermées à l'écran, \
ni hors du périmètre DMX (*Maîtres* pour l'image, *Cues* pour page, cue et \
arrêt des cues).
- *Accès* sous 128 : l'instance n'obéit à rien, sauf *Fonction › noir*. \
L'image de la zone revient au neutre.
- 16 bits : le canal gros (poids fort) puis le canal fin.
- Les canaux centrés valent exactement le neutre à 128 (32768 en 16 bits).
- Lissage : taille, position et rotation sont interpolées entre deux trames \
reçues (60 ms par défaut, réglable par appareil). Page, cue, fonction et accès \
ne sont jamais lissés.
- Le stroboscope passe par le limiteur de stroboscope comme tout le reste.
- Intensité, taille, position, rotation et couleur s'appliquent à l'image de \
la zone, après les maîtres et avant la calibration et les étages de sécurité. \
La vitesse d'animation, les cues et l'arrêt des cues agissent sur tout le \
studio (les cues jouent sur leurs propres zones).
";

fn md_table(mode: Personality, out: &mut String) {
    out.push_str(&format!("## {}\n\n", mode.label_fr()));
    out.push_str("| Canal | Nom | Clé | Type | Défaut | Valeurs | Lissé |\n|---:|---|---|---|---:|---|---|\n");
    for d in mode.channels() {
        out.push_str(&format!(
            "| {} | {} | `{}` | {} | {} | {} | {} |\n",
            d.offset + 1,
            d.label_fr,
            d.key,
            d.kind.label_fr(),
            d.default,
            d.values_fr,
            if d.smoothed { "oui" } else { "non" }
        ));
    }
    out.push('\n');
}

/// The published chart (one personality, or both with the rules).
pub fn markdown(mode: Option<Personality>) -> String {
    let mut out = String::new();
    match mode {
        Some(m) => md_table(m, &mut out),
        None => {
            out.push_str("# Laser Studio — personnalités DMX\n\n");
            out.push_str("<!-- Généré depuis studio/src/dmx/personality.rs par `cargo test -p laser-studio published_chart` (UPDATE_DMX_CHART=1 pour le réécrire). Ne pas modifier à la main. -->\n\n");
            out.push_str(&format!("Version de la table : {VERSION}.\n\n"));
            out.push_str(MD_INTRO);
            out.push('\n');
            for m in Personality::ALL {
                md_table(m, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_contiguous_unique_offsets() {
        assert_eq!((Personality::Compact.width(), Personality::Full.width()), (16, 32));
        for mode in Personality::ALL {
            let t = mode.channels();
            for (i, d) in t.iter().enumerate() {
                assert_eq!(d.offset as usize, i, "{mode:?} {}", d.key);
                assert!(t[..i].iter().all(|o| o.key != d.key), "{mode:?}: key {} twice", d.key);
                if d.kind == ChanKind::Fine {
                    assert_eq!((t[i - 1].kind, t[i - 1].param), (ChanKind::Coarse, d.param), "a fine channel follows its coarse one");
                }
                if d.kind == ChanKind::Coarse {
                    assert_eq!(t[i + 1].kind, ChanKind::Fine);
                }
            }
            assert_eq!(t.iter().filter(|d| d.kind == ChanKind::Access).count(), 1);
            assert_eq!(t.iter().filter(|d| d.param == Param::Function).count(), 1);
            // Only geometry is smoothed; page, cue, function and access never.
            for d in t.iter().filter(|d| matches!(d.param, Param::Page | Param::Cue | Param::Function | Param::Access)) {
                assert!(!d.smoothed, "{}", d.key);
            }
        }
        // Full: size, position and rotation in 16 bits.
        let full16: Vec<&str> = FULL.iter().filter(|d| d.kind == ChanKind::Coarse).map(|d| d.key).collect();
        assert_eq!(full16, ["size", "pos_x", "pos_y", "rotation"]);
    }

    #[test]
    fn sixteen_bits_coarse_then_fine() {
        let mut v = defaults(Personality::Full);
        // Position X: 0x40 then 0x80.
        (v[3], v[4]) = (Some(0x40), Some(0x80));
        let d = decode(Personality::Full, &v);
        let want = norm16(0x4080, true) * 2.0 - 1.0;
        assert!((d.remote.pos[0] - want).abs() < 1e-6);
        assert!(d.remote.pos[0] < -0.49 && d.remote.pos[0] > -0.5);
        // The extremes and the true centre.
        assert_eq!((norm16(0, true), norm16(32768, true), norm16(65535, true)), (0.0, 0.5, 1.0));
        assert_eq!((norm8(0, true), norm8(128, true), norm8(255, true)), (0.0, 0.5, 1.0));
        assert_eq!(norm16(65535, false), 1.0);
    }

    #[test]
    fn defaults_are_neutral() {
        for mode in Personality::ALL {
            let d = decode(mode, &defaults(mode));
            assert_eq!(d.remote, ZoneRemote::default(), "{mode:?}");
            assert_eq!((d.anim_speed, d.page, d.cue, d.function, d.access), (1.0, 0, 0, Func::Nothing, false), "{mode:?}");
            // A short packet: the missing channels are their defaults.
            assert_eq!(decode(mode, &[]).remote, ZoneRemote::default());
        }
    }

    #[test]
    fn ranges_and_values() {
        assert_eq!((Func::of(0), Func::of(127), Func::of(128), Func::of(191), Func::of(192), Func::of(255)), (Func::Nothing, Func::Nothing, Func::StopCues, Func::StopCues, Func::Blackout, Func::Blackout));
        assert_eq!((strobe_hz(0), strobe_hz(7), strobe_hz(8), strobe_hz(255)), (0.0, 0.0, 1.0, STROBE_MAX_HZ));
        let mut v = defaults(Personality::Compact);
        (v[7], v[15], v[11], v[10]) = (Some(64), Some(128), Some(5), Some(2));
        let d = decode(Personality::Compact, &v);
        assert_eq!((d.remote.color_mode, d.access, d.cue, d.page), (2, true, 5, 2));
        v[7] = Some(200);
        assert_eq!(decode(Personality::Compact, &v).remote.color_mode, 0, "reserved range: the master's colours");
        // Each table's ranges cover 0..=255 without gaps.
        for d in COMPACT.iter().chain(FULL).filter(|d| !d.ranges.is_empty()) {
            assert_eq!(d.ranges[0].from, 0, "{}", d.key);
            assert_eq!(d.ranges.last().unwrap().to, 255, "{}", d.key);
            for w in d.ranges.windows(2) {
                assert_eq!(w[0].to as u16 + 1, w[1].from as u16, "{}", d.key);
            }
        }
    }

    /// The published chart is generated from the tables the receiver uses;
    /// the versioned file must match (`UPDATE_DMX_CHART=1` rewrites it).
    #[test]
    fn published_chart() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dmx-personality.md");
        let want = markdown(None);
        if std::env::var_os("UPDATE_DMX_CHART").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(have == want, "{} differs from the tables: run with UPDATE_DMX_CHART=1 and commit it", path.display());
        for mode in Personality::ALL {
            for d in mode.channels() {
                assert!(want.contains(&format!("`{}`", d.key)));
            }
        }
    }
}
