//! Audio reactivity presets (T-239): a set of audio routes (T-153)
//! applied in one click from LIVE › Musique, so the operator gets a
//! finished result without wiring ten routes by hand.
//!
//! Our own settings, built on the lighting rule of thumb in
//! docs/research/audio-analysis.md §6.1: low frequencies move large
//! parameters slowly, high frequencies small ones quickly, events make
//! discrete changes. Nothing is copied from another product.
//!
//! - **Punch**: kick → brightness and size pulses.
//! - **Groove**: bass → size, low-mids → rotation, snare → colour.
//! - **Scintillement**: hi-hat → colour spread and a small jitter, highs →
//!   animation speed (fine detail, never brightness).
//! - **Festival**: the lot, plus build-up → speed and rotation, drop →
//!   full opening of the size.
//! - **Calme**: bass → a slow size breath only.
//!
//! Applying one replaces the routes and keeps the *Temps ↔ Audio*
//! crossfader and everything else (looks, scenes, live modifiers). A route
//! whose target control doesn't exist is left out with a warning.
//!
//! **Safety**: a preset is only routes, so every rule of routes.rs holds
//! (brightness only dims below the fader, the T-245 flash cap, neutral
//! when the audio goes away, then the output safety stage). No preset
//! routes a continuous high-frequency value (`high`, `hat_strength`,
//! `centroid`) or the hi-hat onto a brightness control: that flickers.

use super::routes::{AudioRoute, AudioRouting};
use super::safety::darkness_sign;
use super::shape::{Curve, Shaper, Span, Target};
use crate::controls::ControlRegistry;

/// One preset: its id (`punch`…), its French name and its routes.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPreset {
    pub id: &'static str,
    pub name_fr: &'static str,
    /// One line for the menu's tooltip.
    pub hint_fr: &'static str,
    pub routes: Vec<AudioRoute>,
}

/// Sources that must never drive a brightness control (flicker).
pub const NEVER_ON_BRIGHTNESS: [&str; 4] = ["high", "hat", "hat_strength", "centroid"];

/// A shaper from the defaults with a few settings changed (its running
/// state is private to shape.rs).
fn shape(edit: impl FnOnce(&mut Shaper)) -> Shaper {
    let mut s = Shaper::default();
    edit(&mut s);
    s
}

fn route(source: &str, target: &str, shape: Shaper) -> AudioRoute {
    AudioRoute { source: source.into(), target: target.into(), shape, enabled: true }
}

/// Kick → brightness: dimmed by `depth` between kicks, full on the kick
/// (a brightness can only be dimmed under the operator's fader).
fn kick_light(depth: f32) -> AudioRoute {
    route(
        "kick",
        "master.brightness",
        shape(|s| {
            s.gate = 0.0;
            s.attack = Span::Ms(0.0);
            s.decay = Span::Beats(0.5);
            s.min = -depth;
            s.max = 0.0;
        }),
    )
}

/// Kick → size: a short push outwards.
fn kick_size(amount: f32) -> AudioRoute {
    route(
        "kick",
        "master.size",
        shape(|s| {
            s.gate = 0.0;
            s.attack = Span::Ms(0.0);
            s.decay = Span::Beats(0.5);
            s.max = amount;
        }),
    )
}

/// Bass → size, following the low end.
fn bass_size(amount: f32, attack_ms: f32, release_ms: f32, curve: Curve) -> AudioRoute {
    route(
        "bass",
        "master.size",
        shape(|s| {
            s.gate = 0.1;
            s.curve = curve;
            s.attack = Span::Ms(attack_ms);
            s.release = Span::Ms(release_ms);
            s.max = amount;
        }),
    )
}

/// Low-mids → rotation speed, slowly.
fn low_mid_rotation(amount: f32) -> AudioRoute {
    route(
        "low_mid",
        "master.rot_z.speed",
        shape(|s| {
            s.gate = 0.1;
            s.attack = Span::Ms(200.0);
            s.release = Span::Ms(800.0);
            s.max = amount;
        }),
    )
}

/// Snare → a hue step that eases back over two beats.
fn snare_hue() -> AudioRoute {
    route(
        "snare",
        "master.color.hue",
        shape(|s| {
            s.gate = 0.2;
            s.attack = Span::Ms(0.0);
            s.decay = Span::Beats(2.0);
            s.max = 0.25;
        }),
    )
}

/// Every preset, in menu order.
pub fn builtin_presets() -> Vec<AudioPreset> {
    vec![
        AudioPreset {
            id: "punch",
            name_fr: "Punch",
            hint_fr: "Kick → luminosité et taille",
            routes: vec![kick_light(0.3), kick_size(0.12)],
        },
        AudioPreset {
            id: "groove",
            name_fr: "Groove",
            hint_fr: "Basses → taille, bas-médiums → rotation, caisse claire → couleur",
            routes: vec![bass_size(0.15, 40.0, 300.0, Curve::Square), low_mid_rotation(0.1), snare_hue()],
        },
        AudioPreset {
            id: "sparkle",
            name_fr: "Scintillement",
            hint_fr: "Charleston → étalement des couleurs et petit frémissement, aigus → vitesse d'animation",
            routes: vec![
                route(
                    "hat",
                    "master.color.spread",
                    shape(|s| {
                        s.gate = 0.2;
                        s.attack = Span::Ms(0.0);
                        s.decay = Span::Ms(150.0);
                        s.max = 0.25;
                    }),
                ),
                route(
                    "hat",
                    "master.rot_z.angle",
                    shape(|s| {
                        s.gate = 0.2;
                        s.attack = Span::Ms(0.0);
                        s.decay = Span::Ms(100.0);
                        s.max = 0.01;
                    }),
                ),
                route(
                    "high",
                    "master.speed",
                    shape(|s| {
                        s.gate = 0.1;
                        s.attack = Span::Ms(30.0);
                        s.release = Span::Ms(200.0);
                        s.max = 0.1;
                    }),
                ),
            ],
        },
        AudioPreset {
            id: "festival",
            name_fr: "Festival",
            hint_fr: "Tout ensemble, plus montée → vitesse et rotation, drop → pleine ouverture",
            routes: vec![
                kick_light(0.25),
                kick_size(0.08),
                bass_size(0.1, 40.0, 300.0, Curve::Square),
                low_mid_rotation(0.08),
                snare_hue(),
                route(
                    "buildup",
                    "master.speed",
                    shape(|s| {
                        s.gate = 0.05;
                        s.attack = Span::Beats(1.0);
                        s.release = Span::Ms(500.0);
                        s.max = 0.25;
                    }),
                ),
                route(
                    "buildup",
                    "master.rot_z.speed",
                    shape(|s| {
                        s.gate = 0.05;
                        s.attack = Span::Beats(1.0);
                        s.release = Span::Ms(500.0);
                        s.max = 0.15;
                    }),
                ),
                route(
                    "drop",
                    "master.size",
                    shape(|s| {
                        s.gate = 0.0;
                        s.attack = Span::Ms(0.0);
                        s.decay = Span::Beats(8.0);
                        s.max = 0.4;
                    }),
                ),
            ],
        },
        AudioPreset {
            id: "calm",
            name_fr: "Calme",
            hint_fr: "Basses → lente respiration de la taille, rien d'autre",
            routes: vec![bass_size(0.08, 400.0, 1500.0, Curve::SCurve)],
        },
    ]
}

/// The preset's routes this registry can take, and a French warning for
/// each one left out (its target control is missing).
fn usable(preset: &AudioPreset, reg: &ControlRegistry) -> (Vec<AudioRoute>, Vec<String>) {
    let mut warnings = Vec::new();
    let routes = preset
        .routes
        .iter()
        .filter(|r| {
            let ok = Target::bind(reg, &r.target).is_some();
            if !ok {
                warnings.push(format!("« {} » introuvable : lien {} → {} ignoré", r.target, r.source, r.target));
            }
            ok
        })
        .cloned()
        .collect();
    (routes, warnings)
}

/// `current` with its routes replaced by preset `id`'s (the crossfader
/// kept), and the warnings for the routes left out. `None`: no such preset.
pub fn apply(id: &str, current: &AudioRouting, reg: &ControlRegistry) -> Option<(AudioRouting, Vec<String>)> {
    let preset = builtin_presets().into_iter().find(|p| p.id == id)?;
    let (routes, warnings) = usable(&preset, reg);
    Some((AudioRouting { routes, ..current.clone() }, warnings))
}

/// The preset whose routes are exactly `routing`'s, if any (else the menu
/// shows *Personnalisé*).
pub fn matching(routing: &AudioRouting, reg: &ControlRegistry) -> Option<&'static str> {
    if routing.routes.is_empty() {
        return None;
    }
    builtin_presets().into_iter().find(|p| usable(p, reg).0 == routing.routes).map(|p| p.id)
}

/// Whether `route` would flicker a brightness: a high-frequency source on
/// a control that changes the light.
pub fn flickers(route: &AudioRoute) -> bool {
    darkness_sign(&route.target).is_some() && NEVER_ON_BRIGHTNESS.contains(&route.source.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::routes::{validate, RouteStore, MAX_ROUTES};
    use crate::audio::safety::Guard;
    use crate::engine::{AudioFeatures, Settings};
    use crate::live::LiveModifiers;
    use crate::presets;
    use std::path::PathBuf;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&presets::catalog())
    }

    #[test]
    fn every_preset_is_valid_with_known_ids_and_its_own_name() {
        let reg = reg();
        let all = builtin_presets();
        let ids: Vec<&str> = all.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["punch", "groove", "sparkle", "festival", "calm"]);
        let names: Vec<&str> = all.iter().map(|p| p.name_fr).collect();
        assert_eq!(names, ["Punch", "Groove", "Scintillement", "Festival", "Calme"]);
        for p in &all {
            assert!(!p.routes.is_empty() && p.routes.len() <= MAX_ROUTES, "{}", p.id);
            let routing = AudioRouting { routes: p.routes.clone(), ..Default::default() };
            validate(&routing, &reg).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            assert_eq!(routing.clone().sanitized(), routing, "{}: numbers already in range", p.id);
            let (routes, warnings) = usable(p, &reg);
            assert_eq!((routes.len(), warnings.len()), (p.routes.len(), 0), "{}", p.id);
        }
    }

    #[test]
    fn no_preset_drives_a_brightness_with_high_frequencies() {
        for p in builtin_presets() {
            for r in &p.routes {
                assert!(!flickers(r), "{}: {} → {}", p.id, r.source, r.target);
                if darkness_sign(&r.target).is_some() {
                    // Only discrete kicks, never a continuous value.
                    assert_eq!(r.source, "kick", "{}: {} on a brightness", p.id, r.source);
                }
            }
        }
        assert!(flickers(&route("high", "master.brightness", Shaper::default())));
        assert!(flickers(&route("hat", "look.brightness", Shaper::default())));
        assert!(!flickers(&route("high", "master.speed", Shaper::default())));
    }

    #[test]
    fn the_presets_follow_the_low_slow_high_fast_rule() {
        let low = ["bass", "sub", "bass_band", "low_mid", "buildup"];
        for p in builtin_presets() {
            for r in &p.routes {
                if low.contains(&r.source.as_str()) {
                    let release = r.shape.release.seconds(0.5);
                    assert!(release >= 0.3, "{}: {} releases in {release} s", p.id, r.source);
                }
                if r.source == "high" || r.source == "hat" {
                    assert!(r.shape.max.abs() <= 0.25, "{}: {} moves its target too far", p.id, r.source);
                }
            }
        }
    }

    #[test]
    fn applying_replaces_the_routes_and_keeps_the_crossfader() {
        let reg = reg();
        let current = AudioRouting { mix: 0.8, routes: vec![AudioRoute::default()] };
        let (punch, warnings) = apply("punch", &current, &reg).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(punch.mix, 0.8);
        assert_eq!(punch.routes.iter().map(|r| (r.source.as_str(), r.target.as_str())).collect::<Vec<_>>(), [("kick", "master.brightness"), ("kick", "master.size")]);
        assert_eq!(matching(&punch, &reg), Some("punch"));
        let (calm, _) = apply("calm", &punch, &reg).unwrap();
        assert_eq!((calm.mix, calm.routes.len(), matching(&calm, &reg)), (0.8, 1, Some("calm")));
        assert_eq!(matching(&current, &reg), None, "custom routes");
        assert_eq!(matching(&AudioRouting::default(), &reg), None);
        assert!(apply("nope", &current, &reg).is_none());
    }

    #[test]
    fn a_missing_control_is_left_out_with_a_warning() {
        let reg = reg();
        let mut p = builtin_presets().remove(0);
        p.routes.push(route("bass", "master.gone", Shaper::default()));
        let (routes, warnings) = usable(&p, &reg);
        assert_eq!(routes.len(), 2);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("master.gone"), "{warnings:?}");
    }

    #[test]
    fn a_kick_under_punch_raises_brightness_and_size() {
        let reg = reg();
        let (routing, _) = apply("punch", &AudioRouting::default(), &reg).unwrap();
        let mut store = RouteStore::load_or_create(PathBuf::from("/nonexistent/audio_routes.json"), &reg);
        store.replace_in_memory(routing, &reg);
        let frame = |store: &mut RouteStore, kick: u64| {
            let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
            let f = AudioFeatures { kick, kick_strength: 1.0, ..Default::default() };
            store.apply(&f, 1.0 / 60.0, 0.5, &Guard::LIVE, &mut settings, &mut live);
            (live.brightness, live.size)
        };
        // Fade in over the attack, settle between kicks.
        let mut between = (0.0, 0.0);
        for _ in 0..60 {
            between = frame(&mut store, 0);
        }
        let on_kick = frame(&mut store, 1);
        assert!(on_kick.0 > between.0 + 0.2, "brightness {between:?} → {on_kick:?}");
        assert!(on_kick.1 > between.1 + 0.2, "size {between:?} → {on_kick:?}");
        assert!(on_kick.0 <= 1.0, "never above the fader");
    }
}
