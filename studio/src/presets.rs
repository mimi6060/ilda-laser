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
}

/// Cue pages, in display order. The last two hold the operator's
/// figures (`figures.rs`) and ILDA files (`ilda_library.rs`), not
/// built-in cues.
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
];

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
        self.presets.push(Preset { id: format!("{}-{n:03}", slug(category)), name, category, settings });
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
            let settings = Settings { content: Content::Text { text: word.to_string() }, color, scale: 0.25, ..Settings::default() };
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

    b.presets
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
    let step = points.len().div_ceil(THUMB_POINTS).max(1);
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
    let bpm = if bpm.is_finite() { bpm.clamp(20.0, 400.0) } else { 120.0 };
    let len = settings.program.as_ref().map_or(4.0, |p| p.length(&settings.content));
    let step = (len / PREVIEW_FRAMES as f64).max(0.25);
    let count = ((len / step).round() as usize).clamp(1, PREVIEW_FRAMES);
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
    fn catalogue_is_large_and_ids_are_unique() {
        let all = catalog();
        assert!(all.len() >= 200, "only {} cues", all.len());
        let ids: HashSet<_> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn every_category_has_cues_and_every_cue_has_a_known_category() {
        let all = catalog();
        for c in CATEGORIES.iter().filter(|c| **c != crate::figures::CATEGORY && **c != crate::ilda_library::CATEGORY) {
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

    #[test]
    fn texture_cues_are_appended_on_the_beams_page() {
        let all = catalog();
        let start = LEGACY_CUES + RHYTHM_CUES + PROGRAM_CUES + CROSSING_CUES;
        let new = &all[start..];
        assert_eq!(new.len(), TEXTURE_CUES);
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

    const GOLDEN_FRAMES: u64 = 5_542_766_730_402_865_477;
    const GOLDEN_IDS: u64 = 1_416_572_363_121_705_351;

    #[test]
    fn ids_are_stable_slugs() {
        assert_eq!(slug("Texte & horloge"), "texte-horloge");
        assert_eq!(slug("Géométrie"), "geometrie");
        assert_eq!(catalog()[0].id, "abstraits-001");
    }
}
