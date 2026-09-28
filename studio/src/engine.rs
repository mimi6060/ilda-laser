//! Turns the current look (`Settings`) plus live audio features into one
//! frame of laser points, 60 times a second. Everything time-based
//! (rotation, wave travel, beat color steps, beat flashes) lives in
//! `Animator`, so a `Settings` value stays a plain, saveable description
//! of a look - the same thing a scene stores.

use crate::beat;
use crate::evolving::{self, EvolvingCue};
use crate::font;
use crate::generators::{self, GenCtx, GenParams};
use crate::patterns::{self, Point};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    Shape { shape: String },
    Text { text: String },
    Wave,
    /// A procedural effect from `generators.rs` (what most cues use).
    Generator {
        generator: String,
        #[serde(default)]
        params: GenParams,
    },
    /// An evolving cue (T-111): keyframes over N beats (`evolving.rs`).
    Evolving(EvolvingCue),
}

impl Content {
    /// Whether both draw the same thing: same kind, same shape, same
    /// generator. The text itself and generator parameters are edits of
    /// that drawing, not a new one.
    pub fn same_drawing(&self, other: &Content) -> bool {
        match (self, other) {
            (Content::Shape { shape: a }, Content::Shape { shape: b }) => a == b,
            (Content::Generator { generator: a, .. }, Content::Generator { generator: b, .. }) => a == b,
            (Content::Text { .. }, Content::Text { .. })
            | (Content::Wave, Content::Wave)
            | (Content::Evolving(_), Content::Evolving(_)) => true,
            _ => false,
        }
    }
}

/// How strongly the music drives the look. Every amount is 0.0..=1.0, and
/// 0.0 means "ignore the music for this parameter".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioReact {
    pub enabled: bool,
    /// Size follows the bass.
    pub size: f32,
    /// Extra rotation speed from the bass (up to one turn per second).
    pub rotate: f32,
    /// Step the hue on every beat.
    pub color_on_beat: bool,
    /// Dim between beats, full brightness on each beat.
    pub flash: f32,
}

impl Default for AudioReact {
    fn default() -> Self {
        Self { enabled: false, size: 0.5, rotate: 0.0, color_on_beat: true, flash: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub content: Content,
    pub color: [u8; 3],
    /// Half-extent of the content, 0.0..=1.0.
    pub scale: f32,
    /// Degrees per second, negative for counter-clockwise.
    pub rotation_speed: f32,
    /// Master brightness, 0.0..=1.0.
    pub brightness: f32,
    pub audio: AudioReact,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            content: Content::Shape { shape: "circle".to_string() },
            color: [0, 255, 0],
            scale: 0.5,
            rotation_speed: 0.0,
            brightness: 0.5,
            audio: AudioReact::default(),
        }
    }
}

/// Features the browser extracts from the microphone (see `index.html`).
/// `level` and `bass` are 0.0..=1.0; `beat` counts detected beats, so a
/// change means "a new beat happened since the last frame".
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct AudioFeatures {
    pub level: f32,
    pub bass: f32,
    pub beat: u64,
}

/// Global output alignment, applied to every point last - the laser
/// equivalent of a projector's position/size/keystone settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub offset_x: f32,
    pub offset_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_deg: f32,
}

impl Default for Calibration {
    fn default() -> Self {
        Self { offset_x: 0.0, offset_y: 0.0, scale_x: 1.0, scale_y: 1.0, rotation_deg: 0.0 }
    }
}

impl Calibration {
    /// Rotate around the origin, scale, then offset, clamped to the valid
    /// -1.0..=1.0 range.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let (xr, yr) = rotate(x, y, self.rotation_deg);
        (
            (xr * self.scale_x + self.offset_x).clamp(-1.0, 1.0),
            (yr * self.scale_y + self.offset_y).clamp(-1.0, 1.0),
        )
    }
}

fn rotate(x: f32, y: f32, degrees: f32) -> (f32, f32) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

/// Longest distance between two consecutive output points. Galvos can't
/// jump: a 4-corner square sent as-is at 30k points/s is drawn as a
/// blurry blob, so long segments are split into steps no larger than this.
const MAX_STEP: f32 = 0.03;

/// Extra copies of a sharp corner, so the mirrors have time to turn
/// before the beam moves on - otherwise corners come out rounded.
const CORNER_DWELL: usize = 3;

/// Direction changes sharper than this (degrees) count as corners.
const CORNER_ANGLE_DEG: f32 = 30.0;

/// The tempo clock as one frame sees it (read from `tempo::TempoClock`,
/// the app's only tempo clock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatClock {
    /// `TempoClock::beat_at(now)`.
    pub beat: f64,
    pub bpm: f64,
    pub beats_per_bar: u8,
}

impl Default for BeatClock {
    fn default() -> Self {
        Self { beat: 0.0, bpm: 120.0, beats_per_bar: 4 }
    }
}

#[derive(Default)]
pub struct Animator {
    angle_deg: f32,
    hue_shift: f32,
    wave_phase: f32,
    gen_time: f32,
    flash: f32,
    last_beat: u64,
    /// Tempo-clock beat the look started at (the cue's launch); `None`
    /// until then = the first rendered frame.
    start_beat: Option<f64>,
    /// Whether a frame has been rendered yet.
    rendered: bool,
    /// The evolving cue being played and the beat it counts from (its
    /// quantized launch). A different evolving cue starts over.
    evolving: Option<(EvolvingCue, f64)>,
    progress: Option<evolving::Progress>,
}

impl Animator {
    /// An animator for a cue launched at tempo-clock beat `beat`.
    pub fn starting_at(beat: f64) -> Self {
        Self { start_beat: Some(beat), ..Default::default() }
    }

    /// Beats since the look started, counted from the first beat of the
    /// bar it started in, so beat-synced motion lands on the "one".
    fn beat_pos(&mut self, clock: &BeatClock) -> f64 {
        let start = *self.start_beat.get_or_insert(clock.beat);
        (clock.beat - beat::bar_start(start, clock.beats_per_bar)).max(0.0)
    }

    /// Where the evolving cue on show is (`None` for any other look), as of
    /// the last rendered frame.
    pub fn progress(&self) -> Option<&evolving::Progress> {
        self.progress.as_ref()
    }

    pub fn render(&mut self, s: &Settings, audio: AudioFeatures, dt: f32, clock: &BeatClock) -> Vec<Point> {
        let points = match &s.content {
            Content::Evolving(cue) => self.render_evolving(cue, s, audio, dt, clock),
            _ => {
                self.evolving = None;
                self.progress = None;
                let beat_pos = self.beat_pos(clock);
                self.render_look(s, audio, dt, clock, beat_pos)
            }
        };
        self.rendered = true;
        points
    }

    /// An evolving cue: sample its keys at its local beat and draw that
    /// look like any generator look, with the local beat as `beat_pos`.
    fn render_evolving(&mut self, cue: &EvolvingCue, s: &Settings, audio: AudioFeatures, dt: f32, clock: &BeatClock) -> Vec<Point> {
        let start = *self.start_beat.get_or_insert(clock.beat);
        if self.evolving.as_ref().is_none_or(|(playing, _)| playing != cue) {
            // A cue counts from its launch; a look switched to (or edited
            // into) another evolving cue counts from now.
            let pressed = if self.rendered { clock.beat } else { start };
            self.evolving = Some((cue.clone(), cue.launch.quantize(pressed, clock.beats_per_bar)));
        }
        let launch = self.evolving.as_ref().map_or(start, |(_, at)| *at);
        let raw = clock.beat - launch;
        let b = cue.local(raw);
        let sample = cue.sample(b);
        self.progress = Some(evolving::Progress {
            pos: b,
            length: cue.length(),
            key: sample.as_ref().map_or(0, |k| k.key),
            keys: cue.keys.len(),
            looped: cue.looped,
            waiting: raw < 0.0,
            ended: cue.ended(raw),
            pass: if cue.looped && raw > 0.0 { (raw / cue.length()).floor() as u64 } else { 0 },
            beats_per_bar: clock.beats_per_bar,
        });
        let Some(k) = sample else { return Vec::new() };
        let look = Settings {
            content: Content::Generator { generator: k.generator, params: k.params },
            color: k.color,
            scale: k.scale,
            rotation_speed: s.rotation_speed,
            brightness: s.brightness.clamp(0.0, 1.0) * k.brightness * evolving::strobe(b, k.strobe_div),
            audio: s.audio.clone(),
        };
        self.render_look(&look, audio, dt, clock, b)
    }

    fn render_look(&mut self, s: &Settings, audio: AudioFeatures, dt: f32, clock: &BeatClock, beat_pos: f64) -> Vec<Point> {
        let react = &s.audio;
        let (bass, level) = if react.enabled {
            (audio.bass.clamp(0.0, 1.0), audio.level.clamp(0.0, 1.0))
        } else {
            (0.0, 0.0)
        };

        if audio.beat != self.last_beat {
            self.last_beat = audio.beat;
            if react.enabled {
                if react.color_on_beat {
                    self.hue_shift = (self.hue_shift + 1.0 / 6.0).fract();
                }
                self.flash = 1.0;
            }
        }
        self.flash *= (-dt * 6.0).exp();

        self.angle_deg = (self.angle_deg + dt * (s.rotation_speed + react.rotate * 360.0 * bass)) % 360.0;
        self.wave_phase = (self.wave_phase + dt * (3.0 + 12.0 * level)) % std::f32::consts::TAU;
        if let Content::Generator { params, .. } = &s.content {
            // Bass speeds generators up, so their motion follows the music.
            self.gen_time += dt * params.speed * (1.0 + 2.0 * bass);
        }

        // size = 0 leaves the scale alone; size = 1 swings it 0.5x..1.5x.
        let scale = s.scale * (1.0 - 0.5 * react.size * (react.enabled as u8 as f32) + react.size * bass);
        let hue_shift = if react.enabled { self.hue_shift } else { 0.0 };
        let (r, g, b) = shift_hue(s.color, hue_shift);
        let flash_gain = 1.0 - react.flash * (react.enabled as u8 as f32) * (1.0 - self.flash);
        let mut gain = s.brightness.clamp(0.0, 1.0) * flash_gain;
        let synced = match &s.content {
            Content::Generator { params, .. } if params.beat_sync => Some(params),
            _ => None,
        };
        if let Some(p) = synced.filter(|p| p.gate_beats > 0.0) {
            gain *= beat::env_stab(beat_pos.rem_euclid(1.0) as f32, p.gate_beats, 0.0);
        }
        let (r, g, b) = (r * gain, g * gain, b * gain);

        let points = match &s.content {
            Content::Shape { shape } => patterns::by_name(shape, scale, r, g, b).unwrap_or_default(),
            Content::Text { text } => font::text_to_points(&text.to_uppercase(), scale, r, g, b),
            Content::Wave => patterns::wave(scale, 0.15 + 0.6 * level, self.wave_phase, r, g, b),
            // Drawn through `render_evolving`, never directly.
            Content::Evolving(_) => Vec::new(),
            Content::Generator { generator, params } => {
                let t = if params.beat_sync { beat_time(params, beat_pos) } else { self.gen_time };
                let ctx = GenCtx { t, beat_pos, bpm: clock.bpm as f32, level, bass, scale };
                match generators::generate(generator, params, &ctx) {
                    Some(geo) => {
                        let (r2, g2, b2) = shift_hue(params.color2, hue_shift);
                        let c2 = (r2 * gain, g2 * gain, b2 * gain);
                        generators::colorize(&geo, params.color_mode, (r, g, b), c2, t, gain)
                    }
                    None => Vec::new(),
                }
            }
        };

        let rotated: Vec<Point> = points
            .into_iter()
            .map(|p| {
                let (x, y) = rotate(p.x, p.y, self.angle_deg);
                Point { x, y, ..p }
            })
            .collect();
        densify(&rotated)
    }
}

/// Generator time for a beat-synced look: one 2π cycle per
/// `period_beats`, backwards when `direction` < 0, so every motion that
/// repeats every 2π of `t` repeats exactly every period at any BPM.
/// `speed` and the bass boost don't apply: the tempo sets the pace.
fn beat_time(p: &GenParams, beat_pos: f64) -> f32 {
    let period = if p.period_beats > 0.0 { p.period_beats as f64 } else { 4.0 };
    let dir = if p.direction < 0 { -1.0 } else { 1.0 };
    (dir * std::f64::consts::TAU * beat_pos / period) as f32
}

/// Split long segments into `MAX_STEP`-sized steps and hold sharp
/// corners for `CORNER_DWELL` extra points. Blanked (travel) segments are
/// split too, so the mirrors move at a controlled speed with the beam off.
pub fn densify(points: &[Point]) -> Vec<Point> {
    let mut out = Vec::with_capacity(points.len() * 2);
    for (i, &p) in points.iter().enumerate() {
        if i > 0 {
            let prev = points[i - 1];
            let dist = ((p.x - prev.x).powi(2) + (p.y - prev.y).powi(2)).sqrt();
            let steps = (dist / MAX_STEP).ceil() as usize;
            let lit = prev.is_lit() && p.is_lit();
            for k in 1..steps {
                let t = k as f32 / steps as f32;
                let x = prev.x + (p.x - prev.x) * t;
                let y = prev.y + (p.y - prev.y) * t;
                out.push(if lit { Point { x, y, ..p } } else { Point::blanked(x, y) });
            }
        }
        out.push(p);
        if is_corner(points, i) {
            for _ in 0..CORNER_DWELL {
                out.push(p);
            }
        }
    }
    out
}

/// Chain several rendered looks into one frame, with blanked travel
/// between them so the beam never draws a line from one look to the next.
/// (The jump from the frame's end back to its start is blanked by the
/// output, as for a single look.)
pub fn join_looks(looks: Vec<Vec<Point>>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    for look in looks.into_iter().filter(|l| !l.is_empty()) {
        if let (Some(&last), Some(&first)) = (out.last(), look.first()) {
            let travel = densify(&[Point::blanked(last.x, last.y), Point::blanked(first.x, first.y)]);
            out.extend(travel);
        }
        out.extend(look);
    }
    out
}

fn is_corner(points: &[Point], i: usize) -> bool {
    if i == 0 || i + 1 >= points.len() {
        return true; // endpoints always get a dwell
    }
    let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
    let (d1x, d1y) = (b.x - a.x, b.y - a.y);
    let (d2x, d2y) = (c.x - b.x, c.y - b.y);
    let (l1, l2) = ((d1x * d1x + d1y * d1y).sqrt(), (d2x * d2x + d2y * d2y).sqrt());
    if l1 < 1e-6 || l2 < 1e-6 {
        return false; // already a dwell
    }
    let cos = ((d1x * d2x + d1y * d2y) / (l1 * l2)).clamp(-1.0, 1.0);
    cos.acos().to_degrees() > CORNER_ANGLE_DEG
}

/// Rotate an RGB color's hue by `shift` turns (0.0..1.0), returned as
/// 0.0..=1.0 floats.
fn shift_hue(rgb: [u8; 3], shift: f32) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    if shift == 0.0 {
        return (r, g, b);
    }
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta < 1e-6 {
        return (r, g, b); // grey/white has no hue to rotate
    }
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    let sat = delta / max;
    hsv_to_rgb((hue + shift).fract(), sat, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h6 = h * 6.0;
    let i = h6.floor() as i32 % 6;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_step(points: &[Point]) -> f32 {
        points
            .windows(2)
            .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
            .fold(0.0, f32::max)
    }

    #[test]
    fn densify_limits_step_size() {
        let square = patterns::square(0.8, 1.0, 1.0, 1.0);
        let dense = densify(&square);
        assert!(max_step(&dense) <= MAX_STEP + 1e-4);
        assert!(dense.len() > square.len() * 10);
    }

    #[test]
    fn densify_keeps_blanked_travel_blanked() {
        let pts = vec![Point::lit(-0.5, 0.0, 1.0, 1.0, 1.0), Point::blanked(0.5, 0.0)];
        let dense = densify(&pts);
        assert!(dense[1..dense.len() - 1].iter().filter(|p| p.x > -0.5 && p.x < 0.5).all(|p| !p.is_lit()));
    }

    #[test]
    fn square_corners_get_dwell_points() {
        let dense = densify(&patterns::square(0.5, 1.0, 1.0, 1.0));
        let corner = dense.iter().filter(|p| (p.x - 0.5).abs() < 1e-6 && (p.y + 0.5).abs() < 1e-6).count();
        assert_eq!(corner, 1 + CORNER_DWELL);
    }

    #[test]
    fn circle_is_not_treated_as_all_corners() {
        let circle = patterns::circle(0.5, 1.0, 1.0, 1.0);
        let dense = densify(&circle);
        assert!(dense.len() < circle.len() + 2 * (CORNER_DWELL + 1) + 10);
    }

    #[test]
    fn hue_shift_of_zero_is_identity_and_white_stays_white() {
        assert_eq!(shift_hue([255, 0, 0], 0.0), (1.0, 0.0, 0.0));
        assert_eq!(shift_hue([255, 255, 255], 0.3), (1.0, 1.0, 1.0));
    }

    #[test]
    fn hue_shift_by_a_third_turns_red_into_green() {
        let (r, g, b) = shift_hue([255, 0, 0], 1.0 / 3.0);
        assert!(r < 0.01 && (g - 1.0).abs() < 0.01 && b < 0.01, "got {r},{g},{b}");
    }

    #[test]
    fn brightness_scales_colors() {
        let mut a = Animator::default();
        let s = Settings { brightness: 0.5, color: [255, 255, 255], ..Default::default() };
        let pts = a.render(&s, AudioFeatures::default(), 1.0 / 60.0, &BeatClock::default());
        assert!(pts.iter().all(|p| (p.r - 0.5).abs() < 1e-6));
    }

    #[test]
    fn audio_is_ignored_when_disabled() {
        let s = Settings::default();
        let loud = AudioFeatures { level: 1.0, bass: 1.0, beat: 7 };
        let quiet = a_frame(&s, AudioFeatures::default());
        assert_eq!(a_frame(&s, loud), quiet);
    }

    #[test]
    fn bass_grows_the_shape_when_size_reaction_is_on() {
        let mut s = Settings::default();
        s.audio.enabled = true;
        s.audio.size = 1.0;
        let extent = |pts: Vec<Point>| pts.iter().map(|p| p.x.abs()).fold(0.0, f32::max);
        let quiet = extent(a_frame(&s, AudioFeatures { bass: 0.0, ..Default::default() }));
        let loud = extent(a_frame(&s, AudioFeatures { bass: 1.0, ..Default::default() }));
        assert!(loud > quiet * 2.5, "quiet {quiet}, loud {loud}");
    }

    #[test]
    fn a_beat_steps_the_hue() {
        let mut s = Settings::default();
        s.audio.enabled = true;
        s.color = [255, 0, 0];
        let mut a = Animator::default();
        let before = a.render(&s, AudioFeatures::default(), 0.016, &BeatClock::default())[0];
        let after = a.render(&s, AudioFeatures { beat: 1, ..Default::default() }, 0.016, &BeatClock::default())[0];
        assert!(after.g > before.g, "hue did not move: {before:?} -> {after:?}");
    }

    #[test]
    fn calibration_is_identity_by_default_and_clamps() {
        let c = Calibration::default();
        assert_eq!(c.apply(0.3, -0.4), (0.3, -0.4));
        let big = Calibration { scale_x: 3.0, scale_y: 3.0, ..Default::default() };
        assert_eq!(big.apply(1.0, 1.0), (1.0, 1.0));
    }

    #[test]
    fn settings_json_round_trips_and_fills_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"content":{"kind":"text","text":"HI"}}"#).unwrap();
        assert_eq!(s.content, Content::Text { text: "HI".into() });
        assert_eq!(s.brightness, Settings::default().brightness);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn joined_looks_travel_blanked_between_them() {
        let a = vec![Point::lit(-0.5, 0.0, 1.0, 0.0, 0.0), Point::lit(-0.4, 0.0, 1.0, 0.0, 0.0)];
        let b = vec![Point::lit(0.5, 0.0, 0.0, 1.0, 0.0), Point::lit(0.6, 0.0, 0.0, 1.0, 0.0)];
        let joined = join_looks(vec![a.clone(), Vec::new(), b.clone()]);
        assert_eq!(&joined[..2], &a[..]);
        assert_eq!(&joined[joined.len() - 2..], &b[..]);
        let travel = &joined[2..joined.len() - 2];
        assert!(travel.len() > 10 && travel.iter().all(|p| !p.is_lit()));
        assert!(max_step(&joined[1..joined.len() - 1]) <= MAX_STEP + 1e-4);
        assert_eq!(join_looks(vec![a.clone()]), a);
        assert!(join_looks(Vec::new()).is_empty());
    }

    fn synced(generator: &str, params: GenParams) -> Settings {
        let params = GenParams { beat_sync: true, ..params };
        Settings { content: Content::Generator { generator: generator.into(), params }, brightness: 1.0, ..Default::default() }
    }

    /// The frame a cue launched at `start` shows at tempo-clock `beat`.
    fn frame_at(s: &Settings, start: f64, beat: f64, bpm: f64) -> Vec<Point> {
        let clock = BeatClock { beat, bpm, beats_per_bar: 4 };
        Animator::starting_at(start).render(s, AudioFeatures::default(), 1.0 / 60.0, &clock)
    }

    fn same(a: &[Point], b: &[Point]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.x - q.x).abs() < 1e-4 && (p.y - q.y).abs() < 1e-4 && p.r == q.r)
    }

    #[test]
    fn beat_synced_motion_repeats_every_period_at_any_bpm() {
        // Generators whose motion repeats every 2π of t.
        for generator in ["beam_circle", "beam_fan", "lissajous", "spiral_arms"] {
            let s = synced(generator, GenParams { period_beats: 4.0, ..Default::default() });
            for bpm in [128.0, 150.0] {
                let a = frame_at(&s, 0.0, 1.3, bpm);
                assert!(same(&a, &frame_at(&s, 0.0, 5.3, bpm)), "{generator} at {bpm} BPM: not back after 4 beats");
                assert!(same(&a, &frame_at(&s, 0.0, 41.3, bpm)), "{generator}: not back after 40 beats");
                assert!(!same(&a, &frame_at(&s, 0.0, 3.3, bpm)), "{generator}: should have moved after 2 beats");
            }
            // Beats, not seconds: the same beat looks the same at any tempo.
            assert!(same(&frame_at(&s, 0.0, 2.7, 90.0), &frame_at(&s, 0.0, 2.7, 174.0)));
        }
    }

    #[test]
    fn beat_position_counts_from_the_bar_the_cue_started_in() {
        let s = synced("beam_circle", GenParams { period_beats: 4.0, ..Default::default() });
        // Launched mid-bar (beat 5.6 or 6.9): the motion is where a cue
        // launched on that bar's "one" (beat 4) would be.
        let on_the_one = frame_at(&s, 4.0, 7.2, 120.0);
        assert!(same(&frame_at(&s, 5.6, 7.2, 120.0), &on_the_one));
        assert!(same(&frame_at(&s, 6.9, 7.2, 120.0), &on_the_one));
        // At its first frame a cue launched on a "one" starts its cycle.
        assert!(same(&frame_at(&s, 12.0, 12.0, 120.0), &frame_at(&s, 0.0, 0.0, 120.0)));
        // Without a launch beat, the first rendered frame is the launch.
        let mut a = Animator::default();
        let clock = |beat| BeatClock { beat, ..Default::default() };
        a.render(&s, AudioFeatures::default(), 0.016, &clock(9.0));
        let later = a.render(&s, AudioFeatures::default(), 0.016, &clock(10.0));
        assert!(same(&later, &frame_at(&s, 8.0, 10.0, 120.0)));
    }

    #[test]
    fn reversed_direction_runs_the_cycle_backwards() {
        let fwd = synced("beam_circle", GenParams::default());
        let rev = synced("beam_circle", GenParams { direction: -1, ..Default::default() });
        assert!(same(&frame_at(&rev, 0.0, 1.0, 128.0), &frame_at(&fwd, 0.0, 3.0, 128.0)));
    }

    #[test]
    fn gate_lights_the_look_only_just_after_each_beat() {
        let s = synced("beam_fan", GenParams { gate_beats: 0.25, ..Default::default() });
        let lit = |beat| frame_at(&s, 0.0, beat, 128.0).iter().any(|p| p.is_lit());
        assert!(lit(3.0) && lit(3.2));
        assert!(!lit(3.3) && !lit(3.9));
        // Without beat_sync the gate does nothing.
        let free = Settings { content: Content::Generator { generator: "beam_fan".into(), params: GenParams { gate_beats: 0.25, ..Default::default() } }, ..Default::default() };
        assert!(frame_at(&free, 0.0, 3.9, 128.0).iter().any(|p| p.is_lit()));
    }

    #[test]
    fn free_running_looks_ignore_the_tempo_clock() {
        let s = Settings { content: Content::Generator { generator: "beam_circle".into(), params: GenParams::default() }, ..Default::default() };
        assert_eq!(frame_at(&s, 0.0, 0.0, 120.0), frame_at(&s, 3.0, 17.4, 150.0));
    }

    fn evolving(cue: crate::evolving::EvolvingCue) -> Settings {
        Settings { content: Content::Evolving(cue), brightness: 1.0, ..Default::default() }
    }

    /// A plain beat-synced look, `beat_pos` beats after its bar's one.
    fn plain_at(generator: &str, scale: f32, color: [u8; 3], beat_pos: f64) -> Vec<Point> {
        let s = Settings { scale, color, ..synced(generator, GenParams::default()) };
        frame_at(&s, 0.0, beat_pos, 120.0)
    }

    #[test]
    fn an_evolving_cue_draws_the_look_of_its_keys() {
        use crate::evolving::{EvolvingCue, EvolvingKey};
        // Two fan keys: size 0.2 → 1.0 over 16 beats.
        let fan = |at_beats, scale| EvolvingKey { at_beats, scale, ..Default::default() };
        let s = evolving(EvolvingCue { keys: vec![fan(0.0, 0.2), fan(16.0, 1.0)], ..Default::default() });
        // Launched on beat 4: at beat 12 it is half-way, size 0.6.
        assert!(same(&frame_at(&s, 4.0, 12.0, 120.0), &plain_at("fan", 0.6, [0, 255, 0], 8.0)));
        assert!(same(&frame_at(&s, 4.0, 4.0, 120.0), &plain_at("fan", 0.2, [0, 255, 0], 0.0)));
        // Beats, not seconds.
        assert!(same(&frame_at(&s, 4.0, 9.5, 120.0), &frame_at(&s, 4.0, 9.5, 150.0)));
        // A key's brightness is on top of the look's; the key's gate applies.
        let dim = Settings { brightness: 0.5, ..s.clone() };
        let lit = frame_at(&dim, 0.0, 2.0, 120.0);
        assert!(lit.iter().any(|p| p.is_lit()) && lit.iter().all(|p| p.g <= 0.5 + 1e-6));
        let gated = evolving(EvolvingCue { keys: vec![EvolvingKey { gate_beats: 0.25, ..Default::default() }], ..Default::default() });
        let lit = |beat| frame_at(&gated, 0.0, beat, 128.0).iter().any(|p| p.is_lit());
        assert!(lit(3.1) && !lit(3.5));
    }

    #[test]
    fn an_evolving_cue_switches_generator_on_the_key_beat() {
        let s = evolving(crate::evolving::test_cue(false));
        let frame = 1.0 / 60.0;
        let before = 8.0 - frame;
        assert!(same(&frame_at(&s, 0.0, before, 120.0), &plain_at("fan", 0.2 + 0.4 * before as f32 / 8.0, [0, 255, 0], before)));
        assert!(same(&frame_at(&s, 0.0, 8.0, 120.0), &plain_at("fan_wave", 0.6, [0, 255, 0], 8.0)));
    }

    #[test]
    fn an_evolving_cue_starts_on_the_next_beat_and_loops() {
        let s = evolving(crate::evolving::test_cue(true));
        let at = |start: f64, beat: f64| {
            let mut a = Animator::starting_at(start);
            let pts = a.render(&s, AudioFeatures::default(), 1.0 / 60.0, &BeatClock { beat, ..Default::default() });
            (pts, a.progress().cloned().unwrap())
        };
        // Pressed at 5.3: holds its first frame until beat 6.
        let (waiting, p) = at(5.3, 5.8);
        assert!(p.waiting && p.pos == 0.0);
        assert!(same(&waiting, &at(6.0, 6.0).0));
        let (_, p) = at(5.3, 7.5);
        assert!(!p.waiting && (p.pos - 1.5).abs() < 1e-9 && p.key == 0);
        // Loop: after 16 beats it is back on key 0, exactly as it started.
        let (looped, p) = at(6.0, 22.0);
        assert!(same(&looped, &at(6.0, 6.0).0));
        assert_eq!((p.pos, p.key, p.pass, p.ended), (0.0, 0, 1, false));
        // A one-shot cue stays on its last key.
        let once = evolving(crate::evolving::test_cue(false));
        let mut a = Animator::starting_at(0.0);
        a.render(&once, AudioFeatures::default(), 0.016, &BeatClock { beat: 30.0, ..Default::default() });
        let p = a.progress().unwrap();
        assert!(p.ended && p.key == 1);
    }

    #[test]
    fn a_look_switched_to_an_evolving_cue_starts_it_then() {
        let mut a = Animator::default();
        let clock = |beat| BeatClock { beat, ..Default::default() };
        a.render(&Settings::default(), AudioFeatures::default(), 0.016, &clock(1.0));
        assert!(a.progress().is_none());
        let s = evolving(crate::evolving::test_cue(false));
        a.render(&s, AudioFeatures::default(), 0.016, &clock(10.2));
        assert!(a.progress().unwrap().waiting);
        a.render(&s, AudioFeatures::default(), 0.016, &clock(13.0));
        assert!((a.progress().unwrap().pos - 2.0).abs() < 1e-9);
        // Editing the look around it (brightness) doesn't restart it...
        let brighter = Settings { brightness: 0.8, ..s.clone() };
        a.render(&brighter, AudioFeatures::default(), 0.016, &clock(14.0));
        assert!((a.progress().unwrap().pos - 3.0).abs() < 1e-9);
        // ...editing its keys does.
        let mut edited = crate::evolving::test_cue(false);
        edited.keys[0].scale = 0.3;
        a.render(&evolving(edited), AudioFeatures::default(), 0.016, &clock(14.0));
        assert_eq!(a.progress().unwrap().pos, 0.0);
    }

    #[test]
    fn evolving_strobe_flashes_on_the_beat_grid() {
        let mut cue = crate::evolving::test_cue(false);
        cue.keys.iter_mut().for_each(|k| k.strobe_div = 2.0);
        let s = evolving(cue);
        let lit = |beat| frame_at(&s, 0.0, beat, 128.0).iter().any(|p| p.is_lit());
        assert!(lit(2.1) && !lit(2.3) && lit(2.6) && !lit(2.8));
    }

    #[test]
    fn evolving_json_loads_as_content() {
        let s: Settings = serde_json::from_str(
            r#"{"content":{"kind":"evolving","length_beats":8,"loop":true,"keys":[{"at_beats":0,"generator":"fan","scale":0.4}]}}"#,
        )
        .unwrap();
        let Content::Evolving(cue) = &s.content else { panic!("{:?}", s.content) };
        assert!(cue.looped && cue.keys[0].scale == 0.4);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(s.content.same_drawing(&Content::Evolving(Default::default())));
    }

    fn a_frame(s: &Settings, audio: AudioFeatures) -> Vec<Point> {
        Animator::default().render(s, audio, 1.0 / 60.0, &BeatClock::default())
    }
}
