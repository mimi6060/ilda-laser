//! Turns the current look (`Settings`) plus live audio features into one
//! frame of laser points, 60 times a second. Everything time-based
//! (rotation, wave travel, beat color steps, beat flashes) lives in
//! `Animator`, so a `Settings` value stays a plain, saveable description
//! of a look - the same thing a scene stores.

use crate::font;
use crate::patterns::{self, Point};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    Shape { shape: String },
    Text { text: String },
    Wave,
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

#[derive(Default)]
pub struct Animator {
    angle_deg: f32,
    hue_shift: f32,
    wave_phase: f32,
    flash: f32,
    last_beat: u64,
}

impl Animator {
    pub fn render(&mut self, s: &Settings, audio: AudioFeatures, dt: f32) -> Vec<Point> {
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

        // size = 0 leaves the scale alone; size = 1 swings it 0.5x..1.5x.
        let scale = s.scale * (1.0 - 0.5 * react.size * (react.enabled as u8 as f32) + react.size * bass);
        let hue_shift = if react.enabled { self.hue_shift } else { 0.0 };
        let (r, g, b) = shift_hue(s.color, hue_shift);
        let flash_gain = 1.0 - react.flash * (react.enabled as u8 as f32) * (1.0 - self.flash);
        let gain = s.brightness.clamp(0.0, 1.0) * flash_gain;
        let (r, g, b) = (r * gain, g * gain, b * gain);

        let points = match &s.content {
            Content::Shape { shape } => patterns::by_name(shape, scale, r, g, b).unwrap_or_default(),
            Content::Text { text } => font::text_to_points(&text.to_uppercase(), scale, r, g, b),
            Content::Wave => patterns::wave(scale, 0.15 + 0.6 * level, self.wave_phase, r, g, b),
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
        let pts = a.render(&s, AudioFeatures::default(), 1.0 / 60.0);
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
        let before = a.render(&s, AudioFeatures::default(), 0.016)[0];
        let after = a.render(&s, AudioFeatures { beat: 1, ..Default::default() }, 0.016)[0];
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

    fn a_frame(s: &Settings, audio: AudioFeatures) -> Vec<Point> {
        Animator::default().render(s, audio, 1.0 / 60.0)
    }
}
