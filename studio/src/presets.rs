//! Our built-in cue library: a few hundred ready-made looks, organised in
//! pages like the cue grids of pro laser software. Every cue is one of our
//! own generators or shapes with chosen parameters and colours - nothing is
//! taken from another product's library. The catalogue is built in code,
//! so ids stay stable as long as the lists below only grow at the end.

use crate::engine::{Animator, AudioFeatures, AudioReact, BeatClock, Content, Settings};
use crate::generators::{ColorMode, GenParams};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub category: &'static str,
    pub settings: Settings,
}

/// Cue pages, in display order. The last one holds the operator's
/// figures (`figures.rs`), not built-in cues.
pub const CATEGORIES: &[&str] =
    &["Abstraits", "Tunnels", "Faisceaux", "Balayages", "Vagues", "Géométrie", "Audio", "Texte & horloge", crate::figures::CATEGORY];

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

    b.presets
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
        for c in CATEGORIES.iter().filter(|c| **c != crate::figures::CATEGORY) {
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
        for p in catalog() {
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
        let ids: String = catalog().iter().map(|p| format!("{}|", p.id)).collect();
        assert_eq!(digest(ids.bytes().map(|b| b as f32)), GOLDEN_IDS);
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
