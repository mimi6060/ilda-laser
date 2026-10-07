//! Enregistrer le jeu en direct dans la timeline (T-169): a good improvised
//! take on the song becomes an editable show.
//!
//! *● Enregistrer* readies the recorder (it never arms the laser); it
//! starts with the timeline's playback (at once if it already plays).
//! Every engine frame (`frame`), after the player moved, it **observes**:
//!
//! - the cues of the deck (`CueDeck::active`): every instance played by
//!   hand, whatever started it (keys, pads, MIDI, OSC, executors, macros,
//!   the command line), becomes an event (cue, layer, start, end) at the
//!   show position it started (from its own start time, so within a frame);
//! - the master live controls that envelopes may drive (`master.*` of the
//!   `lfo` allow-list: size, position, rotation, speed, dimmer, trace,
//!   colour…): their moves become envelope nodes, one per control at most
//!   every `MIN_KEY_GAP_S` (≤ 20 a second), simplified at the end
//!   (Douglas-Peucker, `simplify`).
//!
//! **What is never recorded**: arming, disarming, the emergency stop, the
//! blackout and every hold (« Noir », « Figer », strobe, which are no
//! envelope target), safety, calibration, zones, outputs, the tempo, the
//! look panel, the timeline's own events. A recording only reads: it never
//! changes what plays now, never arms, never sends anything.
//!
//! **When it ends**: *■ Arrêter l'enregistrement*, the timeline stopping,
//! pausing, jumping (seek, loop wrap, timecode jump) or changing show, the
//! emergency stop (`Shared::emergency_stop` ends it at once), or the
//! memory cap (`MAX_TAKE_EVENTS`, `MAX_TAKE_KEYS`). What was captured is
//! kept as the pending take (`Recorder::take`).
//!
//! **Keeping it**: `build` lays the take out as new tracks *Prise n* (cue
//! tracks per layer, overlapping cues on lanes of their own; bus tracks
//! *réglages* carrying the envelopes on every layer in use), starts
//! optionally quantised to the show's beat or bar. The page adds them to
//! its copy of the show as **one undoable edit** that goes through
//! `/api/timeline/edit` like any other: limits, track locks (new tracks
//! only, never a locked one touched) and show mode (refused, 423) are the
//! editor's. Recording itself is allowed in show mode: it only observes;
//! the take waits until the operator keeps or drops it. Played back, the
//! take's events go through the same layers, live stage, calibration and
//! safety chain as everything else.

use crate::evolving::KeyEase;
use crate::layers::LAYER_COUNT;
use crate::timeline::{Clock, EnvMode, Envelope, Event, EventSource, Key, Show, TimeBase, TimeMode, Track, TrackKind, MAX_ENVELOPES, MAX_ENV_KEYS, MAX_EVENTS, MAX_TRACKS};
use crate::Shared;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Cue events in one take at most: beyond, the recording ends (kept).
pub const MAX_TAKE_EVENTS: usize = 1000;
/// Envelope nodes in one take at most (all controls), before simplifying.
pub const MAX_TAKE_KEYS: usize = 50_000;
/// One node per control at most this often (real seconds): ≤ 20 a second.
pub const MIN_KEY_GAP_S: f64 = 0.05;
/// Douglas-Peucker tolerance, as a share of the control's range.
const SIMPLIFY_SHARE: f32 = 0.005;
/// Shortest event (show units): a cue tapped within one frame.
const MIN_LEN: f64 = 0.01;

/// *Quantifier les débuts*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantize {
    /// As played (within a frame).
    #[default]
    Off,
    /// On the nearest beat of the show.
    Beat,
    /// On the nearest bar of the show.
    Bar,
}

/// Why a recording ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// *■ Arrêter l'enregistrement*.
    User,
    /// The timeline stopped, paused, jumped or changed show.
    Transport,
    /// The emergency stop.
    EStop,
    /// The memory cap.
    Full,
}

impl EndReason {
    pub fn label_fr(self) -> &'static str {
        match self {
            EndReason::User => "arrêté",
            EndReason::Transport => "lecture arrêtée",
            EndReason::EStop => "arrêt d'urgence",
            EndReason::Full => "limite atteinte",
        }
    }
}

/// A cue played during the take, in show units.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeEvent {
    pub cue: String,
    pub layer: u8,
    pub start: f64,
    pub end: f64,
}

/// A master control moved during the take: its nodes (show position,
/// value), simplified, the first one at the take's start.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeEnvelope {
    pub target: String,
    pub keys: Vec<(f64, f32)>,
}

/// A finished recording, waiting to be kept or dropped.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Take {
    /// The show it was played on (its positions are that show's).
    pub show: String,
    pub time_base: TimeBase,
    pub start: f64,
    pub end: f64,
    pub events: Vec<TakeEvent>,
    pub envelopes: Vec<TakeEnvelope>,
    pub reason: EndReason,
}

/// A cue instance of the deck this frame: (instance id, cue id, layer,
/// show position it started at).
pub type LiveCue<'a> = (u64, &'a str, u8, f64);

struct Open {
    instance: u64,
    cue: String,
    layer: u8,
    start: f64,
}

struct Lane {
    target: String,
    range: (f32, f32),
    initial: f32,
    keys: Vec<(f64, f32)>,
    last: f32,
    /// Real time of the last node written.
    last_t: f64,
    /// A move not written yet (too soon after the last node).
    pending: Option<(f64, f32)>,
}

struct Capture {
    show: String,
    time_base: TimeBase,
    /// `Player::jumps` at the start: any change is a jump of the playhead.
    jumps: u64,
    start: f64,
    /// Show position at the last frame observed.
    pos: f64,
    open: Vec<Open>,
    events: Vec<TakeEvent>,
    lanes: Vec<Lane>,
    keys: usize,
}

/// The live take recorder (`Shared::take`). In memory only.
#[derive(Default)]
pub struct Recorder {
    /// *● Enregistrer* pressed, waiting for the timeline to play.
    waiting: bool,
    cap: Option<Capture>,
    /// The last recording, until kept or dropped.
    pub take: Option<Take>,
    /// How the last recording ended (also when it captured nothing).
    last: Option<EndReason>,
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        self.cap.is_some()
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting && self.cap.is_none()
    }

    /// *● Enregistrer*. Refused while a take waits: the operator keeps or
    /// drops it first, so nothing is ever lost by a second click.
    pub fn start(&mut self) -> Result<(), &'static str> {
        if self.take.is_some() {
            return Err("une prise attend : gardez-la ou jetez-la d'abord");
        }
        if self.cap.is_none() {
            self.waiting = true;
            self.last = None;
        }
        Ok(())
    }

    /// Starts capturing at show position `pos`. `lanes`: the controls
    /// watched, with their range and current value.
    pub fn begin(&mut self, show: &str, time_base: TimeBase, jumps: u64, pos: f64, t: f64, lanes: Vec<(String, (f32, f32), f32)>) {
        self.waiting = false;
        let lanes = lanes
            .into_iter()
            .map(|(target, range, v)| Lane { target, range, initial: v, keys: Vec::new(), last: v, last_t: t, pending: None })
            .collect();
        self.cap = Some(Capture { show: show.to_string(), time_base, jumps, start: pos, pos, open: Vec::new(), events: Vec::new(), lanes, keys: 0 });
    }

    /// One frame at show position `pos`, real time `t`: the deck's cues
    /// now, and the current values of the watched controls (in the order
    /// given to `begin`). False when the take is full: call `finish`.
    pub fn observe(&mut self, pos: f64, t: f64, cues: &[LiveCue], values: &[Option<f32>]) -> bool {
        let Some(cap) = self.cap.as_mut() else { return true };
        let prev = cap.pos;
        cap.pos = pos;
        // Cues gone since the last frame end there.
        let mut i = 0;
        while i < cap.open.len() {
            if cues.iter().any(|c| c.0 == cap.open[i].instance) {
                i += 1;
            } else {
                let o = cap.open.swap_remove(i);
                cap.events.push(TakeEvent { cue: o.cue, layer: o.layer, start: o.start, end: prev.max(o.start) });
            }
        }
        for &(instance, cue, layer, started) in cues {
            if cap.open.iter().any(|o| o.instance == instance) {
                continue;
            }
            if cap.open.len() + cap.events.len() >= MAX_TAKE_EVENTS {
                return false;
            }
            let start = if started.is_finite() { started.clamp(cap.start, pos.max(cap.start)) } else { pos };
            cap.open.push(Open { instance, cue: cue.to_string(), layer, start });
        }
        for (lane, v) in cap.lanes.iter_mut().zip(values) {
            let Some(v) = *v else { continue };
            if v.is_finite() && v != lane.last {
                // After a still stretch, a node where it still was: the
                // curve holds until the move, it doesn't ramp from afar.
                let idle = lane.pending.is_none() && t - lane.last_t >= MIN_KEY_GAP_S;
                if idle && prev > lane.keys.last().map_or(cap.start, |k| k.0) {
                    lane.keys.push((prev, lane.last));
                    cap.keys += 1;
                }
                lane.pending = Some((pos, v));
                lane.last = v;
            }
            if let Some(p) = lane.pending {
                if t - lane.last_t >= MIN_KEY_GAP_S - 1e-9 {
                    lane.keys.push(p);
                    lane.pending = None;
                    lane.last_t = t;
                    cap.keys += 1;
                }
            }
        }
        cap.keys < MAX_TAKE_KEYS
    }

    /// Ends the recording (or the wait for playback) and keeps what was
    /// captured as the pending take, if anything was.
    pub fn finish(&mut self, reason: EndReason) {
        self.waiting = false;
        let Some(cap) = self.cap.take() else { return };
        self.last = Some(reason);
        let end = cap.pos;
        let mut events = cap.events;
        events.extend(cap.open.into_iter().map(|o| TakeEvent { end: end.max(o.start), cue: o.cue, layer: o.layer, start: o.start }));
        events.sort_by(|a, b| a.start.total_cmp(&b.start));
        let envelopes: Vec<TakeEnvelope> = cap
            .lanes
            .into_iter()
            .filter_map(|mut l| {
                l.keys.extend(l.pending);
                if l.keys.is_empty() {
                    return None;
                }
                let mut keys = vec![(cap.start, l.initial)];
                keys.extend(l.keys);
                let eps = SIMPLIFY_SHARE * (l.range.1 - l.range.0).abs();
                Some(TakeEnvelope { target: l.target, keys: simplify(&keys, eps) })
            })
            .collect();
        if events.is_empty() && envelopes.is_empty() {
            return;
        }
        self.take = Some(Take { show: cap.show, time_base: cap.time_base, start: cap.start, end, events, envelopes, reason });
    }

    /// *■ Arrêter*: ends a recording, or cancels the wait for playback.
    pub fn stop(&mut self, reason: EndReason) {
        self.finish(reason);
    }

    /// *Jeter la prise*.
    pub fn discard(&mut self) {
        self.take = None;
    }

    /// For the page: the recorder's state and the pending take's summary.
    pub fn view(&self) -> Value {
        let state = if self.cap.is_some() {
            "recording"
        } else if self.waiting {
            "waiting"
        } else {
            "idle"
        };
        let cap = self.cap.as_ref();
        json!({
            "state": state,
            "show": cap.map(|c| c.show.clone()),
            "start": cap.map(|c| c.start),
            "position": cap.map(|c| c.pos),
            "events": cap.map_or(0, |c| c.events.len() + c.open.len()),
            "envelopes": cap.map_or(0, |c| c.lanes.iter().filter(|l| !l.keys.is_empty() || l.pending.is_some()).count()),
            "last": self.last.map(|r| json!({ "reason": r, "label": r.label_fr() })),
            "take": self.take.as_ref().map(|t| json!({
                "show": t.show,
                "start": t.start,
                "end": t.end,
                "events": t.events.len(),
                "envelopes": t.envelopes.iter().map(|e| e.target.clone()).collect::<Vec<_>>(),
                "reason": t.reason,
                "reason_fr": t.reason.label_fr(),
            })),
        })
    }
}

/// Douglas-Peucker on envelope nodes: drops the nodes within `eps` (in
/// value) of the line between the nodes kept around them. First and last
/// are always kept.
pub fn simplify(keys: &[(f64, f32)], eps: f32) -> Vec<(f64, f32)> {
    let n = keys.len();
    if n <= 2 {
        return keys.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let ((ta, va), (tb, vb)) = (keys[a], keys[b]);
        let mut worst = (0.0f32, a);
        for (i, &(t, v)) in keys.iter().enumerate().take(b).skip(a + 1) {
            let line = if tb - ta > 1e-12 { va + (vb - va) * ((t - ta) / (tb - ta)) as f32 } else { va };
            let d = (v - line).abs();
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > eps {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    keys.iter().zip(keep).filter(|(_, k)| *k).map(|(k, _)| *k).collect()
}

/// The next take number of `show`: 1 + the highest *Prise n* among its tracks.
pub fn next_take_number(show: &Show) -> u32 {
    show.tracks
        .iter()
        .filter_map(|t| {
            let rest = t.name.strip_prefix("Prise ")?;
            rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// `pos` (show units) on the nearest beat or bar of `show`. `beats_per_bar`:
/// the tempo clock's meter, for a Temps show.
pub fn quantize(show: &Show, pos: f64, q: Quantize, beats_per_bar: u8) -> f64 {
    let bpb = beats_per_bar.max(1) as f64;
    let out = match (show.time_base, q) {
        (_, Quantize::Off) => pos,
        (TimeBase::Seconds, Quantize::Beat) => show.s_at_beat(show.beat_at_s(pos).round()),
        (TimeBase::Seconds, Quantize::Bar) => show.s_at_bar(show.bar_at_s(pos).round()),
        (TimeBase::Beats, Quantize::Beat) => pos.round(),
        (TimeBase::Beats, Quantize::Bar) => (pos / bpb).round() * bpb,
    };
    if out.is_finite() {
        out.max(0.0)
    } else {
        pos
    }
}

/// The take laid out as new tracks for `show`.
#[derive(Debug)]
pub struct Built {
    pub tracks: Vec<Track>,
    /// Cue events placed.
    pub events: usize,
    /// Cues played that no longer exist (`known` false), left out.
    pub skipped: usize,
    pub number: u32,
}

/// Lays `take` out as tracks *Prise n* for `show` (the show it was played
/// on): cue tracks per layer, overlapping cues of a layer on lanes of
/// their own, starts quantised by `q` (the length is kept), and, if a
/// control moved, bus tracks *réglages* on every layer the show or the
/// take uses, carrying the envelopes (split into consecutive bus events
/// so no envelope holds more than `MAX_ENV_KEYS` nodes, and over several
/// tracks beyond `MAX_ENVELOPES` controls). Errors (French) past the
/// editor's limits or for an empty take.
pub fn build(take: &Take, show: &Show, q: Quantize, beats_per_bar: u8, known: impl Fn(&str) -> bool) -> Result<Built> {
    let number = next_take_number(show);
    let base = format!("Prise {number}");
    let mut skipped = 0;
    // Per layer, lanes of non-overlapping events.
    let mut layers: Vec<Vec<Vec<Event>>> = vec![Vec::new(); LAYER_COUNT];
    let mut placed = 0;
    for e in &take.events {
        if !known(&e.cue) {
            skipped += 1;
            continue;
        }
        let len = (e.end - e.start).max(MIN_LEN);
        let start = quantize(show, e.start, q, beats_per_bar);
        let layer = e.layer.clamp(1, LAYER_COUNT as u8);
        let lanes = &mut layers[(layer - 1) as usize];
        let free = lanes.iter().position(|l| l.last().is_none_or(|p| p.start + p.len <= start + 1e-9));
        let ev = Event { start, len, source: EventSource::Cue { id: e.cue.clone() }, time: TimeMode::Beats, ..Default::default() };
        match free {
            Some(i) => lanes[i].push(ev),
            None => lanes.push(vec![ev]),
        }
        placed += 1;
    }
    let cue_tracks = layers.iter().map(Vec::len).sum::<usize>();
    let bus_layers: Vec<u8> = if take.envelopes.is_empty() || take.end - take.start <= MIN_LEN {
        Vec::new()
    } else {
        let mut used: Vec<u8> = (1..=LAYER_COUNT as u8)
            .filter(|&l| !layers[(l - 1) as usize].is_empty() || show.tracks.iter().any(|t| t.kind == TrackKind::Cues && t.layer == l && !t.events.is_empty()))
            .collect();
        if used.is_empty() {
            used.push(1);
        }
        used
    };
    if placed == 0 && bus_layers.is_empty() {
        bail!("prise vide : aucun cue joué ni réglage bougé");
    }
    let mut tracks = Vec::new();
    let single = cue_tracks == 1 && bus_layers.is_empty();
    for (li, lanes) in layers.into_iter().enumerate() {
        for (k, events) in lanes.into_iter().enumerate() {
            let name = if single {
                base.clone()
            } else if k == 0 {
                format!("{base} · C{}", li + 1)
            } else {
                format!("{base} · C{} ({})", li + 1, k + 1)
            };
            tracks.push(Track { name, layer: li as u8 + 1, events, ..Default::default() });
        }
    }
    let groups: Vec<&[TakeEnvelope]> = take.envelopes.chunks(MAX_ENVELOPES).collect();
    for layer in bus_layers {
        for (g, envs) in groups.iter().enumerate() {
            let name = if g == 0 { format!("{base} · réglages C{layer}") } else { format!("{base} · réglages C{layer} ({})", g + 1) };
            tracks.push(Track { name, kind: TrackKind::Bus, layer, events: bus_events(envs, take.start, take.end), ..Default::default() });
        }
    }
    if show.tracks.len() + tracks.len() > MAX_TRACKS {
        bail!("trop de pistes pour garder la prise ({} au plus)", MAX_TRACKS);
    }
    let total = show.tracks.iter().map(|t| t.events.len()).sum::<usize>() + tracks.iter().map(|t| t.events.len()).sum::<usize>();
    if total > MAX_EVENTS {
        bail!("trop d'événements pour garder la prise ({} au plus)", MAX_EVENTS);
    }
    Ok(Built { tracks, events: placed, skipped, number })
}

/// The value of linear nodes at `x`.
fn value_at(keys: &[(f64, f32)], x: f64) -> f32 {
    let k: Vec<(f64, f32, KeyEase)> = keys.iter().map(|&(t, v)| (t, v, KeyEase::Linear)).collect();
    crate::program::curve_at(&k, x).unwrap_or(0.0)
}

/// Consecutive bus events over `[start, end]`, each with every envelope
/// of `envs`, cut so no envelope gets more than `MAX_ENV_KEYS` nodes: the
/// nodes inside the event, plus its value where it starts and ends.
fn bus_events(envs: &[TakeEnvelope], start: f64, end: f64) -> Vec<Event> {
    let room = MAX_ENV_KEYS - 2;
    // Where to cut: walk the nodes in time order, cut before one that
    // would overflow its envelope in the current event.
    let mut nodes: Vec<(f64, usize)> = envs.iter().enumerate().flat_map(|(i, e)| e.keys.iter().map(move |k| (k.0, i))).collect();
    nodes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts = vec![start];
    let mut counts = vec![0usize; envs.len()];
    for (t, i) in nodes {
        if t <= start || t >= end {
            continue;
        }
        if counts[i] + 1 > room && t > *cuts.last().unwrap() {
            cuts.push(t);
            counts.iter_mut().for_each(|c| *c = 0);
        }
        counts[i] += 1;
    }
    cuts.push(end);
    cuts.windows(2)
        .filter(|w| w[1] - w[0] > 1e-9)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let envelopes = envs
                .iter()
                .map(|e| {
                    let mut keys = vec![Key { at: 0.0, value: value_at(&e.keys, a), ease: KeyEase::Linear }];
                    keys.extend(e.keys.iter().filter(|k| k.0 > a && k.0 < b).map(|&(t, v)| Key { at: t - a, value: v, ease: KeyEase::Linear }));
                    keys.push(Key { at: b - a, value: value_at(&e.keys, b), ease: KeyEase::Linear });
                    Envelope { target: e.target.clone(), mode: EnvMode::Absolute, keys }
                })
                .collect();
            Event { start: a, len: b - a, source: EventSource::Cue { id: String::new() }, envelopes, ..Default::default() }
        })
        .collect()
}

/// The master controls a take watches: `master.*` ids envelopes may drive.
fn watched(s: &Shared) -> Vec<(String, (f32, f32))> {
    s.controls
        .list()
        .iter()
        .filter(|d| d.id.starts_with("master."))
        .filter_map(|d| crate::lfo::modulatable(&s.controls, &d.id).map(|r| (d.id.clone(), r)))
        .collect()
}

/// Once per engine frame, after the timeline player moved (`main.rs`):
/// starts a waiting recording when the timeline plays, ends one on the
/// e-stop or a transport change, and otherwise observes. Reads only.
pub fn frame(s: &mut Shared, c: &Clock) {
    if !s.take.waiting && s.take.cap.is_none() {
        return;
    }
    if s.estop.is_latched() {
        s.take.finish(EndReason::EStop);
        return;
    }
    let Some(show) = s.timeline.show.as_ref() else {
        s.take.finish(EndReason::Transport);
        return;
    };
    let playing = s.timeline.is_playing();
    let jumps = s.timeline.jumps();
    let pos = s.timeline.position_at(c);
    match s.take.cap.as_ref() {
        Some(cap) if !playing || cap.jumps != jumps || cap.show != show.name => {
            s.take.finish(EndReason::Transport);
            return;
        }
        Some(_) => {}
        None if playing => {
            let (name, base) = (show.name.clone(), show.time_base);
            let lanes = watched(s);
            let ids: Vec<&str> = lanes.iter().map(|l| l.0.as_str()).collect();
            let values = crate::lfo::master_values(&ids, &s.live);
            let lanes = lanes.into_iter().zip(values).map(|((id, r), v)| (id, r, v.unwrap_or(0.0))).collect();
            s.take.begin(&name, base, jumps, pos, c.t, lanes);
        }
        None => return,
    }
    let base = s.timeline.show.as_ref().map_or(TimeBase::Seconds, |sh| sh.time_base);
    // Show units per real second, to place a cue at its own start time.
    let rate = match base {
        TimeBase::Seconds => 1.0,
        TimeBase::Beats => c.bpm / 60.0,
    };
    let values = {
        let ids: Vec<&str> = s.take.cap.as_ref().map(|cap| cap.lanes.iter().map(|l| l.target.as_str()).collect()).unwrap_or_default();
        crate::lfo::master_values(&ids, &s.live)
    };
    let cues: Vec<LiveCue> = s.deck.active.iter().map(|a| (a.id, a.cue.as_str(), a.layer, pos - (c.t - a.started_s).max(0.0) * rate)).collect();
    if !s.take.observe(pos, c.t, &cues, &values) {
        s.take.finish(EndReason::Full);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{self, ControlInput};
    use crate::timeline::Player;
    use std::collections::HashMap;

    const FPS: f64 = 60.0;

    fn show(base: TimeBase) -> Show {
        Show::new_empty("Morceau", base)
    }

    type Live = Vec<(u64, &'static str, u8, f64)>;

    /// Frames at 60 fps from `from` to `to` s (show = real seconds from 0),
    /// with the cues given by `at(t)` and the one fader `fader(t)`.
    fn run(rec: &mut Recorder, from: f64, to: f64, at: impl Fn(f64) -> Live, fader: impl Fn(f64) -> f32) {
        let mut k = (from * FPS).round() as u64;
        while (k as f64) / FPS <= to + 1e-9 {
            let t = k as f64 / FPS;
            let cues = at(t);
            assert!(rec.observe(t, t, &cues, &[Some(fader(t))]));
            k += 1;
        }
    }

    fn lane() -> Vec<(String, (f32, f32), f32)> {
        vec![("master.size".to_string(), (0.0, 2.0), 1.0)]
    }

    #[test]
    fn three_cues_in_30_s_make_three_events_at_their_instants() {
        let mut rec = Recorder::default();
        rec.start().unwrap();
        rec.begin("Morceau", TimeBase::Seconds, 1, 0.0, 0.0, lane());
        // Cue a 2.0–9.5 s, b 10.0–20.0 s, c 21.3 s to the end (layer 2).
        let at = |t: f64| {
            let mut v: Live = Vec::new();
            if (2.0..9.5).contains(&t) {
                v.push((1, "a", 1, 2.0));
            }
            if (10.0..20.0).contains(&t) {
                v.push((2, "b", 1, 10.0));
            }
            if t >= 21.3 {
                v.push((3, "c", 2, 21.3));
            }
            v
        };
        run(&mut rec, 0.0, 30.0, at, |_| 1.0);
        rec.stop(EndReason::User);
        let take = rec.take.clone().unwrap();
        assert!(take.envelopes.is_empty(), "the fader never moved");
        let frame = 1.0 / FPS;
        assert_eq!(take.events.len(), 3);
        for (e, (cue, layer, s, end)) in take.events.iter().zip([("a", 1, 2.0, 9.5), ("b", 1, 10.0, 20.0), ("c", 2, 21.3, 30.0)]) {
            assert_eq!((e.cue.as_str(), e.layer), (cue, layer));
            assert!((e.start - s).abs() <= frame + 1e-9, "{cue} starts at {}", e.start);
            assert!((e.end - end).abs() <= frame + 1e-9, "{cue} ends at {}", e.end);
        }
        assert_eq!(rec.view()["take"]["events"], 3);
        assert!(rec.start().is_err(), "a pending take is never overwritten");
        rec.discard();
        assert!(rec.start().is_ok());
    }

    #[test]
    fn a_fader_moved_all_along_gives_at_most_20_nodes_a_second() {
        let mut rec = Recorder::default();
        rec.begin("Morceau", TimeBase::Seconds, 1, 0.0, 0.0, lane());
        // A wobbly hand: never still, never a straight line.
        run(&mut rec, 0.0, 10.0, |_| Vec::new(), |t| 1.0 + 0.8 * (t * 2.3).sin() as f32 + 0.05 * (t * 31.0).sin() as f32);
        let raw = rec.cap.as_ref().unwrap().lanes[0].keys.len();
        assert!(raw <= 20 * 10 + 1, "{raw} nodes before simplifying");
        rec.stop(EndReason::User);
        let env = &rec.take.as_ref().unwrap().envelopes[0];
        assert_eq!(env.target, "master.size");
        assert!(env.keys.len() <= raw + 2 && env.keys.len() > 10, "{}", env.keys.len());
        assert_eq!(env.keys[0], (0.0, 1.0), "starts from where it was");
        // A slow straight move collapses to its ends.
        let mut rec = Recorder::default();
        rec.begin("Morceau", TimeBase::Seconds, 1, 0.0, 0.0, lane());
        run(&mut rec, 0.0, 4.0, |_| Vec::new(), |t| 1.0 + (t as f32 / 4.0) * 0.5);
        rec.stop(EndReason::User);
        assert!(rec.take.as_ref().unwrap().envelopes[0].keys.len() <= 3);
    }

    #[test]
    fn a_still_fader_then_a_jump_holds_until_the_move() {
        let mut rec = Recorder::default();
        rec.begin("Morceau", TimeBase::Seconds, 1, 0.0, 0.0, lane());
        run(&mut rec, 0.0, 6.0, |_| Vec::new(), |t| if t < 5.0 { 1.0 } else { 0.4 });
        rec.stop(EndReason::User);
        let keys = &rec.take.as_ref().unwrap().envelopes[0].keys;
        assert_eq!(value_at(keys, 2.5), 1.0, "no ramp from the start: {keys:?}");
        assert!((value_at(keys, 5.5) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn douglas_peucker_keeps_the_corners_only() {
        let line: Vec<(f64, f32)> = (0..50).map(|i| (i as f64, i as f32 * 0.1)).collect();
        assert_eq!(simplify(&line, 0.01), vec![(0.0, 0.0), (49.0, 4.9)]);
        let mut tri: Vec<(f64, f32)> = (0..=10).map(|i| (i as f64, i as f32)).collect();
        tri.extend((11..=20).map(|i| (i as f64, (20 - i) as f32)));
        assert_eq!(simplify(&tri, 0.01), vec![(0.0, 0.0), (10.0, 10.0), (20.0, 0.0)]);
        assert_eq!(simplify(&[(0.0, 1.0)], 0.1), vec![(0.0, 1.0)]);
    }

    fn take_of(events: Vec<TakeEvent>, envelopes: Vec<TakeEnvelope>, end: f64) -> Take {
        Take { show: "Morceau".into(), time_base: TimeBase::Seconds, start: 0.0, end, events, envelopes, reason: EndReason::User }
    }

    fn ev(cue: &str, layer: u8, start: f64, end: f64) -> TakeEvent {
        TakeEvent { cue: cue.into(), layer, start, end }
    }

    #[test]
    fn the_take_becomes_new_tracks_prise_n_with_lanes_and_quantised_starts() {
        let mut sh = show(TimeBase::Seconds);
        sh.tracks[0].events.push(Event { start: 0.0, len: 4.0, source: EventSource::Cue { id: "x".into() }, ..Default::default() });
        sh.tracks[0].locked = true;
        sh.tracks.push(Track { name: "Prise 2".into(), ..Default::default() });
        sh.sanitize(); // as loaded
        let take = take_of(vec![ev("a", 1, 0.48, 2.0), ev("b", 1, 1.0, 3.0), ev("gone", 3, 1.0, 2.0)], Vec::new(), 4.0);
        let b = build(&take, &sh, Quantize::Beat, 4, |id| id != "gone").unwrap();
        assert_eq!((b.number, b.events, b.skipped), (3, 2, 1));
        let names: Vec<&str> = b.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Prise 3 · C1", "Prise 3 · C1 (2)"], "overlapping cues on their own lane");
        // 120 BPM: a beat is 0.5 s; the length is kept.
        let a = &b.tracks[0].events[0];
        assert_eq!(a.start, 0.5);
        assert!((a.len - 1.52).abs() < 1e-9);
        assert_eq!(b.tracks[1].events[0].start, 1.0);
        // Appended: the locked track is untouched, the editor accepts it.
        let mut merged = sh.clone();
        merged.tracks.extend(b.tracks);
        merged.sanitize();
        sh.check_locks(&merged).unwrap();
        merged.validate_edit(|id| ["a", "b", "x"].contains(&id)).unwrap();
        // One cue track alone is just « Prise n »; bars in a Temps show.
        let take = take_of(vec![ev("a", 2, 5.0, 6.0)], Vec::new(), 8.0);
        let mut beats = show(TimeBase::Beats);
        beats.tracks.clear();
        let b = build(&take, &beats, Quantize::Bar, 4, |_| true).unwrap();
        assert_eq!(b.tracks[0].name, "Prise 1");
        assert_eq!((b.tracks[0].layer, b.tracks[0].events[0].start), (2, 4.0));
        assert!(build(&take_of(Vec::new(), Vec::new(), 3.0), &beats, Quantize::Off, 4, |_| true).is_err());
    }

    #[test]
    fn envelopes_go_on_bus_tracks_split_to_the_node_limit() {
        // 200 nodes of a zigzag (nothing to simplify), on two controls.
        let zig: Vec<(f64, f32)> = (0..200).map(|i| (i as f64 * 0.1, if i % 2 == 0 { 0.5 } else { 1.5 })).collect();
        let envs = vec![
            TakeEnvelope { target: "master.size".into(), keys: zig.clone() },
            TakeEnvelope { target: "master.pos_x".into(), keys: vec![(0.0, 0.0), (10.0, 0.5)] },
        ];
        let take = take_of(vec![ev("a", 1, 0.0, 20.0), ev("b", 3, 2.0, 4.0)], envs, 20.0);
        let b = build(&take, &show(TimeBase::Seconds), Quantize::Off, 4, |_| true).unwrap();
        let bus: Vec<&Track> = b.tracks.iter().filter(|t| t.kind == TrackKind::Bus).collect();
        let names: Vec<(&str, u8)> = bus.iter().map(|t| (t.name.as_str(), t.layer)).collect();
        assert_eq!(names, [("Prise 1 · réglages C1", 1), ("Prise 1 · réglages C3", 3)]);
        let events = &bus[0].events;
        assert!(events.len() >= 4, "{} bus events", events.len());
        for e in events {
            assert_eq!(e.envelopes.len(), 2);
            assert!(e.envelopes.iter().all(|env| env.keys.len() <= MAX_ENV_KEYS));
        }
        // Contiguous, and the curve is the recorded one everywhere.
        for w in events.windows(2) {
            assert!((w[0].start + w[0].len - w[1].start).abs() < 1e-9);
        }
        for i in 0..400 {
            let x = i as f64 * 0.05;
            let e = events.iter().rev().find(|e| e.start <= x).unwrap();
            let got = e.envelopes[0].value_at(x - e.start).unwrap();
            assert!((got - value_at(&zig, x)).abs() < 1e-4, "at {x}: {got}");
        }
        let mut merged = show(TimeBase::Seconds);
        merged.tracks.extend(b.tracks);
        merged.validate_edit(|_| true).unwrap();
    }

    /// Played back, the take plays the same cues at the same instants as
    /// live (within a frame).
    #[test]
    fn the_take_played_back_gives_the_same_cues_at_the_same_instants() {
        let live = |t: f64| {
            let mut v: Live = Vec::new();
            if (1.0..4.0).contains(&t) {
                v.push((1, "a", 1, 1.0));
            }
            if (3.0..7.25).contains(&t) {
                v.push((2, "b", 1, 3.0));
            }
            if (5.5..9.0).contains(&t) {
                v.push((3, "c", 4, 5.5));
            }
            v
        };
        let mut rec = Recorder::default();
        rec.begin("Morceau", TimeBase::Seconds, 1, 0.0, 0.0, lane());
        run(&mut rec, 0.0, 10.0, live, |_| 1.0);
        rec.stop(EndReason::User);
        let mut sh = show(TimeBase::Seconds);
        sh.tracks.clear();
        sh.tracks.extend(build(rec.take.as_ref().unwrap(), &sh, Quantize::Off, 4, |_| true).unwrap().tracks);
        let mut player = Player::default();
        player.load(sh);
        let loaded = player.show.clone().unwrap();
        let ids: HashMap<u64, String> = loaded
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .map(|e| (e.id, if let EventSource::Cue { id } = &e.source { id.clone() } else { String::new() }))
            .collect();
        let clock = |t: f64| Clock { t, beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 };
        player.play(&clock(0.0));
        let frame = 1.0 / FPS;
        for k in 0..590 {
            let t = k as f64 * frame + frame / 2.0;
            let mut played: Vec<String> = player.frame(&clock(t)).iter().map(|c| ids[&c.event].clone()).collect();
            played.sort();
            // Away from the edges by more than a frame, exactly the live set.
            let near_edge = [1.0, 3.0, 4.0, 5.5, 7.25, 9.0].iter().any(|e| (t - e).abs() <= frame * 1.5);
            if !near_edge {
                let mut want: Vec<String> = live(t).iter().map(|c| c.1.to_string()).collect();
                want.sort();
                assert_eq!(played, want, "at {t}");
            }
        }
    }

    fn at(s: &mut Shared, t: f64) -> Clock {
        s.sim_now = Some(s.epoch + std::time::Duration::from_secs_f64(t));
        Clock { t, beat: s.tempo.effect_beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar }
    }

    /// One engine frame as far as the take sees it.
    fn tick(s: &mut Shared, t: f64) {
        let c = at(s, t);
        s.timeline.frame(&c);
        frame(s, &c);
    }

    #[test]
    fn recording_in_the_studio_observes_only_and_stops_on_the_estop() {
        let mut s = crate::test_support::shared();
        let (a, b) = (s.presets[0].id.clone(), s.presets[1].id.clone());
        let mut sh = show(TimeBase::Seconds);
        // Something late in the show, so it plays on.
        sh.tracks[1].events.push(Event { start: 50.0, len: 1.0, source: EventSource::Cue { id: b }, ..Default::default() });
        s.timeline.load(sh);
        assert!(s.take.start().is_ok());
        tick(&mut s, 10.0);
        assert!(s.take.is_waiting() && !s.take.is_recording(), "waits for the timeline");
        let c = at(&mut s, 10.0);
        controls::timeline_play_at(&mut s, &c).unwrap();
        tick(&mut s, 10.0);
        assert!(s.take.is_recording());
        let before = (s.gate.is_armed(), s.live.clone(), s.deck.active.len());
        tick(&mut s, 10.5);
        assert_eq!(before, (s.gate.is_armed(), s.live.clone(), s.deck.active.len()), "observing changes nothing");
        at(&mut s, 11.0);
        assert!(controls::press_cue(&mut s, &a, None, true));
        tick(&mut s, 11.01);
        controls::apply(&mut s, "master.size", ControlInput::Value(0.5), false).unwrap();
        tick(&mut s, 12.0);
        // Holds and safety are no envelope target: never recorded.
        controls::apply(&mut s, "transport.black_hold", ControlInput::Value(1.0), false).unwrap();
        tick(&mut s, 12.5);
        s.emergency_stop(crate::interlock::ArmSource::Ui);
        assert!(!s.take.is_recording(), "the e-stop ends it at once");
        tick(&mut s, 13.0);
        let take = s.take.take.clone().expect("kept what it captured");
        assert_eq!(take.reason, EndReason::EStop);
        assert_eq!(take.events.len(), 1);
        assert_eq!(take.events[0].cue, a);
        assert!((take.events[0].start - 1.0).abs() < 0.02, "at its show position: {}", take.events[0].start);
        let targets: Vec<&str> = take.envelopes.iter().map(|e| e.target.as_str()).collect();
        assert_eq!(targets, ["master.size"]);
        assert!(!s.gate.is_armed());
        // Kept through the editor: new tracks only.
        let shown = s.timeline.show.clone().unwrap();
        let built = build(&take, &shown, Quantize::Off, 4, |_| true).unwrap();
        let names: Vec<&str> = built.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Prise 1 · C1", "Prise 1 · réglages C1", "Prise 1 · réglages C2"]);
    }

    #[test]
    fn a_transport_jump_ends_the_recording_and_an_empty_one_leaves_nothing() {
        let mut s = crate::test_support::shared();
        let mut sh = show(TimeBase::Seconds);
        sh.tracks[0].events.push(Event { start: 0.0, len: 60.0, source: EventSource::Cue { id: s.presets[0].id.clone() }, ..Default::default() });
        s.timeline.load(sh);
        let c = at(&mut s, 1.0);
        controls::timeline_play_at(&mut s, &c).unwrap();
        s.take.start().unwrap();
        tick(&mut s, 1.0);
        assert!(s.take.is_recording(), "starts at once while playing");
        tick(&mut s, 2.0);
        let c = at(&mut s, 2.0);
        s.timeline.seek(30.0, &c);
        tick(&mut s, 2.1);
        assert!(!s.take.is_recording());
        assert!(s.take.take.is_none(), "nothing played, nothing kept");
        assert_eq!(s.take.view()["last"]["reason"], "transport");
    }
}
