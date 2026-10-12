//! Timeline shows: tracks of timed events, a tempo map, and the player
//! that turns the show position into the cues playing this frame.
//!
//! A show has one **time base**:
//! - *Secondes* (a show on a given track): event positions are seconds and
//!   stay put whatever the tempo; beats and bars are derived from the
//!   show's tempo map (changing the BPM of a section moves no event).
//! - *Temps* (a phrase that follows the live tempo): positions are beats of
//!   the one `TempoClock`. Playback starts on the next bar, and a BPM change
//!   speeds the phrase up without a jump, because the clock never jumps.
//!
//! The player never renders anything itself: every frame it hands the
//! engine the cues of its active events (`TimelineCue`), which go through
//! the same layers, mixer, master live modifiers, calibration and output
//! gate as the cues played by hand. It never touches the arm state; an
//! emergency stop halts it (see `main.rs`).

use crate::controls::ControlRegistry;
use crate::engine::Settings;
use crate::evolving::KeyEase;
use crate::layers::LAYER_COUNT;
use crate::live::LiveModifiers;
use crate::presets::Preset;
use crate::tempo::{MAX_BPM, MIN_BPM};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Tempo a cue in *Secondes* mode runs at: its beat-synced motion plays as
/// it would at the default tempo, whatever the show's tempo.
pub const NOMINAL_BPM: f64 = 120.0;
/// Length of a cue's program for *Ajuster* (stretched to the event). Cue
/// programs with their own length arrive with T-157.
pub const FIT_BEATS: f64 = 16.0;
/// Animator ids of timeline events start here, far above the cue deck's
/// instance ids (which count up from 1; 0 is the manual look).
pub const INSTANCE_BASE: u64 = 1 << 62;
const EPS: f64 = 1e-9;
/// Editor limits (T-162), checked by `Show::validate_edit`.
pub const MAX_TRACKS: usize = 64;
pub const MAX_EVENTS: usize = 5000;
pub const MAX_MARKERS: usize = 500;
/// Longest track or marker name, and cue id.
pub const MAX_LABEL: usize = 64;
const MAX_CUE_ID: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBase {
    /// Positions in seconds; beats come from the tempo map.
    #[default]
    Seconds,
    /// Positions in beats of the live tempo clock.
    Beats,
}

/// From `at_s` on, the show runs at `bpm` (Secondes shows only).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TempoPoint {
    pub at_s: f64,
    pub bpm: f32,
    pub beats_per_bar: u8,
}

impl Default for TempoPoint {
    fn default() -> Self {
        Self { at_s: 0.0, bpm: 120.0, beats_per_bar: 4 }
    }
}

/// Largest audio offset, either way (latency compensation, T-161).
pub const MAX_AUDIO_OFFSET_S: f64 = 0.5;

/// The song a Secondes show is locked to (T-161, `audio/playback.rs`): a
/// file of `studio-data/media/audio/`, heard from show time `offset_s`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRef {
    pub file: String,
    /// *Décalage*, ±0.5 s: positive = the song is heard later than the
    /// laser (compensates a laser path slower than the sound).
    pub offset_s: f64,
    /// *Volume*, 0..1.
    pub gain: f32,
    /// Length of the song, noted when it is attached, so the show lasts
    /// until the end of the song.
    pub duration_s: f64,
}

impl Default for AudioRef {
    fn default() -> Self {
        Self { file: String::new(), offset_s: 0.0, gain: 1.0, duration_s: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Events play cues on the track's layer.
    #[default]
    Cues,
    /// Envelopes applied to every track of its layer (evaluated by T-163);
    /// a bus track plays no content.
    Bus,
    /// DMX events (T-453, dmx_out.rs): each sets a named channel of the DMX
    /// output (haze machine, simple lights) for its length. Plays no
    /// content and never touches the laser.
    Dmx,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    /// Layer 1..=4 (layers.rs) its cues play on.
    pub layer: u8,
    pub mute: bool,
    pub solo: bool,
    pub events: Vec<Event>,
    /// Verrou de piste (T-445): the editor leaves its events, name, layer
    /// and kind alone (`Show::check_locks`). Playback ignores it.
    pub locked: bool,
}

impl Default for Track {
    fn default() -> Self {
        Self { name: String::new(), kind: TrackKind::Cues, layer: 1, mute: false, solo: false, events: Vec::new(), locked: false }
    }
}

/// What an event plays.
// A look stored in the event is the common case of a show, and events are
// few: not worth a box (the figure parts of T-441 pushed it over the lint).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventSource {
    /// A cue of the catalogue / grid, by preset id.
    Cue { id: String },
    /// A look stored in the show itself.
    Look { settings: Settings },
    /// A DMX track's event (T-453): the named channel of the DMX output
    /// (`dmx_out.json`) at `value` for the event's length, with its fades.
    Dmx { channel: String, value: u8 },
}

/// Which clock the event's content runs on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeMode {
    /// *Temps musicaux*: beat-synced motion follows the show's beats.
    #[default]
    Beats,
    /// *Secondes*: runs at `NOMINAL_BPM` whatever the tempo.
    Seconds,
    /// *Ajuster*: the cue's program (`FIT_BEATS`) is stretched to the event.
    Fit,
}

/// What happens when the event's length is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndAction {
    /// *Arrêt*: the event ends.
    #[default]
    Stop,
    /// *Garder*: the last frame stays (frozen) until the next event of the track.
    Hold,
    /// *Continuer*: keeps playing until the next event of the track.
    Continue,
}

/// A duration in seconds or in beats, whatever the show's time base.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dur {
    Seconds(f64),
    Beats(f64),
}

impl Default for Dur {
    fn default() -> Self {
        Dur::Seconds(0.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    #[default]
    Cut,
    /// Crossfade: this event fades out over the first `length` of the next.
    Fade,
    /// Fade out before the next event, which fades in: half the length each.
    FadeThroughBlack,
    /// Played as `Fade` until point morphing exists.
    Morph,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transition {
    pub kind: TransitionKind,
    pub length: Dur,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvMode {
    /// *Absolu*: the envelope's value replaces the parameter.
    #[default]
    Absolute,
    /// *Relatif*: the envelope's value multiplies the parameter.
    Relative,
}

/// An envelope node, `at` relative to the event start, in show units,
/// eased towards the next node with the cue programs' easing (T-157).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    pub at: f64,
    pub value: f32,
    /// Named `curve` before T-163 (linear / step / smooth, still read).
    #[serde(alias = "curve")]
    pub ease: KeyEase,
}

/// Envelopes on one event, and nodes on one envelope.
pub const MAX_ENVELOPES: usize = 16;
pub const MAX_ENV_KEYS: usize = crate::program::MAX_KEYS;

/// A parameter automation (T-163): the control `target` (a control id
/// that LFOs may move, `lfo::modulatable`) follows the curve through
/// `keys` while its event plays. On a cue track it acts on the event's
/// look and modifiers; on a *Bus* track, on every event of the layer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Envelope {
    pub target: String,
    pub mode: EnvMode,
    pub keys: Vec<Key>,
}

impl Envelope {
    /// The value `local` show units after the event start (see
    /// `program::curve_at`); `None` without nodes.
    pub fn value_at(&self, local: f64) -> Option<f32> {
        let keys: Vec<(f64, f32, KeyEase)> = self.keys.iter().map(|k| (k.at, k.value, k.ease)).collect();
        crate::program::curve_at(&keys, local)
    }

    /// Finite nodes only, at 0 or later, sorted, at most `MAX_ENV_KEYS`.
    fn sanitize(&mut self) {
        self.keys.retain(|k| k.at.is_finite() && k.value.is_finite());
        for k in &mut self.keys {
            k.at = k.at.max(0.0);
        }
        self.keys.sort_by(|a, b| a.at.total_cmp(&b.at));
        self.keys.truncate(MAX_ENV_KEYS);
    }
}

/// One envelope's value this frame, for the engine (`apply_envelopes`).
#[derive(Clone, Debug, PartialEq)]
pub struct EnvValue {
    pub target: String,
    pub mode: EnvMode,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    /// Unique within the show (fixed up on load).
    pub id: u64,
    /// Start and length in the show's time base (seconds or beats).
    pub start: f64,
    pub len: f64,
    pub source: EventSource,
    pub time: TimeMode,
    /// Where the content starts, in its own beats (e.g. the second half
    /// of an evolving cue).
    pub offset_beats: f64,
    pub end: EndAction,
    pub fade_in: Dur,
    pub fade_out: Dur,
    pub to_next: Option<Transition>,
    /// Cue-level modifiers, applied to this event's points before the mix.
    pub modifiers: LiveModifiers,
    pub envelopes: Vec<Envelope>,
    /// Scan preset (T-415, `scan.rs`); `None` = « Comme le cue ».
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    /// The operator's notes and tags on the event (T-430), as on a cue.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Referenced presets (T-422, pools.rs): replace their families in
    /// `modifiers`, every frame. None in older shows.
    #[serde(skip_serializing_if = "crate::pools::PresetRefs::is_empty")]
    pub refs: crate::pools::PresetRefs,
    /// *Étalement* (T-421, spread.rs): the event drawn once per zone of a
    /// group, shifted from zone to zone. None in older shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<crate::spread::Spread>,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            id: 0,
            start: 0.0,
            len: 1.0,
            source: EventSource::Cue { id: String::new() },
            time: TimeMode::Beats,
            offset_beats: 0.0,
            end: EndAction::Stop,
            fade_in: Dur::default(),
            fade_out: Dur::default(),
            to_next: None,
            modifiers: LiveModifiers::default(),
            envelopes: Vec::new(),
            scan: None,
            notes: String::new(),
            tags: Vec::new(),
            refs: Default::default(),
            spread: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marker {
    pub at: f64,
    pub name: String,
    pub color: [u8; 3],
}

/// A show, saved as `studio-data/shows/<name>.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Show {
    pub name: String,
    pub time_base: TimeBase,
    /// Secondes shows only; empty = 120 BPM in 4/4. A Temps show plays on
    /// the live tempo: there it only notes the tempo it was written for
    /// (the built-in examples' « tempo conseillé »).
    pub tempo_map: Vec<TempoPoint>,
    pub audio: Option<AudioRef>,
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    /// Region played by *Boucle*, in show units; `None` = the whole show.
    pub loop_region: Option<(f64, f64)>,
    /// Its own incoming timecode (T-409): source, rate and offset; `None`
    /// follows the global settings (older shows have none).
    pub timecode: Option<crate::timecode::ShowTimecode>,
}

impl Show {
    /// *Nouveau show*: two cue tracks on layers 1 and 2, 120 BPM in 4/4.
    pub fn new_empty(name: &str, time_base: TimeBase) -> Self {
        let track = |n: u8| Track { name: format!("Piste {n}"), layer: n, ..Default::default() };
        Self {
            name: name.to_string(),
            time_base,
            tempo_map: if time_base == TimeBase::Seconds { vec![TempoPoint::default()] } else { Vec::new() },
            tracks: vec![track(1), track(2)],
            ..Default::default()
        }
    }

    /// Clamp everything into range, sort, and give events unique ids.
    pub fn sanitize(&mut self) {
        let finite = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
        for p in &mut self.tempo_map {
            p.at_s = finite(p.at_s);
            p.bpm = if p.bpm.is_finite() { p.bpm.clamp(MIN_BPM as f32, MAX_BPM as f32) } else { 120.0 };
            p.beats_per_bar = p.beats_per_bar.clamp(1, 16);
        }
        self.tempo_map.sort_by(|a, b| a.at_s.total_cmp(&b.at_s));
        for m in &mut self.markers {
            m.at = finite(m.at);
        }
        self.markers.sort_by(|a, b| a.at.total_cmp(&b.at));
        let mut seen = std::collections::HashSet::new();
        let mut next_id = self.tracks.iter().flat_map(|t| &t.events).map(|e| e.id).max().unwrap_or(0) + 1;
        for track in &mut self.tracks {
            track.layer = track.layer.clamp(1, LAYER_COUNT as u8);
            for e in &mut track.events {
                e.start = finite(e.start);
                e.len = finite(e.len);
                e.offset_beats = finite(e.offset_beats);
                sanitize_modifiers(&mut e.modifiers);
                e.envelopes.truncate(MAX_ENVELOPES);
                e.envelopes.iter_mut().for_each(Envelope::sanitize);
                crate::cues::sanitize_notes(&mut e.notes);
                crate::cues::sanitize_tags(&mut e.tags);
                e.spread = e.spread.take().map(crate::spread::Spread::sanitized).filter(crate::spread::Spread::is_set);
                if let EventSource::Dmx { channel, .. } = &mut e.source {
                    *channel = channel.trim().chars().take(MAX_LABEL).collect();
                }
                if e.id == 0 || !seen.insert(e.id) {
                    e.id = next_id;
                    seen.insert(next_id);
                    next_id += 1;
                }
            }
            track.events.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
        self.loop_region = self.loop_region.filter(|&(a, b)| a.is_finite() && b.is_finite() && a >= 0.0 && b > a);
        if let Some(a) = &mut self.audio {
            a.file = a.file.trim().to_string();
            a.offset_s = if a.offset_s.is_finite() { a.offset_s.clamp(-MAX_AUDIO_OFFSET_S, MAX_AUDIO_OFFSET_S) } else { 0.0 };
            a.gain = if a.gain.is_finite() { a.gain.clamp(0.0, 1.0) } else { 1.0 };
            a.duration_s = finite(a.duration_s);
        }
        if let Some(tc) = &mut self.timecode {
            let max = crate::timecode::MAX_OFFSET_S;
            tc.offset_s = if tc.offset_s.is_finite() { tc.offset_s.clamp(-max, max) } else { 0.0 };
        }
    }

    /// What the editor may send (T-162), checked before it replaces the
    /// loaded show: limits, name lengths, positive lengths, and cues that
    /// exist (`known`). French messages, shown as they are.
    pub fn validate_edit(&self, known: impl Fn(&str) -> bool) -> Result<()> {
        if self.tracks.len() > MAX_TRACKS {
            bail!("trop de pistes ({} au plus)", MAX_TRACKS);
        }
        let events = self.tracks.iter().map(|t| t.events.len()).sum::<usize>();
        if events > MAX_EVENTS {
            bail!("trop d'événements ({} au plus)", MAX_EVENTS);
        }
        if self.markers.len() > MAX_MARKERS {
            bail!("trop de marqueurs ({} au plus)", MAX_MARKERS);
        }
        if let Some(t) = self.tracks.iter().find(|t| t.name.chars().count() > MAX_LABEL) {
            bail!("nom de piste trop long ({} caractères au plus) : {}", MAX_LABEL, t.name.chars().take(20).collect::<String>());
        }
        if self.markers.iter().any(|m| m.name.chars().count() > MAX_LABEL) {
            bail!("nom de marqueur trop long ({} caractères au plus)", MAX_LABEL);
        }
        for (track, e) in self.tracks.iter().flat_map(|t| t.events.iter().map(move |e| (t, e))) {
            if !(e.start.is_finite() && e.len.is_finite() && e.start >= 0.0 && e.len > 0.0) {
                bail!("événement {} : début ou durée invalide", e.id);
            }
            // A bus event plays nothing: its source is never read.
            match (track.kind, &e.source) {
                (TrackKind::Cues, EventSource::Cue { id }) => {
                    if id.is_empty() || id.len() > MAX_CUE_ID || !known(id) {
                        bail!("cue inconnu : {}", id.chars().take(40).collect::<String>());
                    }
                }
                (TrackKind::Cues, EventSource::Dmx { .. }) => bail!("événement {} : un événement DMX va sur une piste DMX", e.id),
                (TrackKind::Dmx, EventSource::Dmx { channel, .. }) => {
                    if channel.trim().is_empty() || channel.chars().count() > MAX_LABEL {
                        bail!("événement {} : canal DMX invalide", e.id);
                    }
                }
                (TrackKind::Dmx, _) => bail!("événement {} : une piste DMX ne reçoit que des événements DMX", e.id),
                _ => {}
            }
            if e.envelopes.len() > MAX_ENVELOPES {
                bail!("événement {} : {} enveloppes au plus", e.id, MAX_ENVELOPES);
            }
            for env in &e.envelopes {
                if env.target.chars().count() > MAX_LABEL {
                    bail!("enveloppe : paramètre trop long ({} caractères au plus)", MAX_LABEL);
                }
                if env.keys.len() > MAX_ENV_KEYS {
                    bail!("enveloppe « {} » : {} nœuds au plus", env.target, MAX_ENV_KEYS);
                }
                if env.keys.iter().any(|k| !k.at.is_finite() || !k.value.is_finite() || k.at < 0.0) {
                    bail!("enveloppe « {} » : nœud invalide", env.target);
                }
            }
        }
        Ok(())
    }

    /// Verrou de piste (T-445): an error if a track locked in `self` (the
    /// show loaded) and still locked in `new` (the edit) changed its events,
    /// name, layer or kind, or is gone. Tracks have no id: they match by
    /// position, so a locked track can't be moved either. Locking and
    /// unlocking are always allowed; *Muet* and *Solo* stay free. An
    /// unlocked track in its place must keep its name: otherwise the
    /// locked one was deleted or moved and the next one took its place.
    pub fn check_locks(&self, new: &Show) -> Result<()> {
        for (i, old) in self.tracks.iter().enumerate().filter(|(_, t)| t.locked) {
            let gone = || anyhow::anyhow!("piste verrouillée « {} » : elle ne peut pas être supprimée ni déplacée", old.name);
            let Some(t) = new.tracks.get(i) else { return Err(gone()) };
            if !t.locked {
                if t.name != old.name {
                    return Err(gone());
                }
                continue;
            }
            if t.events != old.events || t.name != old.name || t.layer != old.layer || t.kind != old.kind {
                bail!("piste verrouillée « {} » : déverrouillez-la pour la modifier", old.name);
            }
        }
        Ok(())
    }

    /// Envelope targets that no envelope may drive (`allowed` false: an
    /// unknown id, or a control outside the modulation allow-list), once
    /// each. They are kept in the file but ignored at playback (T-163).
    pub fn unknown_envelope_targets(&self, allowed: impl Fn(&str) -> bool) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for env in self.tracks.iter().flat_map(|t| &t.events).flat_map(|e| &e.envelopes) {
            if !allowed(&env.target) && !out.contains(&env.target) {
                out.push(env.target.clone());
            }
        }
        out
    }

    /// Cue ids used by the show's events (bus tracks play none).
    pub fn cue_ids(&self) -> std::collections::HashSet<&str> {
        self.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Cues)
            .flat_map(|t| &t.events)
            .filter_map(|e| match &e.source {
                EventSource::Cue { id } => Some(id.as_str()),
                EventSource::Look { .. } | EventSource::Dmx { .. } => None,
            })
            .collect()
    }

    /// The song this show plays: Secondes shows with a file only.
    pub fn song(&self) -> Option<&AudioRef> {
        self.audio.as_ref().filter(|a| self.time_base == TimeBase::Seconds && !a.file.is_empty())
    }

    /// End of the last event, or of the song if it lasts longer, in show units.
    pub fn end(&self) -> f64 {
        let events = self.tracks.iter().flat_map(|t| &t.events).map(|e| e.start + e.len).fold(0.0, f64::max);
        self.song().map_or(events, |a| events.max(a.offset_s + a.duration_s))
    }

    fn tempo_points(&self) -> Vec<TempoPoint> {
        if self.tempo_map.is_empty() {
            vec![TempoPoint::default()]
        } else {
            self.tempo_map.clone()
        }
    }

    /// Walk the tempo map's segments `(from_s, to_s, bpm, beats_per_bar)`;
    /// the first one starts at 0 s and the last one never ends.
    fn segments(&self) -> Vec<(f64, f64, f64, f64)> {
        let pts = self.tempo_points();
        (0..pts.len())
            .map(|i| {
                let from = if i == 0 { 0.0 } else { pts[i].at_s };
                let to = pts.get(i + 1).map_or(f64::INFINITY, |p| p.at_s);
                (from, to, pts[i].bpm as f64, pts[i].beats_per_bar as f64)
            })
            .collect()
    }

    /// Beats since 0 s, through the tempo map.
    pub fn beat_at_s(&self, s: f64) -> f64 {
        let mut beats = 0.0;
        for (from, to, bpm, _) in self.segments() {
            if s < to {
                return beats + (s - from) * bpm / 60.0;
            }
            beats += (to - from) * bpm / 60.0;
        }
        beats
    }

    /// Seconds at a beat: the inverse of `beat_at_s`.
    pub fn s_at_beat(&self, beat: f64) -> f64 {
        let mut beats = 0.0;
        for (from, to, bpm, _) in self.segments() {
            let span = (to - from) * bpm / 60.0;
            if beat < beats + span {
                return from + (beat - beats) * 60.0 / bpm;
            }
            beats += span;
        }
        0.0
    }

    /// Seconds at a bar (0-based, fractional): the inverse of `bar_at_s`.
    pub fn s_at_bar(&self, bar: f64) -> f64 {
        let mut bars = 0.0;
        for (from, to, bpm, bpb) in self.segments() {
            let span = (to - from) * bpm / 60.0 / bpb;
            if bar < bars + span {
                return from + (bar - bars) * bpb * 60.0 / bpm;
            }
            bars += span;
        }
        0.0
    }

    /// Bars since 0 s (fractional, 0-based), with the meter of each section.
    pub fn bar_at_s(&self, s: f64) -> f64 {
        let mut bars = 0.0;
        for (from, to, bpm, bpb) in self.segments() {
            if s < to {
                return bars + (s - from) * bpm / 60.0 / bpb;
            }
            bars += (to - from) * bpm / 60.0 / bpb;
        }
        bars
    }
}

/// The tempo clock at this frame, as the player needs it.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    /// Seconds since startup (`Shared::now_s`).
    pub t: f64,
    /// `TempoClock::beat_at(t)`.
    pub beat: f64,
    pub bpm: f64,
    pub beats_per_bar: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// One active event this frame, for the engine to render like a cue.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineCue {
    /// Animator id: stays the same while the event stays active, so its
    /// motion is continuous (also across a loop jump).
    pub instance: u64,
    pub event: u64,
    pub layer: u8,
    pub source: EventSource,
    /// Fades and transitions, 0..1.
    pub gain: f32,
    /// Beat position of the content (its own clock, see `TimeMode`).
    pub content_beat: f64,
    /// Paused, or held after its end: the animation doesn't advance.
    pub frozen: bool,
    pub modifiers: LiveModifiers,
    /// Envelope values this frame (T-163): the event's own, then the bus
    /// envelopes of its layer, in that order (`apply_envelopes`).
    pub envelopes: Vec<EnvValue>,
    /// Seconds of content played since the event started (held events
    /// stop counting at their end). An animator created mid-event (a seek,
    /// a timecode jump) starts there, as if played from the event's start
    /// (T-409).
    pub elapsed_s: f64,
    /// The event's own scan preset (T-415).
    pub scan: Option<String>,
    /// The event's referenced presets (T-422), resolved by the engine.
    pub refs: crate::pools::PresetRefs,
    /// The event's spread (T-421), else the cue's (resolved by the engine).
    pub spread: Option<crate::spread::Spread>,
}

struct Instance {
    id: u64,
    /// Content seconds, accumulated (Secondes mode in a Temps show).
    content_s: f64,
    last_t: f64,
}

#[derive(Default)]
pub struct Player {
    pub show: Option<Show>,
    pub transport: Transport,
    /// *Boucle*: the loop region (or the whole show) plays again and again.
    pub loop_on: bool,
    /// Show position at `anchor_clock` (while playing).
    anchor: f64,
    /// Seconds (Secondes shows) or tempo-clock beats (Temps shows) at which
    /// the show is at `anchor`. Later than now = waiting for the next bar.
    anchor_clock: f64,
    /// Position when not playing, and the last one computed while playing.
    position: f64,
    instances: HashMap<u64, Instance>,
    next_instance: u64,
    last_active: Vec<u64>,
    /// Counts every discontinuity of the playhead (load, play, pause, stop,
    /// seek, loop wrap, halt), so the song player knows when to re-seek.
    jumps: u64,
    /// Counts every start of playback (`play`), so the DMX output (T-453)
    /// knows the operator started the show again after a disarm.
    plays: u64,
    /// Counts every change of the loaded show (load, edit, song, loop
    /// region, save), so the editor knows when to fetch it again (T-162).
    rev: u64,
    /// Edited since it was loaded or saved.
    modified: bool,
    /// A built-in example (examples.rs) not saved yet: *Enregistrer* asks
    /// for a name, so the copy never overwrites a show by surprise.
    example: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TimelineState {
    pub name: Option<String>,
    pub time_base: TimeBase,
    /// In show units (seconds or beats).
    pub position: f64,
    pub length: f64,
    /// Show beat and bar (fractional, 0-based) at the position.
    pub beat: f64,
    pub bar: f64,
    /// Meter at the position (Temps shows: the tempo clock's).
    pub beats_per_bar: u8,
    pub playing: bool,
    pub paused: bool,
    /// Playing, but waiting for the next bar to start.
    pub waiting: bool,
    #[serde(rename = "loop")]
    pub loop_on: bool,
    pub loop_region: Option<(f64, f64)>,
    /// Ids of the events active in the last frame.
    pub active: Vec<u64>,
    /// The show's song (T-161), if any.
    pub audio: Option<AudioRef>,
    /// See `Player::rev`.
    pub rev: u64,
    /// Edited and not saved yet.
    pub modified: bool,
    /// See `Player::example`.
    pub example: bool,
}

impl Player {
    /// Replace the show; playback stops.
    pub fn load(&mut self, mut show: Show) {
        show.sanitize();
        self.stop();
        self.show = Some(show);
        self.rev += 1;
        self.modified = false;
        self.example = false;
    }

    /// Load a built-in example: a copy in memory only, saved under a new
    /// name (`saved`). Nothing is written, so the project stays unmodified.
    pub fn load_example(&mut self, show: Show) {
        self.load(show);
        self.example = true;
    }

    /// The loaded show's name if it has edits not saved yet (T-433).
    pub fn unsaved(&self) -> Option<&str> {
        self.show.as_ref().filter(|_| self.modified).map(|s| s.name.as_str())
    }

    /// The loaded show is a built-in example not saved yet.
    pub fn is_example(&self) -> bool {
        self.show.is_some() && self.example
    }

    /// A show the studio made (T-167, autoshow.rs): loaded like `load`,
    /// but not on disk yet (*Enregistrer* writes it).
    pub fn load_unsaved(&mut self, show: Show) {
        self.load(show);
        self.modified = true;
    }

    /// The editor's change (T-162): the loaded show's tracks, markers,
    /// tempo map and loop region are replaced; its name, time base, song
    /// and timecode settings (T-409) are kept. Playback carries on where it is: events that stay
    /// under the playhead keep their animation, others start or stop as
    /// if the playhead had reached them. False without a loaded show.
    pub fn edit(&mut self, mut show: Show) -> bool {
        let Some(cur) = self.show.as_mut() else { return false };
        show.name = cur.name.clone();
        show.time_base = cur.time_base;
        show.audio = cur.audio.take();
        show.timecode = cur.timecode.take();
        show.sanitize();
        *cur = show;
        self.rev += 1;
        self.modified = true;
        true
    }

    /// The loaded show changed outside the editor (song, loop region).
    pub fn touch(&mut self) {
        self.rev += 1;
    }

    /// The loaded show was written to disk, maybe under a new name.
    pub fn saved(&mut self) {
        self.rev += 1;
        self.modified = false;
        self.example = false;
    }

    /// See `jumps` on the struct.
    pub fn jumps(&self) -> u64 {
        self.jumps
    }

    /// See `plays` on the struct.
    pub fn plays(&self) -> u64 {
        self.plays
    }

    /// Follow an outside clock (the song, T-161): while a Secondes show
    /// plays, pull the playhead towards `position`. Small errors are taken
    /// up by 10 % a frame (callback jitter is smoothed, a steady drift
    /// leaves no lag worth measuring); beyond `SNAP_S` it jumps there.
    pub fn follow(&mut self, position: f64, c: &Clock) {
        if !self.is_playing() || self.base() != TimeBase::Seconds || !position.is_finite() {
            return;
        }
        const SNAP_S: f64 = 0.1;
        let now = self.position_at(c);
        let err = position - now;
        self.anchor = if err.abs() > SNAP_S { position } else { now + 0.1 * err };
        self.anchor_clock = c.t;
    }

    /// Chase an outside timecode (T-168, timecode/): while a Secondes show
    /// plays, `target` is where it should be now. Further than `jump`
    /// away is a jump of the source: seek there (the events there start
    /// over). Otherwise take up `pull` (0..1) of the error. True on a
    /// jump.
    pub fn chase(&mut self, target: f64, c: &Clock, pull: f64, jump: f64) -> bool {
        if !self.is_playing() || self.base() != TimeBase::Seconds || !target.is_finite() {
            return false;
        }
        let now = self.position_at(c);
        let err = target - now;
        if err.abs() > jump {
            self.seek(target, c);
            return true;
        }
        self.anchor = now + pull.clamp(0.0, 1.0) * err;
        self.anchor_clock = c.t;
        false
    }

    fn base(&self) -> TimeBase {
        self.show.as_ref().map_or(TimeBase::Seconds, |s| s.time_base)
    }

    fn clock_value(&self, c: &Clock) -> f64 {
        match self.base() {
            TimeBase::Seconds => c.t,
            TimeBase::Beats => c.beat,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.transport == Transport::Playing
    }

    /// Show position at `c` (before any loop wrap of this frame).
    pub fn position_at(&self, c: &Clock) -> f64 {
        match self.transport {
            Transport::Playing => self.anchor + (self.clock_value(c) - self.anchor_clock).max(0.0),
            _ => self.position,
        }
    }

    /// Start (or resume) playback. A Temps show starts on the next bar of
    /// the tempo clock (now, if now is on the one). False without a show.
    pub fn play(&mut self, c: &Clock) -> bool {
        let Some(show) = &self.show else { return false };
        if self.is_playing() {
            return true;
        }
        if self.position >= show.end() && !self.loop_on {
            self.position = 0.0;
        }
        self.anchor = self.position;
        self.anchor_clock = match show.time_base {
            TimeBase::Seconds => c.t,
            TimeBase::Beats => {
                let bpb = c.beats_per_bar.max(1) as f64;
                let bars = c.beat / bpb;
                let next = if (bars - bars.round()).abs() < 1e-6 { bars.round() } else { bars.ceil() };
                next * bpb
            }
        };
        self.transport = Transport::Playing;
        self.jumps += 1;
        self.plays += 1;
        true
    }

    pub fn pause(&mut self, c: &Clock) {
        if self.is_playing() {
            self.position = self.position_at(c);
            self.transport = Transport::Paused;
            self.jumps += 1;
        }
    }

    /// Back to the start; nothing plays.
    pub fn stop(&mut self) {
        self.transport = Transport::Stopped;
        self.position = 0.0;
        self.instances.clear();
        self.last_active.clear();
        self.jumps += 1;
    }

    /// Jump to `position` (show units). Playing continues from there at
    /// once; the events there start their content over.
    pub fn seek(&mut self, position: f64, c: &Clock) {
        let p = if position.is_finite() { position.max(0.0) } else { 0.0 };
        self.position = p;
        if self.is_playing() {
            self.anchor = p;
            self.anchor_clock = self.clock_value(c);
        }
        self.instances.clear();
        self.jumps += 1;
    }

    /// The loop region in use: the show's, else the whole show.
    fn loop_bounds(&self) -> Option<(f64, f64)> {
        let show = self.show.as_ref()?;
        let (a, b) = show.loop_region.unwrap_or((0.0, show.end()));
        (b - a > EPS).then_some((a, b))
    }

    /// Move the playhead on: wrap in the loop, or stop at the end.
    fn tick(&mut self, c: &Clock) {
        if !self.is_playing() {
            return;
        }
        let mut pos = self.position_at(c);
        match self.loop_bounds() {
            Some((a, b)) if self.loop_on && pos >= b => {
                // Keep the remainder: the playhead lands where it would
                // have been, without a hiccup.
                let shift = ((pos - a) / (b - a)).floor() * (b - a);
                pos -= shift;
                self.anchor -= shift;
                self.jumps += 1;
            }
            _ => {}
        }
        let end = self.show.as_ref().map_or(0.0, Show::end);
        if !self.loop_on && pos >= end {
            self.stop();
            return;
        }
        self.position = pos;
    }

    /// T-456: a backup machine in standby follows the main machine's
    /// playhead: `transport`, and `position` (show units) as it is at `c`.
    /// A gap over `jump_s` seconds seeks there; a smaller one is taken up
    /// by half each time. Without a show, or with a position that is not a
    /// number, nothing changes.
    pub fn mirror(&mut self, transport: Transport, position: f64, loop_on: bool, c: &Clock, jump_s: f64) {
        if self.show.is_none() || !position.is_finite() {
            return;
        }
        self.loop_on = loop_on;
        let position = position.max(0.0);
        let jump = match self.base() {
            TimeBase::Seconds => jump_s,
            TimeBase::Beats => jump_s * c.bpm.max(1.0) / 60.0,
        };
        match transport {
            Transport::Playing if self.is_playing() => {
                let now = self.position_at(c);
                let err = position - now;
                if err.abs() > jump {
                    self.seek(position, c);
                } else {
                    self.anchor = now + 0.5 * err;
                    self.anchor_clock = self.clock_value(c);
                }
            }
            Transport::Playing => {
                // At once, where the main machine is: never a wait for the bar.
                self.position = position;
                self.anchor = position;
                self.anchor_clock = self.clock_value(c);
                self.transport = Transport::Playing;
                self.instances.clear();
                self.jumps += 1;
                self.plays += 1;
            }
            Transport::Paused => {
                if self.transport != Transport::Paused || (self.position - position).abs() > EPS {
                    self.position = position;
                    self.transport = Transport::Paused;
                    self.jumps += 1;
                }
            }
            Transport::Stopped => {
                if self.transport != Transport::Stopped {
                    self.stop();
                }
            }
        }
    }

    /// Changes since the show was loaded (`rev`), for the backup machine.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Emergency stop: nothing plays any more (not even a frozen frame),
    /// and it never resumes by itself. The position is kept, so *Lecture*
    /// after the reset carries on from there.
    pub fn halt(&mut self, c: &Clock) {
        if self.transport == Transport::Stopped {
            return;
        }
        self.pause(c);
        self.transport = Transport::Stopped;
        self.instances.clear();
        self.last_active.clear();
        self.jumps += 1;
    }

    /// Advance the playhead and list the events to render this frame.
    pub fn frame(&mut self, c: &Clock) -> Vec<TimelineCue> {
        self.tick(c);
        let out = match (&self.show, self.transport) {
            (Some(show), Transport::Playing | Transport::Paused) => {
                let waiting = self.is_playing() && self.clock_value(c) < self.anchor_clock;
                if waiting && self.position <= EPS {
                    // A show launched from the top appears on the bar, not before.
                    Vec::new()
                } else {
                    // Paused, or resuming on the next bar: the frozen frame.
                    active_events(show, self.position, c, waiting || self.transport == Transport::Paused)
                }
            }
            _ => Vec::new(),
        };
        // Instance ids: kept while an event stays active, new when it starts.
        self.instances.retain(|id, _| out.iter().any(|a| a.cue.event == *id));
        let mut cues = Vec::with_capacity(out.len());
        for mut a in out {
            let next = &mut self.next_instance;
            let inst = self.instances.entry(a.cue.event).or_insert_with(|| {
                *next += 1;
                Instance { id: INSTANCE_BASE + *next, content_s: a.seconds_estimate, last_t: c.t }
            });
            if let Some(seconds_mode) = a.accumulate_seconds {
                // Temps show, Secondes content: count real seconds, so a
                // tempo change doesn't make the content jump.
                if !a.cue.frozen {
                    inst.content_s += (c.t - inst.last_t).max(0.0);
                }
                a.cue.content_beat = inst.content_s * NOMINAL_BPM / 60.0 + seconds_mode;
            }
            inst.last_t = c.t;
            a.cue.instance = inst.id;
            cues.push(a.cue);
        }
        self.last_active = cues.iter().map(|c| c.event).collect();
        cues
    }

    /// The DMX tracks' levels at the playhead (T-453), apart from the cues:
    /// never through the layers, never the laser. Paused: the frozen
    /// position's; stopped (or waiting for the bar at the top): none, so
    /// every channel is back at rest. Call after `frame`.
    pub fn dmx(&self, c: &Clock) -> Vec<DmxLevel> {
        let Some(show) = &self.show else { return Vec::new() };
        match self.transport {
            Transport::Stopped => Vec::new(),
            Transport::Playing if self.clock_value(c) < self.anchor_clock && self.position <= EPS => Vec::new(),
            Transport::Playing | Transport::Paused => dmx_levels(show, self.position, c),
        }
    }

    pub fn state(&self, c: &Clock) -> TimelineState {
        let show = self.show.as_ref();
        let base = self.base();
        let position = self.position;
        let (beat, bar, beats_per_bar) = match (show, base) {
            (Some(s), TimeBase::Seconds) => {
                let meter = s.tempo_points().iter().rev().find(|p| p.at_s <= position).map_or(4, |p| p.beats_per_bar);
                (s.beat_at_s(position), s.bar_at_s(position), meter)
            }
            _ => (position, position / c.beats_per_bar.max(1) as f64, c.beats_per_bar),
        };
        TimelineState {
            name: show.map(|s| s.name.clone()),
            time_base: base,
            position,
            length: show.map_or(0.0, Show::end),
            beat,
            bar,
            beats_per_bar,
            playing: self.is_playing(),
            paused: self.transport == Transport::Paused,
            waiting: self.is_playing() && self.clock_value(c) < self.anchor_clock,
            loop_on: self.loop_on,
            loop_region: show.and_then(|s| s.loop_region),
            active: self.last_active.clone(),
            audio: show.and_then(|s| s.song()).cloned(),
            rev: self.rev,
            modified: self.modified,
            example: self.is_example(),
        }
    }
}

struct Active {
    cue: TimelineCue,
    /// Some(offset) when the content beat must come from accumulated seconds.
    accumulate_seconds: Option<f64>,
    seconds_estimate: f64,
}

/// A duration in show units, starting at show position `at`.
fn span(show: &Show, d: Dur, at: f64, c: &Clock) -> f64 {
    let v = match (show.time_base, d) {
        (TimeBase::Seconds, Dur::Seconds(s)) => s,
        (TimeBase::Seconds, Dur::Beats(n)) => show.s_at_beat(show.beat_at_s(at) + n) - at,
        (TimeBase::Beats, Dur::Beats(n)) => n,
        (TimeBase::Beats, Dur::Seconds(s)) => s * c.bpm / 60.0,
    };
    if v.is_finite() {
        v.max(0.0)
    } else {
        0.0
    }
}

/// A duration in show units, ending at show position `end`.
fn span_before(show: &Show, d: Dur, end: f64, c: &Clock) -> f64 {
    match (show.time_base, d) {
        (TimeBase::Seconds, Dur::Beats(n)) => {
            let v = end - show.s_at_beat((show.beat_at_s(end) - n).max(0.0));
            if v.is_finite() {
                v.max(0.0)
            } else {
                0.0
            }
        }
        _ => span(show, d, end, c),
    }
}

/// 0..1 ramp up over `[from, from + len]`.
fn ramp(x: f64, from: f64, len: f64) -> f64 {
    if len <= EPS {
        1.0
    } else {
        ((x - from) / len).clamp(0.0, 1.0)
    }
}

/// Where an event's own length ends (*Garder* and *Continuer* run on to
/// the next event of the track, or the end of the show).
fn base_end(e: &Event, next_start: Option<f64>, show_end: f64) -> f64 {
    match e.end {
        EndAction::Stop => e.start + e.len,
        EndAction::Hold | EndAction::Continue => next_start.unwrap_or(show_end).max(e.start + e.len),
    }
}

/// Values at `pos` of `e`'s envelopes (non-empty ones), appended to `out`.
fn envelope_values(e: &Event, pos: f64, out: &mut Vec<EnvValue>) {
    for env in &e.envelopes {
        if let Some(value) = env.value_at(pos - e.start) {
            out.push(EnvValue { target: env.target.clone(), mode: env.mode, value });
        }
    }
}

/// The envelopes of the unmuted bus tracks at `pos`, per layer (index
/// layer - 1). A bus event's envelopes act from its start to its end
/// (*Garder* / *Continuer*: until the next bus event of the track).
fn bus_envelopes(show: &Show, pos: f64, show_end: f64) -> [Vec<EnvValue>; LAYER_COUNT] {
    let mut out: [Vec<EnvValue>; LAYER_COUNT] = Default::default();
    for track in show.tracks.iter().filter(|t| t.kind == TrackKind::Bus && !t.mute) {
        let layer = (track.layer.clamp(1, LAYER_COUNT as u8) - 1) as usize;
        for (i, e) in track.events.iter().enumerate() {
            let end = base_end(e, track.events.get(i + 1).map(|n| n.start), show_end);
            if pos >= e.start && pos < end {
                envelope_values(e, pos, &mut out[layer]);
            }
        }
    }
    out
}

/// One channel's level from a DMX track (T-453): `value` (0..255) reached
/// at `gain` 1; the output mixes it with the channel's rest value.
#[derive(Clone, Debug, PartialEq)]
pub struct DmxLevel {
    pub channel: String,
    pub value: f32,
    /// Fade in / fade out, 0..1.
    pub gain: f32,
}

/// The DMX events active at `pos` on the unmuted DMX tracks: per track
/// and channel, the last one started wins. An event lasts its length
/// (no *Garder* / *Continuer*).
fn dmx_levels(show: &Show, pos: f64, c: &Clock) -> Vec<DmxLevel> {
    let mut out = Vec::new();
    for track in show.tracks.iter().filter(|t| t.kind == TrackKind::Dmx && !t.mute) {
        let mut mine: Vec<DmxLevel> = Vec::new();
        // Sorted by start (`sanitize`): a later start replaces.
        for e in &track.events {
            let EventSource::Dmx { channel, value } = &e.source else { continue };
            let end = e.start + e.len;
            if pos < e.start || pos >= end {
                continue;
            }
            let fade_in = span(show, e.fade_in, e.start, c);
            let fade_out = span_before(show, e.fade_out, end, c).min(e.len);
            let gain_in = ramp(pos, e.start, fade_in);
            let gain_out = if fade_out > EPS { 1.0 - ramp(pos, end - fade_out, fade_out) } else { 1.0 };
            let level = DmxLevel { channel: channel.clone(), value: *value as f32, gain: gain_in.min(gain_out) as f32 };
            match mine.iter_mut().find(|l| l.channel == *channel) {
                Some(l) => *l = level,
                None => mine.push(level),
            }
        }
        out.extend(mine);
    }
    out
}

/// Every event of an audible cue track active at `pos`.
fn active_events(show: &Show, pos: f64, c: &Clock, frozen_all: bool) -> Vec<Active> {
    let show_end = show.end();
    let any_solo = show.tracks.iter().any(|t| t.solo && t.kind == TrackKind::Cues);
    let bus = bus_envelopes(show, pos, show_end);
    let mut out = Vec::new();
    for track in &show.tracks {
        if track.kind != TrackKind::Cues || track.mute || (any_solo && !track.solo) {
            continue;
        }
        let events = &track.events;
        for (i, e) in events.iter().enumerate() {
            // A DMX event is never drawn (refused on a cue track by the editor).
            if matches!(e.source, EventSource::Dmx { .. }) {
                continue;
            }
            let next_start = events.get(i + 1).map(|n| n.start);
            let base_end = base_end(e, next_start, show_end);
            // Transition to the next event: (effective end, fade-out window).
            let (end, fade_out_from) = match (e.to_next, next_start) {
                (Some(t), Some(ns)) if t.kind != TransitionKind::Cut => {
                    let l = span(show, t.length, ns, c);
                    match t.kind {
                        TransitionKind::FadeThroughBlack => (ns, ns - l / 2.0),
                        _ => (ns + l, ns),
                    }
                }
                _ => (base_end, base_end - span_before(show, e.fade_out, base_end, c)),
            };
            if pos < e.start || pos >= end {
                continue;
            }
            let local = pos - e.start;
            let mut fade_in = span(show, e.fade_in, e.start, c);
            if let Some(prev) = i.checked_sub(1).map(|p| &events[p]) {
                if let Some(t) = prev.to_next.filter(|t| t.kind != TransitionKind::Cut) {
                    let l = span(show, t.length, e.start, c);
                    fade_in = fade_in.max(if t.kind == TransitionKind::FadeThroughBlack { l / 2.0 } else { l });
                }
            }
            let gain_in = ramp(pos, e.start, fade_in);
            let gain_out = if end - fade_out_from > EPS { 1.0 - ramp(pos, fade_out_from, end - fade_out_from) } else { 1.0 };
            let held = e.end == EndAction::Hold && local >= e.len;
            let local_c = if e.end == EndAction::Hold { local.min(e.len) } else { local };
            let fit = if e.len > EPS { local_c / e.len * FIT_BEATS } else { 0.0 };
            let mut accumulate_seconds = None;
            let content = match (show.time_base, e.time) {
                (_, TimeMode::Fit) => fit,
                (TimeBase::Seconds, TimeMode::Beats) => show.beat_at_s(e.start + local_c) - show.beat_at_s(e.start),
                (TimeBase::Seconds, TimeMode::Seconds) => local_c * NOMINAL_BPM / 60.0,
                (TimeBase::Beats, TimeMode::Beats) => local_c,
                (TimeBase::Beats, TimeMode::Seconds) => {
                    accumulate_seconds = Some(e.offset_beats);
                    local_c * 60.0 / c.bpm.max(1.0) * NOMINAL_BPM / 60.0
                }
            };
            let mut envelopes = Vec::new();
            envelope_values(e, pos, &mut envelopes);
            envelopes.extend(bus[(track.layer.clamp(1, LAYER_COUNT as u8) - 1) as usize].iter().cloned());
            out.push(Active {
                cue: TimelineCue {
                    instance: 0,
                    event: e.id,
                    layer: track.layer,
                    source: e.source.clone(),
                    gain: (gain_in.min(gain_out)) as f32,
                    content_beat: content + e.offset_beats,
                    frozen: frozen_all || held,
                    modifiers: e.modifiers.clone(),
                    envelopes,
                    elapsed_s: match show.time_base {
                        TimeBase::Seconds => local_c,
                        TimeBase::Beats => local_c * 60.0 / c.bpm.max(1.0),
                    },
                    scan: e.scan.clone(),
                    refs: e.refs.clone(),
                    spread: e.spread.clone(),
                },
                accumulate_seconds,
                seconds_estimate: local_c * 60.0 / c.bpm.max(1.0),
            });
        }
    }
    out
}

/// Event modifiers into the ranges of the master controls (non-finite →
/// neutral), so no show file or edit can push the geometry far off.
pub(crate) fn sanitize_modifiers(m: &mut LiveModifiers) {
    let d = LiveModifiers::default();
    let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
    m.brightness = fix(m.brightness, 0.0, 1.0, d.brightness);
    m.size = fix(m.size, 0.0, 2.0, d.size);
    m.size_x = fix(m.size_x, -2.0, 2.0, d.size_x);
    m.size_y = fix(m.size_y, -2.0, 2.0, d.size_y);
    m.pos_x = fix(m.pos_x, -1.0, 1.0, d.pos_x);
    m.pos_y = fix(m.pos_y, -1.0, 1.0, d.pos_y);
    for i in 0..3 {
        m.rot_angle[i] = fix(m.rot_angle[i], -3600.0, 3600.0, 0.0);
        m.rot_speed[i] = fix(m.rot_speed[i], -3600.0, 3600.0, 0.0);
    }
    m.perspective = fix(m.perspective, 0.0, 1.0, d.perspective);
    m.speed = fix(m.speed, 0.0, 4.0, d.speed);
    // Live effects (T-142): in range, and never a held « Noir », « Figer »
    // or strobe flash stored in an event or a cue.
    m.sanitize_effects();
    m.release_momentary();
}

/// The look an event draws: its catalogue cue or its own look, with the
/// fade applied to the brightness. `None` for an unknown cue id.
pub fn look_of(cue: &TimelineCue, presets: &[Preset]) -> Option<Settings> {
    let mut settings = match &cue.source {
        EventSource::Cue { id } => presets.iter().find(|p| &p.id == id)?.settings.clone(),
        EventSource::Look { settings } => settings.clone(),
        EventSource::Dmx { .. } => return None,
    };
    settings.brightness = (settings.brightness * cue.gain).clamp(0.0, 1.0);
    Some(settings)
}

/// Applies an event's envelope values (T-163) to the engine's per-frame
/// copies of its look and of its own modifiers, before the master live
/// stage, calibration and the whole safety chain. Only controls LFOs may
/// move (`lfo::modulatable`: never arm, safety, calibration, zones,
/// outputs or caps), through `lfo::envelope`, so a brightness envelope
/// only dims; an unknown or forbidden target is skipped (the editor and
/// the load warn about it). In order: the event's envelopes, then its bus's.
pub fn apply_envelopes(values: &[EnvValue], reg: &ControlRegistry, look: &mut Settings, modifiers: &mut LiveModifiers) {
    let mut recolor = false;
    for v in values {
        let Some(range) = crate::lfo::modulatable(reg, &v.target) else { continue };
        recolor |= crate::lfo::envelope(&v.target, range, v.mode == EnvMode::Relative, v.value, look, modifiers);
    }
    if recolor {
        crate::lfo::recolor_live(modifiers);
    }
}

/// A show as listed by `GET /api/shows`.
#[derive(Clone, Debug, Serialize)]
pub struct ShowInfo {
    pub name: String,
    pub time_base: TimeBase,
    pub length: f64,
    pub tracks: usize,
    /// The show's markers, for the global search (T-429): « Refrain » leads
    /// to its show.
    pub markers: Vec<Marker>,
}

/// A show name that is also a safe file name (already trimmed): letters,
/// digits, spaces, `-` and `_`, 1 to 64 characters.
pub fn valid_show_name(name: &str) -> bool {
    !name.is_empty() && name == name.trim() && name.chars().count() <= 64 && name.chars().all(|c| c.is_alphanumeric() || " -_".contains(c))
}

/// `studio-data/shows/`, one JSON file per show.
pub struct ShowStore {
    dir: PathBuf,
}

impl ShowStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The file of a show. Names are file names: letters, digits, spaces,
    /// `-` and `_` only, so a name can never leave the shows folder.
    fn path(&self, name: &str) -> Result<PathBuf> {
        let name = name.trim();
        if !valid_show_name(name) {
            bail!("nom de show invalide (lettres, chiffres, espaces, - et _ seulement)");
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Every readable show, sorted by name.
    pub fn load_all(&self) -> Vec<Show> {
        self.list().into_iter().filter_map(|info| self.load(&info.name).ok()).collect()
    }

    /// A damaged file is moved to `<data-dir>/quarantine` (T-402): it
    /// would otherwise be left out of a saved project and deleted by the
    /// next project open.
    pub fn load(&self, name: &str) -> Result<Show> {
        let path = self.path(name)?;
        let quarantine = self.dir.parent().unwrap_or(std::path::Path::new(".")).join(crate::datafile::QUARANTINE);
        let mut show: Show = match crate::datafile::load(&path, &quarantine, false) {
            crate::datafile::Loaded::Ok(show) => show,
            crate::datafile::Loaded::Missing => bail!("show introuvable : {}", name.trim()),
            crate::datafile::Loaded::Quarantined(w) => bail!("show illisible : {}", w.message_fr()),
        };
        show.name = name.trim().to_string();
        show.sanitize();
        Ok(show)
    }

    pub fn save(&self, show: &Show) -> Result<()> {
        let path = self.path(&show.name)?;
        std::fs::create_dir_all(&self.dir).with_context(|| format!("failed to create {}", self.dir.display()))?;
        let mut show = show.clone();
        show.name = show.name.trim().to_string();
        show.sanitize();
        crate::datafile::replace(&path, serde_json::to_string_pretty(&show)?.as_bytes()).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn list(&self) -> Vec<ShowInfo> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut list: Vec<ShowInfo> = entries
            .filter_map(|e| {
                let path = e.ok()?.path();
                let name = path.file_stem()?.to_str()?.to_string();
                (path.extension()? == "json").then_some(())?;
                let show = self.load(&name).ok()?;
                Some(ShowInfo { name, time_base: show.time_base, length: show.end(), tracks: show.tracks.len(), markers: show.markers })
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(t: f64) -> Clock {
        Clock { t, beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 }
    }

    fn cue(id: &str) -> EventSource {
        EventSource::Cue { id: id.into() }
    }

    fn event(id: u64, start: f64, len: f64) -> Event {
        Event { id, start, len, source: cue("c"), ..Default::default() }
    }

    fn show(base: TimeBase, events: Vec<Event>) -> Show {
        let mut s = Show { name: "t".into(), time_base: base, tracks: vec![Track { events, ..Default::default() }], ..Default::default() };
        s.sanitize();
        s
    }

    fn player(s: Show) -> Player {
        let mut p = Player::default();
        p.load(s);
        p
    }

    fn active_ids(p: &mut Player, c: &Clock) -> Vec<u64> {
        p.frame(c).iter().map(|c| c.event).collect()
    }

    #[test]
    fn a_seconds_event_is_active_exactly_in_its_window() {
        let mut p = player(show(TimeBase::Seconds, vec![event(1, 4.0, 2.0), event(2, 9.0, 1.0)]));
        p.play(&clock(100.0));
        for (dt, on) in [(0.0, false), (3.999, false), (4.0, true), (5.0, true), (5.999, true), (6.0, false), (7.0, false)] {
            assert_eq!(active_ids(&mut p, &clock(100.0 + dt)) == [1], on, "at {dt} s");
        }
    }

    #[test]
    fn tempo_map_conversion_is_continuous_and_exact() {
        let mut s = show(TimeBase::Seconds, vec![]);
        s.tempo_map = vec![TempoPoint { at_s: 0.0, bpm: 120.0, beats_per_bar: 4 }, TempoPoint { at_s: 30.0, bpm: 140.0, beats_per_bar: 4 }];
        assert!((s.beat_at_s(30.0) - 60.0).abs() < 1e-9);
        assert!((s.beat_at_s(30.0 - 1e-9) - s.beat_at_s(30.0)).abs() < 1e-6, "no jump at the change");
        assert!((s.beat_at_s(60.0) - (60.0 + 70.0)).abs() < 1e-9); // 30 s at 140 = 70 beats
        assert!((s.bar_at_s(60.0) - 130.0 / 4.0).abs() < 1e-9);
        for s_ in [0.0, 12.3, 29.99, 30.0, 30.01, 45.5, 600.0] {
            assert!((s.s_at_beat(s.beat_at_s(s_)) - s_).abs() < 1e-9, "round trip at {s_}");
        }
        // Moving the tempo change moves no event: positions are seconds.
        let mut p = player(Show { tempo_map: s.tempo_map.clone(), ..show(TimeBase::Seconds, vec![event(1, 40.0, 1.0)]) });
        p.play(&clock(0.0));
        assert_eq!(active_ids(&mut p, &clock(40.5)), [1]);
    }

    #[test]
    fn a_beats_show_starts_on_the_next_bar() {
        let mut p = player(show(TimeBase::Beats, vec![event(1, 0.0, 4.0)]));
        // Bar 3.5 (0-based bar 2.5 = beat 10 at 120 BPM → t = 5 s).
        p.play(&clock(5.0));
        assert!(p.state(&clock(5.0)).waiting);
        assert!(active_ids(&mut p, &clock(5.5)).is_empty(), "waits for the bar");
        assert_eq!(p.state(&clock(5.5)).position, 0.0);
        // Beat 12 = bar 4.0 (0-based 3) at t = 6 s.
        assert_eq!(active_ids(&mut p, &clock(6.0)), [1]);
        assert!(!p.state(&clock(6.0)).waiting);
        assert!((p.position_at(&clock(6.25)) - 0.5).abs() < 1e-9);
        // Exactly on a bar: starts at once.
        let mut q = player(show(TimeBase::Beats, vec![event(1, 0.0, 4.0)]));
        q.play(&clock(2.0));
        assert_eq!(active_ids(&mut q, &clock(2.0)), [1]);
    }

    #[test]
    fn a_tempo_change_keeps_a_beats_show_on_the_same_bar() {
        let mut tempo = crate::tempo::TempoClock::default();
        tempo.set_bpm_manual(128.0, 0.0);
        let at = |tempo: &crate::tempo::TempoClock, t: f64| Clock { t, beat: tempo.beat_at(t), bpm: tempo.bpm, beats_per_bar: 4 };
        let mut p = player(show(TimeBase::Beats, vec![event(1, 0.0, 16.0), event(2, 16.0, 16.0)]));
        p.play(&at(&tempo, 0.0));
        let t = 5.0; // beat 10.67: in event 1, bar 2
        let before = p.state(&at(&tempo, t));
        let _ = p.frame(&at(&tempo, t));
        let pos_before = p.position_at(&at(&tempo, t));
        tempo.set_bpm_manual(150.0, t);
        let pos_after = p.position_at(&at(&tempo, t));
        assert!((pos_after - pos_before).abs() < 1e-9, "no jump: {pos_before} → {pos_after}");
        assert_eq!(active_ids(&mut p, &at(&tempo, t)), [1]);
        assert_eq!(before.name.as_deref(), Some("t"));
        // …and it now runs faster: 1 s later is 2.5 beats on, not 2.13.
        assert!((p.position_at(&at(&tempo, t + 1.0)) - pos_before - 2.5).abs() < 1e-9);
    }

    #[test]
    fn the_loop_wraps_without_losing_time() {
        let mut s = show(TimeBase::Seconds, vec![event(1, 0.0, 10.0)]);
        s.loop_region = Some((2.0, 4.0));
        let mut p = player(s);
        p.loop_on = true;
        p.play(&clock(0.0));
        let _ = p.frame(&clock(3.9));
        let id_before = p.frame(&clock(3.95))[0].instance;
        let after = p.frame(&clock(4.05));
        assert!((p.position - 2.05).abs() < 1e-9, "position {}", p.position);
        assert_eq!(after[0].instance, id_before, "the event keeps its animation across the jump");
        // Far past the end (a long frame): still in the region, same phase.
        let _ = p.frame(&clock(9.3));
        assert!((p.position - 3.3).abs() < 1e-9, "position {}", p.position);
        // Without a region the whole show loops.
        let mut q = player(show(TimeBase::Seconds, vec![event(1, 0.0, 2.0)]));
        q.loop_on = true;
        q.play(&clock(0.0));
        let _ = q.frame(&clock(2.5));
        assert!((q.position - 0.5).abs() < 1e-9);
        assert!(q.is_playing());
    }

    #[test]
    fn fades_and_end_actions() {
        let mut e = event(1, 2.0, 4.0);
        e.fade_in = Dur::Seconds(1.0);
        e.fade_out = Dur::Beats(2.0); // 1 s at 120 BPM
        let mut p = player(show(TimeBase::Seconds, vec![e, event(2, 10.0, 1.0)]));
        p.play(&clock(0.0));
        let gain = |p: &mut Player, t: f64| p.frame(&clock(t)).first().map(|c| c.gain);
        assert!((gain(&mut p, 2.5).unwrap() - 0.5).abs() < 1e-6);
        assert!((gain(&mut p, 4.0).unwrap() - 1.0).abs() < 1e-6);
        assert!((gain(&mut p, 5.5).unwrap() - 0.5).abs() < 1e-6);
        assert!(p.frame(&clock(6.0)).is_empty(), "Arrêt: over at its end");

        // Garder: frozen on its last frame until the next event.
        let mut held = event(1, 0.0, 2.0);
        held.end = EndAction::Hold;
        let mut p = player(show(TimeBase::Seconds, vec![held, event(2, 5.0, 1.0)]));
        p.play(&clock(0.0));
        let c = p.frame(&clock(3.0));
        assert_eq!(c.len(), 1);
        assert!(c[0].frozen);
        assert!((c[0].content_beat - 4.0).abs() < 1e-9, "content stops at the end (2 s = 4 beats)");
        assert_eq!(active_ids(&mut p, &clock(5.0)), [2]);

        // Continuer: keeps running past its length.
        let mut cont = event(1, 0.0, 2.0);
        cont.end = EndAction::Continue;
        let mut p = player(show(TimeBase::Seconds, vec![cont, event(2, 5.0, 1.0)]));
        p.play(&clock(0.0));
        let c = p.frame(&clock(3.0));
        assert!(!c[0].frozen);
        assert!((c[0].content_beat - 6.0).abs() < 1e-9);
    }

    #[test]
    fn a_crossfade_overlaps_the_next_event() {
        let mut a = event(1, 0.0, 4.0);
        a.to_next = Some(Transition { kind: TransitionKind::Fade, length: Dur::Seconds(1.0) });
        let mut p = player(show(TimeBase::Seconds, vec![a, event(2, 4.0, 4.0)]));
        p.play(&clock(0.0));
        let c = p.frame(&clock(4.5));
        let g: HashMap<u64, f32> = c.iter().map(|c| (c.event, c.gain)).collect();
        assert!((g[&1] - 0.5).abs() < 1e-6 && (g[&2] - 0.5).abs() < 1e-6, "{g:?}");
        assert_eq!(active_ids(&mut p, &clock(5.0)), [2]);
    }

    #[test]
    fn content_clocks_follow_the_time_mode() {
        let mut s = show(TimeBase::Seconds, vec![]);
        s.tempo_map = vec![TempoPoint { bpm: 60.0, ..Default::default() }];
        let mk = |time, id| Event { time, ..event(id, 0.0, 8.0) };
        s.tracks = vec![Track { events: vec![mk(TimeMode::Beats, 1)], ..Default::default() }, Track {
            events: vec![mk(TimeMode::Seconds, 2)],
            ..Default::default()
        }, Track { events: vec![Event { offset_beats: 1.0, ..mk(TimeMode::Fit, 3) }], ..Default::default() }];
        let mut p = player(s);
        p.play(&clock(0.0));
        let c: HashMap<u64, f64> = p.frame(&clock(2.0)).iter().map(|c| (c.event, c.content_beat)).collect();
        assert!((c[&1] - 2.0).abs() < 1e-9, "60 BPM show: 2 beats");
        assert!((c[&2] - 4.0).abs() < 1e-9, "Secondes: 120 BPM whatever the show");
        assert!((c[&3] - (FIT_BEATS / 4.0 + 1.0)).abs() < 1e-9, "Ajuster: a quarter of the program, plus the offset");
    }

    #[test]
    fn mute_solo_and_bus_tracks() {
        let mut s = show(TimeBase::Seconds, vec![]);
        s.tracks = vec![
            Track { layer: 1, events: vec![event(1, 0.0, 5.0)], ..Default::default() },
            Track { layer: 2, events: vec![event(2, 0.0, 5.0)], ..Default::default() },
            Track { kind: TrackKind::Bus, layer: 2, events: vec![event(3, 0.0, 5.0)], ..Default::default() },
        ];
        let mut p = player(s.clone());
        p.play(&clock(0.0));
        let c = p.frame(&clock(1.0));
        assert_eq!(c.iter().map(|c| (c.event, c.layer)).collect::<Vec<_>>(), [(1, 1), (2, 2)], "bus tracks play no content");
        s.tracks[1].solo = true;
        p.load(s.clone());
        p.play(&clock(0.0));
        assert_eq!(active_ids(&mut p, &clock(1.0)), [2]);
        s.tracks[1].mute = true;
        p.load(s);
        p.play(&clock(0.0));
        assert!(active_ids(&mut p, &clock(1.0)).is_empty(), "mute wins");
    }

    #[test]
    fn pause_freezes_and_stop_rewinds() {
        let mut p = player(show(TimeBase::Seconds, vec![event(1, 0.0, 10.0)]));
        p.play(&clock(0.0));
        let _ = p.frame(&clock(3.0));
        p.pause(&clock(3.0));
        let c = p.frame(&clock(8.0));
        assert!(c[0].frozen);
        assert_eq!(p.position, 3.0);
        p.play(&clock(20.0));
        let _ = p.frame(&clock(21.0));
        assert!((p.position - 4.0).abs() < 1e-9);
        p.seek(7.5, &clock(21.0));
        let _ = p.frame(&clock(21.5));
        assert!((p.position - 8.0).abs() < 1e-9);
        p.stop();
        assert!(p.frame(&clock(22.0)).is_empty());
        assert_eq!(p.position, 0.0);
    }

    #[test]
    fn sixty_seconds_of_playback_at_60_fps() {
        // 30 events of 2 s back to back, then the show ends by itself.
        let events: Vec<Event> = (0..30).map(|i| event(i + 1, i as f64 * 2.0, 2.0)).collect();
        let mut p = player(show(TimeBase::Seconds, events));
        p.play(&clock(1000.0));
        let mut starts = Vec::new();
        let mut last_instance = 0;
        for frame in 0..=3660 {
            let t = 1000.0 + frame as f64 / 60.0;
            let cues = p.frame(&clock(t));
            if t - 1000.0 < 60.0 - 1e-6 {
                assert_eq!(cues.len(), 1, "exactly one event at {t}");
                if cues[0].instance != last_instance {
                    last_instance = cues[0].instance;
                    starts.push(cues[0].event);
                }
            }
        }
        assert_eq!(starts, (1..=30).collect::<Vec<_>>());
        assert!(!p.is_playing(), "stopped at the end");
    }

    #[test]
    fn beats_show_seconds_content_does_not_jump_on_tempo_change() {
        let mut ev = event(1, 0.0, 64.0);
        ev.time = TimeMode::Seconds;
        let mut p = player(show(TimeBase::Beats, vec![ev]));
        let mut tempo = crate::tempo::TempoClock::default();
        let at = |tempo: &crate::tempo::TempoClock, t: f64| Clock { t, beat: tempo.beat_at(t), bpm: tempo.bpm, beats_per_bar: 4 };
        p.play(&at(&tempo, 0.0));
        let _ = p.frame(&at(&tempo, 0.0));
        let a = p.frame(&at(&tempo, 1.0))[0].content_beat;
        tempo.set_bpm_manual(200.0, 1.0);
        let b = p.frame(&at(&tempo, 1.0))[0].content_beat;
        let c = p.frame(&at(&tempo, 2.0))[0].content_beat;
        assert!((a - 2.0).abs() < 1e-9 && (b - a).abs() < 1e-9, "{a} {b}");
        assert!((c - 4.0).abs() < 1e-9, "1 s = 2 nominal beats whatever the tempo: {c}");
    }

    #[test]
    fn sanitize_fixes_ids_layers_and_regions() {
        let mut s = Show {
            tracks: vec![Track { layer: 9, events: vec![event(0, 5.0, -1.0), event(3, 1.0, 1.0), event(3, 2.0, 1.0)], ..Default::default() }],
            tempo_map: vec![TempoPoint { at_s: 10.0, bpm: 999.0, beats_per_bar: 0 }, TempoPoint::default()],
            loop_region: Some((4.0, 2.0)),
            ..Default::default()
        };
        s.sanitize();
        let t = &s.tracks[0];
        assert_eq!(t.layer, 4);
        let ids: std::collections::HashSet<u64> = t.events.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(t.events.windows(2).all(|w| w[0].start <= w[1].start));
        assert_eq!(t.events[2].len, 0.0);
        assert_eq!(s.tempo_map[0].at_s, 0.0);
        assert_eq!((s.tempo_map[1].bpm, s.tempo_map[1].beats_per_bar), (MAX_BPM as f32, 1));
        assert_eq!(s.loop_region, None);
    }

    #[test]
    fn event_notes_and_tags_load_empty_and_are_sanitized() {
        let old: Event = serde_json::from_str(r#"{"start":1,"len":2}"#).unwrap();
        assert!(old.notes.is_empty() && old.tags.is_empty());
        assert!(!serde_json::to_string(&old).unwrap().contains("tags"));
        let mut e = event(1, 0.0, 1.0);
        e.notes = "fumée".repeat(200);
        e.tags = vec!["Drop".into(), "drop".into(), "".into()];
        let mut s = show(TimeBase::Beats, vec![e]);
        s.sanitize();
        let e = &s.tracks[0].events[0];
        assert_eq!(e.notes.chars().count(), crate::cues::MAX_NOTES_LEN);
        assert_eq!(e.tags, ["Drop"]);
    }

    #[test]
    fn shows_round_trip_through_the_store_and_names_stay_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("laser-studio-shows-{}", std::process::id()));
        let store = ShowStore::new(dir.clone());
        let mut s = show(TimeBase::Beats, vec![event(1, 0.0, 4.0)]);
        s.name = "Drop 16".into();
        s.markers = vec![Marker { at: 4.0, name: "drop".into(), color: [255, 0, 0] }];
        store.save(&s).unwrap();
        assert_eq!(store.load("Drop 16").unwrap(), s);
        let list = store.list();
        assert_eq!((list[0].name.as_str(), list[0].time_base, list[0].length), ("Drop 16", TimeBase::Beats, 4.0));
        assert_eq!(list[0].markers, s.markers, "markers listed for the search (T-429)");
        for bad in ["", "../x", "a/b", "a.b", "..", "x\\y"] {
            assert!(store.load(bad).is_err() && store.save(&Show { name: bad.into(), ..Default::default() }).is_err(), "{bad:?}");
        }
        // An old or partial file loads with defaults.
        std::fs::write(dir.join("min.json"), r#"{"tracks":[{"events":[{"start":1,"len":2,"source":{"kind":"cue","id":"x"}}]}]}"#).unwrap();
        let min = store.load("min").unwrap();
        assert_eq!((min.time_base, min.tracks[0].layer, min.tracks[0].events[0].id), (TimeBase::Seconds, 1, 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A random Secondes show: a tempo map, tracks of events with fades,
    /// transitions, end actions, time modes and envelopes.
    fn random_show(rng: &mut crate::safety_invariants::Rng) -> Show {
        let dur = |rng: &mut crate::safety_invariants::Rng| if rng.chance(0.5) { Dur::Seconds(rng.range(0.0, 2.0) as f64) } else { Dur::Beats(rng.range(0.0, 4.0) as f64) };
        let tempo_map = (0..1 + rng.below(3)).map(|i| TempoPoint { at_s: if i == 0 { 0.0 } else { rng.range(1.0, 30.0) as f64 }, bpm: rng.range(70.0, 180.0), beats_per_bar: 3 + rng.below(2) as u8 }).collect();
        let mut id = 0;
        let tracks = (0..1 + rng.below(3))
            .map(|k| {
                let events = (0..1 + rng.below(6))
                    .map(|_| {
                        id += 1;
                        Event {
                            id,
                            start: rng.range(0.0, 30.0) as f64,
                            len: rng.range(0.5, 8.0) as f64,
                            source: cue("c"),
                            time: *rng.pick(&[TimeMode::Beats, TimeMode::Seconds, TimeMode::Fit]),
                            offset_beats: rng.range(0.0, 2.0) as f64,
                            end: *rng.pick(&[EndAction::Stop, EndAction::Hold, EndAction::Continue]),
                            fade_in: dur(rng),
                            fade_out: dur(rng),
                            to_next: rng.chance(0.5).then(|| Transition { kind: *rng.pick(&[TransitionKind::Cut, TransitionKind::Fade, TransitionKind::FadeThroughBlack]), length: dur(rng) }),
                            envelopes: vec![Envelope { target: "master.size".into(), mode: EnvMode::Absolute, keys: vec![Key { at: 0.0, value: rng.f(), ..Default::default() }, Key { at: rng.range(0.5, 6.0) as f64, value: rng.f(), ..Default::default() }] }],
                            ..Default::default()
                        }
                    })
                    .collect();
                Track { name: format!("Piste {k}"), layer: 1 + k as u8, events, ..Default::default() }
            })
            .collect();
        let mut s = Show { name: "p".into(), time_base: TimeBase::Seconds, tempo_map, tracks, ..Default::default() };
        s.sanitize();
        s
    }

    /// What a frame draws, without the animator ids (new after a seek).
    fn drawn(mut cues: Vec<TimelineCue>) -> Vec<TimelineCue> {
        cues.iter_mut().for_each(|c| c.instance = 0);
        cues.sort_by_key(|c| c.event);
        cues
    }

    /// T-409: seeking to `t` (a timecode jump) gives the frame playing
    /// from 0 to `t` gives: same events, fades, content clock, envelopes
    /// and time elapsed in each event (the animators start from it).
    #[test]
    fn seeking_to_t_draws_what_playing_to_t_draws() {
        let mut rng = crate::safety_invariants::Rng::new(409);
        let mut checked = 0;
        for _ in 0..40 {
            let s = random_show(&mut rng);
            let end = s.end();
            let mut played = player(s.clone());
            played.play(&clock(0.0));
            let mut t = 0.0;
            for _ in 0..3 {
                let target = t + rng.range(0.1, 12.0) as f64;
                if target >= end {
                    break;
                }
                while t + 1.0 / 60.0 < target {
                    t += 1.0 / 60.0;
                    played.frame(&clock(t));
                }
                t = target;
                let a = drawn(played.frame(&clock(t)));
                // Another player, another clock: seek there, then play.
                let mut sought = player(s.clone());
                let c = clock(1000.0 + t);
                sought.seek(t, &c);
                sought.play(&c);
                let b = drawn(sought.frame(&c));
                assert_eq!(a.len(), b.len(), "t = {t}");
                for (x, y) in a.iter().zip(&b) {
                    assert_eq!((x.event, x.layer, x.frozen, &x.source, &x.modifiers), (y.event, y.layer, y.frozen, &y.source, &y.modifiers));
                    assert!((x.gain - y.gain).abs() < 1e-4, "gain at {t}: {} vs {}", x.gain, y.gain);
                    assert!((x.content_beat - y.content_beat).abs() < 1e-4, "content at {t}");
                    assert!((x.elapsed_s - y.elapsed_s).abs() < 1e-4, "elapsed at {t}");
                    assert_eq!(x.envelopes.len(), y.envelopes.len());
                    for (e, f) in x.envelopes.iter().zip(&y.envelopes) {
                        assert!(e.target == f.target && (e.value - f.value).abs() < 1e-4, "envelope at {t}");
                    }
                }
                checked += a.len();
            }
        }
        assert!(checked > 20, "{checked} events compared");
    }

    #[test]
    fn look_of_applies_the_fade_and_skips_unknown_cues() {
        let presets = crate::presets::catalog();
        let mut c = TimelineCue {
            instance: 1,
            event: 1,
            layer: 1,
            source: cue(&presets[0].id),
            gain: 0.5,
            content_beat: 0.0,
            frozen: false,
            modifiers: LiveModifiers::default(),
            envelopes: Vec::new(),
            elapsed_s: 0.0,
            scan: None,
            refs: Default::default(),
            spread: None,
        };
        let s = look_of(&c, &presets).unwrap();
        assert!((s.brightness - presets[0].settings.brightness * 0.5).abs() < 1e-6);
        c.source = cue("no-such-cue");
        assert!(look_of(&c, &presets).is_none());
    }

    #[test]
    fn a_song_sets_the_length_and_is_sanitized() {
        let mut s = show(TimeBase::Seconds, vec![event(1, 0.0, 10.0)]);
        s.audio = Some(AudioRef { file: " chanson.wav ".into(), offset_s: 3.0, gain: 7.0, duration_s: 12.0 });
        s.sanitize();
        let a = s.song().unwrap();
        assert_eq!((a.file.as_str(), a.offset_s, a.gain), ("chanson.wav", MAX_AUDIO_OFFSET_S, 1.0));
        assert_eq!(s.end(), 12.5, "until the end of the song");
        s.audio.as_mut().unwrap().offset_s = f64::NAN;
        s.sanitize();
        assert_eq!(s.end(), 12.0);
        // Old show files without the new fields still load.
        let old: Show = serde_json::from_str(r#"{"name":"x","audio":{"file":"a.wav","offset_s":0.1}}"#).unwrap();
        assert_eq!(old.audio.unwrap().gain, 1.0);
        // No song for a Temps show, or with no file.
        s.time_base = TimeBase::Beats;
        assert!(s.song().is_none());
        assert_eq!(s.end(), 10.0);
        s.time_base = TimeBase::Seconds;
        s.audio.as_mut().unwrap().file.clear();
        assert!(s.song().is_none());
    }

    #[test]
    fn follow_pulls_the_playhead_and_jumps_are_counted() {
        let mut p = player(show(TimeBase::Seconds, vec![event(1, 0.0, 100.0)]));
        let j0 = p.jumps();
        p.follow(5.0, &clock(0.0));
        assert_eq!(p.position_at(&clock(0.0)), 0.0, "not playing: ignored");
        p.play(&clock(0.0));
        assert_eq!(p.jumps(), j0 + 1);
        // A small error is taken up by 10 % a frame, a big one at once.
        p.follow(1.01, &clock(1.0));
        assert!((p.position_at(&clock(1.0)) - 1.001).abs() < 1e-9);
        p.follow(3.0, &clock(1.0));
        assert_eq!(p.position_at(&clock(1.0)), 3.0);
        assert_eq!(p.position_at(&clock(1.5)), 3.5, "runs on from there");
        p.follow(f64::NAN, &clock(1.5));
        assert_eq!(p.position_at(&clock(1.5)), 3.5);
        let j = p.jumps();
        p.seek(10.0, &clock(2.0));
        p.pause(&clock(2.1));
        p.stop();
        assert_eq!(p.jumps(), j + 3);
        // The loop wrap is a jump too (the song seeks back with it).
        let mut p = player(show(TimeBase::Seconds, vec![event(1, 0.0, 4.0)]));
        p.loop_on = true;
        p.play(&clock(0.0));
        let j = p.jumps();
        p.frame(&clock(3.0));
        assert_eq!(p.jumps(), j);
        p.frame(&clock(5.0));
        assert_eq!(p.jumps(), j + 1);
        // A Temps show never follows a song.
        let mut p = player(show(TimeBase::Beats, vec![event(1, 0.0, 100.0)]));
        p.play(&clock(0.0));
        let at = p.position_at(&clock(1.0));
        p.follow(50.0, &clock(1.0));
        assert_eq!(p.position_at(&clock(1.0)), at);
    }

    #[test]
    fn an_edit_keeps_playback_and_the_show_identity() {
        let mut s = show(TimeBase::Seconds, vec![event(1, 0.0, 10.0), event(2, 20.0, 5.0)]);
        s.audio = Some(AudioRef { file: "a.wav".into(), duration_s: 30.0, ..Default::default() });
        let mut p = player(s);
        let rev = p.state(&clock(0.0)).rev;
        p.play(&clock(100.0));
        let before = p.frame(&clock(102.0));
        assert_eq!(before.len(), 1);
        let jumps = p.jumps();

        // The editor moves event 1 (still under the playhead) and event 2, and
        // tries to rename the show, change its base and drop its song.
        let mut edited = p.show.clone().unwrap();
        edited.name = "autre".into();
        edited.time_base = TimeBase::Beats;
        edited.audio = None;
        edited.tracks[0].events[0].len = 12.0;
        edited.tracks[0].events[1].start = 1.0;
        edited.markers = vec![Marker { at: 8.0, name: "Refrain".into(), color: [255, 0, 0] }, Marker { at: 4.0, ..Default::default() }];
        assert!(p.edit(edited));

        let st = p.state(&clock(102.5));
        assert!(st.playing && st.modified);
        assert_eq!(st.rev, rev + 1);
        assert_eq!(p.jumps(), jumps, "an edit is no playhead jump: the song carries on");
        let sh = p.show.as_ref().unwrap();
        assert_eq!((sh.name.as_str(), sh.time_base), ("t", TimeBase::Seconds));
        assert_eq!(sh.audio.as_ref().unwrap().file, "a.wav");
        assert_eq!(sh.markers[0].at, 4.0, "markers sorted");
        let after = p.frame(&clock(102.5));
        let ids: Vec<u64> = after.iter().map(|c| c.event).collect();
        assert_eq!(ids, vec![1, 2], "event 2 now starts under the playhead");
        assert_eq!(after[0].instance, before[0].instance, "event 1 keeps its animation");
        assert!(p.state(&clock(102.5)).position > 2.0);

        p.saved();
        assert!(!p.state(&clock(103.0)).modified);
        let mut empty = Player::default();
        assert!(!empty.edit(Show::default()), "nothing loaded");
    }

    #[test]
    fn edits_are_validated() {
        let known = |id: &str| id == "c";
        let ok = show(TimeBase::Seconds, vec![event(1, 0.0, 1.0)]);
        assert!(ok.validate_edit(known).is_ok());
        let mut bad = ok.clone();
        bad.tracks[0].events[0].source = cue("inconnu");
        assert!(bad.validate_edit(known).unwrap_err().to_string().contains("cue inconnu"));
        let mut bad = ok.clone();
        bad.tracks[0].events[0].len = 0.0;
        assert!(bad.validate_edit(known).is_err());
        let mut bad = ok.clone();
        bad.tracks[0].events[0].start = -1.0;
        assert!(bad.validate_edit(known).is_err());
        let mut bad = ok.clone();
        bad.tracks = vec![Track::default(); MAX_TRACKS + 1];
        assert!(bad.validate_edit(known).is_err());
        let mut bad = ok.clone();
        bad.tracks[0].events = (0..MAX_EVENTS as u64 + 1).map(|i| event(i + 1, i as f64, 1.0)).collect();
        assert!(bad.validate_edit(known).unwrap_err().to_string().contains("trop d'événements"));
        let mut bad = ok.clone();
        bad.markers = vec![Marker::default(); MAX_MARKERS + 1];
        assert!(bad.validate_edit(known).is_err());
        let mut bad = ok.clone();
        bad.tracks[0].name = "x".repeat(MAX_LABEL + 1);
        assert!(bad.validate_edit(known).is_err());
        let mut bad = ok;
        bad.markers = vec![Marker { name: "é".repeat(MAX_LABEL + 1), ..Default::default() }];
        assert!(bad.validate_edit(known).is_err());
    }

    /// T-445: a locked track (before and after the edit) keeps its events,
    /// name, layer and kind, and stays where it is; locking, unlocking,
    /// Muet and Solo are always allowed.
    #[test]
    fn locked_tracks_refuse_edits() {
        let mut cur = show(TimeBase::Seconds, vec![event(1, 0.0, 1.0), event(2, 2.0, 1.0)]);
        cur.tracks.push(Track { name: "libre".into(), events: vec![event(3, 0.0, 1.0)], ..Default::default() });
        cur.tracks[0].name = "verrou".into();
        cur.tracks[0].locked = true;
        assert!(cur.check_locks(&cur).is_ok(), "no change");

        let mut moved = cur.clone();
        moved.tracks[0].events[0].start = 0.004;
        assert!(moved.validate_edit(|_| true).is_ok());
        assert!(cur.check_locks(&moved).unwrap_err().to_string().contains("piste verrouillée « verrou »"));
        let mut removed = cur.clone();
        removed.tracks[0].events.pop();
        assert!(cur.check_locks(&removed).is_err(), "event deleted");
        let mut added = cur.clone();
        added.tracks[0].events.push(event(9, 8.0, 1.0));
        assert!(cur.check_locks(&added).is_err(), "event added");
        let mut renamed = cur.clone();
        renamed.tracks[0].name = "autre".into();
        assert!(cur.check_locks(&renamed).is_err());
        let mut layer = cur.clone();
        layer.tracks[0].layer = 2;
        assert!(cur.check_locks(&layer).is_err());
        let mut gone = cur.clone();
        gone.tracks.remove(0);
        assert!(cur.check_locks(&gone).is_err(), "track removed (the free one moved up)");
        gone.tracks[0].locked = true;
        assert!(cur.check_locks(&gone).is_err(), "track removed (the next one, locked, moved up)");
        let mut none = cur.clone();
        none.tracks.clear();
        assert!(cur.check_locks(&none).unwrap_err().to_string().contains("supprimée"));
        let mut swapped = cur.clone();
        swapped.tracks.swap(0, 1);
        swapped.tracks[0].locked = true;
        assert!(cur.check_locks(&swapped).is_err(), "a locked track can't be moved");

        // Free: the other track, Muet / Solo, locking and unlocking.
        let mut free = cur.clone();
        free.tracks[1].events[0].start = 5.0;
        free.tracks[1].name = "renommée".into();
        free.tracks[0].mute = true;
        free.tracks[0].solo = true;
        assert!(cur.check_locks(&free).is_ok());
        let mut unlocked = cur.clone();
        unlocked.tracks[0].locked = false;
        unlocked.tracks[0].events.clear();
        assert!(cur.check_locks(&unlocked).is_ok(), "unlocked in the same edit");
        let mut lock_all = cur.clone();
        lock_all.tracks[1].locked = true;
        lock_all.tracks[1].events.clear();
        assert!(cur.check_locks(&lock_all).is_ok(), "locked by this edit: nothing to compare yet");
        assert!(lock_all.check_locks(&cur).is_ok(), "unlocking is always allowed");
    }

    /// T-445: a show saved before the lock existed loads with every track
    /// free; the lock is saved and read back; playback ignores it.
    #[test]
    fn old_shows_load_unlocked() {
        let old = r#"{"name":"vieux","tracks":[{"name":"A","layer":2,"events":[{"id":1,"start":0,"len":4,"source":{"kind":"cue","id":"c"}}]},{"name":"B"}]}"#;
        let s: Show = serde_json::from_str(old).unwrap();
        assert!(s.tracks.iter().all(|t| !t.locked));
        let mut locked = s.clone();
        locked.tracks[0].locked = true;
        let back: Show = serde_json::from_str(&serde_json::to_string(&locked).unwrap()).unwrap();
        assert!(back.tracks[0].locked && !back.tracks[1].locked);
        let (mut a, mut b) = (player(s), player(back));
        a.play(&clock(100.0));
        b.play(&clock(100.0));
        let (fa, fb) = (a.frame(&clock(101.0)), b.frame(&clock(101.0)));
        assert_eq!(fa.len(), 1);
        assert_eq!(fa.iter().map(|c| c.event).collect::<Vec<_>>(), fb.iter().map(|c| c.event).collect::<Vec<_>>(), "a lock changes nothing at playback");
    }

    #[test]
    fn event_modifiers_are_kept_in_range() {
        let mut e = event(1, 0.0, 1.0);
        e.modifiers.size = 1e30;
        e.modifiers.pos_x = -7.0;
        e.modifiers.rot_speed = [f32::NAN, f32::INFINITY, 10.0];
        e.modifiers.brightness = f32::NAN;
        let s = show(TimeBase::Seconds, vec![e]);
        let m = &s.tracks[0].events[0].modifiers;
        assert_eq!((m.size, m.pos_x, m.rot_speed, m.brightness), (2.0, -1.0, [0.0, 0.0, 10.0], 1.0));
    }

    #[test]
    fn a_new_show_has_two_tracks_and_a_tempo() {
        let s = Show::new_empty("Nouveau", TimeBase::Seconds);
        assert_eq!(s.tracks.iter().map(|t| t.layer).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(s.tempo_map, vec![TempoPoint::default()]);
        assert!(Show::new_empty("P", TimeBase::Beats).tempo_map.is_empty());
    }

    #[test]
    fn a_damaged_show_is_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("shows", "shows/Mon show.json");
        let store = ShowStore::new(path.parent().unwrap().to_path_buf());
        assert!(store.list().is_empty(), "left out, and moved aside");
        assert!(store.load("Mon show").is_err());
        store.save(&Show { name: "Mon show".into(), ..Default::default() }).unwrap();
        crate::datafile::tests::check_kept(&dir, "shows/Mon show.json", false);
    }

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&crate::presets::catalog())
    }

    fn env(target: &str, mode: EnvMode, keys: &[(f64, f32)]) -> Envelope {
        Envelope { target: target.into(), mode, keys: keys.iter().map(|&(at, value)| Key { at, value, ease: KeyEase::Linear }).collect() }
    }

    /// The look and modifiers of the first event playing at `t`, through its envelopes.
    fn rendered(p: &mut Player, t: f64, base: &Settings) -> Vec<(u64, Settings, LiveModifiers)> {
        p.frame(&clock(t))
            .into_iter()
            .map(|cue| {
                let (mut look, mut m) = (base.clone(), cue.modifiers.clone());
                apply_envelopes(&cue.envelopes, &reg(), &mut look, &mut m);
                (cue.event, look, m)
            })
            .collect()
    }

    #[test]
    fn a_brightness_envelope_one_to_zero_over_four_beats_is_half_at_two() {
        let mut e = event(1, 0.0, 8.0);
        e.envelopes = vec![env("look.brightness", EnvMode::Absolute, &[(0.0, 1.0), (4.0, 0.0)])];
        let mut p = player(show(TimeBase::Beats, vec![e]));
        p.play(&clock(0.0));
        let base = Settings { brightness: 1.0, ..Default::default() };
        // clock(t): 2 beats a second.
        for (t, want) in [(0.0, 1.0), (0.5, 0.75), (1.0, 0.5), (2.0, 0.0), (3.0, 0.0)] {
            let got = rendered(&mut p, t, &base)[0].1.brightness;
            assert!((got - want).abs() < 1e-5, "beat {}: {got}", t * 2.0);
        }
        // Stopped: no event, no envelope.
        p.stop();
        assert!(p.frame(&clock(4.0)).is_empty());
    }

    #[test]
    fn absolute_and_relative_modes_and_eases() {
        let base = Settings { scale: 0.8, brightness: 0.6, ..Default::default() };
        let one = |envelope: Envelope| {
            let mut e = event(1, 0.0, 10.0);
            e.envelopes = vec![envelope];
            let mut p = player(show(TimeBase::Seconds, vec![e]));
            p.play(&clock(0.0));
            rendered(&mut p, 1.0, &base).remove(0)
        };
        let (_, look, _) = one(env("look.size", EnvMode::Absolute, &[(0.0, 0.5)]));
        assert!((look.scale - 0.5).abs() < 1e-6, "absolute replaces");
        let (_, look, _) = one(env("look.size", EnvMode::Relative, &[(0.0, 0.5)]));
        assert!((look.scale - 0.4).abs() < 1e-6, "relative multiplies");
        // Brightness only dims, in both modes.
        let (_, look, _) = one(env("look.brightness", EnvMode::Absolute, &[(0.0, 1.0)]));
        assert!((look.brightness - 0.6).abs() < 1e-6);
        let (_, look, _) = one(env("look.brightness", EnvMode::Relative, &[(0.0, 3.0)]));
        assert!((look.brightness - 0.6).abs() < 1e-6);
        let (_, _, m) = one(env("master.brightness", EnvMode::Relative, &[(0.0, 0.5)]));
        assert!((m.brightness - 0.5).abs() < 1e-6, "an event's own dimmer");
        // Out of range: clamped to the control's range.
        let (_, _, m) = one(env("master.size", EnvMode::Absolute, &[(0.0, 50.0)]));
        assert!(m.size <= 2.0);
        // The cue programs' easing: a step holds until the next node.
        let mut stepped = env("look.size", EnvMode::Absolute, &[(0.0, 0.2), (2.0, 1.0)]);
        stepped.keys[0].ease = KeyEase::Step;
        let (_, look, _) = one(stepped);
        assert!((look.scale - 0.2).abs() < 1e-6);
    }

    #[test]
    fn a_bus_envelope_acts_on_every_track_of_its_layer() {
        let mut s = show(TimeBase::Seconds, vec![event(1, 0.0, 10.0)]);
        s.tracks[0].layer = 2;
        s.tracks.push(Track { layer: 2, events: vec![event(2, 0.0, 10.0)], ..Default::default() });
        s.tracks.push(Track { layer: 1, events: vec![event(3, 0.0, 10.0)], ..Default::default() });
        let mut bus_event = Event { id: 4, start: 0.0, len: 4.0, ..Default::default() };
        bus_event.envelopes = vec![env("master.brightness", EnvMode::Absolute, &[(0.0, 0.25)])];
        s.tracks.push(Track { kind: TrackKind::Bus, layer: 2, events: vec![bus_event], ..Default::default() });
        s.sanitize();
        assert!(s.validate_edit(|id| id == "c").is_ok(), "a bus event needs no cue");
        assert!(!s.cue_ids().contains(""));
        let mut p = player(s.clone());
        p.play(&clock(0.0));
        let base = Settings::default();
        let mut got: Vec<(u64, f32)> = rendered(&mut p, 1.0, &base).into_iter().map(|(id, _, m)| (id, m.brightness)).collect();
        got.sort_by_key(|g| g.0);
        assert_eq!(got, vec![(1, 0.25), (2, 0.25), (3, 1.0)], "layer 2 only; the bus plays nothing itself");
        // After the bus event: back to the event's own value.
        assert!(rendered(&mut p, 5.0, &base).iter().all(|(_, _, m)| m.brightness == 1.0));
        // A muted bus does nothing.
        s.tracks[3].mute = true;
        let mut p = player(s);
        p.play(&clock(0.0));
        assert!(rendered(&mut p, 1.0, &base).iter().all(|(_, _, m)| m.brightness == 1.0));
    }

    #[test]
    fn an_unknown_or_forbidden_target_is_ignored_without_panic() {
        let mut e = event(1, 0.0, 10.0);
        e.envelopes = vec![
            env("nope.nothing", EnvMode::Absolute, &[(0.0, 1.0)]),
            env("transport.arm", EnvMode::Absolute, &[(0.0, 1.0)]),
            env("tempo.bpm", EnvMode::Absolute, &[(0.0, 200.0)]),
            env("layer.1.dimmer", EnvMode::Absolute, &[(0.0, 0.0)]),
            env("look.size", EnvMode::Absolute, &[(0.0, 0.5)]),
        ];
        let s = show(TimeBase::Seconds, vec![e]);
        let r = reg();
        let unknown = s.unknown_envelope_targets(|id| crate::lfo::modulatable(&r, id).is_some());
        assert_eq!(unknown, vec!["nope.nothing", "transport.arm", "tempo.bpm", "layer.1.dimmer"]);
        let mut p = player(s);
        p.play(&clock(0.0));
        let (_, look, m) = rendered(&mut p, 1.0, &Settings::default()).remove(0);
        assert!((look.scale - 0.5).abs() < 1e-6, "the allowed one still plays");
        assert_eq!(m, LiveModifiers::default());
    }

    #[test]
    fn envelopes_are_sanitized_validated_and_old_files_still_load() {
        // Before T-163 a node had `curve` (linear / step / smooth).
        let old = r#"{"tracks":[{"events":[{"id":1,"start":0,"len":4,"source":{"kind":"cue","id":"c"},
            "envelopes":[{"target":"look.size","mode":"relative","keys":[{"at":2,"value":0.5,"curve":"step"},{"at":1,"value":0.1,"curve":"smooth"}]}]}]}]}"#;
        let mut s: Show = serde_json::from_str(old).unwrap();
        s.sanitize();
        let keys = &s.tracks[0].events[0].envelopes[0].keys;
        assert_eq!(keys.iter().map(|k| (k.at, k.ease)).collect::<Vec<_>>(), vec![(1.0, KeyEase::Smooth), (2.0, KeyEase::Step)], "sorted");
        // Without envelopes at all: an empty list.
        let none: Show = serde_json::from_str(r#"{"tracks":[{"events":[{"id":1}]}]}"#).unwrap();
        assert!(none.tracks[0].events[0].envelopes.is_empty());
        // Bad numbers are dropped on load, refused from the editor.
        let mut e = event(1, 0.0, 4.0);
        e.envelopes = vec![env("look.size", EnvMode::Absolute, &[(f64::NAN, 1.0), (-2.0, 0.5), (1.0, f32::INFINITY)])];
        let mut bad = Show { tracks: vec![Track { events: vec![e], ..Default::default() }], ..Default::default() };
        assert!(bad.validate_edit(|_| true).is_err());
        bad.sanitize();
        assert_eq!(bad.tracks[0].events[0].envelopes[0].keys, vec![Key { at: 0.0, value: 0.5, ease: KeyEase::Linear }]);
        let mut many = event(1, 0.0, 4.0);
        many.envelopes = vec![env("look.size", EnvMode::Absolute, &[(0.0, 1.0)]); MAX_ENVELOPES + 1];
        let many = Show { tracks: vec![Track { events: vec![many], ..Default::default() }], ..Default::default() };
        assert!(many.validate_edit(|_| true).is_err());
        let mut long = event(1, 0.0, 4.0);
        long.envelopes = vec![env("look.size", EnvMode::Absolute, &vec![(0.0, 1.0); MAX_ENV_KEYS + 1])];
        let long = Show { tracks: vec![Track { events: vec![long], ..Default::default() }], ..Default::default() };
        assert!(long.validate_edit(|_| true).is_err());
    }
}
