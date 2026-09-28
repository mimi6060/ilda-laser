//! Output-side safety applied to every look (T-101): a strobe limiter and a
//! provisional beam horizon. Both run on the finished frame - after the
//! layers mix, the live stage (colour, LFOs) and calibration - just before
//! the output gate. Nothing that makes light - a beat
//! gate, a brightness LFO, the audio flash, a flash cue, a fast chase -
//! can reach the laser without passing through them, and the preview
//! shows the limited frame, so it shows what the laser would draw.
//!
//! **Strobe limiter.** The limiter only looks at the light that comes
//! out: the mean drive level of the frame (the average of each point's
//! brightest colour, blanked points counting 0), which is proportional to
//! the optical power at a fixed point rate. A frame is "on" once it reaches
//! `ON_FRAC` of the recent peak and "off" once it drops to `OFF_FRAC`
//! (hysteresis, so noise can't count as flashes). Every off → on edge is a
//! flash. Flashing faster than `strobe_max_hz` (research: at most about
//! 4 Hz sustained, docs/research/festival-looks.md §4.4) starts a burst;
//! after `strobe_burst_s` (5 s) of burst the output is held continuously
//! lit - dark frames are replaced by the last lit one - for at least
//! `strobe_cooldown_s` (2 s) **and** until the input has stopped fast
//! flashing for that long. A pause shorter than the cooldown does not
//! reset the burst clock, so 4.9 s bursts with short gaps are still cut.
//!
//! **Beam horizon.** Until T-003's zones land, a plain floor: a beam (a
//! lit point held in place, which is what reads as a beam in haze) below
//! `beam_floor_y` is blanked. Outlines, text and lines are left alone.
//! The floor is in output coordinates (after calibration), i.e. where the
//! beam really goes, which is also what the preview draws.
//!
//! The settings can only be *tightened* here. Looser strobe limits are
//! refused (an expert mode is out of scope); lowering the horizon below
//! the default is an explicit setting the operator has to change.

use crate::patterns::Point;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Photosensitivity practice (festival-looks.md §4.4): sustained flashing
/// at or below 4 per second, faster bursts at most 5 s.
pub const DEFAULT_MAX_HZ: f32 = 4.0;
pub const DEFAULT_BURST_S: f32 = 5.0;
pub const DEFAULT_COOLDOWN_S: f32 = 2.0;
pub const DEFAULT_BEAM_FLOOR_Y: f32 = 0.0;

/// Global output safety settings (not part of any look), saved in
/// `safety.json`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetySettings {
    /// Fastest sustained flash rate allowed, in flashes per second.
    pub strobe_max_hz: f32,
    /// Longest burst of faster flashing, in seconds.
    pub strobe_burst_s: f32,
    /// How long the output stays steady after a burst was cut, in seconds.
    pub strobe_cooldown_s: f32,
    /// Beams (held points) below this height are blanked (-1..1, 0 = centre).
    pub beam_floor_y: f32,
}

impl Default for SafetySettings {
    fn default() -> Self {
        Self {
            strobe_max_hz: DEFAULT_MAX_HZ,
            strobe_burst_s: DEFAULT_BURST_S,
            strobe_cooldown_s: DEFAULT_COOLDOWN_S,
            beam_floor_y: DEFAULT_BEAM_FLOOR_Y,
        }
    }
}

const MAX_HZ_RANGE: (f32, f32) = (0.5, DEFAULT_MAX_HZ);
const BURST_RANGE: (f32, f32) = (0.0, DEFAULT_BURST_S);
const COOLDOWN_RANGE: (f32, f32) = (DEFAULT_COOLDOWN_S, 30.0);
const FLOOR_RANGE: (f32, f32) = (-1.0, 1.0);

impl SafetySettings {
    /// Refuses values that are not numbers, out of range, or looser than
    /// the safe defaults for the strobe (French messages for the UI).
    pub fn validate(&self) -> Result<()> {
        let check = |v: f32, (lo, hi): (f32, f32), what: &str| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("{what} : {v} hors limites ({lo} à {hi})");
            }
            Ok(())
        };
        check(self.strobe_max_hz, MAX_HZ_RANGE, "Strobe max (Hz)")?;
        check(self.strobe_burst_s, BURST_RANGE, "Rafale max (s)")?;
        check(self.strobe_cooldown_s, COOLDOWN_RANGE, "Pause après rafale (s)")?;
        check(self.beam_floor_y, FLOOR_RANGE, "Horizon des faisceaux")?;
        Ok(())
    }

    /// Clamps every value into its allowed range (NaN → default), for a
    /// file edited by hand.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let fix = |v: f32, (lo, hi): (f32, f32), def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        Self {
            strobe_max_hz: fix(self.strobe_max_hz, MAX_HZ_RANGE, d.strobe_max_hz),
            strobe_burst_s: fix(self.strobe_burst_s, BURST_RANGE, d.strobe_burst_s),
            strobe_cooldown_s: fix(self.strobe_cooldown_s, COOLDOWN_RANGE, d.strobe_cooldown_s),
            beam_floor_y: fix(self.beam_floor_y, FLOOR_RANGE, d.beam_floor_y),
        }
    }
}

pub struct SafetyStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    settings: SafetySettings,
}

impl SafetyStore {
    /// A missing or unreadable file gives the safe defaults.
    pub fn load_or_create(path: PathBuf) -> Self {
        let settings: SafetySettings = crate::load_json(&path);
        Self { path: Some(path), settings: settings.sanitized() }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { path: None, settings: SafetySettings::default() }
    }

    pub fn get(&self) -> SafetySettings {
        self.settings
    }

    pub fn set(&mut self, settings: SafetySettings) -> Result<()> {
        settings.validate()?;
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&settings).context("failed to serialize safety settings")?;
            std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
        }
        self.settings = settings;
        Ok(())
    }
}

// ---------- beam horizon ----------

/// This many coincident lit points in a row make a beam. Beams are drawn
/// with 12 (generators) or 6 (the "dots" shape) held points; outline
/// corners and line ends get 4 (`engine::CORNER_DWELL` + 1).
const BEAM_RUN: usize = 6;
/// Points closer than this (normalised units) count as the same spot, so
/// a figure shrunk to nearly nothing counts as a beam too.
const SAME_SPOT: f32 = 2e-3;

/// Blanks every beam whose position is below `floor_y`. Returns how many
/// beams were blanked. Idempotent.
pub fn blank_low_beams(points: &mut [Point], floor_y: f32) -> usize {
    let mut blanked = 0;
    let mut i = 0;
    while i < points.len() {
        let anchor = points[i];
        let mut j = i + 1;
        while j < points.len() && (points[j].x - anchor.x).abs() <= SAME_SPOT && (points[j].y - anchor.y).abs() <= SAME_SPOT {
            j += 1;
        }
        let lit = points[i..j].iter().filter(|p| p.is_lit()).count();
        if lit >= BEAM_RUN && points[i..j].iter().any(|p| p.is_lit() && p.y < floor_y) {
            for p in &mut points[i..j] {
                *p = Point::blanked(p.x, p.y);
            }
            blanked += 1;
        }
        i = j;
    }
    blanked
}

// ---------- strobe limiter ----------

/// A frame counts as "on" at this fraction of the recent peak level...
const ON_FRAC: f32 = 0.6;
/// ...and as "off" again at this fraction.
const OFF_FRAC: f32 = 0.3;
/// Below this mean level a frame is dark whatever the peak.
const MIN_LEVEL: f32 = 1e-3;
/// Time constant of the peak follower, in seconds.
const PEAK_DECAY_S: f32 = 1.0;
/// Flash onsets kept to measure the rate.
const RATE_ONSETS: usize = 8;
/// Timing slack for frame quantisation (two frames at 60 fps), so a
/// strobe at exactly the limit, sampled with jitter, is never cut.
const SLACK_S: f64 = 0.034;

/// What the limiter is doing, for the UI and `/api/frame`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct StrobeStatus {
    /// The output is being held steady (the « Limiteur actif » light).
    pub active: bool,
    /// The input is flashing faster than the limit right now.
    pub fast: bool,
    /// Measured flash rate of the input (0 when not flashing).
    pub rate_hz: f32,
    /// Seconds of the current fast burst (0 when none).
    pub burst_s: f32,
    /// Beams blanked by the horizon in the last frame.
    pub beams_blanked: usize,
}

#[derive(Default)]
pub struct StrobeLimiter {
    last_t: Option<f64>,
    peak: f32,
    on: bool,
    onsets: VecDeque<f64>,
    burst_start: Option<f64>,
    last_fast: Option<f64>,
    hold_since: Option<f64>,
    dark_since: Option<f64>,
    held: Vec<Point>,
    status: StrobeStatus,
}

/// Mean drive level of a frame: proportional to its light output.
pub fn level(frame: &[Point]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    frame.iter().map(|p| p.r.max(p.g).max(p.b).clamp(0.0, 1.0)).sum::<f32>() / frame.len() as f32
}

impl StrobeLimiter {
    pub fn status(&self) -> StrobeStatus {
        self.status
    }

    /// Takes the frame rendered at time `t` (seconds, monotonic) and
    /// returns the frame to output.
    pub fn process(&mut self, frame: Vec<Point>, t: f64, cfg: &SafetySettings) -> Vec<Point> {
        let cfg = cfg.sanitized();
        let dt = self.last_t.map_or(0.0, |last| (t - last).max(0.0)) as f32;
        self.last_t = Some(t);

        let e = level(&frame);
        self.peak = e.max(self.peak * (-dt / PEAK_DECAY_S).exp());
        let bright = e > MIN_LEVEL && e >= ON_FRAC * self.peak;
        if !self.on && bright {
            self.on = true;
            self.onsets.push_back(t);
            if self.onsets.len() > RATE_ONSETS {
                self.onsets.pop_front();
            }
        } else if self.on && (e <= MIN_LEVEL || e <= OFF_FRAC * self.peak) {
            self.on = false;
        }

        let period = 1.0 / cfg.strobe_max_hz as f64;
        let fast_since = self.fast_since(t, period);
        if let Some(start) = fast_since {
            self.last_fast = Some(t);
            self.burst_start.get_or_insert(start);
        } else if self.hold_since.is_none() && self.last_fast.is_some_and(|lf| t - lf >= cfg.strobe_cooldown_s as f64) {
            self.burst_start = None;
        }
        if self.hold_since.is_none() && self.burst_start.is_some_and(|b| t - b >= cfg.strobe_burst_s as f64 - 1e-9) {
            self.hold_since = Some(t);
        }
        if let Some(h) = self.hold_since {
            let quiet = self.last_fast.is_none_or(|lf| t - lf >= cfg.strobe_cooldown_s as f64);
            if t - h >= cfg.strobe_cooldown_s as f64 && quiet {
                self.hold_since = None;
                self.burst_start = None;
            }
        }

        let out = if bright {
            self.held.clone_from(&frame);
            self.dark_since = None;
            frame
        } else {
            let dark_for = t - *self.dark_since.get_or_insert(t);
            // While held, a dip shorter than one allowed flash period is
            // filled with the last lit frame; a longer one (the look was
            // really stopped) goes dark, which is at most one slow flash.
            if self.hold_since.is_some() && dark_for < period && !self.held.is_empty() {
                self.held.clone()
            } else {
                frame
            }
        };

        self.status = StrobeStatus {
            active: self.hold_since.is_some(),
            fast: fast_since.is_some(),
            rate_hz: self.rate(t, period),
            burst_s: self.burst_start.map_or(0.0, |b| (t - b) as f32),
            beams_blanked: 0,
        };
        out
    }

    /// When the input is flashing faster than one flash per `period`, the
    /// time of the first onset of the longest recent run that is too fast.
    /// A run of k onsets is too fast if it spans less than (k-1) periods,
    /// minus a little slack for frame timing.
    fn fast_since(&self, t: f64, period: f64) -> Option<f64> {
        let last = *self.onsets.back()?;
        if t - last > period + SLACK_S {
            return None; // stopped (or slowed) since the last flash
        }
        let n = self.onsets.len();
        (3..=n)
            .rev()
            .map(|k| self.onsets[n - k])
            .zip((3..=n).rev())
            .find(|&(first, k)| last - first < (k - 1) as f64 * period - SLACK_S)
            .map(|(first, _)| first)
    }

    /// Flash rate over the onsets that are still recent (0 when idle).
    fn rate(&self, t: f64, period: f64) -> f32 {
        let recent: Vec<f64> = self.onsets.iter().copied().filter(|&o| t - o <= 2.0).collect();
        match (recent.first(), recent.last()) {
            (Some(&a), Some(&b)) if recent.len() >= 2 && b > a && t - b <= 2.0 * period + SLACK_S => {
                ((recent.len() - 1) as f64 / (b - a)) as f32
            }
            _ => 0.0,
        }
    }
}

/// The whole T-101 stage: horizon, strobe limiter, then the horizon again
/// (a held frame may predate a horizon change). Returns the output frame.
pub fn apply(frame: Vec<Point>, t: f64, cfg: &SafetySettings, limiter: &mut StrobeLimiter) -> Vec<Point> {
    let mut frame = frame;
    let blanked = blank_low_beams(&mut frame, cfg.beam_floor_y);
    let mut out = limiter.process(frame, t, cfg);
    blank_low_beams(&mut out, cfg.beam_floor_y);
    limiter.status.beams_blanked = blanked;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, BeatClock, Content, Settings};
    use crate::generators::GenParams;

    const FPS: f64 = 60.0;

    fn lit_frame() -> Vec<Point> {
        (0..100).map(|i| Point::lit(i as f32 / 100.0, 0.5, 1.0, 1.0, 1.0)).collect()
    }

    fn dark_frame() -> Vec<Point> {
        (0..100).map(|i| Point::blanked(i as f32 / 100.0, 0.5)).collect()
    }

    /// Square-wave strobe at `hz` (duty 0..1), lit at t = 0.
    fn strobe(hz: f64, duty: f64) -> impl Fn(f64) -> bool {
        move |t: f64| (t * hz).fract() < duty
    }

    /// Runs `input(t)` (lit or dark) through a limiter at 60 fps for
    /// `secs`, returning (t, output lit?) per frame.
    fn run(input: impl Fn(f64) -> bool, secs: f64, cfg: &SafetySettings) -> (Vec<(f64, bool)>, StrobeLimiter) {
        let mut lim = StrobeLimiter::default();
        let mut out = Vec::new();
        for i in 0..(secs * FPS) as usize {
            let t = i as f64 / FPS;
            let frame = if input(t) { lit_frame() } else { dark_frame() };
            let f = lim.process(frame, t, cfg);
            out.push((t, level(&f) > 0.5));
        }
        (out, lim)
    }

    /// Time of the first frame from which the output stays lit to the end
    /// of `until` (None if it still flickers).
    fn steady_from(out: &[(f64, bool)], until: f64) -> Option<f64> {
        let within: Vec<_> = out.iter().filter(|(t, _)| *t < until).collect();
        let last_dark = within.iter().rposition(|(_, lit)| !lit);
        match last_dark {
            None => within.first().map(|(t, _)| *t),
            Some(i) if i + 1 < within.len() => Some(within[i + 1].0),
            Some(_) => None,
        }
    }

    fn edges(out: &[(f64, bool)], from: f64, to: f64) -> usize {
        let w: Vec<_> = out.iter().filter(|(t, _)| *t >= from && *t < to).collect();
        w.windows(2).filter(|p| !p[0].1 && p[1].1).count()
    }

    #[test]
    fn eight_hz_is_cut_to_steady_after_five_seconds() {
        let cfg = SafetySettings::default();
        let (out, lim) = run(strobe(8.0, 0.5), 7.0, &cfg);
        assert!(edges(&out, 0.0, 4.9) >= 38, "flashes freely during the burst");
        let steady = steady_from(&out, 7.0).expect("held steady");
        assert!((steady - 5.0).abs() <= 0.1, "cut after 5 s ± 0.1 s, got {steady}");
        assert!(lim.status().active);
        assert!(lim.status().rate_hz > 7.5 && lim.status().rate_hz < 8.5, "{:?}", lim.status());
    }

    #[test]
    fn seventeen_hz_is_cut_after_five_seconds_even_with_short_pulses() {
        let cfg = SafetySettings::default();
        // 1/8 beat at 128 BPM, 20 % duty (a stab).
        let (out, _) = run(strobe(17.0, 0.2), 7.0, &cfg);
        let steady = steady_from(&out, 7.0).expect("held steady");
        assert!((steady - 5.0).abs() <= 0.1, "got {steady}");
    }

    #[test]
    fn four_hz_and_slower_are_never_limited() {
        let cfg = SafetySettings::default();
        for hz in [2.0, 3.0, 4.0] {
            for duty in [0.1, 0.5] {
                let (out, lim) = run(strobe(hz, duty), 30.0, &cfg);
                let flashes = edges(&out, 0.0, 30.0);
                assert!(flashes as f64 >= hz * 30.0 - 2.0, "{hz} Hz: {flashes} flashes");
                assert!(out.iter().all(|&(t, lit)| lit == strobe(hz, duty)(t)), "{hz} Hz output = input");
                assert!(!lim.status().active && !lim.status().fast);
            }
        }
    }

    #[test]
    fn four_hz_with_frame_jitter_is_never_limited() {
        // Frames 10..26 ms apart (a busy machine), strobe exactly 4 Hz.
        let cfg = SafetySettings::default();
        let mut lim = StrobeLimiter::default();
        let (mut t, mut i) = (0.0, 0u64);
        let input = strobe(4.0, 0.5);
        while t < 30.0 {
            let frame = if input(t) { lit_frame() } else { dark_frame() };
            let f = lim.process(frame, t, &cfg);
            assert_eq!(level(&f) > 0.5, input(t), "t={t}");
            assert!(!lim.status().active);
            i += 1;
            t += 0.010 + (crate::beat::seeded_rand(7, i) as f64) * 0.016;
        }
    }

    #[test]
    fn a_gate_at_250_bpm_is_over_the_limit() {
        // T-100's beat gate flashes once per beat: 250 BPM = 4.17 Hz.
        let (out, _) = run(strobe(250.0 / 60.0, 0.25), 8.0, &SafetySettings::default());
        let steady = steady_from(&out, 8.0).expect("held steady");
        assert!(steady < 6.6, "cut once the burst reached 5 s (rate needs a few flashes to measure), got {steady}");
    }

    #[test]
    fn the_hold_lasts_while_the_strobe_goes_on_then_releases_after_the_cooldown() {
        let cfg = SafetySettings::default();
        // 8 Hz for 12 s, then steady light for 5 s, then 8 Hz again.
        let input = |t: f64| if !(12.0..17.0).contains(&t) { strobe(8.0, 0.5)(t) } else { true };
        let (out, _) = run(input, 20.0, &cfg);
        assert_eq!(edges(&out, 5.05, 17.0), 0, "held for as long as the fast input lasts");
        // Fast input stopped at 12 s; released at ~14 s, then fast strobing
        // is allowed again for a new burst.
        assert!(edges(&out, 17.0, 20.0) >= 20, "a new burst is allowed after the cooldown");
    }

    #[test]
    fn short_pauses_do_not_reset_the_burst_clock() {
        // 4 s of 10 Hz, 1 s dark, 4 s of 10 Hz: still one burst.
        let input = |t: f64| if (4.0..5.0).contains(&t) { false } else { strobe(10.0, 0.5)(t) };
        let (out, _) = run(input, 9.0, &SafetySettings::default());
        assert_eq!(edges(&out, 6.1, 9.0), 0, "cut 5 s into the burst (at ~6 s), not at 10 s");
    }

    #[test]
    fn stopping_the_look_while_held_goes_dark() {
        let input = |t: f64| t < 6.0 && strobe(8.0, 0.5)(t);
        let (out, _) = run(input, 8.0, &SafetySettings::default());
        assert!(out.iter().filter(|(t, _)| *t > 6.3).all(|(_, lit)| !lit), "dark 0.25 s after the look stopped");
    }

    #[test]
    fn a_tighter_setting_cuts_sooner() {
        let cfg = SafetySettings { strobe_max_hz: 2.0, strobe_burst_s: 1.0, ..Default::default() };
        let (out, _) = run(strobe(3.0, 0.5), 5.0, &cfg);
        let steady = steady_from(&out, 5.0).expect("held");
        assert!(steady < 2.0, "3 Hz is over a 2 Hz limit, cut after ~1 s: {steady}");
    }

    #[test]
    fn a_brightness_dip_that_is_not_a_flash_is_ignored() {
        // 17 Hz between 100 % and 50 %: a shimmer, not an on/off flash.
        let mut lim = StrobeLimiter::default();
        for i in 0..600 {
            let t = i as f64 / FPS;
            let k = if strobe(17.0, 0.5)(t) { 1.0 } else { 0.5 };
            let frame: Vec<Point> = (0..100).map(|j| Point::lit(j as f32 / 100.0, 0.5, k, k, k)).collect();
            lim.process(frame, t, &SafetySettings::default());
            assert!(!lim.status().active);
        }
    }

    #[test]
    fn looser_settings_are_refused_and_file_values_are_clamped() {
        let d = SafetySettings::default();
        assert!(d.validate().is_ok());
        for bad in [
            SafetySettings { strobe_max_hz: 8.0, ..d },
            SafetySettings { strobe_burst_s: 10.0, ..d },
            SafetySettings { strobe_cooldown_s: 0.5, ..d },
            SafetySettings { beam_floor_y: f32::NAN, ..d },
            SafetySettings { beam_floor_y: -2.0, ..d },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(SafetySettings { strobe_max_hz: 3.0, strobe_burst_s: 2.0, strobe_cooldown_s: 5.0, beam_floor_y: -0.5 }.validate().is_ok());
        let s = SafetySettings { strobe_max_hz: 20.0, strobe_burst_s: f32::NAN, strobe_cooldown_s: 0.0, beam_floor_y: 3.0 }.sanitized();
        assert_eq!(s, SafetySettings { strobe_max_hz: 4.0, strobe_burst_s: 5.0, strobe_cooldown_s: 2.0, beam_floor_y: 1.0 });
        let old: SafetySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, d);
    }

    #[test]
    fn the_store_saves_and_reloads() {
        let dir = std::env::temp_dir().join(format!("laser-studio-safety-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("safety.json");
        std::fs::remove_file(&path).ok();
        let mut store = SafetyStore::load_or_create(path.clone());
        assert_eq!(store.get(), SafetySettings::default());
        assert!(store.set(SafetySettings { strobe_max_hz: 9.0, ..Default::default() }).is_err());
        store.set(SafetySettings { beam_floor_y: 0.2, ..Default::default() }).unwrap();
        assert_eq!(SafetyStore::load_or_create(path).get().beam_floor_y, 0.2);
        std::fs::remove_dir_all(dir).ok();
    }

    // ---------- horizon ----------

    fn render(content: Content, secs: f32) -> Vec<Point> {
        let settings = Settings { content, brightness: 1.0, scale: 0.8, ..Default::default() };
        let mut a = Animator::default();
        let mut frame = Vec::new();
        for _ in 0..(secs * 60.0) as usize {
            frame = a.render(&settings, Default::default(), 1.0 / 60.0, &BeatClock::default());
        }
        frame
    }

    fn lit_beams_below(frame: &[Point], floor: f32) -> usize {
        let mut f = frame.to_vec();
        blank_low_beams(&mut f, floor)
    }

    #[test]
    fn beams_below_the_floor_are_blanked() {
        // A fan of beams pushed down to y = -0.5.
        let gen = |b: f32| Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 6, b, ..Default::default() } };
        let mut frame = render(gen(-0.6), 0.5);
        assert!(frame.iter().any(|p| p.is_lit()));
        assert!(frame.iter().filter(|p| p.is_lit()).all(|p| p.y < 0.0));
        let n = blank_low_beams(&mut frame, 0.0);
        assert_eq!(n, 6);
        assert!(frame.iter().all(|p| !p.is_lit()), "no lit point left");
        // Above the floor: untouched.
        let high = render(gen(0.6), 0.5);
        let mut kept = high.clone();
        assert_eq!(blank_low_beams(&mut kept, 0.0), 0);
        assert_eq!(kept, high);
        // Positions are kept (the mirrors still travel the same way).
        assert_eq!(frame.len(), render(gen(-0.6), 0.5).len());
    }

    #[test]
    fn a_beam_circle_keeps_only_its_upper_beams() {
        let content = Content::Generator { generator: "beam_circle".into(), params: GenParams { count: 8, ..Default::default() } };
        let mut frame = render(content, 0.3);
        blank_low_beams(&mut frame, 0.0);
        assert!(frame.iter().any(|p| p.is_lit()));
        assert!(frame.iter().filter(|p| p.is_lit()).all(|p| p.y >= 0.0));
        assert_eq!(lit_beams_below(&frame, 0.0), 0, "idempotent");
    }

    #[test]
    fn outlines_and_text_are_not_beams() {
        for shape in ["circle", "square", "triangle", "cross", "line", "star", "spiral"] {
            let frame = render(Content::Shape { shape: shape.into() }, 0.2);
            assert_eq!(lit_beams_below(&frame, 1.0), 0, "{shape} has no beam");
        }
        let text = render(Content::Text { text: "LASER 42".into() }, 0.2);
        assert_eq!(lit_beams_below(&text, 1.0), 0);
        let wave = render(Content::Wave, 0.2);
        assert_eq!(lit_beams_below(&wave, 1.0), 0);
        // The "dots" shape is a grid of beams: its lower half goes.
        let mut dots = render(Content::Shape { shape: "dots".into() }, 0.2);
        assert_eq!(blank_low_beams(&mut dots, 0.0), 8);
    }

    #[test]
    fn a_figure_shrunk_to_a_point_is_a_beam() {
        let settings = Settings { content: Content::Shape { shape: "circle".into() }, scale: 0.0, brightness: 1.0, ..Default::default() };
        let mut frame = Animator::default().render(&settings, Default::default(), 1.0 / 60.0, &BeatClock::default());
        for p in &mut frame {
            p.y -= 0.3;
        }
        assert!(blank_low_beams(&mut frame, 0.0) >= 1);
        assert!(frame.iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn the_stage_blanks_low_beams_and_reports_them() {
        let content = Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 4, b: -0.5, ..Default::default() } };
        let frame = render(content, 0.2);
        let mut lim = StrobeLimiter::default();
        let out = apply(frame, 0.0, &SafetySettings::default(), &mut lim);
        assert!(out.iter().all(|p| !p.is_lit()));
        assert_eq!(lim.status().beams_blanked, 4);
        // Lowering the floor (explicit setting) lets them through.
        let frame = render(Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 4, b: -0.5, ..Default::default() } }, 0.2);
        let out = apply(frame, 0.1, &SafetySettings { beam_floor_y: -1.0, ..Default::default() }, &mut lim);
        assert!(out.iter().any(|p| p.is_lit()));
    }

    #[test]
    fn a_beat_gated_look_through_the_real_renderer_is_limited() {
        // A T-100 beat gate (lit for 1/4 beat after each beat), rendered by
        // the real Animator with the clock running at 8 beats per second.
        let params = GenParams { beat_sync: true, gate_beats: 0.25, count: 5, ..Default::default() };
        let settings = Settings { content: Content::Generator { generator: "beam_fan".into(), params }, brightness: 1.0, ..Default::default() };
        let mut a = Animator::default();
        let mut lim = StrobeLimiter::default();
        let cfg = SafetySettings { beam_floor_y: -1.0, ..Default::default() };
        let mut lit_after = Vec::new();
        for i in 0..(8.0 * FPS) as usize {
            let t = i as f64 / FPS;
            // 480 BPM worth of beats = 8 gates per second.
            let clock = BeatClock { beat: t * 8.0, bpm: 480.0, beats_per_bar: 4 };
            let frame = a.render(&settings, Default::default(), 1.0 / FPS as f32, &clock);
            let out = apply(frame, t, &cfg, &mut lim);
            if t > 5.2 {
                lit_after.push(out.iter().any(|p| p.is_lit()));
            }
        }
        assert!(lit_after.iter().all(|&l| l), "steady after the burst");
        assert!(lim.status().active);
    }
}
