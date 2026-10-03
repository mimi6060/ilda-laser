//! Safety of the audio reactivity (T-245, docs/research/audio-analysis.md
//! §6.4). Audio can move brightness several times a second; this module
//! holds the guards that sit between the analysis and the looks, *before*
//! the output safety stage (calibration clamp, zones, horizon and the T-101
//! strobe limiter in safety.rs), which still runs last on every frame.
//!
//! - **Flash cap** (`FlashLimiter`): every audio route on a brightness or
//!   visibility control (`master.brightness`, `look.brightness`,
//!   `audio.flash`) goes through it. A new dip of the light may only start
//!   `1 / max_flash_hz` after the previous one; in between the light can
//!   only come back up (held lit, never held dark). So the audio alone
//!   can't flash faster than `max_flash_hz` (default 10, at most
//!   `MAX_FLASH_HZ`), and the T-101 limiter still cuts sustained flashing
//!   above its own, lower limit after its burst time. The look's own beat
//!   flash (`AudioReact.flash`) gets the same cap through its beat counter
//!   (`AudioGuard::frame` → `visual`).
//! - **Neutral when the audio goes away** (`Guard::live`): stale features,
//!   a lost device, no source, or a sustained silence make every route
//!   fade to its neutral value (no offset: the operator's look) over its
//!   own release, linearly, so it is there `release` + one frame after the
//!   engine noticed - never frozen at a loud value. A change of source
//!   (native ↔ browser) fades out the same way, forgets the event
//!   counters of the old source, then fades back in.
//! - **Silence action** (`SilenceAction`): after `SILENCE_HOLD_S` of
//!   silence (with an audio source chosen) the look is kept without
//!   modulation (*Garder*, the default), replaced by a saved scene (*Look
//!   calme*), or faded to black through the master brightness (*Noir*).
//!   None of these touch the transport: audio can never arm, disarm, latch
//!   or release a blackout, and Escape stays an immediate blackout.
//!
//! The settings are machine settings, saved with the audio input in
//! `audio.json` (`AudioConfig::safety`), never in a project.

use super::Active;
use crate::engine::AudioFeatures;
use serde::{Deserialize, Serialize};

/// Default cap on audio-driven flashes, per second.
pub const DEFAULT_MAX_FLASH_HZ: f32 = 10.0;
/// The cap can be lowered to this...
pub const MIN_FLASH_HZ: f32 = 0.5;
/// ...and never raised above this.
pub const MAX_FLASH_HZ: f32 = 10.0;
/// Silence (or no audio) this long before the silence action starts and
/// the routes fall to neutral. Stale audio needs no wait: it is already
/// `STALE` old.
pub const SILENCE_HOLD_S: f32 = 1.0;
/// *Noir*: fade to black over this long...
pub const BLACKOUT_FADE_S: f32 = 0.5;
/// ...and back up over this long once the sound returns.
pub const SOUND_BACK_FADE_S: f32 = 0.2;
/// Changes of the darkness smaller than this (fraction of the control's
/// range) are noise, not a new dip.
const FLASH_EPS: f32 = 0.01;
/// Float slack on the dip period (frame times are summed).
const PERIOD_SLACK_S: f32 = 1e-4;

/// What happens once the music stops.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceAction {
    /// *Garder* : the look stays, without audio modulation.
    #[default]
    Keep,
    /// *Look calme* : this saved scene replaces the manual look.
    CalmLook(String),
    /// *Noir* : the output fades to black (not the transport blackout).
    Blackout,
}

impl SilenceAction {
    pub fn id(&self) -> &'static str {
        match self {
            SilenceAction::Keep => "keep",
            SilenceAction::CalmLook(_) => "calm_look",
            SilenceAction::Blackout => "blackout",
        }
    }
}

/// Saved in `audio.json` (`AudioConfig::safety`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSafety {
    /// Most audio-driven flashes per second, `MIN_FLASH_HZ..=MAX_FLASH_HZ`.
    pub max_flash_hz: f32,
    pub silence_action: SilenceAction,
}

impl Default for AudioSafety {
    fn default() -> Self {
        Self { max_flash_hz: DEFAULT_MAX_FLASH_HZ, silence_action: SilenceAction::Keep }
    }
}

impl AudioSafety {
    /// Cap in range (NaN → default), a calm look without a name → keep.
    pub fn sanitized(mut self) -> Self {
        self.max_flash_hz = clamp_hz(self.max_flash_hz);
        if let SilenceAction::CalmLook(name) = &self.silence_action {
            let name = name.trim();
            self.silence_action = if name.is_empty() { SilenceAction::Keep } else { SilenceAction::CalmLook(name.to_string()) };
        }
        self
    }
}

fn clamp_hz(hz: f32) -> f32 {
    if hz.is_finite() { hz.clamp(MIN_FLASH_HZ, MAX_FLASH_HZ) } else { DEFAULT_MAX_FLASH_HZ }
}

/// What the routes may do this frame (`RouteStore::apply`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guard {
    /// Fresh, not silent audio: routes run (else they fade to neutral).
    pub live: bool,
    /// The source just changed: fade out, forget the old counters.
    pub source_changed: bool,
    pub max_flash_hz: f32,
}

impl Guard {
    /// Audio flowing, default cap (tests).
    pub const LIVE: Guard = Guard { live: true, source_changed: false, max_flash_hz: DEFAULT_MAX_FLASH_HZ };
}

/// Caps how often an audio-driven light can dip and come back. Works on
/// the *darkness* of a control (0 = the operator's level, more = dimmer):
/// it may fall (light up) at any time; it may rise (a new dip) only once
/// per `1 / max_hz`; while that is refused the light is held where it is.
/// A dip already under way may go on deepening.
#[derive(Clone, Copy, Debug)]
pub struct FlashLimiter {
    out: f32,
    dimming: bool,
    since_dip: f32,
    limited: bool,
}

impl Default for FlashLimiter {
    fn default() -> Self {
        Self { out: 0.0, dimming: false, since_dip: f32::INFINITY, limited: false }
    }
}

impl FlashLimiter {
    /// One frame: `darkness` wanted, `dt` seconds since the last frame.
    /// With `bypass` (fading to neutral) the input passes as is, still
    /// counted. Returns the darkness to apply.
    pub fn step(&mut self, darkness: f32, dt: f32, max_hz: f32, bypass: bool) -> f32 {
        let darkness = if darkness.is_finite() { darkness } else { 0.0 };
        if dt.is_finite() && dt > 0.0 {
            self.since_dip += dt;
        }
        self.limited = false;
        if darkness > self.out + FLASH_EPS && !self.dimming {
            // A new dip.
            if bypass || self.since_dip + PERIOD_SLACK_S >= 1.0 / clamp_hz(max_hz) {
                self.dimming = true;
                self.since_dip = 0.0;
                self.out = darkness;
            } else {
                self.limited = true;
            }
        } else if darkness >= self.out {
            // Deepening the current dip, or a change too small to count.
            if self.dimming || bypass {
                self.out = darkness;
            }
        } else {
            if darkness < self.out - FLASH_EPS {
                self.dimming = false;
            }
            self.out = darkness;
        }
        self.out
    }

    /// A dip was refused on the last frame.
    pub fn limited(&self) -> bool {
        self.limited
    }
}

/// Which way a control's darkness goes: −1 when a negative amount dims it
/// (brightness), +1 when a positive one does (the beat flash depth),
/// `None` for anything that doesn't change the light.
pub fn darkness_sign(id: &str) -> Option<f32> {
    match id {
        "master.brightness" | "look.brightness" => Some(-1.0),
        "audio.flash" => Some(1.0),
        _ => None,
    }
}

/// A route's share of its output, 0..1: down to 0 over its release when
/// the audio is gone, back up over its attack when it returns. Linear, so
/// neutral is reached exactly, never approached forever.
pub fn fade_step(fade: f32, live: bool, dt: f32, attack_s: f32, release_s: f32) -> f32 {
    let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    let (target, time) = if live { (1.0, attack_s) } else { (0.0, release_s) };
    if time <= 0.0 || !time.is_finite() {
        return target;
    }
    let step = dt / time;
    if live { (fade + step).min(1.0) } else { (fade - step).max(0.0) }
}

/// What the guard did on the last frame, for `/api/frame` and the UI.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct GuardStatus {
    /// Routes are running on fresh, not silent audio.
    pub live: bool,
    /// The silence action is on.
    pub silent: bool,
    /// `keep`, `calm_look` or `blackout`.
    pub action: &'static str,
    /// Output gain of *Noir* (1 = untouched).
    pub gain: f32,
    /// An audio flash was held back by the cap on the last frame.
    pub flash_limited: bool,
    pub max_flash_hz: f32,
}

impl Default for GuardStatus {
    fn default() -> Self {
        Self { live: false, silent: false, action: "keep", gain: 1.0, flash_limited: false, max_flash_hz: DEFAULT_MAX_FLASH_HZ }
    }
}

/// One frame's verdict (`AudioGuard::frame`).
#[derive(Clone, Copy, Debug)]
pub struct GuardFrame {
    pub guard: Guard,
    /// The features for the looks themselves (their beat counter capped).
    pub visual: AudioFeatures,
    /// The silence action is on.
    pub silent: bool,
    /// Multiply the master brightness copy by this (*Noir*).
    pub gain: f32,
}

/// The engine's audio guard: freshness, source changes, silence, the beat
/// cap of the looks' own flash. Engine-only state, one per engine.
#[derive(Clone, Debug)]
pub struct AudioGuard {
    last_source: Option<Active>,
    quiet_for: f32,
    gain: f32,
    shown_beat: Option<u64>,
    since_beat: f32,
    status: GuardStatus,
}

impl Default for AudioGuard {
    fn default() -> Self {
        Self { last_source: None, quiet_for: SILENCE_HOLD_S, gain: 1.0, shown_beat: None, since_beat: f32::INFINITY, status: GuardStatus::default() }
    }
}

impl AudioGuard {
    /// `features`/`active`: this frame's `AudioHub::frame`; `source_on`:
    /// an audio source is chosen (not *Aucune*). Never touches arming.
    pub fn frame(&mut self, features: &AudioFeatures, active: Active, source_on: bool, cfg: &AudioSafety, dt: f32) -> GuardFrame {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let fresh = active != Active::None;
        let source_changed = fresh && self.last_source.is_some_and(|last| last != active);
        if fresh {
            self.last_source = Some(active);
        }
        if !fresh || features.silent {
            self.quiet_for += dt;
        } else {
            self.quiet_for = 0.0;
        }
        let quiet = self.quiet_for >= SILENCE_HOLD_S;
        let live = fresh && !quiet;
        let silent = source_on && quiet;
        let black = silent && cfg.silence_action == SilenceAction::Blackout;
        self.gain = if black { (self.gain - dt / BLACKOUT_FADE_S).max(0.0) } else { (self.gain + dt / SOUND_BACK_FADE_S).min(1.0) };

        // The looks' beat flash: its counter moves at most max_flash_hz.
        let max_hz = clamp_hz(cfg.max_flash_hz);
        self.since_beat += dt;
        let shown = match self.shown_beat {
            Some(b) if b != features.beat && self.since_beat + PERIOD_SLACK_S >= 1.0 / max_hz => {
                self.since_beat = 0.0;
                features.beat
            }
            Some(b) => b,
            None => features.beat,
        };
        self.shown_beat = Some(shown);
        let visual = AudioFeatures { beat: shown, ..*features };

        self.status = GuardStatus { live, silent, action: cfg.silence_action.id(), gain: self.gain, flash_limited: false, max_flash_hz: max_hz };
        GuardFrame { guard: Guard { live, source_changed, max_flash_hz: max_hz }, visual, silent, gain: self.gain }
    }

    /// Records whether the routes held a flash back this frame.
    pub fn set_flash_limited(&mut self, limited: bool) {
        self.status.flash_limited = limited;
    }

    pub fn status(&self) -> GuardStatus {
        self.status
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const FPS60: f32 = 1.0 / 60.0;

    /// Off → on edges of a 0..1 light level, with hysteresis (on at 0.6,
    /// off at 0.3), as the T-101 limiter counts them.
    pub(crate) fn count_flashes(levels: impl IntoIterator<Item = f32>) -> usize {
        let (mut on, mut n) = (false, 0);
        for l in levels {
            if !on && l >= 0.6 {
                on = true;
                n += 1;
            } else if on && l <= 0.3 {
                on = false;
            }
        }
        n
    }

    #[test]
    fn a_20_hz_square_dip_is_capped_at_the_rate() {
        for max_hz in [10.0, 4.0, 2.0] {
            let mut lim = FlashLimiter::default();
            // Dark/light every 3 frames: 10 dips a second at 60 fps... and
            // every other frame: 30 a second.
            for period in [6usize, 2] {
                let levels: Vec<f32> = (0..600)
                    .map(|i| {
                        let dark = if (i / (period / 2)) % 2 == 0 { 1.0 } else { 0.0 };
                        1.0 - lim.step(dark, FPS60, max_hz, false)
                    })
                    .collect();
                let flashes = count_flashes(levels) as f32 / 10.0;
                assert!(flashes <= max_hz + 0.1, "{max_hz} Hz cap, {period}-frame square: {flashes} flashes/s");
                assert!(flashes >= max_hz.min(60.0 / period as f32) * 0.8, "still flashes: {flashes}");
            }
        }
    }

    #[test]
    fn the_light_comes_back_at_once_and_is_never_held_dark() {
        let mut lim = FlashLimiter::default();
        assert_eq!(lim.step(1.0, FPS60, 10.0, false), 1.0, "the first dip is free");
        assert_eq!(lim.step(0.0, FPS60, 10.0, false), 0.0, "back up at once");
        assert_eq!(lim.step(1.0, FPS60, 10.0, false), 0.0, "a second dip too soon: held lit");
        assert!(lim.limited());
        for _ in 0..3 {
            lim.step(1.0, FPS60, 10.0, false);
        }
        assert_eq!(lim.step(1.0, FPS60, 10.0, false), 1.0, "allowed after 100 ms");
        assert!(!lim.limited());
        // A slow fade down follows exactly (one dip, deepening).
        let mut lim = FlashLimiter::default();
        for i in 0..=60 {
            let d = i as f32 / 60.0;
            assert!((lim.step(d, FPS60, 10.0, false) - d).abs() < 0.02, "{i}");
        }
    }

    #[test]
    fn bypass_passes_the_input() {
        let mut lim = FlashLimiter::default();
        lim.step(1.0, FPS60, 10.0, false);
        lim.step(0.0, FPS60, 10.0, false);
        assert_eq!(lim.step(0.5, FPS60, 10.0, true), 0.5);
    }

    #[test]
    fn fade_reaches_neutral_in_its_release() {
        let mut f = 1.0;
        let mut frames = 0;
        while f > 0.0 {
            f = fade_step(f, false, FPS60, 0.01, 0.15);
            frames += 1;
        }
        assert!(frames as f32 <= 0.15 / FPS60 + 1.0, "{frames}");
        assert_eq!(fade_step(0.3, false, FPS60, 0.0, 0.0), 0.0);
        assert_eq!(fade_step(0.3, true, FPS60, 0.0, 0.0), 1.0);
        assert!(fade_step(0.0, true, FPS60, 0.1, 0.0) > 0.1);
    }

    #[test]
    fn serde_defaults_sanitize_and_format() {
        let s: AudioSafety = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AudioSafety { max_flash_hz: 10.0, silence_action: SilenceAction::Keep });
        let s: AudioSafety = serde_json::from_str(r#"{"max_flash_hz":99,"silence_action":{"calm_look":"  Doux "}}"#).unwrap();
        let s = s.sanitized();
        assert_eq!(s, AudioSafety { max_flash_hz: MAX_FLASH_HZ, silence_action: SilenceAction::CalmLook("Doux".into()) });
        let s = AudioSafety { max_flash_hz: f32::NAN, silence_action: SilenceAction::CalmLook(" ".into()) }.sanitized();
        assert_eq!(s, AudioSafety::default());
        assert_eq!(AudioSafety { max_flash_hz: 0.0, ..Default::default() }.sanitized().max_flash_hz, MIN_FLASH_HZ);
        assert_eq!(serde_json::to_value(SilenceAction::Blackout).unwrap(), "blackout");
        assert_eq!(serde_json::to_value(SilenceAction::CalmLook("A".into())).unwrap(), serde_json::json!({ "calm_look": "A" }));
    }

    fn loud() -> AudioFeatures {
        AudioFeatures { level: 0.8, bass: 0.8, level_db: -12.0, ..Default::default() }
    }

    #[test]
    fn guard_goes_neutral_on_stale_audio_silence_and_source_change() {
        let cfg = AudioSafety::default();
        let mut g = AudioGuard::default();
        assert!(!g.frame(&AudioFeatures::default(), Active::None, true, &cfg, FPS60).guard.live, "nothing yet");
        let f = g.frame(&loud(), Active::Browser, true, &cfg, FPS60);
        assert!(f.guard.live && !f.guard.source_changed && !f.silent);
        // Stale: at once.
        assert!(!g.frame(&loud().neutral(), Active::None, true, &cfg, FPS60).guard.live);
        assert!(g.frame(&loud(), Active::Browser, true, &cfg, FPS60).guard.live);
        // Silent hops: only after the hold.
        let quiet = AudioFeatures { silent: true, ..Default::default() };
        let mut frames = 0;
        while g.frame(&quiet, Active::Browser, true, &cfg, FPS60).guard.live {
            frames += 1;
        }
        assert!((frames as f32 - SILENCE_HOLD_S / FPS60).abs() <= 1.0, "{frames}");
        assert!(g.status().silent && !g.status().live);
        // Native takes over: a source change.
        let f = g.frame(&loud(), Active::Native, true, &cfg, FPS60);
        assert!(f.guard.source_changed && f.guard.live && !f.silent);
        assert!(!g.frame(&loud(), Active::Native, true, &cfg, FPS60).guard.source_changed);
    }

    #[test]
    fn silence_blackout_fades_and_comes_back_without_touching_anything_else() {
        let cfg = AudioSafety { silence_action: SilenceAction::Blackout, ..Default::default() };
        let mut g = AudioGuard::default();
        g.frame(&loud(), Active::Browser, true, &cfg, FPS60);
        let mut gains = Vec::new();
        for _ in 0..120 {
            gains.push(g.frame(&AudioFeatures::default().neutral(), Active::None, true, &cfg, FPS60).gain);
        }
        assert!(gains.windows(2).all(|w| w[1] <= w[0]), "fades, never jumps up");
        assert_eq!(*gains.last().unwrap(), 0.0);
        assert!(gains[(SILENCE_HOLD_S / FPS60) as usize - 2] == 1.0, "not before the hold");
        // No source chosen: no silence action.
        let mut g2 = AudioGuard::default();
        assert_eq!(g2.frame(&AudioFeatures::default(), Active::None, false, &cfg, 2.0).gain, 1.0);
        // Sound back: up within SOUND_BACK_FADE_S.
        let mut n = 0;
        while g.frame(&loud(), Active::Browser, true, &cfg, FPS60).gain < 1.0 {
            n += 1;
        }
        assert!(n as f32 <= SOUND_BACK_FADE_S / FPS60 + 1.0);
    }

    #[test]
    fn the_looks_beat_flash_is_capped() {
        let cfg = AudioSafety { max_flash_hz: 5.0, ..Default::default() };
        let mut g = AudioGuard::default();
        let mut f = loud();
        let mut changes = 0;
        let mut last = None;
        for i in 0..600 {
            if i % 3 == 0 {
                f.beat += 1; // 20 beats a second
            }
            let v = g.frame(&f, Active::Browser, true, &cfg, FPS60).visual;
            if last.is_some_and(|l| l != v.beat) {
                changes += 1;
            }
            last = Some(v.beat);
            assert_eq!((v.bass, v.kick), (f.bass, f.kick), "only the beat counter is held");
        }
        assert!((40..=50).contains(&changes), "{changes} beats in 10 s");
    }
}
