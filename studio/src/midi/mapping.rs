//! Mapping types stored in profiles (T-202): which MIDI message drives
//! which control id, and how. Pure helpers only (value curves, relative
//! encoder decoding, the pickup rule); the runtime lives in `engine.rs`.

use super::MidiMsg;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Note,
    Cc,
    PitchBend,
    ProgramChange,
}

/// The hardware control a mapping listens to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidiInput {
    pub kind: InputKind,
    /// `None` = any channel. The APCs use the channel for the track (0–7),
    /// so their profiles set it.
    #[serde(default)]
    pub channel: Option<u8>,
    /// Note, CC or program number (ignored for pitch bend).
    #[serde(default)]
    pub number: u8,
}

impl MidiInput {
    pub fn matches(&self, m: &Incoming) -> bool {
        self.kind == m.kind && self.channel.is_none_or(|c| c == m.channel) && (self.kind == InputKind::PitchBend || self.number == m.number)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapMode {
    /// Note On (or CC > 63) fires the action once.
    Trigger,
    /// Each press flips a boolean.
    Toggle,
    /// On while held (flash cue, strobe).
    Momentary,
    /// 0–127 (or 14-bit pitch bend) → `min..max`.
    Absolute,
    /// Endless encoder: each message is a signed number of steps.
    Relative,
    /// Slot `args.slot` of the current cue page (T-204); momentary.
    Grid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    /// Finer near `min` (brightness, speeds): 1 % of travel at the bottom
    /// is ~0.05 % of the range, the top half covers ~90 %.
    Log,
}

impl Curve {
    /// Fader position 0..1 → fraction of the range 0..1.
    pub fn apply(self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        match self {
            Curve::Linear => n,
            Curve::Log => (100f32.powf(n) - 1.0) / 99.0,
        }
    }

    /// Fraction of the range → fader position (for pickup).
    pub fn inverse(self, f: f32) -> f32 {
        let f = f.clamp(0.0, 1.0);
        match self {
            Curve::Linear => f,
            Curve::Log => (1.0 + 99.0 * f).ln() / 100f32.ln(),
        }
    }
}

/// The three common ways endless encoders send a step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelEncoding {
    /// APC40 / most Akai: 1–63 = +n, 127…64 = −1…−64.
    #[default]
    TwosComplement,
    /// Bit 6 = minus: 1–63 = +n, 65–127 = −1…−63.
    SignBit,
    /// 64 = no move, 65 = +1, 63 = −1.
    Offset64,
}

impl RelEncoding {
    pub fn delta(self, v: u8) -> i32 {
        let v = (v & 0x7F) as i32;
        match self {
            RelEncoding::TwosComplement => {
                if v >= 64 {
                    v - 128
                } else {
                    v
                }
            }
            RelEncoding::SignBit => {
                if v & 0x40 != 0 {
                    -(v & 0x3F)
                } else {
                    v
                }
            }
            RelEncoding::Offset64 => v - 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    pub input: MidiInput,
    /// Only used while the profile's Shift key is held.
    #[serde(default)]
    pub shift: bool,
    /// Control id (controls.rs), e.g. "master.size". Ignored in `grid` mode.
    #[serde(default)]
    pub target: String,
    /// Extra arguments, e.g. `{ "slot": 12 }` for `grid`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
    pub mode: MapMode,
    /// Range override (native units); default = the control's range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    /// Native units per encoder step (relative); default = 1/127 of the range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f32>,
    #[serde(default)]
    pub curve: Curve,
    /// Soft takeover for `absolute` (see `pickup_catches`).
    #[serde(default)]
    pub pickup: bool,
    /// Relative encoders only.
    #[serde(default)]
    pub encoding: RelEncoding,
    /// Generic LED feedback (T-211): the button's LED, for devices without
    /// a dedicated driver. `None` = the studio leaves it alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<LedFeedback>,
}

/// Values a generic controller lights its LED with, sent back on the
/// mapping's own message (Note On velocity, or CC value) and channel.
/// Many controllers light a pad with the velocity of a Note On on the
/// same note: 0 = off, 127 (or 1) = on, sometimes another value blinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedFeedback {
    #[serde(default)]
    pub off: u8,
    #[serde(default = "full")]
    pub on: u8,
    /// Flash cue held, or a button lit by its Shift mapping; `None` = `on`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blink: Option<u8>,
    /// Grid pad holding a cue that isn't playing; `None` = `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<u8>,
}

fn full() -> u8 {
    127
}

impl Default for LedFeedback {
    fn default() -> Self {
        LedFeedback { off: 0, on: 127, blink: None, present: None }
    }
}

impl LedFeedback {
    /// Values are 7-bit MIDI data bytes.
    pub fn clamped(self) -> Self {
        LedFeedback { off: self.off & 0x7F, on: self.on & 0x7F, blink: self.blink.map(|v| v & 0x7F), present: self.present.map(|v| v & 0x7F) }
    }
}

/// An incoming channel message, reduced to what mappings look at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Incoming {
    pub kind: InputKind,
    pub channel: u8,
    pub number: u8,
    /// Raw 7-bit value (velocity, CC value, program); pitch bend: MSB.
    pub raw: u8,
    /// Position 0..1 (pitch bend uses all 14 bits).
    pub norm: f32,
    /// Button state: `Some(true)` pressed, `Some(false)` released. Program
    /// Change is a press with no release.
    pub pressed: Option<bool>,
}

impl Incoming {
    /// `None` for SysEx, clock and transport.
    pub fn from_msg(msg: &MidiMsg) -> Option<Incoming> {
        let (kind, channel, number, raw, norm, pressed) = match *msg {
            MidiMsg::NoteOn { channel, note, velocity } => (InputKind::Note, channel, note, velocity, velocity as f32 / 127.0, Some(true)),
            MidiMsg::NoteOff { channel, note } => (InputKind::Note, channel, note, 0, 0.0, Some(false)),
            MidiMsg::Cc { channel, number, value } => (InputKind::Cc, channel, number, value, value as f32 / 127.0, Some(value > 63)),
            MidiMsg::PitchBend { channel, value } => {
                let norm = value.min(16383) as f32 / 16383.0;
                (InputKind::PitchBend, channel, 0, (value >> 7) as u8, norm, Some(value >= 8192))
            }
            MidiMsg::ProgramChange { channel, program } => (InputKind::ProgramChange, channel, program, program, 1.0, Some(true)),
            _ => return None,
        };
        Some(Incoming { kind, channel: channel & 0x0F, number, raw, norm, pressed })
    }
}

/// Pickup threshold: 3 % of the fader travel.
pub const PICKUP_WINDOW: f32 = 0.03;

/// Whether a fader at `pos` (0..1) takes over a control whose value sits at
/// fader position `target`: close enough, or it just crossed it coming
/// from `prev`.
pub fn pickup_catches(prev: Option<f32>, pos: f32, target: f32) -> bool {
    if (pos - target).abs() <= PICKUP_WINDOW {
        return true;
    }
    prev.is_some_and(|p| (p - target).signum() != (pos - target).signum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_encodings() {
        let tc = RelEncoding::TwosComplement;
        assert_eq!([1, 63, 64, 127, 0].map(|v| tc.delta(v)), [1, 63, -64, -1, 0]);
        let sb = RelEncoding::SignBit;
        assert_eq!([1, 63, 65, 127, 64].map(|v| sb.delta(v)), [1, 63, -1, -63, 0]);
        let off = RelEncoding::Offset64;
        assert_eq!([64, 65, 63, 127, 0].map(|v| off.delta(v)), [0, 1, -1, 63, -64]);
    }

    #[test]
    fn curves_hit_their_bounds_and_invert() {
        for c in [Curve::Linear, Curve::Log] {
            assert_eq!(c.apply(0.0), 0.0);
            assert!((c.apply(1.0) - 1.0).abs() < 1e-6);
            assert_eq!(c.apply(-2.0), 0.0);
            for n in [0.1, 0.5, 0.9] {
                assert!((c.inverse(c.apply(n)) - n).abs() < 1e-4, "{c:?} {n}");
            }
        }
        assert!(Curve::Log.apply(0.5) < 0.1, "log is fine at the bottom");
    }

    #[test]
    fn pickup_rule() {
        assert!(!pickup_catches(None, 1.0, 0.2), "far and no history: ignored");
        assert!(pickup_catches(None, 0.22, 0.2), "within 3 %");
        assert!(!pickup_catches(None, 0.24, 0.2));
        assert!(pickup_catches(Some(0.1), 0.5, 0.2), "crossed going up");
        assert!(pickup_catches(Some(0.9), 0.1, 0.2), "crossed going down");
        assert!(!pickup_catches(Some(0.9), 0.5, 0.2), "still above");
    }

    #[test]
    fn channel_none_is_any_channel() {
        let any = MidiInput { kind: InputKind::Cc, channel: None, number: 7 };
        let ch3 = MidiInput { kind: InputKind::Cc, channel: Some(3), number: 7 };
        let msg = |channel| Incoming::from_msg(&MidiMsg::Cc { channel, number: 7, value: 10 }).unwrap();
        assert!(any.matches(&msg(0)) && any.matches(&msg(3)));
        assert!(ch3.matches(&msg(3)) && !ch3.matches(&msg(0)));
        let note = Incoming::from_msg(&MidiMsg::NoteOn { channel: 3, note: 7, velocity: 1 }).unwrap();
        assert!(!ch3.matches(&note), "kind is part of the key");
    }

    #[test]
    fn incoming_messages() {
        let off = Incoming::from_msg(&MidiMsg::NoteOff { channel: 1, note: 0x51 }).unwrap();
        assert_eq!(off.pressed, Some(false));
        let bend = Incoming::from_msg(&MidiMsg::PitchBend { channel: 0, value: 16383 }).unwrap();
        assert_eq!(bend.norm, 1.0);
        assert_eq!(Incoming::from_msg(&MidiMsg::Cc { channel: 0, number: 1, value: 64 }).unwrap().pressed, Some(true));
        assert!(Incoming::from_msg(&MidiMsg::Clock).is_none());
    }

    #[test]
    fn mapping_json_round_trip_and_defaults() {
        let m: Mapping = serde_json::from_str(r#"{ "input": { "kind": "note", "number": 81 }, "target": "transport.blackout", "mode": "trigger" }"#).unwrap();
        assert_eq!(m.input.channel, None);
        assert!(!m.shift && !m.pickup && m.min.is_none() && m.step.is_none());
        assert_eq!((m.curve, m.encoding), (Curve::Linear, RelEncoding::TwosComplement));
        let full = Mapping {
            input: MidiInput { kind: InputKind::Cc, channel: Some(2), number: 13 },
            shift: true,
            target: "tempo.bpm".into(),
            args: serde_json::json!({ "slot": 3 }),
            mode: MapMode::Relative,
            min: Some(60.0),
            max: Some(180.0),
            step: Some(0.5),
            curve: Curve::Log,
            pickup: true,
            encoding: RelEncoding::Offset64,
            led: Some(LedFeedback { off: 0, on: 1, blink: Some(2), present: None }),
        };
        let back: Mapping = serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
        assert!(serde_json::from_str::<Mapping>(r#"{ "input": { "kind": "note", "number": 1 }, "mode": "wobble" }"#).is_err());
        assert!(m.led.is_none() && !serde_json::to_string(&m).unwrap().contains("led"), "no LED unless asked");
    }

    #[test]
    fn led_feedback_defaults() {
        let led: LedFeedback = serde_json::from_str("{}").unwrap();
        assert_eq!(led, LedFeedback { off: 0, on: 127, blink: None, present: None });
        let led: LedFeedback = serde_json::from_str(r#"{ "off": 0, "on": 1, "blink": 2 }"#).unwrap();
        assert_eq!((led.on, led.blink), (1, Some(2)));
        let wild = LedFeedback { off: 200, on: 255, blink: Some(128), present: Some(130) };
        assert_eq!(wild.clamped(), LedFeedback { off: 72, on: 127, blink: Some(0), present: Some(2) });
    }
}
