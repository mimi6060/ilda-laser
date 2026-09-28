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
//! Velocities and channels: Akai's public protocol documents
//! (docs/research/midi-apc40.md §1.3, §2.3). The mkII pad colours are
//! placeholders until T-206: white = cue present, green = playing.

use super::engine::{profile_of, range};
use super::mapping::{InputKind, MapMode, Mapping};
use super::profile::Driver;
use super::Model;
use crate::controls::{self, ControlKind, GRID_COLS, GRID_ROWS};
use crate::presets::CATEGORIES;
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
/// APC40 mkII palette indexes (§2.4). T-206 will pick per-cue colours.
pub const MK2_WHITE: u8 = 3;
pub const MK2_GREEN: u8 = 21;
pub const MK2_DIM_GREEN: u8 = 22;
pub const MK2_ORANGE: u8 = 9;

/// Desired state of every LED the studio drives on one device:
/// (status byte, note or CC number) → velocity or value. The status keeps
/// the channel: the APC40's grid and track buttons use it for the column.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedFrame(pub BTreeMap<(u8, u8), u8>);

impl LedFrame {
    fn set(&mut self, status: u8, key: u8, value: u8) {
        self.0.insert((status, key), value);
    }

    /// Messages that take a device showing `last` to `self`. An LED no
    /// longer driven is switched off (Note On velocity 0, CC value 0).
    pub fn diff(&self, last: &LedFrame) -> Vec<[u8; 3]> {
        let mut msgs: Vec<[u8; 3]> = self.0.iter().filter(|(k, v)| last.0.get(k) != Some(v)).map(|(&(st, key), &v)| [st, key, v]).collect();
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
    /// A cue sits in this pad.
    Present,
    Playing,
    /// Playing while the pad is held (flash / solo).
    Flashing,
}

/// Everything `render` looks at, gathered once per frame.
struct View<'a> {
    s: &'a Shared,
    /// Cue ids of the current page, grid order.
    page: Vec<&'a str>,
    /// Playing cues: id → held (flash / solo).
    playing: HashMap<&'a str, bool>,
    /// Soft blink phase (mkII flash pads), from the tempo clock.
    beat_on: bool,
}

impl View<'_> {
    fn grid(&self, slot: usize) -> Look {
        let Some(cue) = self.page.get(slot) else { return Look::Off };
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
                "mute" => return Some(!s.mixer.layer(n).mute),
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

    fn look(&self, mp: &Mapping) -> Option<Look> {
        if mp.mode == MapMode::Grid {
            let slot = mp.args.get("slot")?.as_u64()? as usize;
            return Some(self.grid(slot));
        }
        let lit = self.lit(&mp.target)?;
        Some(match (lit, mp.shift) {
            (false, _) => Look::Off,
            (true, false) => Look::On,
            (true, true) => Look::Alt,
        })
    }
}

fn velocity(model: Model, kind: Kind, look: Look, blink_on: bool) -> u8 {
    match (model, kind, look) {
        (_, _, Look::Off) => 0,
        (Model::Apc40Mk2, Kind::Grid | Kind::Scene, look) => match look {
            Look::Present => MK2_WHITE,
            Look::Alt => MK2_ORANGE,
            Look::Flashing if !blink_on => MK2_DIM_GREEN,
            _ => MK2_GREEN,
        },
        (_, Kind::Grid, look) => match look {
            Look::Present => MK1_YELLOW,
            Look::Flashing | Look::Alt => MK1_GREEN_BLINK,
            _ => MK1_GREEN,
        },
        (_, Kind::Scene | Kind::ClipStop, Look::Alt | Look::Flashing) => BLINK,
        _ => ON,
    }
}

/// The LEDs `port` (an APC on `driver`) should show at `t` (seconds on
/// the studio clock, `Shared::now_s`). Cheap: runs under the lock.
pub fn render(driver: Driver, s: &Shared, port: &str, t: f64) -> LedFrame {
    let model = driver.model();
    let mut frame = LedFrame::default();
    if !matches!(model, Model::Apc40 | Model::Apc40Mk2) {
        return frame;
    }
    let Some(profile) = profile_of(s, port) else { return frame };
    let beat = s.tempo.beat_at(t);
    let phase = beat.rem_euclid(1.0);
    let category = CATEGORIES.get(s.cue_page).copied();
    let view = View {
        s,
        page: s.presets.iter().filter(|p| Some(p.category) == category).take(GRID_ROWS * GRID_COLS).map(|p| p.id.as_str()).collect(),
        playing: s.deck.active.iter().fold(HashMap::new(), |mut m, a| {
            *m.entry(a.cue.as_str()).or_insert(true) &= a.held;
            m
        }),
        beat_on: phase < 0.5,
    };

    // Buttons and pads. The plain mapping speaks first; the Shift one
    // only lights a button its plain mapping leaves dark.
    let mut looks: BTreeMap<(u8, u8), (Kind, Look)> = BTreeMap::new();
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
            let entry = looks.entry((channel, note)).or_insert((kind, Look::Off));
            if entry.1 == Look::Off {
                entry.1 = look;
            }
        }
    }
    for ((channel, note), (kind, look)) in looks {
        frame.set(0x90 | channel, note, velocity(model, kind, look, view.beat_on));
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
        let cat = CATEGORIES[s.cue_page];
        s.presets.iter().filter(|p| p.category == cat).nth(slot).unwrap().id.clone()
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
        let small = (0..CATEGORIES.len()).find(|&p| s.presets.iter().filter(|c| c.category == CATEGORIES[p]).count() < 40);
        if let Some(page) = small {
            s.cue_page = page;
            let f = render(Driver::Apc40, &s, PORT, T);
            assert_eq!(at(&f, pad(Model::Apc40, 4, 7)), Some(0), "empty pad = off");
        }
    }

    #[test]
    fn mk2_pads_are_white_then_green_and_flash_follows_the_beat() {
        let mut s = setup(Driver::Apc40Mk2);
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40Mk2, 0, 0)), Some(MK2_WHITE));
        let cue = page_cue(&s, 9);
        play(&mut s, &cue);
        let f = render(Driver::Apc40Mk2, &s, PORT, T);
        assert_eq!(at(&f, pad(Model::Apc40Mk2, 1, 1)), Some(MK2_GREEN));

        let held = page_cue(&s, 3);
        assert!(controls::press_cue(&mut s, &held, Some(crate::cues::ClickMode::Flash), true));
        // 120 BPM: first half of a beat bright, second half dim.
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, 0.1), pad(Model::Apc40Mk2, 0, 3)), Some(MK2_GREEN));
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, 0.4), pad(Model::Apc40Mk2, 0, 3)), Some(MK2_DIM_GREEN));
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

        let mut s = setup(Driver::Apc40Mk2);
        s.cue_page = 6;
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, T), (0x90, 0x53)), Some(MK2_ORANGE));
        s.cue_page = 0;
        assert_eq!(at(&render(Driver::Apc40Mk2, &s, PORT, T), (0x90, 0x52)), Some(MK2_GREEN));
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
        assert!(full.contains(&[0x90, 32, MK2_WHITE]) && full.contains(&[0x90, 0x52, MK2_GREEN]));

        // Idle: nothing to send (off-beat, same state).
        assert_eq!(led.update(render(Driver::Apc40Mk2, &s, PORT, T + 0.1), t0 + LED_EVERY), Vec::<[u8; 3]>::new());

        // A cue starts: its pad and the Clip Stop of its layer.
        let cue = page_cue(&s, 0);
        play(&mut s, &cue);
        let msgs = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY * 2);
        assert!(msgs.contains(&[0x90, 32, MK2_GREEN]) && msgs.contains(&[0x90, NOTE_CLIP_STOP, 1]), "{msgs:?}");
        // (the cue's look may also flip a Record Arm toggle such as « couleur au beat »)
        assert!(msgs.iter().all(|m| m[1] == 32 || m[1] > 0x27), "no other pad: {msgs:?}");

        // Page 2: the grid is redrawn, scene 1 → 2, nothing else.
        s.cue_page = 1;
        let msgs = led.update(render(Driver::Apc40Mk2, &s, PORT, T), t0 + LED_EVERY * 3);
        assert!(msgs.contains(&[0x90, 0x52, 0]) && msgs.contains(&[0x90, 0x53, MK2_GREEN]));
        assert!(msgs.iter().all(|m| m[1] <= 0x27 || m[1] == 0x52 || m[1] == 0x53), "{msgs:?}");

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
}
