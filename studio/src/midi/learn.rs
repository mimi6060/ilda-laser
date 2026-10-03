//! MIDI learn (T-203): the user points at a control on screen, touches the
//! controller, and the next Note On / CC / pitch bend / program change of
//! any port becomes a mapping in that port's profile.
//!
//! - The message it binds is taken by the learn: it doesn't also act.
//! - Built-in profiles are never modified: the first edit writes a
//!   `<slug>-perso` copy and assigns it to the port.
//! - The mode is inferred from the target (button → trigger / toggle /
//!   momentary) and the input (CC of a known encoder, or one repeating an
//!   encoder step → relative; other CC / pitch bend → absolute with pickup).
//! - `transport.arm` can only be learned with the T-208 option on, and only
//!   as a Shift mapping (the arming gesture needs Shift). Learning never
//!   arms or disarms anything by itself.
//!
//! Runs under the `Shared` lock from the worker batch, like the engine.

use super::engine;
use super::mapping::{Curve, Incoming, InputKind, LedFeedback, MapMode, Mapping, MidiInput, RelEncoding};
use super::profile::GENERIC;
use super::safety::ARM;
use super::MidiEvent;
use crate::controls::{ControlKind, GRID_COLS, GRID_ROWS};
use crate::Shared;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Learning gives up after this long without a usable message.
pub const LEARN_TIMEOUT: Duration = Duration::from_secs(15);
/// Pseudo target of a `grid` mapping (slot `args.slot` of the current page).
pub const GRID: &str = "grid";

const ARM_REFUSED: &str =
    "« Allumer le laser » ne peut être appris que si « Autoriser l'armement depuis le contrôleur » est coché (Sécurité)";

#[derive(Clone, Debug)]
pub struct LearnRequest {
    /// Canonical control id, or `GRID`.
    pub target: String,
    pub args: Value,
    pub shift: bool,
    pub until: Instant,
}

/// The last mapping created, for the UI.
#[derive(Clone, Debug, Serialize)]
pub struct Learned {
    pub seq: u64,
    pub port: String,
    pub profile: String,
    pub index: usize,
    pub message: String,
    pub channel: Option<u8>,
    pub target: String,
    pub target_label: String,
    pub mode: MapMode,
    pub shift: bool,
    /// Label of the target it replaced, if any.
    pub replaced: Option<String>,
}

/// The message is already mapped: the UI asks before replacing.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub port: String,
    pub mapping: Mapping,
    /// Target label of the mapping in place.
    pub existing: String,
    /// Fader position the message had (pickup seed).
    pos: Option<f32>,
}

#[derive(Default)]
pub struct LearnState {
    pub pending: Option<LearnRequest>,
    pub conflict: Option<Conflict>,
    pub learned: Option<Learned>,
    /// Last outcome that isn't a mapping (expired, refused…), numbered so
    /// the UI shows each once.
    pub notice: Option<(u64, String)>,
    seq: u64,
    /// Last value of every CC seen, per (port, channel, number): a CC that
    /// repeats its value isn't a fader being moved.
    cc_last: HashMap<(String, u8, u8), u8>,
    /// Encoder-step values seen per CC (`step_class` bits): tells the
    /// three relative encodings apart once both directions were turned.
    cc_steps: HashMap<(String, u8, u8), u8>,
}

pub type LearnError = (u16, String);

/// `POST /api/midi/learn`: waits for the next message (15 s).
pub fn start(s: &mut Shared, target: &str, args: Value, shift: bool, now: Instant) -> Result<(), LearnError> {
    if !s.midi.enabled {
        return Err((409, "MIDI désactivé (--no-midi) : rien à apprendre".into()));
    }
    let (target, args) = if target == GRID {
        if grid_slot(&args).is_none() {
            return Err((400, format!("« grid » : « args.slot » de 0 à {} attendu", GRID_ROWS * GRID_COLS - 1)));
        }
        (GRID.to_string(), args)
    } else {
        let Some(d) = s.controls.get(target) else { return Err((404, format!("contrôle inconnu : {target}"))) };
        if d.id == ARM {
            if !s.midi.store.devices.safety.allow_arm {
                return Err((403, ARM_REFUSED.into()));
            }
        } else if !d.external {
            return Err((403, format!("« {} » ne peut pas être piloté par un contrôleur", d.label_fr)));
        }
        (d.id.clone(), Value::Null)
    };
    // The MIDI arming gesture is Shift + hold: a plain arm mapping would never work.
    let shift = shift || target == ARM;
    let st = &mut s.midi.learn;
    st.pending = Some(LearnRequest { target, args, shift, until: now + LEARN_TIMEOUT });
    st.conflict = None;
    Ok(())
}

/// `POST /api/midi/learn/cancel` (and Échap in the UI).
pub fn cancel(s: &mut Shared) {
    let st = &mut s.midi.learn;
    if st.pending.take().is_some() | st.conflict.take().is_some() {
        notice(s, "Apprentissage MIDI annulé".into());
    }
}

/// Called once per engine frame: gives up after `LEARN_TIMEOUT`.
pub fn expire(s: &mut Shared, now: Instant) {
    if s.midi.learn.pending.as_ref().is_some_and(|r| now >= r.until) {
        s.midi.learn.pending = None;
        notice(s, format!("Apprentissage MIDI annulé : aucun message reçu en {} s", LEARN_TIMEOUT.as_secs()));
    }
}

/// Remembers CC values (see `LearnState::cc_last`). After `capture`.
pub fn observe(s: &mut Shared, ev: &MidiEvent) {
    if let super::MidiMsg::Cc { channel, number, value } = ev.msg {
        let key = (ev.port.clone(), channel & 0x0F, number);
        *s.midi.learn.cc_steps.entry(key.clone()).or_default() |= step_class(value);
        s.midi.learn.cc_last.insert(key, value);
    }
}

/// While learning: binds this event if it is usable. True = taken (the
/// engine must not also act on it). Releases, real-time messages, the
/// Shift key and CCs that don't move go on to the engine.
pub fn capture(s: &mut Shared, ev: &MidiEvent) -> bool {
    let Some(req) = s.midi.learn.pending.clone() else { return false };
    if ev.at >= req.until {
        expire(s, ev.at);
        return false;
    }
    let Some(m) = Incoming::from_msg(&ev.msg) else { return false };
    let Some(profile) = engine::profile_of(s, &ev.port).cloned() else { return false };
    if engine::is_shift_key(&profile, &m) {
        return false;
    }
    let mut encoding = None;
    // A first encoder-like value from an unknown CC: fader or encoder? The
    // next message tells (same value = encoder).
    let mut unsure = false;
    match m.kind {
        InputKind::Note if m.pressed != Some(true) => return false,
        InputKind::Cc => {
            let known = profile.encoders.iter().find(|e| e.input.kind == InputKind::Cc && e.input.matches(&m)).map(|e| e.encoding);
            let key = (ev.port.clone(), m.channel, m.number);
            let still = s.midi.learn.cc_last.get(&key) == Some(&m.raw);
            let seen = s.midi.learn.cc_steps.get(&key).copied().unwrap_or(0) | step_class(m.raw);
            encoding = known.or_else(|| if still { encoder_step(m.raw).map(|guess| refine_encoding(seen, guess)) } else { None });
            if still && encoding.is_none() {
                return false;
            }
            unsure = known.is_none() && !still && encoder_step(m.raw).is_some();
        }
        _ => {}
    }

    let mode = if req.target == GRID {
        MapMode::Grid
    } else {
        let Some(d) = s.controls.get(&req.target) else {
            s.midi.learn.pending = None;
            notice(s, format!("contrôle inconnu : {}", req.target));
            return true;
        };
        match d.kind {
            ControlKind::Trigger => MapMode::Trigger,
            ControlKind::Toggle { .. } => MapMode::Toggle,
            ControlKind::Momentary => MapMode::Momentary,
            ControlKind::Continuous { .. } | ControlKind::Choice { .. } => {
                if matches!(m.kind, InputKind::Note | InputKind::ProgramChange) {
                    let label = d.label_fr.clone();
                    notice(s, format!("« {label} » attend un potard, un fader ou un encodeur : {} ignoré", input_label(&input_of(&m))));
                    return true;
                }
                if unsure {
                    return true;
                }
                if encoding.is_some() {
                    MapMode::Relative
                } else {
                    MapMode::Absolute
                }
            }
        }
    };
    s.midi.learn.pending = None;
    if req.shift && profile.shift_key.is_none() {
        notice(s, format!("Le profil « {} » n'a pas de touche Shift : apprentissage annulé", profile.name));
        return true;
    }

    let grid = req.target == GRID;
    let mapping = Mapping {
        input: input_of(&m),
        shift: req.shift,
        target: if grid { String::new() } else { req.target.clone() },
        args: if grid { req.args.clone() } else { Value::Null },
        mode,
        min: None,
        max: None,
        step: None,
        curve: Curve::Linear,
        // Absolute: the control's own range, soft takeover.
        pickup: mode == MapMode::Absolute,
        encoding: encoding.unwrap_or_default(),
        led: None,
    };
    let pos = (mode == MapMode::Absolute).then_some(m.norm);
    match find_conflict(&profile.mappings, &mapping) {
        // Learning the same control again just updates it.
        Some(i) if same_target(&profile.mappings[i], &mapping) => commit(s, &ev.port, mapping, Some(i), pos),
        Some(i) => {
            let existing = target_label(s, &profile.mappings[i]);
            s.midi.learn.conflict = Some(Conflict { port: ev.port.clone(), mapping, existing, pos });
        }
        None => commit(s, &ev.port, mapping, None, pos),
    }
    true
}

/// `POST /api/midi/learn/confirm {replace}`: answer to « Remplacer
/// l'ancienne affectation ? ».
pub fn confirm(s: &mut Shared, replace: bool) -> Result<(), LearnError> {
    let Some(c) = s.midi.learn.conflict.take() else { return Err((409, "aucune affectation en attente".into())) };
    if !replace {
        notice(s, format!("Affectation conservée : {}", c.existing));
        return Ok(());
    }
    // The profile may have changed meanwhile: look the old one up again.
    let index = profile_for(s, &c.port).and_then(|(_, p)| find_conflict(&p.mappings, &c.mapping));
    commit(s, &c.port, c.mapping, index, c.pos);
    Ok(())
}

/// `POST /api/midi/mapping/delete {port, index}`.
pub fn delete(s: &mut Shared, port: &str, index: usize) -> Result<(), LearnError> {
    if !s.midi.devices.iter().any(|d| d.name == port) {
        return Err((404, format!("appareil MIDI inconnu : {port}")));
    }
    let Some((slug, mut profile)) = profile_for(s, port) else { return Err((404, format!("profil introuvable pour {port}"))) };
    if index >= profile.mappings.len() {
        return Err((404, format!("pas d'affectation n° {index}")));
    }
    profile.mappings.remove(index);
    save(s, port, &slug, profile).map(|_| ()).map_err(|e| (500, e))
}

/// Changes asked for one mapping (`POST /api/midi/mapping/update`, T-211).
#[derive(Clone, Debug, Default)]
pub struct MappingPatch {
    /// `absolute` or `relative`, for a CC on a fader-like control.
    pub mode: Option<MapMode>,
    pub encoding: Option<RelEncoding>,
    /// `Some(None)` removes the LED feedback.
    pub led: Option<Option<LedFeedback>>,
}

/// Edits mapping `index` of `port`'s profile (a built-in one goes to its
/// `-perso` copy, like a learn): CC absolute / relative and its encoding,
/// LED feedback. The target and the message stay as learned.
pub fn update(s: &mut Shared, port: &str, index: usize, patch: MappingPatch) -> Result<(), LearnError> {
    if !s.midi.devices.iter().any(|d| d.name == port) {
        return Err((404, format!("appareil MIDI inconnu : {port}")));
    }
    let Some((slug, mut profile)) = profile_for(s, port) else { return Err((404, format!("profil introuvable pour {port}"))) };
    let Some(mp) = profile.mappings.get_mut(index) else { return Err((404, format!("pas d'affectation n° {index}"))) };
    if let Some(mode) = patch.mode.or(patch.encoding.map(|_| MapMode::Relative)) {
        let fader_like = matches!(mp.mode, MapMode::Absolute | MapMode::Relative);
        if !fader_like || !matches!(mode, MapMode::Absolute | MapMode::Relative) || (mode == MapMode::Relative && mp.input.kind != InputKind::Cc) {
            return Err((400, "seul un CC sur un potard / fader passe d'absolu à relatif".into()));
        }
        mp.mode = mode;
        mp.pickup = mode == MapMode::Absolute;
    }
    if let Some(encoding) = patch.encoding {
        mp.encoding = encoding;
    }
    if let Some(led) = patch.led {
        if led.is_some() && !matches!(mp.input.kind, InputKind::Note | InputKind::Cc) {
            return Err((400, "retour LED : seulement pour une note ou un CC".into()));
        }
        mp.led = led.map(LedFeedback::clamped);
    }
    save(s, port, &slug, profile).map(|_| ()).map_err(|e| (500, e))
}

/// `POST /api/midi/mapping/forget {target}` (« Oublier MIDI »): removes
/// every mapping to this control, on every port. Returns how many.
pub fn forget(s: &mut Shared, target: &str) -> Result<usize, LearnError> {
    let Some(id) = s.controls.get(target).map(|d| d.id.clone()) else { return Err((404, format!("contrôle inconnu : {target}"))) };
    let mut removed = 0;
    let ports: Vec<String> = s.midi.devices.iter().map(|d| d.name.clone()).collect();
    for port in ports {
        let Some((slug, mut profile)) = profile_for(s, &port) else { continue };
        let before = profile.mappings.len();
        profile.mappings.retain(|mp| mp.mode == MapMode::Grid || s.controls.get(&mp.target).is_none_or(|d| d.id != id));
        if profile.mappings.len() != before {
            removed += before - profile.mappings.len();
            save(s, &port, &slug, profile).map_err(|e| (500, e))?;
        }
    }
    Ok(removed)
}

/// `/api/midi` fields: `learn`, `learn_conflict`, `learned`,
/// `learn_notice`, `mappings`.
pub fn state(s: &Shared, now: Instant) -> Vec<(&'static str, Value)> {
    let st = &s.midi.learn;
    let learn = st.pending.as_ref().map(|r| {
        json!({
            "target": if r.target == GRID { GRID } else { &r.target },
            "target_label": request_label(s, r),
            "shift": r.shift,
            "remaining_ms": r.until.saturating_duration_since(now).as_millis() as u64,
        })
    });
    let conflict = st.conflict.as_ref().map(|c| {
        json!({
            "port": c.port,
            "message": input_label(&c.mapping.input),
            "target_label": target_label(s, &c.mapping),
            "existing": c.existing,
        })
    });
    let mut mappings = Vec::new();
    for d in &s.midi.devices {
        let Some(p) = s.midi.store.get(&d.profile) else { continue };
        for (index, mp) in p.mappings.iter().enumerate() {
            mappings.push(json!({
                "port": d.name,
                "profile": d.profile,
                "builtin": s.midi.store.is_builtin(&d.profile),
                "index": index,
                "message": input_label(&mp.input),
                "channel": mp.input.channel,
                "target": if mp.mode == MapMode::Grid { GRID } else { &mp.target },
                "target_label": target_label(s, mp),
                "mode": mp.mode,
                "shift": mp.shift,
                "kind": mp.input.kind,
                "encoding": mp.encoding,
                "led": mp.led,
            }));
        }
    }
    vec![
        ("learn", learn.unwrap_or(Value::Null)),
        ("learn_conflict", conflict.unwrap_or(Value::Null)),
        ("learned", st.learned.as_ref().map_or(Value::Null, |l| json!(l))),
        ("learn_notice", st.notice.as_ref().map_or(Value::Null, |(seq, text)| json!({ "seq": seq, "text": text }))),
        ("mappings", Value::Array(mappings)),
    ]
}

/// « CC 14 », « Note 82 »…
pub fn input_label(i: &MidiInput) -> String {
    match i.kind {
        InputKind::Note => format!("Note {}", i.number),
        InputKind::Cc => format!("CC {}", i.number),
        InputKind::PitchBend => "Pitch bend".into(),
        InputKind::ProgramChange => format!("Programme {}", i.number),
    }
}

fn input_of(m: &Incoming) -> MidiInput {
    MidiInput { kind: m.kind, channel: Some(m.channel), number: if m.kind == InputKind::PitchBend { 0 } else { m.number } }
}

/// A value an endless encoder sends for a small move, if it is one: ±1–3
/// in two's complement (most common: 1–3 / 125–127) or offset 64 (61–63 /
/// 65–67).
fn encoder_step(v: u8) -> Option<RelEncoding> {
    match v {
        1..=3 | 125..=127 => Some(RelEncoding::TwosComplement),
        61..=63 | 65..=67 => Some(RelEncoding::Offset64),
        _ => None,
    }
}

const STEP_PLUS_LOW: u8 = 1; // 1–3: +n (two's complement, sign bit)
const STEP_MINUS_TC: u8 = 2; // 125–127: −n in two's complement
const STEP_BELOW_64: u8 = 4; // 61–63: −n in offset 64
const STEP_ABOVE_64: u8 = 8; // 65–67: +n in offset 64, −n with a sign bit

fn step_class(v: u8) -> u8 {
    match v {
        1..=3 => STEP_PLUS_LOW,
        125..=127 => STEP_MINUS_TC,
        61..=63 => STEP_BELOW_64,
        65..=67 => STEP_ABOVE_64,
        _ => 0,
    }
}

/// The only encoding whose small steps explain every step value seen on
/// this CC, when there is one (an encoder turned both ways: 1–3 and 65–67
/// = sign bit). Otherwise the first guess: values of a single direction
/// fit two encodings, and a fader swept through them fits none. The user
/// can still change it in the mappings list.
fn refine_encoding(seen: u8, guess: RelEncoding) -> RelEncoding {
    let explains = |e: RelEncoding| {
        let ok = match e {
            RelEncoding::TwosComplement => STEP_PLUS_LOW | STEP_MINUS_TC,
            RelEncoding::SignBit => STEP_PLUS_LOW | STEP_ABOVE_64,
            RelEncoding::Offset64 => STEP_BELOW_64 | STEP_ABOVE_64,
        };
        seen & !ok == 0
    };
    let fits: Vec<RelEncoding> = [RelEncoding::TwosComplement, RelEncoding::SignBit, RelEncoding::Offset64].into_iter().filter(|&e| explains(e)).collect();
    match fits[..] {
        [only] => only,
        _ => guess,
    }
}

fn grid_slot(args: &Value) -> Option<usize> {
    args.get("slot")?.as_u64().map(|n| n as usize).filter(|&n| n < GRID_ROWS * GRID_COLS)
}

/// Same hardware control (a `None` channel overlaps every channel) on the
/// same Shift layer.
fn find_conflict(mappings: &[Mapping], new: &Mapping) -> Option<usize> {
    let overlaps = |a: &MidiInput, b: &MidiInput| {
        a.kind == b.kind
            && (a.kind == InputKind::PitchBend || a.number == b.number)
            && (a.channel.is_none() || b.channel.is_none() || a.channel == b.channel)
    };
    mappings.iter().position(|mp| mp.shift == new.shift && overlaps(&mp.input, &new.input))
}

fn same_target(a: &Mapping, b: &Mapping) -> bool {
    a.mode == MapMode::Grid && b.mode == MapMode::Grid && a.args == b.args || a.mode != MapMode::Grid && b.mode != MapMode::Grid && a.target == b.target
}

fn target_label(s: &Shared, mp: &Mapping) -> String {
    if mp.mode == MapMode::Grid {
        return grid_slot(&mp.args).map_or("Grille".into(), |n| format!("Grille, case {}", n + 1));
    }
    s.controls.get(&mp.target).map_or_else(|| format!("{} (inconnu)", mp.target), |d| d.label_fr.clone())
}

fn request_label(s: &Shared, r: &LearnRequest) -> String {
    if r.target == GRID {
        return grid_slot(&r.args).map_or("Grille".into(), |n| format!("Grille, case {}", n + 1));
    }
    s.controls.get(&r.target).map_or_else(|| r.target.clone(), |d| d.label_fr.clone())
}

/// The profile a port uses now, and its slug.
fn profile_for(s: &Shared, port: &str) -> Option<(String, super::profile::Profile)> {
    let slug = s.midi.devices.iter().find(|d| d.name == port).map_or(GENERIC, |d| d.profile.as_str()).to_string();
    let p = s.midi.store.get(&slug)?.clone();
    Some((slug, p))
}

/// Writes an edited profile of `port` (a built-in one goes to a new
/// `-perso` copy, then assigned to the port). Returns the slug written.
fn save(s: &mut Shared, port: &str, slug: &str, profile: super::profile::Profile) -> Result<String, String> {
    let dest = s.midi.store.edit_slug(slug);
    let assign = (dest != slug).then_some(port);
    let written = s.midi.store.save_profile(assign, &dest, profile)?;
    // Used from the next message on; the worker agrees within 250 ms.
    if let Some(d) = s.midi.devices.iter_mut().find(|d| d.name == port) {
        d.profile = written.clone();
    }
    Ok(written)
}

fn commit(s: &mut Shared, port: &str, mapping: Mapping, replace: Option<usize>, pos: Option<f32>) {
    // Re-checked here: the option may have been turned off while waiting.
    if mapping.target == ARM && !s.midi.store.devices.safety.allow_arm {
        return notice(s, ARM_REFUSED.into());
    }
    let Some((slug, mut profile)) = profile_for(s, port) else {
        return notice(s, format!("profil introuvable pour {port} : apprentissage annulé"));
    };
    let mut replaced = None;
    let index = match replace.filter(|&i| i < profile.mappings.len()) {
        Some(i) => {
            let old = &profile.mappings[i];
            if !same_target(old, &mapping) {
                replaced = Some(target_label(s, old));
            }
            // Same button learned again: its LED settings stay.
            let led = if old.input == mapping.input { old.led } else { None };
            profile.mappings[i] = Mapping { led, ..mapping.clone() };
            i
        }
        None => {
            profile.mappings.push(mapping.clone());
            profile.mappings.len() - 1
        }
    };
    let written = match save(s, port, &slug, profile) {
        Ok(w) => w,
        Err(e) => return notice(s, format!("affectation non enregistrée : {e}")),
    };
    if let Some(pos) = pos {
        engine::seed_pickup(s, port, &mapping.target, pos);
    }
    let st = &mut s.midi.learn;
    st.seq += 1;
    let learned = Learned {
        seq: st.seq,
        port: port.to_string(),
        profile: written,
        index,
        message: input_label(&mapping.input),
        channel: mapping.input.channel,
        target: if mapping.mode == MapMode::Grid { GRID.into() } else { mapping.target.clone() },
        target_label: target_label(s, &mapping),
        mode: mapping.mode,
        shift: mapping.shift,
        replaced,
    };
    log::info!("MIDI : {} de {port} → {} ({:?}, profil « {} »)", learned.message, learned.target_label, learned.mode, learned.profile);
    s.midi.learn.learned = Some(learned);
}

fn notice(s: &mut Shared, text: String) {
    log::info!("MIDI : {text}");
    let st = &mut s.midi.learn;
    st.seq += 1;
    st.notice = Some((st.seq, text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::engine::{frame, handle_batch};
    use crate::midi::profile::{Profile, ProfileStore};
    use crate::midi::{MidiMsg, Model};
    use crate::test_support::shared;
    use std::path::PathBuf;

    const PORT: &str = "Test APC40 mkII";

    /// MIDI on, one connected controller on the built-in (empty) generic profile.
    fn setup() -> Shared {
        let mut s = shared();
        s.midi.enabled = true;
        let d = s.midi.device_mut(PORT);
        d.model = Model::Apc40Mk2;
        d.profile = "generic".into();
        d.connected = true;
        d.connected_at = Some(Instant::now());
        s
    }

    fn play(s: &mut Shared, msgs: &[MidiMsg]) {
        let now = Instant::now();
        let evs: Vec<MidiEvent> = msgs.iter().map(|m| MidiEvent { port: PORT.into(), msg: m.clone(), at: now }).collect();
        handle_batch(s, &evs);
        frame(s, now);
    }

    fn on(n: u8) -> MidiMsg {
        MidiMsg::NoteOn { channel: 0, note: n, velocity: 127 }
    }

    fn off(n: u8) -> MidiMsg {
        MidiMsg::NoteOff { channel: 0, note: n }
    }

    fn cc(n: u8, value: u8) -> MidiMsg {
        MidiMsg::Cc { channel: 0, number: n, value }
    }

    fn learn(s: &mut Shared, target: &str) {
        start(s, target, Value::Null, false, Instant::now()).unwrap();
    }

    fn profile(s: &Shared) -> Profile {
        let slug = &s.midi.devices.iter().find(|d| d.name == PORT).unwrap().profile;
        s.midi.store.get(slug).unwrap().clone()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-learn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_fader_learns_absolute_with_pickup_in_a_perso_copy() {
        let mut s = setup();
        s.live.size = 1.0;
        learn(&mut s, "master.size");
        play(&mut s, &[MidiMsg::Cc { channel: 2, number: 7, value: 40 }]);
        assert!(s.midi.learn.pending.is_none());
        assert_eq!(s.live.size, 1.0, "the learned message itself does nothing");
        let p = profile(&s);
        assert_eq!(s.midi.devices[0].profile, "generic-perso");
        assert!(s.midi.store.get("generic").unwrap().mappings.is_empty(), "built-in untouched");
        let mp = &p.mappings[0];
        assert_eq!((mp.input.kind, mp.input.channel, mp.input.number), (InputKind::Cc, Some(2), 7));
        assert_eq!((mp.mode, mp.pickup, mp.target.as_str(), mp.min, mp.max), (MapMode::Absolute, true, "master.size", None, None));
        assert_eq!(s.midi.learn.learned.as_ref().unwrap().message, "CC 7");

        // Pickup starts from the learned position: crossing 50 % takes over.
        play(&mut s, &[MidiMsg::Cc { channel: 2, number: 7, value: 50 }]);
        assert_eq!(s.live.size, 1.0);
        play(&mut s, &[MidiMsg::Cc { channel: 2, number: 7, value: 70 }]);
        assert!((s.live.size - 2.0 * 70.0 / 127.0).abs() < 1e-4, "{}", s.live.size);
        // Another channel is another fader.
        play(&mut s, &[MidiMsg::Cc { channel: 3, number: 7, value: 0 }]);
        assert!((s.live.size - 2.0 * 70.0 / 127.0).abs() < 1e-4);
    }

    #[test]
    fn learning_on_the_apc_layout_keeps_it_in_the_perso_copy() {
        let mut s = setup();
        s.midi.devices[0].profile = "apc40-mk2".into();
        let builtin = s.midi.store.get("apc40-mk2").unwrap().clone();
        assert!(!builtin.mappings.is_empty(), "T-204 layout");
        learn(&mut s, "master.size");
        play(&mut s, &[cc(0x10, 40)]); // a device knob the layout leaves free
        let p = profile(&s);
        assert_eq!(s.midi.devices[0].profile, "apc40-mk2-perso");
        assert_eq!(p.mappings.len(), builtin.mappings.len() + 1);
        assert_eq!((p.shift_key.clone(), p.encoders.clone()), (builtin.shift_key.clone(), builtin.encoders.clone()));
        assert_eq!(s.midi.store.get("apc40-mk2").unwrap(), &builtin, "built-in untouched");
        // Track fader 1 is in the layout: replacing it asks first.
        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x07, 50)]);
        assert!(s.midi.learn.conflict.is_some());
    }

    #[test]
    fn buttons_follow_the_target_type_and_the_press_is_taken() {
        let mut s = setup();
        let audio = |s: &Shared| s.settings.audio.enabled;
        learn(&mut s, "audio.enabled");
        play(&mut s, &[on(0x30), off(0x30)]);
        assert!(!audio(&s), "the learned press didn't toggle");
        assert_eq!(profile(&s).mappings[0].mode, MapMode::Toggle);
        play(&mut s, &[on(0x30), off(0x30)]);
        assert!(audio(&s), "the next press does");

        learn(&mut s, "tempo.tap");
        play(&mut s, &[cc(0x40, 127)]);
        learn(&mut s, "master.rot.reverse");
        play(&mut s, &[on(0x31)]);
        learn(&mut s, "grid.1.1.2");
        play(&mut s, &[on(0x20)]);
        learn(&mut s, "live.pos_x"); // alias: stored under its canonical id
        play(&mut s, &[MidiMsg::PitchBend { channel: 0, value: 9000 }]);
        learn(&mut s, "page.2");
        play(&mut s, &[MidiMsg::ProgramChange { channel: 5, program: 3 }]);
        let got: Vec<(InputKind, u8, String, MapMode)> =
            profile(&s).mappings.iter().map(|m| (m.input.kind, m.input.number, m.target.clone(), m.mode)).collect();
        assert_eq!(
            got[1..],
            [
                (InputKind::Cc, 0x40, "tempo.tap".into(), MapMode::Trigger),
                (InputKind::Note, 0x31, "master.rot.reverse".into(), MapMode::Momentary),
                (InputKind::Note, 0x20, "grid.1.1.2".into(), MapMode::Momentary),
                (InputKind::PitchBend, 0, "master.pos_x".into(), MapMode::Absolute),
                (InputKind::ProgramChange, 3, "page.2".into(), MapMode::Trigger),
            ]
        );
        assert_eq!(s.cue_page, 0, "the learned program change didn't turn the page");
    }

    #[test]
    fn encoders_learn_relative() {
        let mut s = setup();
        let p = Profile::parse(r#"{ "name": "Knobs", "encoders": [{ "kind": "cc", "number": 47 }] }"#).unwrap();
        s.midi.store.save_profile(None, "knobs", p).unwrap();
        s.midi.devices[0].profile = "knobs".into();
        learn(&mut s, "master.pos_x");
        play(&mut s, &[cc(0x2F, 1)]); // declared as an encoder in the profile
        let mp = &profile(&s).mappings[0];
        assert_eq!((mp.mode, mp.encoding, mp.pickup), (MapMode::Relative, RelEncoding::TwosComplement, false));

        // An unknown encoder: a step value waits for the next message; the
        // same value again = encoder.
        learn(&mut s, "master.pos_y");
        play(&mut s, &[cc(0x51, 65)]);
        assert!(s.midi.learn.pending.is_some(), "65 could be a fader");
        play(&mut s, &[cc(0x51, 65)]);
        let mp = &profile(&s).mappings[1];
        assert_eq!((mp.input.number, mp.mode, mp.encoding), (0x51, MapMode::Relative, RelEncoding::Offset64));
        learn(&mut s, "master.rot_x.angle");
        play(&mut s, &[cc(0x53, 127), cc(0x53, 127)]);
        assert_eq!((profile(&s).mappings[2].mode, profile(&s).mappings[2].encoding), (MapMode::Relative, RelEncoding::TwosComplement));
        // A fader moving on from a step value is absolute.
        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x52, 2)]);
        assert!(s.midi.learn.pending.is_some());
        play(&mut s, &[cc(0x52, 4), cc(0x52, 6)]);
        let mp = &profile(&s).mappings[3];
        assert_eq!((mp.input.number, mp.mode, mp.pickup), (0x52, MapMode::Absolute, true));
        // A button target doesn't wait.
        learn(&mut s, "tempo.tap");
        play(&mut s, &[cc(0x54, 127)]);
        assert_eq!(profile(&s).mappings[4].mode, MapMode::Trigger);
    }

    /// T-211: the three relative encodings of unknown encoders, on any
    /// channel. One direction is ambiguous (first guess); both directions
    /// tell, and the mappings list can still change it.
    #[test]
    fn relative_encodings_are_told_apart() {
        let mut s = setup();
        let ch = |channel: u8, n: u8, value: u8| MidiMsg::Cc { channel, number: n, value };
        let learned = |s: &Shared| {
            let p = profile(s);
            let mp = p.mappings.last().unwrap().clone();
            (mp.input.channel, mp.input.number, mp.mode, mp.encoding)
        };
        // Sign bit: turned right (1) earlier, then left (65, 65) while learning.
        play(&mut s, &[ch(9, 0x20, 1), ch(9, 0x20, 1)]);
        learn(&mut s, "master.pos_x");
        play(&mut s, &[ch(9, 0x20, 65), ch(9, 0x20, 65)]);
        assert_eq!(learned(&s), (Some(9), 0x20, MapMode::Relative, RelEncoding::SignBit));
        // Offset 64, turned left: 63 only exists there.
        learn(&mut s, "master.pos_y");
        play(&mut s, &[ch(15, 0x21, 63), ch(15, 0x21, 63)]);
        assert_eq!(learned(&s), (Some(15), 0x21, MapMode::Relative, RelEncoding::Offset64));
        // Two's complement, turned left: 127.
        learn(&mut s, "master.size");
        play(&mut s, &[ch(0, 0x22, 127), ch(0, 0x22, 127)]);
        assert_eq!(learned(&s), (Some(0), 0x22, MapMode::Relative, RelEncoding::TwosComplement));
        // A fader swept through every step value doesn't confuse it.
        let sweep: Vec<MidiMsg> = (0..=127).map(|v| ch(1, 0x23, v)).collect();
        play(&mut s, &sweep);
        learn(&mut s, "master.speed");
        play(&mut s, &[ch(1, 0x23, 2), ch(1, 0x23, 2)]);
        assert_eq!(learned(&s).3, RelEncoding::TwosComplement);

        // Changed by hand afterwards (mappings list), in the same profile.
        let n = profile(&s).mappings.len() - 1;
        let patch = |mode, encoding| MappingPatch { mode, encoding, led: None };
        update(&mut s, PORT, n, patch(None, Some(RelEncoding::SignBit))).unwrap();
        assert_eq!(profile(&s).mappings[n].encoding, RelEncoding::SignBit);
        update(&mut s, PORT, n, patch(Some(MapMode::Absolute), None)).unwrap();
        assert_eq!((profile(&s).mappings[n].mode, profile(&s).mappings[n].pickup), (MapMode::Absolute, true));
        // The engine follows: sign bit 65 = one step down.
        update(&mut s, PORT, 0, patch(None, Some(RelEncoding::SignBit))).unwrap();
        let before = s.live.pos_x;
        play(&mut s, &[ch(9, 0x20, 65)]);
        assert!(s.live.pos_x < before, "{} → {}", before, s.live.pos_x);
    }

    #[test]
    fn mapping_updates_are_checked() {
        let mut s = setup();
        learn(&mut s, "audio.enabled");
        play(&mut s, &[on(0x30)]);
        learn(&mut s, "live.pos_x");
        play(&mut s, &[MidiMsg::PitchBend { channel: 0, value: 9000 }]);
        let led = LedFeedback { off: 0, on: 1, blink: Some(2), present: None };
        let patch = |mode, encoding, led| MappingPatch { mode, encoding, led };
        update(&mut s, PORT, 0, patch(None, None, Some(Some(led)))).unwrap();
        assert_eq!(profile(&s).mappings[0].led, Some(led));
        // Learning the same button again keeps its LED.
        learn(&mut s, "audio.enabled");
        play(&mut s, &[on(0x30)]);
        assert_eq!(profile(&s).mappings[0].led, Some(led));
        update(&mut s, PORT, 0, patch(None, None, Some(None))).unwrap();
        assert_eq!(profile(&s).mappings[0].led, None);
        assert_eq!(update(&mut s, PORT, 0, patch(Some(MapMode::Relative), None, None)).unwrap_err().0, 400, "a toggle stays a toggle");
        assert_eq!(update(&mut s, PORT, 1, patch(None, Some(RelEncoding::Offset64), None)).unwrap_err().0, 400, "pitch bend is never relative");
        assert_eq!(update(&mut s, PORT, 1, patch(None, None, Some(Some(led)))).unwrap_err().0, 400, "no LED on pitch bend");
        assert_eq!(update(&mut s, PORT, 9, MappingPatch::default()).unwrap_err().0, 404);
        assert_eq!(update(&mut s, "nope", 0, MappingPatch::default()).unwrap_err().0, 404);
        assert!(s.midi.store.get("generic").unwrap().mappings.is_empty(), "built-in untouched");
    }

    #[test]
    fn what_is_not_learned() {
        let mut s = setup();
        let mut p = Profile::parse(r#"{ "name": "Pad", "shift_key": { "kind": "note", "number": 98 } }"#).unwrap();
        p.mappings.clear();
        s.midi.store.save_profile(None, "pad", p).unwrap();
        s.midi.devices[0].profile = "pad".into();
        play(&mut s, &[cc(0x10, 30)]);
        learn(&mut s, "master.size");
        play(&mut s, &[MidiMsg::Clock, MidiMsg::Start, off(3), on(98), off(98), cc(0x10, 30), MidiMsg::SysEx { bytes: vec![0xF0, 0xF7] }]);
        assert!(s.midi.learn.pending.is_some(), "real-time, Note Off, Shift, a still CC, SysEx: nothing learned");
        play(&mut s, &[on(3)]);
        assert!(s.midi.learn.pending.is_some(), "a note can't drive a fader");
        assert!(s.midi.learn.notice.as_ref().unwrap().1.contains("Note 3 ignoré"));
        assert!(profile(&s).mappings.is_empty());
        play(&mut s, &[cc(0x10, 31)]);
        assert_eq!(profile(&s).mappings.len(), 1);
    }

    #[test]
    fn an_existing_mapping_is_only_replaced_when_confirmed() {
        let mut s = setup();
        learn(&mut s, "master.size");
        play(&mut s, &[cc(0x07, 10)]);
        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x07, 20)]);
        let c = s.midi.learn.conflict.as_ref().unwrap();
        assert_eq!(c.existing, "Taille maître");
        assert_eq!(profile(&s).mappings.len(), 1);
        confirm(&mut s, false).unwrap();
        assert_eq!(profile(&s).mappings[0].target, "master.size");
        assert!(confirm(&mut s, true).is_err(), "nothing pending any more");

        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x07, 30)]);
        confirm(&mut s, true).unwrap();
        let p = profile(&s);
        assert_eq!((p.mappings.len(), p.mappings[0].target.as_str()), (1, "master.speed"));
        assert_eq!(s.midi.learn.learned.as_ref().unwrap().replaced.as_deref(), Some("Taille maître"));

        // Same control again: updated without asking. Shift is another layer.
        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x07, 40)]);
        assert!(s.midi.learn.conflict.is_none());
        assert_eq!(profile(&s).mappings.len(), 1);
    }

    #[test]
    fn learning_expires_after_15_s() {
        let mut s = setup();
        let t0 = Instant::now();
        start(&mut s, "master.size", Value::Null, false, t0).unwrap();
        frame(&mut s, t0 + Duration::from_secs(14));
        assert!(s.midi.learn.pending.is_some());
        frame(&mut s, t0 + LEARN_TIMEOUT);
        assert!(s.midi.learn.pending.is_none());
        assert!(s.midi.learn.notice.as_ref().unwrap().1.contains("aucun message reçu en 15 s"));
        // A late message is an ordinary one.
        handle_batch(&mut s, &[MidiEvent { port: PORT.into(), msg: cc(7, 10), at: t0 + LEARN_TIMEOUT }]);
        assert!(profile(&s).mappings.is_empty());

        start(&mut s, "master.size", Value::Null, false, t0).unwrap();
        cancel(&mut s);
        assert!(s.midi.learn.pending.is_none());
        assert_eq!(s.midi.learn.notice.as_ref().unwrap().1, "Apprentissage MIDI annulé");
    }

    #[test]
    fn arm_is_learned_only_with_the_option_and_never_arms() {
        let mut s = setup();
        assert_eq!(start(&mut s, "transport.arm", Value::Null, false, Instant::now()).unwrap_err().0, 403);
        assert!(s.midi.learn.pending.is_none());

        s.midi.store.set_safety(crate::midi::safety::MidiSafety { allow_arm: true, blackout_on_disconnect: false }).unwrap();
        // The generic profile has no Shift key: refused.
        learn(&mut s, "transport.arm");
        assert!(s.midi.learn.pending.as_ref().unwrap().shift, "arming is always a Shift mapping");
        play(&mut s, &[on(0x5B)]);
        assert!(profile(&s).mappings.is_empty());
        assert!(s.midi.learn.notice.as_ref().unwrap().1.contains("pas de touche Shift"));

        let mut p = profile(&s);
        p.shift_key = Some(MidiInput { kind: InputKind::Note, channel: None, number: 0x62 });
        s.midi.store.save_profile(None, "shifty", p).unwrap();
        s.midi.devices[0].profile = "shifty".into();
        learn(&mut s, "transport.arm");
        play(&mut s, &[on(0x62), on(0x5B)]);
        assert!(!s.gate.is_armed(), "learning never arms");
        let mp = &profile(&s).mappings[0];
        assert_eq!((mp.target.as_str(), mp.shift), ("transport.arm", true));

        // Armed by hand: learning something else leaves it armed.
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        learn(&mut s, "master.size");
        play(&mut s, &[cc(0x07, 30)]);
        assert!(s.gate.is_armed(), "learning never disarms either");

        // Option turned off while waiting: refused at capture.
        learn(&mut s, "transport.arm");
        s.midi.store.devices.safety.allow_arm = false;
        play(&mut s, &[on(0x5C)]);
        assert_eq!(profile(&s).mappings.len(), 2);
        assert!(s.gate.is_armed());
    }

    #[test]
    fn shift_learning_needs_a_shift_key() {
        let mut s = setup();
        start(&mut s, "master.reset", Value::Null, true, Instant::now()).unwrap();
        play(&mut s, &[on(0x40)]);
        assert!(profile(&s).mappings.is_empty());
        assert!(s.midi.learn.notice.as_ref().unwrap().1.contains("Shift"));
    }

    #[test]
    fn bad_requests() {
        let mut s = setup();
        assert_eq!(start(&mut s, "nope", Value::Null, false, Instant::now()).unwrap_err().0, 404);
        assert_eq!(start(&mut s, GRID, json!({ "slot": 40 }), false, Instant::now()).unwrap_err().0, 400);
        start(&mut s, GRID, json!({ "slot": 39 }), false, Instant::now()).unwrap();
        play(&mut s, &[on(0x00)]);
        let mp = &profile(&s).mappings[0];
        assert_eq!((mp.mode, mp.args.clone()), (MapMode::Grid, json!({ "slot": 39 })));
        s.midi.enabled = false;
        assert_eq!(start(&mut s, "master.size", Value::Null, false, Instant::now()).unwrap_err().0, 409);
    }

    #[test]
    fn delete_forget_and_restart() {
        let dir = temp_dir("persist");
        let mut s = setup();
        s.midi.store = ProfileStore::load(dir.clone());
        // An older copy exists: never overwritten, the next free name is used.
        s.midi.store.save_profile(None, "generic-perso", Profile::parse(r#"{ "name": "old" }"#).unwrap()).unwrap();
        learn(&mut s, "master.size");
        play(&mut s, &[cc(0x07, 10)]);
        learn(&mut s, "master.speed");
        play(&mut s, &[cc(0x08, 10)]);
        learn(&mut s, "master.size");
        play(&mut s, &[on(0x09)]);
        s.midi.learn.pending = None;
        assert_eq!(s.midi.devices[0].profile, "generic-perso-2");
        assert_eq!(s.midi.store.get("generic-perso").unwrap().name, "old");

        let reloaded = ProfileStore::load(dir.clone());
        assert_eq!(reloaded.choose(PORT, Model::Apc40Mk2).slug, "generic-perso-2");
        assert_eq!(reloaded.get("generic-perso-2").unwrap().mappings.len(), 2);

        assert_eq!(delete(&mut s, PORT, 5).unwrap_err().0, 404);
        assert_eq!(delete(&mut s, "Nope", 0).unwrap_err().0, 404);
        delete(&mut s, PORT, 1).unwrap();
        assert_eq!(profile(&s).mappings.len(), 1);
        assert_eq!(forget(&mut s, "live.size").unwrap(), 1, "alias of master.size");
        assert!(profile(&s).mappings.is_empty());
        assert_eq!(forget(&mut s, "nope").unwrap_err().0, 404);
        assert!(ProfileStore::load(dir.clone()).get("generic-perso-2").unwrap().mappings.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn state_lists_mappings_and_the_pending_learn() {
        let mut s = setup();
        learn(&mut s, "master.size");
        play(&mut s, &[cc(0x07, 10)]);
        learn(&mut s, "master.speed");
        let st: serde_json::Map<String, Value> = state(&s, Instant::now()).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(st["learn"]["target_label"], "Vitesse d'animation");
        assert!(st["learn"]["remaining_ms"].as_u64().unwrap() > 14_000);
        let m = &st["mappings"][0];
        assert_eq!((m["message"].as_str(), m["target_label"].as_str(), m["mode"].as_str()), (Some("CC 7"), Some("Taille maître"), Some("absolute")));
        assert_eq!(st["learned"]["profile"], "generic-perso");
    }
}
