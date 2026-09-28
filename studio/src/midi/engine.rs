//! The mapping engine (T-202) and its safety rules (T-208): turns MIDI
//! events into control changes through `controls::apply(…, true)`, the same
//! entry point as `/api/control`. There is no MIDI-only control path; the
//! one exception is the opt-in "Shift + hold" arming gesture, which sets
//! arming itself after its own checks, through the interlock gate (see `frame`).
//!
//! Runs under the `Shared` lock (worker batch, engine frame), so it is
//! cheap: a linear scan of the port's mappings per event, no I/O. Faders
//! and encoders are coalesced: their last value per control is written
//! once per engine frame (`frame`), however many CCs arrive.

use super::mapping::{pickup_catches, Incoming, InputKind, MapMode, Mapping};
use super::profile::{Profile, GENERIC};
use super::safety::{self, ARM, ARM_HOLD, BLACKOUT, PLUG_GUARD};
use super::{learn, MidiEvent};
use crate::controls::{self, ControlInput, ControlKind, GRID_COLS, GRID_ROWS};
use crate::Shared;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// One hardware control of one port.
type Key = (String, InputKind, u8, u8);

/// What a held button does when released. Decided at press time, so a
/// release goes to the same control even if Shift or the cue page changed.
#[derive(Debug, PartialEq)]
enum Held {
    Nothing,
    /// Momentary / grid: send 0 to this control.
    Release(String),
    /// Shift + arm button, counting towards `ARM_HOLD`.
    Arm,
}

#[derive(Debug)]
struct Pickup {
    /// Last physical position (0..1), to see the fader cross the value.
    prev: Option<f32>,
    engaged: bool,
    /// What we last wrote: if the value moved since, someone else (UI,
    /// another control, a reset) changed it and the fader must catch it again.
    written: Option<f32>,
}

#[derive(Debug)]
struct ArmHold {
    key: Key,
    since: Instant,
}

/// Runtime state (never saved): Shift, held buttons, pickup, pending writes.
#[derive(Default)]
pub struct MapState {
    /// Ports whose Shift key is held.
    shift: HashSet<String>,
    held: HashMap<Key, Held>,
    /// Per (port, control id).
    pickup: HashMap<(String, String), Pickup>,
    /// Toggle state for controls that don't report a value.
    toggles: HashMap<String, bool>,
    /// Coalesced continuous writes: (control id, native value).
    pending: Vec<(String, f32)>,
    arm_hold: Option<ArmHold>,
    warned: HashSet<String>,
    /// Ignored mappings (unknown ids…), in French, for `/api/midi`.
    pub warnings: Vec<String>,
    /// Number of `controls::apply` calls, for tests.
    writes: u64,
    /// Control ids in the order they were applied (tests only).
    #[cfg(test)]
    applied: Vec<String>,
}

/// A batch of events from the worker. Blackout goes first: any press that
/// maps to it is applied before anything else in the batch.
pub fn handle_batch(s: &mut Shared, events: &[MidiEvent]) {
    let mut shift = s.midi.map.shift.clone();
    let mut blackout = false;
    for ev in events {
        let Some(m) = Incoming::from_msg(&ev.msg) else { continue };
        let Some(profile) = profile_of(s, &ev.port) else { continue };
        if is_shift_key(profile, &m) {
            if m.pressed == Some(true) {
                shift.insert(ev.port.clone());
            } else {
                shift.remove(&ev.port);
            }
        } else if m.pressed == Some(true) {
            let target = find(s, profile, &m, shift.contains(&ev.port)).map(|mp| mp.target.as_str());
            blackout |= target.and_then(|t| s.controls.get(t)).is_some_and(|d| d.id == BLACKOUT);
        }
    }
    if blackout {
        do_blackout(s);
    }
    for ev in events {
        // MIDI learn (T-203) takes the message it binds: it doesn't act.
        let learned = learn::capture(s, ev);
        learn::observe(s, ev);
        if !learned {
            handle_event(s, ev);
        }
    }
}

/// Once per engine frame: flush coalesced fader/encoder writes and finish
/// a Shift + arm hold that lasted long enough.
pub fn frame(s: &mut Shared, now: Instant) {
    learn::expire(s, now);
    if s.midi.map.pending.is_empty() && s.midi.map.arm_hold.is_none() {
        return;
    }
    for (id, v) in std::mem::take(&mut s.midi.map.pending) {
        send(s, &id, v);
    }
    let Some(hold) = s.midi.map.arm_hold.as_ref() else { return };
    if now.saturating_duration_since(hold.since) < ARM_HOLD {
        return;
    }
    let port = hold.key.0.clone();
    let still_held = s.midi.map.held.get(&hold.key) == Some(&Held::Arm) && s.midi.map.shift.contains(&port);
    s.midi.map.arm_hold = None;
    let connected = s.midi.devices.iter().any(|d| d.name == port && d.connected);
    if still_held && connected && s.midi.store.devices.safety.allow_arm {
        match s.request_arm_midi_opt_in() {
            Ok(()) => log::warn!("MIDI : laser armé depuis {port} (Shift + maintien)"),
            Err(why) => log::warn!("MIDI : armement refusé depuis {port} : {}", why.join(" ; ")),
        }
    }
}

/// The controller went away (unplugged or disabled): drop its Shift,
/// held buttons, pickup and any arming in progress. The studio state stays
/// as it is, unless the user asked for a blackout on disconnect.
pub fn port_closed(s: &mut Shared, port: &str, unplugged: bool) {
    let map = &mut s.midi.map;
    map.shift.remove(port);
    map.held.retain(|k, _| k.0 != port);
    map.pickup.retain(|k, _| k.0 != port);
    if map.arm_hold.as_ref().is_some_and(|h| h.key.0 == port) {
        map.arm_hold = None;
    }
    // A hold-to-run pad on a closed port can never send its release.
    s.presence.set_midi_hold(false);
    if unplugged && s.midi.store.devices.safety.blackout_on_disconnect {
        log::warn!("MIDI : {port} déconnecté, blackout");
        do_blackout(s);
    }
}

pub(super) fn profile_of<'a>(s: &'a Shared, port: &str) -> Option<&'a Profile> {
    let slug = s.midi.devices.iter().find(|d| d.name == port).map_or(GENERIC, |d| d.profile.as_str());
    s.midi.store.get(slug)
}

pub(super) fn is_shift_key(p: &Profile, m: &Incoming) -> bool {
    p.shift_key.as_ref().is_some_and(|k| k.matches(m))
}

/// Shift held: Shift mappings first, then the normal ones. While arming
/// from MIDI is off, mappings to the arm control are skipped, so Shift +
/// Stop All (the default profiles' arm gesture) stays a blackout.
fn find<'a>(s: &Shared, p: &'a Profile, m: &Incoming, shift: bool) -> Option<&'a Mapping> {
    let arm_off = !s.midi.store.devices.safety.allow_arm;
    let usable = |mp: &Mapping| !(arm_off && s.controls.get(&mp.target).is_some_and(|d| d.id == ARM));
    let layer = |sh: bool| p.mappings.iter().find(|mp| mp.shift == sh && mp.input.matches(m) && usable(mp));
    if shift {
        layer(true).or_else(|| layer(false))
    } else {
        layer(false)
    }
}

fn handle_event(s: &mut Shared, ev: &MidiEvent) {
    let Some(m) = Incoming::from_msg(&ev.msg) else { return };
    let Some(profile) = profile_of(s, &ev.port) else { return };
    if is_shift_key(profile, &m) {
        if m.pressed == Some(true) {
            s.midi.map.shift.insert(ev.port.clone());
        } else {
            s.midi.map.shift.remove(&ev.port);
        }
        return;
    }
    let shift = s.midi.map.shift.contains(&ev.port);
    let Some(mp) = find(s, profile, &m, shift).cloned() else {
        release(s, &(ev.port.clone(), m.kind, m.channel, m.number), &m);
        return;
    };
    let key: Key = (ev.port.clone(), m.kind, m.channel, m.number);
    if release(s, &key, &m) {
        return;
    }

    let id = if mp.mode == MapMode::Grid {
        match grid_cell(s.cue_page, &mp.args) {
            Some(id) => id,
            None => return warn(s, format!("grid:{}", mp.args), format!("correspondance « grille » sans « slot » valide ({})", mp.args)),
        }
    } else {
        match s.controls.get(&mp.target) {
            Some(d) => d.id.clone(),
            None => return warn(s, mp.target.clone(), format!("contrôle « {} » inconnu : correspondance ignorée", mp.target)),
        }
    };

    let press = m.pressed == Some(true);
    if id == BLACKOUT {
        if press {
            do_blackout(s);
        }
        return;
    }
    if id == ARM {
        if press && !s.midi.map.held.contains_key(&key) {
            arm_press(s, ev, key, shift);
        }
        return;
    }

    match mp.mode {
        MapMode::Trigger | MapMode::Toggle | MapMode::Momentary | MapMode::Grid => {
            if !press || s.midi.map.held.contains_key(&key) {
                return;
            }
            let held = match mp.mode {
                MapMode::Trigger => {
                    send(s, &id, 1.0);
                    Held::Nothing
                }
                MapMode::Toggle => {
                    let on = !value_now(s, &id).map_or_else(|| s.midi.map.toggles.get(&id).copied().unwrap_or(false), |v| v >= 0.5);
                    s.midi.map.toggles.insert(id.clone(), on);
                    send(s, &id, if on { 1.0 } else { 0.0 });
                    Held::Nothing
                }
                // Momentary, Grid. An empty grid slot just does nothing.
                _ => {
                    if send_quiet(s, &id, 1.0) {
                        Held::Release(id)
                    } else {
                        Held::Nothing
                    }
                }
            };
            // Program Change has no release: never "held".
            if m.kind != InputKind::ProgramChange {
                s.midi.map.held.insert(key, held);
            }
        }
        MapMode::Absolute => {
            if m.kind == InputKind::Note && !press {
                return; // Note Off: the key went up, not to zero
            }
            absolute(s, &ev.port, &id, &mp, &m);
        }
        MapMode::Relative => {
            if m.kind == InputKind::Cc {
                relative(s, &id, &mp, m.raw);
            }
        }
    }
}

/// A release of a held button. True if it was one (and is now handled).
fn release(s: &mut Shared, key: &Key, m: &Incoming) -> bool {
    if m.pressed != Some(false) {
        return false;
    }
    match s.midi.map.held.remove(key) {
        None => false,
        Some(Held::Release(id)) => {
            send_quiet(s, &id, 0.0);
            true
        }
        Some(Held::Arm) => {
            if s.midi.map.arm_hold.as_ref().is_some_and(|h| &h.key == key) {
                s.midi.map.arm_hold = None;
            }
            true
        }
        Some(Held::Nothing) => true,
    }
}

/// A mapping just learned from a fader at `pos`: pickup starts from there,
/// so moving it on across the value takes over.
pub(super) fn seed_pickup(s: &mut Shared, port: &str, id: &str, pos: f32) {
    s.midi.map.pickup.insert((port.to_string(), id.to_string()), Pickup { prev: Some(pos), engaged: false, written: None });
}

/// "grid" slot n (row-major from the top left) on the current page.
fn grid_cell(page: usize, args: &serde_json::Value) -> Option<String> {
    let slot = args.get("slot")?.as_u64()? as usize;
    (slot < GRID_ROWS * GRID_COLS).then(|| format!("grid.{}.{}.{}", page + 1, slot / GRID_COLS + 1, slot % GRID_COLS + 1))
}

fn arm_press(s: &mut Shared, ev: &MidiEvent, key: Key, shift: bool) {
    let refuse = if !s.midi.store.devices.safety.allow_arm {
        Some("armement depuis le contrôleur désactivé (réglage Sécurité)")
    } else if !shift {
        Some("armement depuis le contrôleur : il faut maintenir Shift")
    } else if s.midi.devices.iter().find(|d| d.name == ev.port).and_then(|d| d.connected_at).is_none_or(|at| ev.at < at + PLUG_GUARD) {
        Some("armement refusé : contrôleur branché depuis moins de 5 s")
    } else {
        None
    };
    match refuse {
        Some(why) => {
            log::info!("MIDI : {why}");
            s.midi.map.held.insert(key, Held::Nothing);
        }
        None => {
            s.midi.map.arm_hold = Some(ArmHold { key: key.clone(), since: ev.at });
            s.midi.map.held.insert(key, Held::Arm);
        }
    }
}

fn do_blackout(s: &mut Shared) {
    s.midi.map.arm_hold = None;
    send(s, BLACKOUT, 1.0);
}

/// Native range a mapping drives: its own `min`/`max`, else the control's.
/// Choices count their options. `None` for buttons.
fn range(kind: &ControlKind, mp: &Mapping) -> Option<(f32, f32)> {
    let (lo, hi) = match kind {
        ControlKind::Continuous { min, max, .. } => (*min, *max),
        ControlKind::Choice { options, .. } => (0.0, options.len().saturating_sub(1) as f32),
        _ => return None,
    };
    Some((mp.min.unwrap_or(lo), mp.max.unwrap_or(hi)))
}

fn absolute(s: &mut Shared, port: &str, id: &str, mp: &Mapping, m: &Incoming) {
    let Some(kind) = s.controls.get(id).map(|d| d.kind.clone()) else { return };
    let range = range(&kind, mp);
    let pos = m.norm;
    let native = match range {
        Some((lo, hi)) => lo + (hi - lo) * mp.curve.apply(pos),
        None => pos,
    };
    let native = settle(&kind, id, native);

    // Brightness is always in pickup, whatever the profile says (T-208).
    if mp.pickup || safety::is_brightness(id) {
        let current = value_now(s, id);
        let span = range.map_or(1.0, |(lo, hi)| (hi - lo).abs().max(f32::EPSILON));
        let target = current.map(|v| match range {
            Some((lo, hi)) if hi != lo => mp.curve.inverse((v - lo) / (hi - lo)),
            _ => v,
        });
        let seed = seed_position(s, port, m);
        let p = s.midi.map.pickup.entry((port.to_string(), id.to_string())).or_insert(Pickup { prev: seed, engaged: false, written: None });
        if p.engaged && p.written.zip(current).is_some_and(|(w, c)| (w - c).abs() > 0.01 * span) {
            p.engaged = false;
        }
        if !p.engaged {
            p.engaged = target.is_none_or(|t| pickup_catches(p.prev, pos, t));
        }
        p.prev = Some(pos);
        if !p.engaged {
            return;
        }
        p.written = Some(native);
    }
    queue(s, id, native);
}

fn relative(s: &mut Shared, id: &str, mp: &Mapping, raw: u8) {
    let delta = mp.encoding.delta(raw);
    let Some(kind) = s.controls.get(id).map(|d| d.kind.clone()) else { return };
    if delta == 0 {
        return;
    }
    let Some((lo, hi)) = range(&kind, mp) else {
        return warn(s, format!("rel:{id}"), format!("encodeur relatif sur « {id} », qui n'a pas de plage : ignoré"));
    };
    let default_step = if matches!(kind, ControlKind::Choice { .. }) { 1.0 } else { (hi - lo).abs() / 127.0 };
    let base = value_now(s, id).unwrap_or(lo);
    let v = (base + delta as f32 * mp.step.unwrap_or(default_step)).clamp(lo.min(hi), lo.max(hi));
    let v = settle(&kind, id, v);
    queue(s, id, v);
}

/// The value the control will really take: within its range, choices on a
/// whole index, brightness under the safety cap.
fn settle(kind: &ControlKind, id: &str, v: f32) -> f32 {
    let v = match kind {
        ControlKind::Continuous { min, max, .. } => v.clamp(*min, *max),
        ControlKind::Choice { .. } => v.round().max(0.0),
        _ => v,
    };
    if safety::is_brightness(id) {
        v.min(safety::BRIGHTNESS_MAX)
    } else {
        v
    }
}

/// APC40 mkII fader positions reported at connect time (T-201), so pickup
/// knows where a fader is before it moves.
fn seed_position(s: &Shared, port: &str, m: &Incoming) -> Option<f32> {
    if m.kind != InputKind::Cc {
        return None;
    }
    let index = match (m.number, m.channel) {
        (0x07, c) if c < 8 => c as usize,
        (0x0E, _) => 8,
        _ => return None,
    };
    let faders = s.midi.devices.iter().find(|d| d.name == port)?.faders?;
    Some(faders[index] as f32 / 127.0)
}

/// Current value, including a write still waiting for the next frame.
fn value_now(s: &Shared, id: &str) -> Option<f32> {
    if let Some((_, v)) = s.midi.map.pending.iter().find(|(p, _)| p == id) {
        return Some(*v);
    }
    let desc = s.controls.get(id)?;
    match controls::current(s, desc)? {
        serde_json::Value::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        v => v.as_f64().map(|v| v as f32),
    }
}

fn queue(s: &mut Shared, id: &str, v: f32) {
    let pending = &mut s.midi.map.pending;
    match pending.iter_mut().find(|(p, _)| p == id) {
        Some(entry) => entry.1 = v,
        None => pending.push((id.to_string(), v)),
    }
}

/// Every MIDI write goes through here: the shared control entry point, as
/// an external caller (it can't arm), brightness capped.
fn send(s: &mut Shared, id: &str, v: f32) {
    if let Err(e) = apply(s, id, v) {
        warn(s, id.to_string(), format!("« {id} » : {e:?}"));
    }
}

/// Same, without a warning (an empty grid slot is normal).
fn send_quiet(s: &mut Shared, id: &str, v: f32) -> bool {
    apply(s, id, v).is_ok()
}

fn apply(s: &mut Shared, id: &str, v: f32) -> Result<(), controls::ControlError> {
    let v = if safety::is_brightness(id) { v.min(safety::BRIGHTNESS_MAX) } else { v };
    s.midi.map.writes += 1;
    #[cfg(test)]
    s.midi.map.applied.push(id.to_string());
    controls::apply(s, id, ControlInput::Value(v), true)
}

/// Logged and listed once per key, never a panic.
fn warn(s: &mut Shared, key: String, msg: String) {
    let map = &mut s.midi.map;
    if map.warned.insert(key) {
        log::warn!("MIDI : {msg}");
        if map.warnings.len() < 20 {
            map.warnings.push(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::mapping::{Curve, MidiInput, RelEncoding};
    use crate::midi::MidiMsg;
    use crate::test_support::shared;
    use serde_json::json;
    use std::time::Duration;

    const PORT: &str = "Test APC";

    fn mapping(kind: InputKind, channel: Option<u8>, number: u8, target: &str, mode: MapMode) -> Mapping {
        Mapping {
            input: MidiInput { kind, channel, number },
            shift: false,
            target: target.into(),
            args: serde_json::Value::Null,
            mode,
            min: None,
            max: None,
            step: None,
            curve: Curve::Linear,
            pickup: false,
            encoding: RelEncoding::TwosComplement,
        }
    }

    fn note(n: u8, target: &str, mode: MapMode) -> Mapping {
        mapping(InputKind::Note, None, n, target, mode)
    }

    fn cc(n: u8, target: &str, mode: MapMode) -> Mapping {
        mapping(InputKind::Cc, None, n, target, mode)
    }

    /// A connected device on `PORT`, plugged in at `t0`, using a profile
    /// with Shift on note 0x62.
    fn setup(mappings: Vec<Mapping>, t0: Instant) -> Shared {
        let mut s = shared();
        let mut p = Profile::parse(r#"{ "name": "Test", "shift_key": { "kind": "note", "number": 98 } }"#).unwrap();
        p.mappings = mappings;
        s.midi.store.save_profile(None, "test", p).unwrap();
        let d = s.midi.device_mut(PORT);
        d.profile = "test".into();
        d.connected = true;
        d.connected_at = Some(t0);
        s
    }

    fn ev_at(msg: MidiMsg, at: Instant) -> MidiEvent {
        MidiEvent { port: PORT.into(), msg, at }
    }

    fn send_msgs(s: &mut Shared, msgs: &[MidiMsg]) {
        let now = Instant::now();
        let evs: Vec<MidiEvent> = msgs.iter().map(|m| ev_at(m.clone(), now)).collect();
        handle_batch(s, &evs);
    }

    fn on(n: u8) -> MidiMsg {
        MidiMsg::NoteOn { channel: 0, note: n, velocity: 127 }
    }

    fn off(n: u8) -> MidiMsg {
        MidiMsg::NoteOff { channel: 0, note: n }
    }

    fn cc_msg(n: u8, value: u8) -> MidiMsg {
        MidiMsg::Cc { channel: 0, number: n, value }
    }

    fn tick(s: &mut Shared) {
        frame(s, Instant::now());
    }

    #[test]
    fn absolute_cc_lands_on_the_next_frame_with_bounds_and_log_curve() {
        let mut s = setup(vec![cc(20, "master.size", MapMode::Absolute), Mapping { curve: Curve::Log, ..cc(21, "master.speed", MapMode::Absolute) }], Instant::now());
        send_msgs(&mut s, &[cc_msg(20, 127)]);
        assert_eq!(s.live.size, 1.0, "not before the frame");
        tick(&mut s);
        assert_eq!(s.live.size, 2.0);
        send_msgs(&mut s, &[cc_msg(20, 0)]);
        tick(&mut s);
        assert_eq!(s.live.size, 0.0);
        send_msgs(&mut s, &[cc_msg(21, 64)]);
        tick(&mut s);
        assert!(s.live.speed > 0.0 && s.live.speed < 0.5, "log: half travel is well under half range ({})", s.live.speed);
        send_msgs(&mut s, &[cc_msg(21, 127)]);
        tick(&mut s);
        assert!((s.live.speed - 4.0).abs() < 1e-4);
    }

    #[test]
    fn mapping_range_override() {
        let mut s = setup(vec![Mapping { min: Some(0.5), max: Some(1.0), ..cc(20, "master.size", MapMode::Absolute) }], Instant::now());
        send_msgs(&mut s, &[cc_msg(20, 0)]);
        tick(&mut s);
        assert_eq!(s.live.size, 0.5);
    }

    #[test]
    fn hundred_ccs_make_one_write() {
        let mut s = setup(vec![cc(20, "master.size", MapMode::Absolute)], Instant::now());
        let msgs: Vec<MidiMsg> = (0..100).map(|i| cc_msg(20, i)).collect();
        for m in &msgs {
            send_msgs(&mut s, std::slice::from_ref(m)); // one per worker batch
        }
        assert_eq!(s.midi.map.writes, 0);
        tick(&mut s);
        assert_eq!(s.midi.map.writes, 1);
        assert!((s.live.size - 2.0 * 99.0 / 127.0).abs() < 1e-5);
        tick(&mut s);
        assert_eq!(s.midi.map.writes, 1, "nothing left to write");
    }

    #[test]
    fn relative_encoder_steps_and_stays_in_bounds() {
        // Cue Level (CC 0x2F), APC two's complement.
        let mut s = setup(vec![Mapping { min: Some(-1.0), max: Some(1.0), step: Some(0.1), ..cc(0x2F, "master.pos_y", MapMode::Relative) }], Instant::now());
        send_msgs(&mut s, &[cc_msg(0x2F, 1), cc_msg(0x2F, 2)]);
        tick(&mut s);
        assert!((s.live.pos_y - 0.3).abs() < 1e-5, "{}", s.live.pos_y);
        send_msgs(&mut s, &[cc_msg(0x2F, 127)]);
        tick(&mut s);
        assert!((s.live.pos_y - 0.2).abs() < 1e-5);
        send_msgs(&mut s, &[cc_msg(0x2F, 63)]);
        tick(&mut s);
        assert_eq!(s.live.pos_y, 1.0, "bounded at max");
        send_msgs(&mut s, &[cc_msg(0x2F, 64), cc_msg(0x2F, 64)]);
        tick(&mut s);
        assert_eq!(s.live.pos_y, -1.0, "bounded at min");
    }

    #[test]
    fn relative_encodings_and_default_step() {
        let mut s = setup(
            vec![
                Mapping { step: Some(0.5), encoding: RelEncoding::Offset64, ..cc(13, "tempo.bpm", MapMode::Relative) },
                Mapping { encoding: RelEncoding::SignBit, ..cc(14, "master.color.palette", MapMode::Relative) },
            ],
            Instant::now(),
        );
        s.tempo.bpm = 120.0;
        send_msgs(&mut s, &[cc_msg(13, 66), cc_msg(14, 1)]);
        tick(&mut s);
        assert_eq!(s.tempo.bpm, 121.0);
        assert_eq!(s.live.color_params.palette, 1, "choices step by one option");
        send_msgs(&mut s, &[cc_msg(13, 62), cc_msg(14, 65)]);
        tick(&mut s);
        assert_eq!(s.tempo.bpm, 120.0);
        assert_eq!(s.live.color_params.palette, 0);
    }

    #[test]
    fn pickup_waits_for_the_fader_to_cross_the_value() {
        let mut s = setup(vec![Mapping { pickup: true, ..cc(20, "master.perspective", MapMode::Absolute) }], Instant::now());
        s.live.perspective = 0.2;
        send_msgs(&mut s, &[cc_msg(20, 127), cc_msg(20, 100), cc_msg(20, 60)]);
        tick(&mut s);
        assert_eq!(s.live.perspective, 0.2, "fader at 100 %, 79 %, 47 %: nothing");
        // Crosses 20 % going down.
        send_msgs(&mut s, &[cc_msg(20, 10)]);
        tick(&mut s);
        assert!((s.live.perspective - 10.0 / 127.0).abs() < 1e-5);
        send_msgs(&mut s, &[cc_msg(20, 90)]);
        tick(&mut s);
        assert!((s.live.perspective - 90.0 / 127.0).abs() < 1e-5, "engaged: follows");

        // Changed elsewhere (the UI): must be caught again, going up.
        s.live.perspective = 0.9;
        send_msgs(&mut s, &[cc_msg(20, 50)]);
        tick(&mut s);
        assert_eq!(s.live.perspective, 0.9);
        send_msgs(&mut s, &[cc_msg(20, 127)]);
        tick(&mut s);
        assert_eq!(s.live.perspective, 1.0, "crossed 90 % going up");
    }

    #[test]
    fn pickup_catches_within_three_percent() {
        let mut s = setup(vec![Mapping { pickup: true, ..cc(20, "master.perspective", MapMode::Absolute) }], Instant::now());
        s.live.perspective = 0.5;
        send_msgs(&mut s, &[cc_msg(20, 68)]); // 53.5 %: outside
        tick(&mut s);
        assert_eq!(s.live.perspective, 0.5);
        let mut s = setup(vec![Mapping { pickup: true, ..cc(20, "master.perspective", MapMode::Absolute) }], Instant::now());
        s.live.perspective = 0.5;
        send_msgs(&mut s, &[cc_msg(20, 66)]); // 52 %: inside
        tick(&mut s);
        assert!((s.live.perspective - 66.0 / 127.0).abs() < 1e-5);
    }

    #[test]
    fn shift_layer_with_fallback() {
        let mut s = setup(
            vec![note(0x63, "tempo.tap", MapMode::Trigger), Mapping { shift: true, ..note(0x63, "tempo.resync", MapMode::Trigger) }, note(0x5B, "page.next", MapMode::Trigger)],
            Instant::now(),
        );
        s.tempo.bpm = 100.0;
        send_msgs(&mut s, &[on(0x62), on(0x5B), off(0x5B)]);
        assert_eq!(s.cue_page, 1, "no Shift mapping: the normal one");
        let taps = |s: &Shared| s.midi.map.writes;
        let before = taps(&s);
        send_msgs(&mut s, &[on(0x63), off(0x63)]);
        assert_eq!(taps(&s), before + 1);
        send_msgs(&mut s, &[off(0x62), on(0x5B)]);
        assert_eq!(s.cue_page, 2);
    }

    #[test]
    fn shift_picks_the_shift_action_and_plain_press_the_normal_one() {
        let mut s = setup(vec![note(0x52, "page.3", MapMode::Trigger), Mapping { shift: true, ..note(0x52, "page.8", MapMode::Trigger) }], Instant::now());
        send_msgs(&mut s, &[on(0x52), off(0x52)]);
        assert_eq!(s.cue_page, 2);
        send_msgs(&mut s, &[on(0x62), on(0x52), off(0x52), off(0x62)]);
        assert_eq!(s.cue_page, 7);
        send_msgs(&mut s, &[on(0x52)]);
        assert_eq!(s.cue_page, 2);
    }

    #[test]
    fn fixed_channel_is_part_of_the_key() {
        let mut s = setup(
            vec![mapping(InputKind::Cc, Some(1), 7, "master.size", MapMode::Absolute), mapping(InputKind::Cc, None, 7, "master.perspective", MapMode::Absolute)],
            Instant::now(),
        );
        send_msgs(&mut s, &[MidiMsg::Cc { channel: 1, number: 7, value: 127 }, MidiMsg::Cc { channel: 5, number: 7, value: 127 }]);
        tick(&mut s);
        assert_eq!(s.live.size, 2.0);
        assert_eq!(s.live.perspective, 1.0, "the any-channel mapping took channel 5");
    }

    #[test]
    fn trigger_fires_once_per_press_and_cc_buttons_work() {
        let mut s = setup(vec![cc(0x40, "page.next", MapMode::Trigger)], Instant::now());
        send_msgs(&mut s, &[cc_msg(0x40, 127), cc_msg(0x40, 127), cc_msg(0x40, 0)]);
        assert_eq!(s.cue_page, 1, "repeated 127 is still the same press");
        send_msgs(&mut s, &[cc_msg(0x40, 127)]);
        assert_eq!(s.cue_page, 2);
    }

    #[test]
    fn toggle_flips_on_each_press() {
        let mut s = setup(vec![note(0x30, "audio.enabled", MapMode::Toggle)], Instant::now());
        send_msgs(&mut s, &[on(0x30), off(0x30)]);
        assert!(s.settings.audio.enabled);
        send_msgs(&mut s, &[on(0x30), off(0x30)]);
        assert!(!s.settings.audio.enabled);
    }

    #[test]
    fn momentary_releases_and_grid_follows_the_page() {
        let mut s = setup(vec![note(0x31, "master.rot.reverse", MapMode::Momentary), Mapping { args: json!({ "slot": 0 }), ..note(0x20, "", MapMode::Grid) }], Instant::now());
        send_msgs(&mut s, &[on(0x31)]);
        assert!(s.live.rot_reverse);
        send_msgs(&mut s, &[off(0x31)]);
        assert!(!s.live.rot_reverse);

        s.cue_page = 1;
        send_msgs(&mut s, &[on(0x20)]);
        let first = s.presets.iter().find(|p| p.category == crate::presets::CATEGORIES[1]).unwrap().id.clone();
        assert_eq!(s.active_cue.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn momentary_flash_cue_stops_on_release_even_after_a_page_change() {
        let mut s = setup(vec![Mapping { args: json!({ "slot": 1 }), ..note(0x21, "", MapMode::Grid) }, note(0x5E, "page.next", MapMode::Trigger)], Instant::now());
        let cue = s.presets.iter().filter(|p| p.category == crate::presets::CATEGORIES[0]).nth(1).unwrap().id.clone();
        s.deck.set_slot(&cue, crate::cues::CueSlot { mode: Some(crate::cues::ClickMode::Flash), group: None, layer: None, show: None });
        send_msgs(&mut s, &[on(0x21)]);
        assert_eq!(s.active_cue.as_deref(), Some(cue.as_str()));
        send_msgs(&mut s, &[on(0x5E), off(0x5E), off(0x21)]);
        assert!(s.deck.active.is_empty(), "the flash stopped on release");
    }

    #[test]
    fn unknown_targets_are_ignored_with_one_warning() {
        let mut s = setup(vec![note(1, "master.nope", MapMode::Trigger), Mapping { args: json!({}), ..note(2, "", MapMode::Grid) }], Instant::now());
        send_msgs(&mut s, &[on(1), off(1), on(1), on(2)]);
        assert_eq!(s.midi.map.warnings.len(), 2, "{:?}", s.midi.map.warnings);
        assert!(s.midi.map.warnings[0].contains("master.nope"));
    }

    #[test]
    fn unmapped_and_realtime_messages_are_harmless() {
        let mut s = setup(vec![], Instant::now());
        send_msgs(&mut s, &[on(1), MidiMsg::Clock, MidiMsg::SysEx { bytes: vec![0xF0, 0xF7] }, MidiMsg::PitchBend { channel: 0, value: 9000 }]);
        tick(&mut s);
        assert_eq!(s.midi.map.writes, 0);
        // Unknown port: generic profile, no mappings.
        handle_batch(&mut s, &[MidiEvent { port: "ghost".into(), msg: on(1), at: Instant::now() }]);
    }

    #[test]
    fn pitch_bend_and_program_change() {
        let mut s = setup(
            vec![mapping(InputKind::PitchBend, None, 0, "master.perspective", MapMode::Absolute), mapping(InputKind::ProgramChange, None, 4, "page.next", MapMode::Trigger)],
            Instant::now(),
        );
        send_msgs(&mut s, &[MidiMsg::PitchBend { channel: 3, value: 16383 }, MidiMsg::ProgramChange { channel: 0, program: 4 }, MidiMsg::ProgramChange { channel: 0, program: 4 }]);
        tick(&mut s);
        assert_eq!(s.live.perspective, 1.0);
        assert_eq!(s.cue_page, 2, "program change has no release: each one fires");
    }

    // ---------- T-208 safety ----------

    const STOP_ALL: u8 = 0x51;
    const SHIFT: u8 = 0x62;

    fn apc_like() -> Vec<Mapping> {
        vec![
            note(STOP_ALL, "transport.blackout", MapMode::Trigger),
            Mapping { shift: true, ..note(STOP_ALL, "transport.arm", MapMode::Trigger) },
            Mapping { args: json!({ "slot": 0 }), ..note(0x20, "", MapMode::Grid) },
            Mapping { pickup: true, ..cc(0x0E, "master.brightness", MapMode::Absolute) },
            mapping(InputKind::Cc, Some(0), 0x07, "master.size", MapMode::Absolute),
        ]
    }

    #[test]
    fn a_batch_with_blackout_ends_disarmed() {
        let mut s = setup(apc_like(), Instant::now());
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        send_msgs(&mut s, &[on(0x20), MidiMsg::Cc { channel: 0, number: 7, value: 100 }, on(STOP_ALL)]);
        tick(&mut s);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn blackout_is_processed_first_in_its_batch() {
        let mut s = setup(apc_like(), Instant::now());
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        send_msgs(&mut s, &[on(0x20), MidiMsg::Cc { channel: 0, number: 0x0E, value: 90 }, on(STOP_ALL)]);
        assert_eq!(s.midi.map.applied.first().map(String::as_str), Some(BLACKOUT), "{:?}", s.midi.map.applied);
        assert!(!s.gate.is_armed());
        // Arming from MIDI off (the default): Shift + Stop All has nothing
        // to arm, so it stays the blackout.
        let mut s = setup(apc_like(), Instant::now());
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        send_msgs(&mut s, &[on(SHIFT), on(STOP_ALL)]);
        assert!(!s.gate.is_armed(), "Shift + Stop All is a blackout while arming is off");
        assert_eq!(s.midi.map.applied.first().map(String::as_str), Some(BLACKOUT));
        // Opted in: a Shift press earlier in the same batch turns Stop All
        // into the arm button: no blackout then (and no arming yet).
        let mut s = setup(apc_like(), Instant::now());
        s.midi.store.devices.safety.allow_arm = true;
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        send_msgs(&mut s, &[on(SHIFT), on(STOP_ALL)]);
        assert!(s.gate.is_armed(), "Shift + Stop All is the arm gesture once opted in");
        assert!(!s.midi.map.applied.iter().any(|id| id == BLACKOUT || id == ARM));
    }

    /// Plugged in 10 s ago, so the plug guard is over.
    fn plugged_long_ago(allow_arm: bool) -> (Shared, Instant) {
        let t0 = Instant::now();
        let mut s = setup(apc_like(), t0);
        s.midi.store.devices.safety.allow_arm = allow_arm;
        (s, t0 + Duration::from_secs(10))
    }

    fn batch_at(s: &mut Shared, msgs: &[MidiMsg], at: Instant) {
        let evs: Vec<MidiEvent> = msgs.iter().map(|m| ev_at(m.clone(), at)).collect();
        handle_batch(s, &evs);
    }

    #[test]
    fn arming_is_refused_by_default_for_every_message() {
        let (mut s, t) = plugged_long_ago(false);
        let mut all = Vec::new();
        for ch in 0..16u8 {
            for n in 0..128u8 {
                all.push(MidiMsg::NoteOn { channel: ch, note: n, velocity: 127 });
                all.push(MidiMsg::Cc { channel: ch, number: n, value: 127 });
                all.push(MidiMsg::ProgramChange { channel: ch, program: n });
            }
        }
        for shift_first in [false, true] {
            let mut msgs = Vec::new();
            if shift_first {
                msgs.push(on(SHIFT));
            }
            msgs.extend(all.iter().cloned());
            batch_at(&mut s, &msgs, t);
            frame(&mut s, t + Duration::from_secs(5));
            assert!(!s.gate.is_armed(), "no MIDI combination arms by default (shift first: {shift_first})");
        }
        batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t + Duration::from_secs(6));
        frame(&mut s, t + Duration::from_secs(9));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn opt_in_arming_needs_shift_and_a_one_second_hold() {
        let (mut s, t) = plugged_long_ago(true);
        batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t);
        frame(&mut s, t + Duration::from_millis(900));
        assert!(!s.gate.is_armed(), "not yet");
        batch_at(&mut s, &[off(STOP_ALL)], t + Duration::from_millis(950));
        frame(&mut s, t + Duration::from_millis(1500));
        assert!(!s.gate.is_armed(), "released before 1 s: nothing");

        batch_at(&mut s, &[on(STOP_ALL)], t + Duration::from_secs(2));
        frame(&mut s, t + Duration::from_millis(3100));
        assert!(s.gate.is_armed(), "held 1.1 s with Shift");

        // Without Shift, Stop All is the blackout.
        batch_at(&mut s, &[off(STOP_ALL), off(SHIFT), on(STOP_ALL)], t + Duration::from_secs(4));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn letting_go_of_shift_cancels_arming() {
        let (mut s, t) = plugged_long_ago(true);
        batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t);
        batch_at(&mut s, &[off(SHIFT)], t + Duration::from_millis(500));
        frame(&mut s, t + Duration::from_secs(2));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn no_arming_in_the_first_five_seconds() {
        let t0 = Instant::now();
        let mut s = setup(apc_like(), t0);
        s.midi.store.devices.safety.allow_arm = true;
        batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t0 + Duration::from_secs(3));
        frame(&mut s, t0 + Duration::from_secs(5));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn unplugging_cancels_arming_and_can_black_out() {
        let (mut s, t) = plugged_long_ago(true);
        batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t);
        port_closed(&mut s, PORT, true);
        frame(&mut s, t + Duration::from_secs(2));
        assert!(!s.gate.is_armed(), "arming in progress dropped");

        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        port_closed(&mut s, PORT, true);
        assert!(s.gate.is_armed(), "state stays by default");
        s.midi.store.devices.safety.blackout_on_disconnect = true;
        port_closed(&mut s, PORT, false);
        assert!(s.gate.is_armed(), "disabling the port by hand is not a disconnect");
        port_closed(&mut s, PORT, true);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn master_fader_at_top_on_connect_leaves_brightness_alone() {
        // Pickup forced even though this profile forgot it.
        let mut s = setup(vec![cc(0x0E, "master.brightness", MapMode::Absolute)], Instant::now());
        s.live.brightness = 0.4;
        send_msgs(&mut s, &[cc_msg(0x0E, 127)]);
        tick(&mut s);
        assert_eq!(s.live.brightness, 0.4);
        send_msgs(&mut s, &[cc_msg(0x0E, 50)]); // crossed 40 %
        tick(&mut s);
        assert!((s.live.brightness - 50.0 / 127.0).abs() < 1e-5);
    }

    #[test]
    fn mk2_reported_fader_position_seeds_pickup() {
        let mut s = setup(apc_like(), Instant::now());
        s.live.brightness = 0.5;
        s.midi.device_mut(PORT).faders = Some([0, 0, 0, 0, 0, 0, 0, 0, 127]);
        // Reported at the top; first move lands at 30 %: it crossed 50 %.
        send_msgs(&mut s, &[cc_msg(0x0E, 38)]);
        tick(&mut s);
        assert!((s.live.brightness - 38.0 / 127.0).abs() < 1e-5);
    }

    #[test]
    fn midi_brightness_never_exceeds_the_safety_maximum() {
        let mut s = setup(
            vec![
                Mapping { max: Some(5.0), ..cc(0x0E, "live.brightness", MapMode::Absolute) },
                Mapping { step: Some(1.0), ..cc(0x2F, "look.brightness", MapMode::Relative) },
                note(1, "master.brightness", MapMode::Toggle),
            ],
            Instant::now(),
        );
        s.live.brightness = 0.99;
        for v in [126, 127] {
            send_msgs(&mut s, &[cc_msg(0x0E, v)]);
            tick(&mut s);
            assert!(s.live.brightness <= safety::BRIGHTNESS_MAX);
        }
        send_msgs(&mut s, &[cc_msg(0x2F, 63)]);
        tick(&mut s);
        assert!(s.settings.brightness <= safety::BRIGHTNESS_MAX);
        send_msgs(&mut s, &[on(1), off(1), on(1)]);
        assert!(s.live.brightness <= safety::BRIGHTNESS_MAX);
    }

    #[test]
    fn builtin_profiles_use_pickup_on_every_absolute_mapping() {
        let store = crate::midi::profile::ProfileStore::in_memory();
        for info in store.list() {
            for mp in &store.get(&info.slug).unwrap().mappings {
                assert!(mp.mode != MapMode::Absolute || mp.pickup, "{}: {} without pickup", info.slug, mp.target);
            }
        }
    }

    // --- T-204: the built-in APC40 / APC40 mkII layouts ---

    const APC_PROFILES: [&str; 2] = ["apc40-mk2", "apc40"];

    /// A connected device on `PORT` using a built-in profile, plugged in
    /// long ago (past the plug guard).
    fn builtin(slug: &str) -> Shared {
        let mut s = shared();
        let d = s.midi.device_mut(PORT);
        d.profile = slug.into();
        d.connected = true;
        d.connected_at = Some(Instant::now() - Duration::from_secs(10));
        s
    }

    fn channels_overlap(a: Option<u8>, b: Option<u8>) -> bool {
        a.is_none() || b.is_none() || a == b
    }

    #[test]
    fn builtin_apc_profiles_only_use_registered_controls() {
        let s = shared();
        for slug in APC_PROFILES {
            let p = s.midi.store.get(slug).unwrap();
            assert_eq!(p.host_mode, 0x41, "{slug}");
            assert_eq!(p.shift_key, Some(MidiInput { kind: InputKind::Note, channel: None, number: 0x62 }), "{slug}");
            let mut slots = Vec::new();
            for mp in &p.mappings {
                if mp.mode == MapMode::Grid {
                    slots.push(grid_cell(0, &mp.args).unwrap_or_else(|| panic!("{slug}: bad grid args {}", mp.args)));
                    continue;
                }
                let d = s.controls.get(&mp.target).unwrap_or_else(|| panic!("{slug}: unknown control « {} »", mp.target));
                assert_eq!(d.id, mp.target, "{slug}: use the canonical id, not an alias");
                // The one non-external target is the opt-in arm gesture: Shift only.
                assert!(d.external || (d.id == ARM && mp.shift), "{slug}: « {} » can't be driven from a controller", mp.target);
                if let (Some(lo), Some(hi), ControlKind::Continuous { min, max, .. }) = (mp.min, mp.max, &d.kind) {
                    assert!(lo >= *min && hi <= *max && lo < hi, "{slug}: {} range {lo}..{hi}", mp.target);
                }
                if mp.mode == MapMode::Relative {
                    let declared = p.encoders.iter().any(|e| e.input.number == mp.input.number);
                    assert!(declared && mp.input.kind == InputKind::Cc, "{slug}: {} on an undeclared encoder", mp.target);
                }
            }
            slots.sort();
            slots.dedup();
            assert_eq!(slots.len(), GRID_ROWS * GRID_COLS, "{slug}: every pad has its own slot");
            // No two mappings can answer the same message in the same layer.
            for (i, a) in p.mappings.iter().enumerate() {
                for b in &p.mappings[i + 1..] {
                    let same = a.shift == b.shift && a.input.kind == b.input.kind && a.input.number == b.input.number && channels_overlap(a.input.channel, b.input.channel);
                    assert!(!same, "{slug}: {:?} (shift {}) mapped twice: {} / {}", a.input, a.shift, a.target, b.target);
                }
            }
            // Nothing but Shift + Stop All targets arming.
            let arm: Vec<_> = p.mappings.iter().filter(|mp| mp.target == ARM).collect();
            assert!(arm.len() == 1 && arm[0].shift && arm[0].input.number == STOP_ALL, "{slug}");
        }
    }

    /// Slot of the grid pad that answers `msg` in `slug`.
    fn slot_of(s: &Shared, slug: &str, msg: MidiMsg) -> Option<u64> {
        let m = Incoming::from_msg(&msg)?;
        let p = s.midi.store.get(slug)?;
        let mut hits = p.mappings.iter().filter(|mp| mp.mode == MapMode::Grid && !mp.shift && mp.input.matches(&m));
        let slot = hits.next()?.args["slot"].as_u64();
        assert!(hits.next().is_none());
        slot
    }

    #[test]
    fn grid_notes_map_to_slots_from_the_top_left_on_both_models() {
        let s = shared();
        // (row from the top, column) → slot: the 4 corners and the centre.
        for (r, c) in [(0u8, 0u8), (0, 7), (4, 0), (4, 7), (2, 3), (2, 4)] {
            let slot = Some((r * 8 + c) as u64);
            // mkII: bottom row = notes 0–7, any channel.
            for ch in [0, 5] {
                assert_eq!(slot_of(&s, "apc40-mk2", MidiMsg::NoteOn { channel: ch, note: 0x20 + c - 8 * r, velocity: 127 }), slot, "mkII ({r},{c})");
            }
            // APC40: note 0x35 + row, channel = column.
            assert_eq!(slot_of(&s, "apc40", MidiMsg::NoteOn { channel: c, note: 0x35 + r, velocity: 127 }), slot, "APC40 ({r},{c})");
        }
        assert_eq!(slot_of(&s, "apc40-mk2", on(0x27)), Some(7), "top right");
        assert_eq!(slot_of(&s, "apc40-mk2", on(0x07)), Some(39), "bottom right");
        assert_eq!(slot_of(&s, "apc40", MidiMsg::NoteOn { channel: 7, note: 0x39, velocity: 127 }), Some(39));
        assert_eq!(slot_of(&s, "apc40", on(0x28)), None, "mkII notes above the grid aren't pads on the APC40");
    }

    fn pad_msg(slug: &str, r: u8, c: u8) -> MidiMsg {
        if slug == "apc40" {
            MidiMsg::NoteOn { channel: c, note: 0x35 + r, velocity: 127 }
        } else {
            on(0x20 + c - 8 * r)
        }
    }

    /// Press and release a pad.
    fn tap_pad(s: &mut Shared, slug: &str, r: u8, c: u8) {
        let MidiMsg::NoteOn { channel, note, .. } = pad_msg(slug, r, c) else { unreachable!() };
        send_msgs(s, &[pad_msg(slug, r, c), MidiMsg::NoteOff { channel, note }]);
    }

    #[test]
    fn default_layout_plays_pads_changes_pages_and_blacks_out() {
        for slug in APC_PROFILES {
            let mut s = builtin(slug);
            tap_pad(&mut s, slug, 0, 0);
            assert!(s.midi.map.applied.contains(&"grid.1.1.1".to_string()), "{slug}: top-left = 1st cue");
            assert_eq!(s.midi.map.applied.first().map(String::as_str), Some("grid.1.1.1"), "{slug}: top-left = 1st cue");
            tap_pad(&mut s, slug, 4, 7);
            assert_eq!(s.midi.map.applied.last().map(String::as_str), Some("grid.1.5.8"), "{slug}: bottom-right = 40th cue");

            send_msgs(&mut s, &[on(0x54), off(0x54)]);
            assert_eq!(s.cue_page, 2, "{slug}: scene launch 3 = page 3");
            send_msgs(&mut s, &[on(SHIFT), on(0x53), off(0x53), off(SHIFT)]);
            assert_eq!(s.cue_page, 6, "{slug}: Shift + scene launch 2 = page 7");
            send_msgs(&mut s, &[on(0x5F), off(0x5F)]);
            assert_eq!(s.cue_page, 7, "{slug}: Down = next page");
            tap_pad(&mut s, slug, 0, 0);
            assert_eq!(s.midi.map.applied.last().map(String::as_str), Some("grid.8.1.1"), "{slug}: the grid follows the page");

            s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
            send_msgs(&mut s, &[on(STOP_ALL)]);
            assert!(!s.gate.is_armed() && s.estop.is_latched(), "{slug}: Stop All = blackout, e-stop latched");

            // Shift + Stop All held long enough: still nothing while the
            // option is off, and it's a blackout too.
            let mut s = builtin(slug);
            s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
            let t = Instant::now();
            batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t);
            frame(&mut s, t + Duration::from_secs(3));
            assert!(!s.gate.is_armed() && s.estop.is_latched(), "{slug}: Shift + Stop All is a blackout by default");
        }
    }

    #[test]
    fn default_layout_arms_only_with_the_opt_in_gesture() {
        for slug in APC_PROFILES {
            let mut s = builtin(slug);
            s.midi.store.devices.safety.allow_arm = true;
            let t = Instant::now();
            batch_at(&mut s, &[on(SHIFT), on(STOP_ALL)], t);
            frame(&mut s, t + Duration::from_millis(1100));
            assert!(s.gate.is_armed(), "{slug}: opted in, Shift + Stop All held 1 s arms (preview)");
            batch_at(&mut s, &[off(STOP_ALL), off(SHIFT), on(STOP_ALL)], t + Duration::from_secs(2));
            assert!(!s.gate.is_armed(), "{slug}: Stop All alone is still the blackout");
        }
    }

    #[test]
    fn default_layout_faders_knobs_and_tempo() {
        for slug in APC_PROFILES {
            let mut s = builtin(slug);
            // Master fader: pickup (brightness 100 %, fader reported/assumed low).
            send_msgs(&mut s, &[cc_msg(0x0E, 20)]);
            tick(&mut s);
            assert_eq!(s.live.brightness, 1.0, "{slug}: no jump");
            send_msgs(&mut s, &[cc_msg(0x0E, 127), cc_msg(0x0E, 64)]);
            tick(&mut s);
            assert!((s.live.brightness - 64.0 / 127.0).abs() < 1e-5, "{slug}: caught at the top, then follows");
            // Track fader 1 = master size (default 1.0 = middle).
            send_msgs(&mut s, &[cc_msg(0x07, 64), cc_msg(0x07, 127)]);
            tick(&mut s);
            assert_eq!(s.live.size, 2.0, "{slug}");
            // Shift + track fader 2 = layer 2 dimmer (default 100 %).
            send_msgs(&mut s, &[on(SHIFT), MidiMsg::Cc { channel: 1, number: 0x07, value: 127 }, MidiMsg::Cc { channel: 1, number: 0x07, value: 0 }, off(SHIFT)]);
            tick(&mut s);
            assert_eq!(s.mixer.layer(2).dimmer, 0.0, "{slug}");
            assert_eq!(s.live.size_x, 1.0, "{slug}: fader 2 without Shift not touched");
            // Activator 3 mutes layer 3, Solo 1 solos layer 1.
            send_msgs(&mut s, &[MidiMsg::NoteOn { channel: 2, note: 0x32, velocity: 127 }, MidiMsg::NoteOn { channel: 0, note: 0x31, velocity: 127 }]);
            assert!(s.mixer.layer(3).mute && s.mixer.layer(1).solo, "{slug}");
            // Nudge: + advances the phase on both (the notes are swapped).
            let plus = if slug == "apc40" { 0x64 } else { 0x65 };
            send_msgs(&mut s, &[on(plus), off(plus)]);
            assert_eq!(s.midi.map.applied.last().map(String::as_str), Some("tempo.nudge_up"), "{slug}");
            // BPM encoder: mkII tempo knob, APC40 Shift + Cue Level.
            let bpm = s.tempo.bpm;
            let knob = if slug == "apc40" { vec![on(SHIFT), cc_msg(0x2F, 2), off(SHIFT)] } else { vec![cc_msg(0x0D, 2)] };
            send_msgs(&mut s, &knob);
            tick(&mut s);
            assert!((s.tempo.bpm - (bpm + 1.0)).abs() < 1e-6, "{slug}: 2 steps of 0.5 BPM ({} → {})", bpm, s.tempo.bpm);
            // Tap and the footswitch both tap.
            send_msgs(&mut s, &[on(0x63), off(0x63), cc_msg(0x40, 127), cc_msg(0x40, 0)]);
            assert_eq!(s.midi.map.applied.iter().filter(|id| *id == "tempo.tap").count(), 2, "{slug}");
        }
    }
}
