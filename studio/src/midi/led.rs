//! LED feedback on the APC40 / APC40 mkII (T-205). From `Shared` (not the
//! UI), `render` computes the LEDs a device should show; `LedState` keeps
//! what was last sent and turns the next frame into the few messages that
//! differ, at most 30 times a second. The worker renders under the lock
//! and sends outside it, so the engine never waits on CoreMIDI.
//!
//! What lights is read from the device's profile: a pad mapped to a grid
//! slot shows that cue, a button mapped to a toggle shows it, a scene
//! button mapped to `page.N` shows the page, a knob mapped to a control
//! shows its value on the ring. Anything learned later (T-203) lights the
//! same way. On top of that, per driver: the Metronome LED beats with the
//! tempo clock (T-150) and the Clip Stop row blinks while the emergency
//! stop is latched.
//!
//! Any other controller (T-211) only lights the buttons whose mapping
//! has a `led` (off / on / blink values the user picked): no driver, no
//! metronome, no rings.
//!
//! Velocities and channels: Akai's public protocol documents
//! (docs/research/midi-apc40.md §1.3, §2.3). The mkII pads are RGB
//! (T-206, `palette.rs`): a cue present shows its colour dimmed, a playing
//! cue its full colour pulsing (1/4) to the dim one, a held flash blinks
//! (1/8); the pad's own LED modes follow the MIDI clock the worker sends.
//! Scene Launch: current page white, other pages grey, pages 6–10 pulsing.

use super::engine::{profile_of, range};
use super::mapping::{InputKind, LedFeedback, MapMode, Mapping};
use super::palette::{self, PadColors, CH_BLINK_EIGHTH, CH_PULSE_QUARTER};
pub use super::palette::{MK2_DIM_GREEN, MK2_GREEN, MK2_GREY, MK2_WHITE, MK2_YELLOW};
use super::profile::Driver;
use super::Model;
use crate::controls::{self, ControlKind, GRID_COLS, GRID_ROWS};
use crate::Shared;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// Shortest time between two LED updates of one device (≤ 30 per second).
pub const LED_EVERY: Duration = Duration::from_millis(33);

/// Metronome LED (T-205): device button 8 on the APC40, Metronome on the mkII.
pub const NOTE_METRONOME_APC40: u8 = 0x41;
pub const NOTE_METRONOME_MK2: u8 = 0x5A;
pub const NOTE_CLIP_STOP: u8 = 0x34;

/// APC40 clip-launch velocities.
const MK1_GREEN: u8 = 1;
const MK1_GREEN_BLINK: u8 = 2;
const MK1_YELLOW: u8 = 5;
/// Scene launch / clip stop: 1 on, 2 blinking (the device's own rate).
const ON: u8 = 1;
const BLINK: u8 = 2;
/// Desired state of every LED the studio drives on one device:
/// (status byte, note or CC number) → velocity or value. The status keeps
/// the channel: the APC40's grid and track buttons use it for the column.
/// The second map is the mkII's LED modes: note (on status 0x90) →
/// (mode channel, secondary colour), sent right after the solid colour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedFrame(pub BTreeMap<(u8, u8), u8>, pub BTreeMap<u8, (u8, u8)>);

impl LedFrame {
    fn set(&mut self, status: u8, key: u8, value: u8) {
        self.0.insert((status, key), value);
    }

    /// A mkII RGB pad / scene button: `color` solid on channel 0, then,
    /// if any, the secondary colour on a mode channel (pulse, blink).
    fn set_rgb(&mut self, note: u8, (color, mode): (u8, Option<(u8, u8)>)) {
        self.set(0x90, note, color);
        match mode {
            Some(m) if color != 0 => self.1.insert(note, m),
            _ => self.1.remove(&note),
        };
    }

    fn mode(&self, status: u8, key: u8) -> Option<&(u8, u8)> {
        if status == 0x90 {
            self.1.get(&key)
        } else {
            None
        }
    }

    /// Messages that take a device showing `last` to `self`. An LED no
    /// longer driven is switched off (Note On velocity 0, CC value 0). A
    /// solid colour on channel 0 also ends a pulse / blink, so a pad whose
    /// mode changes gets its colour again, then its new mode.
    pub fn diff(&self, last: &LedFrame) -> Vec<[u8; 3]> {
        let mut msgs = Vec::new();
        for (&(st, key), &v) in &self.0 {
            let mode = self.mode(st, key);
            if last.0.get(&(st, key)) != Some(&v) || mode != last.mode(st, key) {
                msgs.push([st, key, v]);
                if let Some(&(ch, secondary)) = mode {
                    msgs.push([0x90 | ch & 0x0F, key, secondary]);
                }
            }
        }
        msgs.extend(last.0.iter().filter(|(k, v)| **v != 0 && !self.0.contains_key(k)).map(|(&(st, key), _)| [st, key, 0]));
        msgs
    }
}

/// Per-device LED state, kept by the worker (`DeviceState` in the task).
#[derive(Debug, Default)]
pub struct LedState {
    /// What the device shows (as far as we know). Empty after a (re)connect
    /// or an Introduction: the next update then sends everything.
    pub last_sent: LedFrame,
    next_at: Option<Instant>,
}

impl LedState {
    pub fn due(&self, now: Instant) -> bool {
        self.next_at.is_none_or(|t| now >= t)
    }

    /// When the next update may run (`None` = now).
    pub fn next_at(&self) -> Option<Instant> {
        self.next_at
    }

    /// Records `frame` as sent and returns the messages to send.
    pub fn update(&mut self, frame: LedFrame, now: Instant) -> Vec<[u8; 3]> {
        let msgs = frame.diff(&self.last_sent);
        self.last_sent = frame;
        self.next_at = Some(now + LED_EVERY);
        msgs
    }

    /// No update before `t` (nothing to drive on this device for now).
    pub fn pause_until(&mut self, t: Instant) {
        self.next_at = Some(t);
    }

    /// The device lost what we sent (Introduction, goodbye): resend all.
    pub fn forget(&mut self) {
        self.last_sent = LedFrame::default();
        self.next_at = None;
    }
}

/// What a hardware LED can do.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Clip launch pad: APC40 green/red/yellow, mkII RGB.
    Grid,
    /// Scene launch: APC40 on/blink, mkII RGB.
    Scene,
    /// Clip stop: on / blink.
    ClipStop,
    /// Track select, Activator, Solo, Record arm, crossfader A/B: on/off,
    /// one per track (the channel).
    Track,
    /// Every other button with a LED: on/off.
    Single,
}

impl Kind {
    /// The channel says which track: a mapping on "any channel" can't be shown.
    fn per_track(self, model: Model) -> bool {
        matches!(self, Kind::ClipStop | Kind::Track) || (self == Kind::Grid && model == Model::Apc40)
    }
}

/// The LED behind `note`, if it has one (§1.3, §2.3). Tap, Shift, arrows,
/// Nudge, Stop All, Bank and the APC40's transport have none.
fn kind_of(model: Model, note: u8) -> Option<Kind> {
    match (model, note) {
        (Model::Apc40Mk2, 0x00..=0x27) | (Model::Apc40, 0x35..=0x39) => Some(Kind::Grid),
        (_, 0x52..=0x56) => Some(Kind::Scene),
        (_, NOTE_CLIP_STOP) => Some(Kind::ClipStop),
        (_, 0x30..=0x33) | (Model::Apc40Mk2, 0x42) => Some(Kind::Track),
        (_, 0x3A..=0x41 | 0x50 | 0x57..=0x5A) | (Model::Apc40Mk2, 0x5B | 0x5D | 0x66) => Some(Kind::Single),
        _ => None,
    }
}

/// What one LED should say.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Look {
    Off,
    On,
    /// Lit by its Shift mapping (e.g. page 6–8 on Scene Launch 1–3).
    Alt,
    /// A cue sits in this pad (mkII Scene Launch: that page exists).
    Present,
    Playing,
    /// Playing while the pad is held (flash / solo).
    Flashing,
}

/// Everything `render` looks at, gathered once per frame.
struct View<'a> {
    s: &'a Shared,
    /// Cue ids of the current page, grid order.
    page: Vec<Option<&'a str>>,
    /// Playing cues: id → held (flash / solo).
    playing: HashMap<&'a str, bool>,
    /// Soft blink phase (generic flash LEDs), from the tempo clock.
    beat_on: bool,
    /// Tempo clock, in beats (pulses made by the studio, T-206).
    beat: f64,
    /// APC layout: a layer's Activator (`layer.N.mute`) is lit while the
    /// layer is heard. Elsewhere a mute button is lit while muted.
    apc: bool,
    /// APC40 mkII: RGB pads, and its Scene Launch buttons show which pages exist.
    mk2: bool,
    /// The port's pad colour options (T-206).
    pad_colors: PadColors,
    pulse_by_studio: bool,
}

impl View<'_> {
    fn grid(&self, slot: usize) -> Look {
        let Some(Some(cue)) = self.page.get(slot) else { return Look::Off };
        match self.playing.get(cue) {
            Some(true) => Look::Flashing,
            Some(false) => Look::Playing,
            None if controls::show_cue_playing(self.s, cue) => Look::Playing,
            None => Look::Present,
        }
    }

    /// Whether a button mapped to `target` should be lit.
    fn lit(&self, target: &str) -> Option<bool> {
        let s = self.s;
        let desc = s.controls.get(target)?;
        let id = desc.id.as_str();
        if let Some(n) = id.strip_prefix("page.").and_then(|n| n.parse::<usize>().ok()) {
            return Some(n == s.cue_page + 1);
        }
        if let Some(rest) = id.strip_prefix("layer.") {
            let (n, param) = rest.split_once('.')?;
            let n: u8 = n.parse().ok()?;
            match param {
                // Activator: lit = the layer is heard.
                "mute" if self.apc => return Some(!s.mixer.layer(n).mute),
                // Clip Stop: lit while the layer plays something.
                "clear" => return Some(s.deck.active.iter().any(|a| a.layer == n)),
                _ => {}
            }
        }
        if matches!(id, "timeline.toggle" | "timeline.play") {
            return Some(s.timeline.is_playing());
        }
        if !matches!(desc.kind, ControlKind::Toggle { .. } | ControlKind::Momentary) {
            return None;
        }
        Some(match controls::current(s, desc)? {
            serde_json::Value::Bool(b) => b,
            v => v.as_f64()? >= 0.5,
        })
    }

    /// A `page.N` target whose page exists.
    fn page_exists(&self, target: &str) -> bool {
        let id = self.s.controls.get(target).map_or(target, |d| d.id.as_str());
        id.strip_prefix("page.").and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| (1..=self.s.grid.page_count()).contains(&n))
    }

    fn look(&self, mp: &Mapping) -> Option<Look> {
        if mp.mode == MapMode::Grid {
            let slot = mp.args.get("slot")?.as_u64()? as usize;
            return Some(self.grid(slot));
        }
        let lit = self.lit(&mp.target)?;
        Some(match (lit, mp.shift) {
            (false, _) if self.mk2 && self.page_exists(&mp.target) => Look::Present,
            (false, _) => Look::Off,
            (true, false) => Look::On,
            (true, true) => Look::Alt,
        })
    }
}

/// A LED that is not a mkII RGB pad.
fn velocity(kind: Kind, look: Look) -> u8 {
    match (kind, look) {
        (_, Look::Off) => 0,
        (Kind::Grid, look) => match look {
            Look::Present => MK1_YELLOW,
            Look::Flashing | Look::Alt => MK1_GREEN_BLINK,
            _ => MK1_GREEN,
        },
        // "That page exists" only shows on the mkII's RGB scene buttons.
        (_, Look::Present) => 0,
        (Kind::Scene | Kind::ClipStop, Look::Alt | Look::Flashing) => BLINK,
        _ => ON,
    }
}

/// An RGB LED: solid colour and, if it pulses / blinks, (mode channel,
/// secondary colour).
type RgbLed = (u8, Option<(u8, u8)>);

impl View<'_> {
    /// A mkII clip pad (`slot` = its grid slot) or scene button (T-206).
    fn rgb(&self, kind: Kind, look: Look, slot: Option<usize>) -> RgbLed {
        let cue = slot.and_then(|n| self.page.get(n).copied().flatten());
        // (present, full, secondary of a pulse / blink)
        let (present, full, secondary) = match (kind, cue.and_then(|c| palette::cue_rgb(self.s, c, self.pad_colors))) {
            (Kind::Scene, _) => (MK2_GREY, MK2_WHITE, MK2_GREY),
            (_, Some(rgb)) => {
                let full = palette::nearest_velocity(rgb);
                let dim = palette::dim_velocity(full);
                (dim, full, dim)
            }
            // « Vert / jaune », like the original APC40.
            (_, None) => (MK2_YELLOW, MK2_GREEN, MK2_DIM_GREEN),
        };
        match look {
            Look::Off => (0, None),
            Look::Present => (present, None),
            Look::On => (full, None),
            Look::Playing | Look::Alt => self.animate(full, secondary, CH_PULSE_QUARTER),
            Look::Flashing => self.animate(full, secondary, CH_BLINK_EIGHTH),
        }
    }

    /// The pad's own pulse / blink (it follows our MIDI clock), or, with
    /// « pulsation par le studio », the two colours swapped on the beat:
    /// a quarter-note pulse is bright for the first half of each beat, an
    /// eighth-note blink for the first half of each eighth.
    fn animate(&self, full: u8, secondary: u8, channel: u8) -> RgbLed {
        if !self.pulse_by_studio {
            return (full, Some((channel, secondary)));
        }
        let per_beat = if channel == CH_PULSE_QUARTER { 1.0 } else { 2.0 };
        ((if (self.beat * per_beat).rem_euclid(1.0) < 0.5 { full } else { secondary }), None)
    }
}

/// Generic LED feedback (T-211): every Note or CC mapping with a `led`
/// lights its own note / CC, on its channel (0 for "any channel"), with
/// the values the user chose. Same rule as the APCs: the plain mapping
/// speaks first, the Shift one only lights a button left dark.
fn render_generic(view: &View, mappings: &[Mapping], frame: &mut LedFrame) {
    let mut values: BTreeMap<(u8, u8), (Look, LedFeedback)> = BTreeMap::new();
    for shift in [false, true] {
        for mp in mappings.iter().filter(|mp| mp.shift == shift) {
            let Some(led) = mp.led.map(LedFeedback::clamped) else { continue };
            let kind = match mp.input.kind {
                InputKind::Note => 0x90,
                InputKind::Cc => 0xB0,
                _ => continue,
            };
            let key = (kind | mp.input.channel.unwrap_or(0) & 0x0F, mp.input.number & 0x7F);
            let look = view.look(mp).unwrap_or(Look::Off);
            let entry = values.entry(key).or_insert((Look::Off, led));
            if entry.0 == Look::Off {
                *entry = (look, led);
            }
        }
    }
    for ((status, key), (look, led)) in values {
        let v = match look {
            Look::Off => led.off,
            Look::On | Look::Playing => led.on,
            Look::Present => led.present.unwrap_or(led.off),
            Look::Alt => led.blink.unwrap_or(led.on),
            // No blink value: blink by hand with the beat.
            Look::Flashing => led.blink.unwrap_or(if view.beat_on { led.on } else { led.off }),
        };
        frame.set(status, key, v);
    }
}

/// The LEDs `port` (an APC on `driver`) should show at `t` (seconds on
/// the studio clock, `Shared::now_s`). Cheap: runs under the lock.
pub fn render(driver: Driver, s: &Shared, port: &str, t: f64) -> LedFrame {
    let model = driver.model();
    let mut frame = LedFrame::default();
    let Some(profile) = profile_of(s, port) else { return frame };
    let apc = matches!(model, Model::Apc40 | Model::Apc40Mk2);
    if !apc && !profile.mappings.iter().any(|mp| mp.led.is_some()) {
        return frame;
    }
    let beat = s.tempo.beat_at(t);
    let phase = beat.rem_euclid(1.0);
    let prefs = s.midi.store.port_prefs(port);
    let view = View {
        s,
        // Pad n = the grid's cell at row n / 8, column n % 8 (dark past its size).
        page: (0..GRID_ROWS * GRID_COLS).map(|n| s.grid.cell(s.cue_page + 1, n / GRID_COLS + 1, n % GRID_COLS + 1)).collect(),
        playing: s.deck.active.iter().fold(HashMap::new(), |mut m, a| {
            *m.entry(a.cue.as_str()).or_insert(true) &= a.held;
            m
        }),
        beat_on: phase < 0.5,
        beat,
        apc,
        mk2: model == Model::Apc40Mk2,
        pad_colors: prefs.pad_colors,
        pulse_by_studio: prefs.pulse_by_studio,
    };
    if !apc {
        render_generic(&view, &profile.mappings, &mut frame);
        return frame;
    }

    // Buttons and pads. The plain mapping speaks first; the Shift one
    // only lights a button its plain mapping leaves dark.
    let mut looks: BTreeMap<(u8, u8), (Kind, Look, Option<usize>)> = BTreeMap::new();
    for shift in [false, true] {
        for mp in profile.mappings.iter().filter(|mp| mp.shift == shift && mp.input.kind == InputKind::Note) {
            let note = mp.input.number;
            let Some(kind) = kind_of(model, note) else { continue };
            let channel = match (kind.per_track(model), mp.input.channel) {
                (true, None) => continue,
                (true, Some(c)) => c & 0x0F,
                // mkII pads: the channel is the LED mode, 0 = solid.
                (false, _) => 0,
            };
            // A mapped LED with nothing to show (a trigger) is kept dark.
            let look = view.look(mp).unwrap_or(Look::Off);
            let slot = (mp.mode == MapMode::Grid).then(|| mp.args.get("slot").and_then(|v| v.as_u64())).flatten().map(|n| n as usize);
            let entry = looks.entry((channel, note)).or_insert((kind, Look::Off, None));
            // A scene button showing "page exists" still lights for its
            // Shift page when that one is current.
            if entry.1 == Look::Off || (entry.1 == Look::Present && kind == Kind::Scene && look != Look::Off) {
                (entry.1, entry.2) = (look, slot);
            }
        }
    }
    for ((channel, note), (kind, look, slot)) in looks {
        if view.mk2 && matches!(kind, Kind::Grid | Kind::Scene) {
            frame.set_rgb(note, view.rgb(kind, look, slot));
        } else {
            frame.set(0x90 | channel, note, velocity(kind, look));
        }
    }

    // Knob rings: the value of the control each top / device knob drives,
    // centred ("pan") for ranges around zero, a bar ("volume") otherwise.
    for mp in profile.mappings.iter().filter(|mp| !mp.shift && mp.input.kind == InputKind::Cc && mp.mode == MapMode::Absolute) {
        let cc = mp.input.number;
        if !matches!(cc, 0x10..=0x17 | 0x30..=0x37) {
            continue;
        }
        let Some(desc) = s.controls.get(&mp.target) else { continue };
        let (Some((lo, hi)), Some(v)) = (range(&desc.kind, mp), controls::current(s, desc).and_then(|v| v.as_f64())) else { continue };
        if hi == lo {
            continue;
        }
        let pos = mp.curve.inverse((v as f32 - lo) / (hi - lo));
        let status = 0xB0 | mp.input.channel.unwrap_or(0) & 0x0F;
        let centred = lo.min(hi) < 0.0 && lo.max(hi) > 0.0;
        frame.set(status, cc, (pos * 127.0).round() as u8);
        frame.set(status, cc + 8, if centred { 3 } else { 2 });
    }

    // Emergency stop latched: the Clip Stop row blinks until « Réinitialiser ».
    if s.estop.is_latched() {
        let v = match model {
            Model::Apc40 => BLINK,
            // The mkII's own blink follows a MIDI clock we don't send: 2 Hz by hand.
            _ => u8::from((t * 2.0).rem_euclid(1.0) < 0.5),
        };
        for ch in 0..8 {
            frame.set(0x90 | ch, NOTE_CLIP_STOP, v);
        }
    }

    // The beat: on for the first eighth of each beat (at least 70 ms, so a
    // 30 Hz refresh never misses it).
    let beat_s = 60.0 / s.tempo.bpm.max(1.0);
    let on_s = (beat_s / 8.0).max(0.07).min(beat_s / 2.0);
    let metronome = if model == Model::Apc40 { NOTE_METRONOME_APC40 } else { NOTE_METRONOME_MK2 };
    frame.set(0x90, metronome, u8::from(phase * beat_s < on_s));
    frame
}

/// What an APC40 mkII on `port` shows on its clip pads (5 rows × 8, top
/// row first) and Scene Launch buttons, for the preview in « Contrôleur »:
/// `{ v, rgb, mode }` per LED, `mode` = "pulse", "blink" or null.
pub fn mk2_preview(s: &Shared, port: &str, t: f64) -> serde_json::Value {
    let frame = render(Driver::Apc40Mk2, s, port, t);
    let led = |note: u8| {
        let v = frame.0.get(&(0x90, note)).copied().unwrap_or(0);
        let mode = frame.1.get(&note).map(|&(ch, _)| match ch {
            6..=10 => "pulse",
            11..=15 => "blink",
            _ => "once",
        });
        serde_json::json!({ "v": v, "rgb": palette::hex(v), "mode": mode })
    };
    // Pad (row, col) is note 32 + col - 8 × row (§2.2).
    let pads: Vec<Vec<serde_json::Value>> = (0..5u8).map(|row| (0..8u8).map(|col| led(32 + col - 8 * row)).collect()).collect();
    let scenes: Vec<serde_json::Value> = (0x52..=0x56).map(led).collect();
    serde_json::json!({ "pads": pads, "scenes": scenes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::profile::Profile;
    use crate::midi::MidiDevice;
    use crate::test_support;

    const PORT: &str = "APC";

    /// A studio whose device `PORT` uses the built-in profile of `driver`.
    fn setup(driver: Driver) -> Shared {
        let mut s = test_support::shared();
        let slug = if driver == Driver::Apc40 { "apc40" } else { "apc40-mk2" };
        let mut d = MidiDevice::new(PORT);
        d.profile = slug.into();
        s.midi.devices.push(d);
        s.tempo.bpm = 120.0;
        s
    }

    /// Off-beat time on the studio clock: the metronome is dark.
    const T: f64 = 0.25;

    fn page_cue(s: &Shared, slot: usize) -> String {
        s.grid.slot(s.cue_page, slot).unwrap().to_string()
    }

    fn pad(model: Model, row: u8, col: u8) -> (u8, u8) {
        match model {
            Model::Apc40Mk2 => (0x90, 32 + col - 8 * row),
            _ => (0x90 | col, 0x35 + row),
        }
    }

    fn at(f: &LedFrame, key: (u8, u8)) -> Option<u8> {
        f.0.get(&key).copied()
    }

    fn play(s: &mut Shared, cue: &str) {
        assert!(controls::press_cue(s, cue, None, true));
    }

    #[test]
    fn apc40_pads_show_empty_present_playing_and_flash() {
        let mut s = setup(Driver::Apc40);
        let f = render(Driver::Apc40, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40, 0, 0)), Some(MK1_YELLOW), "cue present = yellow");
        let first = page_cue(&s, 0);
        play(&mut s, &first);
        let f = render(Driver::Apc40, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40, 0, 0)), Some(MK1_GREEN), "playing = green");
        assert_eq!(at(&f, pad(Model::Apc40, 0, 1)), Some(MK1_YELLOW));

        // The next cue replaces it: the old pad goes back to yellow.
        let second = page_cue(&s, 1);
        play(&mut s, &second);
        let f = render(Driver::Apc40, &s, PORT, T);
        assert_eq!((at(&f, pad(Model::Apc40, 0, 0)), at(&f, pad(Model::Apc40, 0, 1))), (Some(MK1_YELLOW), Some(MK1_GREEN)));

        // A flash cue held down blinks green.
        let third = page_cue(&s, 2);
        assert!(controls::press_cue(&mut s, &third, Some(crate::cues::ClickMode::Flash), true));
        let f = render(Driver::Apc40, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40, 0, 2)), Some(MK1_GREEN_BLINK));

        // A page with fewer than 40 cues: its empty pads are dark.
        let small = (0..s.grid.page_count()).find(|&p| s.grid.slot(p, 39).is_none());
        if let Some(page) = small {
            s.cue_page = page;
            let f = render(Driver::Apc40, &s, PORT, T);
            assert_eq!(at(&f, pad(Model::Apc40, 4, 7)), Some(0), "empty pad = off");
        }
    }

    /// The pad colour of the cue in `slot` of the current page (full, dim).
    fn colour(s: &Shared, slot: usize) -> (u8, u8) {
        let full = palette::nearest_velocity(palette::cue_rgb(s, &page_cue(s, slot), PadColors::Cue).unwrap());
        (full, palette::dim_velocity(full))
    }

    fn mode(f: &LedFrame, key: (u8, u8)) -> Option<(u8, u8)> {
        f.1.get(&key.1).copied()
    }

    fn set_pads(s: &mut Shared, colors: Option<PadColors>, by_studio: Option<bool>) {
        s.midi.store.set_port_pads(PORT, colors, by_studio).unwrap();
    }

    /// T-206: green / red / blue cues show green / red / blue, dimmed.
    #[test]
    fn mk2_pads_show_the_cue_colours_dimmed() {
        let mut s = setup(Driver::Apc40Mk2);
        // A page of our own: green, red and blue cues (colours of presets.rs).
        let ids: Vec<String> = s.presets.iter().take(3).map(|p| p.id.clone()).collect();
        for (id, rgb) in ids.iter().zip([[0, 255, 0], [255, 0, 0], [0, 80, 255]]) {
            let p = s.presets.iter_mut().find(|p| &p.id == id).unwrap();
            p.settings.color = rgb;
            p.settings.content = crate::engine::Content::Wave;
        }
        let presets = s.presets.clone();
        let page = s.grid.new_page("RGB", &presets).unwrap();
        for (slot, id) in ids.iter().enumerate() {
            s.grid.put(page, slot, Some(crate::grid::CueRef::Builtin(id.clone()))).unwrap();
        }
        s.grid.resolve(&presets);
        s.cue_page = page;
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        let row: Vec<Option<u8>> = (0..4).map(|c| at(&f, pad(Model::Apc40Mk2, 0, c))).collect();
        assert_eq!(row[0], Some(22), "dim green");
        assert_eq!(row[1], Some(palette::dim_velocity(palette::nearest_velocity([255, 0, 0]))), "dim red");
        assert!(matches!(row[1], Some(6) | Some(120) | Some(121)), "dim red: {row:?}");
        assert!((41..=47).contains(&row[2].unwrap()), "dim blue: {row:?}");
        assert_eq!(row[3], Some(0), "empty cell = dark");
        assert!(f.1.keys().all(|&n| n >= 0x52), "nothing plays: no pulse on the pads");
    }

    /// The cue playing: full colour, pulsing (1/4) to its dim colour; a
    /// held flash blinks (1/8); with the pulse made by the studio, both
    /// colours swap on the beat.
    #[test]
    fn mk2_playing_pulses_and_flash_blinks() {
        let mut s = setup(Driver::Apc40Mk2);
        let (full, dim) = colour(&s, 9);
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, T), pad(Model::Apc40Mk2, 1, 1)), Some(dim));
        let cue = page_cue(&s, 9);
        play(&mut s, &cue);
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        let key = pad(Model::Apc40Mk2, 1, 1);
        assert_eq!((at(&f, key), mode(&f, key)), (Some(full), Some((CH_PULSE_QUARTER, dim))));

        let (hfull, hdim) = colour(&s, 3);
        let held = page_cue(&s, 3);
        assert!(controls::press_cue(&mut s, &held, Some(crate::cues::ClickMode::Flash), true));
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        let key = pad(Model::Apc40Mk2, 0, 3);
        assert_eq!((at(&f, key), mode(&f, key)), (Some(hfull), Some((CH_BLINK_EIGHTH, hdim))));

        // By the studio: no LED mode, bright then dim within each beat (120 BPM).
        set_pads(&mut s, None, Some(true));
        let by_hand = |t: f64| {
            let f = render(Driver::Apc40Mk2, &s, PORT, t);
            (at(&f, pad(Model::Apc40Mk2, 1, 1)), at(&f, key), f.1.is_empty())
        };
        assert_eq!(by_hand(0.1), (Some(full), Some(hfull), true));
        assert_eq!(by_hand(0.2), (Some(full), Some(hdim), true), "the blink is twice as fast");
        assert_eq!(by_hand(0.3), (Some(dim), Some(hfull), true));
    }

    /// « Vert / jaune »: yellow / green like the APC40.
    #[test]
    fn mk2_classic_scheme_is_yellow_and_green() {
        let mut s = setup(Driver::Apc40Mk2);
        set_pads(&mut s, Some(PadColors::Classic), None);
        let cue = page_cue(&s, 1);
        play(&mut s, &cue);
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40Mk2, 0, 0)), Some(MK2_YELLOW));
        assert_eq!(at(&f, pad(Model::Apc40Mk2, 0, 1)), Some(MK2_GREEN));
        assert_eq!(mode(&f, pad(Model::Apc40Mk2, 0, 1)), Some((CH_PULSE_QUARTER, MK2_DIM_GREEN)));
    }

    /// « Couleur de la case »: the user cue's band, else the category colour.
    #[test]
    fn mk2_cell_scheme_uses_the_category_colours() {
        let mut s = setup(Driver::Apc40Mk2);
        set_pads(&mut s, Some(PadColors::Cell), None);
        s.cue_page = 1;
        let want = palette::dim_velocity(palette::nearest_velocity(palette::CATEGORY_COLORS[1]));
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, T), pad(Model::Apc40Mk2, 0, 0)), Some(want));
    }

    /// T-140: a colour changed live shows on the playing pad at the next update.
    #[test]
    fn mk2_live_colour_change_reaches_the_pad() {
        let mut s = setup(Driver::Apc40Mk2);
        let cue = page_cue(&s, 0);
        play(&mut s, &cue);
        s.settings.color = [255, 0, 0];
        s.settings.content = crate::engine::Content::Wave;
        let mut led = LedState::default();
        let t0 = Instant::now();
        led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0);
        s.settings.color = [0, 0, 255];
        let msgs = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY);
        let blue = palette::nearest_velocity([0, 0, 255]);
        // Note On channel 0 (colour), then channel 9 (pulse to the dim colour).
        assert_eq!(msgs, vec![[0x90, 32, blue], [0x90 | CH_PULSE_QUARTER, 32, palette::dim_velocity(blue)]]);
    }

    #[test]
    fn scene_launch_shows_the_page_and_blinks_for_pages_6_and_up() {
        let mut s = setup(Driver::Apc40);
        s.cue_page = 2;
        let f = render(Driver::Apc40, &s, PORT, T);
        let scenes: Vec<Option<u8>> = (0x52..=0x56).map(|n| at(&f, (0x90, n))).collect();
        assert_eq!(scenes, [Some(0), Some(0), Some(ON), Some(0), Some(0)]);
        s.cue_page = 6; // page 7 = Shift + Scene Launch 2
        let f = render(Driver::Apc40, &s, PORT, T);
        let scenes: Vec<Option<u8>> = (0x52..=0x56).map(|n| at(&f, (0x90, n))).collect();
        assert_eq!(scenes, [Some(0), Some(BLINK), Some(0), Some(0), Some(0)]);

        // mkII: current page white, the other pages grey, pages 6–10 pulse.
        let mut s = setup(Driver::Apc40Mk2);
        let scenes = |s: &Shared| {
            let f = render(Driver::Apc40Mk2, s, PORT, T);
            (0x52..=0x56).map(|n| (at(&f, (0x90, n)).unwrap(), mode(&f, (0x90, n)))).collect::<Vec<_>>()
        };
        assert!(s.grid.page_count() >= 7);
        assert_eq!(scenes(&s)[..2], [(MK2_WHITE, None), (MK2_GREY, None)]);
        s.cue_page = 6;
        assert_eq!(scenes(&s)[..2], [(MK2_GREY, None), (MK2_WHITE, Some((CH_PULSE_QUARTER, MK2_GREY)))]);
    }

    #[test]
    fn layer_buttons_show_mute_solo_and_what_plays() {
        for driver in [Driver::Apc40, Driver::Apc40Mk2] {
            let mut s = setup(driver);
            let f = render(driver, &s, PORT, T);
            assert_eq!(at(&f, (0x90, 0x32)), Some(ON), "{driver:?}: activator 1 lit = layer heard");
            assert_eq!(at(&f, (0x91, 0x31)), Some(0), "solo 2 off");
            assert_eq!(at(&f, (0x90, NOTE_CLIP_STOP)), Some(0), "nothing plays on layer 1");
            assert_eq!(at(&f, (0x94, 0x30)), Some(0), "record arm 5: audio reaction off");
            s.mixer.layer_mut(1).mute = true;
            s.mixer.layer_mut(2).solo = true;
            s.settings.audio.enabled = true;
            let cue = page_cue(&s, 0);
            play(&mut s, &cue);
            let f = render(driver, &s, PORT, T);
            assert_eq!(at(&f, (0x90, 0x32)), Some(0), "muted");
            assert_eq!(at(&f, (0x91, 0x31)), Some(ON), "solo");
            assert_eq!(at(&f, (0x90, NOTE_CLIP_STOP)), Some(ON), "layer 1 plays");
            assert_eq!(at(&f, (0x91, NOTE_CLIP_STOP)), Some(0));
            assert_eq!(at(&f, (0x94, 0x30)), Some(ON), "audio reaction on");
            assert_eq!(at(&f, (0x90, 0x62)), None, "Shift has no LED");
            assert_eq!(at(&f, (0x90, 0x51)), None, "Stop All has no LED");
            assert_eq!(at(&f, (0x90, 0x63)), None, "Tap has no LED");
        }
    }

    #[test]
    fn metronome_beats_with_the_tempo_clock() {
        for (driver, note) in [(Driver::Apc40, NOTE_METRONOME_APC40), (Driver::Apc40Mk2, NOTE_METRONOME_MK2)] {
            let mut s = setup(driver);
            let beat = |s: &Shared, t: f64| at(&render(driver, s, PORT, t), (0x90, note));
            // 120 BPM: a beat every 0.5 s, lit for 70 ms (1/8 beat = 62.5 ms).
            assert_eq!(beat(&s, 0.0), Some(1));
            assert_eq!(beat(&s, 0.06), Some(1));
            assert_eq!(beat(&s, 0.1), Some(0));
            assert_eq!(beat(&s, 0.5), Some(1));
            // 60 BPM: 1/8 beat = 125 ms.
            s.tempo.bpm = 60.0;
            assert_eq!(beat(&s, 0.1), Some(1));
            assert_eq!(beat(&s, 0.2), Some(0));
            // Tap moved the beat: the LED follows the clock, not the wall.
            s.tempo.resync(0.3);
            assert_eq!(beat(&s, 0.35), Some(1), "{driver:?}: a beat at t = 0.3");
            assert_eq!(beat(&s, 0.05), Some(0));
        }
    }

    #[test]
    fn estop_blinks_the_clip_stop_row() {
        let s = setup(Driver::Apc40);
        s.estop.trip(crate::interlock::ArmSource::Ui);
        let f = render(Driver::Apc40, &s, PORT, T);
        assert!((0..8).all(|ch| at(&f, (0x90 | ch, NOTE_CLIP_STOP)) == Some(BLINK)));

        let s = setup(Driver::Apc40Mk2);
        s.estop.trip(crate::interlock::ArmSource::Ui);
        let on = render(Driver::Apc40Mk2, &s, PORT, 0.1);
        let off = render(Driver::Apc40Mk2, &s, PORT, 0.3);
        assert!((0..8).all(|ch| at(&on, (0x90 | ch, NOTE_CLIP_STOP)) == Some(1) && at(&off, (0x90 | ch, NOTE_CLIP_STOP)) == Some(0)));
    }

    #[test]
    fn rings_show_values_and_their_type() {
        let mut s = setup(Driver::Apc40Mk2);
        s.live.pos_y = 0.0;
        s.live.perspective = 1.0;
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        assert_eq!(at(&f, (0xB0, 0x30)), Some(64), "position Y centred");
        assert_eq!(at(&f, (0xB0, 0x38)), Some(3), "pan ring for a centred value");
        assert_eq!(at(&f, (0xB0, 0x36)), Some(127), "perspective at max");
        assert_eq!(at(&f, (0xB0, 0x3E)), Some(2), "volume ring");
        s.live.pos_y = -1.0;
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, T), (0xB0, 0x30)), Some(0), "value changed by the UI");
    }

    #[test]
    fn diff_sends_only_changes_and_everything_after_a_reconnect() {
        let mut s = setup(Driver::Apc40Mk2);
        let mut led = LedState::default();
        let t0 = Instant::now();
        let full = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0);
        assert!(full.len() > 60, "first update: every LED ({})", full.len());
        let (vfull, vdim) = colour(&s, 0);
        assert!(full.contains(&[0x90, 32, vdim]) && full.contains(&[0x90, 0x52, MK2_WHITE]));

        // Idle: nothing to send (off-beat, same state).
        assert_eq!(led.update(render(Driver::Apc40Mk2, &s, PORT, T + 0.1), t0 + LED_EVERY), Vec::<[u8; 3]>::new());

        // A cue starts: its pad and the Clip Stop of its layer.
        let cue = page_cue(&s, 0);
        play(&mut s, &cue);
        let msgs = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY * 2);
        assert!(msgs.contains(&[0x90, 32, vfull]) && msgs.contains(&[0x99, 32, vdim]) && msgs.contains(&[0x90, NOTE_CLIP_STOP, 1]), "{msgs:?}");
        // (the cue's look may also flip a Record Arm toggle such as « couleur au beat »)
        assert!(msgs.iter().all(|m| m[1] == 32 || m[1] > 0x27), "no other pad: {msgs:?}");

        // Page 2: the grid is redrawn, scene 1 → 2, nothing else.
        s.cue_page = 1;
        let msgs = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY * 3);
        assert!(msgs.contains(&[0x90, 0x52, MK2_GREY]) && msgs.contains(&[0x90, 0x53, MK2_WHITE]));
        assert!(msgs.iter().all(|m| m[1] <= 0x27 || m[1] == 0x52 || m[1] == 0x53), "{msgs:?}");
        assert!(msgs.len() <= 2 * 40 + 2, "bounded: {}", msgs.len());

        // Reconnect / new Introduction: everything again.
        led.forget();
        assert!(led.due(t0));
        assert!(led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY * 4).len() > 60);
    }

    #[test]
    fn dropped_leds_are_switched_off() {
        let mut last = LedFrame::default();
        last.set(0x90, 5, 21);
        last.set(0x90, 6, 0);
        last.set(0xB0, 0x30, 40);
        let next = LedFrame::default();
        assert_eq!(next.diff(&last), vec![[0x90, 5, 0], [0xB0, 0x30, 0]]);
    }

    /// A pad that stops pulsing gets its solid colour again (channel 0
    /// ends the mode), never a black secondary colour.
    #[test]
    fn mode_changes_resend_the_colour_first() {
        let mut pulsing = LedFrame::default();
        pulsing.set_rgb(32, (21, Some((CH_PULSE_QUARTER, 22))));
        let mut solid = LedFrame::default();
        solid.set_rgb(32, (22, None));
        assert_eq!(pulsing.diff(&solid), vec![[0x90, 32, 21], [0x99, 32, 22]]);
        assert_eq!(solid.diff(&pulsing), vec![[0x90, 32, 22]]);
        // Same colour, the mode alone changes: the colour goes again first.
        let mut blinking = LedFrame::default();
        blinking.set_rgb(32, (21, Some((CH_BLINK_EIGHTH, 22))));
        assert_eq!(blinking.diff(&pulsing), vec![[0x90, 32, 21], [0x9D, 32, 22]]);
        let mut same = LedFrame::default();
        same.set_rgb(32, (21, None));
        assert_eq!(same.diff(&pulsing), vec![[0x90, 32, 21]]);
        assert!(pulsing.diff(&pulsing).is_empty());
    }

    #[test]
    fn mk2_preview_shows_colours_and_modes() {
        let mut s = setup(Driver::Apc40Mk2);
        let (full, _) = colour(&s, 0);
        let cue = page_cue(&s, 0);
        play(&mut s, &cue);
        let v = mk2_preview(&s, PORT, T);
        assert_eq!(v["pads"].as_array().unwrap().len(), 5);
        assert_eq!(v["pads"][0][0]["v"], full);
        assert_eq!(v["pads"][0][0]["mode"], "pulse");
        assert_eq!(v["pads"][0][0]["rgb"], palette::hex(full));
        assert_eq!(v["pads"][0][1]["mode"], serde_json::Value::Null);
        assert_eq!(v["scenes"][0]["v"], MK2_WHITE);
    }

    #[test]
    fn at_most_30_updates_per_second() {
        let mut led = LedState::default();
        let t0 = Instant::now();
        let mut updates = 0;
        for ms in 0..1000 {
            let now = t0 + Duration::from_millis(ms);
            if led.due(now) {
                led.update(LedFrame::default(), now);
                updates += 1;
            }
        }
        assert!((29..=31).contains(&updates), "{updates}");
        assert!(1000 / LED_EVERY.as_millis() as usize <= 30);
    }

    #[test]
    fn learned_mappings_light_too_and_generic_devices_get_nothing() {
        let mut s = setup(Driver::Apc40Mk2);
        let p = Profile::parse(
            r#"{ "name": "t", "driver": "apc40mk2", "match": { "port_contains": [] },
                "mappings": [ { "input": { "kind": "note", "number": 87 }, "target": "timeline.loop", "mode": "toggle" },
                              { "input": { "kind": "note", "number": 99 }, "target": "tempo.tap", "mode": "trigger" },
                              { "input": { "kind": "note", "number": 58 }, "target": "cue.multi", "mode": "toggle" } ] }"#,
        )
        .unwrap();
        s.midi.store.save_profile(None, "mine", p).unwrap();
        s.midi.devices[0].profile = "mine".into();
        s.deck.multi = true;
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        assert_eq!(at(&f, (0x90, 87)), Some(0), "Pan button: loop off");
        assert_eq!(at(&f, (0x90, 58)), Some(ON), "device button: multi on");
        assert_eq!(at(&f, (0x90, 99)), None, "Tap has no LED");
        assert!(render(Driver::Generic, &s, PORT, T).0.is_empty());
    }

    /// T-211: a generic controller lights only the mappings with a `led`,
    /// on their own note / CC and channel, with the values chosen.
    #[test]
    fn generic_led_feedback_uses_the_mapping_values() {
        let mut s = setup(Driver::Apc40);
        let p = Profile::parse(
            r#"{ "name": "pad", "shift_key": { "kind": "note", "number": 98 },
                "mappings": [
                  { "input": { "kind": "note", "channel": 3, "number": 10 }, "target": "cue.multi", "mode": "toggle", "led": { "off": 0, "on": 1, "blink": 2 } },
                  { "input": { "kind": "cc", "number": 20 }, "target": "layer.1.mute", "mode": "toggle", "led": { "off": 0, "on": 127 } },
                  { "input": { "kind": "note", "number": 11 }, "target": "cue.multi", "mode": "toggle" },
                  { "input": { "kind": "note", "number": 12 }, "mode": "grid", "args": { "slot": 0 }, "led": { "off": 0, "on": 21, "present": 3 } },
                  { "input": { "kind": "note", "number": 14 }, "target": "page.1", "mode": "trigger", "led": { "off": 5, "on": 6 } },
                  { "input": { "kind": "note", "number": 14 }, "shift": true, "target": "page.2", "mode": "trigger", "led": { "off": 5, "on": 6, "blink": 7 } },
                  { "input": { "kind": "note", "number": 15 }, "target": "page.3", "mode": "trigger", "led": { "off": 5, "on": 6 } },
                  { "input": { "kind": "note", "number": 15 }, "shift": true, "target": "page.1", "mode": "trigger", "led": { "off": 5, "on": 6, "blink": 7 } },
                  { "input": { "kind": "pitch_bend" }, "target": "master.size", "mode": "absolute", "led": {} } ] }"#,
        )
        .unwrap();
        s.midi.store.save_profile(None, "pad", p).unwrap();
        s.midi.devices[0].profile = "pad".into();
        let f = render(Driver::Generic, &s, PORT, T);
        assert_eq!(at(&f, (0x93, 10)), Some(0), "multi off, on channel 4");
        assert_eq!(at(&f, (0xB0, 20)), Some(0), "layer 1 not muted");
        assert_eq!(at(&f, (0x90, 11)), None, "no `led`: left alone");
        assert_eq!(at(&f, (0x90, 12)), Some(3), "cue present");
        assert_eq!(at(&f, (0x90, 14)), Some(6), "page 1 lit");
        assert_eq!(at(&f, (0x90, 15)), Some(7), "dark plain mapping: the Shift one (page 1) blinks");
        assert_eq!(f.0.len(), 5, "no metronome, no rings, no pitch bend LED: {f:?}");

        s.deck.multi = true;
        s.mixer.layer_mut(1).mute = true;
        let first = page_cue(&s, 0);
        play(&mut s, &first);
        let f = render(Driver::Generic, &s, PORT, T);
        assert_eq!(at(&f, (0x93, 10)), Some(1), "multi on");
        assert_eq!(at(&f, (0xB0, 20)), Some(127), "muted = lit (not an APC Activator)");
        assert_eq!(at(&f, (0x90, 12)), Some(21), "playing");
    }
}
