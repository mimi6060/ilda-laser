//! Our built-in cue library: a few hundred ready-made looks, organised in
//! pages like the cue grids of pro laser software. Every cue is one of our
//! own generators or shapes with chosen parameters and colours - nothing is
//! taken from another product's library. The catalogue is built in code,
//! so ids stay stable as long as the lists below only grow at the end.

use crate::engine::{Animator, AudioFeatures, AudioReact, BeatClock, Content, GateMode, Settings};
use crate::evolving::{EvolvingCue, EvolvingKey, KeyEase};
use crate::generators::{ColorMode, GenParams};
use crate::lfo::{Modulator, Wave};
use crate::live::Rate;
use crate::program::{CueProgram, Key, Lane, PlayMode};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub category: &'static str,
    pub settings: Settings,
    /// Where in a track the cue belongs (the « Festival » page, T-110):
    /// a coloured dot on its grid cell. `None` = untagged (reads as
    /// « Groove »).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<CueTag>,
}

/// The part of a track a cue is made for (T-110).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueTag {
    Intro,
    #[default]
    Groove,
    Break,
    /// « Montée ».
    Build,
    Drop,
}

/// The « Festival » page (T-110): a curated pick of the festival
/// generators, each in a festival palette.
pub const FESTIVAL: &str = "Festival";

/// The « Festival évolutifs » page (T-112 to T-123): twelve cues that
/// evolve by themselves over 16 or 32 beats (builds, drops, breaks).
pub const EVOLVING: &str = "Festival évolutifs";

/// Cue pages, in display order. Figures (`figures.rs`), ILDA files
/// (`ilda_library.rs`) and 3D objects (`wire.rs`) hold the operator's
/// libraries, not built-in cues.
/// The last `LATE_PAGES` came later: in the default grid they go after
/// the overflow pages of the ones above (« Abstraits 2 », « Faisceaux
/// 2 »), so every older page keeps its number (`page.N`, F-keys, MIDI,
/// DMX and APC page mappings).
pub const CATEGORIES: &[&str] = &[
    "Abstraits",
    "Tunnels",
    "Faisceaux",
    "Balayages",
    "Vagues",
    "Géométrie",
    "Audio",
    "Texte & horloge",
    crate::figures::CATEGORY,
    crate::ilda_library::CATEGORY,
    FESTIVAL,
    EVOLVING,
    crate::wire::CATEGORY,
];

/// How many of the last `CATEGORIES` are appended pages (see above).
pub const LATE_PAGES: usize = 3;

const GREEN: [u8; 3] = [0, 255, 0];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 80, 255];
const CYAN: [u8; 3] = [0, 255, 255];
const MAGENTA: [u8; 3] = [255, 0, 255];
const YELLOW: [u8; 3] = [255, 220, 0];
const WHITE: [u8; 3] = [255, 255, 255];

/// A colour recipe: main colour, colour mode, second colour, and its name
/// as shown in the cue label.
#[derive(Clone, Copy)]
struct Look(&'static str, [u8; 3], ColorMode, [u8; 3]);

const SOLID_GREEN: Look = Look("vert", GREEN, ColorMode::Solid, GREEN);
const SOLID_RED: Look = Look("rouge", RED, ColorMode::Solid, RED);
const SOLID_BLUE: Look = Look("bleu", BLUE, ColorMode::Solid, BLUE);
const SOLID_CYAN: Look = Look("cyan", CYAN, ColorMode::Solid, CYAN);
const SOLID_WHITE: Look = Look("blanc", WHITE, ColorMode::Solid, WHITE);
const SOLID_YELLOW: Look = Look("jaune", YELLOW, ColorMode::Solid, YELLOW);
const RAINBOW: Look = Look("arc-en-ciel", WHITE, ColorMode::Rainbow, WHITE);
const CYAN_MAGENTA: Look = Look("cyan → magenta", CYAN, ColorMode::Gradient, MAGENTA);
const GREEN_RED: Look = Look("vert → rouge", GREEN, ColorMode::Gradient, RED);
const RED_BLUE: Look = Look("rouge / bleu", RED, ColorMode::Alternate, BLUE);
const GREEN_YELLOW: Look = Look("vert / jaune", GREEN, ColorMode::Alternate, YELLOW);
const SOLID_MAGENTA: Look = Look("magenta", MAGENTA, ColorMode::Solid, MAGENTA);
/// Scissors in « Alterné »: the left group cyan, the right one blue.
const CYAN_BLUE: Look = Look("cyan / bleu", CYAN, ColorMode::Alternate, BLUE);
/// Chasers in « Alterné »: a white head with a blue tail.
const WHITE_BLUE: Look = Look("blanc / bleu", WHITE, ColorMode::Alternate, BLUE);

struct Builder {
    presets: Vec<Preset>,
}

impl Builder {
    fn add(&mut self, category: &'static str, name: String, settings: Settings) {
        let n = self.presets.iter().filter(|p| p.category == category).count() + 1;
        self.presets.push(Preset { id: format!("{}-{n:03}", slug(category)), name, category, settings, tag: None });
    }

    #[allow(clippy::too_many_arguments)]
    fn generator(&mut self, category: &'static str, label: &str, generator: &str, count: u32, a: f32, b: f32, look: Look, scale: f32) {
        let Look(color_name, color, color_mode, color2) = look;
        let params = GenParams { count, a, b, speed: 1.0, color_mode, color2, ..GenParams::default() };
        let settings = Settings {
            content: Content::Generator { generator: generator.to_string(), params },
            color,
            scale,
            ..Settings::default()
        };
        self.add(category, format!("{label} · {color_name}"), settings);
    }

    /// A beat-synced beam cue (T-103) on the « Faisceaux » page: `rhythm`
    /// sets the look's gate or strobe.
    #[allow(clippy::too_many_arguments)]
    fn synced(&mut self, label: &str, generator: &str, count: u32, a: f32, b: f32, steps: f32, look: Look, rhythm: impl FnOnce(&mut Settings)) {
        let Look(color_name, color, color_mode, color2) = look;
        let params = GenParams { count, a, b, color_mode, color2, beat_sync: true, steps_per_beat: steps, ..GenParams::default() };
        let mut settings = Settings {
            content: Content::Generator { generator: generator.to_string(), params },
            color,
            scale: 0.7,
            ..Settings::default()
        };
        rhythm(&mut settings);
        self.add("Faisceaux", format!("{label} · {color_name}"), settings);
    }

    /// A beat-synced graphic (T-108) on page `category`: `steps` per beat
    /// (the countdown), a motion over `period` beats (ring, 3D objects).
    #[allow(clippy::too_many_arguments)]
    fn graphic(&mut self, category: &'static str, label: &str, generator: &str, count: u32, a: f32, steps: f32, period: f32, look: Look, scale: f32) {
        let Look(color_name, color, color_mode, color2) = look;
        let params = GenParams { count, a, color_mode, color2, beat_sync: true, steps_per_beat: steps, period_beats: period, ..GenParams::default() };
        let settings = Settings { content: Content::Generator { generator: generator.to_string(), params }, color, scale, ..Settings::default() };
        self.add(category, format!("{label} · {color_name}"), settings);
    }
}

pub fn catalog() -> Vec<Preset> {
    let mut b = Builder { presets: Vec::new() };

    // Abstraits
    for (x, y) in [(1.0, 2.0), (2.0, 3.0), (3.0, 4.0), (3.0, 5.0), (4.0, 5.0), (5.0, 6.0), (1.0, 3.0), (2.0, 5.0)] {
        for look in [SOLID_GREEN, RAINBOW, CYAN_MAGENTA] {
            b.generator("Abstraits", &format!("Lissajous {x}:{y}"), "lissajous", 1, x, y, look, 0.6);
        }
    }
    for (i, k) in [2.5, 3.0, 3.5, 4.0, 5.0, 7.0].into_iter().enumerate() {
        for pen in [0.6, 1.2] {
            let look = if i % 2 == 0 { RAINBOW } else { SOLID_CYAN };
            b.generator("Abstraits", &format!("Spirographe {k} / {pen}"), "spirograph", 1, k, pen, look, 0.6);
        }
    }
    for petals in [3, 4, 5, 6, 7, 8] {
        for look in [SOLID_RED, RAINBOW] {
            b.generator("Abstraits", &format!("Rosace {petals} pétales"), "rose", petals, 0.0, 0.0, look, 0.6);
        }
    }
    for count in [6, 8, 12] {
        for look in [SOLID_GREEN, RAINBOW] {
            b.generator("Abstraits", &format!("Fleur {count}"), "flower", count, 0.0, 0.0, look, 0.5);
        }
    }
    for look in [SOLID_BLUE, RAINBOW, CYAN_MAGENTA] {
        b.generator("Abstraits", "Vortex", "vortex", 1, 0.0, 0.0, look, 0.6);
    }

    // Tunnels
    for count in [4, 6, 8] {
        for look in [SOLID_CYAN, RED_BLUE, RAINBOW] {
            b.generator("Tunnels", &format!("Tunnel {count} anneaux"), "tunnel", count, 0.0, 0.0, look, 0.7);
        }
    }
    for sides in [3, 4, 5, 6, 8] {
        for (twist, look) in [(0.0, CYAN_MAGENTA), (1.5, RAINBOW)] {
            let label = if twist > 0.0 { format!("Tunnel {sides} côtés torsadé") } else { format!("Tunnel {sides} côtés") };
            b.generator("Tunnels", &label, "polygon_tunnel", 6, sides as f32, twist, look, 0.7);
        }
    }
    for count in [3, 5] {
        for look in [SOLID_GREEN, GREEN_YELLOW] {
            b.generator("Tunnels", &format!("Anneaux pulsés {count}"), "pulse_rings", count, 0.0, 0.0, look, 0.7);
        }
    }

    // Faisceaux (beam looks: best in haze)
    for count in [3, 5, 8, 12] {
        for look in [SOLID_GREEN, SOLID_BLUE, SOLID_RED, RAINBOW] {
            b.generator("Faisceaux", &format!("Éventail {count}"), "beam_fan", count, 0.0, 0.3, look, 0.7);
        }
    }
    for count in [6, 10, 16] {
        for look in [SOLID_CYAN, RED_BLUE, RAINBOW] {
            b.generator("Faisceaux", &format!("Cône {count}"), "beam_circle", count, 0.0, 0.0, look, 0.6);
        }
    }
    for count in [8, 12] {
        for look in [SOLID_GREEN, GREEN_YELLOW, RAINBOW] {
            b.generator("Faisceaux", &format!("Vague de faisceaux {count}"), "beam_wave", count, 3.0, 0.0, look, 0.7);
        }
    }

    // Balayages
    for look in [SOLID_GREEN, SOLID_RED, SOLID_CYAN, RAINBOW] {
        b.generator("Balayages", "Balayage", "sweep", 1, 0.0, 0.0, look, 0.7);
    }
    for look in [SOLID_GREEN, SOLID_BLUE, CYAN_MAGENTA, RAINBOW] {
        b.generator("Balayages", "Nappe (liquid sky)", "liquid_sky", 1, 0.0, 0.6, look, 0.8);
    }
    for count in [3, 5] {
        for (cross, label) in [(0.0, "Lignes"), (1.0, "Grille")] {
            for look in [SOLID_GREEN, RED_BLUE] {
                b.generator("Balayages", &format!("{label} de balayage {count}"), "grid_scan", count, cross, 0.0, look, 0.7);
            }
        }
    }

    // Vagues
    for count in [2, 3, 5] {
        for look in [SOLID_CYAN, RAINBOW, GREEN_RED] {
            b.generator("Vagues", &format!("Oscillateurs ×{count}"), "sine_stack", count, 1.0, 0.8, look, 0.7);
        }
    }
    for look in [SOLID_GREEN, RED_BLUE, CYAN_MAGENTA, RAINBOW] {
        b.generator("Vagues", "Hélice ADN", "helix", 2, 1.5, 0.0, look, 0.7);
    }
    for count in [2, 3, 4, 6] {
        for look in [SOLID_GREEN, RAINBOW] {
            b.generator("Vagues", &format!("Spirale {count} bras"), "spiral_arms", count, 1.0, 0.0, look, 0.6);
        }
    }
    for count in [8, 12, 24] {
        for look in [SOLID_YELLOW, RAINBOW] {
            b.generator("Vagues", &format!("Soleil {count} rayons"), "starburst", count, 0.0, 0.0, look, 0.6);
        }
    }

    // Géométrie
    for (shape, label) in [("circle", "Cercle"), ("square", "Carré"), ("triangle", "Triangle"), ("star", "Étoile")] {
        for (color_name, color, rotation) in [("vert", GREEN, 0.0), ("rouge", RED, 45.0), ("bleu", BLUE, -90.0)] {
            let settings = Settings {
                content: Content::Shape { shape: shape.to_string() },
                color,
                rotation_speed: rotation,
                ..Settings::default()
            };
            let spin = if rotation != 0.0 { " qui tourne" } else { "" };
            b.add("Géométrie", format!("{label}{spin} · {color_name}"), settings);
        }
    }

    // Audio (react to the microphone out of the box)
    for count in [6, 10, 14] {
        for look in [GREEN_RED, RAINBOW] {
            b.generator("Audio", &format!("Spectre {count} barres"), "spectrum", count, 0.0, 0.0, look, 0.7);
        }
    }
    for (label, generator, count, look) in [
        ("Rosace qui pulse", "rose", 5, RAINBOW),
        ("Soleil qui pulse", "starburst", 16, SOLID_YELLOW),
        ("Tunnel au rythme", "tunnel", 6, RED_BLUE),
        ("Éventail au rythme", "beam_fan", 8, SOLID_GREEN),
        ("Oscillateurs au rythme", "sine_stack", 3, CYAN_MAGENTA),
        ("Spirale au rythme", "spiral_arms", 4, RAINBOW),
        ("Vortex au rythme", "vortex", 1, CYAN_MAGENTA),
        ("Cône au rythme", "beam_circle", 10, RED_BLUE),
        ("Anneaux au rythme", "pulse_rings", 4, SOLID_GREEN),
    ] {
        b.generator("Audio", label, generator, count, 1.0, 0.8, look, 0.6);
    }
    for p in b.presets.iter_mut().filter(|p| p.category == "Audio") {
        p.settings.audio = AudioReact { enabled: true, size: 0.4, rotate: 0.1, color_on_beat: true, flash: 0.3 };
    }

    // Texte & horloge
    for look in [SOLID_WHITE, SOLID_GREEN, RAINBOW] {
        b.generator("Texte & horloge", "Horloge", "clock", 1, 0.0, 0.0, look, 0.7);
    }
    for word in ["HELLO", "PARTY", "DJ", "LOVE", "WOW", "MERCI"] {
        for (color_name, color) in [("vert", GREEN), ("rouge", RED), ("cyan", CYAN)] {
            let settings = Settings { content: Content::Text { text: word.to_string(), style: Default::default() }, color, scale: 0.25, ..Settings::default() };
            b.add("Texte & horloge", format!("{word} · {color_name}"), settings);
        }
    }

    // Chasers, kick stabs and beam strobes (T-103), appended so every
    // earlier id stays the same. Chase steps and gates from the research
    // (§4.4, §4.5): 1/2-beat chases with a 2-beam tail, 0.2-beat stabs.
    let none = |_: &mut Settings| ();
    b.synced("Chaser →", "chase_fan", 8, 0.0, 2.0, 2.0, SOLID_GREEN, none);
    b.synced("Chaser aller-retour", "chase_fan", 8, 2.0, 2.0, 2.0, WHITE_BLUE, none);
    b.synced("Chaser centre → bords", "chase_fan", 8, 3.0, 1.0, 1.0, SOLID_CYAN, none);
    b.synced("Chaser bords → centre", "chase_fan", 8, 4.0, 1.0, 1.0, SOLID_RED, none);
    b.synced("Chaser pair / impair", "chase_fan", 8, 5.0, 0.0, 1.0, GREEN_YELLOW, none);
    b.synced("Chaser aléatoire", "chase_fan", 12, 6.0, 0.0, 2.0, RAINBOW, none);
    b.synced("Chaser remplissage", "chase_fan", 8, 7.0, 0.0, 1.0, SOLID_BLUE, none);
    b.synced("Coups sur le kick", "fan", 8, 0.0, 0.3, 1.0, SOLID_WHITE, |s| s.gate = GateMode::Beat);
    b.synced("Coups qui s'éteignent", "fan", 8, 0.0, 0.3, 1.0, SOLID_RED, |s| {
        s.gate = GateMode::Beat;
        s.gate_decay = true;
    });
    b.synced("Positions temps + contretemps", "positions", 8, 0.0, 0.3, 2.0, SOLID_MAGENTA, |s| s.gate = GateMode::BeatAndOffbeat);
    b.synced("Strobe 1/2 temps", "fan", 12, 0.0, 0.4, 1.0, SOLID_WHITE, |s| s.strobe_div = 2.0);

    // Cue programs (T-157), appended: a cue as a mini-timeline. Our own
    // settings, on pages with free grid cells.
    for (category, name, settings) in program_cues() {
        b.add(category, name.to_string(), settings);
    }

    // Crossings, hot beam and convergences (T-104), appended on the
    // « Faisceaux » page (its second grid page). Values from the research
    // (§A.5, 11, 12, 29): scissors cross every 2 beats with a = 0.5, the
    // hot beam pans 0.3 over 16 beats at 60 %, bursts open in 1/4 beat.
    let shape = |period: f32, flash: bool| {
        move |s: &mut Settings| {
            if let Content::Generator { params, .. } = &mut s.content {
                params.period_beats = period;
                params.cross_flash = flash;
            }
        }
    };
    b.synced("Ciseaux (X)", "scissor", 8, 0.5, 0.3, 1.0, RED_BLUE, shape(2.0, false));
    b.synced("Ciseaux (X) flash blanc", "scissor", 8, 0.5, 0.3, 1.0, GREEN_YELLOW, shape(2.0, true));
    b.synced("Ciseaux lents (X)", "scissor", 12, 0.45, 0.4, 1.0, CYAN_BLUE, shape(4.0, false));
    b.synced("Faisceau chaud", "hot_beam", 1, 0.3, 0.6, 1.0, SOLID_WHITE, shape(16.0, false));
    b.synced("Faisceau chaud", "hot_beam", 1, 0.3, 0.6, 1.0, SOLID_RED, shape(16.0, false));
    b.synced("Faisceaux qui s'ouvrent", "bundle_burst", 10, 0.25, 0.35, 1.0, SOLID_GREEN, shape(4.0, false));
    b.synced("Faisceaux qui s'ouvrent lents", "bundle_burst", 12, 1.0, 0.45, 1.0, SOLID_CYAN, shape(8.0, false));
    b.synced("V de faisceaux", "beam_v", 10, 0.0, 0.0, 1.0, SOLID_CYAN, none);
    b.synced("Λ de faisceaux", "beam_v", 10, 0.0, 1.0, 1.0, SOLID_WHITE, none);
    b.synced("V qui bascule", "beam_v", 10, 15.0, 0.0, 1.0, SOLID_MAGENTA, none);

    // Textures (T-107), appended on the « Faisceaux » page. Values from the
    // research (§D.30 to 32): 4 to 10 stars redrawn every 1/4 beat, a bolt
    // on every beat, 4 to 6 fat beams of radius 0.01 to 0.03.
    b.synced("Ciel étoilé", "starfield", 6, 0.0, 0.1, 4.0, SOLID_WHITE, none);
    b.synced("Ciel étoilé dense", "starfield", 12, 0.0, 0.1, 4.0, SOLID_CYAN, none);
    b.synced("Ciel étoilé lent", "starfield", 4, 0.0, 0.3, 1.0, SOLID_BLUE, none);
    b.synced("Pluie d'étoiles", "starfield", 8, 1.0, 0.2, 4.0, CYAN_BLUE, none);
    b.synced("Éclairs", "lightning", 8, 0.0, 0.1, 1.0, SOLID_WHITE, none);
    b.synced("Éclairs", "lightning", 7, 0.0, 0.2, 1.0, SOLID_MAGENTA, none);
    b.synced("Faisceaux épais", "fat_beams", 5, 0.02, 0.3, 1.0, SOLID_GREEN, shape(8.0, false));
    b.synced("Faisceaux épais", "fat_beams", 4, 0.03, 0.4, 1.0, RED_BLUE, shape(8.0, false));
    b.synced("Faisceaux épais fins", "fat_beams", 6, 0.015, 0.25, 1.0, SOLID_CYAN, shape(8.0, false));

    // The « Festival » page (T-110), appended: one click per moment of a
    // track, each cue tagged with that moment.
    for (tag, name, settings) in festival_cues() {
        b.add(FESTIVAL, name.to_string(), settings);
        if let Some(p) = b.presets.last_mut() {
            p.tag = Some(tag);
        }
    }

    // Graphics (T-108), appended: countdowns and progress rings on « Texte
    // & horloge », wireframe objects on « Géométrie » (both pages keep room
    // on their first grid page). Research look 34: one number a bar, black
    // on the last 1/2 beat; look 36: a turn every 2 bars.
    const TEXT: &str = "Texte & horloge";
    b.graphic(TEXT, "Compte à rebours 4-3-2-1", "countdown", 4, 0.0, 0.25, 4.0, SOLID_WHITE, 0.4);
    b.graphic(TEXT, "Compte à rebours 4-3-2-1", "countdown", 4, 0.0, 0.25, 4.0, SOLID_RED, 0.4);
    b.graphic(TEXT, "Compte à rebours 8 temps", "countdown", 8, 0.0, 1.0, 4.0, SOLID_CYAN, 0.4);
    b.graphic(TEXT, "Anneau de progression 8 temps", "clock", 1, 0.0, 1.0, 8.0, SOLID_CYAN, 0.6);
    b.graphic(TEXT, "Anneau de progression 16 temps", "clock", 1, 0.0, 1.0, 16.0, GREEN_RED, 0.6);
    b.graphic(TEXT, "Anneau de progression 32 temps", "clock", 1, 0.0, 1.0, 32.0, RAINBOW, 0.6);
    for (a, label, look) in [
        (0.0, "Cube 3D", SOLID_CYAN),
        (1.0, "Octaèdre 3D", SOLID_MAGENTA),
        (2.0, "Icosaèdre 3D", SOLID_GREEN),
        (3.0, "Pyramide 3D", SOLID_YELLOW),
        (4.0, "Globe 3D", SOLID_BLUE),
        (0.0, "Cube 3D", RAINBOW),
    ] {
        b.graphic("Géométrie", label, "wire3d", 1, a, 1.0, 8.0, look, 0.6);
    }

    // The « Festival évolutifs » page (T-112 to T-123), appended.
    for (tag, name, settings) in evolving_cues() {
        b.add(EVOLVING, name, settings);
        if let Some(p) = b.presets.last_mut() {
            p.tag = Some(tag);
        }
    }

    // Abstracts made of stacked oscillators (T-440), appended to
    // « Abstraits » (they land on « Abstraits 2 », so no page moves).
    for (name, settings) in crate::abstracts::catalog() {
        b.add(crate::abstracts::CATEGORY, name, settings);
    }

    b.presets
}

/// Festival palettes (`festival::PALETTES`, T-130) by name.
const PAL_GREEN: usize = 0;
const PAL_WHITE: usize = 1;
const PAL_ICE: usize = 2;
const PAL_HARD: usize = 3;
const PAL_AFTERLIFE: usize = 4;
const PAL_GREEN_BLUE: usize = 5;
const PAL_SUNSET: usize = 6;
const PAL_POP: usize = 7;

/// A « Festival » look: `generator` in tempo, in festival palette
/// `palette` with colour mode `mode`; `tune` sets the other parameters.
#[allow(clippy::too_many_arguments)]
fn fest(generator: &str, count: u32, a: f32, b: f32, scale: f32, palette: usize, mode: ColorMode, tune: impl FnOnce(&mut GenParams)) -> Settings {
    let pal = crate::festival::PALETTES[palette];
    let mut params = GenParams { count, a, b, color_mode: mode, color2: pal.c2, beat_sync: true, ..GenParams::default() };
    tune(&mut params);
    Settings { content: Content::Generator { generator: generator.to_string(), params }, color: pal.c1, scale, ..Settings::default() }
}

/// The « Festival » page (T-110): 40 cues from the festival generators
/// (T-102 to T-107), named by what they are for in a track. Starting
/// values from the research (docs/research/festival-looks.md, section 4):
/// sweeps of ±0.25 to ±0.45 over 1 bar (drop) to 4 bars (break), 8 to 12
/// beams, rotations of 1/64 (glacial) to 1/4 (fast) turn per beat, chase
/// steps of 1/2 beat (groove) to 1/4 beat (build), 0.2-beat stabs and a
/// 1/2-beat strobe at most (the sustained-safe rate). Our own looks, with
/// our own names: nothing taken from another product's library.
fn festival_cues() -> Vec<(CueTag, &'static str, Settings)> {
    use crate::beat::{Easing, GroupMode, LoopMode};
    use ColorMode::*;
    use CueTag::*;
    let none = |_: &mut GenParams| ();
    let period = |beats: f32| move |p: &mut GenParams| p.period_beats = beats;
    let steps = |n: f32| move |p: &mut GenParams| p.steps_per_beat = n;
    vec![
        // Fans and sweeps (10).
        (Intro, "Intro – éventail blanc", fest("fan", 8, 0.0, 0.25, 0.6, PAL_WHITE, Solid, none)),
        (Drop, "Drop – éventail blanc qui pompe", fest("fan", 10, 1.0, 0.25, 0.7, PAL_WHITE, Solid, none)),
        (Groove, "Groove – balayage vert / bleu", fest("fan_sweep", 8, 0.35, 0.3, 0.7, PAL_GREEN_BLUE, Alternate, period(8.0))),
        (
            Drop,
            "Drop – balayage miroir rouge",
            fest("fan_sweep", 10, 0.45, 0.3, 0.7, PAL_HARD, Alternate, |p| {
                p.period_beats = 4.0;
                p.groups = 2;
                p.group_mode = GroupMode::Mirror;
            }),
        ),
        (Break, "Break – balayage lent glacier", fest("fan_sweep", 8, 0.25, 0.35, 0.6, PAL_ICE, Gradient, period(16.0))),
        (
            Groove,
            "Groove – vague de balayages pop",
            fest("fan_sweep", 12, 0.3, 0.3, 0.7, PAL_POP, ColorChase, |p| {
                p.period_beats = 8.0;
                p.groups = 4;
                p.group_mode = GroupMode::Offset;
            }),
        ),
        (Build, "Montée – éventail qui se lève", fest("fan_tilt", 10, 0.0, 0.0, 0.7, PAL_SUNSET, ToWhite, period(16.0))),
        (
            Drop,
            "Drop – éventail qui retombe",
            fest("fan_tilt", 10, 0.0, 0.0, 0.7, PAL_GREEN, Solid, |p| {
                p.period_beats = 4.0;
                p.direction = -1;
            }),
        ),
        (Groove, "Groove – vague Afterlife", fest("fan_wave", 12, 0.25, 0.35, 0.7, PAL_AFTERLIFE, Gradient, none)),
        (Drop, "Drop – positions sur le kick", fest("positions", 8, 0.0, 0.3, 0.7, PAL_WHITE, Solid, none)),
        // Chasers and stabs (6).
        (Groove, "Groove – chaser vert", fest("chase_fan", 8, 0.0, 2.0, 0.7, PAL_GREEN, Solid, steps(2.0))),
        (Build, "Montée – chaser aller-retour 1/4", fest("chase_fan", 8, 2.0, 2.0, 0.7, PAL_ICE, Alternate, steps(4.0))),
        (Build, "Montée – remplissage rouge", fest("chase_fan", 8, 7.0, 0.0, 0.7, PAL_HARD, Solid, none)),
        (Drop, "Drop – coups blancs sur le kick", Settings { gate: GateMode::Beat, ..fest("fan", 8, 0.0, 0.3, 0.7, PAL_WHITE, Solid, none) }),
        (
            Drop,
            "Drop – coups rouges qui s'éteignent",
            Settings { gate: GateMode::Beat, gate_decay: true, ..fest("fan", 8, 0.0, 0.3, 0.7, PAL_HARD, Solid, none) },
        ),
        (Build, "Montée – strobe blanc 1/2 temps", Settings { strobe_div: 2.0, ..fest("fan", 12, 0.0, 0.4, 0.7, PAL_WHITE, Solid, none) }),
        // Crossings and the hot beam (4).
        (Drop, "Drop – ciseaux rouge / blanc", fest("scissor", 8, 0.5, 0.3, 0.7, PAL_HARD, Solid, period(2.0))),
        (
            Drop,
            "Drop – ciseaux pop flash",
            fest("scissor", 8, 0.5, 0.3, 0.7, PAL_POP, Solid, |p| {
                p.period_beats = 2.0;
                p.cross_flash = true;
            }),
        ),
        (Break, "Break – faisceau chaud blanc", fest("hot_beam", 1, 0.3, 0.6, 0.7, PAL_WHITE, Solid, period(16.0))),
        (Drop, "Drop – faisceaux qui explosent", fest("bundle_burst", 10, 0.25, 0.35, 0.7, PAL_ICE, Solid, period(4.0))),
        // Tunnels and suns (8).
        (Break, "Break – tunnel de doigts glacier", fest("finger_tunnel", 12, 1.0 / 32.0, 0.0, 0.35, PAL_ICE, Alternate, none)),
        (Drop, "Drop – tunnel de doigts rapide", fest("finger_tunnel", 12, 0.25, 0.0, 0.4, PAL_GREEN, Solid, none)),
        (Groove, "Groove – tunnel de doigts pop", fest("finger_tunnel", 10, 1.0 / 16.0, 0.0, 0.4, PAL_POP, Alternate, none)),
        (Build, "Montée – tunnel qui se ferme", fest("tunnel_pump", 1, 0.0, 1.0, 0.45, PAL_ICE, ToWhite, period(16.0))),
        (Drop, "Drop – tunnel qui pompe", fest("tunnel_pump", 1, 0.3, 0.0, 0.35, PAL_HARD, Solid, none)),
        (Groove, "Groove – double tunnel Afterlife", fest("twin_tunnel", 1, 1.0 / 8.0, 0.0, 0.4, PAL_AFTERLIFE, Alternate, none)),
        (Intro, "Intro – soleil levant", fest("sunburst", 16, 1.0 / 64.0, 0.0, 0.8, PAL_SUNSET, Gradient, none)),
        (Drop, "Drop – soleil pair / impair", fest("sunburst", 18, 1.0 / 16.0, 1.0, 0.8, PAL_WHITE, Solid, none)),
        // Sheets (8).
        (Break, "Break – plafond bleu", fest("ceiling", 1, 0.02, 0.15, 0.9, PAL_AFTERLIFE, Gradient, none)),
        (Intro, "Intro – plafond vert", fest("ceiling", 1, 0.01, 0.1, 0.9, PAL_GREEN, Solid, none)),
        (Break, "Break – aurore vert / bleu", fest("aurora", 1, 0.2, 0.35, 0.8, PAL_GREEN_BLUE, Gradient, none)),
        (
            Groove,
            "Groove – lame qui bascule",
            fest("blade", 1, 0.0, 0.3, 0.8, PAL_ICE, Solid, |p| {
                p.period_beats = 16.0;
                p.easing = Easing::Sine;
            }),
        ),
        (Intro, "Intro – rideaux blancs", fest("curtain", 4, 0.0, 0.1, 0.7, PAL_WHITE, Solid, period(16.0))),
        (Break, "Break – cascade glacier", fest("waterfall", 6, 0.0, 0.1, 0.8, PAL_ICE, Gradient, steps(2.0))),
        (
            Build,
            "Montée – scanner qui balaie",
            fest("scanner", 1, 0.2, 0.0, 0.8, PAL_HARD, Solid, |p| {
                p.period_beats = 4.0;
                p.loop_mode = LoopMode::PingPong;
            }),
        ),
        (Groove, "Groove – lamelles vert / bleu", fest("slats", 10, 1.0, 0.3, 0.8, PAL_GREEN_BLUE, Alternate, steps(2.0))),
        // Textures (4).
        (Break, "Break – ciel étoilé", fest("starfield", 6, 0.0, 0.3, 0.7, PAL_WHITE, Solid, none)),
        (Build, "Montée – étoiles qui s'agitent", fest("starfield", 10, 0.0, 0.1, 0.7, PAL_ICE, Solid, steps(4.0))),
        (Drop, "Drop – éclairs blancs", fest("lightning", 8, 0.0, 0.1, 0.7, PAL_WHITE, Solid, none)),
        (Groove, "Groove – faisceaux épais Afterlife", fest("fat_beams", 5, 0.02, 0.3, 0.7, PAL_AFTERLIFE, Alternate, period(8.0))),
    ]
}

/// A beat-synced generator look.
fn synced_look(generator: &str, count: u32, a: f32, b: f32, color: [u8; 3], scale: f32) -> Settings {
    let params = GenParams { count, a, b, beat_sync: true, ..GenParams::default() };
    Settings { content: Content::Generator { generator: generator.to_string(), params }, color, scale, ..Settings::default() }
}

/// A curve: (beat, value, ease) keys.
fn lane(target: &str, keys: &[(f32, f32, KeyEase)]) -> Lane {
    Lane { target: target.to_string(), keys: keys.iter().map(|&(beat, value, ease)| Key { beat, value, ease }).collect() }
}

/// The three example cue programs (T-157).
fn program_cues() -> Vec<(&'static str, &'static str, Settings)> {
    use KeyEase::*;
    // A tunnel with a cone of dots breathing inside it (an inner layer
    // with its own keys), spinning up over 8 beats and back down.
    let cone = |at_beats: f32, scale: f32| EvolvingKey {
        at_beats,
        generator: "beam_circle".into(),
        params: GenParams { count: 8, ..GenParams::default() },
        color: MAGENTA,
        scale,
        ease: Smooth,
        ..EvolvingKey::default()
    };
    let tunnel = Settings {
        program: Some(CueProgram {
            length_beats: Some(16.0),
            play: PlayMode::Loop,
            layers: vec![Content::Evolving(EvolvingCue { length_beats: 16.0, looped: true, keys: vec![cone(0.0, 0.25), cone(8.0, 0.45)], ..EvolvingCue::default() })],
            lanes: vec![lane("rot_z.speed", &[(0.0, 0.0, EaseIn), (8.0, 120.0, EaseOut), (16.0, 0.0, Linear)])],
            lfos: vec![Modulator { target: "size".into(), wave: Wave::Sine, rate: Rate::Beats(4.0), depth: 0.05, ..Modulator::default() }],
        }),
        ..synced_look("tunnel", 6, 0.0, 0.0, CYAN, 0.7)
    };
    // A sheet that fades in low and rises over 16 beats, then holds.
    let sheet = Settings {
        program: Some(CueProgram {
            length_beats: Some(16.0),
            play: PlayMode::Once,
            lanes: vec![
                lane("pos_y", &[(0.0, -0.4, Smooth), (16.0, 0.1, Linear)]),
                lane("size", &[(0.0, 0.6, Linear), (16.0, 1.0, Linear)]),
                lane("brightness", &[(0.0, 0.3, EaseIn), (4.0, 1.0, Linear)]),
            ],
            ..CueProgram::default()
        }),
        ..synced_look("liquid_sky", 1, 0.0, 0.6, GREEN, 0.8)
    };
    // A helix whose hue goes green → magenta and back, with a 1/2-beat
    // strobe on the turn-around and a slow 3D sway.
    let helix = Settings {
        program: Some(CueProgram {
            length_beats: Some(8.0),
            play: PlayMode::PingPong,
            lanes: vec![lane("color.hue", &[(0.0, 120.0, Linear), (8.0, 300.0, Linear)]), lane("strobe.div", &[(0.0, 0.0, Step), (6.0, 2.0, Step)])],
            lfos: vec![Modulator { target: "rot_y.angle".into(), wave: Wave::Triangle, rate: Rate::Beats(8.0), depth: 0.15, ..Modulator::default() }],
            ..CueProgram::default()
        }),
        ..synced_look("helix", 2, 1.5, 0.0, GREEN, 0.7)
    };
    vec![
        ("Tunnels", "Tunnel + cône qui tourne · programme", tunnel),
        ("Balayages", "Nappe qui monte · une fois", sheet),
        ("Vagues", "Hélice teinte + strobe · aller-retour", helix),
    ]
}

/// Beats before a jump where `keys_at` puts a held key.
const JUMP: f32 = 1.0 / 64.0;

/// An evolving cue's keys, generated from `look(b)` (the look at beat `b`;
/// its `at_beats` is set here) at the beats of `at` and `jumps`. Values
/// ease from one key to the next, as the keys say; on a beat of `jumps`
/// they change at once instead, through a key held (« Palier ») from
/// `JUMP` before it. A curve the eases can't draw is keyed every beat or
/// less, with no special case in the engine.
fn keys_at(at: impl IntoIterator<Item = f32>, jumps: &[f32], look: impl Fn(f32) -> EvolvingKey) -> Vec<EvolvingKey> {
    let mut times: Vec<f32> = at.into_iter().chain(jumps.iter().copied()).collect();
    times.sort_by(f32::total_cmp);
    times.dedup();
    let mut keys = Vec::with_capacity(times.len() + jumps.len());
    for t in times {
        if t > JUMP && jumps.contains(&t) {
            keys.push(EvolvingKey { at_beats: t - JUMP, ease: KeyEase::Step, ..look(t - JUMP) });
        }
        keys.push(EvolvingKey { at_beats: t, ..look(t) });
    }
    keys
}

/// Beats `from`, `from + step`… up to `to` included.
fn every(from: f32, to: f32, step: f32) -> impl Iterator<Item = f32> {
    let n = ((to - from) / step).round() as usize;
    (0..=n).map(move |i| from + i as f32 * step)
}

/// Turns done by beat `b`, from beat 0, at `omega(b)` turns per beat.
fn turns(b: f32, omega: impl Fn(f32) -> f32) -> f32 {
    let n = (b * 64.0).ceil().max(1.0) as usize;
    let h = b / n as f32;
    (0..n).map(|i| omega((i as f32 + 0.5) * h)).sum::<f32>() * h
}

/// The tunnels' and the sun's `a` (their angle is `a` × beat) that puts
/// the look at `theta` turns at beat `b`. Keyed every beat or less, it
/// lets a rotation speed up, slow down or turn back without a jump.
fn spin_at(theta: f32, b: f32) -> f32 {
    if b > 0.0 { (theta / b).clamp(0.0, crate::tunnels::MAX_TURNS) } else { 0.0 }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(x: f32) -> f32 {
    crate::beat::smoothstep(x)
}

/// `a` → `b`, `k` of the way (0..1).
fn mix(a: [u8; 3], b: [u8; 3], k: f32) -> [u8; 3] {
    std::array::from_fn(|i| lerp(a[i] as f32, b[i] as f32, k.clamp(0.0, 1.0)).round() as u8)
}

/// `c` with its saturation taken down by `k` (1 = white).
fn to_white(c: [u8; 3], k: f32) -> [u8; 3] {
    let (r, g, b) = crate::generators::toward_white((c[0] as f32, c[1] as f32, c[2] as f32), k);
    [r, g, b].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// `c` with its hue turned by `degrees` (a rotation about the grey axis).
fn hue_turn(c: [u8; 3], degrees: f32) -> [u8; 3] {
    let (s, co) = degrees.to_radians().sin_cos();
    let k = (1.0 - co) / 3.0;
    let r = s / 3f32.sqrt();
    let m = [[co + k, k - r, k + r], [k + r, co + k, k - r], [k - r, k + r, co + k]];
    let v = c.map(|x| x as f32);
    std::array::from_fn(|i| (m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]).round().clamp(0.0, 255.0) as u8)
}

/// Beat-locked parameters (an evolving cue plays in beats anyway).
fn synced() -> GenParams {
    GenParams { beat_sync: true, ..GenParams::default() }
}

/// A key drawing `generator` in `color`, at size `scale`.
fn key(generator: &str, params: GenParams, color: [u8; 3], scale: f32) -> EvolvingKey {
    EvolvingKey { generator: generator.to_string(), params, color, scale, ..EvolvingKey::default() }
}

fn evolving(length_beats: f32, looped: bool, keys: Vec<EvolvingKey>) -> Content {
    Content::Evolving(EvolvingCue { length_beats, looped, launch: crate::evolving::Launch::Beat, keys })
}

/// An evolving cue on its page: « <moment> – <name> · <n> temps ».
fn evolving_cue(tag: CueTag, name: &str, content: Content, program: Option<CueProgram>) -> (CueTag, String, Settings) {
    let beats = match &content {
        Content::Evolving(cue) => cue.length_beats,
        _ => 0.0,
    };
    let moment = match tag {
        CueTag::Intro => "Intro",
        CueTag::Groove => "Groove",
        CueTag::Break => "Break",
        CueTag::Build => "Montée",
        CueTag::Drop => "Drop",
    };
    (tag, format!("{moment} – {name} · {beats} temps"), Settings { content, program, ..Settings::default() })
}

/// The twelve evolving cues of the « Festival évolutifs » page (T-112 to
/// T-123, docs/research/festival-looks.md section 5), in order E1 to E12.
/// Our own looks, written as maths over the beat with our generators:
/// nothing taken from another product's library.
fn evolving_cues() -> Vec<(CueTag, String, Settings)> {
    vec![
        evolving_e1(),
        evolving_e2(),
        evolving_e3(),
        evolving_e4(),
        evolving_e5(),
        evolving_e6(),
        evolving_e7(),
        evolving_e8(),
        evolving_e9(),
        evolving_e10(),
        evolving_e11(),
        evolving_e12(),
    ]
}

/// The chase shape of the first 1/64 beat of a chase cue (E1, E7): the
/// frame an evolving cue holds while it waits for its beat (T-111) is its
/// key 0, and a lone beam held still is blanked by the strict static-beam
/// guard - it would then blink against a flash of the same cue. The fan's
/// two outer beams (« bords → centre ») instead, for 8 ms at 128 BPM.
const LAUNCH_SHAPE: f32 = 4.0;

/// E1, 16 beats, one shot: a fill chase of 10 beams that widens, rises,
/// speeds up (steps of 1, 1/2 then 1/4 beat) and goes to white, then half
/// a beat of black. The chase fan has a fixed height: the cue's own
/// `pos_y` curve lifts it.
fn evolving_e1() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_SUNSET];
    let look = |b: f32| {
        let steps = if b < 8.0 { 1.0 } else if b < 12.0 { 2.0 } else { 4.0 };
        let launch = b < JUMP;
        let params = GenParams { count: 10, a: if launch { LAUNCH_SHAPE } else { 7.0 }, b: 0.0, steps_per_beat: steps, ..synced() };
        EvolvingKey {
            brightness: if (15.5..16.0).contains(&b) { 0.0 } else { 1.0 },
            ease: if launch { KeyEase::Step } else { KeyEase::Linear },
            ..key("chase_fan", params, to_white(pal.c1, smooth((b - 8.0) / 8.0)), lerp(0.15, 0.7, smooth(b / 16.0)))
        }
    };
    let keys = keys_at(every(0.0, 16.0, 1.0).chain([JUMP, 15.5]), &[15.5, 16.0], look);
    let program = CueProgram {
        play: PlayMode::Once,
        lanes: vec![lane("pos_y", &[(0.0, e1_lift(E1_LOW), KeyEase::EaseIn), (16.0, e1_lift(E1_HIGH), KeyEase::Linear)])],
        ..CueProgram::default()
    };
    evolving_cue(CueTag::Build, "éventail qui monte", evolving(16.0, false, keys), Some(program))
}

/// E1's fan height above the horizon, at the start and at the end.
const E1_LOW: f32 = 0.05;
const E1_HIGH: f32 = 0.6;

/// The `pos_y` that puts the chase fan (drawn at `CHASE_HEIGHT`) `y` above
/// the horizon.
fn e1_lift(y: f32) -> f32 {
    crate::fans::HORIZON + y - crate::chases::CHASE_HEIGHT
}

/// E2's pump depth: radius 0.35 at rest, 0.45 on the kick.
const E2_PUMP: f32 = 0.1 / 0.35;

/// E2, 32 beats, one shot: 16 beats of build (the tunnel closes to a
/// point and spins up, strobes from beat 12, half a beat of black), then
/// the drop (wide, pumping on every kick, turning back every 4 beats, in
/// colour 2). A plain circle shows no rotation: 8 beams on it (an inner
/// layer) carry the spin.
fn evolving_e2() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_ICE];
    let radius = |b: f32| if b < 16.0 { lerp(0.35, 0.04, smooth(b / 16.0)) } else { 0.35 };
    let colour = |b: f32| if b < 16.0 { pal.c1 } else { pal.c2 };
    let tunnel = keys_at(every(0.0, 16.0, 1.0), &[16.0], |b| {
        let a = if b < 16.0 { 0.0 } else { E2_PUMP };
        key("tunnel_pump", GenParams { count: 1, a, b: 0.0, ..synced() }, colour(b), radius(b))
    });
    let beams = keys_at(every(0.0, 32.0, 1.0), &[16.0], |b| {
        key("finger_tunnel", GenParams { count: 8, a: spin_at(turns(b, e2_omega), b), ..synced() }, colour(b), radius(b))
    });
    use KeyEase::Step;
    let program = CueProgram {
        play: PlayMode::Once,
        layers: vec![evolving(32.0, false, beams)],
        lanes: vec![
            lane("brightness", &[(0.0, 1.0, Step), (15.5, 0.0, Step), (16.0, 1.0, Step)]),
            lane("strobe.div", &[(0.0, 0.0, Step), (12.0, 2.0, Step), (14.0, 4.0, Step), (15.5, 0.0, Step)]),
        ],
        ..CueProgram::default()
    };
    evolving_cue(CueTag::Build, "tunnel qui zoome sur le drop", evolving(32.0, false, tunnel), Some(program))
}

/// E2's rotation in turns per beat: 1/32 → 1/4 over the build, then 1/4
/// turning back every 4 beats.
fn e2_omega(b: f32) -> f32 {
    if b < 16.0 {
        lerp(1.0 / 32.0, 0.25, b / 16.0)
    } else if ((b - 16.0) / 4.0).floor() as i32 % 2 == 0 {
        0.25
    } else {
        -0.25
    }
}

/// E3, 32 beats, loop: scissors of 2 × 5 beams tilted ±15°, crossing every
/// 2 beats, twice as fast on beats 4 to 8 of every 8, both groups white
/// for the first 1/8 of every beat.
fn evolving_e3() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_POP];
    let look = |b: f32| {
        let period = if b.rem_euclid(8.0) < 4.0 { 2.0 } else { 1.0 };
        let flash = b.rem_euclid(1.0) < 1.0 / 8.0;
        let (c1, c2) = if flash { (WHITE, WHITE) } else { (pal.c1, pal.c2) };
        let params = GenParams { count: 10, a: 0.45, b: 0.3, period_beats: period, color2: c2, ..synced() };
        EvolvingKey { ease: KeyEase::Step, ..key("scissor", params, c1, 0.7) }
    };
    let keys = keys_at(every(0.0, 31.0, 1.0).flat_map(|k| [k, k + 0.125]), &[], look);
    evolving_cue(CueTag::Drop, "ciseaux croisés", evolving(32.0, true, keys), None)
}

/// E4, 16 beats, loop: four positions (wide fan high, narrow fans tilted
/// either way, a V), one per beat lit 0.2 beat; from beat 8 one per half
/// beat lit 0.15 beat. Colour 1 and colour 2 take turns every bar.
fn evolving_e4() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_HARD];
    let look = |b: f32| {
        let (step, gate) = if b < 8.0 { (1.0, 0.2) } else { (0.5, 0.15) };
        let lit = b.rem_euclid(step) < gate - 1e-4;
        let color = if (b / 4.0).floor() as i32 % 2 == 0 { pal.c1 } else { pal.c2 };
        let params = GenParams { count: 8, a: 0.0, b: 0.3, steps_per_beat: 1.0 / step, ..synced() };
        EvolvingKey { brightness: if lit { 1.0 } else { 0.0 }, ease: KeyEase::Step, ..key("positions", params, color, 0.7) }
    };
    let on_beats = every(0.0, 7.0, 1.0).flat_map(|k| [k, k + 0.2]);
    let halves = every(8.0, 15.5, 0.5).flat_map(|k| [k, k + 0.15]);
    let keys = keys_at(on_beats.chain(halves), &[], look);
    evolving_cue(CueTag::Drop, "coups sur 4 positions", evolving(16.0, true, keys), None)
}

/// E5, 32 beats, loop: the ceiling (liquid sky v2) comes down from 0.6 to
/// 0.08 above the horizon, its ripple growing, in a colour 1 → colour 2
/// gradient whose hue drifts ±15°, breathing over 8 beats (the ceiling's
/// own breath); on the last half beat it is back up, ready to loop.
fn evolving_e5() -> (CueTag, String, Settings) {
    let look = |b: f32| {
        let b = if b >= 31.5 { 0.0 } else { b };
        let u = smooth(b / 32.0);
        let drift = 15.0 * (std::f32::consts::TAU * b / 32.0).sin();
        let params = GenParams { count: 1, a: 0.01 + 0.02 * u, b: lerp(0.6, 0.08, u), color_mode: ColorMode::Gradient, color2: hue_turn(BLUE, drift), ..synced() };
        key("ceiling", params, hue_turn(CYAN, drift), 0.8)
    };
    let keys = keys_at(every(0.0, 31.0, 1.0).chain([31.5]), &[31.5], look);
    evolving_cue(CueTag::Break, "plafond qui descend", evolving(32.0, true, keys), None)
}

/// E6's amber.
const SUN_AMBER: [u8; 3] = [255, 140, 0];

/// E6, 32 beats, one shot: a 16-ray sun over the horizon, its rays coming
/// in by pairs (one pair a beat, all 16 by beat 8); from beat 16 it turns
/// 1/64 of a turn a beat and its odd and even rays take turns at 100 % and
/// 40 %. Amber to white along the rays, all white by beat 31.
fn evolving_e6() -> (CueTag, String, Settings) {
    let look = |b: f32| {
        let count = if b < 8.0 { 2 * (b.floor() as u32 + 1) } else { 16 };
        let (a, odd_even) = if b < 16.0 { (0.0, 0.0) } else { (spin_at((b - 16.0) / 64.0, b), 1.0) };
        let params = GenParams { count, a, b: odd_even, color_mode: ColorMode::Gradient, color2: WHITE, ..synced() };
        key("sunburst", params, mix(SUN_AMBER, WHITE, smooth((b - 16.0) / 15.0)), 0.85)
    };
    let keys = keys_at(every(0.0, 32.0, 1.0), &[16.0], look);
    evolving_cue(CueTag::Intro, "lever de soleil", evolving(32.0, false, keys), None)
}

/// E7, 32 beats, one shot: a bouncing chase of 8 beams with a 2-beam tail
/// in colour 2, a step every 1, 1/2, 1/4 then 1/8 beat; from beat 28 the
/// whole fan strobes at 1/4 beat (3.5 beats: a burst well under the
/// limiter's 5 s), then half a beat of black.
fn evolving_e7() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_GREEN_BLUE];
    let chase = |at_beats: f32, steps: f32| {
        let params = GenParams { count: 8, a: 2.0, b: 2.0, steps_per_beat: steps, color_mode: ColorMode::Alternate, color2: pal.c2, ..synced() };
        EvolvingKey { at_beats, ease: KeyEase::Step, ..key("chase_fan", params, pal.c1, 0.7) }
    };
    // The chase's own line: 8 beams at its height, all lit.
    let fan = |at_beats: f32, brightness: f32| {
        let params = GenParams { count: 8, a: 0.0, b: crate::chases::CHASE_HEIGHT - crate::fans::HORIZON, color2: pal.c2, ..synced() };
        EvolvingKey { at_beats, brightness, strobe_div: 4.0, ease: KeyEase::Step, ..key("fan", params, pal.c1, 0.7) }
    };
    let launch = EvolvingKey { params: GenParams { a: LAUNCH_SHAPE, ..chase(0.0, 1.0).params }, ..chase(0.0, 1.0) };
    let keys = vec![launch, chase(JUMP, 1.0), chase(8.0, 2.0), chase(16.0, 4.0), chase(24.0, 8.0), fan(28.0, 1.0), fan(31.5, 0.0)];
    evolving_cue(CueTag::Build, "chaser qui accélère", evolving(32.0, false, keys), None)
}

/// E8's radius pump: +25 % on the kick, back over half a beat.
const E8_PUMP: f32 = 1.25;

/// E8, 32 beats, loop: a 12-beam finger tunnel turning 1/4 turn a beat,
/// the other way every 8 beats through half a beat of easing, its odd and
/// even beams in colours 1 and 2, swapped on every turn-around. The pump
/// on every kick is the cue's own size curve, kept centred on the tunnel
/// by a matching `pos_y` curve.
fn evolving_e8() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_AFTERLIFE];
    let look = |b: f32| {
        let swapped = (b / 8.0).floor() as i32 % 2 == 1;
        let (c1, c2) = if swapped { (pal.c2, pal.c1) } else { (pal.c1, pal.c2) };
        let params = GenParams { count: 12, a: spin_at(turns(b, e8_omega), b), color_mode: ColorMode::Alternate, color2: c2, ..synced() };
        key("finger_tunnel", params, c1, 0.3)
    };
    let turnarounds = [8.0f32, 16.0, 24.0].into_iter().flat_map(|c| [c - 0.25, c - 0.125, c + 0.125, c + 0.25]);
    let keys = keys_at(every(0.0, 32.0, 1.0).chain(turnarounds).chain([0.125, 0.25, 31.75, 31.875]), &[8.0, 16.0, 24.0, 32.0], look);
    let pump = |rest: f32, kick: f32| -> Vec<(f32, f32, KeyEase)> {
        (0..32).flat_map(|k| [(k as f32, kick, KeyEase::EaseOut), (k as f32 + 0.5, rest, KeyEase::Step)]).collect()
    };
    let centre = |size: f32| crate::tunnels::TUNNEL_CY * (1.0 - size);
    let program = CueProgram {
        play: PlayMode::Loop,
        lanes: vec![lane("size", &pump(1.0, E8_PUMP)), lane("pos_y", &pump(centre(1.0), centre(E8_PUMP)))],
        ..CueProgram::default()
    };
    evolving_cue(CueTag::Drop, "rayons tournants inversés", evolving(32.0, true, keys), Some(program))
}

/// E8's rotation in turns per beat: ±1/4, through 0 over the half beat
/// around every 8th beat (forwards on beats 0 to 8, backwards on 8 to 16…).
fn e8_omega(b: f32) -> f32 {
    let c = (b / 8.0).round() * 8.0;
    let after = if (c / 8.0) as i32 % 2 == 0 { 0.25 } else { -0.25 };
    let d = b - c;
    if d.abs() < 0.25 {
        after * (2.0 * smooth((d + 0.25) / 0.5) - 1.0)
    } else if d > 0.0 {
        after
    } else {
        -after
    }
}

/// E9, 16 beats, loop: a wave through 14 beams, half a wavelength a beat,
/// its height growing 0.05 → 0.35 over 8 beats; at beat 8 it turns back
/// and a colour-2 front runs through the beams (a beam every 1/4 beat);
/// from beat 12 it calms down to 0.05 again for the loop.
fn evolving_e9() -> (CueTag, String, Settings) {
    let (c1, c2) = (crate::festival::PALETTES[PAL_SUNSET].c1, crate::festival::PALETTES[PAL_POP].c1);
    let wave = |at_beats: f32, a: f32, direction: i8, color_mode: ColorMode, steps: f32| {
        let params = GenParams { count: 14, a, b: 0.4, direction, color_mode, steps_per_beat: steps, color2: c2, ..synced() };
        EvolvingKey { at_beats, ..key("fan_wave", params, c1, 0.7) }
    };
    let keys = vec![
        wave(0.0, 0.05, 1, ColorMode::Solid, 1.0),
        wave(8.0, 0.35, -1, ColorMode::ColorChase, 4.0),
        wave(12.0, 0.35, -1, ColorMode::ColorChase, 4.0),
    ];
    evolving_cue(CueTag::Break, "vague", evolving(16.0, true, keys), None)
}

/// E10, 32 beats, one shot: four curtains (x = ±0.2, ±0.6, from the
/// horizon to 0.6 above it) come in one every 4 beats, then close in
/// towards the centre, lit half of every beat, and merge into one line at
/// beat 31. They come in through the cue's « Points visibles » curve, in
/// the curtains' drawing order (left to right).
fn evolving_e10() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_ICE];
    let look = |b: f32| {
        let closing = b >= 16.0;
        let squeeze = if closing { 1.0 - 0.5 * smooth((b - 16.0) / 16.0) } else { 1.0 };
        let params = GenParams { count: if b >= 31.0 { 1 } else { 4 }, a: 0.0, b: 0.0, ..synced() };
        EvolvingKey {
            gate_beats: if closing { 0.5 } else { 0.0 },
            ease: if closing { KeyEase::Linear } else { KeyEase::Step },
            ..key("curtain", params, pal.c1, E10_SIZE * squeeze)
        }
    };
    let keys = keys_at(std::iter::once(0.0).chain(every(16.0, 31.0, 1.0)), &[16.0], look);
    use KeyEase::Step;
    let program = CueProgram {
        play: PlayMode::Once,
        lanes: vec![lane("trace", &[(0.0, 0.25, Step), (4.0, 0.5, Step), (8.0, 0.75, Step), (12.0, 1.0, Step)])],
        ..CueProgram::default()
    };
    evolving_cue(CueTag::Intro, "couloir de rideaux", evolving(32.0, false, keys), Some(program))
}

/// E10's size: four curtains 0.4 apart, at x = ±0.2 and ±0.6.
const E10_SIZE: f32 = 0.8;

/// E11's meeting point, above the horizon.
const E11_POINT: f32 = 0.3;
/// E11's bundle size at its tightest.
const E11_TIGHT: f32 = 0.05;

/// E11, 16 beats, one shot: a starry sky that thickens (3 → 12 stars; the
/// starfield draws 4 at least, for the static-beam guard) and quickens (a
/// new sky every beat → every 1/4 beat) over 12 beats; then its stars
/// close in on one point (the cue's size and `pos_y` curves), one hot
/// white beam for half a beat, and black.
fn evolving_e11() -> (CueTag, String, Settings) {
    let look = |b: f32| {
        let u = (b / 12.0).clamp(0.0, 1.0);
        let params = GenParams { count: lerp(3.0, 12.0, u).round() as u32, a: 0.0, b: 0.1, steps_per_beat: 1.0 / lerp(1.0, 0.25, u), ..synced() };
        EvolvingKey { ease: KeyEase::Step, ..key("starfield", params, WHITE, 0.7) }
    };
    let mut keys = keys_at(every(0.0, 14.0, 1.0), &[], look);
    let hot = |at_beats: f32, brightness: f32| {
        let params = GenParams { count: 1, a: 0.0, b: crate::crossings::HOT_DEFAULT_CAP, period_beats: 16.0, ..synced() };
        EvolvingKey { at_beats, brightness, ease: KeyEase::Step, ..key("hot_beam", params, WHITE, 0.7) }
    };
    keys.extend([hot(15.0, 1.0), hot(15.5, 0.0)]);
    // Positions → (0, E11_POINT): size s about the origin, then up by
    // E11_POINT × (1 - s), both linear over the same beats.
    use KeyEase::{Linear, Step};
    let tight = 15.0 - JUMP;
    let program = CueProgram {
        play: PlayMode::Once,
        lanes: vec![
            lane("size", &[(12.0, 1.0, Linear), (tight, E11_TIGHT, Step), (15.0, 1.0, Step)]),
            lane("pos_y", &[(12.0, 0.0, Linear), (tight, E11_POINT * (1.0 - E11_TIGHT), Step), (15.0, 0.0, Step)]),
        ],
        ..CueProgram::default()
    };
    evolving_cue(CueTag::Build, "étoiles puis éclatement", evolving(16.0, false, keys), Some(program))
}

/// E12, 32 beats, one shot: a grid of 5 + 5 lines in the upper field,
/// revealed a line a beat (rows and columns in turn), tightening to 0.4 of
/// its size from beat 16 to 28 as it scrolls a line a beat, strobing at
/// 1/4 beat for the last bar (a 4-beat burst, under the limiter's 5 s),
/// then a single ceiling.
fn evolving_e12() -> (CueTag, String, Settings) {
    let pal = crate::festival::PALETTES[PAL_GREEN];
    let grid = |at_beats: f32, lines: u32, scale: f32, strobe_div: f32, ease: KeyEase| {
        let params = GenParams { count: lines.div_ceil(2).max(1), a: (lines / 2) as f32, b: 0.1, ..synced() };
        EvolvingKey { at_beats, strobe_div, ease, ..key("grid", params, pal.c1, scale) }
    };
    let mut keys: Vec<EvolvingKey> = (0..10).map(|k| grid(k as f32, k + 1, E12_SIZE, 0.0, KeyEase::Step)).collect();
    keys.push(grid(16.0, 10, E12_SIZE, 0.0, KeyEase::Linear));
    keys.push(grid(28.0, 10, E12_SIZE * 0.4, 4.0, KeyEase::Step));
    let sheet = GenParams { count: 1, a: 0.01, b: 0.3, ..synced() };
    keys.push(EvolvingKey { at_beats: 32.0, ease: KeyEase::Step, ..key("ceiling", sheet, pal.c1, 0.8) });
    evolving_cue(CueTag::Build, "grille qui se resserre", evolving(32.0, false, keys), None)
}

/// E12's grid size before it tightens.
const E12_SIZE: f32 = 0.7;

fn slug(category: &str) -> String {
    category
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' => 'e',
            'à' => 'a',
            c if c.is_ascii_alphanumeric() => c.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Most points in a cue thumbnail: enough for the shape, small enough
/// that the whole grid's thumbnails stay a light request.
const THUMB_POINTS: usize = 160;

/// A small still of what a look draws, for the cue grid's buttons: the
/// look rendered half a second in (so rotations and generators have
/// moved), with some music so audio-reactive cues show something, then
/// thinned to at most `THUMB_POINTS` points. Packed as one hex string,
/// 5 bytes per point (x, y as 0..=255 for -1..1, then r, g, b), so the
/// whole grid's pictures stay a light request. Display only: it never
/// reaches the output.
pub fn thumbnail(settings: &Settings) -> String {
    let audio = AudioFeatures { level: 0.6, bass: 0.6, ..AudioFeatures::default() };
    let mut animator = Animator::starting_at(0.0);
    let mut points = Vec::new();
    for i in 1..=30 {
        let clock = BeatClock { beat: i as f64 / 30.0, ..BeatClock::default() };
        points = animator.render(settings, audio, 1.0 / 60.0, &clock);
    }
    pack(&points)
}

/// Points → a thumbnail string (see `thumbnail`).
fn pack(points: &[crate::patterns::Point]) -> String {
    pack_max(points, THUMB_POINTS)
}

/// `pack` thinned to at most `max` points.
pub(crate) fn pack_max(points: &[crate::patterns::Point], max: usize) -> String {
    let step = points.len().div_ceil(max.max(1)).max(1);
    let pos = |v: f32| ((v.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8;
    let col = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    points
        .iter()
        .step_by(step)
        .flat_map(|p| [pos(p.x), pos(p.y), col(p.r), col(p.g), col(p.b)])
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Most pictures in a hover preview.
const PREVIEW_FRAMES: usize = 64;

/// The cue grid's hover preview (T-157): the look played from its launch,
/// one thumbnail every `step_beats`, for the UI to play at the tempo.
#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    pub step_beats: f64,
    pub frames: Vec<String>,
}

/// A look over its program's length (4 beats without a program) at `bpm`.
/// Display only: it never reaches the output.
pub fn preview(settings: &Settings, bpm: f64, beats_per_bar: u8) -> Preview {
    preview_with(settings, bpm, beats_per_bar, PREVIEW_FRAMES, 0.25)
}

/// Most pictures in the cue editor's live preview (T-164): smoother than
/// the grid's hover, still a light request.
pub const EDITOR_PREVIEW_FRAMES: usize = 128;

/// `preview` with at most `max_frames` pictures, `min_step` beats apart or more.
pub fn preview_with(settings: &Settings, bpm: f64, beats_per_bar: u8, max_frames: usize, min_step: f64) -> Preview {
    let bpm = if bpm.is_finite() { bpm.clamp(20.0, 400.0) } else { 120.0 };
    let len = settings.program.as_ref().map_or(4.0, |p| p.length(&settings.content));
    let step = (len / max_frames.max(1) as f64).max(min_step);
    let count = ((len / step).round() as usize).clamp(1, max_frames.max(1));
    let dt = (step * 60.0 / bpm) as f32;
    let audio = AudioFeatures { level: 0.6, bass: 0.6, ..AudioFeatures::default() };
    let mut animator = Animator::starting_at(0.0);
    let frames = (0..count)
        .map(|i| {
            let clock = BeatClock { beat: i as f64 * step, bpm, beats_per_bar };
            pack(&animator.render(settings, audio, if i == 0 { 0.0 } else { dt }, &clock))
        })
        .collect();
    Preview { step_beats: step, frames }
}

/// Pictures in an animated grid thumbnail (T-273): one every quarter beat
/// over `LOOP_BEATS`, each thinned to `LOOP_POINTS` points (a light page:
/// about 19 kB per cue), well under the 300 the task allows.
pub const LOOP_FRAMES: usize = 16;
pub const LOOP_POINTS: usize = 120;
/// One 4/4 bar, the UI loops it at the current tempo, on the beat.
pub const LOOP_BEATS: f64 = 4.0;
/// The tempo the loop is drawn at. Only time-based motion (not beat-based)
/// depends on it, and a thumbnail doesn't need to be exact.
const LOOP_BPM: f64 = 120.0;

/// A cue's animated grid thumbnail (T-273): its first bar from the launch,
/// with its own Animator (the live one is never touched).
/// Display only: it never reaches the output.
pub fn thumb_loop(settings: &Settings) -> Preview {
    let step = LOOP_BEATS / LOOP_FRAMES as f64;
    let dt = (step * 60.0 / LOOP_BPM) as f32;
    let audio = AudioFeatures { level: 0.6, bass: 0.6, ..AudioFeatures::default() };
    let mut animator = Animator::starting_at(0.0);
    let frames = (0..LOOP_FRAMES)
        .map(|i| {
            let clock = BeatClock { beat: i as f64 * step, bpm: LOOP_BPM, beats_per_bar: 4 };
            pack_max(&animator.render(settings, audio, if i == 0 { 0.0 } else { dt }, &clock), LOOP_POINTS)
        })
        .collect();
    Preview { step_beats: step, frames }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures, BeatClock};
    use std::collections::HashSet;

    #[test]
    fn every_cue_has_a_small_visible_thumbnail() {
        for p in catalog() {
            let t = thumbnail(&p.settings);
            assert!(!t.is_empty() && t.len().is_multiple_of(10) && t.len() <= THUMB_POINTS * 10, "{}: {} chars", p.id, t.len());
            let bytes: Vec<u8> = (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap()).collect();
            assert!(bytes.chunks(5).any(|q| q[2..].iter().any(|&c| c > 0)), "{} has a dark thumbnail", p.id);
        }
    }

    #[test]
    fn thumbnail_loops_are_small_lit_and_move() {
        // A sample across the pages (the whole catalogue is slow in debug).
        let all = catalog();
        let mut moving = 0;
        for p in all.iter().step_by(all.len() / 24) {
            let l = thumb_loop(&p.settings);
            assert_eq!((l.frames.len(), l.step_beats), (LOOP_FRAMES, LOOP_BEATS / LOOP_FRAMES as f64), "{}", p.id);
            for f in &l.frames {
                assert!(f.len().is_multiple_of(10) && f.len() / 10 <= LOOP_POINTS && LOOP_POINTS <= 300, "{}: {} points", p.id, f.len() / 10);
            }
            assert!(l.frames.iter().any(|f| !f.is_empty()), "{} has an empty loop", p.id);
            moving += usize::from(l.frames.iter().any(|f| *f != l.frames[0]));
        }
        assert!(moving * 2 > 24, "only {moving} of the sampled loops move");
    }

    #[test]
    fn catalogue_is_large_and_ids_are_unique() {
        let all = catalog();
        assert!(all.len() >= 200, "only {} cues", all.len());
        let ids: HashSet<_> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn every_category_has_cues_and_every_cue_has_a_known_category() {
        let all = catalog();
        for c in CATEGORIES.iter().filter(|c| ![crate::figures::CATEGORY, crate::ilda_library::CATEGORY, crate::wire::CATEGORY].contains(*c)) {
            assert!(all.iter().any(|p| p.category == *c), "empty page '{c}'");
        }
        assert!(all.iter().all(|p| CATEGORIES.contains(&p.category)));
    }

    #[test]
    fn every_cue_renders_something_within_budget() {
        for p in catalog() {
            let mut a = Animator::default();
            let pts = a.render(&p.settings, AudioFeatures { level: 0.5, bass: 0.5, beat: 1, ..Default::default() }, 1.0 / 60.0, &BeatClock::default());
            assert!(pts.iter().any(|q| q.is_lit()), "cue '{}' ({}) is dark", p.name, p.id);
            assert!(pts.len() <= 4000, "cue '{}' makes {} points", p.name, pts.len());
        }
    }

    /// One second (60 frames, the tempo clock and the music moving) of
    /// every cue with the point optimiser on or off: the frames.
    fn second_of(p: &Preset, optimizer: &crate::optimize::OptimizeParams) -> Vec<Vec<crate::patterns::Point>> {
        let mut a = Animator::starting_at(0.0);
        a.set_optimizer(optimizer);
        (0..60u64)
            .map(|i| {
                let clock = BeatClock { beat: i as f64 / 30.0, ..BeatClock::default() };
                let audio = AudioFeatures { level: 0.5, bass: 0.5, beat: i / 15, ..Default::default() };
                a.render(&p.settings, audio, 1.0 / 60.0, &clock)
            })
            .collect()
    }

    /// T-002: with the optimiser on, every cue still draws at 30 frames/s
    /// or more at 30 kpps (≤ 1000 points), stays in range, and its lit
    /// points never jump more than a lit step (stretched at most 2x by the
    /// frame cap).
    #[test]
    fn every_cue_fits_30_fps_with_the_optimizer() {
        let on = crate::optimize::OptimizeParams { enabled: true, ..Default::default() };
        for p in catalog() {
            let mut lit = false;
            for frame in second_of(&p, &on) {
                assert!(frame.len() <= 1000, "cue '{}' makes {} points", p.id, frame.len());
                assert!(frame.iter().all(|q| (-1.0..=1.0).contains(&q.x) && (-1.0..=1.0).contains(&q.y)), "{}", p.id);
                for w in frame.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()) {
                    let d = ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt();
                    assert!(d <= 2.0 * on.lit_step + 1e-4, "cue '{}': lit step {d}", p.id);
                }
                lit |= frame.iter().any(|q| q.is_lit());
            }
            assert!(lit, "cue '{}' is dark", p.id);
        }
    }

    /// Measurement for the PR note (T-002), not a check:
    /// `cargo test --release -p laser-studio optimizer_measurement -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn optimizer_measurement() {
        let off = crate::optimize::OptimizeParams::default();
        let on = crate::optimize::OptimizeParams { enabled: true, ..Default::default() };
        for (name, params) in [("densify", &off), ("optimiser", &on)] {
            let (mut total, mut frames, mut worst, mut worst_id, mut over_750) = (0usize, 0usize, 0usize, String::new(), 0usize);
            let started = std::time::Instant::now();
            for p in catalog() {
                let most = second_of(&p, params).iter().map(|f| f.len()).inspect(|&n| {
                    total += n;
                    frames += 1;
                }).max().unwrap_or(0);
                if most > worst {
                    (worst, worst_id) = (most, p.id.clone());
                }
                over_750 += (most > 750) as usize;
            }
            let per_frame = started.elapsed().as_secs_f64() * 1e6 / frames as f64;
            println!(
                "{name}: {} cues, mean {:.0} points/frame, worst {worst} ({worst_id}, {:.1} fps at 30 kpps), {over_750} cues over 750, {per_frame:.1} µs per rendered frame",
                catalog().len(),
                total as f64 / frames as f64,
                30_000.0 / worst as f64,
            );
        }
        // Stress: the optimiser alone on 64 rings of 61 points (3904 raw
        // points, 64 paths to order), far heavier than any cue.
        let rings: Vec<crate::patterns::Point> = (0..64)
            .flat_map(|i| {
                let (cx, cy, r) = ((i % 8) as f32 * 0.25 - 0.875, (i / 8) as f32 * 0.25 - 0.875, 0.1);
                let mut ring: Vec<_> = (0..=60).map(|k| {
                    let a = k as f32 / 60.0 * std::f32::consts::TAU;
                    crate::patterns::Point::lit(cx + r * a.cos(), cy + r * a.sin(), 1.0, 1.0, 1.0)
                }).collect();
                ring.insert(0, crate::patterns::Point::blanked(cx + r, cy));
                ring
            })
            .collect();
        let mut o = crate::optimize::Optimizer::new(on.clone());
        let started = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..1000 {
            n = o.optimize(&rings).len();
        }
        println!("stress: {} raw points → {n}, {:.1} µs per frame", rings.len(), started.elapsed().as_secs_f64() * 1e3);
    }

    /// FNV-1a over values rounded to 1e-3, so libm last-bit differences
    /// between platforms don't matter but any real change does.
    fn digest(values: impl Iterator<Item = f32>) -> u64 {
        values.fold(0xcbf2_9ce4_8422_2325u64, |h, v| {
            let q = (v * 1000.0).round() as i64 as u64;
            (h ^ q).wrapping_mul(0x0100_0000_01b3)
        })
    }

    #[test]
    fn cue_frames_are_unchanged() {
        // Golden digest of every cue's 30th frame (clock cues excluded: they
        // show the system time), taken before the beat-synced generator work.
        let mut values = Vec::new();
        for p in catalog().into_iter().take(LEGACY_CUES) {
            if matches!(&p.settings.content, Content::Generator { generator, .. } if generator == "clock") {
                continue;
            }
            let mut a = Animator::default();
            let mut pts = Vec::new();
            for i in 0..30u64 {
                pts = a.render(&p.settings, AudioFeatures { level: 0.4, bass: 0.3, beat: i / 10, ..Default::default() }, 1.0 / 60.0, &BeatClock::default());
            }
            values.extend(pts.iter().flat_map(|q| [q.x, q.y, q.r, q.g, q.b]));
        }
        assert_eq!(digest(values.into_iter()), GOLDEN_FRAMES);
    }

    #[test]
    fn cue_ids_are_unchanged() {
        let ids: String = catalog().iter().take(LEGACY_CUES).map(|p| format!("{}|", p.id)).collect();
        assert_eq!(digest(ids.bytes().map(|b| b as f32)), GOLDEN_IDS);
    }

    /// Cues that existed before the festival cues (T-103 on): the golden
    /// digests cover them; later cues are only ever appended.
    const LEGACY_CUES: usize = 202;
    /// The T-103 rhythm cues, right after them.
    const RHYTHM_CUES: usize = 11;
    /// The T-157 cue programs, after those.
    const PROGRAM_CUES: usize = 3;
    /// The T-104 crossings, after those.
    const CROSSING_CUES: usize = 10;
    /// The T-107 textures, after those.
    const TEXTURE_CUES: usize = 9;
    /// The T-110 « Festival » page, after those.
    const FESTIVAL_CUES: usize = 40;

    /// The T-108 graphics, after those.
    const GRAPHIC_CUES: usize = 12;
    /// The T-112 to T-123 « Festival évolutifs » page, after those.
    const EVOLVING_CUES: usize = 12;
    /// The T-440 abstracts (stacked oscillators), after those.
    const ABSTRACT_CUES: usize = 11;

    fn festival() -> Vec<Preset> {
        catalog().into_iter().skip(LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES + CROSSING_CUES + TEXTURE_CUES).take(FESTIVAL_CUES).collect()
    }

    fn generator_of(p: &Preset) -> (&str, &GenParams) {
        match &p.settings.content {
            Content::Generator { generator, params } => (generator.as_str(), params),
            _ => panic!("{} is not a generator", p.id),
        }
    }

    #[test]
    fn festival_cues_are_appended_on_their_own_page() {
        let all = catalog();
        let new = festival();
        assert_eq!(new.len(), FESTIVAL_CUES);
        assert!(new.iter().all(|p| p.category == FESTIVAL && p.settings.program.is_none()));
        assert_eq!((new[0].id.as_str(), new[39].id.as_str()), ("festival-001", "festival-040"));
        // One grid page (8×5) holds them all.
        assert!(new.len() <= crate::controls::GRID_ROWS * crate::controls::GRID_COLS);
        // Only they are tagged, and every one is.
        assert!(new.iter().all(|p| p.tag.is_some()));
        assert!(all.iter().filter(|p| p.category != FESTIVAL && p.category != EVOLVING).all(|p| p.tag.is_none()));
        for tag in [CueTag::Intro, CueTag::Groove, CueTag::Break, CueTag::Build, CueTag::Drop] {
            assert!(new.iter().any(|p| p.tag == Some(tag)), "a {tag:?} cue");
        }
    }

    #[test]
    fn festival_names_are_french_unique_and_say_their_moment() {
        let new = festival();
        let names: HashSet<_> = new.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), new.len(), "unique names");
        for p in &new {
            let word = match p.tag.unwrap() {
                CueTag::Intro => "Intro",
                CueTag::Groove => "Groove",
                CueTag::Break => "Break",
                CueTag::Build => "Montée",
                CueTag::Drop => "Drop",
            };
            assert!(p.name.starts_with(&format!("{word} – ")), "{}", p.name);
        }
    }

    #[test]
    fn festival_cues_follow_the_requested_mix_in_tempo_and_in_palette() {
        let new = festival();
        let mut mix = std::collections::HashMap::new();
        for p in &new {
            let (g, params) = generator_of(p);
            assert!(params.beat_sync, "{} follows the tempo", p.name);
            // Colours from one festival palette (T-130).
            assert!(
                crate::festival::PALETTES.iter().any(|pal| pal.c1 == p.settings.color && pal.c2 == params.color2),
                "{} is in a festival palette",
                p.name
            );
            // Strobes stay at the sustained-safe rate (research §4.4).
            assert!(p.settings.strobe_div <= 2.0);
            let rhythm = p.settings.gate != GateMode::Off || p.settings.strobe_div > 0.0;
            let family = match g {
                "chase_fan" => "chasers",
                _ if rhythm => "chasers",
                "fan" | "fan_sweep" | "fan_tilt" | "fan_wave" | "positions" => "fans",
                "scissor" | "hot_beam" | "bundle_burst" | "beam_v" => "crossings",
                "finger_tunnel" | "tunnel_pump" | "twin_tunnel" | "sunburst" => "tunnels",
                "ceiling" | "blade" | "curtain" | "waterfall" | "scanner" | "slats" | "aurora" | "grid" => "sheets",
                "starfield" | "lightning" | "fat_beams" => "textures",
                other => panic!("{other} is not a festival generator"),
            };
            *mix.entry(family).or_insert(0) += 1;
        }
        let want = [("fans", 10), ("chasers", 6), ("crossings", 4), ("tunnels", 8), ("sheets", 8), ("textures", 4)];
        for (family, n) in want {
            assert_eq!(mix.get(family), Some(&n), "{family}");
        }
    }

    #[test]
    fn every_festival_cue_draws_above_the_horizon() {
        for p in festival() {
            let mut lit = 0;
            let mut a = Animator::starting_at(0.0);
            for i in 0..120u64 {
                let clock = BeatClock { beat: i as f64 / 30.0, ..BeatClock::default() };
                let audio = AudioFeatures { level: 0.5, bass: 0.5, beat: i / 15, ..Default::default() };
                let pts = a.render(&p.settings, audio, 1.0 / 60.0, &clock);
                assert!(!pts.is_empty() && pts.len() <= 1000, "{}: {} points", p.name, pts.len());
                for q in pts.iter().filter(|q| q.is_lit()) {
                    assert!(q.y >= crate::fans::HORIZON - 1e-3, "{} draws at y = {}", p.name, q.y);
                    lit += 1;
                }
            }
            assert!(lit > 0, "{} is dark", p.name);
        }
    }

    #[test]
    fn graphic_cues_are_appended_with_room_on_their_pages() {
        let all = catalog();
        let start = LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES + CROSSING_CUES + TEXTURE_CUES + FESTIVAL_CUES;
        assert_eq!(all.len(), start + GRAPHIC_CUES + EVOLVING_CUES + ABSTRACT_CUES, "appended before the evolving cues");
        let new = &all[start..start + GRAPHIC_CUES];
        assert_eq!((new[0].id.as_str(), new[6].id.as_str()), ("texte-horloge-022", "geometrie-013"));
        for p in new {
            let (g, params) = generator_of(p);
            assert!(params.beat_sync && p.settings.program.is_none() && p.tag.is_none(), "{}", p.id);
            match g {
                "countdown" => assert!(p.category == "Texte & horloge" && [0.25, 1.0].contains(&params.steps_per_beat)),
                "clock" => assert!(p.category == "Texte & horloge" && params.period_beats >= 8.0),
                "wire3d" => assert!(p.category == "Géométrie" && params.period_beats == 8.0, "a turn every 2 bars"),
                other => panic!("{other}"),
            }
            // On the page's first grid page: no new overflow page, so every
            // page keeps its number.
            let index = all.iter().filter(|q| q.category == p.category).position(|q| q.id == p.id).unwrap();
            assert!(index < crate::controls::GRID_ROWS * crate::controls::GRID_COLS, "{}", p.id);
        }
        let solids: HashSet<u32> = new.iter().filter(|p| generator_of(p).0 == "wire3d").map(|p| generator_of(p).1.a as u32).collect();
        assert_eq!(solids.len(), crate::graphics::SOLIDS, "a cue per solid");
        // The countdown at its first bar shows 4 (in the thumbnail too).
        let (_, p) = generator_of(&new[0]);
        assert_eq!(crate::graphics::countdown_value(p, 1.0), Some(4));
    }

    #[test]
    fn texture_cues_are_appended_on_the_beams_page() {
        let all = catalog();
        let start = LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES + CROSSING_CUES;
        let new = &all[start..start + TEXTURE_CUES];
        assert!(new.iter().all(|p| p.category == "Faisceaux" && p.settings.program.is_none()));
        assert_eq!(new[0].id, "faisceaux-053");
        let gens: Vec<(String, GenParams)> = new
            .iter()
            .map(|p| match &p.settings.content {
                Content::Generator { generator, params } => (generator.clone(), params.clone()),
                _ => panic!("{} is not a generator", p.id),
            })
            .collect();
        assert!(gens.iter().all(|(_, p)| p.beat_sync), "they follow the tempo");
        for name in ["starfield", "lightning", "fat_beams"] {
            assert!(gens.iter().any(|(g, _)| g == name), "a cue for {name}");
        }
        // Research values: 4 to 12 stars every 1/4 beat (or every beat when
        // slow), bolts of 6 to 10 segments on every beat, 4 to 6 fat beams
        // of radius 0.01 to 0.03.
        for (g, p) in &gens {
            match g.as_str() {
                "starfield" => assert!((4..=12).contains(&p.count) && [1.0, 4.0].contains(&p.steps_per_beat)),
                "lightning" => assert!((6..=10).contains(&p.count) && p.a < 0.5),
                _ => assert!((4..=6).contains(&p.count) && (0.01..=0.03).contains(&p.a)),
            }
        }
    }

    #[test]
    fn crossing_cues_are_appended_on_the_beams_page() {
        let all = catalog();
        let start = LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES;
        let new = &all[start..start + CROSSING_CUES];
        assert!(new.iter().all(|p| p.category == "Faisceaux" && p.settings.program.is_none()));
        // Numbered after the page's 42 older cues.
        assert_eq!(new[0].id, "faisceaux-043");
        let gens: Vec<(String, GenParams)> = new
            .iter()
            .map(|p| match &p.settings.content {
                Content::Generator { generator, params } => (generator.clone(), params.clone()),
                _ => panic!("{} is not a generator", p.id),
            })
            .collect();
        assert!(gens.iter().all(|(_, p)| p.beat_sync), "they follow the tempo");
        for name in ["scissor", "hot_beam", "bundle_burst", "beam_v"] {
            assert!(gens.iter().any(|(g, _)| g == name), "a cue for {name}");
        }
        assert!(gens.iter().any(|(g, p)| g == "scissor" && p.cross_flash && p.period_beats == 2.0));
        // Hot beams pan 0.3 over 16 beats, capped at 60 %.
        assert!(gens.iter().filter(|(g, _)| g == "hot_beam").all(|(_, p)| p.a == 0.3 && p.b == 0.6 && p.period_beats == 16.0));
        // The page's first grid page is full: they go on « Faisceaux 2 ».
        let index = all.iter().filter(|q| q.category == "Faisceaux").position(|q| q.id == new[0].id).unwrap();
        assert!(index >= crate::controls::GRID_ROWS * crate::controls::GRID_COLS);
        assert!(all[..start].iter().all(|p| !matches!(&p.settings.content, Content::Generator { params, .. } if params.cross_flash)));
    }

    #[test]
    fn program_cues_are_appended_after_the_rhythm_cues() {
        let all = catalog();
        let new = &all[LEGACY_CUES + RHYTHM_CUES..LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES];
        assert!(new.iter().all(|p| p.settings.program.as_ref().is_some_and(|p| p.validate().is_ok())));
        assert_eq!(new.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["tunnels-024", "balayages-017", "vagues-028"]);
        let plays: Vec<PlayMode> = new.iter().map(|p| p.settings.program.as_ref().unwrap().play).collect();
        assert_eq!(plays, [PlayMode::Loop, PlayMode::Once, PlayMode::PingPong]);
        assert!(new.iter().any(|p| !p.settings.program.as_ref().unwrap().layers.is_empty()), "one has an inner layer");
        // Older cues have no program.
        assert!(all[..LEGACY_CUES + RHYTHM_CUES].iter().all(|p| p.settings.program.is_none()));
        // Each fits a grid page, so it has a pad (`grid.<page>.<row>.<col>`).
        for p in new {
            let index = all.iter().filter(|q| q.category == p.category).position(|q| q.id == p.id).unwrap();
            assert!(index < crate::controls::GRID_ROWS * crate::controls::GRID_COLS, "{}", p.id);
        }
    }

    #[test]
    fn a_preview_plays_the_program_over_its_length() {
        let all = catalog();
        let helix = all.iter().find(|p| p.id == "vagues-028").unwrap();
        let pv = preview(&helix.settings, 120.0, 4);
        assert_eq!((pv.step_beats, pv.frames.len()), (0.25, 32), "8 beats, a picture every 1/4 beat");
        assert!(pv.frames.iter().any(|f| f.is_empty() || f.as_bytes().chunks(10).all(|c| &c[4..] == b"000000")), "a dark strobe slot");
        assert!(pv.frames.windows(2).any(|w| w[0] != w[1]), "it moves");
        assert_eq!(preview(&helix.settings, 120.0, 4).frames, pv.frames, "the same every time");
        let plain = preview(&all[0].settings, 120.0, 4);
        assert_eq!(plain.frames.len(), 16, "4 beats without a program");
    }

    #[test]
    fn rhythm_cues_are_appended_on_the_beams_page() {
        let all = catalog();
        let new = &all[LEGACY_CUES..LEGACY_CUES + RHYTHM_CUES];
        assert!(new.iter().all(|p| p.category == "Faisceaux"));
        // Numbered after the page's 31 older cues.
        assert_eq!(new[0].id, "faisceaux-032");
        let gen = |p: &Preset| match &p.settings.content {
            Content::Generator { generator, params } => (generator.clone(), params.beat_sync),
            _ => (String::new(), false),
        };
        assert!(new.iter().all(|p| gen(p).1), "every rhythm cue follows the tempo");
        assert_eq!(new.iter().filter(|p| gen(p).0 == "chase_fan").count(), 7, "one per chase shape but the reversed one");
        assert!(new.iter().any(|p| p.settings.gate == GateMode::Beat && !p.settings.gate_decay));
        assert!(new.iter().any(|p| p.settings.gate_decay));
        assert!(new.iter().any(|p| p.settings.gate == GateMode::BeatAndOffbeat));
        // The only strobe cue stays at the sustained-safe rate (1/2 beat).
        assert!(new.iter().all(|p| p.settings.strobe_div <= 2.0) && new.iter().any(|p| p.settings.strobe_div > 0.0));
        // Older cues have no rhythm.
        assert!(all[..LEGACY_CUES].iter().all(|p| p.settings.gate == GateMode::Off && p.settings.strobe_div == 0.0));
    }

    /// The « Festival évolutifs » cues, E1 to E12.
    fn evolving_page() -> Vec<Preset> {
        catalog().into_iter().filter(|p| p.category == EVOLVING).collect()
    }

    /// Evolving cue E`n` (1-based): its keys and its program.
    fn evo(n: usize) -> (EvolvingCue, Option<CueProgram>) {
        let p = &evolving_page()[n - 1];
        match &p.settings.content {
            Content::Evolving(cue) => (cue.clone(), p.settings.program.clone()),
            _ => panic!("{} is not an evolving cue", p.id),
        }
    }

    fn at(cue: &EvolvingCue, b: f64) -> crate::evolving::Sample {
        cue.sample(cue.local(b)).unwrap()
    }

    /// The program's modifiers of E`n` at beat `b`.
    fn mods(n: usize, b: f64) -> crate::program::Mods {
        let (cue, program) = evo(n);
        let program = program.unwrap();
        program.modifiers(b, program.length(&Content::Evolving(cue)), 128.0).unwrap()
    }

    /// One frame of E`n` at beat `b` of the cue (launched on beat 0).
    fn frame_of(n: usize, b: f64, bpm: f64) -> Vec<crate::patterns::Point> {
        let s = evolving_page()[n - 1].settings.clone();
        Animator::starting_at(0.0).render(&s, AudioFeatures::default(), 0.0, &BeatClock { beat: b, bpm, beats_per_bar: 4 })
    }

    /// Lit places of a frame, rounded to `grid` (a beam is many points).
    fn lit_places(points: &[crate::patterns::Point], grid: f32) -> Vec<(i32, i32)> {
        let mut places: Vec<(i32, i32)> = points.iter().filter(|q| q.is_lit()).map(|q| ((q.x / grid).round() as i32, (q.y / grid).round() as i32)).collect();
        places.sort();
        places.dedup();
        places
    }

    /// Distinct lit x of a frame, rounded to `grid`.
    fn lit_xs(points: &[crate::patterns::Point], grid: f32) -> Vec<i32> {
        let mut xs: Vec<i32> = points.iter().filter(|q| q.is_lit()).map(|q| (q.x / grid).round() as i32).collect();
        xs.sort();
        xs.dedup();
        xs
    }

    /// T-440: the shipped abstracts come last, on « Abstraits » after its
    /// older cues (so every older id keeps its number, and they land on
    /// « Abstraits 2 », which has room: no page moves).
    #[test]
    fn abstract_cues_are_appended_to_the_abstracts_page() {
        let all = catalog();
        let new = &all[all.len() - ABSTRACT_CUES..];
        assert!(new.iter().all(|p| p.category == "Abstraits" && matches!(p.settings.content, Content::Abstract(_))));
        let older = all.iter().filter(|p| p.category == "Abstraits").count() - ABSTRACT_CUES;
        assert_eq!(new[0].id, format!("abstraits-{:03}", older + 1));
        let grid = crate::controls::GRID_ROWS * crate::controls::GRID_COLS;
        assert!(older > grid && older + ABSTRACT_CUES <= 2 * grid, "on « Abstraits 2 », within one page");
        assert_eq!(new[0].name, "Lissajous 3:2");
    }

    #[test]
    fn evolving_cues_are_appended_on_their_own_page() {
        let all = catalog();
        let new = &all[all.len() - ABSTRACT_CUES - EVOLVING_CUES..all.len() - ABSTRACT_CUES];
        assert!(new.iter().all(|p| p.category == EVOLVING && p.tag.is_some()));
        assert_eq!((new[0].id.as_str(), new[11].id.as_str()), ("festival-evolutifs-001", "festival-evolutifs-012"));
        assert!(new.len() <= crate::controls::GRID_ROWS * crate::controls::GRID_COLS, "one grid page");
        // E1 to E12: length in beats and loop, as specified.
        let spec = [(16.0, false), (32.0, false), (32.0, true), (16.0, true), (32.0, true), (32.0, false), (32.0, false), (32.0, true), (16.0, true), (32.0, false), (16.0, false), (32.0, false)];
        let names: HashSet<&str> = all.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), all.len(), "every cue name is unique (the examples find cues by name)");
        for (p, (len, looped)) in new.iter().zip(spec) {
            let Content::Evolving(cue) = &p.settings.content else { panic!("{} is not an evolving cue", p.id) };
            assert_eq!((cue.length_beats, cue.looped), (len, looped), "{}", p.name);
            assert!(p.name.ends_with(&format!(" · {len} temps")), "{} says its length", p.name);
            let word = match p.tag.unwrap() {
                CueTag::Intro => "Intro",
                CueTag::Groove => "Groove",
                CueTag::Break => "Break",
                CueTag::Build => "Montée",
                CueTag::Drop => "Drop",
            };
            assert!(p.name.starts_with(&format!("{word} – ")), "{}", p.name);
            // Keys only ever name known generators; a program is valid and plays like its cue.
            assert!(cue.keys.iter().all(|k| crate::generators::GENERATOR_NAMES.contains(&k.generator.as_str())), "{}", p.name);
            if let Some(program) = &p.settings.program {
                program.validate().unwrap();
                assert_eq!(program.length(&p.settings.content), len as f64);
                assert_eq!(program.play == PlayMode::Loop, looped, "{}", p.name);
            }
        }
        for tag in [CueTag::Intro, CueTag::Break, CueTag::Build, CueTag::Drop] {
            assert!(new.iter().any(|p| p.tag == Some(tag)), "a {tag:?} cue");
        }
    }

    /// Every evolving cue over its whole length (and a bit after), every
    /// 1/4 beat, at 120 and 150 BPM: a frame on every step, within the
    /// point budget with the optimiser, above the horizon, lit at some
    /// point; and the same frames at both tempos (E11's stars and hot beam
    /// live a time in seconds, for the static-beam guard: not them).
    #[test]
    fn every_evolving_cue_plays_in_beats_above_the_horizon_and_within_budget() {
        let on = crate::optimize::OptimizeParams { enabled: true, ..Default::default() };
        for (n, p) in evolving_page().iter().enumerate() {
            let Content::Evolving(cue) = &p.settings.content else { unreachable!() };
            let steps = (cue.length() * 4.0) as usize + 8;
            let play = |bpm: f64| {
                let mut a = Animator::starting_at(0.0);
                a.set_optimizer(&on);
                (0..=steps)
                    .map(|i| {
                        let clock = BeatClock { beat: i as f64 / 4.0, bpm, beats_per_bar: 4 };
                        a.render(&p.settings, AudioFeatures::default(), if i == 0 { 0.0 } else { (15.0 / bpm) as f32 }, &clock)
                    })
                    .collect::<Vec<_>>()
            };
            let (slow, fast) = (play(120.0), play(150.0));
            let mut lit = 0;
            for (i, f) in slow.iter().enumerate() {
                assert!(!f.is_empty() && f.len() <= 1000, "{} at beat {}: {} points", p.name, i as f64 / 4.0, f.len());
                for q in f.iter().filter(|q| q.is_lit()) {
                    assert!(q.y >= crate::fans::HORIZON - 1e-3 && q.x.abs() <= 1.0, "{} draws at ({}, {}) at beat {}", p.name, q.x, q.y, i as f64 / 4.0);
                    lit += 1;
                }
            }
            assert!(lit > 0, "{} is dark", p.name);
            if n + 1 != 11 {
                assert_eq!(slow, fast, "{} plays in beats, whatever the tempo", p.name);
            }
        }
    }

    /// Strobes faster than 1/2 beat (over 4 Hz at 128 BPM) only come in
    /// bursts shorter than the limiter's 5 s, and never faster than 1/4 beat.
    #[test]
    fn evolving_strobes_stay_short_bursts() {
        let max_burst_beats = crate::safety::DEFAULT_BURST_S as f64 * 128.0 / 60.0;
        for (n, p) in evolving_page().iter().enumerate() {
            let (cue, program) = evo(n + 1);
            let len = cue.length();
            let (mut run, mut longest, mut strobed) = (0.0, 0.0f64, false);
            for i in 0..(len * 16.0) as usize {
                let b = i as f64 / 16.0;
                let key = at(&cue, b).strobe_div;
                let lane = program.as_ref().and_then(|pr| pr.modifiers(b, len, 128.0)).map_or(0.0, |m| m.strobe_div);
                let div = key.max(lane);
                assert!(div <= 4.0, "{}: {div} flashes a beat", p.name);
                strobed |= div > 0.0;
                run = if div > 2.0 { run + 1.0 / 16.0 } else { 0.0 };
                longest = longest.max(run);
            }
            assert!(longest <= max_burst_beats, "{}: {longest} beats of fast strobe", p.name);
            if [2, 7, 12].contains(&(n + 1)) {
                assert!(strobed && longest >= 1.0, "{} has its strobe", p.name);
            }
        }
    }

    #[test]
    fn e1_fan_rises_widens_speeds_up_and_whitens() {
        let (cue, _) = evo(1);
        assert!((at(&cue, 0.0).scale - 0.15).abs() < 0.01 && (at(&cue, 16.0).scale - 0.7).abs() < 0.01);
        assert!((at(&cue, 8.0).scale - 0.425).abs() < 0.01, "smooth: half way at beat 8");
        let steps = |b: f64| at(&cue, b).params.steps_per_beat;
        assert_eq!((steps(4.0), steps(7.9), steps(8.0), steps(11.5), steps(12.0), steps(15.0)), (1.0, 1.0, 2.0, 2.0, 4.0, 4.0));
        assert_eq!(at(&cue, 0.5).params.a, 7.0, "fill chase");
        // Held while it waits for its beat: two beams, not a lone one.
        assert_eq!((at(&cue, 0.0).params.a, at(&cue, 0.02).params.a), (LAUNCH_SHAPE, 7.0));
        assert_eq!(lit_places(&frame_of(1, 0.0, 120.0), 0.02).len(), 2);
        let bright = |b: f64| at(&cue, b).brightness;
        assert_eq!((bright(15.4), bright(15.5), bright(15.9), bright(16.0)), (1.0, 0.0, 0.0, 1.0));
        assert!(frame_of(1, 15.7, 120.0).iter().all(|q| !q.is_lit()), "half a beat of black");
        assert_eq!(at(&cue, 4.0).color, crate::festival::PALETTES[PAL_SUNSET].c1, "colour 1 for the first 8 beats");
        assert!(at(&cue, 16.0).color.iter().all(|&c| c >= 250), "white at the end: {:?}", at(&cue, 16.0).color);
        // Height above the horizon: 0.05, then ease-in to 0.6.
        let height = |b: f64| {
            let ys: Vec<f32> = frame_of(1, b, 120.0).iter().filter(|q| q.is_lit()).map(|q| q.y).collect();
            ys.iter().sum::<f32>() / ys.len() as f32
        };
        assert!((height(0.1) - 0.05).abs() < 0.01, "{}", height(0.1));
        let want = 0.05 + 0.55 * (15.0f32 / 16.0).powi(2);
        assert!((height(15.0) - want).abs() < 0.01, "{} vs {want}", height(15.0));
    }

    #[test]
    fn e2_tunnel_closes_then_pumps_and_turns_back_every_4_beats() {
        let (cue, program) = evo(2);
        let beams = match &program.as_ref().unwrap().layers[0] {
            Content::Evolving(c) => c.clone(),
            _ => panic!("a layer of beams"),
        };
        assert!(at(&cue, 15.0).scale <= 0.05 && (at(&cue, 0.0).scale - 0.35).abs() < 1e-3);
        // Beat 16: the drop opens at 0.45 (the pump's peak) in colour 2.
        let ring: Vec<f32> = frame_of(2, 16.0, 120.0).iter().filter(|q| q.is_lit()).map(|q| q.x.hypot(q.y - crate::tunnels::TUNNEL_CY)).collect();
        let widest = ring.iter().cloned().fold(0.0, f32::max);
        assert!((widest - 0.45).abs() < 0.01, "{widest}");
        assert_eq!(at(&cue, 16.0).color, crate::festival::PALETTES[PAL_ICE].c2);
        assert_eq!(at(&cue, 15.9).color, crate::festival::PALETTES[PAL_ICE].c1);
        // The beams' angle (a × beat) turns back on beats 20, 24 and 28.
        let angle = |b: f64| at(&beams, b).params.a as f64 * b;
        for (turn, before) in [(20.0, 1.0), (24.0, -1.0), (28.0, 1.0)] {
            assert!((angle(turn) - angle(turn - 0.5)) * before > 0.05, "beat {turn}");
            assert!((angle(turn + 0.5) - angle(turn)) * before < -0.05, "beat {turn}");
        }
        // It speeds up over the build: 1/32 → 1/4 turn a beat.
        let speed = |b: f64| (angle(b + 0.5) - angle(b - 0.5)).abs();
        assert!(speed(1.5) < 0.06 && speed(14.5) > 0.2, "{} {}", speed(1.5), speed(14.5));
        // Strobe from beat 12 (1/2 beat), 14 (1/4 beat); black from 15.5.
        assert_eq!((mods(2, 11.0).strobe_div, mods(2, 12.5).strobe_div, mods(2, 14.5).strobe_div), (0.0, 2.0, 4.0));
        assert_eq!((mods(2, 15.6).live.brightness, mods(2, 16.0).live.brightness), (0.0, 1.0));
    }

    #[test]
    fn e3_scissors_double_their_speed_and_flash_white_on_every_beat() {
        let (cue, _) = evo(3);
        let period = |b: f64| cue.keys[at(&cue, b).key].params.period_beats;
        for b in [0.5, 1.5, 3.9, 8.2, 11.0, 16.5, 27.0] {
            assert_eq!(period(b), 2.0, "beat {b}");
        }
        for b in [4.0, 5.5, 7.9, 12.3, 31.5] {
            assert_eq!(period(b), 1.0, "beat {b}");
        }
        for k in 0..32 {
            let flash = at(&cue, k as f64 + 0.05);
            assert_eq!((flash.color, flash.params.color2), (WHITE, WHITE), "beat {k}");
            let after = at(&cue, k as f64 + 0.2);
            assert_ne!(after.color, after.params.color2, "two groups, two colours");
        }
        let colours: HashSet<(u8, u8, u8)> =
            frame_of(3, 0.5, 120.0).iter().filter(|q| q.is_lit()).map(|q| ((q.r * 255.0) as u8, (q.g * 255.0) as u8, (q.b * 255.0) as u8)).collect();
        assert_eq!(colours.len(), 2, "{colours:?}");
    }

    #[test]
    fn e4_stabs_step_through_four_positions_then_add_the_off_beat() {
        let (cue, _) = evo(4);
        let pal = crate::festival::PALETTES[PAL_HARD];
        let places = |b: f64| lit_places(&frame_of(4, b, 120.0), 0.02);
        for k in 0..4 {
            let b = k as f64 + 0.05;
            assert!(!places(b).is_empty(), "lit on beat {k}");
            assert_eq!(places(b), places(b + 4.0), "beat {k} and {} show P[{k}]", k + 4);
            assert_ne!(places(b), places(b + 1.0), "beat {k} and {} differ", k + 1);
            assert!(places(k as f64 + 0.3).is_empty(), "dark after 0.2 beat");
        }
        assert_eq!(cue.keys[at(&cue, 8.0).key].params.steps_per_beat, 2.0);
        assert!(!places(8.55).is_empty() && places(8.25).is_empty() && places(8.7).is_empty(), "the off-beat, lit 0.15 beat");
        assert_ne!(places(8.05), places(8.55), "a new position on the off-beat");
        assert_eq!((at(&cue, 0.05).color, at(&cue, 4.05).color, at(&cue, 8.05).color, at(&cue, 12.6).color), (pal.c1, pal.c2, pal.c1, pal.c2));
    }

    #[test]
    fn e5_ceiling_comes_down_and_snaps_back_on_the_last_half_beat() {
        let (cue, _) = evo(5);
        let height = |b: f64| at(&cue, b).params.b;
        assert!((height(0.0) - 0.6).abs() < 1e-4);
        assert!((height(31.0) - 0.08).abs() < 0.005, "{}", height(31.0));
        assert!(height(16.0) < height(8.0) && height(24.0) < height(16.0));
        assert!((height(31.6) - 0.6).abs() < 1e-4 && (height(31.99) - 0.6).abs() < 1e-4, "back up for the loop");
        assert!(at(&cue, 31.0).params.a > at(&cue, 1.0).params.a, "the ripple grows");
        assert_eq!(at(&cue, 0.0).params.color_mode, ColorMode::Gradient);
        assert_ne!(at(&cue, 8.0).color, at(&cue, 24.0).color, "the hue drifts");
        assert_eq!(at(&cue, 32.0), at(&cue, 0.0), "loops");
    }

    #[test]
    fn e6_sun_rays_come_in_by_pairs_then_turn_and_whiten() {
        let (cue, _) = evo(6);
        for k in 0..12 {
            let rays = lit_places(&frame_of(6, k as f64 + 0.5, 120.0), 0.01).len();
            assert_eq!(rays, (2 * (k + 1)).min(16), "beat {k}");
        }
        assert!((0..16).all(|b| at(&cue, b as f64 + 0.5).params.a == 0.0), "no rotation before beat 16");
        assert!(at(&cue, 20.0).params.a > 0.0);
        let turned = |b: f64| at(&cue, b).params.a as f64 * b;
        assert!((turned(24.0) - turned(20.0) - 4.0 / 64.0).abs() < 2e-3, "1/64 turn a beat");
        assert_eq!(at(&cue, 31.0).color, WHITE);
        assert_eq!(at(&cue, 2.0).color, SUN_AMBER);
        assert!(at(&cue, 17.0).params.b >= 0.5 && at(&cue, 15.0).params.b < 0.5, "odd / even from beat 16");
    }

    #[test]
    fn e7_chase_speeds_up_then_strobes_then_goes_black() {
        let (cue, _) = evo(7);
        let steps = |b: f64| at(&cue, b).params.steps_per_beat;
        assert_eq!((steps(4.0), steps(12.0), steps(20.0), steps(26.0)), (1.0, 2.0, 4.0, 8.0));
        assert_eq!((at(&cue, 0.0).params.a, at(&cue, 0.5).params.a), (LAUNCH_SHAPE, 2.0), "held on two beams, then a bounce");
        assert_eq!(lit_places(&frame_of(7, 0.0, 120.0), 0.02).len(), 2);
        assert!((0..28).all(|b| at(&cue, b as f64).generator == "chase_fan" && at(&cue, b as f64).strobe_div == 0.0));
        let strobe = at(&cue, 29.0);
        assert_eq!((strobe.generator.as_str(), strobe.strobe_div), ("fan", 4.0));
        assert!(lit_places(&frame_of(7, 29.0, 120.0), 0.02).len() == 8 && frame_of(7, 29.15, 120.0).iter().all(|q| !q.is_lit()));
        for b in [31.6, 31.9, 40.0] {
            assert!(frame_of(7, b, 120.0).iter().all(|q| !q.is_lit()), "black at {b}");
        }
        // 3.5 beats of 1/4-beat strobe: 1.6 s at 128 BPM.
        let strobing = (0..32 * 16).filter(|i| {
            let s = at(&cue, *i as f64 / 16.0);
            s.strobe_div > 0.0 && s.brightness > 0.0
        });
        let beats = strobing.count() as f64 / 16.0;
        assert_eq!(beats, 3.5);
        assert!(beats * 60.0 / 128.0 <= crate::safety::DEFAULT_BURST_S as f64);
    }

    #[test]
    fn e8_spokes_turn_back_through_zero_and_swap_colours() {
        let (cue, _) = evo(8);
        let pal = crate::festival::PALETTES[PAL_AFTERLIFE];
        let angle = |b: f64| at(&cue, b).params.a as f64 * b;
        let speed = |b: f64| (angle(b + 0.02) - angle(b - 0.02)) / 0.04;
        for turn in [8.0, 16.0, 24.0] {
            assert!(speed(turn).abs() < 0.03, "still at beat {turn}: {}", speed(turn));
            assert!(speed(turn - 1.0).abs() > 0.2 && speed(turn + 1.0).abs() > 0.2, "beat {turn}");
            assert!(speed(turn - 1.0) * speed(turn + 1.0) < 0.0, "turned back at {turn}");
            let (before, after) = (at(&cue, turn - 0.1), at(&cue, turn));
            assert_eq!((before.color, after.color), (after.params.color2, before.params.color2), "colours swapped at {turn}");
        }
        assert_eq!((at(&cue, 1.0).color, at(&cue, 9.0).color), (pal.c1, pal.c2));
        // The radius pumps: largest just after each beat, back half a beat later.
        for k in [0.0, 5.0, 17.0] {
            let (kick, later, rest) = (mods(8, k + 0.01).live.size, mods(8, k + 0.3).live.size, mods(8, k + 0.6).live.size);
            assert!(kick > 1.2 && kick > later && later > rest && rest == 1.0, "{kick} {later} {rest}");
            // ... around the tunnel's centre.
            let pos = mods(8, k + 0.01).live.pos_y;
            assert!((crate::tunnels::TUNNEL_CY * kick + pos - crate::tunnels::TUNNEL_CY).abs() < 1e-3);
        }
    }

    #[test]
    fn e9_wave_grows_turns_back_with_a_colour_chase_and_loops_smoothly() {
        let (cue, _) = evo(9);
        let s = |b: f64| at(&cue, b);
        assert!((s(8.0).params.a - 0.35).abs() < 1e-4 && (s(4.0).params.a - 0.2).abs() < 1e-4);
        assert_eq!((s(7.9).params.direction, s(8.0).params.direction), (1, -1));
        assert_eq!((s(7.9).params.color_mode, s(8.0).params.color_mode, s(15.0).params.color_mode), (ColorMode::Solid, ColorMode::ColorChase, ColorMode::ColorChase));
        assert_eq!(s(9.0).params.steps_per_beat, 4.0, "the front moves a beam every 1/4 beat");
        assert!((s(10.0).params.a - 0.35).abs() < 1e-4, "held to beat 12");
        assert!((s(15.999).params.a - 0.05).abs() < 1e-3 && (s(16.0).params.a - 0.05).abs() < 1e-6, "back to 0.05 for the loop");
    }

    #[test]
    fn e10_curtains_come_in_then_close_in_to_one_line() {
        let (cue, _) = evo(10);
        let curtains = |b: f64| lit_xs(&frame_of(10, b, 120.0), 0.1);
        assert_eq!(curtains(0.5), [-6]);
        assert_eq!(curtains(4.5), [-6, -2]);
        assert_eq!(curtains(12.5), [-6, -2, 2, 6]);
        assert!(curtains(20.25).len() == 4 && curtains(20.75).is_empty(), "lit half of every beat");
        let width = |b: f64| at(&cue, b).scale;
        assert!(width(16.0) > width(20.0) && width(20.0) > width(28.0), "closing in");
        assert_eq!(at(&cue, 31.0).params.count, 1);
        assert_eq!(curtains(31.1), [0], "one centre line");
    }

    #[test]
    fn e11_stars_thicken_quicken_close_in_then_one_beam_then_black() {
        let (cue, _) = evo(11);
        let s = |b: f64| at(&cue, b);
        assert_eq!((s(0.0).params.count, s(12.0).params.count), (3, 12));
        assert_eq!((s(0.0).params.steps_per_beat, s(12.0).params.steps_per_beat), (1.0, 4.0));
        let stars = |b: f64| lit_places(&frame_of(11, b, 120.0), 0.002).len();
        assert!(stars(0.5) >= crate::textures::STAR_MIN, "the starfield's floor");
        assert_eq!(frame_of(11, 6.3, 120.0), frame_of(11, 6.3, 120.0), "the same sky for the same beat");
        // Beats 12 to 15: closing in on (0, 0.3).
        let spread = |b: f64| {
            let f = frame_of(11, b, 120.0);
            f.iter().filter(|q| q.is_lit()).map(|q| q.x.hypot(q.y - E11_POINT)).fold(0.0, f32::max)
        };
        assert!(spread(14.9) < 0.06 && spread(14.9) < spread(13.0) && spread(13.0) < spread(11.0), "{} {} {}", spread(11.0), spread(13.0), spread(14.9));
        assert_eq!(s(15.2).generator, "hot_beam");
        assert!(stars(15.2) >= 1 && frame_of(11, 15.2, 120.0).iter().filter(|q| q.is_lit()).all(|q| q.r == q.g && q.g == q.b), "one white beam");
        for b in [15.6, 15.9, 20.0] {
            assert!(frame_of(11, b, 120.0).iter().all(|q| !q.is_lit()), "black at {b}");
        }
    }

    #[test]
    fn e12_grid_is_revealed_tightens_strobes_then_becomes_a_ceiling() {
        let (cue, _) = evo(12);
        let lines = |b: f64| {
            let p = at(&cue, b).params;
            p.count + p.a.round() as u32
        };
        assert_eq!((lines(0.0), lines(0.9), lines(1.0), lines(4.5), lines(9.0), lines(15.0)), (1, 1, 2, 5, 10, 10));
        assert!((at(&cue, 28.0).scale / at(&cue, 16.0).scale - 0.4).abs() < 1e-4);
        assert!((at(&cue, 22.0).scale / at(&cue, 16.0).scale - 0.7).abs() < 1e-4);
        assert_eq!((at(&cue, 27.9).strobe_div, at(&cue, 28.0).strobe_div, at(&cue, 31.9).strobe_div), (0.0, 4.0, 4.0));
        assert_eq!((at(&cue, 32.0).generator.as_str(), at(&cue, 40.0).generator.as_str(), at(&cue, 32.0).strobe_div), ("ceiling", "ceiling", 0.0));
    }

    const GOLDEN_FRAMES: u64 = 5_542_766_730_402_865_477;
    const GOLDEN_IDS: u64 = 1_416_572_363_121_705_351;

    #[test]
    fn ids_are_stable_slugs() {
        assert_eq!(slug("Texte & horloge"), "texte-horloge");
        assert_eq!(slug("Géométrie"), "geometrie");
        assert_eq!(catalog()[0].id, "abstraits-001");
    }
}
