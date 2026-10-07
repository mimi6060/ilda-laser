//! Incoming timecode (T-168, T-409): the timeline chases a show
//! controller's clock, read as MIDI Time Code (mtc.rs, from the MIDI
//! worker), as LTC on an audio input channel (ltc.rs, decoded on the audio
//! analysis thread) or as Art-Net timecode (artnet.rs, received by the
//! Art-Net listener of netin/).
//!
//! What timecode may do, and what it never does:
//! - It drives the **timeline only**: seek, play, and the playhead's
//!   speed. It never arms the laser, never touches a cue, a master or a
//!   safety setting. The laser stays as the operator left it.
//! - It acts only when the operator armed **TC-IN** (memory only, off at
//!   every start), the external inputs gate is open and the *Timecode*
//!   source's scope includes *Timeline* (external.rs, T-408). TC-IN is
//!   that source's access. The Art-Net receiver is open only while the
//!   source is Art-Net and TC-IN is armed (receive only).
//! - An emergency stop (Échap) or the laser being disarmed drops TC-IN:
//!   after a blackout, timecode restarts nothing until TC-IN is armed
//!   again by hand.
//! - Only shows in seconds follow it; a time outside the show (after the
//!   offset) is ignored and stops a show it was driving.
//!
//! Source, rate and offset are the global settings (`timecode.json`)
//! unless the loaded show has its own (`Show::timecode`, T-409).
//!
//! Chase policy, every engine frame (`frame`):
//! - **Pré-roll**: a moving time is followed once it has run on
//!   consecutive frames for `pre_roll_frames` frames; the frames before
//!   are ignored. A frame that doesn't follow the one before (a jump, a
//!   gap) starts the pre-roll over.
//! - **Suivi** (past the pre-roll, a moving time less than 4 frames old):
//!   the show plays at `timecode - offset`. Started if needed; a playhead
//!   more than `JUMP_FRAMES` away is a jump of the source: seek.
//!   Otherwise, with *Lissage*, a tenth of the error is taken up per frame
//!   (jitter smoothed, the source's speed followed), without it the
//!   playhead is set on the time.
//! - **Post-roll** (no new time, up to `after_roll_s` seconds after the
//!   last one followed): the show runs on by itself. Beyond that the show
//!   it was driving stops (it keeps its position, nothing is drawn) and
//!   the status says *Timecode perdu*. `after_roll_s = None` (*Continuer*)
//!   is a post-roll without limit.
//! - A time that doesn't move (an MTC full frame when the source locates,
//!   frozen LTC, a paused Art-Net sender) cues a stopped show up at that
//!   position.
//!
//! The frame rate comes from the signal (MTC and Art-Net carry it; LTC
//! from its measured speed and drop-frame flag) unless one is forced.

pub mod artnet;
pub mod ltc;
pub mod mtc;
pub mod out;

use crate::external;
use crate::interlock::ArmSource;
use crate::midi::{MidiEvent, MidiMsg};
use crate::timeline::{Clock, Player, Show, TimeBase};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::path::PathBuf;

pub const DEFAULT_PRE_ROLL_FRAMES: u8 = 5;
pub const MAX_PRE_ROLL_FRAMES: u8 = 100;
pub const DEFAULT_AFTER_ROLL_S: f32 = 1.0;
pub const MAX_AFTER_ROLL_S: f32 = 60.0;
/// T-168's *roue libre* was counted in frames at 25 im/s.
const LEGACY_FREEWHEEL_FPS: f64 = 25.0;
/// No new moving time for longer than this: the signal dropped.
const DROPOUT_FRAMES: f64 = 4.0;
/// The playhead further than this from the timecode: a jump, seek.
pub const JUMP_FRAMES: f64 = 5.0;
/// *Lissage*: share of the error taken up each engine frame.
const PULL: f64 = 0.1;
/// A still time older than this no longer counts as « à l'arrêt ».
const STILL_FOR_S: f64 = 1.5;
/// Largest offset: a day.
pub const MAX_OFFSET_S: f64 = 86_400.0;
const MAX_LTC_CHANNEL: u16 = 64;

/// One frame rate of SMPTE / MIDI timecode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rate {
    #[serde(rename = "24")]
    F24,
    #[serde(rename = "25")]
    F25,
    #[serde(rename = "29.97df")]
    F2997Df,
    #[serde(rename = "30")]
    F30,
}

impl Rate {
    pub const ALL: [Rate; 4] = [Rate::F24, Rate::F25, Rate::F2997Df, Rate::F30];

    /// Frame labels per second (30 for 29.97 drop).
    pub fn nominal(self) -> u32 {
        match self {
            Rate::F24 => 24,
            Rate::F25 => 25,
            Rate::F2997Df | Rate::F30 => 30,
        }
    }

    /// Real frames per second.
    pub fn fps(self) -> f64 {
        match self {
            Rate::F2997Df => 30_000.0 / 1_001.0,
            r => r.nominal() as f64,
        }
    }

    pub fn drop_frame(self) -> bool {
        self == Rate::F2997Df
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            Rate::F24 => "24 im/s",
            Rate::F25 => "25 im/s",
            Rate::F2997Df => "29,97 im/s drop",
            Rate::F30 => "30 im/s",
        }
    }

    /// The two rate bits of MTC.
    pub fn from_mtc_code(code: u8) -> Rate {
        match code & 0x03 {
            0 => Rate::F24,
            1 => Rate::F25,
            2 => Rate::F2997Df,
            _ => Rate::F30,
        }
    }

    pub fn mtc_code(self) -> u8 {
        match self {
            Rate::F24 => 0,
            Rate::F25 => 1,
            Rate::F2997Df => 2,
            Rate::F30 => 3,
        }
    }

    /// LTC: the drop-frame flag, else the nearest of 24, 25 and 30 to the
    /// measured speed (29.97 without drop frame reads as 30).
    pub fn from_ltc(fps: f64, drop_frame: bool) -> Rate {
        if drop_frame {
            return Rate::F2997Df;
        }
        [Rate::F24, Rate::F25, Rate::F30].into_iter().min_by(|a, b| (a.fps() - fps).abs().total_cmp(&(b.fps() - fps).abs())).unwrap_or(Rate::F25)
    }
}

/// A timecode label, hh:mm:ss:ff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tc {
    pub h: u8,
    pub m: u8,
    pub s: u8,
    pub f: u8,
}

impl Tc {
    /// A label that exists at `rate` (drop frame skips frames 0 and 1 of
    /// every minute but each tenth).
    pub fn valid(&self, rate: Rate) -> bool {
        let dropped = rate.drop_frame() && self.s == 0 && self.f < 2 && !self.m.is_multiple_of(10);
        self.h < 24 && self.m < 60 && self.s < 60 && (self.f as u32) < rate.nominal() && !dropped
    }

    /// Frames since 00:00:00:00.
    pub fn frames(&self, rate: Rate) -> i64 {
        let n = rate.nominal() as i64;
        let (h, m, s, f) = (self.h as i64, self.m as i64, self.s as i64, self.f as i64);
        let labels = ((h * 60 + m) * 60 + s) * n + f;
        if rate.drop_frame() {
            let minutes = h * 60 + m;
            labels - 2 * (minutes - minutes / 10)
        } else {
            labels
        }
    }

    pub fn from_frames(frames: i64, rate: Rate) -> Tc {
        let n = rate.nominal() as i64;
        let day = 24 * 3600 * n - if rate.drop_frame() { 2 * (24 * 60 - 24 * 6) } else { 0 };
        let mut k = frames.rem_euclid(day);
        if rate.drop_frame() {
            // Put the skipped labels back: 2 per minute, none on the tenths.
            let (per_10, per_1) = (17_982, 1_798);
            let (d, r) = (k / per_10, k % per_10);
            k += 18 * d + if r > 1 { 2 * ((r - 2) / per_1) } else { 0 };
        }
        Tc { h: (k / (3600 * n)) as u8, m: (k / (60 * n) % 60) as u8, s: (k / n % 60) as u8, f: (k % n) as u8 }
    }

    pub fn seconds(&self, rate: Rate) -> f64 {
        self.frames(rate) as f64 / rate.fps()
    }

    /// The frame playing at `secs` (negative times count back from midnight).
    pub fn from_seconds(secs: f64, rate: Rate) -> Tc {
        let frames = if secs.is_finite() { (secs * rate.fps() + 1e-6).floor() as i64 } else { 0 };
        Tc::from_frames(frames, rate)
    }
}

impl fmt::Display for Tc {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        write!(out, "{:02}:{:02}:{:02}:{:02}", self.h, self.m, self.s, self.f)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcSource {
    #[default]
    Off,
    Mtc,
    Ltc,
    #[serde(rename = "artnet")]
    ArtNet,
}

impl TcSource {
    pub fn label_fr(self) -> &'static str {
        match self {
            TcSource::Off => "Aucune",
            TcSource::Mtc => "MTC",
            TcSource::Ltc => "LTC",
            TcSource::ArtNet => "Art-Net",
        }
    }
}

/// *Images/s*: detected from the signal, or forced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TcFps {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "24")]
    F24,
    #[serde(rename = "25")]
    F25,
    #[serde(rename = "29.97df")]
    F2997Df,
    #[serde(rename = "30")]
    F30,
}

impl TcFps {
    pub fn forced(self) -> Option<Rate> {
        match self {
            TcFps::Auto => None,
            TcFps::F24 => Some(Rate::F24),
            TcFps::F25 => Some(Rate::F25),
            TcFps::F2997Df => Some(Rate::F2997Df),
            TcFps::F30 => Some(Rate::F30),
        }
    }
}

/// `<data-dir>/timecode.json`. Never TC-IN itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TcConfig {
    pub source: TcSource,
    pub fps: TcFps,
    /// *Pré-roll*: consecutive frames needed before a time is followed.
    pub pre_roll_frames: u8,
    /// *Post-roll*: seconds the show runs on after the timecode drops;
    /// `None` = *Continuer* (no limit).
    pub after_roll_s: Option<f32>,
    /// *Lissage*: pull the playhead gently instead of setting it.
    pub smooth: bool,
    /// Show position = timecode - offset (s): a show starting at TC
    /// 01:00:00:00 has 3600.
    pub offset_s: f64,
    /// MTC from this port only (none: any port).
    pub midi_port: Option<String>,
    /// LTC on this input channel (1-based); 0: the mix of all channels.
    pub ltc_channel: u16,
    /// Art-Net: the address of this Mac to listen on, at the Art-Net port
    /// of RÉGLAGES › Entrées réseau; "" = none. With the Art-Net chart on,
    /// its socket carries the timecode as well.
    pub interface: String,
    /// The timecode output (out.rs, T-417): off by default. Set through
    /// `POST /api/timecode/out` only.
    pub out: out::TimecodeOut,
}

impl Default for TcConfig {
    fn default() -> Self {
        TcConfig {
            source: TcSource::Off,
            fps: TcFps::Auto,
            pre_roll_frames: DEFAULT_PRE_ROLL_FRAMES,
            after_roll_s: Some(DEFAULT_AFTER_ROLL_S),
            smooth: true,
            offset_s: 0.0,
            midi_port: None,
            ltc_channel: 0,
            interface: String::new(),
            out: out::TimecodeOut::default(),
        }
    }
}

fn valid_offset(offset_s: f64) -> Result<(), String> {
    if !offset_s.is_finite() || offset_s.abs() > MAX_OFFSET_S {
        return Err("décalage hors limites (± 24 h)".into());
    }
    Ok(())
}

impl TcConfig {
    pub fn validate(&self) -> Result<(), String> {
        valid_offset(self.offset_s)?;
        if self.pre_roll_frames > MAX_PRE_ROLL_FRAMES {
            return Err(format!("pré-roll : de 0 à {MAX_PRE_ROLL_FRAMES} images"));
        }
        if self.after_roll_s.is_some_and(|s| !(0.0..=MAX_AFTER_ROLL_S).contains(&s)) {
            return Err(format!("post-roll : de 0 à {MAX_AFTER_ROLL_S} s"));
        }
        if self.ltc_channel > MAX_LTC_CHANNEL {
            return Err(format!("voie LTC : de 0 à {MAX_LTC_CHANNEL}"));
        }
        if self.midi_port.as_ref().is_some_and(|p| p.is_empty() || p.len() > 128) {
            return Err("port MIDI invalide".into());
        }
        if !self.interface.is_empty() {
            crate::netin::parse_bind(&self.interface, 0)?;
        }
        Ok(())
    }

    /// A saved file of T-168: `keep_running` becomes a post-roll without
    /// limit, `freewheel_frames` the same time in seconds.
    fn migrate(v: &mut Value) {
        let Some(obj) = v.as_object_mut() else { return };
        let keep = obj.remove("keep_running").and_then(|k| k.as_bool());
        let frames = obj.remove("freewheel_frames").and_then(|f| f.as_f64());
        if obj.contains_key("after_roll_s") {
            return;
        }
        if keep == Some(true) {
            obj.insert("after_roll_s".into(), Value::Null);
        } else if let Some(n) = frames {
            obj.insert("after_roll_s".into(), json!(((n / LEGACY_FREEWHEEL_FPS) as f32).clamp(0.0, MAX_AFTER_ROLL_S)));
        }
    }
}

/// A show's own timecode (T-409), saved in the show. A show without one
/// follows the global settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowTimecode {
    pub source: TcSource,
    pub fps: TcFps,
    pub offset_s: f64,
}

impl ShowTimecode {
    pub fn validate(&self) -> Result<(), String> {
        valid_offset(self.offset_s)
    }
}

/// One time read off the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub tc: Tc,
    /// The rate the source says (MTC, Art-Net) or seems to run at (LTC).
    pub rate: Rate,
    /// Studio seconds (`Shared::now_s`) at which the source was at `tc`
    /// plus `lag` frames.
    pub t: f64,
    pub lag: f64,
    /// Quarter frames, LTC or Art-Net (false: an MTC full frame, a locate).
    pub running: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Off,
    NoSignal,
    Preroll,
    Locked,
    Afterroll,
    Stopped,
    Lost,
}

impl Status {
    pub fn label_fr(self) -> &'static str {
        match self {
            Status::Off => "Pas de source",
            Status::NoSignal => "Pas de timecode",
            Status::Preroll => "Pré-roll",
            Status::Locked => "Suivi",
            Status::Afterroll => "Post-roll",
            Status::Stopped => "Timecode à l'arrêt",
            Status::Lost => "Timecode perdu",
        }
    }

    /// The five states of the API and the big readout.
    pub fn state(self) -> &'static str {
        match self {
            Status::Off | Status::NoSignal => "off",
            Status::Preroll => "preroll",
            Status::Locked | Status::Stopped => "locked",
            Status::Afterroll => "afterroll",
            Status::Lost => "lost",
        }
    }
}

/// What a new input is: a change forgets the readings and drops TC-IN.
type InputKey = (TcSource, Option<String>, u16, String);

/// `Shared.timecode`.
pub struct TimecodeState {
    config: TcConfig,
    path: Option<PathBuf>,
    /// The loaded show's own settings (`sync`).
    show: Option<ShowTimecode>,
    input: InputKey,
    /// TC-IN: memory only, off at every start.
    pub armed: bool,
    estop_trips: u64,
    laser_armed: bool,
    mtc: mtc::MtcDecoder,
    last: Option<Reading>,
    /// The newest reading that moved on from the one before.
    moved: Option<Reading>,
    last_moved: bool,
    /// Frames run on consecutively (the pre-roll).
    consecutive: u32,
    /// Studio time of the last reading followed (past the pre-roll).
    locked_at: Option<f64>,
    /// Readings received, and the count the chase has looked at.
    received: u64,
    handled: u64,
    /// LTC frames already taken from the audio hub.
    ltc_gen: u64,
    /// The timeline plays because of the timecode.
    chasing: bool,
    jumps: u64,
    status: Status,
    /// Why the timecode doesn't drive the timeline right now.
    blocked: Option<String>,
    /// The timecode output's queue and status (T-417).
    pub out: out::OutRuntime,
}

impl TimecodeState {
    fn with(config: TcConfig, path: Option<PathBuf>) -> Self {
        let input = (config.source, config.midi_port.clone(), config.ltc_channel, config.interface.clone());
        TimecodeState {
            config,
            path,
            show: None,
            input,
            armed: false,
            estop_trips: 0,
            laser_armed: false,
            mtc: Default::default(),
            last: None,
            moved: None,
            last_moved: false,
            consecutive: 0,
            locked_at: None,
            received: 0,
            handled: 0,
            ltc_gen: 0,
            chasing: false,
            jumps: 0,
            status: Status::Off,
            blocked: None,
            out: out::OutRuntime::default(),
        }
    }

    pub fn load(path: PathBuf) -> Self {
        let mut raw: Value = crate::load_json(&path);
        TcConfig::migrate(&mut raw);
        let mut config: TcConfig = if raw.is_null() { TcConfig::default() } else { serde_json::from_value(raw).unwrap_or_default() };
        if let Err(e) = config.out.validate() {
            log::warn!("timecode.json (sortie) : {e} ; sortie désactivée");
            config.out = out::TimecodeOut::default();
        }
        if let Err(e) = config.validate() {
            log::warn!("timecode.json : {e} ; réglages par défaut");
            config = TcConfig { out: config.out, ..TcConfig::default() };
        }
        Self::with(config, Some(path))
    }

    pub fn in_memory() -> Self {
        Self::with(TcConfig::default(), None)
    }

    pub fn config(&self) -> &TcConfig {
        &self.config
    }

    /// The output's settings (checked by `out::update`).
    pub fn set_out(&mut self, cfg: out::TimecodeOut) {
        self.config.out = cfg;
    }

    /// The source in use: the loaded show's, else the global one.
    pub fn source(&self) -> TcSource {
        self.show.as_ref().map_or(self.config.source, |s| s.source)
    }

    fn fps(&self) -> TcFps {
        self.show.as_ref().map_or(self.config.fps, |s| s.fps)
    }

    pub fn offset_s(&self) -> f64 {
        self.show.as_ref().map_or(self.config.offset_s, |s| s.offset_s)
    }

    fn input_key(&self) -> InputKey {
        (self.source(), self.config.midi_port.clone(), self.config.ltc_channel, self.config.interface.clone())
    }

    /// What to write, out of the `Shared` lock.
    pub fn save_job(&self) -> Option<(PathBuf, TcConfig)> {
        self.path.clone().map(|p| (p, self.config.clone()))
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn is_chasing(&self) -> bool {
        self.chasing
    }

    fn forget(&mut self) {
        self.mtc.reset();
        self.last = None;
        self.moved = None;
        self.last_moved = false;
        self.consecutive = 0;
        self.locked_at = None;
        self.chasing = false;
    }

    fn rate_of(&self, r: &Reading) -> Rate {
        self.fps().forced().unwrap_or(r.rate)
    }

    fn secs_of(&self, r: &Reading) -> f64 {
        let rate = self.rate_of(r);
        r.tc.seconds(rate) + r.lag / rate.fps()
    }

    fn prerolled(&self) -> bool {
        self.consecutive >= self.config.pre_roll_frames as u32
    }

    /// A time from the source.
    pub fn push(&mut self, r: Reading) {
        self.received += 1;
        let rate = self.rate_of(&r);
        let window = DROPOUT_FRAMES / rate.fps();
        let prev = self.last.filter(|p| p.running && r.running);
        // The same frame again (a sender faster than its frames): not news
        // while the time moves; a time that stays is a stop.
        if prev.is_some_and(|p| p.tc == r.tc) && self.moved.is_some_and(|m| r.t - m.t <= window) {
            return;
        }
        let moved = prev.is_some_and(|p| p.tc != r.tc);
        self.consecutive = match prev {
            Some(p) if p.tc == r.tc => self.consecutive,
            Some(p) => {
                let step = r.tc.frames(rate) - p.tc.frames(rate);
                if (1..=2).contains(&step) && r.t - p.t <= window {
                    self.consecutive.saturating_add(step as u32)
                } else {
                    1
                }
            }
            None if r.running => 1,
            None => 0,
        };
        if moved {
            self.moved = Some(r);
            if self.prerolled() {
                self.locked_at = Some(r.t);
            }
        }
        self.last_moved = moved;
        self.last = Some(r);
    }

    /// The timecode now (seconds) and its rate: run on from the last time
    /// while it moves.
    pub fn now(&self, t: f64) -> Option<(f64, Rate)> {
        let last = self.last?;
        let mut secs = self.secs_of(&last);
        if matches!(self.status, Status::Locked | Status::Preroll) {
            secs += (t - last.t).max(0.0);
        }
        Some((secs, self.rate_of(&last)))
    }

    /// The post-roll is over (never with *Continuer*).
    fn after_roll_over(&self, t: f64) -> bool {
        match (self.locked_at, self.config.after_roll_s) {
            (Some(at), Some(limit)) => t - at > limit as f64,
            _ => false,
        }
    }

    fn update_status(&mut self, t: f64) {
        let Some(last) = self.last else {
            self.status = if self.source() == TcSource::Off { Status::Off } else { Status::NoSignal };
            return;
        };
        let frame_s = 1.0 / self.rate_of(&last).fps();
        let fresh = self.last_moved && self.moved.is_some_and(|m| t - m.t <= DROPOUT_FRAMES * frame_s);
        self.status = if fresh && self.prerolled() {
            Status::Locked
        } else if fresh {
            Status::Preroll
        } else if !self.last_moved && t - last.t <= STILL_FOR_S {
            Status::Stopped
        } else if self.locked_at.is_some() && !self.after_roll_over(t) {
            Status::Afterroll
        } else if self.moved.is_some() {
            Status::Lost
        } else {
            Status::NoSignal
        };
    }

    /// One engine frame: follow the source with `player`, if `allowed`.
    pub fn chase(&mut self, player: &mut Player, c: &Clock, allowed: Result<(), String>) {
        self.update_status(c.t);
        let fresh = self.received != self.handled;
        self.handled = self.received;
        if let Err(why) = allowed {
            self.blocked = Some(why);
            self.chasing = false;
            return;
        }
        self.blocked = None;
        let end = player.show.as_ref().map_or(0.0, Show::end);
        let Some((secs, rate)) = self.now(c.t) else { return };
        let target = secs - self.offset_s();
        let inside = (0.0..end).contains(&target);
        match self.status {
            Status::Locked if !inside => {
                if self.chasing {
                    player.halt(c);
                    self.chasing = false;
                }
                self.blocked = Some("timecode hors du show".into());
            }
            Status::Locked => {
                if !player.is_playing() {
                    player.seek(target, c);
                    player.play(c);
                    self.jumps += 1;
                } else {
                    let pull = if self.config.smooth { PULL } else { 1.0 };
                    if player.chase(target, c, pull, JUMP_FRAMES / rate.fps()) {
                        self.jumps += 1;
                    }
                }
                self.chasing = true;
            }
            // The show runs on by itself; pre-roll frames are not followed.
            Status::Preroll | Status::Afterroll => {
                if self.chasing && self.after_roll_over(c.t) {
                    player.halt(c);
                    self.chasing = false;
                }
            }
            _ => {
                let keep = self.status == Status::Stopped && self.config.after_roll_s.is_none();
                if self.chasing && !keep {
                    // Stopped where it is: nothing drawn, position kept.
                    player.halt(c);
                    self.chasing = false;
                }
                if self.status == Status::Stopped && fresh && !player.is_playing() && inside {
                    player.seek(target, c);
                }
            }
        }
    }

    /// The readout: `01:02:03:04` while a time is known.
    pub fn display(&self, t: f64) -> Option<String> {
        self.now(t).map(|(secs, rate)| Tc::from_seconds(secs, rate).to_string())
    }

    pub fn rate(&self) -> Option<Rate> {
        self.last.map(|r| self.rate_of(&r))
    }

    /// What the audio callback copies for the LTC decoder: -1 nothing.
    fn ltc_tap(&self) -> i32 {
        if self.source() == TcSource::Ltc {
            self.config.ltc_channel as i32
        } else {
            -1
        }
    }
}

/// Follows the loaded show's own settings; drops TC-IN after an emergency
/// stop, when the laser is disarmed, and when the input changes (another
/// source, port, channel or interface: its readings are forgotten).
pub fn sync(s: &mut Shared) {
    let show = s.timeline.show.as_ref().and_then(|sh| sh.timecode.clone());
    if show != s.timecode.show {
        s.timecode.show = show;
    }
    let input = s.timecode.input_key();
    if input != s.timecode.input {
        s.timecode.input = input;
        s.timecode.forget();
        if s.timecode.armed {
            disarm(s, ArmSource::System, "source");
        }
    }
    let trips = s.estop.trips();
    let laser = s.gate.is_armed();
    let tc = &mut s.timecode;
    let estop = trips != tc.estop_trips || s.estop.is_latched();
    let disarmed = tc.laser_armed && !laser;
    tc.estop_trips = trips;
    tc.laser_armed = laser;
    if tc.armed && (estop || disarmed) {
        let reason = if estop { "estop" } else { "disarm" };
        disarm(s, ArmSource::System, reason);
    }
}

fn disarm(s: &mut Shared, src: ArmSource, reason: &str) {
    s.timecode.armed = false;
    s.timecode.chasing = false;
    log::info!("TC-IN désarmé ({reason})");
    s.gate.log().record("timecode_in", Some(src.id()), json!({ "armed": false, "reason": reason }));
}

/// The operator arms or disarms TC-IN. Arming is refused while the
/// emergency stop is latched and without a source. Disarming leaves the
/// timeline as it is (a show keeps playing by itself).
pub fn set_armed(s: &mut Shared, on: bool, src: ArmSource) -> Result<(), &'static str> {
    sync(s);
    if on == s.timecode.armed {
        return Ok(());
    }
    if !on {
        disarm(s, src, "operator");
        return Ok(());
    }
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réinitialisez-le avant d'armer TC-IN");
    }
    if s.timecode.source() == TcSource::Off {
        return Err("choisissez d'abord une source de timecode");
    }
    s.timecode.armed = true;
    log::info!("TC-IN armé ({})", src.label_fr());
    s.gate.log().record("timecode_in", Some(src.id()), json!({ "armed": true, "source": s.timecode.source() }));
    Ok(())
}

/// MTC from the MIDI worker's batch (quarter frames and full frames).
pub fn midi(s: &mut Shared, events: &[MidiEvent]) {
    if s.timecode.source() != TcSource::Mtc {
        return;
    }
    let epoch = s.epoch;
    let tc = &mut s.timecode;
    for ev in events {
        if tc.config.midi_port.as_ref().is_some_and(|p| *p != ev.port) {
            continue;
        }
        let t = ev.at.saturating_duration_since(epoch).as_secs_f64();
        match &ev.msg {
            MidiMsg::QuarterFrame { data } => {
                if let Some((code, rate)) = tc.mtc.quarter(*data) {
                    tc.push(Reading { tc: code, rate, t, lag: mtc::PIECE7_LAG_FRAMES, running: true });
                }
            }
            MidiMsg::SysEx { bytes } => {
                if let Some((code, rate)) = mtc::full_frame(bytes) {
                    tc.mtc.reset();
                    tc.push(Reading { tc: code, rate, t, lag: 0.0, running: false });
                }
            }
            _ => {}
        }
    }
}

/// Art-Net timecode: tells the listener whether to receive it (and
/// where), and takes what arrived.
fn artnet_in(s: &mut Shared) {
    let tc = &s.timecode;
    let wanted = tc.source() == TcSource::ArtNet && tc.armed;
    let hub = std::sync::Arc::clone(&s.netin.hub);
    let bind = if wanted && !tc.config.interface.is_empty() { crate::netin::parse_bind(&tc.config.interface, hub.effective().artnet.port).ok() } else { None };
    hub.set_timecode(wanted, bind);
    if !wanted {
        return;
    }
    let epoch = s.epoch;
    for (code, rate, at) in hub.take_timecode() {
        let t = at.saturating_duration_since(epoch).as_secs_f64();
        s.timecode.push(Reading { tc: code, rate, t, lag: 0.0, running: true });
    }
}

/// `POST /api/test/timecode` (`--test-hooks`): one ArtTimeCode datagram,
/// handed over as the listener would. False: not wanted (another source,
/// or TC-IN disarmed), ignored like on the network.
pub fn inject_artnet(s: &Shared, packet: &[u8]) -> Result<bool, String> {
    match crate::netin::artnet::decode(packet)? {
        crate::netin::artnet::Packet::TimeCode { tc, rate } => Ok(s.netin.hub.push_timecode((tc, rate, std::time::Instant::now()))),
        _ => Err("pas un ArtTimeCode".into()),
    }
}

/// Whether the timecode may drive the timeline now; the reason if not.
fn allowed(s: &mut Shared) -> Result<(), String> {
    if s.timecode.source() == TcSource::Off {
        return Err("aucune source".into());
    }
    if !s.timecode.armed {
        return Err("TC-IN désarmé".into());
    }
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché".into());
    }
    external::timecode_allows(s).map_err(|r| r.label_fr().to_string())?;
    if s.test_pattern.active.is_some() {
        return Err("mire de test affichée".into());
    }
    match s.timeline.show.as_ref().map(|sh| sh.time_base) {
        None => Err("aucun show chargé".into()),
        Some(TimeBase::Beats) => Err("show en temps : le timecode ne pilote que les shows en secondes".into()),
        Some(TimeBase::Seconds) => Ok(()),
    }
}

/// Once per engine frame, before the timeline advances (main.rs).
pub fn frame(s: &mut Shared, c: &Clock) {
    sync(s);
    s.audio_in.set_ltc_tap(s.timecode.ltc_tap());
    if s.timecode.source() == TcSource::Ltc {
        if let Some((gen, f, t)) = s.audio_in.ltc_since(s.timecode.ltc_gen) {
            s.timecode.ltc_gen = gen;
            s.timecode.push(Reading { tc: f.tc, rate: Rate::from_ltc(f.fps, f.drop_frame), t, lag: 1.0, running: true });
        }
    }
    artnet_in(s);
    let ok = allowed(s);
    let (tc, player) = (&mut s.timecode, &mut s.timeline);
    tc.chase(player, c, ok);
}

/// The panel's summary (`/api/frame`, `/api/events`).
pub fn summary(s: &Shared) -> Value {
    let tc = &s.timecode;
    let t = s.now_s();
    json!({
        "source": tc.source(),
        "armed": tc.armed,
        "status": tc.status,
        "status_fr": tc.status.label_fr(),
        "state": tc.status.state(),
        "tc": tc.display(t),
        "rate": tc.rate(),
        "rate_fr": tc.rate().map(Rate::label_fr),
        "blocked": tc.blocked,
        "chasing": tc.chasing,
        "offset_s": tc.offset_s(),
        // The loaded show has its own source, rate and offset.
        "per_show": tc.show.is_some(),
    })
}

/// `GET /api/timecode`.
pub fn view(s: &mut Shared) -> Value {
    sync(s);
    let mut v = summary(s);
    let audio = s.audio_in.status();
    let native = s.audio_in.config().0.source == crate::audio::AudioInputSource::Native;
    let net = s.netin.hub.effective();
    v["config"] = json!(s.timecode.config);
    v["show"] = json!(s.timecode.show);
    v["show_name"] = json!(s.timeline.show.as_ref().map(|sh| sh.name.as_str()));
    v["midi_ports"] = json!(s.midi.devices.iter().filter(|d| d.connected).map(|d| d.name.as_str()).collect::<Vec<_>>());
    v["audio"] = json!({ "native": native, "channels": audio.channels, "message": audio.message });
    v["artnet"] = json!({
        "port": net.artnet.port,
        "chart": net.artnet.enabled,
        "listening": s.netin.hub.status(crate::netin::Proto::ArtNet).local,
        "interfaces": crate::netin::interfaces(),
    });
    v["rates"] = json!(Rate::ALL.iter().map(|r| json!({ "id": r, "label": r.label_fr() })).collect::<Vec<_>>());
    v
}

/// `POST /api/timecode` `{ "source": "mtc", "offset_s": 3600, … }`: the
/// fields given replace the current ones; `"show": { "source", "fps",
/// "offset_s" }` (or null) sets the loaded show's own settings, saved with
/// the show. Returns whether anything changed; the caller saves the
/// global settings. A new source forgets the old one's readings and drops
/// TC-IN. An input on the timecode output's port or interface is refused
/// (409, the self-drive trap, T-417).
pub fn update(s: &mut Shared, patch: &Value) -> Result<bool, out::Refused> {
    let obj = patch.as_object().ok_or("objet JSON attendu")?;
    let show_patch = match obj.get("show") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            let st: ShowTimecode = serde_json::from_value(v.clone()).map_err(|e| format!("réglage du show invalide : {e}"))?;
            st.validate()?;
            Some(Some(st))
        }
    };
    if show_patch.is_some() && s.timeline.show.is_none() {
        return Err("aucun show chargé".into());
    }
    let mut v = json!(s.timecode.config);
    for (k, val) in obj.iter().filter(|(k, _)| *k != "show") {
        if v.get(k).is_none() || k == "out" {
            return Err(format!("réglage inconnu : {k}").into());
        }
        v[k] = val.clone();
    }
    let cfg: TcConfig = serde_json::from_value(v).map_err(|e| format!("réglage invalide : {e}"))?;
    cfg.validate()?;
    if cfg.out.enabled {
        let source = match &show_patch {
            Some(Some(st)) => st.source,
            Some(None) => cfg.source,
            None => s.timecode.show.as_ref().map_or(cfg.source, |sh| sh.source),
        };
        if let Some(why) = out::conflicts(&cfg.out, &cfg, source) {
            return Err(out::Refused::conflict(why));
        }
    }
    let before = s.timecode.config.clone();
    let mut changed = false;
    if cfg != before {
        if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
            s.gate.log().record("timecode_settings", Some(ArmSource::Ui.id()), change);
        }
        s.timecode.config = cfg;
        changed = true;
    }
    if let Some(st) = show_patch {
        changed |= set_show(s, st);
    }
    sync(s);
    Ok(changed)
}

/// The loaded show's own settings: written to its file at once (a
/// built-in example's copy only once it is saved under a name).
fn set_show(s: &mut Shared, st: Option<ShowTimecode>) -> bool {
    let example = s.timeline.is_example();
    let Some(show) = s.timeline.show.as_mut() else { return false };
    if show.timecode == st {
        return false;
    }
    s.gate.log().record("timecode_settings", Some(ArmSource::Ui.id()), json!({ "show": show.name, "timecode": st }));
    show.timecode = st;
    let saved = !example && crate::timeline::valid_show_name(&show.name) && s.shows.save(show).is_ok();
    if saved {
        s.timeline.saved();
    } else {
        s.timeline.touch();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;
    use crate::timeline::{Event, EventSource, Track};

    fn tc(h: u8, m: u8, s: u8, f: u8) -> Tc {
        Tc { h, m, s, f }
    }

    fn show(len: f64) -> Show {
        let event = Event { id: 1, start: 0.0, len, source: EventSource::Cue { id: "x".into() }, ..Default::default() };
        Show { name: "tc".into(), tracks: vec![Track { events: vec![event], ..Default::default() }], ..Default::default() }
    }

    fn clock(t: f64) -> Clock {
        Clock { t, beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 }
    }

    #[test]
    fn labels_frames_and_seconds() {
        for rate in Rate::ALL {
            for n in [0i64, 1, 29, 1_798, 1_799, 1_800, 17_982, 17_983, 107_892, 2_589_407] {
                let n = n % (24 * 3600 * rate.fps() as i64);
                let label = Tc::from_frames(n, rate);
                assert!(label.valid(rate), "{rate:?} {n} {label}");
                assert_eq!(label.frames(rate), n, "{rate:?} {label}");
            }
        }
        // Drop frame: 00:00:59;29 is followed by 00:01:00;02, and ten
        // minutes are exactly 17 982 frames.
        let r = Rate::F2997Df;
        assert_eq!(Tc::from_frames(tc(0, 0, 59, 29).frames(r) + 1, r), tc(0, 1, 0, 2));
        assert_eq!(tc(0, 10, 0, 0).frames(r), 17_982);
        assert!((tc(1, 0, 0, 0).seconds(r) - 3600.0).abs() < 0.004, "an hour of drop frame is an hour");
        assert_eq!(tc(1, 0, 0, 0).seconds(Rate::F25), 3600.0);
        assert_eq!(tc(0, 0, 1, 12).seconds(Rate::F24), 1.5);
        assert_eq!(Tc::from_seconds(3723.5, Rate::F30).to_string(), "01:02:03:15");
        assert_eq!(Tc::from_seconds(-1.0 / 25.0, Rate::F25).to_string(), "23:59:59:24");
    }

    #[test]
    fn ltc_rates_from_speed_and_flag() {
        assert_eq!(Rate::from_ltc(23.9, false), Rate::F24);
        assert_eq!(Rate::from_ltc(25.3, false), Rate::F25);
        assert_eq!(Rate::from_ltc(29.97, false), Rate::F30);
        assert_eq!(Rate::from_ltc(29.97, true), Rate::F2997Df);
    }

    /// MTC quarter frames sent on time: the show plays at the timecode
    /// within one frame, started by the second complete time.
    #[test]
    fn simulated_mtc_drives_the_show_within_a_frame() {
        for rate in Rate::ALL {
            let mut st = TimecodeState::in_memory();
            st.config.source = TcSource::Mtc;
            st.config.offset_s = 3600.0;
            let mut player = Player::default();
            player.load(show(120.0));
            let start = tc(1, 0, 10, 0).frames(rate);
            let frame_s = 1.0 / rate.fps();
            // Quarter frames at t0 + k / 4 frames; the engine at 60 fps.
            let t0 = 100.0;
            let mut qf = Vec::new();
            for pair in 0..60 {
                let bytes = mtc::quarter_frames(Tc::from_frames(start + 2 * pair, rate), rate);
                for (k, b) in bytes.iter().enumerate() {
                    qf.push((t0 + ((2 * pair) as f64 + k as f64 / 4.0) * frame_s, *b));
                }
            }
            let mut i = 0;
            let mut t = t0;
            let mut checked = 0;
            while t < t0 + 110.0 * frame_s {
                while i < qf.len() && qf[i].0 <= t {
                    if let Some((code, r)) = st.mtc.quarter(qf[i].1) {
                        st.push(Reading { tc: code, rate: r, t: qf[i].0, lag: mtc::PIECE7_LAG_FRAMES, running: true });
                    }
                    i += 1;
                }
                st.chase(&mut player, &clock(t), Ok(()));
                if t > t0 + 8.0 * frame_s {
                    // Where the source is now, in show time.
                    let want = (start as f64 * frame_s - 3600.0) + (t - t0);
                    let got = player.position_at(&clock(t));
                    assert!(player.is_playing(), "{rate:?} at {t}");
                    assert!((got - want).abs() <= frame_s, "{rate:?}: {got} vs {want}");
                    assert_eq!(st.status(), Status::Locked);
                    checked += 1;
                }
                t += 1.0 / 60.0;
            }
            assert!(checked > 100);
            assert_eq!(st.rate(), Some(rate));
        }
    }

    fn running(st: &mut TimecodeState, label: Tc, t: f64) {
        st.push(Reading { tc: label, rate: Rate::F25, t, lag: 0.0, running: true });
    }

    /// The source at `x` s at time `t` (the part of a frame in `lag`).
    fn running_at(st: &mut TimecodeState, x: f64, t: f64) {
        let label = Tc::from_seconds(x, Rate::F25);
        st.push(Reading { tc: label, rate: Rate::F25, t, lag: (x - label.seconds(Rate::F25)) * 25.0, running: true });
    }

    /// 25 im/s timecode at source time `src(t)`, from engine time `t0` to
    /// `t1`, chased at 60 fps; returns the engine time at the end.
    fn feed(st: &mut TimecodeState, player: &mut Player, t0: f64, t1: f64, src: impl Fn(f64) -> f64) -> f64 {
        let mut t = t0;
        let mut next = t0;
        while t < t1 {
            while next <= t {
                running_at(st, src(next), next);
                next += 0.04;
            }
            st.chase(player, &clock(t), Ok(()));
            t += 1.0 / 60.0;
        }
        t
    }

    fn ltc_state() -> (TimecodeState, Player) {
        let mut st = TimecodeState::in_memory();
        st.config.source = TcSource::Ltc;
        let mut player = Player::default();
        player.load(show(600.0));
        (st, player)
    }

    #[test]
    fn a_dropout_runs_the_post_roll_then_stops_after_a_second() {
        let (mut st, mut player) = ltc_state();
        let t = feed(&mut st, &mut player, 0.0, 1.0, |t| 5.0 + t);
        assert!(player.is_playing() && st.is_chasing());
        // Signal gone: a few frames later, post-roll; the show runs on.
        st.chase(&mut player, &clock(t + 0.2), Ok(()));
        assert_eq!(st.status(), Status::Afterroll);
        assert!(player.is_playing());
        st.chase(&mut player, &clock(t + 0.9), Ok(()));
        assert!(player.is_playing(), "still within the 1 s post-roll");
        // After 1 s: stopped where it was, nothing drawn.
        st.chase(&mut player, &clock(t + 1.05), Ok(()));
        assert_eq!(st.status(), Status::Lost);
        assert!(!player.is_playing() && !st.is_chasing());
        assert!(player.frame(&clock(t + 1.1)).is_empty());
        let pos = player.state(&clock(t + 1.1)).position;
        assert!((pos - 7.05).abs() < 0.1, "position kept: {pos}");
        // The timecode comes back: chased again from where it is.
        let t2 = feed(&mut st, &mut player, t + 3.0, t + 3.5, |x| 20.0 + x - (t + 3.0));
        assert!(player.is_playing());
        assert!((player.position_at(&clock(t2)) - (20.0 + t2 - (t + 3.0))).abs() < 0.04);
    }

    #[test]
    fn continuer_is_a_post_roll_without_limit() {
        let (mut st, mut player) = ltc_state();
        st.config.after_roll_s = None;
        let t = feed(&mut st, &mut player, 0.0, 1.0, |t| 5.0 + t);
        st.chase(&mut player, &clock(t + 5.0), Ok(()));
        assert_eq!(st.status(), Status::Afterroll);
        assert!(player.is_playing());
        assert!((player.position_at(&clock(t + 5.0)) - (10.0 + t)).abs() < 0.04);
    }

    #[test]
    fn jumps_seek_and_small_errors_are_smoothed() {
        let (mut st, mut player) = ltc_state();
        feed(&mut st, &mut player, 0.0, 1.0, |t| 10.0 + t);
        let jumps = st.jumps;
        // The source jumps to 200 s: the show follows at once.
        let src = |t: f64| 200.0 + (t - 1.0);
        let t = feed(&mut st, &mut player, 1.0, 1.2, src);
        assert_eq!(st.jumps, jumps + 1);
        assert!((player.position_at(&clock(t)) - src(t)).abs() < 0.04);
        // Two frames ahead, with Lissage: no seek, taken up gradually.
        let src = |t: f64| 200.08 + (t - 1.0);
        let t = feed(&mut st, &mut player, t, 1.25, src);
        assert_eq!(st.jumps, jumps + 1, "not a jump");
        let lag = src(t) - player.position_at(&clock(t));
        assert!(lag > 0.02, "not in one step: {lag}");
        let t = feed(&mut st, &mut player, t, 2.25, src);
        assert!((player.position_at(&clock(t)) - src(t)).abs() < 0.01, "caught up");
        // Without Lissage the playhead is set on the time.
        st.config.smooth = false;
        let src = |t: f64| 200.16 + (t - 1.0);
        let t = feed(&mut st, &mut player, t, t + 0.4, src);
        assert_eq!(st.jumps, jumps + 1);
        assert!((player.position_at(&clock(t)) - src(t)).abs() < 0.02);
    }

    #[test]
    fn outside_the_show_is_ignored_and_a_locate_cues_up() {
        let mut st = TimecodeState::in_memory();
        st.config.source = TcSource::Mtc;
        st.config.offset_s = 100.0;
        let mut player = Player::default();
        player.load(show(30.0));
        // Before the show (pre-roll): nothing plays.
        feed(&mut st, &mut player, 0.0, 1.0, |t| 95.0 + t);
        assert!(!player.is_playing());
        assert_eq!(st.blocked.as_deref(), Some("timecode hors du show"));
        // A full frame (still) at 110 s: cued up at 10 s, not playing.
        st.push(Reading { tc: tc(0, 1, 50, 0), rate: Rate::F25, t: 10.0, lag: 0.0, running: false });
        st.chase(&mut player, &clock(10.0), Ok(()));
        assert_eq!(st.status(), Status::Stopped);
        assert!(!player.is_playing());
        assert_eq!(player.state(&clock(10.0)).position, 10.0);
        // Running, then past the end: the show it drove stops.
        feed(&mut st, &mut player, 20.0, 21.0, |t| 108.0 + t - 20.0);
        assert!(player.is_playing());
        feed(&mut st, &mut player, 21.0, 21.5, |t| 131.0 + t - 21.0);
        assert!(!player.is_playing());
        assert_eq!(st.blocked.as_deref(), Some("timecode hors du show"));
    }

    #[test]
    fn nothing_happens_when_not_allowed() {
        let mut st = TimecodeState::in_memory();
        st.config.source = TcSource::Ltc;
        let mut player = Player::default();
        player.load(show(60.0));
        let mut t = 0.0;
        for i in 0..50 {
            running(&mut st, Tc::from_frames(250 + i, Rate::F25), t);
            st.chase(&mut player, &clock(t), Err("TC-IN désarmé".into()));
            t += 0.04;
        }
        assert!(!player.is_playing());
        assert_eq!(st.status(), Status::Locked);
        assert_eq!(st.blocked.as_deref(), Some("TC-IN désarmé"));
        assert_eq!(st.display(t).as_deref(), Some("00:00:12:00"));
    }

    fn armed_studio() -> Shared {
        let mut s = shared();
        update(&mut s, &json!({ "source": "mtc" })).unwrap();
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        s.timeline.load(show(60.0));
        s
    }

    fn mtc_batch(s: &mut Shared, from: i64, pairs: i64) {
        let at = std::time::Instant::now();
        let mut events = Vec::new();
        for p in 0..pairs {
            for b in mtc::quarter_frames(Tc::from_frames(from + 2 * p, Rate::F25), Rate::F25) {
                events.push(MidiEvent { port: "MTC".into(), msg: MidiMsg::QuarterFrame { data: b }, at });
            }
        }
        midi(s, &events);
    }

    /// Through `Shared`: the gate, the scope and TC-IN, MTC from the
    /// worker's batch, and an emergency stop that drops TC-IN for good.
    #[test]
    fn tc_in_gate_and_estop() {
        let mut s = armed_studio();
        mtc_batch(&mut s, 250, 3);
        let c = s.timeline_clock();
        frame(&mut s, &c);
        assert!(s.timeline.is_playing(), "{:?}", s.timecode.blocked);
        assert!(!s.gate.is_armed(), "timecode never arms");

        // Échap: TC-IN drops, the timeline halts, and timecode restarts nothing.
        s.emergency_stop(ArmSource::Keyboard);
        // As the engine does (main.rs `timeline_cues`).
        let c = s.timeline_clock();
        s.timeline.halt(&c);
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        for _ in 0..3 {
            mtc_batch(&mut s, 400, 3);
            let c = s.timeline_clock();
            frame(&mut s, &c);
            assert!(!s.timeline.is_playing());
        }
        assert!(!s.timecode.armed);
        assert_eq!(s.timecode.blocked.as_deref(), Some("TC-IN désarmé"));
        // Re-armed by hand: it chases again.
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        mtc_batch(&mut s, 500, 3);
        let c = s.timeline_clock();
        frame(&mut s, &c);
        assert!(s.timeline.is_playing());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn gate_scope_and_laser_disarm_block_it() {
        let mut s = armed_studio();
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        mtc_batch(&mut s, 250, 3);
        let c = s.timeline_clock();
        frame(&mut s, &c);
        assert!(!s.timeline.is_playing());
        assert!(s.timecode.blocked.as_deref().unwrap().contains("fermées"), "{:?}", s.timecode.blocked);
        // A scope without « Timeline »: refused too.
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        external::update_scopes(&mut s, &json!({ "scopes": [{ "source": "timecode", "groups": ["master"] }] })).unwrap();
        frame(&mut s, &c);
        assert!(!s.timeline.is_playing());
        assert!(s.timecode.blocked.as_deref().unwrap().contains("périmètre"));
        // A laser disarmed (it was armed): TC-IN drops.
        s.timecode.laser_armed = true;
        sync(&mut s);
        assert!(!s.timecode.armed);
        // Arming TC-IN is refused under a latched e-stop and without a source.
        s.emergency_stop(ArmSource::Ui);
        assert!(set_armed(&mut s, true, ArmSource::Ui).is_err());
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        update(&mut s, &json!({ "source": "off" })).unwrap();
        assert!(set_armed(&mut s, true, ArmSource::Ui).is_err());
    }

    #[test]
    fn port_filter_beats_shows_and_settings() {
        let mut s = armed_studio();
        update(&mut s, &json!({ "midi_port": "Autre" })).unwrap();
        assert!(!s.timecode.armed, "a new source drops TC-IN");
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        mtc_batch(&mut s, 250, 3);
        assert!(s.timecode.last.is_none(), "another port is ignored");
        update(&mut s, &json!({ "midi_port": null })).unwrap();
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        // A show in beats: the timecode is shown, never applied.
        let mut beats = show(16.0);
        beats.time_base = TimeBase::Beats;
        s.timeline.load(beats);
        mtc_batch(&mut s, 250, 3);
        let c = s.timeline_clock();
        frame(&mut s, &c);
        assert!(!s.timeline.is_playing());
        assert!(s.timecode.blocked.as_deref().unwrap().contains("en temps"));
        assert!(summary(&s)["tc"].as_str().is_some());
        // Settings: validated, unknown keys refused, saved without TC-IN.
        for bad in [json!({ "offset_s": 1e9 }), json!({ "after_roll_s": 61 }), json!({ "pre_roll_frames": 101 }), json!({ "interface": "239.1.1.1" }), json!({ "interface": "nom" }), json!({ "nope": 1 }), json!({ "source": "smpte" }), json!([1])] {
            assert!(update(&mut s, &bad).is_err(), "{bad}");
        }
        assert_eq!(update(&mut s, &json!({ "offset_s": 3600, "smooth": false })), Ok(true));
        assert_eq!(update(&mut s, &json!({ "offset_s": 3600 })), Ok(false));
        let saved = serde_json::to_string(s.timecode.config()).unwrap();
        assert!(!saved.contains("armed"), "{saved}");
        let back: TcConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(back, TcConfig::default());
    }

    /// LTC-like frames at 25 im/s from `from`, one per 40 ms from `t0`;
    /// returns the time of the last one.
    fn frames(st: &mut TimecodeState, player: &mut Player, from: i64, n: i64, t0: f64) -> f64 {
        let mut t = t0;
        for k in 0..n {
            t = t0 + k as f64 * 0.04;
            running(st, Tc::from_frames(from + k, Rate::F25), t);
            st.chase(player, &clock(t), Ok(()));
        }
        t
    }

    #[test]
    fn pre_roll_ignores_the_first_frames_and_restarts_on_a_gap() {
        let (mut st, mut player) = ltc_state();
        assert_eq!(st.config.pre_roll_frames, 5);
        // n - 1 frames: nothing followed.
        let t = frames(&mut st, &mut player, 250, 4, 0.0);
        assert_eq!(st.status(), Status::Preroll);
        assert!(!player.is_playing() && !st.is_chasing());
        // The n-th: followed, from that frame.
        running(&mut st, Tc::from_frames(254, Rate::F25), t + 0.04);
        st.chase(&mut player, &clock(t + 0.04), Ok(()));
        assert_eq!(st.status(), Status::Locked);
        assert!(player.is_playing());
        assert!((player.position_at(&clock(t + 0.04)) - 254.0 / 25.0).abs() < 1e-6);
        // A frame that doesn't follow (a jump of 3 s): pre-roll again, the
        // show runs on by itself meanwhile, then seeks once locked.
        let t = t + 0.08;
        running(&mut st, Tc::from_frames(330, Rate::F25), t);
        st.chase(&mut player, &clock(t), Ok(()));
        assert_eq!(st.status(), Status::Preroll);
        assert!((player.position_at(&clock(t)) - (254.0 / 25.0 + 0.04)).abs() < 1e-6, "not followed yet");
        let t = frames(&mut st, &mut player, 331, 4, t + 0.04);
        assert_eq!(st.status(), Status::Locked);
        assert!((player.position_at(&clock(t)) - 334.0 / 25.0).abs() < 1e-6);
        // Without a pre-roll, the second frame locks.
        let (mut st, mut player) = ltc_state();
        st.config.pre_roll_frames = 0;
        frames(&mut st, &mut player, 100, 2, 0.0);
        assert!(player.is_playing());
    }

    #[test]
    fn a_gap_shorter_than_the_post_roll_keeps_the_show_going() {
        let (mut st, mut player) = ltc_state();
        st.config.after_roll_s = Some(0.5);
        let t = frames(&mut st, &mut player, 250, 20, 0.0);
        assert!(player.is_playing());
        // 0.4 s without timecode: post-roll, the show runs on.
        st.chase(&mut player, &clock(t + 0.4), Ok(()));
        assert_eq!(st.status(), Status::Afterroll);
        assert!(player.is_playing() && st.is_chasing());
        // Back after a gap: a new pre-roll, the show still running.
        let t2 = frames(&mut st, &mut player, 280, 6, t + 0.44);
        assert_eq!(st.status(), Status::Locked);
        assert!(player.is_playing());
        // 0.6 s without: stopped, « Timecode perdu ».
        st.chase(&mut player, &clock(t2 + 0.6), Ok(()));
        assert_eq!(st.status(), Status::Lost);
        assert_eq!(st.status().label_fr(), "Timecode perdu");
        assert_eq!(st.status().state(), "lost");
        assert!(!player.is_playing() && !st.is_chasing());
    }

    #[test]
    fn a_time_outside_the_show_is_ignored() {
        let (mut st, mut player) = ltc_state();
        st.config.offset_s = 3600.0;
        frames(&mut st, &mut player, 25 * 60, 10, 0.0);
        assert!(!player.is_playing());
        assert_eq!(st.blocked.as_deref(), Some("timecode hors du show"));
    }

    #[test]
    fn drop_frame_labels_and_seconds_over_a_day() {
        let r = Rate::F2997Df;
        assert_eq!(Tc::from_frames(tc(0, 0, 59, 29).frames(r) + 1, r).to_string(), "00:01:00:02");
        let day = 24 * 3600 * 30 - 2 * (24 * 60 - 24 * 6);
        assert_eq!(tc(23, 59, 59, 29).frames(r), day - 1);
        // Every frame of the day: a label that exists, and back to the
        // same frame count and the exact time.
        for n in 0..day {
            let label = Tc::from_frames(n, r);
            assert!(label.valid(r), "{label}");
            assert_eq!(label.frames(r), n);
            assert_eq!(label.seconds(r), n as f64 / r.fps());
        }
        assert_eq!(Tc::from_frames(day, r), tc(0, 0, 0, 0), "wraps at midnight");
        assert!((tc(23, 59, 59, 29).seconds(r) - (day - 1) as f64 * 1.001 / 30.0).abs() < 1e-6);
    }

    #[test]
    fn legacy_settings_become_the_post_roll() {
        let dir = std::env::temp_dir().join(format!("laser-studio-tc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("timecode.json");
        std::fs::write(&path, r#"{"source":"ltc","keep_running":true,"freewheel_frames":50}"#).unwrap();
        let st = TimecodeState::load(path.clone());
        assert_eq!((st.config.source, st.config.after_roll_s), (TcSource::Ltc, None));
        std::fs::write(&path, r#"{"source":"mtc","freewheel_frames":50}"#).unwrap();
        assert_eq!(TimecodeState::load(path.clone()).config.after_roll_s, Some(2.0));
        assert_eq!(TimecodeState::load(dir.join("none.json")).config, TcConfig::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn artnet_studio() -> Shared {
        let mut s = shared();
        update(&mut s, &json!({ "source": "artnet" })).unwrap();
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        s.timeline.load(show(60.0));
        s
    }

    /// One engine frame, `n` consecutive Art-Net frames from `from` at
    /// 25 im/s handed over as the listener does, one more engine frame.
    /// Returns the packets the receiver took.
    fn artnet_frames(s: &mut Shared, from: i64, n: i64) -> usize {
        let c = s.timeline_clock();
        frame(s, &c);
        let mut taken = 0;
        for k in 0..n {
            let packet = artnet::encode(Tc::from_frames(from + k, Rate::F25), Rate::F25);
            taken += inject_artnet(s, &packet).unwrap() as usize;
        }
        let c = s.timeline_clock();
        frame(s, &c);
        taken
    }

    /// Art-Net timecode through `Shared`: received only with TC-IN armed,
    /// moves the playhead past the pre-roll, never arms; after Échap
    /// nothing restarts until TC-IN is armed again by hand.
    #[test]
    fn artnet_drives_the_show_and_never_arms() {
        let mut s = artnet_studio();
        assert_eq!(artnet_frames(&mut s, 250, 3), 3);
        assert!(!s.timeline.is_playing(), "inside the pre-roll");
        assert_eq!(s.timecode.status(), Status::Preroll);
        artnet_frames(&mut s, 253, 3);
        assert!(s.timeline.is_playing(), "{:?}", s.timecode.blocked);
        let pos = s.timeline.position_at(&s.timeline_clock());
        assert!((pos - 255.0 / 25.0).abs() < 0.1, "{pos}");
        assert_eq!(summary(&s)["state"], "locked");
        assert!(!s.gate.is_armed(), "timecode never arms");

        s.emergency_stop(ArmSource::Keyboard);
        let c = s.timeline_clock();
        s.timeline.halt(&c);
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        for _ in 0..3 {
            assert_eq!(artnet_frames(&mut s, 400, 8), 0, "the receiver is closed");
            assert!(!s.timeline.is_playing());
        }
        assert!(!s.timecode.armed && !s.gate.is_armed());
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        artnet_frames(&mut s, 500, 8);
        assert!(s.timeline.is_playing());
        assert!(!s.gate.is_armed());
        // TC-IN off: the receiver closes and drops what waits.
        set_armed(&mut s, false, ArmSource::Ui).unwrap();
        assert_eq!(artnet_frames(&mut s, 600, 8), 0);
    }

    #[test]
    fn a_show_has_its_own_source_and_offset_or_follows_the_global_ones() {
        let mut s = artnet_studio();
        // An older show file, without `timecode`: the global settings.
        let old: Show = serde_json::from_str(r#"{"name":"vieux","tracks":[{"events":[{"start":0,"len":30,"source":{"kind":"cue","id":"x"}}]}]}"#).unwrap();
        assert_eq!(old.timecode, None);
        s.timeline.load(old);
        sync(&mut s);
        assert_eq!((s.timecode.source(), s.timecode.offset_s()), (TcSource::ArtNet, 0.0));
        assert!(s.timecode.armed);
        // Its own: MTC from 01:00:00:00. The input changed: TC-IN drops.
        update(&mut s, &json!({ "show": { "source": "mtc", "fps": "25", "offset_s": 3600 } })).unwrap();
        assert_eq!((s.timecode.source(), s.timecode.offset_s()), (TcSource::Mtc, 3600.0));
        assert!(!s.timecode.armed, "a new source drops TC-IN");
        assert_eq!(s.timeline.show.as_ref().unwrap().timecode.as_ref().unwrap().fps, TcFps::F25);
        assert_eq!(summary(&s)["per_show"], true);
        assert_eq!(s.timecode.config().source, TcSource::ArtNet, "the global setting is kept");
        // The editor keeps it.
        let mut edited = s.timeline.show.clone().unwrap();
        edited.timecode = None;
        s.timeline.edit(edited);
        assert!(s.timeline.show.as_ref().unwrap().timecode.is_some());
        // Back to the global settings; refused when invalid or without a show.
        update(&mut s, &json!({ "show": null })).unwrap();
        assert_eq!(s.timecode.source(), TcSource::ArtNet);
        assert!(update(&mut s, &json!({ "show": { "offset_s": 1e9 } })).is_err());
        s.timeline.show = None;
        assert!(update(&mut s, &json!({ "show": { "source": "ltc" } })).is_err());
    }
}
