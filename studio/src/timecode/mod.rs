//! Incoming timecode (T-168): the timeline chases a show controller's
//! clock, read as MIDI Time Code (mtc.rs, from the MIDI worker) or as LTC
//! on an audio input channel (ltc.rs, decoded on the audio analysis
//! thread).
//!
//! What timecode may do, and what it never does:
//! - It drives the **timeline only**: seek, play, and the playhead's
//!   speed. It never arms the laser, never touches a cue, a master or a
//!   safety setting. The laser stays as the operator left it.
//! - It acts only when the operator armed **TC-IN** (memory only, off at
//!   every start), the external inputs gate is open and the *Timecode*
//!   source's scope includes *Timeline* (external.rs, T-408). TC-IN is
//!   that source's access.
//! - An emergency stop (Échap) or the laser being disarmed drops TC-IN:
//!   after a blackout, timecode restarts nothing until TC-IN is armed
//!   again by hand.
//! - Only shows in seconds follow it; a time outside the show (after the
//!   offset) is ignored and stops a show it was driving.
//!
//! Chase policy, every engine frame (`frame`):
//! - **Locked** (a moving time less than 4 frames old): the show plays at
//!   `timecode - offset`. Started if needed; a playhead more than
//!   `JUMP_FRAMES` away is a jump of the source: seek. Otherwise, with
//!   *Lissage*, a tenth of the error is taken up per frame (jitter
//!   smoothed, the source's speed followed), without it the playhead is
//!   set on the time.
//! - **Roue libre** (no new time, for up to `freewheel_frames` frames):
//!   the show runs on by itself.
//! - Beyond that: the show it was driving stops (it keeps its position,
//!   nothing is drawn), unless *Continuer si perte* is ticked.
//! - A time that doesn't move (an MTC full frame when the source
//!   locates, frozen LTC) cues a stopped show up at that position.
//!
//! The frame rate comes from the signal (MTC carries it; LTC from its
//! measured speed and drop-frame flag) unless one is forced.

pub mod ltc;
pub mod mtc;

use crate::external;
use crate::interlock::ArmSource;
use crate::midi::{MidiEvent, MidiMsg};
use crate::timeline::{Clock, Player, Show, TimeBase};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::path::PathBuf;

pub const DEFAULT_FREEWHEEL_FRAMES: u32 = 25;
/// MTC completes a time every 2 frames: less than this would flicker.
pub const MIN_FREEWHEEL_FRAMES: u32 = 5;
pub const MAX_FREEWHEEL_FRAMES: u32 = 300;
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

    #[cfg(test)]
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
}

impl TcSource {
    pub fn label_fr(self) -> &'static str {
        match self {
            TcSource::Off => "Aucune",
            TcSource::Mtc => "MTC",
            TcSource::Ltc => "LTC",
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
    /// *Continuer si perte*: the show runs on after the freewheel.
    pub keep_running: bool,
    /// Frames of *roue libre* before a show driven by timecode stops.
    pub freewheel_frames: u32,
    /// *Lissage*: pull the playhead gently instead of setting it.
    pub smooth: bool,
    /// Show position = timecode - offset (s): a show starting at TC
    /// 01:00:00:00 has 3600.
    pub offset_s: f64,
    /// MTC from this port only (none: any port).
    pub midi_port: Option<String>,
    /// LTC on this input channel (1-based); 0: the mix of all channels.
    pub ltc_channel: u16,
}

impl Default for TcConfig {
    fn default() -> Self {
        TcConfig { source: TcSource::Off, fps: TcFps::Auto, keep_running: false, freewheel_frames: DEFAULT_FREEWHEEL_FRAMES, smooth: true, offset_s: 0.0, midi_port: None, ltc_channel: 0 }
    }
}

impl TcConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.offset_s.is_finite() || self.offset_s.abs() > MAX_OFFSET_S {
            return Err("décalage hors limites (± 24 h)".into());
        }
        if !(MIN_FREEWHEEL_FRAMES..=MAX_FREEWHEEL_FRAMES).contains(&self.freewheel_frames) {
            return Err(format!("roue libre : de {MIN_FREEWHEEL_FRAMES} à {MAX_FREEWHEEL_FRAMES} images"));
        }
        if self.ltc_channel > MAX_LTC_CHANNEL {
            return Err(format!("voie LTC : de 0 à {MAX_LTC_CHANNEL}"));
        }
        if self.midi_port.as_ref().is_some_and(|p| p.is_empty() || p.len() > 128) {
            return Err("port MIDI invalide".into());
        }
        Ok(())
    }

    /// What the audio callback copies for the LTC decoder: -1 nothing.
    pub fn ltc_tap(&self) -> i32 {
        if self.source == TcSource::Ltc {
            self.ltc_channel as i32
        } else {
            -1
        }
    }
}

/// One time read off the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub tc: Tc,
    /// The rate the source says (MTC) or seems to run at (LTC).
    pub rate: Rate,
    /// Studio seconds (`Shared::now_s`) at which the source was at `tc`
    /// plus `lag` frames.
    pub t: f64,
    pub lag: f64,
    /// Quarter frames or LTC (false: an MTC full frame, a locate).
    pub running: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Off,
    NoSignal,
    Locked,
    Freewheel,
    Stopped,
    Lost,
}

impl Status {
    pub fn label_fr(self) -> &'static str {
        match self {
            Status::Off => "Pas de source",
            Status::NoSignal => "Pas de timecode",
            Status::Locked => "Verrouillé",
            Status::Freewheel => "Roue libre",
            Status::Stopped => "Timecode à l'arrêt",
            Status::Lost => "Timecode perdu",
        }
    }
}

/// `Shared.timecode`.
pub struct TimecodeState {
    config: TcConfig,
    path: Option<PathBuf>,
    /// TC-IN: memory only, off at every start.
    pub armed: bool,
    estop_trips: u64,
    laser_armed: bool,
    mtc: mtc::MtcDecoder,
    last: Option<Reading>,
    /// The newest reading that moved on from the one before.
    moved: Option<Reading>,
    last_moved: bool,
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
}

impl TimecodeState {
    fn with(config: TcConfig, path: Option<PathBuf>) -> Self {
        TimecodeState {
            config,
            path,
            armed: false,
            estop_trips: 0,
            laser_armed: false,
            mtc: Default::default(),
            last: None,
            moved: None,
            last_moved: false,
            received: 0,
            handled: 0,
            ltc_gen: 0,
            chasing: false,
            jumps: 0,
            status: Status::Off,
            blocked: None,
        }
    }

    pub fn load(path: PathBuf) -> Self {
        let mut config: TcConfig = crate::load_json(&path);
        if let Err(e) = config.validate() {
            log::warn!("timecode.json : {e} ; réglages par défaut");
            config = TcConfig::default();
        }
        Self::with(config, Some(path))
    }

    pub fn in_memory() -> Self {
        Self::with(TcConfig::default(), None)
    }

    pub fn config(&self) -> &TcConfig {
        &self.config
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
        self.chasing = false;
    }

    fn rate_of(&self, r: &Reading) -> Rate {
        self.config.fps.forced().unwrap_or(r.rate)
    }

    fn secs_of(&self, r: &Reading) -> f64 {
        let rate = self.rate_of(r);
        r.tc.seconds(rate) + r.lag / rate.fps()
    }

    /// A time from the source.
    pub fn push(&mut self, r: Reading) {
        let moved = r.running && self.last.is_some_and(|p| p.running && p.tc != r.tc);
        if moved {
            self.moved = Some(r);
        }
        self.last_moved = moved;
        self.last = Some(r);
        self.received += 1;
    }

    /// The timecode now (seconds) and its rate: run on from the last time
    /// while locked.
    pub fn now(&self, t: f64) -> Option<(f64, Rate)> {
        let last = self.last?;
        let mut secs = self.secs_of(&last);
        if self.status == Status::Locked {
            secs += (t - last.t).max(0.0);
        }
        Some((secs, self.rate_of(&last)))
    }

    fn update_status(&mut self, t: f64) {
        let Some(last) = self.last else {
            self.status = if self.config.source == TcSource::Off { Status::Off } else { Status::NoSignal };
            return;
        };
        let frame_s = 1.0 / self.rate_of(&last).fps();
        let age = self.moved.map(|m| t - m.t);
        let freewheel = self.config.freewheel_frames.max(MIN_FREEWHEEL_FRAMES) as f64 * frame_s;
        self.status = match age {
            Some(a) if self.last_moved && a <= DROPOUT_FRAMES * frame_s => Status::Locked,
            Some(a) if a <= freewheel => Status::Freewheel,
            _ if !self.last_moved && t - last.t <= STILL_FOR_S => Status::Stopped,
            Some(_) => Status::Lost,
            None => Status::NoSignal,
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
        let target = secs - self.config.offset_s;
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
            // The show runs on by itself.
            Status::Freewheel => {}
            _ => {
                if self.chasing && !self.config.keep_running {
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

    /// The readout: `TC 01:02:03:04` while a time is known.
    pub fn display(&self, t: f64) -> Option<String> {
        self.now(t).map(|(secs, rate)| Tc::from_seconds(secs, rate).to_string())
    }

    pub fn rate(&self) -> Option<Rate> {
        self.last.map(|r| self.rate_of(&r))
    }
}

/// Drops TC-IN after an emergency stop or when the laser is disarmed.
pub fn sync(s: &mut Shared) {
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
    if s.timecode.config.source == TcSource::Off {
        return Err("choisissez d'abord une source de timecode");
    }
    s.timecode.armed = true;
    log::info!("TC-IN armé ({})", src.label_fr());
    s.gate.log().record("timecode_in", Some(src.id()), json!({ "armed": true, "source": s.timecode.config.source }));
    Ok(())
}

/// MTC from the MIDI worker's batch (quarter frames and full frames).
pub fn midi(s: &mut Shared, events: &[MidiEvent]) {
    if s.timecode.config.source != TcSource::Mtc {
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

/// Whether the timecode may drive the timeline now; the reason if not.
fn allowed(s: &mut Shared) -> Result<(), String> {
    if s.timecode.config.source == TcSource::Off {
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
    s.audio_in.set_ltc_tap(s.timecode.config.ltc_tap());
    if s.timecode.config.source == TcSource::Ltc {
        if let Some((gen, f, t)) = s.audio_in.ltc_since(s.timecode.ltc_gen) {
            s.timecode.ltc_gen = gen;
            s.timecode.push(Reading { tc: f.tc, rate: Rate::from_ltc(f.fps, f.drop_frame), t, lag: 1.0, running: true });
        }
    }
    let ok = allowed(s);
    let (tc, player) = (&mut s.timecode, &mut s.timeline);
    tc.chase(player, c, ok);
}

/// The panel's summary (`/api/frame`, `/api/events`).
pub fn summary(s: &Shared) -> Value {
    let tc = &s.timecode;
    let t = s.now_s();
    json!({
        "source": tc.config.source,
        "armed": tc.armed,
        "status": tc.status,
        "status_fr": tc.status.label_fr(),
        "tc": tc.display(t),
        "rate": tc.rate(),
        "rate_fr": tc.rate().map(Rate::label_fr),
        "blocked": tc.blocked,
        "chasing": tc.chasing,
    })
}

/// `GET /api/timecode`.
pub fn view(s: &mut Shared) -> Value {
    sync(s);
    let mut v = summary(s);
    let audio = s.audio_in.status();
    let native = s.audio_in.config().0.source == crate::audio::AudioInputSource::Native;
    v["config"] = json!(s.timecode.config);
    v["midi_ports"] = json!(s.midi.devices.iter().filter(|d| d.connected).map(|d| d.name.as_str()).collect::<Vec<_>>());
    v["audio"] = json!({ "native": native, "channels": audio.channels, "message": audio.message });
    v["rates"] = json!(Rate::ALL.iter().map(|r| json!({ "id": r, "label": r.label_fr() })).collect::<Vec<_>>());
    v
}

/// `POST /api/timecode` `{ "source": "mtc", "offset_s": 3600, … }`: the
/// fields given replace the current ones. Returns whether anything
/// changed; the caller saves. A new source forgets the old one's
/// readings and drops TC-IN.
pub fn update(s: &mut Shared, patch: &Value) -> Result<bool, String> {
    let obj = patch.as_object().ok_or("objet JSON attendu")?;
    let mut v = json!(s.timecode.config);
    for (k, val) in obj {
        if v.get(k).is_none() {
            return Err(format!("réglage inconnu : {k}"));
        }
        v[k] = val.clone();
    }
    let cfg: TcConfig = serde_json::from_value(v).map_err(|e| format!("réglage invalide : {e}"))?;
    cfg.validate()?;
    let before = s.timecode.config.clone();
    if cfg == before {
        return Ok(false);
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
        s.gate.log().record("timecode_settings", Some(ArmSource::Ui.id()), change);
    }
    let new_source = cfg.source != before.source || cfg.midi_port != before.midi_port || cfg.ltc_channel != before.ltc_channel;
    s.timecode.config = cfg;
    if new_source {
        s.timecode.forget();
        if s.timecode.armed {
            disarm(s, ArmSource::Ui, "source");
        }
    }
    Ok(true)
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
    fn a_dropout_freewheels_then_stops_after_a_second() {
        let (mut st, mut player) = ltc_state();
        let t = feed(&mut st, &mut player, 0.0, 1.0, |t| 5.0 + t);
        assert!(player.is_playing() && st.is_chasing());
        // Signal gone: a few frames later, roue libre; the show runs on.
        st.chase(&mut player, &clock(t + 0.2), Ok(()));
        assert_eq!(st.status(), Status::Freewheel);
        assert!(player.is_playing());
        st.chase(&mut player, &clock(t + 0.9), Ok(()));
        assert!(player.is_playing(), "still within the 25 frames");
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
    fn keep_running_continues_without_timecode() {
        let (mut st, mut player) = ltc_state();
        st.config.keep_running = true;
        let t = feed(&mut st, &mut player, 0.0, 1.0, |t| 5.0 + t);
        st.chase(&mut player, &clock(t + 5.0), Ok(()));
        assert_eq!(st.status(), Status::Lost);
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
        let t = feed(&mut st, &mut player, t, t + 0.1, src);
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
        mtc_batch(&mut s, 250, 2);
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
            mtc_batch(&mut s, 400, 2);
            let c = s.timeline_clock();
            frame(&mut s, &c);
            assert!(!s.timeline.is_playing());
        }
        assert!(!s.timecode.armed);
        assert_eq!(s.timecode.blocked.as_deref(), Some("TC-IN désarmé"));
        // Re-armed by hand: it chases again.
        set_armed(&mut s, true, ArmSource::Ui).unwrap();
        mtc_batch(&mut s, 500, 2);
        let c = s.timeline_clock();
        frame(&mut s, &c);
        assert!(s.timeline.is_playing());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn gate_scope_and_laser_disarm_block_it() {
        let mut s = armed_studio();
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        mtc_batch(&mut s, 250, 2);
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
        for bad in [json!({ "offset_s": 1e9 }), json!({ "freewheel_frames": 1 }), json!({ "nope": 1 }), json!({ "source": "smpte" }), json!([1])] {
            assert!(update(&mut s, &bad).is_err(), "{bad}");
        }
        assert_eq!(update(&mut s, &json!({ "offset_s": 3600, "smooth": false })), Ok(true));
        assert_eq!(update(&mut s, &json!({ "offset_s": 3600 })), Ok(false));
        let saved = serde_json::to_string(s.timecode.config()).unwrap();
        assert!(!saved.contains("armed"), "{saved}");
        let back: TcConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(back, TcConfig::default());
    }
}
