//! The on-screen APC40 / APC40 mkII (T-210): a drawing of the controller
//! in « RÉGLAGES › Contrôleur » that shows what each button does, lights
//! like the real one and can be clicked, with or without a controller.
//!
//! - Layout: our own geometric drawing, from the button list of Akai's
//!   public protocol documents (docs/research/midi-apc40.md §1–2). No
//!   artwork. Positions are in drawing units; the page scales the SVG.
//! - Mappings and LEDs come from the profile the real device uses when an
//!   APC is connected ("attached"), else from the virtual port's own
//!   profile (the built-in one for the chosen model unless learned over).
//!   LEDs are `led::render`, exactly what the device is sent.
//! - A click becomes the MIDI message that button sends, fed to the
//!   mapping engine on its own port (`PORT`): same mappings, Shift, held
//!   buttons and pickup as the hardware, but `engine::handle_screen`
//!   records it as the UI and never arms (only the arm button and Space do).

use super::engine;
use super::learn;
use super::led;
use super::mapping::{Incoming, InputKind, MapMode, Mapping};
use super::profile::Driver;
use super::{MidiDevice, MidiEvent, MidiMsg, Model};
use crate::controls::{self, GRID_COLS};
use crate::Shared;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::time::Instant;

/// The on-screen controller's port name: never a CoreMIDI port, never
/// opened. Its Shift, held buttons and pickup are its own.
pub const PORT: &str = "Laser Studio · contrôleur à l'écran";

/// How a drawn element is played.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    /// Clip launch pad (APC40: green / red / yellow; mkII: RGB).
    Pad,
    /// Scene Launch: round, mkII RGB.
    Round,
    /// Button with a LED.
    Button,
    /// Button without a LED (Shift, Tap, arrows, Nudge, Stop All…).
    Key,
    /// Vertical fader, absolute CC.
    Fader,
    /// The crossfader, absolute CC.
    Crossfader,
    /// Knob with a LED ring, absolute CC.
    Knob,
    /// Endless encoder, relative CC (Cue Level, Tempo).
    Encoder,
}

impl Widget {
    fn is_button(self) -> bool {
        matches!(self, Widget::Pad | Widget::Round | Widget::Button | Widget::Key)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Element {
    /// Stable within a model: `pad.<row>.<col>`, `fader.3`, `shift`…
    pub id: String,
    /// The button's name, as printed on the hardware or a plain French one.
    pub label: String,
    pub widget: Widget,
    /// Note or CC, on `channel` (0–15), number `number`.
    pub kind: InputKind,
    pub channel: u8,
    pub number: u8,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Drawing units.
const X0: f32 = 16.0;
const PITCH: f32 = 56.0;
const ROW: f32 = 38.0;
const PAD_W: f32 = 48.0;
const PAD_H: f32 = 30.0;
const KNOB: f32 = 40.0;
const FADER_H: f32 = 130.0;

pub const NOTE_SHIFT: u8 = 0x62;
pub const NOTE_STOP_ALL: u8 = 0x51;

fn el(id: impl Into<String>, label: impl Into<String>, widget: Widget, kind: InputKind, channel: u8, number: u8, (x, y, w, h): (f32, f32, f32, f32)) -> Element {
    Element { id: id.into(), label: label.into(), widget, kind, channel, number, x, y, w, h }
}

fn note(id: impl Into<String>, label: impl Into<String>, widget: Widget, channel: u8, number: u8, rect: (f32, f32, f32, f32)) -> Element {
    el(id, label, widget, InputKind::Note, channel, number, rect)
}

fn cc(id: impl Into<String>, label: impl Into<String>, widget: Widget, channel: u8, number: u8, rect: (f32, f32, f32, f32)) -> Element {
    el(id, label, widget, InputKind::Cc, channel, number, rect)
}

/// Every control of the model, with where it is drawn. Anything but an
/// APC40 is drawn as a mkII (the default when nothing is connected).
pub fn elements(model: Model) -> Vec<Element> {
    let mk2 = model != Model::Apc40;
    let mut v = Vec::new();
    let col = |c: u8| X0 + c as f32 * PITCH;
    let scene_x = col(8) + 8.0;

    // Left: (mkII) top knobs, clip grid + Scene Launch, track rows, faders.
    let mut y = 14.0;
    if mk2 {
        for c in 0..8u8 {
            v.push(cc(format!("knob.top.{}", c + 1), format!("Potard {} (haut)", c + 1), Widget::Knob, 0, 0x30 + c, (col(c) + 4.0, y, KNOB, KNOB)));
        }
        y += 52.0;
    }
    for r in 0..5u8 {
        let ry = y + r as f32 * ROW;
        for c in 0..8u8 {
            // mkII: notes from the bottom left (§2.2); APC40: note = row, channel = column.
            let (ch, n) = if mk2 { (0, 32 + c - 8 * r) } else { (c, 0x35 + r) };
            v.push(note(format!("pad.{}.{}", r + 1, c + 1), format!("Pad {}·{}", r + 1, c + 1), Widget::Pad, ch, n, (col(c), ry, PAD_W, PAD_H)));
        }
        v.push(note(format!("scene.{}", r + 1), format!("Scene Launch {}", r + 1), Widget::Round, 0, 0x52 + r, (scene_x, ry, PAD_W, PAD_H)));
    }
    y += 5.0 * ROW + 4.0;
    let mut rows: Vec<(u8, &str, &str)> =
        vec![(0x34, "stop", "Clip Stop"), (0x33, "select", "Track Select"), (0x32, "activator", "Activator"), (0x31, "solo", "Solo"), (0x30, "rec", "Rec Arm")];
    if mk2 {
        rows.push((0x42, "ab", "Crossfader A/B"));
    }
    for (n, id, label) in rows {
        for c in 0..8u8 {
            v.push(note(format!("{id}.{}", c + 1), format!("{label} {}", c + 1), Widget::Button, c, n, (col(c), y, PAD_W, 18.0)));
        }
        match n {
            0x34 => v.push(note("stop_all", "Stop All Clips", Widget::Key, 0, NOTE_STOP_ALL, (scene_x, y, PAD_W, 18.0))),
            0x33 => v.push(note("master", "Master", Widget::Button, 0, 0x50, (scene_x, y, PAD_W, 18.0))),
            _ => {}
        }
        y += 26.0;
    }
    y += 8.0;
    for c in 0..8u8 {
        v.push(cc(format!("fader.{}", c + 1), format!("Fader {}", c + 1), Widget::Fader, c, 0x07, (col(c) + 14.0, y, 20.0, FADER_H)));
    }
    v.push(cc("fader.master", "Fader master", Widget::Fader, 0, 0x0E, (scene_x + 14.0, y, 20.0, FADER_H)));
    let left_bottom = y + FADER_H;

    // Right: knobs, device buttons, banks, transport, tempo, arrows, Shift, crossfader.
    let x1 = scene_x + PAD_W + 40.0;
    let kx = |i: u8| x1 + (i % 4) as f32 * PITCH;
    let mut y = 14.0;
    let knobs = |v: &mut Vec<Element>, y: &mut f32, prefix: &str, label: &str, base: u8| {
        for i in 0..8u8 {
            v.push(cc(format!("knob.{prefix}.{}", i + 1), format!("{label} {}", i + 1), Widget::Knob, 0, base + i, (kx(i) + 4.0, *y + (i / 4) as f32 * 52.0, KNOB, KNOB)));
        }
        *y += 112.0;
    };
    if !mk2 {
        // APC40: the track control knobs sit top right.
        knobs(&mut v, &mut y, "top", "Potard piste", 0x30);
    }
    knobs(&mut v, &mut y, "dev", "Potard device", 0x10);
    let device: [&str; 8] = if mk2 {
        ["Device ◀", "Device ▶", "Bank ◀", "Bank ▶", "Device on/off", "Device Lock", "Clip/Device View", "Detail View"]
    } else {
        ["Clip/Track", "Device on/off", "◀", "▶", "Detail View", "Rec Quantization", "MIDI Overdub", "Metronome"]
    };
    for (i, label) in (0u8..).zip(device) {
        v.push(note(format!("device.{}", i + 1), label, Widget::Button, 0, 0x3A + i, (kx(i), y + (i / 4) as f32 * 28.0, PAD_W, 20.0)));
    }
    y += 64.0;
    let banks: [&str; 4] = if mk2 { ["Pan", "Sends", "User", "Metronome"] } else { ["Pan", "Send A", "Send B", "Send C"] };
    for (i, label) in (0u8..).zip(banks) {
        v.push(note(format!("bank.{}", i + 1), label, Widget::Button, 0, 0x57 + i, (kx(i), y, PAD_W, 20.0)));
    }
    y += 36.0;
    // The APC40's transport has no LED; the mkII's Play / Record / Session do.
    let transport: Vec<(&str, &str, u8)> =
        if mk2 { vec![("play", "Play", 0x5B), ("record", "Record", 0x5D), ("session", "Session", 0x66)] } else { vec![("play", "Play", 0x5B), ("stop", "Stop", 0x5C), ("record", "Record", 0x5D)] };
    for (i, (id, label, n)) in (0u8..).zip(transport) {
        v.push(note(id, label, if mk2 { Widget::Button } else { Widget::Key }, 0, n, (kx(i), y, PAD_W, 20.0)));
    }
    y += 36.0;
    // Nudge is reversed between the models (§2.2).
    let (n64, n65) = if mk2 { ("Nudge −", "Nudge +") } else { ("Nudge +", "Nudge −") };
    v.push(note("tap", "Tap Tempo", Widget::Key, 0, 0x63, (kx(0), y, PAD_W, 20.0)));
    v.push(note("nudge.64", n64, Widget::Key, 0, 0x64, (kx(1), y, PAD_W, 20.0)));
    v.push(note("nudge.65", n65, Widget::Key, 0, 0x65, (kx(2), y, PAD_W, 20.0)));
    y += 34.0;
    v.push(cc("cue_level", "Cue Level", Widget::Encoder, 0, 0x2F, (kx(0) + 4.0, y, KNOB, KNOB)));
    if mk2 {
        v.push(cc("tempo", "Tempo", Widget::Encoder, 0, 0x0D, (kx(1) + 4.0, y, KNOB, KNOB)));
    }
    // Arrows as a cross on the right.
    let ax = kx(2) + 28.0;
    v.push(note("up", "Haut", Widget::Key, 0, 0x5E, (ax, y - 2.0, 36.0, 18.0)));
    v.push(note("left", "Gauche", Widget::Key, 0, 0x61, (ax - 40.0, y + 20.0, 36.0, 18.0)));
    v.push(note("right", "Droite", Widget::Key, 0, 0x60, (ax + 40.0, y + 20.0, 36.0, 18.0)));
    v.push(note("down", "Bas", Widget::Key, 0, 0x5F, (ax, y + 42.0, 36.0, 18.0)));
    y += 72.0;
    v.push(note("shift", "Shift", Widget::Key, 0, NOTE_SHIFT, (kx(0), y, PAD_W, 22.0)));
    if mk2 {
        v.push(note("bank", "Bank", Widget::Button, 0, 0x67, (kx(1), y, PAD_W, 22.0)));
    }
    // Level with the faders' foot, below Shift.
    let xf_y = (left_bottom - 22.0).max(y + 40.0);
    v.push(cc("crossfader", "Crossfader", Widget::Crossfader, 0, 0x0F, (x1, xf_y, 4.0 * PITCH - 8.0, 22.0)));
    v
}

/// Size of the drawing (width, height) in its units.
pub fn size(elements: &[Element]) -> (f32, f32) {
    elements.iter().fold((0.0f32, 0.0f32), |(w, h), e| (w.max(e.x + e.w + X0), h.max(e.y + e.h + 14.0)))
}

/// The connected APC the drawing mirrors, if any (the first one).
pub fn attached(s: &Shared) -> Option<&MidiDevice> {
    s.midi.devices.iter().find(|d| d.connected && matches!(d.model, Model::Apc40 | Model::Apc40Mk2))
}

/// Model drawn: the connected APC's, else the one picked on screen.
pub fn model(s: &Shared) -> Model {
    attached(s).map_or(s.midi.virt_model, |d| d.model)
}

/// The port whose profile and options the drawing uses (and MIDI learn
/// through the drawing writes to).
pub fn lookup_port(s: &Shared) -> String {
    attached(s).map_or_else(|| PORT.to_string(), |d| d.name.clone())
}

/// Profile of the drawing: the connected APC's, else the virtual port's
/// own choice (saved preference, else the built-in one of the model).
pub fn profile_slug(s: &Shared) -> String {
    match attached(s) {
        Some(d) => d.profile.clone(),
        None => s.midi.store.choose(PORT, s.midi.virt_model).slug,
    }
}

fn driver(model: Model) -> Driver {
    if model == Model::Apc40 {
        Driver::Apc40
    } else {
        Driver::Apc40Mk2
    }
}

fn incoming(e: &Element, pressed: bool) -> Incoming {
    Incoming { kind: e.kind, channel: e.channel, number: e.number, raw: 0, norm: 0.0, pressed: Some(pressed) }
}

/// « Note 0x52 », « CC 0x07 · can. 3 » (the channel only where it says
/// the track: track rows, faders, the APC40's pads).
fn message(e: &Element) -> String {
    let kind = if e.kind == InputKind::Note { "Note" } else { "CC" };
    let group = e.id.split('.').next().unwrap_or("");
    let per_track = matches!(group, "stop" | "select" | "activator" | "solo" | "rec" | "ab")
        || (group == "fader" && e.id != "fader.master")
        || (e.widget == Widget::Pad && (0x35..=0x39).contains(&e.number));
    if per_track {
        format!("{kind} 0x{:02X} · can. {}", e.number, e.channel + 1)
    } else {
        format!("{kind} 0x{:02X}", e.number)
    }
}

/// The mapping a press of `e` hits on one layer, with its index.
fn mapping_of<'a>(mappings: &'a [Mapping], e: &Element, shift: bool) -> Option<(usize, &'a Mapping)> {
    let m = incoming(e, true);
    mappings.iter().enumerate().find(|(_, mp)| mp.shift == shift && mp.input.matches(&m))
}

/// French label of what a mapping does now (a grid pad: the cue it plays
/// on the current page).
fn target_label(s: &Shared, mp: &Mapping) -> String {
    if mp.mode == MapMode::Grid {
        let Some(slot) = mp.args.get("slot").and_then(Value::as_u64).map(|n| n as usize) else { return "Grille".into() };
        let cue = s.grid.cell(s.cue_page + 1, slot / GRID_COLS + 1, slot % GRID_COLS + 1);
        let name = cue.and_then(|id| s.presets.iter().find(|p| p.id == id)).map(|p| p.name.as_str());
        return match name {
            Some(name) => format!("Cue {} de la page : {name}", slot + 1),
            None => format!("Case {} de la page (vide)", slot + 1),
        };
    }
    s.controls.get(&mp.target).map_or_else(|| format!("{} (inconnu)", mp.target), |d| d.label_fr.clone())
}

fn map_json(s: &Shared, found: Option<(usize, &Mapping)>) -> Value {
    match found {
        Some((index, mp)) => json!({
            "index": index,
            "label": target_label(s, mp),
            "target": if mp.mode == MapMode::Grid { learn::GRID } else { mp.target.as_str() },
            "mode": mp.mode,
        }),
        None => Value::Null,
    }
}

/// `GET /api/midi/virtual`: the drawing, each element's mappings (plain
/// and Shift) and MIDI message, and what to draw it with.
pub fn layout(s: &Shared) -> Value {
    let model = model(s);
    let els = elements(model);
    let (w, h) = size(&els);
    let slug = profile_slug(s);
    let profile = engine::profile_of(s, PORT);
    let mappings: &[Mapping] = profile.map_or(&[], |p| p.mappings.as_slice());
    let shift_key = profile.and_then(|p| p.shift_key.clone());
    let list: Vec<Value> = els
        .iter()
        .map(|e| {
            let is_shift = shift_key.as_ref().is_some_and(|k| k.matches(&incoming(e, true)));
            let mut v = json!(e);
            v["message"] = json!(message(e));
            v["map"] = if is_shift { json!({ "index": null, "label": "Shift (couche Shift du profil)", "target": null, "mode": null }) } else { map_json(s, mapping_of(mappings, e, false)) };
            v["shift_map"] = map_json(s, mapping_of(mappings, e, true));
            v
        })
        .collect();
    let attached = attached(s);
    json!({
        "model": model,
        "model_label": model.label(),
        "port": attached.map(|d| d.name.as_str()),
        "profile": slug,
        "profile_name": profile.map(|p| p.name.as_str()),
        "builtin": s.midi.store.is_builtin(&slug),
        "width": w,
        "height": h,
        "elements": list,
        "palette": (0..128u8).map(super::palette::hex).collect::<Vec<_>>(),
    })
}

/// `GET /api/midi/leds`: the LEDs the drawn controller shows (exactly what
/// the real one is sent), its fader / knob positions and its Shift.
/// `leds`: `"note:<n>:<ch>"` / `"cc:<n>:<ch>"` → value; `modes`: mkII
/// pulse / blink, `"note:<n>:0"` → [mode channel, secondary colour].
pub fn leds(s: &Shared) -> Value {
    let model = model(s);
    let port = lookup_port(s);
    let frame = led::render(driver(model), s, &port, s.now_s());
    let mut leds = Map::new();
    for (&(status, key), &v) in &frame.0 {
        let kind = if status & 0xF0 == 0xB0 { "cc" } else { "note" };
        leds.insert(format!("{kind}:{key}:{}", status & 0x0F), json!(v));
    }
    let modes: Map<String, Value> = frame.1.iter().map(|(n, &(ch, sec))| (format!("note:{n}:0"), json!([ch, sec]))).collect();
    // Faders and knobs show where their control is (like motorised faders).
    let profile = engine::profile_of(s, PORT);
    let mut values = Map::new();
    for e in elements(model).iter().filter(|e| matches!(e.widget, Widget::Fader | Widget::Crossfader | Widget::Knob)) {
        let Some((_, mp)) = profile.and_then(|p| mapping_of(&p.mappings, e, false)) else { continue };
        if let Some(pos) = position(s, mp) {
            values.insert(e.id.clone(), json!((pos * 1000.0).round() / 1000.0));
        }
    }
    json!({
        "port": attached(s).map(|d| d.name.as_str()),
        "model": model,
        "leds": leds,
        "modes": modes,
        "values": values,
        "shift": s.midi.map.shift_held(PORT),
    })
}

/// Where a fader driving `mp` sits now (0..1), for an absolute mapping.
fn position(s: &Shared, mp: &Mapping) -> Option<f32> {
    if mp.mode != MapMode::Absolute {
        return None;
    }
    let desc = s.controls.get(&mp.target)?;
    let (lo, hi) = engine::range(&desc.kind, mp)?;
    let v = match controls::current(s, desc)? {
        Value::Bool(b) => f32::from(u8::from(b)),
        v => v.as_f64()? as f32,
    };
    (hi != lo).then(|| mp.curve.inverse(((v - lo) / (hi - lo)).clamp(0.0, 1.0)).clamp(0.0, 1.0))
}

/// What the page does with an element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// A button pressed / released.
    Press(bool),
    /// A fader or knob moved to 0–127.
    Value(u8),
    /// An encoder turned by n steps (−63..63).
    Turn(i8),
    /// « Apprendre ici »: the message a learn should take from this element.
    Learn,
}

pub type PressError = (u16, String);

fn element(model: Model, id: &str) -> Result<Element, PressError> {
    elements(model).into_iter().find(|e| e.id == id).ok_or_else(|| (404, format!("élément inconnu : {id}")))
}

fn msg(e: &Element, action: Action) -> Result<Option<MidiMsg>, PressError> {
    let (channel, number) = (e.channel, e.number);
    Ok(Some(match (e.widget, action) {
        (w, Action::Press(true)) if w.is_button() => MidiMsg::NoteOn { channel, note: number, velocity: 127 },
        (w, Action::Press(false)) if w.is_button() => MidiMsg::NoteOff { channel, note: number },
        (Widget::Fader | Widget::Crossfader | Widget::Knob, Action::Value(v)) => MidiMsg::Cc { channel, number, value: v.min(127) },
        // The APC's encoders: two's complement (1–63 up, 127–65 down).
        (Widget::Encoder, Action::Turn(0)) => return Ok(None),
        (Widget::Encoder, Action::Turn(n)) => MidiMsg::Cc { channel, number, value: (n.clamp(-63, 63) as i16).rem_euclid(128) as u8 },
        _ => return Err((400, format!("« {} » ne se joue pas ainsi", e.label))),
    }))
}

/// `POST /api/midi/virtual`: plays one element of the drawing.
pub fn press(s: &mut Shared, id: &str, action: Action, now: Instant) -> Result<(), PressError> {
    let e = element(model(s), id)?;
    if action == Action::Learn {
        return learn_from(s, &e, now);
    }
    let Some(msg) = msg(&e, action)? else { return Ok(()) };
    engine::handle_screen(s, &[MidiEvent { port: PORT.to_string(), msg, at: now }]);
    Ok(())
}

/// The learn waiting (`/api/midi/learn`) takes this element's message, as
/// if it came from the port the drawing mirrors: the mapping lands in that
/// port's profile. A fader sends a value the learn reads as a fader, an
/// encoder one step.
fn learn_from(s: &mut Shared, e: &Element, now: Instant) -> Result<(), PressError> {
    if s.midi.learn.pending.is_none() {
        return Err((409, "aucun apprentissage MIDI en cours".into()));
    }
    let port = lookup_port(s);
    let msgs = match e.widget {
        Widget::Encoder => vec![MidiMsg::Cc { channel: e.channel, number: e.number, value: 1 }],
        Widget::Fader | Widget::Crossfader | Widget::Knob => {
            // A value no encoder sends, and not the last one seen (a CC that
            // doesn't move isn't taken).
            let last = s.midi.learn.cc_last(&port, e.channel, e.number);
            let value = [64u8, 32, 96].into_iter().find(|&v| Some(v) != last).unwrap_or(64);
            vec![MidiMsg::Cc { channel: e.channel, number: e.number, value }]
        }
        _ => vec![MidiMsg::NoteOn { channel: e.channel, note: e.number, velocity: 127 }, MidiMsg::NoteOff { channel: e.channel, note: e.number }],
    };
    for msg in msgs {
        let ev = MidiEvent { port: port.clone(), msg, at: now };
        learn::capture(s, &ev);
        learn::observe(s, &ev);
    }
    Ok(())
}

/// `POST /api/midi/virtual/unmap`: « Retirer l'affectation » of one
/// element on one layer, in the profile the drawing uses (a built-in one
/// gets its `-perso` copy, like a learn).
pub fn unmap(s: &mut Shared, id: &str, shift: bool) -> Result<(), PressError> {
    let e = element(model(s), id)?;
    let port = lookup_port(s);
    let Some((slug, mut profile)) = learn::profile_for(s, &port) else { return Err((404, "profil introuvable".into())) };
    let Some((index, _)) = mapping_of(&profile.mappings, &e, shift) else { return Err((404, format!("« {} » n'a pas d'affectation", e.label))) };
    profile.mappings.remove(index);
    learn::save(s, &port, &slug, profile).map(|_| ()).map_err(|err| (500, err))
}

/// `POST /api/midi/virtual/model`: the model drawn when no APC is connected.
pub fn set_model(s: &mut Shared, model: Model) -> Result<(), PressError> {
    if !matches!(model, Model::Apc40 | Model::Apc40Mk2) {
        return Err((400, "modèle : « Apc40 » ou « Apc40Mk2 »".into()));
    }
    s.midi.virt_model = model;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::ControlSource;
    use crate::test_support;
    use std::collections::HashSet;
    use std::time::Duration;

    fn tap(s: &mut Shared, id: &str) {
        press(s, id, Action::Press(true), Instant::now()).unwrap();
        press(s, id, Action::Press(false), Instant::now()).unwrap();
    }

    fn el_json<'a>(layout: &'a Value, id: &str) -> &'a Value {
        layout["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("{id}"))
    }

    fn page_cue(s: &Shared, slot: usize) -> String {
        s.grid.slot(s.cue_page, slot).unwrap().to_string()
    }

    #[test]
    fn layouts_have_every_control_once_and_fit() {
        for model in [Model::Apc40, Model::Apc40Mk2] {
            let els = elements(model);
            let (w, h) = size(&els);
            assert_eq!(els.iter().filter(|e| e.widget == Widget::Pad).count(), 40, "{model:?}");
            assert_eq!(els.iter().filter(|e| e.widget == Widget::Round).count(), 5);
            assert_eq!(els.iter().filter(|e| e.widget == Widget::Fader).count(), 9);
            assert_eq!(els.iter().filter(|e| e.widget == Widget::Knob).count(), 16);
            let ids: HashSet<&str> = els.iter().map(|e| e.id.as_str()).collect();
            assert_eq!(ids.len(), els.len(), "{model:?}: unique ids");
            let msgs: HashSet<(InputKind, u8, u8)> = els.iter().map(|e| (e.kind, e.channel, e.number)).collect();
            assert_eq!(msgs.len(), els.len(), "{model:?}: one message per element");
            assert!(els.iter().all(|e| e.x >= 0.0 && e.y >= 0.0 && e.x + e.w <= w && e.y + e.h <= h));
            for (i, a) in els.iter().enumerate() {
                for b in &els[i + 1..] {
                    let apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                    assert!(apart, "{model:?}: {} overlaps {}", a.id, b.id);
                }
            }
            // Wider than tall, like the hardware, and small enough for 1280 px.
            assert!(w > h && w < 1000.0, "{model:?}: {w} × {h}");
        }
    }

    /// Every mapping of the built-in APC profiles has its button on the
    /// drawing (the footswitch excepted: not on the panel).
    #[test]
    fn every_builtin_mapping_is_drawn() {
        let s = test_support::shared();
        for (slug, model) in [("apc40", Model::Apc40), ("apc40-mk2", Model::Apc40Mk2)] {
            let els = elements(model);
            for mp in &s.midi.store.get(slug).unwrap().mappings {
                if mp.input.kind == InputKind::Cc && mp.input.number == 0x40 {
                    continue;
                }
                let hit = els.iter().any(|e| mp.input.matches(&incoming(e, true)));
                assert!(hit, "{slug}: {:?} has no button", mp.input);
            }
        }
    }

    #[test]
    fn hover_shows_mapping_shift_layer_and_message() {
        let s = test_support::shared();
        let l = layout(&s);
        assert_eq!((l["model"].as_str(), l["profile"].as_str(), l["port"].as_str()), (Some("Apc40Mk2"), Some("apc40-mk2"), None));
        let f = el_json(&l, "fader.1");
        assert_eq!(format!("{} — {}", f["label"].as_str().unwrap(), f["map"]["label"].as_str().unwrap()), "Fader 1 — Taille maître");
        assert_eq!(f["message"], "CC 0x07 · can. 1");
        assert_eq!(f["shift_map"]["target"], "layer.1.dimmer");
        let scene = el_json(&l, "scene.1");
        assert_eq!((scene["message"].as_str(), scene["map"]["target"].as_str(), scene["shift_map"]["target"].as_str()), (Some("Note 0x52"), Some("page.1"), Some("page.6")));
        let pad = el_json(&l, "pad.1.1");
        assert_eq!(pad["map"]["target"], "grid");
        assert!(pad["map"]["label"].as_str().unwrap().starts_with("Cue 1 de la page : "), "{}", pad["map"]["label"]);
        assert_eq!(el_json(&l, "shift")["map"]["label"], "Shift (couche Shift du profil)");
        assert_eq!(el_json(&l, "bank.1")["map"], Value::Null, "not mapped");
        assert_eq!(l["palette"].as_array().unwrap().len(), 128);
    }

    /// No controller at all: a click on the top-left pad plays cue 1 of the
    /// page, recorded as the UI; its pad lights like the real one would.
    #[test]
    fn click_plays_the_pad_cue_and_lights_it() {
        let mut s = test_support::shared();
        let first = page_cue(&s, 0);
        tap(&mut s, "pad.1.1");
        assert!(s.deck.active.iter().any(|a| a.cue == first), "cue 1 plays");
        assert_eq!(s.touches.get("grid.1.1.1").map(|t| t.source), Some(ControlSource::Ui), "the UI, not MIDI");
        let l = leds(&s);
        // mkII: pad (0,0) = note 32, full cue colour, pulsing to its dim one.
        assert!(l["leds"]["note:32:0"].as_u64().unwrap() > 0);
        assert_eq!(l["modes"]["note:32:0"][0], crate::midi::palette::CH_PULSE_QUARTER);
        // Scene Launch 1 = current page, white.
        assert_eq!(l["leds"]["note:82:0"], crate::midi::palette::MK2_WHITE);

        set_model(&mut s, Model::Apc40).unwrap();
        // APC40: pad (0,0) = note 0x35 on channel 0, green while playing.
        assert_eq!(leds(&s)["leds"]["note:53:0"], 1);
        assert_eq!(leds(&s)["model"], "Apc40");
        assert_eq!(layout(&s)["profile"], "apc40");
        assert_eq!(set_model(&mut s, Model::ApcMini).unwrap_err().0, 400);
    }

    #[test]
    fn scene_launch_shift_and_arrows_follow_the_profile() {
        let mut s = test_support::shared();
        tap(&mut s, "scene.3");
        assert_eq!(s.cue_page, 2);
        // Shift locked on screen: down stays down until released.
        press(&mut s, "shift", Action::Press(true), Instant::now()).unwrap();
        assert_eq!(leds(&s)["shift"], true);
        tap(&mut s, "scene.2");
        assert_eq!(s.cue_page, 6, "Shift + Scene Launch 2 = page 7");
        press(&mut s, "shift", Action::Press(false), Instant::now()).unwrap();
        assert_eq!(leds(&s)["shift"], false);
        tap(&mut s, "down");
        assert_eq!(s.cue_page, 7);
    }

    /// The safety rule: the on-screen controller never arms, whatever the
    /// MIDI arming option and the profile say; Stop All is a blackout.
    #[test]
    fn never_arms_and_stop_all_blacks_out() {
        let mut s = test_support::shared();
        s.midi.store.set_safety(crate::midi::safety::MidiSafety { allow_arm: true, ..Default::default() }).unwrap();
        // Arming on Play, with and without Shift.
        let mut p = s.midi.store.get("apc40-mk2").unwrap().clone();
        p.mappings.retain(|mp| !(mp.input.kind == InputKind::Note && mp.input.number == 0x5B));
        let arm = p.mappings.iter().find(|mp| mp.target == "transport.arm").unwrap().clone();
        for shift in [false, true] {
            let mut mp = arm.clone();
            (mp.input.number, mp.shift) = (0x5B, shift);
            p.mappings.push(mp);
        }
        s.midi.store.save_profile(Some(PORT), "arm-test", p).unwrap();
        assert_eq!(profile_slug(&s), "arm-test");
        let t0 = Instant::now();
        for shift in [false, true] {
            press(&mut s, "shift", Action::Press(shift), t0).unwrap();
            press(&mut s, "play", Action::Press(true), t0).unwrap();
            crate::midi::engine::frame(&mut s, t0 + Duration::from_secs(3));
            press(&mut s, "play", Action::Press(false), t0).unwrap();
        }
        assert!(!s.gate.is_armed());

        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        assert!(s.gate.is_armed());
        press(&mut s, "stop_all", Action::Press(true), t0).unwrap();
        assert!(!s.gate.is_armed(), "Shift + Stop All = blackout on screen, never arming");
        crate::midi::engine::frame(&mut s, t0 + Duration::from_secs(3));
        press(&mut s, "stop_all", Action::Press(false), t0).unwrap();
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn faders_use_pickup_and_encoders_step() {
        let mut s = test_support::shared();
        // Master size sits at 1.0 of 0..2: the fader must pass mid-travel.
        assert_eq!(leds(&s)["values"]["fader.1"], 0.5);
        press(&mut s, "fader.1", Action::Value(127), Instant::now()).unwrap();
        crate::midi::engine::frame(&mut s, Instant::now());
        assert_eq!(s.live.size, 1.0, "pickup: not caught yet");
        press(&mut s, "fader.1", Action::Value(64), Instant::now()).unwrap();
        press(&mut s, "fader.1", Action::Value(127), Instant::now()).unwrap();
        crate::midi::engine::frame(&mut s, Instant::now());
        assert_eq!(s.live.size, 2.0);
        assert_eq!(s.touches.get("master.size").map(|t| t.source), Some(ControlSource::Ui));
        assert_eq!(leds(&s)["values"]["fader.1"], 1.0);

        let bpm = s.tempo.bpm;
        press(&mut s, "tempo", Action::Turn(2), Instant::now()).unwrap();
        crate::midi::engine::frame(&mut s, Instant::now());
        assert!(s.tempo.bpm > bpm, "{} → {}", bpm, s.tempo.bpm);
        let up = s.tempo.bpm;
        press(&mut s, "tempo", Action::Turn(-2), Instant::now()).unwrap();
        crate::midi::engine::frame(&mut s, Instant::now());
        assert!(s.tempo.bpm < up);

        assert_eq!(press(&mut s, "fader.1", Action::Press(true), Instant::now()).unwrap_err().0, 400);
        assert_eq!(press(&mut s, "pad.1.1", Action::Value(3), Instant::now()).unwrap_err().0, 400);
        assert_eq!(press(&mut s, "nope", Action::Press(true), Instant::now()).unwrap_err().0, 404);
    }

    /// « Apprendre ici » and « Retirer l'affectation » without a
    /// controller: the virtual port gets its own `-perso` profile.
    #[test]
    fn learn_here_and_unmap_on_the_virtual_profile() {
        let mut s = test_support::shared();
        s.midi.enabled = true;
        assert_eq!(press(&mut s, "bank.1", Action::Learn, Instant::now()).unwrap_err().0, 409, "no learn waiting");
        learn::start(&mut s, "master.brightness", Value::Null, false, Instant::now()).unwrap();
        press(&mut s, "knob.dev.1", Action::Learn, Instant::now()).unwrap();
        assert!(s.midi.learn.pending.is_none(), "taken");
        assert_eq!(profile_slug(&s), "apc40-mk2-perso");
        let l = layout(&s);
        assert_eq!(el_json(&l, "knob.dev.1")["map"]["target"], "master.brightness");
        assert_eq!(el_json(&l, "knob.dev.1")["map"]["mode"], "absolute");
        assert_eq!(s.midi.store.get("apc40-mk2").unwrap().mappings.len(), 135, "built-in untouched");

        // A button on the Shift layer, an encoder in relative mode.
        learn::start(&mut s, "cue.multi", Value::Null, true, Instant::now()).unwrap();
        press(&mut s, "bank.2", Action::Learn, Instant::now()).unwrap();
        learn::start(&mut s, "master.size_x", Value::Null, false, Instant::now()).unwrap();
        press(&mut s, "cue_level", Action::Learn, Instant::now()).unwrap();
        let l = layout(&s);
        assert_eq!((el_json(&l, "bank.2")["map"].is_null(), el_json(&l, "bank.2")["shift_map"]["target"].as_str()), (true, Some("cue.multi")));
        assert_eq!(el_json(&l, "cue_level")["map"]["mode"], "relative");

        unmap(&mut s, "knob.dev.1", false).unwrap();
        assert_eq!(el_json(&layout(&s), "knob.dev.1")["map"], Value::Null);
        assert_eq!(unmap(&mut s, "knob.dev.1", false).unwrap_err().0, 404);
        assert!(!s.gate.is_armed());
    }

    /// With an APC connected, the drawing is that APC: its model, profile
    /// and LEDs; a learn through the drawing lands in its profile.
    #[test]
    fn an_attached_apc_is_mirrored() {
        let mut s = test_support::shared();
        s.midi.enabled = true;
        let d = s.midi.device_mut("APC40");
        (d.connected, d.model, d.profile) = (true, Model::Apc40, "apc40".into());
        assert_eq!((model(&s), lookup_port(&s), profile_slug(&s)), (Model::Apc40, "APC40".to_string(), "apc40".to_string()));
        assert_eq!(layout(&s)["port"], "APC40");
        tap(&mut s, "pad.1.1");
        assert_eq!(leds(&s)["leds"]["note:53:0"], 1, "same LEDs as the device");
        learn::start(&mut s, "master.brightness", Value::Null, false, Instant::now()).unwrap();
        press(&mut s, "knob.dev.2", Action::Learn, Instant::now()).unwrap();
        assert_eq!(s.midi.devices[0].profile, "apc40-perso", "the device's profile");
        assert_eq!(el_json(&layout(&s), "knob.dev.2")["map"]["target"], "master.brightness");
        // The device's own Shift is not the drawing's.
        press(&mut s, "shift", Action::Press(true), Instant::now()).unwrap();
        assert!(!s.midi.map.shift_held("APC40") && s.midi.map.shift_held(PORT));
    }
}
