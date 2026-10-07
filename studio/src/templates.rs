//! Timeline templates (T-165): phrases ready to lay down.
//!
//! A template is a phrase of `length_bars` bars whose events name a
//! *slot* (« A », « B »…) instead of a cue, with positions, lengths and
//! envelope nodes in **beats** from the start of the phrase. *Appliquer*
//! lays it on a region of the loaded show: each slot gets a cue (or
//! *Auto*: one picked from a page of the catalogue), the phrase is
//! stretched, repeated or cropped to the region, and the result becomes
//! ordinary events (a copy, editable afterwards). Beats become show units
//! through the show's tempo map, so in a *Secondes* show the events are
//! seconds and stay put if the BPM of a section changes later.
//!
//! `render` only computes the new show: the page sends it through
//! `POST /api/timeline/edit` like any edit (its undo, the track locks
//! of T-445, show mode's 423). Templates are content only: nothing here
//! reads or writes the arm state.
//!
//! User templates live in `studio-data/templates/<name>.json`; built-in
//! ones are written in code (`builtin`, filled by T-166), all our own.

use crate::layers::LAYER_COUNT;
use crate::evolving::KeyEase;
use crate::live::{ChaseSpread, ColorOverride, LiveModifiers, Rate, Strobe};
use crate::presets::Preset;
use crate::timeline::{valid_show_name, EndAction, EnvMode, Envelope, Event, EventSource, Key, Show, TimeBase, TimeMode, Track, TrackKind, MAX_ENVELOPES, MAX_ENV_KEYS, MAX_EVENTS, MAX_LABEL, MAX_TRACKS};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Slots « A » to « Z ».
pub const MAX_SLOTS: usize = 26;
pub const MAX_TEMPLATE_EVENTS: usize = 500;
pub const MAX_LENGTH_BARS: u16 = 256;
pub const MAX_DESCRIPTION: usize = 300;
const EPS: f64 = 1e-6;

/// How a template fills a region longer or shorter than itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// *Étirer*: positions and lengths scaled to the region.
    #[default]
    Stretch,
    /// *Répéter*: copies end to end, aligned on the template's bars; the
    /// last one is cut at the end of the region.
    Repeat,
    /// *Couper*: one copy, cut at the end of the region.
    Crop,
}

/// One event of a template, in beats from the start of the phrase.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateEvent {
    /// The template's own cue track (0 = the first target track).
    pub track: u8,
    pub start_beat: f64,
    pub len_beats: f64,
    pub slot: String,
    pub time: TimeMode,
    pub end: EndAction,
    pub modifiers: LiveModifiers,
    /// Node positions in beats from the event start.
    pub envelopes: Vec<Envelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
}

impl Default for TemplateEvent {
    fn default() -> Self {
        Self {
            track: 0,
            start_beat: 0.0,
            len_beats: 4.0,
            slot: "A".into(),
            time: TimeMode::Beats,
            end: EndAction::Stop,
            modifiers: LiveModifiers::default(),
            envelopes: Vec::new(),
            scan: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub length_bars: u16,
    pub beats_per_bar: u8,
    pub slots: Vec<String>,
    pub events: Vec<TemplateEvent>,
    /// Envelopes over the whole phrase, on a *Bus* track of the first
    /// target track's layer; node positions in beats from the phrase start.
    pub bus_envelopes: Vec<Envelope>,
    /// The catalogue page each slot suggests (« Auto : page »), preselected
    /// in the dialog; the operator can still pick anything. Built-ins
    /// fill it, a saved selection leaves it empty.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub auto_pages: BTreeMap<String, String>,
}

impl Default for Template {
    fn default() -> Self {
        Self { name: String::new(), description: String::new(), length_bars: 8, beats_per_bar: 4, slots: Vec::new(), events: Vec::new(), bus_envelopes: Vec::new(), auto_pages: BTreeMap::new() }
    }
}

fn check_envelopes(envs: &[Envelope]) -> Result<()> {
    if envs.len() > MAX_ENVELOPES {
        bail!("{} enveloppes au plus", MAX_ENVELOPES);
    }
    for env in envs {
        if env.target.is_empty() || env.target.chars().count() > MAX_LABEL {
            bail!("enveloppe : paramètre invalide");
        }
        if env.keys.len() > MAX_ENV_KEYS {
            bail!("enveloppe « {} » : {} nœuds au plus", env.target, MAX_ENV_KEYS);
        }
        if env.keys.iter().any(|k| !k.at.is_finite() || !k.value.is_finite() || k.at < 0.0) {
            bail!("enveloppe « {} » : nœud invalide", env.target);
        }
    }
    Ok(())
}

impl Template {
    /// Length of the phrase in beats.
    pub fn beats(&self) -> f64 {
        self.length_bars.max(1) as f64 * self.beats_per_bar.max(1) as f64
    }

    /// Everything a file or a request may get wrong. French messages.
    pub fn validate(&self) -> Result<()> {
        if !valid_show_name(&self.name) {
            bail!("nom de modèle invalide (lettres, chiffres, espaces, - et _ seulement)");
        }
        if self.description.chars().count() > MAX_DESCRIPTION {
            bail!("description trop longue ({} caractères au plus)", MAX_DESCRIPTION);
        }
        if !(1..=MAX_LENGTH_BARS).contains(&self.length_bars) {
            bail!("longueur : 1 à {} mesures", MAX_LENGTH_BARS);
        }
        if !(1..=16).contains(&self.beats_per_bar) {
            bail!("temps par mesure : 1 à 16");
        }
        if self.slots.len() > MAX_SLOTS {
            bail!("{} emplacements au plus", MAX_SLOTS);
        }
        for (i, s) in self.slots.iter().enumerate() {
            if s.is_empty() || s.chars().count() > 16 || self.slots[..i].contains(s) {
                bail!("emplacement invalide : {}", s.chars().take(20).collect::<String>());
            }
        }
        if self.events.len() > MAX_TEMPLATE_EVENTS {
            bail!("{} événements au plus", MAX_TEMPLATE_EVENTS);
        }
        let beats = self.beats();
        for e in &self.events {
            if !self.slots.contains(&e.slot) {
                bail!("emplacement inconnu : {}", e.slot.chars().take(20).collect::<String>());
            }
            if e.track as usize >= MAX_TRACKS {
                bail!("trop de pistes ({} au plus)", MAX_TRACKS);
            }
            if !(e.start_beat.is_finite() && e.len_beats.is_finite() && e.start_beat >= 0.0 && e.len_beats > 0.0 && e.start_beat < beats) {
                bail!("événement hors du modèle (début ou durée invalide)");
            }
            check_envelopes(&e.envelopes)?;
        }
        for (slot, page) in &self.auto_pages {
            if !self.slots.contains(slot) || page.is_empty() || page.chars().count() > 64 {
                bail!("page suggérée invalide pour l'emplacement {}", slot.chars().take(20).collect::<String>());
            }
        }
        check_envelopes(&self.bus_envelopes)
    }
}

/// What a slot plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotChoice {
    /// A cue of the catalogue, by preset id.
    Cue(String),
    /// *Auto*: a cue of this catalogue page.
    Auto(String),
}

/// Where and how to lay a template.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApplyOptions {
    /// The region, in show units (seconds or beats).
    pub from: f64,
    pub to: f64,
    pub fit: Fit,
    /// The show track the template's first cue track lands on; the next
    /// ones follow on the next cue tracks (created when missing).
    pub track: usize,
    /// Remove the events that start in the region on the target tracks first.
    pub replace: bool,
    pub slots: HashMap<String, SlotChoice>,
}

/// A template laid on a show: the new show and what changed.
#[derive(Debug)]
pub struct Applied {
    pub show: Show,
    pub added: usize,
    pub removed: usize,
}

fn beat_of(show: &Show, u: f64) -> f64 {
    match show.time_base {
        TimeBase::Seconds => show.beat_at_s(u),
        TimeBase::Beats => u,
    }
}

fn u_of(show: &Show, b: f64) -> f64 {
    match show.time_base {
        TimeBase::Seconds => show.s_at_beat(b),
        TimeBase::Beats => b,
    }
}

/// Beats per bar at `u` (Secondes: the tempo map; Temps: the meter noted
/// in the show, else 4).
fn meter_at(show: &Show, u: f64) -> u8 {
    let pts = &show.tempo_map;
    let p = match show.time_base {
        TimeBase::Seconds => pts.iter().rev().find(|p| p.at_s <= u + EPS).or(pts.first()),
        TimeBase::Beats => pts.first(),
    };
    p.map_or(4, |p| p.beats_per_bar.clamp(1, 16))
}

/// The beat where the bar holding `u` starts.
fn bar_start_beat(show: &Show, u: f64) -> f64 {
    match show.time_base {
        TimeBase::Seconds => show.beat_at_s(show.s_at_bar((show.bar_at_s(u) + EPS).floor())),
        TimeBase::Beats => {
            let bpb = meter_at(show, u) as f64;
            ((u / bpb) + EPS).floor() * bpb
        }
    }
}

/// A cue id per slot. *Auto* slots of the same page get different cues
/// (in catalogue order) as long as the page has enough.
fn resolve_slots(t: &Template, choices: &HashMap<String, SlotChoice>, presets: &[Preset]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut taken: HashMap<&str, usize> = HashMap::new();
    for slot in &t.slots {
        let id = match choices.get(slot) {
            None => bail!("emplacement {slot} : choisissez un cue"),
            Some(SlotChoice::Cue(id)) => {
                if !presets.iter().any(|p| p.id == *id) {
                    bail!("emplacement {slot} : cue inconnu ({})", id.chars().take(40).collect::<String>());
                }
                id.clone()
            }
            Some(SlotChoice::Auto(cat)) => {
                let pool: Vec<&Preset> = presets.iter().filter(|p| p.category == cat.as_str()).collect();
                if pool.is_empty() {
                    bail!("emplacement {slot} : page de cues vide ou inconnue ({})", cat.chars().take(40).collect::<String>());
                }
                let n = taken.entry(pool[0].category).or_insert(0);
                let id = pool[*n % pool.len()].id.clone();
                *n += 1;
                id
            }
        };
        out.insert(slot.clone(), id);
    }
    Ok(out)
}

/// Lays `t` on `show` (see the module docs). The result is sanitized
/// (events numbered and sorted) but not validated: the edit route does.
pub fn render(show: &Show, t: &Template, o: &ApplyOptions, presets: &[Preset]) -> Result<Applied> {
    t.validate()?;
    if !(o.from.is_finite() && o.to.is_finite() && o.from >= 0.0 && o.to > o.from) {
        bail!("région invalide : choisissez un début et une fin");
    }
    let cues = resolve_slots(t, &o.slots, presets)?;
    let (b0, b1) = (beat_of(show, o.from), beat_of(show, o.to));
    let tlen = t.beats();
    if b1 - b0 < EPS {
        bail!("région trop courte");
    }
    let copies: Vec<(f64, f64)> = match o.fit {
        Fit::Stretch => vec![(b0, (b1 - b0) / tlen)],
        Fit::Crop => vec![(b0, 1.0)],
        Fit::Repeat => {
            let n = ((b1 - b0) / tlen - EPS).ceil().max(1.0);
            if n * (t.events.len() + 1) as f64 > MAX_EVENTS as f64 {
                bail!("trop d'événements ({} au plus) : région trop longue pour ce modèle", MAX_EVENTS);
            }
            (0..n as usize).map(|i| (b0 + i as f64 * tlen, 1.0)).collect()
        }
    };

    let mut out = show.clone();
    if o.track > out.tracks.len() {
        bail!("piste de départ inconnue");
    }
    // The template's cue tracks → the show's cue tracks from `o.track` on.
    let wanted = t.events.iter().map(|e| e.track as usize + 1).max().unwrap_or(0);
    let mut targets: Vec<usize> = Vec::new();
    let mut i = o.track;
    while targets.len() < wanted {
        if i >= out.tracks.len() {
            if out.tracks.len() >= MAX_TRACKS {
                bail!("trop de pistes ({} au plus)", MAX_TRACKS);
            }
            let n = out.tracks.len() + 1;
            out.tracks.push(Track { name: format!("Piste {n}"), layer: ((n - 1) % LAYER_COUNT) as u8 + 1, ..Default::default() });
        }
        if out.tracks[i].kind == TrackKind::Cues {
            targets.push(i);
        }
        i += 1;
    }
    let mut used: Vec<usize> = t.events.iter().map(|e| targets[e.track as usize]).collect();
    // The bus track of the first target's layer, made when missing.
    let bus = if t.bus_envelopes.is_empty() {
        None
    } else {
        let layer = targets.first().or(Some(&o.track)).and_then(|&i| out.tracks.get(i)).map_or(1, |t| t.layer);
        Some(match out.tracks.iter().position(|t| t.kind == TrackKind::Bus && t.layer == layer) {
            Some(i) => i,
            None => {
                if out.tracks.len() >= MAX_TRACKS {
                    bail!("trop de pistes ({} au plus)", MAX_TRACKS);
                }
                out.tracks.push(Track { name: format!("Bus {layer}"), kind: TrackKind::Bus, layer, ..Default::default() });
                out.tracks.len() - 1
            }
        })
    };
    used.extend(bus);
    used.sort_unstable();
    used.dedup();
    if let Some(&i) = used.iter().find(|&&i| out.tracks[i].locked) {
        bail!("piste verrouillée « {} » : déverrouillez-la ou choisissez une autre piste", out.tracks[i].name);
    }
    let mut removed = 0;
    if o.replace {
        for &i in &used {
            let before = out.tracks[i].events.len();
            out.tracks[i].events.retain(|e| !(e.start >= o.from - EPS && e.start < o.to - EPS));
            removed += before - out.tracks[i].events.len();
        }
    }

    // Envelope nodes in beats from `base` → show units from `start`.
    let keys = |env: &Envelope, base: f64, scale: f64, start: f64| Envelope {
        keys: env.keys.iter().map(|k| Key { at: (u_of(show, base + k.at * scale) - start).max(0.0), ..*k }).collect(),
        ..env.clone()
    };
    let mut added = 0;
    for (off, scale) in copies {
        for te in &t.events {
            let sb = off + te.start_beat * scale;
            let eb = (off + (te.start_beat + te.len_beats) * scale).min(b1);
            if sb >= b1 - EPS || eb <= sb + EPS {
                continue;
            }
            let start = u_of(show, sb);
            let event = Event {
                id: 0,
                start,
                len: u_of(show, eb) - start,
                source: EventSource::Cue { id: cues[&te.slot].clone() },
                time: te.time,
                end: te.end,
                modifiers: te.modifiers.clone(),
                envelopes: te.envelopes.iter().map(|env| keys(env, sb, scale, start)).collect(),
                scan: te.scan.clone(),
                ..Default::default()
            };
            out.tracks[targets[te.track as usize]].events.push(event);
            added += 1;
        }
        if let Some(bi) = bus {
            let eb = (off + tlen * scale).min(b1);
            if eb > off + EPS {
                let start = u_of(show, off);
                let event = Event {
                    id: 0,
                    start,
                    len: u_of(show, eb) - start,
                    source: EventSource::Cue { id: String::new() },
                    envelopes: t.bus_envelopes.iter().map(|env| keys(env, off, scale, start)).collect(),
                    ..Default::default()
                };
                out.tracks[bi].events.push(event);
                added += 1;
            }
        }
    }
    out.sanitize();
    Ok(Applied { show: out, added, removed })
}

/// *Enregistrer la sélection comme modèle*: the events `ids` of `show`
/// become a template starting on the bar of the first one. Each cue
/// becomes a slot (« A », « B »… in order of appearance); events of bus
/// tracks give the bus envelopes; events playing a look stored in the
/// show are left out (a warning each time).
pub fn from_selection(show: &Show, ids: &[u64], name: &str, description: &str) -> Result<(Template, Vec<String>)> {
    let picked: Vec<(usize, &Track, &Event)> =
        show.tracks.iter().enumerate().flat_map(|(i, t)| t.events.iter().filter(|e| ids.contains(&e.id)).map(move |e| (i, t, e))).collect();
    if picked.is_empty() {
        bail!("sélectionnez des événements à enregistrer");
    }
    let first = picked.iter().map(|(_, _, e)| e.start).fold(f64::INFINITY, f64::min);
    let bpb = meter_at(show, first);
    let origin = bar_start_beat(show, first);
    let first_cue_track = picked.iter().filter(|(_, t, _)| t.kind == TrackKind::Cues).map(|(i, _, _)| *i).min().unwrap_or(0);
    let rank = |i: usize| show.tracks[first_cue_track..i].iter().filter(|t| t.kind == TrackKind::Cues).count();
    let rel_keys = |env: &Envelope, at: f64, base: f64| Envelope {
        keys: env.keys.iter().map(|k| Key { at: (beat_of(show, at + k.at) - base).max(0.0), ..*k }).collect(),
        ..env.clone()
    };

    let mut t = Template { name: name.trim().to_string(), description: description.trim().to_string(), beats_per_bar: bpb, ..Default::default() };
    let mut slot_of: HashMap<&str, String> = HashMap::new();
    let (mut looks, mut end) = (0, 0.0f64);
    for (i, track, e) in &picked {
        let (sb, eb) = (beat_of(show, e.start) - origin, beat_of(show, e.start + e.len) - origin);
        if track.kind == TrackKind::Bus {
            for env in &e.envelopes {
                let rel = rel_keys(env, e.start, origin);
                match t.bus_envelopes.iter_mut().find(|b| b.target == env.target && b.mode == env.mode) {
                    Some(b) => b.keys.extend(rel.keys),
                    None => t.bus_envelopes.push(rel),
                }
            }
            end = end.max(eb);
            continue;
        }
        let EventSource::Cue { id } = &e.source else {
            looks += 1;
            continue;
        };
        let slot = match slot_of.get(id.as_str()) {
            Some(s) => s.clone(),
            None => {
                if t.slots.len() >= MAX_SLOTS {
                    bail!("trop de cues différents ({} emplacements au plus)", MAX_SLOTS);
                }
                let s = ((b'A' + t.slots.len() as u8) as char).to_string();
                t.slots.push(s.clone());
                slot_of.insert(id, s.clone());
                s
            }
        };
        let start_beat = beat_of(show, e.start);
        t.events.push(TemplateEvent {
            track: rank(*i) as u8,
            start_beat: sb.max(0.0),
            len_beats: eb - sb,
            slot,
            time: e.time,
            end: e.end,
            modifiers: e.modifiers.clone(),
            envelopes: e.envelopes.iter().map(|env| rel_keys(env, e.start, start_beat)).collect(),
            scan: e.scan.clone(),
        });
        end = end.max(eb);
    }
    for b in &mut t.bus_envelopes {
        b.keys.sort_by(|a, b| a.at.total_cmp(&b.at));
        b.keys.truncate(MAX_ENV_KEYS);
    }
    t.bus_envelopes.truncate(MAX_ENVELOPES);
    if t.events.is_empty() && t.bus_envelopes.is_empty() {
        bail!("aucun événement de cue dans la sélection");
    }
    let bars = (end / bpb as f64 - EPS).ceil().max(1.0);
    if bars > MAX_LENGTH_BARS as f64 {
        bail!("sélection trop longue ({} mesures au plus)", MAX_LENGTH_BARS);
    }
    t.length_bars = bars as u16;
    t.validate()?;
    let mut warnings = Vec::new();
    if looks > 0 {
        warnings.push(format!("{looks} événement(s) avec un look du show laissé(s) de côté : seuls les cues deviennent des emplacements"));
    }
    Ok((t, warnings))
}

// ---- The built-in pack (T-166) ----------------------------------------
//
// Eight phrases of our own, written here in beats on a 4/4 grid. Every slot
// is filled from a page of our catalogue (`auto_pages`, « Auto » by
// default), so a phrase never names a cue: it is a shape (when things
// start, how long they last, which curves move them) that the operator
// fills with any cue. Only controls that LFOs may move are automated
// (`lfo::modulatable`); a brightness curve only dims, a latched strobe in
// an event goes through the strobe limiter like the operator's own, and
// nothing here touches the arm state.

/// Built-in palettes (`live::builtin_palettes`), by index.
const PAL_COLD: usize = 0;
const PAL_WARM: usize = 1;
const PAL_FIRE: usize = 2;
const PAL_OCEAN: usize = 3;
const PAL_NEON: usize = 4;

/// An envelope through `keys` (beats, value), eased as given.
fn curve(target: &str, mode: EnvMode, keys: &[(f64, f32)], ease: KeyEase) -> Envelope {
    Envelope { target: target.into(), mode, keys: keys.iter().map(|&(at, value)| Key { at, value, ease }).collect() }
}

/// An event of slot `slot` on template track `track`, beats `start..start+len`.
fn at(track: u8, slot: &str, start: f64, len: f64) -> TemplateEvent {
    TemplateEvent { track, start_beat: start, len_beats: len, slot: slot.into(), ..Default::default() }
}

impl TemplateEvent {
    /// A latched strobe synced on the tempo, one flash every `beats`.
    fn strobe(mut self, beats: f32, duty: f32) -> Self {
        self.modifiers.strobe = Strobe { on: true, hold: false, rate: Rate::Beats(beats), duty };
        self
    }
    /// The whole frame or each stroke takes the next colour of `palette`
    /// every `step` beats.
    fn chase(mut self, palette: usize, step: f32, spread: ChaseSpread) -> Self {
        self.modifiers.color = ColorOverride::Chase { palette, step: Rate::Beats(step), spread };
        self
    }
    fn speed(mut self, speed: f32) -> Self {
        self.modifiers.speed = speed;
        self
    }
    fn with(mut self, env: Envelope) -> Self {
        self.envelopes.push(env);
        self
    }
}

/// A 4/4 template of `bars` bars; `slots` are (slot, suggested page).
fn phrase(name: &str, description: &str, bars: u16, slots: &[(&str, &str)], events: Vec<TemplateEvent>, bus: Vec<Envelope>) -> Template {
    Template {
        name: name.into(),
        description: description.into(),
        length_bars: bars,
        beats_per_bar: 4,
        slots: slots.iter().map(|(s, _)| s.to_string()).collect(),
        events,
        bus_envelopes: bus,
        auto_pages: slots.iter().map(|(s, p)| (s.to_string(), p.to_string())).collect(),
    }
}

/// A build-up over `bars` bars: slot A grows and speeds up (bus curves),
/// slot B fades in on the second half, and the last bar strobes. From 16
/// bars on, the strobe starts a bar earlier at half the rate.
fn build_up(name: &str, description: &str, bars: u16) -> Template {
    let beats = bars as f64 * 4.0;
    let half = beats / 2.0;
    let mut events = Vec::new();
    if bars >= 16 {
        events.push(at(0, "A", 0.0, beats - 8.0));
        events.push(at(0, "A", beats - 8.0, 4.0).strobe(0.5, 0.5));
    } else {
        events.push(at(0, "A", 0.0, beats - 4.0));
    }
    events.push(at(0, "A", beats - 4.0, 4.0).strobe(0.25, 0.4));
    events.push(at(1, "B", half, half).with(curve("master.brightness", EnvMode::Absolute, &[(0.0, 0.3), (half, 1.0)], KeyEase::EaseIn)));
    let bus = vec![
        curve("master.size", EnvMode::Relative, &[(0.0, 0.5), (beats, 1.0)], KeyEase::EaseIn),
        curve("master.speed", EnvMode::Relative, &[(0.0, 0.6), (beats, 2.0)], KeyEase::EaseIn),
    ];
    phrase(name, description, bars, &[("A", "Tunnels"), ("B", "Faisceaux")], events, bus)
}

/// Whether `name` is a built-in template's (read-only) name.
pub fn is_builtin(name: &str) -> bool {
    builtin().iter().any(|b| b.name == name.trim())
}

/// The built-in templates (T-166), all our own, written procedurally from
/// the shapes above; their slots are filled from our catalogue pages.
/// Their names can't be taken by a user template and they can't be deleted.
pub fn builtin() -> Vec<Template> {
    let drop = {
        // Beams with a colour chase on the beat, a second beam over the
        // last 4 bars; the second track flashes on every bar's first beat.
        let mut events = vec![at(0, "A", 0.0, 48.0).chase(PAL_NEON, 1.0, ChaseSpread::Whole), at(0, "B", 48.0, 16.0).chase(PAL_NEON, 0.5, ChaseSpread::Whole)];
        events.extend((0..16).map(|bar| at(1, "C", bar as f64 * 4.0, 1.0)));
        phrase(
            "Drop 16",
            "Faisceaux en chenillard de couleurs au temps, un flash sur le premier temps de chaque mesure (deuxième piste), relance sur les 4 dernières mesures.",
            16,
            &[("A", "Faisceaux"), ("B", "Faisceaux"), ("C", "Géométrie")],
            events,
            vec![curve("master.size", EnvMode::Relative, &[(0.0, 1.0), (48.0, 1.0), (64.0, 1.15)], KeyEase::Smooth)],
        )
    };
    let pause = phrase(
        "Pause 8",
        "Un graphique lent en couleurs froides, à 60 % de luminosité, qui respire doucement : pour un break.",
        8,
        &[("A", "Vagues")],
        vec![at(0, "A", 0.0, 32.0).speed(0.4).chase(PAL_COLD, 8.0, ChaseSpread::Stroke)],
        vec![
            curve("master.brightness", EnvMode::Absolute, &[(0.0, 0.6), (32.0, 0.6)], KeyEase::Linear),
            curve("master.size", EnvMode::Relative, &[(0.0, 0.9), (16.0, 1.0), (32.0, 0.9)], KeyEase::Smooth),
        ],
    );
    let verse = phrase(
        "Couplet 16",
        "Deux cues qui alternent toutes les 4 mesures (A B A B), avec une lente rotation qui revient à mi-phrase.",
        16,
        &[("A", "Géométrie"), ("B", "Géométrie")],
        (0..4).map(|i| at(0, if i % 2 == 0 { "A" } else { "B" }, i as f64 * 16.0, 16.0)).collect(),
        vec![curve("master.rot_z.angle", EnvMode::Absolute, &[(0.0, 0.0), (32.0, 45.0), (64.0, 0.0)], KeyEase::Smooth)],
    );
    let outro = phrase(
        "Sortie 8",
        "Le cue ralentit et s'éteint en fondu jusqu'au noir sur 8 mesures.",
        8,
        &[("A", "Abstraits")],
        vec![at(0, "A", 0.0, 32.0)],
        vec![
            curve("master.brightness", EnvMode::Absolute, &[(0.0, 1.0), (32.0, 0.0)], KeyEase::EaseOut),
            curve("master.speed", EnvMode::Relative, &[(0.0, 1.0), (32.0, 0.3)], KeyEase::Linear),
        ],
    );
    let colours = phrase(
        "Chenillard couleurs 4",
        "Les traits du cue changent de couleur à chaque demi-temps, une palette par mesure : néon, froid, feu, océan.",
        4,
        &[("A", "Balayages")],
        [PAL_NEON, PAL_COLD, PAL_FIRE, PAL_OCEAN].iter().enumerate().map(|(bar, &p)| at(0, "A", bar as f64 * 4.0, 4.0).chase(p, 0.5, ChaseSpread::Stroke)).collect(),
        Vec::new(),
    );
    let finale = phrase(
        "Stroboscope final 1",
        "Une mesure de stroboscope qui accélère (au demi-temps puis au quart de temps) en couleurs chaudes, pour finir une phrase.",
        1,
        &[("A", "Faisceaux")],
        vec![
            at(0, "A", 0.0, 2.0).strobe(0.5, 0.5).chase(PAL_WARM, 1.0, ChaseSpread::Whole),
            at(0, "A", 2.0, 2.0).strobe(0.25, 0.4).chase(PAL_WARM, 0.5, ChaseSpread::Whole),
        ],
        vec![curve("master.size", EnvMode::Relative, &[(0.0, 1.0), (4.0, 1.2)], KeyEase::EaseIn)],
    );
    vec![
        build_up("Montée 8 mesures", "Le cue grandit et accélère sur 8 mesures, un faisceau entre en fondu à mi-phrase, stroboscope sur la dernière mesure.", 8),
        build_up(
            "Montée 16 mesures",
            "La même montée sur 16 mesures : taille et vitesse qui montent, faisceau en fondu sur la seconde moitié, stroboscope sur les deux dernières mesures.",
            16,
        ),
        drop,
        pause,
        verse,
        outro,
        colours,
        finale,
    ]
}

/// A template as listed: its content and where it comes from.
#[derive(Serialize)]
pub struct TemplateView {
    #[serde(flatten)]
    pub template: Template,
    pub builtin: bool,
}

/// `studio-data/templates/`, one JSON file per template.
pub struct TemplateStore {
    dir: PathBuf,
}

impl TemplateStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        let name = name.trim();
        if !valid_show_name(name) {
            bail!("nom de modèle invalide (lettres, chiffres, espaces, - et _ seulement)");
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// A damaged or invalid file is moved to the quarantine, as a show is.
    pub fn load(&self, name: &str) -> Result<Template> {
        let path = self.path(name)?;
        let quarantine = self.dir.parent().unwrap_or(std::path::Path::new(".")).join(crate::datafile::QUARANTINE);
        let mut t: Template = match crate::datafile::load(&path, &quarantine, false) {
            crate::datafile::Loaded::Ok(t) => t,
            crate::datafile::Loaded::Missing => bail!("modèle introuvable : {}", name.trim()),
            crate::datafile::Loaded::Quarantined(w) => bail!("modèle illisible : {}", w.message_fr()),
        };
        t.name = name.trim().to_string();
        if let Err(e) = t.validate() {
            let w = crate::datafile::set_aside(&path, &quarantine, &format!("{e:#}"), false);
            bail!("modèle illisible : {}", w.message_fr());
        }
        Ok(t)
    }

    /// User templates, sorted by name.
    pub fn list(&self) -> Vec<Template> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut list: Vec<Template> = entries
            .filter_map(|e| {
                let path = e.ok()?.path();
                (path.extension()? == "json").then_some(())?;
                self.load(path.file_stem()?.to_str()?).ok()
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Built-in templates first, then the user's (a user file named like a
    /// built-in, from before the pack, is hidden: the built-in wins).
    pub fn all(&self) -> Vec<TemplateView> {
        let builtins = builtin();
        let user: Vec<TemplateView> =
            self.list().into_iter().filter(|t| !builtins.iter().any(|b| b.name == t.name)).map(|template| TemplateView { template, builtin: false }).collect();
        builtins.into_iter().map(|template| TemplateView { template, builtin: true }).chain(user).collect()
    }

    /// A template by name, built-in or the user's.
    pub fn find(&self, name: &str) -> Result<Template> {
        match builtin().into_iter().find(|t| t.name == name.trim()) {
            Some(t) => Ok(t),
            None => self.load(name),
        }
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).map(|p| p.exists()).unwrap_or(false)
    }

    pub fn save(&self, t: &Template) -> Result<()> {
        t.validate()?;
        if is_builtin(&t.name) {
            bail!("« {} » est un modèle intégré : choisissez un autre nom", t.name);
        }
        let path = self.path(&t.name)?;
        std::fs::create_dir_all(&self.dir).with_context(|| format!("failed to create {}", self.dir.display()))?;
        crate::datafile::replace(&path, serde_json::to_string_pretty(t)?.as_bytes()).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        if is_builtin(name) {
            bail!("un modèle intégré ne se supprime pas");
        }
        let path = self.path(name)?;
        if !path.exists() {
            bail!("modèle introuvable : {}", name.trim());
        }
        std::fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TempoPoint;

    fn preset(id: &str, category: &'static str) -> Preset {
        Preset { id: id.into(), name: id.into(), category, settings: Default::default(), tag: None }
    }

    fn presets() -> Vec<Preset> {
        vec![preset("a", "Faisceaux"), preset("b", "Faisceaux"), preset("c", "Tunnels")]
    }

    /// 8 bars in 4/4: A on track 0 at bar 1 (4 beats), B on track 1 at
    /// bar 5 (8 beats), an envelope node 2 beats into A.
    fn eight_bars() -> Template {
        let env = Envelope { target: "size".into(), keys: vec![Key { at: 0.0, value: 0.5, ..Default::default() }, Key { at: 2.0, value: 1.0, ..Default::default() }], ..Default::default() };
        Template {
            name: "Phrase".into(),
            length_bars: 8,
            slots: vec!["A".into(), "B".into()],
            events: vec![
                TemplateEvent { track: 0, start_beat: 0.0, len_beats: 4.0, slot: "A".into(), envelopes: vec![env], ..Default::default() },
                TemplateEvent { track: 1, start_beat: 16.0, len_beats: 8.0, slot: "B".into(), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    fn choices() -> HashMap<String, SlotChoice> {
        HashMap::from([("A".into(), SlotChoice::Cue("a".into())), ("B".into(), SlotChoice::Cue("c".into()))])
    }

    fn seconds_show(bpm: f32) -> Show {
        let mut s = Show::new_empty("S", TimeBase::Seconds);
        s.tempo_map = vec![TempoPoint { at_s: 0.0, bpm, beats_per_bar: 4 }];
        s
    }

    fn spans(show: &Show, track: usize) -> Vec<(f64, f64)> {
        show.tracks[track].events.iter().map(|e| ((e.start * 1e6).round() / 1e6, (e.len * 1e6).round() / 1e6)).collect()
    }

    fn opts(from: f64, to: f64, fit: Fit) -> ApplyOptions {
        ApplyOptions { from, to, fit, slots: choices(), ..Default::default() }
    }

    #[test]
    fn repeat_lays_exact_copies_aligned_on_bars() {
        // 120 BPM: a bar is 2 s; 16 bars from bar 2 (2 s) to bar 18 (34 s).
        let show = seconds_show(120.0);
        let r = render(&show, &eight_bars(), &opts(2.0, 34.0, Fit::Repeat), &presets()).unwrap();
        assert_eq!(r.added, 4);
        assert_eq!(spans(&r.show, 0), [(2.0, 2.0), (18.0, 2.0)]);
        assert_eq!(spans(&r.show, 1), [(10.0, 4.0), (26.0, 4.0)]);
        let a = &r.show.tracks[0].events;
        assert_eq!(a[0].source, EventSource::Cue { id: "a".into() });
        assert_eq!(a[0].envelopes, a[1].envelopes, "two exact copies");
        assert_eq!(a[0].envelopes[0].keys[1].at, 1.0, "2 beats = 1 s at 120 BPM");
    }

    #[test]
    fn stretch_multiplies_positions_by_the_ratio() {
        let show = Show::new_empty("T", TimeBase::Beats);
        let r = render(&show, &eight_bars(), &opts(0.0, 64.0, Fit::Stretch), &presets()).unwrap();
        assert_eq!(spans(&r.show, 0), [(0.0, 8.0)]);
        assert_eq!(spans(&r.show, 1), [(32.0, 16.0)]);
        assert_eq!(r.show.tracks[0].events[0].envelopes[0].keys[1].at, 4.0, "nodes stretched too");
    }

    #[test]
    fn crop_cuts_at_the_end_of_the_region_and_repeat_cuts_the_last_copy() {
        let show = Show::new_empty("T", TimeBase::Beats);
        let r = render(&show, &eight_bars(), &opts(0.0, 20.0, Fit::Crop), &presets()).unwrap();
        assert_eq!((spans(&r.show, 0), spans(&r.show, 1)), (vec![(0.0, 4.0)], vec![(16.0, 4.0)]));
        let r = render(&show, &eight_bars(), &opts(0.0, 36.0, Fit::Repeat), &presets()).unwrap();
        assert_eq!(spans(&r.show, 0), [(0.0, 4.0), (32.0, 4.0)]);
        assert_eq!(spans(&r.show, 1), [(16.0, 8.0)]);
    }

    #[test]
    fn a_saved_selection_reapplied_elsewhere_gives_the_same_events_shifted() {
        let mut show = seconds_show(128.0);
        let bar = 60.0 / 128.0 * 4.0;
        let env = Envelope { target: "size".into(), keys: vec![Key { at: 0.5, value: 0.2, ..Default::default() }], ..Default::default() };
        show.tracks[0].events = vec![
            Event { id: 1, start: bar, len: bar * 2.0, source: EventSource::Cue { id: "a".into() }, envelopes: vec![env], ..Default::default() },
            Event { id: 2, start: bar * 3.0, len: bar, source: EventSource::Cue { id: "b".into() }, ..Default::default() },
        ];
        show.tracks[1].events = vec![Event { id: 3, start: bar * 1.5, len: bar, source: EventSource::Cue { id: "a".into() }, ..Default::default() }];
        show.sanitize();
        let (t, warnings) = from_selection(&show, &[1, 2, 3], "Ma phrase", "").unwrap();
        assert!(warnings.is_empty());
        assert_eq!((t.slots.len(), t.length_bars, t.events[0].start_beat), (2, 3, 0.0), "starts on the bar of the first event");
        assert_eq!(t.events.iter().filter(|e| e.slot == "A").count(), 2, "one slot per cue");

        let slots = HashMap::from([("A".into(), SlotChoice::Cue("a".into())), ("B".into(), SlotChoice::Cue("b".into()))]);
        let o = ApplyOptions { from: bar * 9.0, to: bar * 12.0, fit: Fit::Crop, slots, ..Default::default() };
        let r = render(&show, &t, &o, &presets()).unwrap();
        for ti in 0..2 {
            let old: Vec<&Event> = show.tracks[ti].events.iter().collect();
            let new: Vec<&Event> = r.show.tracks[ti].events.iter().filter(|e| e.start >= bar * 9.0 - 1e-9).collect();
            assert_eq!(old.len(), new.len());
            for (o, n) in old.iter().zip(new) {
                assert!((n.start - o.start - bar * 8.0).abs() < 1e-9 && (n.len - o.len).abs() < 1e-9, "{} vs {}", n.start, o.start);
                assert_eq!((&n.source, n.envelopes.len()), (&o.source, o.envelopes.len()));
                for (ne, oe) in n.envelopes.iter().zip(&o.envelopes) {
                    assert!((ne.keys[0].at - oe.keys[0].at).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn events_laid_in_seconds_stay_put_when_the_bpm_changes() {
        let show = seconds_show(120.0);
        let mut r = render(&show, &eight_bars(), &opts(0.0, 16.0, Fit::Stretch), &presets()).unwrap().show;
        let before = spans(&r, 1);
        r.tempo_map = vec![TempoPoint { at_s: 0.0, bpm: 120.0, beats_per_bar: 4 }, TempoPoint { at_s: 4.0, bpm: 90.0, beats_per_bar: 4 }];
        r.sanitize();
        assert_eq!(spans(&r, 1), before, "seconds are the reference");
        // Laid after the change, the beats follow the new tempo map.
        // B at beat 16: 8 beats in the first 4 s, then 8 at 90 BPM.
        let mut fresh = r.clone();
        fresh.tracks.iter_mut().for_each(|t| t.events.clear());
        let r2 = render(&fresh, &eight_bars(), &opts(0.0, 30.0, Fit::Crop), &presets()).unwrap().show;
        let (start, len) = (r2.tracks[1].events[0].start, r2.tracks[1].events[0].len);
        assert!((start - (4.0 + 8.0 * 60.0 / 90.0)).abs() < 1e-9 && (len - 8.0 * 60.0 / 90.0).abs() < 1e-9, "{start} {len}");
    }

    #[test]
    fn auto_slots_pick_different_cues_of_the_page() {
        let show = Show::new_empty("T", TimeBase::Beats);
        let slots = HashMap::from([("A".into(), SlotChoice::Auto("Faisceaux".into())), ("B".into(), SlotChoice::Auto("Faisceaux".into()))]);
        let r = render(&show, &eight_bars(), &ApplyOptions { from: 0.0, to: 32.0, slots, ..Default::default() }, &presets()).unwrap();
        let ids: Vec<_> = r.show.cue_ids().into_iter().collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|id| ["a", "b"].contains(id)));
        let bad = HashMap::from([("A".into(), SlotChoice::Auto("Nope".into())), ("B".into(), SlotChoice::Cue("a".into()))]);
        assert!(render(&show, &eight_bars(), &ApplyOptions { from: 0.0, to: 32.0, slots: bad, ..Default::default() }, &presets()).is_err());
        let missing = HashMap::from([("A".into(), SlotChoice::Cue("a".into()))]);
        let e = render(&show, &eight_bars(), &ApplyOptions { from: 0.0, to: 32.0, slots: missing, ..Default::default() }, &presets()).unwrap_err();
        assert!(format!("{e:#}").contains("emplacement B"));
    }

    #[test]
    fn locked_tracks_bus_tracks_and_replace() {
        let mut show = Show::new_empty("T", TimeBase::Beats);
        show.tracks[0].events.push(Event { id: 9, start: 4.0, len: 2.0, source: EventSource::Cue { id: "a".into() }, ..Default::default() });
        show.tracks[1].locked = true;
        let e = render(&show, &eight_bars(), &opts(0.0, 32.0, Fit::Crop), &presets()).unwrap_err();
        assert!(format!("{e:#}").contains("verrouillée"));
        // A bus track in between is skipped; a third cue track is created.
        show.tracks[1].locked = false;
        show.tracks.insert(1, Track { name: "Bus".into(), kind: TrackKind::Bus, layer: 1, ..Default::default() });
        let mut t = eight_bars();
        t.bus_envelopes = vec![Envelope { target: "brightness".into(), keys: vec![Key { at: 32.0, value: 0.0, ..Default::default() }], ..Default::default() }];
        let r = render(&show, &t, &ApplyOptions { replace: true, ..opts(0.0, 64.0, Fit::Stretch) }, &presets()).unwrap();
        assert_eq!((r.added, r.removed), (3, 1));
        assert_eq!(spans(&r.show, 2), [(32.0, 16.0)], "B on the next cue track");
        assert_eq!(r.show.tracks[1].events.len(), 1, "the phrase's bus event on the existing bus of layer 1");
        assert_eq!(r.show.tracks[1].events[0].envelopes[0].keys[0].at, 64.0);
        assert!(r.show.tracks.iter().flat_map(|t| &t.events).all(|e| e.id != 0), "numbered");
        // Starting on the last track: the missing ones are made.
        let r = render(&show, &eight_bars(), &ApplyOptions { track: 2, ..opts(0.0, 32.0, Fit::Crop) }, &presets()).unwrap();
        assert_eq!(r.show.tracks.len(), 4);
        assert_eq!(r.show.tracks[3].kind, TrackKind::Cues);
    }

    #[test]
    fn a_selection_with_bus_and_look_events() {
        let mut show = Show::new_empty("T", TimeBase::Beats);
        show.tracks[0].events = vec![
            Event { id: 1, start: 6.0, len: 4.0, source: EventSource::Cue { id: "a".into() }, ..Default::default() },
            Event { id: 2, start: 10.0, len: 2.0, source: EventSource::Look { settings: Default::default() }, ..Default::default() },
        ];
        show.tracks.push(Track { name: "Bus".into(), kind: TrackKind::Bus, ..Default::default() });
        let env = Envelope { target: "size".into(), keys: vec![Key { at: 1.0, value: 0.3, ..Default::default() }], ..Default::default() };
        show.tracks[2].events = vec![Event { id: 3, start: 8.0, len: 4.0, source: EventSource::Cue { id: String::new() }, envelopes: vec![env], ..Default::default() }];
        let (t, warnings) = from_selection(&show, &[1, 2, 3], "Bus", "ma description").unwrap();
        assert_eq!(warnings.len(), 1, "the look is left out");
        assert_eq!((t.events.len(), t.events[0].start_beat, t.length_bars), (1, 2.0, 2), "the bus event ends the phrase at beat 8");
        assert_eq!(t.bus_envelopes[0].keys[0].at, 5.0, "from the bar start (beat 4)");
        assert!(from_selection(&show, &[], "X", "").is_err());
        assert!(from_selection(&show, &[2], "X", "").is_err(), "nothing but a look");
    }

    #[test]
    fn validation_and_the_store() {
        let mut t = eight_bars();
        assert!(t.validate().is_ok());
        t.events[0].slot = "Z".into();
        assert!(t.validate().is_err());
        let mut t = eight_bars();
        t.events[1].start_beat = 32.0;
        assert!(t.validate().is_err(), "starts after the end");
        t = eight_bars();
        t.name = "../x".into();
        assert!(t.validate().is_err());

        let dir = std::env::temp_dir().join(format!("laser-studio-test-templates-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TemplateStore::new(dir.join("templates"));
        assert!(store.all().is_empty() || store.all().iter().all(|v| v.builtin));
        store.save(&eight_bars()).unwrap();
        assert!(store.exists("Phrase"));
        assert_eq!(store.find("Phrase").unwrap(), eight_bars());
        assert_eq!(store.list().len(), 1);
        std::fs::write(dir.join("templates/Cassé.json"), "{ nope").unwrap();
        assert_eq!(store.list().len(), 1, "a damaged file is set aside");
        store.delete("Phrase").unwrap();
        assert!(store.delete("Phrase").is_err());
        assert!(store.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each slot on its suggested page (« Auto »), as the dialog preselects.
    fn auto_choices(t: &Template) -> HashMap<String, SlotChoice> {
        t.slots.iter().map(|s| (s.clone(), SlotChoice::Auto(t.auto_pages[s].clone()))).collect()
    }

    #[test]
    fn the_builtin_pack_is_eight_valid_french_phrases_on_our_pages() {
        let pack = builtin();
        let names: Vec<&str> = pack.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["Montée 8 mesures", "Montée 16 mesures", "Drop 16", "Pause 8", "Couplet 16", "Sortie 8", "Chenillard couleurs 4", "Stroboscope final 1"]
        );
        let catalog = crate::presets::catalog();
        let reg = crate::controls::ControlRegistry::build(&catalog);
        for t in &pack {
            t.validate().unwrap_or_else(|e| panic!("{}: {e:#}", t.name));
            assert!(t.description.chars().count() > 20, "{}: a French description", t.name);
            assert!(!t.slots.is_empty() && t.slots.iter().all(|s| t.auto_pages.contains_key(s)), "{}: a page per slot", t.name);
            for page in t.auto_pages.values() {
                assert!(crate::presets::CATEGORIES.contains(&page.as_str()) && catalog.iter().any(|p| p.category == page), "{}: page {page}", t.name);
            }
            for env in t.bus_envelopes.iter().chain(t.events.iter().flat_map(|e| &e.envelopes)) {
                assert!(crate::lfo::modulatable(&reg, &env.target).is_some(), "{}: {} is not automatable", t.name, env.target);
            }
            assert!(t.events.iter().all(|e| e.start_beat + e.len_beats <= t.beats() + EPS), "{}: inside the phrase", t.name);
            assert!(t.events.iter().all(|e| !e.modifiers.strobe.hold && !e.modifiers.black && !e.modifiers.freeze), "{}: nothing momentary", t.name);
            // Through JSON (the list route) and back, unchanged.
            let back: Template = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
            assert_eq!(&back, t);
        }
        // Montées: size and speed rise; the last bar strobes.
        for t in &pack[..2] {
            let last = t.events.iter().filter(|e| e.track == 0).max_by(|a, b| a.start_beat.total_cmp(&b.start_beat)).unwrap();
            assert!(last.modifiers.strobe.on && (last.start_beat - (t.beats() - 4.0)).abs() < EPS);
            for target in ["master.size", "master.speed"] {
                let env = t.bus_envelopes.iter().find(|e| e.target == target).unwrap();
                assert!(env.keys.last().unwrap().value > env.keys[0].value, "{target} rises");
            }
        }
        // Drop 16: a flash on beat 1 of every bar on the second track.
        let flashes: Vec<f64> = pack[2].events.iter().filter(|e| e.track == 1).map(|e| e.start_beat).collect();
        assert_eq!(flashes, (0..16).map(|b| b as f64 * 4.0).collect::<Vec<_>>());
        // Sortie 8 ends in black.
        let fade = pack[5].bus_envelopes.iter().find(|e| e.target == "master.brightness").unwrap();
        assert_eq!(fade.keys.last().unwrap().value, 0.0);
    }

    #[test]
    fn every_builtin_lays_on_8_16_and_32_bars_with_existing_cues_inside_the_region() {
        let catalog = crate::presets::catalog();
        let reg = crate::controls::ControlRegistry::build(&catalog);
        for base in [TimeBase::Beats, TimeBase::Seconds] {
            // An empty 64-bar show (128 BPM in Secondes).
            let mut show = Show::new_empty("Vide", base);
            if base == TimeBase::Seconds {
                show.tempo_map = vec![TempoPoint { at_s: 0.0, bpm: 128.0, beats_per_bar: 4 }];
            }
            let u_at_bar = |bar: f64| match base {
                TimeBase::Beats => bar * 4.0,
                TimeBase::Seconds => bar * 4.0 * 60.0 / 128.0,
            };
            for t in builtin() {
                for bars in [8.0, 16.0, 32.0] {
                    for fit in [Fit::Stretch, Fit::Repeat, Fit::Crop] {
                        let (from, to) = (u_at_bar(16.0), u_at_bar(16.0 + bars));
                        let o = ApplyOptions { from, to, fit, slots: auto_choices(&t), ..Default::default() };
                        let r = render(&show, &t, &o, &catalog).unwrap_or_else(|e| panic!("{} {bars} {fit:?}: {e:#}", t.name));
                        let what = format!("{} on {bars} bars, {fit:?}, {base:?}", t.name);
                        assert!(r.added > 0, "{what}");
                        r.show.validate_edit(|id| catalog.iter().any(|p| p.id == id)).unwrap_or_else(|e| panic!("{what}: {e:#}"));
                        assert!(r.show.unknown_envelope_targets(|id| crate::lfo::modulatable(&reg, id).is_some()).is_empty(), "{what}");
                        for e in r.show.tracks.iter().flat_map(|t| &t.events) {
                            assert!(e.start >= from - 1e-9 && e.start + e.len <= to + 1e-9 && e.len > 0.0, "{what}: {} + {}", e.start, e.len);
                            // Cut copies keep their later nodes (never read
                            // after the event's end); stretched ones fit.
                            if fit == Fit::Stretch {
                                assert!(e.envelopes.iter().flat_map(|env| &env.keys).all(|k| k.at <= e.len + 1e-9), "{what}: nodes inside the event");
                            }
                            let EventSource::Cue { id } = &e.source else { panic!("{what}: a look") };
                            let bus = r.show.tracks.iter().any(|tr| tr.kind == TrackKind::Bus && tr.events.iter().any(|b| b.id == e.id));
                            assert!(bus || catalog.iter().any(|p| &p.id == id), "{what}: unknown cue {id}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn builtins_are_read_only() {
        let dir = std::env::temp_dir().join(format!("laser-studio-test-templates-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TemplateStore::new(dir.join("templates"));
        let all = store.all();
        assert_eq!((all.len(), all.iter().all(|v| v.builtin)), (8, true));
        let mut mine = builtin().remove(2);
        assert!(store.save(&mine).is_err(), "can't be overwritten");
        assert!(store.delete("Drop 16").is_err(), "can't be deleted");
        assert_eq!(store.find("Drop 16").unwrap(), builtin()[2]);
        // A file of the same name (from before the pack) is hidden.
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        mine.description = "ancien".into();
        std::fs::write(dir.join("templates/Drop 16.json"), serde_json::to_string(&mine).unwrap()).unwrap();
        assert_eq!(store.all().len(), 8);
        assert_eq!(store.find("Drop 16").unwrap(), builtin()[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
