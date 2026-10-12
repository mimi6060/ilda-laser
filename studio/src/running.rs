//! « En cours » (T-423): one answer to « what is playing, and who touched
//! it last? » - the cues, the timeline and the playlist, the modulators
//! and audio links that are on, the masters away from neutral, and the
//! modes that change what comes out (test pattern, audience, show mode,
//! external inputs, output). Plus the stop buttons, by row and by kind.
//!
//! Read-only over `Shared` except `stop`. Nothing here arms, and nothing
//! here is on a safety path: the e-stop, blackout and disarm act on the
//! output stage and ignore the cue protections.

use crate::controls::{self, ControlDesc, ControlInput, ControlKind, ControlSource};
use crate::cues::{ActiveCue, ClickMode, CLICK_MODES, CLICK_MODE_LABELS};
use crate::lfo::Wave;
use crate::live::Rate;
use crate::timeline::Transport;
use crate::transition::CuePhase;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `GET /api/running`. Cheap: a walk over the deck, the cue list, the two small stores
/// and the master/layer controls (a few dozen), under the lock already held.
pub fn snapshot(s: &Shared) -> Value {
    json!({
        "cues": cues(s),
        "timeline": timeline(s),
        "playlist": playlist(s),
        "cuelist": cuelist(s),
        // Running macros (T-425), with the step they are at.
        "macros": crate::macros::running(s),
        // Scripts (T-458): runs in flight and automatic triggers armed.
        "scripts": crate::script::running(s),
        "lfos": lfos(s),
        "audio_links": audio_links(s),
        "masters": masters(s),
        "status": status(s),
    })
}

fn mode_label(mode: ClickMode) -> &'static str {
    CLICK_MODES.iter().position(|&m| m == mode).map_or("", |i| CLICK_MODE_LABELS[i])
}

/// Where a cue is in its life (T-158/T-159), for the « Cues » rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Waiting for its quantised beat (« en attente »).
    Pending,
    /// Fading or morphing in (« démarrage »).
    Starting,
    Playing,
    /// Stopped, still drawn while it fades out (« en fondu »).
    Ending,
}

impl Phase {
    pub fn label_fr(self) -> &'static str {
        match self {
            Phase::Pending => "en attente",
            Phase::Starting => "démarrage",
            Phase::Playing => "joue",
            Phase::Ending => "en fondu",
        }
    }
}

/// One « Cues » row: `instance` is `None` for a launch still waiting (it
/// has no instance yet); `progress` is the fade's 0..1, else `None`.
struct Row<'a> {
    cue: &'a str,
    instance: Option<u64>,
    layer: u8,
    mode: ClickMode,
    held: bool,
    shown: bool,
    source: ControlSource,
    phase: Phase,
    progress: Option<f32>,
}

fn cue_row(s: &Shared, r: Row) -> Value {
    let Row { cue, instance, layer, mode, held, shown, source, phase, progress } = r;
    let name = s.presets.iter().find(|p| p.id == cue).map_or(cue, |p| p.name.as_str());
    // The first page holding the cue (1-based), for « show it ».
    let page = s.grid.view().iter().position(|pg| pg.cells.iter().any(|c| c.as_deref() == Some(cue)));
    let slot = s.deck.slot(cue);
    json!({
        "instance": instance,
        "cue": cue,
        "name": name,
        "page": page.map(|p| p + 1),
        "page_name": page.and_then(|p| s.grid.view().get(p)).map(|pg| pg.name.as_str()),
        "layer": layer,
        "mode": mode,
        "mode_label": mode_label(mode),
        "held": held,
        "shown": shown,
        "phase": phase,
        "phase_label": phase.label_fr(),
        "progress": progress,
        "source": source,
        "source_label": source.label_fr(),
        "protect_stop": slot.protect_stop,
        "protect_solo": slot.protect_solo,
    })
}

/// Playing cues, then the ones waiting for their beat, then the ones
/// fading out.
fn cues(s: &Shared) -> Vec<Value> {
    let now = s.now_s();
    let shown: Vec<u64> = s.deck.visible().iter().map(|a| a.id).collect();
    let mode_of = |a: &ActiveCue| match (a.held, a.solo) {
        (true, true) => ClickMode::Solo,
        (true, false) => ClickMode::Flash,
        _ => s.deck.mode_of(&a.cue),
    };
    let mut out: Vec<Value> = s
        .deck
        .active
        .iter()
        .map(|a| {
            let (phase, progress) = match s.deck.phase(a.id, now) {
                Some(CuePhase::Starting(p)) => (Phase::Starting, Some(p)),
                _ => (Phase::Playing, None),
            };
            cue_row(s, Row { cue: &a.cue, instance: Some(a.id), layer: a.layer, mode: mode_of(a), held: a.held, shown: shown.contains(&a.id), source: a.source, phase, progress })
        })
        .collect();
    out.extend(s.deck.pending.iter().map(|p| {
        let layer = s.deck.slot(&p.cue).layer.unwrap_or(1);
        cue_row(s, Row { cue: &p.cue, instance: None, layer, mode: p.mode, held: false, shown: false, source: p.source, phase: Phase::Pending, progress: None })
    }));
    let drawn: Vec<u64> = s.deck.ending_visible().map(|e| e.cue.id).collect();
    out.extend(s.deck.ending.iter().map(|e| {
        let a = &e.cue;
        let (progress, shown) = (Some(e.fade.progress(now)), drawn.contains(&a.id));
        cue_row(s, Row { cue: &a.cue, instance: Some(a.id), layer: a.layer, mode: mode_of(a), held: a.held, shown, source: a.source, phase: Phase::Ending, progress })
    }));
    out
}

/// The loaded show while it plays or is paused; `null` when stopped.
fn timeline(s: &Shared) -> Value {
    if s.timeline.transport == Transport::Stopped {
        return Value::Null;
    }
    let st = s.timeline.state(&s.timeline_clock());
    json!({
        // Driven by incoming timecode (T-168).
        "timecode": s.timecode.is_chasing(),
        "name": st.name,
        "playing": st.playing,
        "paused": st.paused,
        "waiting": st.waiting,
        "position": st.position,
        "length": st.length,
        "time_base": st.time_base,
        "loop": st.loop_on,
    })
}

/// The loaded cue list (T-410) while a step is on show or waits for its
/// delay; `null` otherwise (loaded but at *Début*, or none).
fn cuelist(s: &Shared) -> Value {
    if !s.cue_list.is_active() {
        return Value::Null;
    }
    s.cue_list.view()
}

fn playlist(s: &Shared) -> Value {
    let Some(p) = &s.playlist else { return Value::Null };
    json!({ "index": p.index, "scene": s.scenes.list().get(p.index).map(|sc| sc.name.as_str()) })
}

fn label_of(s: &Shared, id: &str) -> String {
    s.controls.get(id).map_or_else(|| id.to_string(), |d| d.label_fr.clone())
}

pub fn wave_label(w: Wave) -> &'static str {
    match w {
        Wave::Sine => "Sinus",
        Wave::Triangle => "Triangle",
        Wave::Square => "Carré",
        Wave::SawUp => "Dent de scie montante",
        Wave::SawDown => "Dent de scie descendante",
        Wave::Random => "Aléatoire",
    }
}

/// The modulators that are on, with their index in `lfos.json`.
fn lfos(s: &Shared) -> Vec<Value> {
    s.lfos
        .list()
        .iter()
        .enumerate()
        .filter(|(_, m)| m.enabled && m.depth > 0.0)
        .map(|(index, m)| {
            let rate = match m.rate {
                Rate::Beats(b) => format!("{b} temps"),
                Rate::Hz(hz) => format!("{hz} Hz"),
            };
            json!({ "index": index, "target": m.target, "target_label": label_of(s, &m.target), "wave": m.wave, "wave_label": wave_label(m.wave), "rate": rate, "depth": m.depth })
        })
        .collect()
}

/// The audio links that are on, with their index in the routing.
fn audio_links(s: &Shared) -> Vec<Value> {
    s.routes
        .routing()
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| r.enabled)
        .map(|(index, r)| {
            let source_label = crate::audio::routes::SOURCES.iter().find(|(id, _)| *id == r.source).map_or(r.source.as_str(), |(_, l)| *l);
            json!({ "index": index, "source": r.source, "source_label": source_label, "target": r.target, "target_label": label_of(s, &r.target) })
        })
        .collect()
}

/// A control's neutral value, as `current` reports it; `None` for the ones
/// without a value (triggers).
fn neutral(desc: &ControlDesc) -> Option<Value> {
    Some(match &desc.kind {
        ControlKind::Continuous { default, .. } => json!(default),
        ControlKind::Toggle { default } => json!(default),
        ControlKind::Momentary => json!(false),
        ControlKind::Choice { default, .. } => json!(default),
        ControlKind::Trigger => return None,
    })
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-4,
        _ => a == b,
    }
}

/// Is a control of the « Maîtres » section: a master or a layer control.
fn is_master(desc: &ControlDesc) -> bool {
    matches!(desc.group, "master" | "layer")
}

/// The master and layer controls away from their registry default, with
/// who changed them last.
pub fn masters(s: &Shared) -> Vec<Value> {
    s.controls
        .list()
        .iter()
        .filter(|d| is_master(d))
        .filter_map(|d| {
            let default = neutral(d)?;
            let value = controls::current(s, d)?;
            if value.is_null() || same(&value, &default) {
                return None;
            }
            let touch = s.touches.get(&d.id);
            Some(json!({
                "id": d.id,
                "label": d.label_fr,
                "value": value,
                "default": default,
                "source": touch.map(|t| t.source),
                "source_label": touch.map(|t| t.source.label_fr()),
            }))
        })
        .collect()
}

/// What else changes the output right now, one line each (French).
fn status(s: &Shared) -> Vec<Value> {
    let mut out = Vec::new();
    let mut add = |kind: &str, label: &str, value: String, warn: bool| out.push(json!({ "kind": kind, "label": label, "value": value, "warn": warn }));
    if let Some(p) = s.test_pattern.active {
        let name = crate::testpattern::LABELS_FR.iter().find(|(q, _)| *q == p).map_or("", |(_, l)| *l);
        add("test_pattern", "Mire de test", name.to_string(), true);
    }
    if s.zone_trace.is_on() {
        let what = match s.zone_trace.only {
            Some(id) => s.safety.get().zones.iter().find(|z| z.id == id).map_or_else(|| format!("zone {id}"), |z| z.name.clone()),
            None => "Toutes les zones".into(),
        };
        add("zone_trace", "Tracé des zones", what, true);
    }
    if s.audience.is_scanning() {
        add("audience", "Mode public", "Balayage du public autorisé".into(), true);
    }
    if s.show_mode.on {
        add("show_mode", "Mode spectacle", "Verrouillé".into(), false);
    }
    add(
        "external",
        "Entrées externes",
        if s.external.gate.open { "Porte ouverte (OSC, DMX, timecode)".into() } else { "Porte fermée".into() },
        false,
    );
    let tc = s.timecode.source();
    if tc != crate::timecode::TcSource::Off {
        // Incoming timecode (T-168): its source, TC-IN and what it reads.
        let st = s.timecode.status();
        let mut value = format!("{} · {} · {}", tc.label_fr(), if s.timecode.armed { "TC-IN armé" } else { "TC-IN désarmé" }, st.label_fr());
        if let Some(code) = s.timecode.display(s.now_s()) {
            value.push_str(&format!(" · TC {code}"));
        }
        let warn = matches!(st, crate::timecode::Status::Preroll | crate::timecode::Status::Afterroll | crate::timecode::Status::Lost);
        add("timecode", "Timecode", value, warn);
    }
    let midi: Vec<&str> = s.midi.devices.iter().filter(|d| d.connected).map(|d| d.name.as_str()).collect();
    if !midi.is_empty() {
        add("midi", "MIDI", midi.join(", "), false);
    }
    let output = match (&s.output_name, s.gate.is_armed()) {
        (None, _) => "Aperçu seulement".to_string(),
        (Some(name), true) => format!("{name} · armé"),
        (Some(name), false) => format!("{name} · désarmé"),
    };
    add("output", "Sortie", output, s.gate.is_armed());
    out
}

/// `POST /api/running/stop`.
#[derive(Debug, Deserialize)]
pub struct StopRequest {
    pub kind: String,
    /// A cue's instance id (`kind: "cue"`), playing or fading out.
    pub instance: Option<u64>,
    /// A preset id (`kind: "pending"`: its waiting launch is cancelled).
    #[serde(default)]
    pub cue: Option<String>,
    /// A modulator's or audio link's index (`kind: "lfo"` / `"audio_link"`).
    pub index: Option<usize>,
    /// A macro's id (`kind: "macro"`, T-425).
    #[serde(default)]
    pub id: Option<String>,
}

/// One stop button of the panel. Every kind is playback, allowed in show
/// mode; none of them arms or disarms. Errors are French, for the UI.
pub fn stop(s: &mut Shared, req: &StopRequest) -> Result<(), String> {
    match req.kind.as_str() {
        "cue" => {
            let instance = req.instance.ok_or("instance manquante")?;
            if !controls::stop_instance(s, instance) {
                return Err("ce cue ne joue plus".into());
            }
        }
        "pending" => {
            let cue = req.cue.as_deref().ok_or("cue manquant")?;
            if !controls::stop_pending(s, cue) {
                return Err("ce cue n'attend plus".into());
            }
        }
        "cues" => controls::stop_cues(s),
        "macro" => {
            let id = req.id.as_deref().ok_or("macro manquante")?;
            if !crate::macros::stop(s, id) {
                return Err("cette macro ne tourne plus".into());
            }
        }
        "macros" => crate::macros::stop_all(s),
        // A script (T-458): its run dropped, its trigger held.
        "script" => {
            let id = req.id.as_deref().and_then(|i| i.parse::<u32>().ok()).ok_or("script manquant")?;
            if !crate::script::stop(s, id) {
                return Err("ce script ne tourne plus".into());
            }
        }
        "scripts" => crate::script::stop_all(s),
        // The cue list back to *Début* (T-410).
        "cuelist" => s.cue_list.top(),
        "timeline" => {
            // The show list with it (T-444): nothing chains after the stop.
            crate::showlist::stop(s);
            s.timeline.stop();
            s.playlist = None;
        }
        "lfo" | "lfos" => {
            let only = if req.kind == "lfo" { Some(req.index.ok_or("index manquant")?) } else { None };
            let mut list = s.lfos.list().to_vec();
            if only.is_some_and(|i| i >= list.len()) {
                return Err("modulateur inconnu".into());
            }
            for (i, m) in list.iter_mut().enumerate() {
                if only.is_none_or(|o| o == i) {
                    m.enabled = false;
                }
            }
            let s = &mut *s;
            s.lfos.set(list, &s.controls).map_err(|e| e.to_string())?;
        }
        "audio_link" | "audio" => {
            let only = if req.kind == "audio_link" { Some(req.index.ok_or("index manquant")?) } else { None };
            let mut routing = s.routes.routing().clone();
            if only.is_some_and(|i| i >= routing.routes.len()) {
                return Err("lien audio inconnu".into());
            }
            for (i, r) in routing.routes.iter_mut().enumerate() {
                if only.is_none_or(|o| o == i) {
                    r.enabled = false;
                }
            }
            let s = &mut *s;
            s.routes.set(routing, &s.controls).map_err(|e| e.to_string())?;
        }
        "masters" => reset_masters(s),
        "all" => controls::stop_running(s),
        other => return Err(format!("type d'arrêt inconnu : {other}")),
    }
    Ok(())
}

/// « Remettre les maîtres à zéro »: every master and layer control back
/// to its default, through the registry like a fader move.
fn reset_masters(s: &mut Shared) {
    let ids: Vec<String> = s.controls.list().iter().filter(|d| is_master(d) && neutral(d).is_some()).map(|d| d.id.clone()).collect();
    let _ = controls::apply_from(s, "master.reset", ControlInput::Value(1.0), ControlSource::Ui);
    for id in ids {
        let Some(desc) = s.controls.get(&id).cloned() else { continue };
        let (Some(default), Some(now)) = (neutral(&desc), controls::current(s, &desc)) else { continue };
        if same(&now, &default) {
            continue;
        }
        let v = default.as_f64().map(|v| v as f32).or_else(|| default.as_bool().map(|b| if b { 1.0 } else { 0.0 }));
        if let Some(v) = v {
            let _ = controls::apply_from(s, &id, ControlInput::Value(v), ControlSource::Ui);
        }
    }
    s.touches.retain(|id, _| !(id.starts_with("master.") || id.starts_with("layer.")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{apply, apply_from};
    use crate::cues::CueSlot;
    use crate::test_support::shared;

    fn ids(v: &[Value], key: &str) -> Vec<String> {
        v.iter().map(|x| x[key].as_str().unwrap_or_default().to_string()).collect()
    }

    #[test]
    fn a_neutral_studio_lists_nothing() {
        let s = shared();
        let snap = snapshot(&s);
        assert_eq!(snap["cues"], json!([]));
        assert_eq!(snap["timeline"], Value::Null);
        assert_eq!(snap["masters"], json!([]), "every master and layer control starts at its default");
        assert_eq!(snap["cuelist"], Value::Null);
    }

    /// T-410: the playing cue list is listed, with who drove it, and its
    /// stop button takes it back to « Début ».
    #[test]
    fn the_playing_cue_list_is_listed_and_stopped() {
        use crate::cuelist::{CueList, Step, StepSource};
        let mut s = shared();
        let step = Step { label: "Intro".into(), source: StepSource::Cue { id: s.presets[0].id.clone() }, ..Step::default() };
        crate::cuelist::save(&mut s, CueList { name: "Acte".into(), steps: vec![step], ..CueList::default() }).unwrap();
        crate::cuelist::load(&mut s, "Acte").unwrap();
        assert_eq!(snapshot(&s)["cuelist"], Value::Null, "loaded at Début: nothing plays");
        apply(&mut s, "list.go", ControlInput::Value(1.0), true).unwrap();
        let row = &snapshot(&s)["cuelist"];
        assert_eq!((row["name"].as_str(), row["index"].as_u64(), row["label"].as_str()), (Some("Acte"), Some(0), Some("Intro")));
        assert_eq!(row["source_label"], "MIDI");
        stop(&mut s, &StopRequest { cue: None, kind: "cuelist".into(), instance: None, index: None, id: None }).unwrap();
        assert_eq!(snapshot(&s)["cuelist"], Value::Null);
    }

    #[test]
    fn masters_away_from_default_are_listed_with_their_source() {
        let mut s = shared();
        apply(&mut s, "master.size", ControlInput::Value(0.3), true).unwrap();
        apply_from(&mut s, "layer.2.dimmer", ControlInput::Value(0.5), ControlSource::Ui).unwrap();
        let m = masters(&s);
        assert_eq!(ids(&m, "id"), ["master.size", "layer.2.dimmer"]);
        assert_eq!(ids(&m, "source_label"), ["MIDI", "Interface"]);
        // Back at its default: gone.
        apply(&mut s, "master.size", ControlInput::Value(1.0), false).unwrap();
        assert_eq!(ids(&masters(&s), "id"), ["layer.2.dimmer"]);
        stop(&mut s, &StopRequest { cue: None, kind: "masters".into(), instance: None, index: None, id: None }).unwrap();
        assert!(masters(&s).is_empty());
        assert!(!s.touches.keys().any(|k| k.starts_with("master.") || k.starts_with("layer.")));
    }

    #[test]
    fn apply_from_records_the_source_and_keeps_the_external_check() {
        let mut s = shared();
        apply_from(&mut s, "master.brightness", ControlInput::Value(0.4), ControlSource::Ui).unwrap();
        assert_eq!(s.touches["master.brightness"].source, ControlSource::Ui);
        apply(&mut s, "master.brightness", ControlInput::Value(0.5), true).unwrap();
        assert_eq!(s.touches["master.brightness"].source, ControlSource::Midi);
        // The page's route never arms, whatever source it records.
        assert!(apply_from(&mut s, "transport.arm", ControlInput::Value(1.0), ControlSource::Ui).is_err());
        assert!(!s.gate.is_armed());
        assert!(!s.touches.contains_key("transport.arm"));
    }

    #[test]
    fn cues_show_their_page_mode_and_source_and_stop_by_row() {
        let mut s = shared();
        let cue = s.presets[0].id.clone();
        let page = s.grid.view().iter().position(|pg| pg.cells.iter().any(|c| c.as_deref() == Some(cue.as_str())));
        assert!(controls::press_cue_from(&mut s, &cue, None, true, Some("midi")));
        let c = cues(&s);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["source_label"], "MIDI");
        assert_eq!(c[0]["mode_label"], "Basculer");
        assert_eq!(c[0]["page"], json!(page.map(|p| p + 1)));
        let instance = c[0]["instance"].as_u64().unwrap();
        assert!(stop(&mut s, &StopRequest { cue: None, kind: "cue".into(), instance: Some(instance), index: None, id: None }).is_ok());
        assert!(s.deck.active.is_empty());
        assert!(stop(&mut s, &StopRequest { cue: None, kind: "cue".into(), instance: Some(instance), index: None, id: None }).is_err());
        assert!(stop(&mut s, &StopRequest { cue: None, kind: "nope".into(), instance: None, index: None, id: None }).is_err());
    }

    #[test]
    fn stop_all_keeps_protected_cues_and_never_arms() {
        let mut s = shared();
        s.deck.multi = true;
        let (a, b) = (s.presets[0].id.clone(), s.presets[1].id.clone());
        s.deck.set_slot(&b, CueSlot { protect_stop: true, ..Default::default() });
        controls::press_cue_from(&mut s, &a, None, true, Some("ui"));
        controls::press_cue_from(&mut s, &b, None, true, Some("ui"));
        s.playlist = Some(crate::Playlist { index: 0, started: std::time::Instant::now() });
        apply(&mut s, "running.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.deck.active.iter().map(|x| x.cue.as_str()).collect::<Vec<_>>(), [b.as_str()]);
        assert!(s.playlist.is_none());
        assert!(!s.gate.is_armed());
        // cue.stop_all honours the protection too; a row stop doesn't.
        apply(&mut s, "cue.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.deck.active.len(), 1);
        let id = s.deck.active[0].id;
        stop(&mut s, &StopRequest { cue: None, kind: "cue".into(), instance: Some(id), index: None, id: None }).unwrap();
        assert!(s.deck.active.is_empty());
    }

    /// T-158/T-159: a launch waiting for its beat and a cue fading out are
    /// listed with their phase; the waiting one keeps who pressed it.
    #[test]
    fn waiting_and_fading_cues_are_listed_and_stoppable() {
        use crate::cues::Quantize;
        use crate::transition::{Transition, TransitionKind, TransitionLength};
        let mut s = shared();
        let (a, b) = (s.presets[0].id.clone(), s.presets[1].id.clone());
        s.deck.quantize = Quantize::Bar;
        s.deck.transition = Transition { kind: TransitionKind::Fade, length: TransitionLength::Seconds(30.0), in_flash: false };
        controls::press_cue_from(&mut s, &a, None, true, Some("midi"));
        let rows = cues(&s);
        let phases = |rows: &[Value]| rows.iter().map(|r| (r["cue"].as_str().unwrap().to_string(), r["phase_label"].as_str().unwrap().to_string())).collect::<Vec<_>>();
        if s.deck.pending.is_empty() {
            // Pressed right on a bar: it started at once.
            assert_eq!(phases(&rows), [(a.clone(), "démarrage".to_string())]);
        } else {
            assert_eq!(phases(&rows), [(a.clone(), "en attente".to_string())]);
            assert_eq!(rows[0]["instance"], Value::Null);
            assert_eq!(rows[0]["source_label"], "MIDI");
            // Its beat comes: it starts, still credited to MIDI.
            s.deck.pending[0].at_beat = f64::MIN;
            let now = s.now_s();
            controls::deck_frame(&mut s, now);
        }
        assert_eq!(s.deck.active[0].source, ControlSource::Midi);
        // b replaces a (one cue per layer): a fades out.
        s.deck.quantize = Quantize::Off;
        controls::press_cue_from(&mut s, &b, None, true, Some("ui"));
        let rows = cues(&s);
        assert_eq!(phases(&rows), [(b.clone(), "démarrage".to_string()), (a.clone(), "en fondu".to_string())]);
        assert!(rows[1]["progress"].as_f64().is_some());
        let ending = rows[1]["instance"].as_u64().unwrap();
        stop(&mut s, &StopRequest { cue: None, kind: "cue".into(), instance: Some(ending), index: None, id: None }).unwrap();
        assert!(s.deck.ending.is_empty());
        // A waiting launch is cancelled by its cue id.
        s.deck.quantize = Quantize::Bar;
        controls::press_cue_from(&mut s, &a, None, true, Some("ui"));
        if !s.deck.pending.is_empty() {
            stop(&mut s, &StopRequest { cue: Some(a.clone()), kind: "pending".into(), instance: None, index: None, id: None }).unwrap();
            assert!(s.deck.pending.is_empty());
        }
        assert!(stop(&mut s, &StopRequest { cue: Some(a), kind: "pending".into(), instance: None, index: None, id: None }).is_err());
    }

    /// The e-stop cuts the output whatever the flags: a protected cue keeps
    /// "playing" in the deck, but nothing lit leaves the output stage.
    #[test]
    fn the_estop_ignores_the_protection_flags() {
        let mut s = shared();
        let a = s.presets[0].id.clone();
        s.deck.set_slot(&a, CueSlot { protect_stop: true, protect_solo: true, ..Default::default() });
        let b = s.presets[1].id.clone();
        s.deck.set_slot(&b, CueSlot { protect_stop: true, ..Default::default() });
        s.deck.transition = crate::transition::Transition {
            kind: crate::transition::TransitionKind::Fade,
            length: crate::transition::TransitionLength::Seconds(30.0),
            in_flash: false,
        };
        controls::press_cue_from(&mut s, &a, None, true, Some("ui"));
        controls::press_cue_from(&mut s, &b, None, true, Some("ui")); // a fades out, protected
        s.deck.quantize = crate::cues::Quantize::Bar;
        controls::press_cue_from(&mut s, &a, None, true, Some("ui")); // may wait for the bar
        assert!(!s.deck.ending.is_empty());
        apply(&mut s, "safety.estop", ControlInput::Value(1.0), true).unwrap();
        assert!(s.estop.is_latched());
        assert!(!s.gate.is_armed());
        // Fades and waits of protected cues are cut like any other.
        assert!(s.deck.ending.is_empty() && s.deck.pending.is_empty());
        assert!(s.deck.active.iter().all(|x| x.fade.is_none()));
    }

    #[test]
    fn cutting_modulators_and_links_disables_without_deleting() {
        let dir = std::env::temp_dir().join(format!("ls-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = shared();
        s.lfos = crate::lfo::LfoStore::load_or_create(dir.join("lfos.json"), &s.controls);
        s.routes = crate::audio::routes::RouteStore::load_or_create(dir.join("routes.json"), &s.controls);
        let m = crate::lfo::Modulator { enabled: true, ..Default::default() };
        let list = vec![m.clone(), crate::lfo::Modulator { target: "master.brightness".into(), ..m }];
        let reg = &s.controls;
        s.lfos.set(list, reg).unwrap();
        let routing = crate::audio::routes::AudioRouting { routes: vec![Default::default(), Default::default()], ..Default::default() };
        s.routes.set(routing, &s.controls).unwrap();
        assert_eq!(lfos(&s).len(), 2);
        assert_eq!(audio_links(&s).len(), 2);
        stop(&mut s, &StopRequest { cue: None, kind: "lfo".into(), instance: None, index: Some(1), id: None }).unwrap();
        assert_eq!(lfos(&s).len(), 1);
        stop(&mut s, &StopRequest { cue: None, kind: "lfos".into(), instance: None, index: None, id: None }).unwrap();
        stop(&mut s, &StopRequest { cue: None, kind: "audio".into(), instance: None, index: None, id: None }).unwrap();
        assert!(lfos(&s).is_empty() && audio_links(&s).is_empty());
        assert_eq!(s.lfos.list().len(), 2, "nothing deleted");
        assert_eq!(s.routes.routing().routes.len(), 2);
        assert!(stop(&mut s, &StopRequest { cue: None, kind: "lfo".into(), instance: None, index: Some(9), id: None }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
