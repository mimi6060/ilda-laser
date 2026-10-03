//! Turns the current look (`Settings`) plus live audio features into one
//! frame of laser points, 60 times a second. Everything time-based
//! (rotation, wave travel, beat color steps, beat flashes) lives in
//! `Animator`, so a `Settings` value stays a plain, saveable description
//! of a look - the same thing a scene stores.

use crate::beat;
use crate::evolving::{self, EvolvingCue};
use crate::figures::Figure;
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
    /// One of the operator's figures (T-296, `figures.rs`): strokes drawn
    /// in the CRÉATION editor, possibly animated.
    Figure(Figure),
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
            (Content::Figure(a), Content::Figure(b)) => a.name == b.name,
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
    /// « Rythme » (T-103), for any look: a beat gate (stabs on the kick)...
    pub gate: GateMode,
    /// ...lit for this many beats after each beat (or off-beat)...
    pub gate_beats: f32,
    /// ...or, instead of the hard cut, an instant flash fading with
    /// `STAB_DECAY_BEATS`.
    pub gate_decay: bool,
    /// Beam strobe: flashes per beat (2 = every 1/2 beat, 4 = 1/4, 8 =
    /// 1/8), 0 = off. The output's strobe limiter (`safety.rs`) still cuts
    /// fast strobes after a burst.
    pub strobe_div: f32,
    /// Fraction of each strobe slot that is lit (`STROBE_DUTY` range).
    pub strobe_duty: f32,
}

/// When a look's beat gate opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    #[default]
    Off,
    /// On every beat.
    Beat,
    /// On every beat and every off-beat (half-way).
    BeatAndOffbeat,
}

/// Time constant of the decaying stab, in beats (research §4.4: 0.1-0.15).
pub const STAB_DECAY_BEATS: f32 = 0.12;
/// Strobe duty cycle range (research §4.4: 30-50 % for a strobe).
pub const STROBE_DUTY: std::ops::RangeInclusive<f32> = 0.3..=0.5;
/// Fastest look strobe, flashes per beat.
pub const MAX_STROBE_DIV: f32 = 8.0;

impl Settings {
    /// The « Rythme » gain at `beat_pos` (beats from the look's bar): the
    /// beat gate times the strobe, 1 when both are off.
    pub fn rhythm_gain(&self, beat_pos: f64) -> f32 {
        let slot = match self.gate {
            GateMode::Off => None,
            GateMode::Beat => Some(1.0),
            GateMode::BeatAndOffbeat => Some(0.5),
        };
        let gate = slot.map_or(1.0, |slot| {
            // A hair of tolerance so the gate opens exactly on its beat.
            let x = ((beat_pos + 1e-9).rem_euclid(slot)) as f32;
            if self.gate_decay {
                beat::env_stab(x, 0.0, STAB_DECAY_BEATS)
            } else {
                let len = if self.gate_beats.is_finite() { self.gate_beats.clamp(0.02, slot as f32) } else { 0.2 };
                if x < len { 1.0 } else { 0.0 }
            }
        });
        let div = if self.strobe_div.is_finite() { self.strobe_div.clamp(0.0, MAX_STROBE_DIV) } else { 0.0 };
        let duty = if self.strobe_duty.is_finite() { self.strobe_duty.clamp(*STROBE_DUTY.start(), *STROBE_DUTY.end()) } else { 0.4 };
        gate * beat::strobe_gate(beat_pos, div, duty)
    }
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
            gate: GateMode::Off,
            gate_beats: 0.2,
            gate_decay: false,
            strobe_div: 0.0,
            strobe_duty: 0.4,
        }
    }
}

pub use crate::audio::spectrum::Bands;

/// Musical section (T-236 detects them; until then the native source only
/// tells `Silence` from `Normal`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Silence,
    #[default]
    Normal,
    Break,
    Buildup,
    Drop,
}

/// Everything the audio analysis tells the engine, one snapshot per frame
/// (T-237). Filled by the native analysis (`audio/`) or by the browser
/// (`POST /api/audio`, which may send only the first three fields).
///
/// - `level`, `bass`, `beat` are the legacy fields every look uses, always
///   filled: `level` 0..1, `bass` 0..1 (native: the normalised low end,
///   `max(bands.sub, bands.bass)`), `beat` counts beats (native: kicks),
///   so a change means "a new beat happened since the last frame".
/// - Counters (`onset`, `kick`, `snare`, `hat`, `drop`) only ever grow:
///   compare with the last frame's value to see an event.
/// - Continuous values are 0..1 unless their name says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFeatures {
    pub level: f32,
    pub bass: f32,
    pub beat: u64,
    /// Full-band RMS, dBFS (−120 = silence).
    pub level_db: f32,
    /// Five bands, auto-gained (T-231).
    pub bands: Bands,
    pub onset: u64,
    pub kick: u64,
    pub snare: u64,
    pub hat: u64,
    /// Strength of the last kick / snare / hat (T-232).
    pub kick_strength: f32,
    pub snare_strength: f32,
    pub hat_strength: f32,
    pub centroid_hz: f32,
    pub silent: bool,
    /// Detected tempo (T-233; 0 = none) and how sure the detector is. A
    /// proposal: the beat clock is `TempoClock`'s.
    pub bpm: f32,
    pub bpm_confidence: f32,
    pub section: Section,
    /// Build-up 0..1 and drop counter (T-236).
    pub buildup: f32,
    pub drop: u64,
    /// Audio time of the analysis (seconds, studio clock).
    pub t: f64,
}

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            level: 0.0,
            bass: 0.0,
            beat: 0,
            level_db: crate::audio::analysis::FLOOR_DB,
            bands: Bands::default(),
            onset: 0,
            kick: 0,
            snare: 0,
            hat: 0,
            kick_strength: 0.0,
            snare_strength: 0.0,
            hat_strength: 0.0,
            centroid_hz: 0.0,
            silent: false,
            bpm: 0.0,
            bpm_confidence: 0.0,
            section: Section::Normal,
            buildup: 0.0,
            drop: 0,
            t: 0.0,
        }
    }
}

/// Continuous signals a route or modulator can read (`AudioFeatures::value`).
pub const AUDIO_VALUES: [&str; 13] =
    ["level", "bass", "sub", "bass_band", "low_mid", "mid", "high", "kick_strength", "snare_strength", "hat_strength", "buildup", "bpm_confidence", "centroid"];
/// Event counters (`AudioFeatures::counter`).
pub const AUDIO_EVENTS: [&str; 6] = ["beat", "onset", "kick", "snare", "hat", "drop"];

fn unit(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}

impl AudioFeatures {
    /// Silence that keeps the event counters and the audio time (so going
    /// silent is never an event), for stale or absent input.
    pub fn neutral(&self) -> Self {
        Self {
            beat: self.beat,
            onset: self.onset,
            kick: self.kick,
            snare: self.snare,
            hat: self.hat,
            drop: self.drop,
            bpm: self.bpm,
            t: self.t,
            silent: true,
            section: Section::Silence,
            ..Default::default()
        }
    }

    /// Every value in its range, nothing NaN (the browser may send anything).
    pub fn sanitized(mut self) -> Self {
        self.level = unit(self.level);
        self.bass = unit(self.bass);
        self.bands = Bands::from_array(self.bands.to_array().map(unit));
        self.kick_strength = unit(self.kick_strength);
        self.snare_strength = unit(self.snare_strength);
        self.hat_strength = unit(self.hat_strength);
        self.buildup = unit(self.buildup);
        self.bpm_confidence = unit(self.bpm_confidence);
        let db = crate::audio::analysis::FLOOR_DB;
        self.level_db = if self.level_db.is_finite() { self.level_db.clamp(db, 12.0) } else { db };
        self.centroid_hz = if self.centroid_hz.is_finite() { self.centroid_hz.clamp(0.0, 24_000.0) } else { 0.0 };
        self.bpm = if self.bpm.is_finite() { self.bpm.clamp(0.0, 400.0) } else { 0.0 };
        if !self.t.is_finite() {
            self.t = 0.0;
        }
        self
    }

    /// A continuous signal by id (`AUDIO_VALUES`), 0..1 (`centroid`:
    /// 0..12 kHz mapped to 0..1).
    pub fn value(&self, id: &str) -> Option<f32> {
        Some(match id {
            "level" => self.level,
            "bass" => self.bass,
            "sub" => self.bands.sub,
            "bass_band" => self.bands.bass,
            "low_mid" => self.bands.low_mid,
            "mid" => self.bands.mid,
            "high" => self.bands.high,
            "kick_strength" => self.kick_strength,
            "snare_strength" => self.snare_strength,
            "hat_strength" => self.hat_strength,
            "buildup" => self.buildup,
            "bpm_confidence" => self.bpm_confidence,
            "centroid" => (self.centroid_hz / 12_000.0).clamp(0.0, 1.0),
            _ => return None,
        })
    }

    /// An event counter by id (`AUDIO_EVENTS`).
    pub fn counter(&self, id: &str) -> Option<u64> {
        Some(match id {
            "beat" => self.beat,
            "onset" => self.onset,
            "kick" => self.kick,
            "snare" => self.snare,
            "hat" => self.hat,
            "drop" => self.drop,
            _ => return None,
        })
    }
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
    /// Seconds a per-second figure has been playing.
    fig_time: f64,
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
            // The look's « Rythme » applies on top of the keys.
            ..s.clone()
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
        if let Content::Figure(_) = &s.content {
            self.fig_time += dt as f64;
        }

        // size = 0 leaves the scale alone; size = 1 swings it 0.5x..1.5x.
        let scale = s.scale * (1.0 - 0.5 * react.size * (react.enabled as u8 as f32) + react.size * bass);
        let hue_shift = if react.enabled { self.hue_shift } else { 0.0 };
        let (r, g, b) = shift_hue(s.color, hue_shift);
        let flash_gain = 1.0 - react.flash * (react.enabled as u8 as f32) * (1.0 - self.flash);
        let mut gain = s.brightness.clamp(0.0, 1.0) * flash_gain * s.rhythm_gain(beat_pos);
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
            Content::Figure(fig) => {
                let pos = match fig.per {
                    crate::figures::RateUnit::Beat => beat_pos,
                    crate::figures::RateUnit::Second => self.fig_time,
                };
                fig.frame_points(fig.frame_index(pos), scale, |c| {
                    let (r, g, b) = shift_hue(c, hue_shift);
                    (r * gain, g * gain, b * gain)
                })
            }
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
        let loud = AudioFeatures { level: 1.0, bass: 1.0, beat: 7, ..Default::default() };
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

    /// Any look (a plain circle here) with a « Rythme » setting.
    fn rhythm(edit: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Settings { brightness: 1.0, ..Default::default() };
        edit(&mut s);
        s
    }

    fn lit_at(s: &Settings, beat: f64, bpm: f64) -> bool {
        frame_at(s, 0.0, beat, bpm).iter().any(|p| p.is_lit())
    }

    #[test]
    fn look_gate_of_0_2_beat_at_128_bpm_lights_about_94_ms_after_each_beat() {
        let s = rhythm(|s| s.gate = GateMode::Beat);
        assert_eq!(s.gate_beats, 0.2, "default gate length");
        // A plain shape: the gate is the look's own, no beat_sync needed.
        assert!(matches!(s.content, Content::Shape { .. }));
        let bpm = 128.0;
        let beat_at_ms = |ms: f64| ms / 1000.0 * bpm / 60.0;
        for k in [0.0, 1.0, 7.0, 30.0] {
            let start = k * 60_000.0 / bpm;
            assert!(lit_at(&s, beat_at_ms(start), bpm), "on the beat {k}");
            assert!(lit_at(&s, beat_at_ms(start + 90.0), bpm), "90 ms after beat {k}");
            assert!(!lit_at(&s, beat_at_ms(start + 98.0), bpm), "98 ms after beat {k}");
            assert!(!lit_at(&s, beat_at_ms(start + 400.0), bpm), "dark until the next beat");
        }
        // Off: always lit.
        assert!(lit_at(&rhythm(|_| ()), 3.5, bpm));
    }

    #[test]
    fn look_gate_on_beat_and_offbeat_and_decay() {
        let both = rhythm(|s| s.gate = GateMode::BeatAndOffbeat);
        assert!(lit_at(&both, 2.1, 128.0) && !lit_at(&both, 2.3, 128.0));
        assert!(lit_at(&both, 2.6, 128.0) && !lit_at(&both, 2.8, 128.0), "the off-beat flashes too");
        // Decay: instant on the beat, e^-1 one time constant later, faint after.
        let decay = rhythm(|s| {
            s.gate = GateMode::Beat;
            s.gate_decay = true;
        });
        assert!((decay.rhythm_gain(5.0) - 1.0).abs() < 1e-6);
        assert!((decay.rhythm_gain(5.0 + STAB_DECAY_BEATS as f64) - (-1.0f32).exp()).abs() < 1e-3);
        assert!(decay.rhythm_gain(5.9) < 0.001);
        let peak = |beat| frame_at(&decay, 0.0, beat, 128.0).iter().map(|p| p.g).fold(0.0, f32::max);
        assert!(peak(4.0) > peak(4.06) && peak(4.06) > peak(4.2) && peak(4.2) > 0.0);
        // A broken length falls back to the default instead of going dark.
        let bad = rhythm(|s| {
            s.gate = GateMode::Beat;
            s.gate_beats = f32::NAN;
        });
        assert!(lit_at(&bad, 1.1, 120.0) && !lit_at(&bad, 1.5, 120.0));
    }

    #[test]
    fn look_strobe_flashes_on_the_beat_grid_with_its_duty() {
        let s = rhythm(|s| s.strobe_div = 4.0);
        // 1/4 beat at 40 %: lit for the first 0.1 beat of each quarter.
        assert!(lit_at(&s, 3.0, 128.0) && lit_at(&s, 3.09, 128.0) && !lit_at(&s, 3.12, 128.0) && lit_at(&s, 3.25, 128.0));
        // Duty is clamped to 30-50 %, the rate to 8 per beat.
        let duty = |d: f32| rhythm(|s| {
            s.strobe_div = 1.0;
            s.strobe_duty = d;
        });
        assert_eq!(duty(0.9).rhythm_gain(0.6), 0.0);
        assert_eq!(duty(0.0).rhythm_gain(0.25), 1.0);
        assert_eq!(rhythm(|s| s.strobe_div = 1e9).rhythm_gain(0.06), 0.0, "at most 8 flashes per beat");
        // Gate and strobe multiply.
        let both = rhythm(|s| {
            s.gate = GateMode::Beat;
            s.strobe_div = 8.0;
        });
        assert_eq!((both.rhythm_gain(1.0), both.rhythm_gain(1.1), both.rhythm_gain(1.13)), (1.0, 0.0, 1.0));
        assert_eq!(both.rhythm_gain(1.5), 0.0);
    }

    #[test]
    fn look_rhythm_applies_to_evolving_cues_too() {
        let s = Settings { gate: GateMode::Beat, ..evolving(crate::evolving::test_cue(true)) };
        assert!(lit_at(&s, 4.1, 128.0) && !lit_at(&s, 4.5, 128.0));
    }

    #[test]
    fn old_looks_load_without_rhythm() {
        let s: Settings = serde_json::from_str(r#"{"content":{"kind":"wave"},"brightness":0.7}"#).unwrap();
        assert_eq!((s.gate, s.gate_beats, s.gate_decay, s.strobe_div, s.strobe_duty), (GateMode::Off, 0.2, false, 0.0, 0.4));
        assert_eq!(s.rhythm_gain(0.73), 1.0);
        let json = serde_json::to_string(&Settings { gate: GateMode::BeatAndOffbeat, ..s }).unwrap();
        assert!(json.contains(r#""gate":"beat_and_offbeat""#), "{json}");
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

    /// A look saved before T-237 (audio reaction on), rendered through a
    /// beat, a swell and a silence: the new fields change nothing, and the
    /// old `POST /api/audio` body gives the very same frames.
    #[test]
    fn a_look_saved_before_v2_renders_the_same_with_the_v2_snapshot() {
        let saved = r#"{"content":{"kind":"shape","shape":"star"},"color":[255,0,0],"scale":0.6,"rotation_speed":45.0,"brightness":0.8,
            "audio":{"enabled":true,"size":0.8,"rotate":0.5,"color_on_beat":true,"flash":0.6}}"#;
        let s: Settings = serde_json::from_str(saved).unwrap();
        let legacy = |i: u64| {
            let x = (i as f32 / 40.0).min(1.0);
            AudioFeatures { level: x * 0.7, bass: x, beat: i / 15, ..Default::default() }
        };
        let (mut old, mut new, mut posted) = (Animator::default(), Animator::default(), Animator::default());
        for i in 0..90u64 {
            let l = legacy(i);
            let v2 = AudioFeatures {
                level_db: -12.0,
                bands: Bands { sub: 0.9, bass: 0.1, low_mid: 0.5, mid: 0.7, high: 1.0 },
                onset: i,
                kick: i / 3,
                snare: i / 7,
                hat: i,
                kick_strength: 1.0,
                centroid_hz: 3000.0,
                bpm: 128.0,
                bpm_confidence: 0.9,
                section: Section::Buildup,
                buildup: 0.7,
                drop: i / 30,
                t: i as f64,
                ..l
            };
            let body = serde_json::json!({ "level": l.level, "bass": l.bass, "beat": l.beat });
            let from_page = crate::audio::browser_features(&body, 0.0).unwrap();
            let clock = BeatClock { beat: i as f64 * 0.03, ..Default::default() };
            let a = old.render(&s, l, 1.0 / 60.0, &clock);
            assert_eq!(new.render(&s, v2, 1.0 / 60.0, &clock), a, "frame {i}");
            assert_eq!(posted.render(&s, from_page, 1.0 / 60.0, &clock), a, "frame {i}");
        }
    }

    #[test]
    fn audio_features_v2_load_the_old_format_and_round_trip() {
        let f: AudioFeatures = serde_json::from_str(r#"{"level":0.5,"bass":0.25,"beat":3}"#).unwrap();
        assert_eq!((f.level, f.bass, f.beat), (0.5, 0.25, 3));
        assert_eq!(f, AudioFeatures { level: 0.5, bass: 0.25, beat: 3, ..Default::default() });
        let full = AudioFeatures { kick: 4, bands: Bands { high: 0.5, ..Default::default() }, section: Section::Drop, drop: 2, t: 1.5, ..f };
        let text = serde_json::to_string(&full).unwrap();
        assert!(text.contains(r#""section":"drop""#), "{text}");
        assert_eq!(serde_json::from_str::<AudioFeatures>(&text).unwrap(), full);
        // Every signal id resolves; a stale snapshot keeps the counters.
        assert!(AUDIO_VALUES.iter().all(|id| full.value(id).is_some()) && full.value("nope").is_none());
        assert!(AUDIO_EVENTS.iter().all(|id| full.counter(id).is_some()) && full.counter("nope").is_none());
        let n = full.neutral();
        assert_eq!((n.level, n.bands.high, n.kick, n.drop, n.beat, n.silent), (0.0, 0.0, 4, 2, 3, true));
        let bad = AudioFeatures { level: f32::NAN, bands: Bands { mid: -2.0, ..Default::default() }, bpm: f32::INFINITY, t: f64::NAN, ..Default::default() }.sanitized();
        assert_eq!((bad.level, bad.bands.mid, bad.bpm, bad.t), (0.0, 0.0, 0.0, 0.0));
    }

    fn a_frame(s: &Settings, audio: AudioFeatures) -> Vec<Point> {
        Animator::default().render(s, audio, 1.0 / 60.0, &BeatClock::default())
    }
}
