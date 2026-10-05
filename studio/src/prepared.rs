//! « Préparer » (T-274): a cue loaded blind, seen and tweaked before it
//! goes out.
//!
//! The operator loads a cue into the prepared slot (Scène › *Préparer*,
//! or Alt + clic on a cell), sees it on its own *Préparé* view, adjusts
//! its size, brightness, rotation and colour, then *Envoyer* plays it
//! through the normal cue path (`controls::send_prepared`): quantised
//! launches (T-159) and transitions (T-158) apply, then the mixer, the
//! master stage, the zones, the calibration and the whole safety stage,
//! like any cue press.
//!
//! The boundary, what keeps the prepared cue off every output:
//!
//! - the slot has its own `Animator`, rendered only by
//!   `GET /api/preview-frame` (`render`), on the HTTP thread, when a page
//!   asks. The engine (`Engine::tick`) never reads `Shared::prepared`:
//!   no layer, deck, cue list, timeline or look holds it, so it never
//!   reaches the output stage, the venue's other projectors, PONK, the
//!   Art-Net SD card signal, the pushed frames (`/api/frame.bin`) or
//!   `/api/frame`'s points;
//! - preparing, adjusting, rendering or clearing changes nothing else:
//!   not the look, the deck, the playlist, the settings revision or the
//!   arming (the safety invariants replay sessions with and without a
//!   prepared cue and compare every output frame bit for bit);
//! - *Envoyer* is the only way out, and it is a cue press: it never arms;
//!   disarmed, the output stays dark as for any cue.
//!
//! The slot is memory only: never saved, not in a project, not in the
//! undo history.

use crate::cues::ClickMode;
use crate::engine::{Animator, BeatClock, Settings};
use crate::patterns::Point;
use crate::{controls, Shared};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

/// The prepared cue: which cell it came from and the look it will start
/// with (the cue's own look, then the operator's adjustments).
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedCue {
    pub id: String,
    pub name: String,
    pub settings: Settings,
}

/// The preview deck (`Shared::prepared`). Its animator is its own: the
/// engine's animators never see it, and it never sees them.
#[derive(Default)]
pub struct PreviewSlot {
    pub cue: Option<PreparedCue>,
    animator: Animator,
    /// Studio time of the last preview render, for the animation step.
    last_t: Option<f64>,
    /// Bumped on every change, for the page.
    pub rev: u64,
}

/// `POST /api/prepared/adjust`: the prepared look's own values. Missing
/// fields are left as they are.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Adjust {
    pub scale: Option<f32>,
    pub brightness: Option<f32>,
    pub rotation_speed: Option<f32>,
    pub color: Option<[u8; 3]>,
}

/// Fastest rotation the adjustment takes, degrees per second.
pub const MAX_ROTATION: f32 = 720.0;

#[derive(Debug, PartialEq)]
pub enum PrepareError {
    /// No such cue (404).
    NoCue,
    /// Nothing is prepared (409).
    Empty,
    /// The request can't be done (400): a cell holding a show, a value
    /// that isn't a number.
    Refused(&'static str),
}

impl PrepareError {
    pub fn status(&self) -> u16 {
        match self {
            PrepareError::NoCue => 404,
            PrepareError::Empty => 409,
            PrepareError::Refused(_) => 400,
        }
    }

    pub fn message_fr(&self) -> &'static str {
        match self {
            PrepareError::NoCue => "cue introuvable",
            PrepareError::Empty => "aucun cue préparé",
            PrepareError::Refused(why) => why,
        }
    }
}

/// Loads cue `id` into the prepared slot, from its start. Nothing else
/// changes.
pub fn prepare(s: &mut Shared, id: &str) -> Result<(), PrepareError> {
    let Some(name) = s.presets.iter().find(|p| p.id == id).map(|p| p.name.clone()) else { return Err(PrepareError::NoCue) };
    if s.deck.slot(id).show.is_some() {
        return Err(PrepareError::Refused("une case de show ne se prépare pas : elle lance la timeline"));
    }
    let settings = controls::cue_settings(s, id).ok_or(PrepareError::NoCue)?;
    s.prepared.cue = Some(PreparedCue { id: id.to_string(), name, settings });
    s.prepared.animator = Animator::default();
    s.prepared.last_t = None;
    s.prepared.rev += 1;
    Ok(())
}

/// The operator's adjustments on the prepared look, each clamped to its
/// range.
pub fn adjust(s: &mut Shared, a: &Adjust) -> Result<(), PrepareError> {
    let finite = |v: Option<f32>| v.is_none_or(f32::is_finite);
    if !(finite(a.scale) && finite(a.brightness) && finite(a.rotation_speed)) {
        return Err(PrepareError::Refused("valeur non numérique"));
    }
    let cue = s.prepared.cue.as_mut().ok_or(PrepareError::Empty)?;
    let look = &mut cue.settings;
    if let Some(v) = a.scale {
        look.scale = v.clamp(0.0, 1.0);
    }
    if let Some(v) = a.brightness {
        look.brightness = v.clamp(0.0, 1.0);
    }
    if let Some(v) = a.rotation_speed {
        look.rotation_speed = v.clamp(-MAX_ROTATION, MAX_ROTATION);
    }
    if let Some(c) = a.color {
        look.color = c;
    }
    s.prepared.rev += 1;
    Ok(())
}

/// Empties the slot.
pub fn clear(s: &mut Shared) {
    if s.prepared.cue.take().is_some() {
        s.prepared.rev += 1;
    }
    s.prepared.animator = Animator::default();
    s.prepared.last_t = None;
}

/// « Envoyer »: the prepared cue goes live through the normal cue path
/// (from its start, replacing itself if it plays; quantise and
/// transitions apply), with the prepared look. The slot is emptied.
/// Returns the cue's id. Never arms.
pub fn send(s: &mut Shared) -> Result<String, PrepareError> {
    let cue = s.prepared.cue.clone().ok_or(PrepareError::Empty)?;
    if !controls::send_prepared(s, &cue.id, Some(ClickMode::Restart), cue.settings, Some("ui")) {
        // The cue was deleted meanwhile: the slot is emptied.
        clear(s);
        return Err(PrepareError::NoCue);
    }
    clear(s);
    Ok(cue.id)
}

/// The prepared view's frame, rendered now on the slot's own animator
/// (`GET /api/preview-frame`): the look alone, as the cue would start,
/// at the studio's tempo and with the audio the engine hears. Empty when
/// nothing is prepared. Never touches the engine's state.
pub fn render(s: &mut Shared) -> Vec<Point> {
    let t = s.now_s();
    let Some(cue) = &s.prepared.cue else { return Vec::new() };
    let dt = s.prepared.last_t.map_or(0.0, |last| (t - last).clamp(0.0, 0.1)) as f32;
    let clock = BeatClock { beat: s.tempo.effect_beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar };
    let audio = s.audio_in.effective(s.audio, s.audio_at, Instant::now()).0;
    let points = s.prepared.animator.render(&cue.settings, audio, dt, &clock);
    s.prepared.last_t = Some(t);
    points
}

/// What the page needs to show the slot (in `/api/state`, `/api/frame`
/// and the pushed state): no points.
pub fn view(s: &Shared) -> Value {
    match &s.prepared.cue {
        None => json!({ "rev": s.prepared.rev, "cue": null }),
        Some(c) => json!({
            "rev": s.prepared.rev,
            "cue": {
                "id": c.id,
                "name": c.name,
                "scale": c.settings.scale,
                "brightness": c.settings.brightness,
                "rotation_speed": c.settings.rotation_speed,
                "color": c.settings.color,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    fn first_cue(s: &Shared) -> String {
        s.presets.iter().find(|p| s.deck.slot(&p.id).show.is_none()).expect("a cue").id.clone()
    }

    #[test]
    fn preparing_changes_nothing_the_output_reads() {
        let mut s = test_support::shared();
        let id = first_cue(&s);
        let (look, rev, look_on) = (s.settings.clone(), s.settings_rev, s.look_on);
        prepare(&mut s, &id).unwrap();
        adjust(&mut s, &Adjust { scale: Some(0.3), brightness: Some(0.5), rotation_speed: Some(90.0), color: Some([1, 2, 3]) }).unwrap();
        for _ in 0..5 {
            assert!(!render(&mut s).is_empty(), "the prepared view draws");
        }
        assert_eq!(s.settings, look);
        assert_eq!(s.settings_rev, rev);
        assert_eq!(s.look_on, look_on);
        assert!(s.deck.active.is_empty() && s.deck.pending.is_empty());
        assert_eq!(s.active_cue, None);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn adjustments_are_clamped_and_need_a_cue() {
        let mut s = test_support::shared();
        assert_eq!(adjust(&mut s, &Adjust { scale: Some(0.5), ..Default::default() }), Err(PrepareError::Empty));
        let id = first_cue(&s);
        prepare(&mut s, &id).unwrap();
        adjust(&mut s, &Adjust { scale: Some(4.0), brightness: Some(-1.0), rotation_speed: Some(1e9), color: None }).unwrap();
        let look = &s.prepared.cue.as_ref().unwrap().settings;
        assert_eq!((look.scale, look.brightness, look.rotation_speed), (1.0, 0.0, MAX_ROTATION));
        assert_eq!(adjust(&mut s, &Adjust { scale: Some(f32::NAN), ..Default::default() }), Err(PrepareError::Refused("valeur non numérique")));
        assert_eq!(prepare(&mut s, "pas-un-cue"), Err(PrepareError::NoCue));
    }

    #[test]
    fn send_plays_the_prepared_look_through_the_deck_and_never_arms() {
        let mut s = test_support::shared();
        let id = first_cue(&s);
        prepare(&mut s, &id).unwrap();
        adjust(&mut s, &Adjust { scale: Some(0.25), color: Some([10, 20, 30]), ..Default::default() }).unwrap();
        assert_eq!(send(&mut s).unwrap(), id);
        let a = s.deck.active.iter().find(|a| a.cue == id).expect("the cue plays");
        assert_eq!((a.settings.scale, a.settings.color), (0.25, [10, 20, 30]));
        assert!(s.prepared.cue.is_none(), "the slot is emptied");
        assert!(!s.gate.is_armed());
        assert_eq!(send(&mut s), Err(PrepareError::Empty));
    }

    #[test]
    fn send_waits_for_the_quantised_boundary() {
        let mut s = test_support::shared();
        s.deck.quantize = crate::cues::Quantize::Bar;
        s.tempo.set_bpm_manual(120.0, 0.0);
        s.sim_now = Some(s.epoch + std::time::Duration::from_millis(700));
        let id = first_cue(&s);
        prepare(&mut s, &id).unwrap();
        send(&mut s).unwrap();
        assert!(s.deck.pending.iter().any(|p| p.cue == id), "waits for the bar");
        assert!(!s.deck.active.iter().any(|a| a.cue == id));
    }

    #[test]
    fn a_show_cell_is_not_prepared() {
        let mut s = test_support::shared();
        let id = first_cue(&s);
        let mut slot = s.deck.slot(&id);
        slot.show = Some("intro".into());
        s.deck.set_slot(&id, slot);
        assert!(matches!(prepare(&mut s, &id), Err(PrepareError::Refused(_))));
    }

    /// Code review in a test: nothing on the output side names the
    /// prepared slot. The engine's frame (`impl Engine` in main.rs) and
    /// every output path never read it.
    #[test]
    fn no_output_path_reads_the_prepared_slot() {
        let main = include_str!("main.rs");
        let start = main.find("impl Engine {").expect("impl Engine");
        let end = start + main[start..].find("\n}\n").expect("end of impl Engine");
        let reads = |src: &str| [".prepared", "prepared::"].iter().any(|k| src.contains(k));
        assert!(!reads(&main[start..end]), "Engine::tick must never read Shared::prepared");
        for (name, src) in [
            ("output.rs", include_str!("output.rs")),
            ("output_manager.rs", include_str!("output_manager.rs")),
            ("push.rs", include_str!("push.rs")),
            ("ponk.rs", include_str!("ponk.rs")),
            ("artnet.rs", include_str!("artnet.rs")),
            ("shownet.rs", include_str!("shownet.rs")),
            ("venue.rs", include_str!("venue.rs")),
            ("projection.rs", include_str!("projection.rs")),
            ("layers.rs", include_str!("layers.rs")),
            ("cues.rs", include_str!("cues.rs")),
            ("cuelist.rs", include_str!("cuelist.rs")),
            ("timeline.rs", include_str!("timeline.rs")),
        ] {
            assert!(!reads(src), "{name} must never read the prepared slot");
        }
    }
}
