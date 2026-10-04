//! Automatic show from the song (T-167): « Générer un show depuis le
//! morceau » in TIMELINE.
//!
//! The song is decoded in isolation like every decode (audio/isolate.rs),
//! then run offline through the very analysers of the live input
//! (audio/analysis.rs: onsets, tempo and beat tracking, sections), hop by
//! hop as the audio bench does. From what they report over the whole song:
//!
//! - **tempo**: the median BPM of the hops where the tracker is *Locked*,
//!   refined by a least-squares fit of a regular grid on the tracked beats
//!   (period and phase);
//! - **downbeats**: the beat of the bar most section changes and drops land
//!   on (4/4), else the first beat that sounds;
//! - **sections**, bar by bar: the detector's section (majority over the
//!   bar), split again where the bar energy jumps by 6 dB or more between
//!   the 4 bars before and the 4 bars after (a Foote-like novelty on the
//!   energy curve), short pieces merged, then labelled *intro / couplet /
//!   montée / drop / pause / fin* from the detector and the energy; a drop
//!   starts on the detector's drop (snapped onto the grid) when it has one.
//!
//! The proposal follows the track grammar of docs/research/festival-looks.md
//! § 2.1: calm looks fading in and out in the intro, breaks and outro, the
//! groove looks changing every 8 bars in a verse, build-up looks getting
//! denser in two halves, a blackout on the beat before a drop, and the
//! strongest beam looks on the drop with a beat-stab accent on its first
//! bar. Cues are picked from our own catalogue by category and generator
//! (presets.rs), with a seeded generator: the same file and seed give the
//! same show; *Autre proposition* draws another seed.
//!
//! Everything runs on its own thread: never on the HTTP worker nor the
//! engine. The studio lock is held only to read the catalogue at the start
//! and to open the show at the end (heartbeats are never delayed). The
//! analysis checks a cancel flag and a time budget every few hundred hops.
//! The show opens unsaved (*Enregistrer* keeps it) and nothing here touches
//! the arm state.

use crate::audio::analysis::{to_db, Analyzer, HOP};
use crate::audio::bpm::DetectState;
use crate::audio::decode::Decoded;
use crate::audio::onsets::Onsets;
use crate::audio::spectrum::AnalysisConfig;
use crate::engine::{Content, GateMode, Section};
use crate::presets::Preset;
use crate::timeline::{AudioRef, Dur, Event, EventSource, Marker, Show, TempoPoint, TimeBase, TimeMode, Track, Transition, TransitionKind};
use crate::Shared;
use anyhow::{bail, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The analysis gives up after this long (a 4-minute song takes a few
/// seconds in a release build).
pub const ANALYSIS_BUDGET: Duration = Duration::from_secs(90);
/// Shortest song worth a show (s).
pub const MIN_SONG_S: f64 = 8.0;
/// The cancel flag and the budget are checked this often (hops).
const CHECK_HOPS: usize = 256;
/// The shows proposed are in 4/4.
const BEATS_PER_BAR: usize = 4;
/// Assumed when no tempo locks (no pulse in the song).
const DEFAULT_BPM: f32 = 120.0;
/// A bar-energy jump this large (dB, 4 bars against 4) is a section change.
const NOVELTY_DB: f32 = 6.0;
const NOVELTY_BARS: usize = 4;
/// Sections shorter than this (bars) are merged into their neighbour.
const MIN_SECTION_BARS: usize = 2;
/// A section this close to the loudest (dB) can be a drop.
const DROP_NEAR_LOUDEST_DB: f32 = 4.0;
/// A drop is this much louder (dB) than what comes before it, and the
/// groove that follows it continues it while within this of it.
const DROP_RISE_DB: f32 = 6.0;
const DROP_KEEP_DB: f32 = 3.0;

// ---------- the analysis ----------

/// What a section of the song is, for the show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    Intro,
    Verse,
    Buildup,
    Drop,
    Break,
    Outro,
}

impl SectionKind {
    /// The marker's name.
    pub fn label(self) -> &'static str {
        match self {
            SectionKind::Intro => "Intro",
            SectionKind::Verse => "Couplet",
            SectionKind::Buildup => "Montée",
            SectionKind::Drop => "Drop",
            SectionKind::Break => "Pause",
            SectionKind::Outro => "Fin",
        }
    }

    /// The marker's colour (the studio's theme hues).
    pub fn color(self) -> [u8; 3] {
        match self {
            SectionKind::Intro => [139, 92, 246],
            SectionKind::Verse => [61, 220, 132],
            SectionKind::Buildup => [255, 176, 32],
            SectionKind::Drop => [255, 61, 127],
            SectionKind::Break => [61, 155, 255],
            SectionKind::Outro => [138, 144, 168],
        }
    }

    fn calm(self) -> bool {
        matches!(self, SectionKind::Intro | SectionKind::Break | SectionKind::Outro)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SongSection {
    pub start_s: f64,
    pub end_s: f64,
    pub kind: SectionKind,
    /// Its loudness against the rest of the song, 0..1.
    pub energy: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Analysis {
    pub duration_s: f64,
    pub bpm: f32,
    /// Share of the song where the tempo was locked, 0..1 (0: no pulse,
    /// `DEFAULT_BPM` assumed).
    pub confidence: f32,
    #[serde(skip)]
    pub beats_s: Vec<f64>,
    #[serde(skip)]
    pub downbeats_s: Vec<f64>,
    pub sections: Vec<SongSection>,
    /// The section detector's drops, as it reported them.
    pub drops_s: Vec<f64>,
}

impl Analysis {
    fn beat_s(&self) -> f64 {
        60.0 / self.bpm.max(1.0) as f64
    }
}

/// The analysis was cancelled (*Annuler*).
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("génération annulée")
    }
}

impl std::error::Error for Cancelled {}

/// Shared between a job's thread and the API: cancel, progress.
#[derive(Debug, Default)]
pub struct Control {
    cancel: AtomicBool,
    /// 0..=1000.
    progress: AtomicU32,
}

impl Control {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<()> {
        if self.cancelled() {
            return Err(Cancelled.into());
        }
        Ok(())
    }

    fn set_progress(&self, x: f64) {
        self.progress.store((x.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }

    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// Sample `frame` as mono.
#[inline]
fn mono(d: &Decoded, frame: usize) -> f32 {
    let ch = d.channels.max(1) as usize;
    let i = frame * ch;
    match (d.samples.get(i), ch) {
        (Some(&l), 1) => l as f32 / 32768.0,
        (Some(&l), _) => (l as f32 + d.samples.get(i + 1).copied().unwrap_or(l) as f32) / 65536.0,
        (None, _) => 0.0,
    }
}

/// What the analysers said over the whole song.
#[derive(Default)]
struct Pass {
    hop_s: f64,
    duration_s: f64,
    /// Mean square of every hop.
    power: Vec<f32>,
    locked_bpm: Vec<f32>,
    confident_bpm: Vec<f32>,
    /// Beats tracked while locked.
    beats: Vec<f64>,
    /// Section changes `(start, section)`, starts made non-decreasing.
    changes: Vec<(f64, Section)>,
    drops: Vec<f64>,
    first_onset: Option<f64>,
}

/// Runs the decoded song through the analysers, then reads tempo, grid and
/// sections from it. Errors: too short, cancelled (`Cancelled`), over
/// `budget`.
pub fn analyze(d: &Decoded, ctl: &Control, budget: Duration) -> Result<Analysis> {
    let started = Instant::now();
    let rate = d.sample_rate.max(1);
    let hops = d.frames() / HOP;
    let duration_s = d.duration_s();
    if duration_s < MIN_SONG_S {
        bail!("morceau trop court pour en tirer un show ({} s au moins)", MIN_SONG_S);
    }
    let mut a = Analyzer::with_config(rate, Onsets::default(), AnalysisConfig::default());
    let mut p = Pass { hop_s: HOP as f64 / rate as f64, duration_s, power: Vec::with_capacity(hops), ..Default::default() };
    let mut buf = [0f32; HOP];
    let (mut prev_beat, mut prev_drop, mut prev_section) = (0.0f64, 0u64, None);
    for h in 0..hops {
        if h % CHECK_HOPS == 0 {
            ctl.check()?;
            if started.elapsed() > budget {
                bail!("analyse trop longue (plus de {} s) : abandonnée", budget.as_secs());
            }
            ctl.set_progress(h as f64 / hops as f64);
        }
        let mut sum = 0.0f32;
        for (i, x) in buf.iter_mut().enumerate() {
            *x = mono(d, h * HOP + i);
            sum += *x * *x;
        }
        let at = ((h + 1) * HOP) as f64 / rate as f64;
        let m = a.process(&buf, at);
        p.power.push(sum / HOP as f32);
        let tempo = m.tempo;
        if tempo.bpm.is_finite() && tempo.bpm > 0.0 {
            if tempo.state == DetectState::Locked {
                p.locked_bpm.push(tempo.bpm);
            }
            if tempo.confidence >= crate::audio::bpm::UNLOCK_CONFIDENCE {
                p.confident_bpm.push(tempo.bpm);
            }
        }
        if tempo.beat_time > 0.0 && tempo.beat_time != prev_beat {
            prev_beat = tempo.beat_time;
            if tempo.state == DetectState::Locked {
                p.beats.push(tempo.beat_time);
            }
        }
        let s = m.sections;
        if prev_section != Some(s.section) {
            let floor = p.changes.last().map_or(0.0, |c| c.0);
            p.changes.push((s.start_t.min(at).max(floor), s.section));
            prev_section = Some(s.section);
        }
        if s.drop > prev_drop {
            prev_drop = s.drop;
            p.drops.push(s.last_drop_t);
        }
        if p.first_onset.is_none() && m.onsets.onset > 0 {
            p.first_onset = Some(m.onsets.last_onset_t);
        }
    }
    ctl.check()?;
    ctl.set_progress(1.0);
    Ok(p.finish())
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// A regular grid `t0 + k·period` fitted on tracked beats: indices
/// unwrapped from the gaps between beats, then least squares, twice,
/// leaving out beats more than 10 % of a beat off the first fit.
fn fit_grid(beats: &[f64], period0: f64) -> Option<(f64, f64)> {
    if beats.len() < 8 || period0.is_nan() || period0 <= 0.0 {
        return None;
    }
    let mut ks = Vec::with_capacity(beats.len());
    let mut k = 0i64;
    ks.push(0i64);
    for w in beats.windows(2) {
        k += ((w[1] - w[0]) / period0).round().max(1.0) as i64;
        ks.push(k);
    }
    let mut keep = vec![true; beats.len()];
    let mut fit = None;
    for _ in 0..2 {
        let pts: Vec<(f64, f64)> = ks.iter().zip(beats).zip(&keep).filter(|(_, &on)| on).map(|((&k, &t), _)| (k as f64, t)).collect();
        if pts.len() < 8 {
            break;
        }
        let n = pts.len() as f64;
        let (mk, mt) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
        let sxx: f64 = pts.iter().map(|p| (p.0 - mk).powi(2)).sum();
        let sxy: f64 = pts.iter().map(|p| (p.0 - mk) * (p.1 - mt)).sum();
        if sxx <= 0.0 {
            break;
        }
        let b = sxy / sxx;
        let a = mt - b * mk;
        fit = Some((a, b));
        for (i, (&k, &t)) in ks.iter().zip(beats).enumerate() {
            keep[i] = (t - (a + b * k as f64)).abs() < 0.1 * b;
        }
    }
    fit.filter(|&(a, b)| a.is_finite() && b > 0.0)
}

impl Pass {
    fn section_at(&self, t: f64) -> Section {
        let i = self.changes.partition_point(|c| c.0 <= t);
        i.checked_sub(1).map_or(Section::Silence, |i| self.changes[i].1)
    }

    /// Mean power over `[from, to)` (hops by their middle).
    fn power_in(&self, from: f64, to: f64) -> f32 {
        let a = ((from / self.hop_s - 0.5).ceil().max(0.0) as usize).min(self.power.len());
        let b = ((to / self.hop_s - 0.5).ceil().max(0.0) as usize).clamp(a, self.power.len());
        if b == a {
            return 0.0;
        }
        self.power[a..b].iter().sum::<f32>() / (b - a) as f32
    }

    fn finish(mut self) -> Analysis {
        let duration = self.duration_s;
        // Tempo.
        let hops = self.power.len().max(1);
        let locked = self.locked_bpm.len();
        let bpm0 = median(&mut self.locked_bpm).or_else(|| median(&mut self.confident_bpm));
        let confidence = (locked as f32 / hops as f32 * 1.25).min(1.0);
        let (mut t0, mut period) = match bpm0 {
            Some(bpm) => {
                let p0 = 60.0 / bpm as f64;
                match fit_grid(&self.beats, p0) {
                    // A fit far from the median is a bad unwrap: the median wins.
                    Some((a, b)) if ((b - p0) / p0).abs() < 0.03 => (a, b),
                    _ => (self.beats.first().copied().or(self.first_onset).unwrap_or(0.0), p0),
                }
            }
            None => (self.first_onset.unwrap_or(0.0), 60.0 / DEFAULT_BPM as f64),
        };
        let bpm = ((60.0 / period) * 100.0).round() / 100.0;
        let bpm = (bpm as f32).clamp(crate::tempo::MIN_BPM as f32, crate::tempo::MAX_BPM as f32);
        period = 60.0 / bpm as f64;
        t0 = t0.rem_euclid(period);
        let beats_s: Vec<f64> = (0..).map(|k| t0 + k as f64 * period).take_while(|&t| t < duration).collect();

        // Downbeats: the beat of the bar section changes and drops land on.
        let mut votes = [0.0f64; BEATS_PER_BAR];
        let mut vote = |t: f64, w: f64| {
            let n = ((t - t0) / period).round();
            if n >= 0.0 && (t - (t0 + n * period)).abs() < 0.35 * period {
                votes[n as usize % BEATS_PER_BAR] += w;
            }
        };
        if let Some(first) = self.first_onset {
            vote(first.max(t0), 0.5);
        }
        for w in self.changes.windows(2) {
            if w[1].1 != Section::Silence && w[0].1 != Section::Silence {
                vote(w[1].0, 1.0);
            }
        }
        for &d in &self.drops {
            vote(d, 2.0);
        }
        let phase = (0..BEATS_PER_BAR).fold(0, |best, j| if votes[j] > votes[best] { j } else { best });
        let downbeats_s: Vec<f64> = beats_s.iter().enumerate().filter(|(k, _)| k % BEATS_PER_BAR == phase).map(|(_, &t)| t).collect();

        let sections = self.sections(&downbeats_s, &beats_s, period);
        Analysis { duration_s: duration, bpm, confidence, beats_s, downbeats_s, sections, drops_s: self.drops.clone() }
    }

    /// Bars → segments → labelled sections.
    fn sections(&self, downbeats: &[f64], beats: &[f64], period: f64) -> Vec<SongSection> {
        let duration = self.duration_s;
        // Bars: from 0 to the first downbeat (a pick-up, joined to the
        // first bar when shorter than half a bar), then bar by bar.
        let mut bounds = vec![0.0];
        let half_bar = period * BEATS_PER_BAR as f64 / 2.0;
        for &d in downbeats {
            if d - bounds.last().unwrap() > 0.05 && (bounds.len() > 1 || d >= half_bar) {
                bounds.push(d);
            }
        }
        if duration - bounds.last().unwrap() > 0.05 {
            bounds.push(duration);
        } else {
            *bounds.last_mut().unwrap() = duration;
        }
        let bars: Vec<(f64, f64)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();
        if bars.is_empty() {
            return vec![SongSection { start_s: 0.0, end_s: duration, kind: SectionKind::Verse, energy: 1.0 }];
        }
        // The detector's section of each bar (majority of 8 points) and its energy.
        let kinds: Vec<Section> = bars
            .iter()
            .map(|&(a, b)| {
                let mut count = [0usize; 5];
                for i in 0..8 {
                    count[section_index(self.section_at(a + (b - a) * (i as f64 + 0.5) / 8.0))] += 1;
                }
                // Ties: the earlier kind in `SECTIONS` order (deterministic).
                SECTIONS[(0..5).fold(0, |best, j| if count[j] > count[best] { j } else { best })]
            })
            .collect();
        let db: Vec<f32> = bars.iter().map(|&(a, b)| to_db(self.power_in(a, b))).collect();

        // Novelty on the energy: 4 bars after against 4 bars before.
        let n = bars.len();
        let mean = |r: std::ops::Range<usize>| r.clone().map(|i| db[i]).sum::<f32>() / r.len().max(1) as f32;
        let nov: Vec<f32> = (0..n)
            .map(|i| {
                let (lo, hi) = (i.saturating_sub(NOVELTY_BARS), (i + NOVELTY_BARS).min(n));
                if i < 2 || hi - i < 2 {
                    0.0
                } else {
                    mean(i..hi) - mean(lo..i)
                }
            })
            .collect();
        let is_novel = |i: usize| {
            let v = nov[i].abs();
            v >= NOVELTY_DB && (i.saturating_sub(2)..(i + 3).min(n)).all(|j| nov[j].abs() < v || (nov[j].abs() == v && j >= i))
        };

        // Segments: [first bar, end bar), with the detector's section.
        let mut segs: Vec<(usize, usize, Section)> = Vec::new();
        for (i, &kind) in kinds.iter().enumerate() {
            match segs.last_mut() {
                Some(s) if s.2 == kind && !is_novel(i) => s.1 = i + 1,
                _ => segs.push((i, i + 1, kind)),
            }
        }
        // Short pieces join the one before (the first joins the next).
        let mut i = 0;
        while i < segs.len() && segs.len() > 1 {
            if segs[i].1 - segs[i].0 < MIN_SECTION_BARS {
                if i == 0 {
                    segs[1].0 = segs[0].0;
                    segs.remove(0);
                } else {
                    segs[i - 1].1 = segs[i].1;
                    segs.remove(i);
                }
                continue;
            }
            i += 1;
        }

        // Labels.
        let seg_db: Vec<f32> = segs.iter().map(|s| to_db(self.power_in(bars[s.0].0, bars[s.1 - 1].1))).collect();
        let loudest = seg_db.iter().copied().fold(f32::MIN, f32::max);
        let mut out: Vec<(usize, usize, SectionKind, f32)> = Vec::new();
        for (k, s) in segs.iter().enumerate() {
            let d = seg_db[k];
            let kind = match s.2 {
                Section::Drop => SectionKind::Drop,
                Section::Buildup => SectionKind::Buildup,
                Section::Break | Section::Silence => SectionKind::Break,
                Section::Normal => match out.last() {
                    Some(p) if p.2 == SectionKind::Drop && d >= p.3 - DROP_KEEP_DB => SectionKind::Drop,
                    Some(p) if d >= loudest - DROP_NEAR_LOUDEST_DB && (matches!(p.2, SectionKind::Break | SectionKind::Buildup) || d >= p.3 + DROP_RISE_DB) => SectionKind::Drop,
                    _ => SectionKind::Verse,
                },
            };
            out.push((s.0, s.1, kind, d));
        }
        if out.len() > 1 {
            if matches!(out[0].2, SectionKind::Verse | SectionKind::Break) {
                out[0].2 = SectionKind::Intro;
            }
            let last = out.len() - 1;
            if matches!(out[last].2, SectionKind::Verse | SectionKind::Break) {
                out[last].2 = SectionKind::Outro;
            }
        }
        // Neighbours of the same kind are one section.
        let mut merged: Vec<(usize, usize, SectionKind)> = Vec::new();
        for (a, b, kind, _) in out {
            match merged.last_mut() {
                Some(m) if m.2 == kind => m.1 = b,
                _ => merged.push((a, b, kind)),
            }
        }

        let mut sections: Vec<SongSection> = merged.iter().map(|&(a, b, kind)| SongSection { start_s: bars[a].0, end_s: bars[b - 1].1, kind, energy: 0.0 }).collect();
        // A drop starts on the detector's drop when it has one near.
        for i in 1..sections.len() {
            if sections[i].kind != SectionKind::Drop || sections[i - 1].kind == SectionKind::Drop {
                continue;
            }
            let start = sections[i].start_s;
            let bar = period * BEATS_PER_BAR as f64;
            let near = self.drops.iter().copied().filter(|d| (d - start).abs() <= bar).min_by(|a, b| (a - start).abs().total_cmp(&(b - start).abs()));
            if let Some(d) = near {
                let snapped = beats.iter().copied().min_by(|a, b| (a - d).abs().total_cmp(&(b - d).abs())).filter(|b| (b - d).abs() < period / 2.0).unwrap_or(d);
                if snapped > sections[i - 1].start_s + period && snapped < sections[i].end_s - period {
                    sections[i].start_s = snapped;
                    sections[i - 1].end_s = snapped;
                }
            }
        }
        // Energy, 0..1 between the quietest and the loudest section.
        let dbs: Vec<f32> = sections.iter().map(|s| to_db(self.power_in(s.start_s, s.end_s))).collect();
        let (lo, hi) = (dbs.iter().copied().fold(f32::MAX, f32::min), dbs.iter().copied().fold(f32::MIN, f32::max));
        for (s, d) in sections.iter_mut().zip(dbs) {
            s.energy = if hi - lo > 1.0 { ((d - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 1.0 };
        }
        sections
    }
}

const SECTIONS: [Section; 5] = [Section::Silence, Section::Normal, Section::Break, Section::Buildup, Section::Drop];

fn section_index(s: Section) -> usize {
    SECTIONS.iter().position(|&x| x == s).unwrap_or(1)
}

/// The default seed: a digest of the decoded song (same file, same show).
pub fn file_seed(d: &Decoded) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u64| {
        h ^= b;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    };
    eat(d.sample_rate as u64);
    eat(d.channels as u64);
    for &s in &d.samples {
        eat(s as u16 as u64);
    }
    h & 0xFFFF_FFFF
}

// ---------- the show ----------

/// Catalogue cues by the role they play in a show, our own choice from the
/// generator of each built-in cue (presets.rs). Pages the operator fills
/// (figures, ILDA, own cues), text and the microphone-driven « Audio » page
/// are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CuePools {
    /// Slow, soft looks: intro, breaks, outro.
    pub calm: Vec<String>,
    /// The groove: fans, cones, sweeps, tunnels.
    pub groove: Vec<String>,
    /// Rising density: tunnels, rings, chases, bursts.
    pub build: Vec<String>,
    /// The strongest beam looks.
    pub drop: Vec<String>,
    /// Beat stabs for the drop's first bar.
    pub hit: Vec<String>,
}

/// Pages the auto-show doesn't draw from. « Festival » repeats looks
/// that are already in the pools (T-110).
const LEFT_OUT: &[&str] = &["Audio", "Texte & horloge", crate::figures::CATEGORY, crate::ilda_library::CATEGORY, crate::presets::FESTIVAL];

impl CuePools {
    pub fn from_presets(presets: &[Preset]) -> Self {
        let mut pools = CuePools::default();
        for p in presets {
            if !crate::presets::CATEGORIES.contains(&p.category) || LEFT_OUT.contains(&p.category) {
                continue;
            }
            let Content::Generator { generator, .. } = &p.settings.content else { continue };
            let id = || p.id.clone();
            let stab = p.settings.gate != GateMode::Off;
            if stab && p.settings.strobe_div <= 0.0 {
                pools.hit.push(id());
            }
            if stab || p.settings.strobe_div > 0.0 {
                continue;
            }
            let g = generator.as_str();
            if matches!(g, "lissajous" | "spirograph" | "rose" | "flower" | "vortex" | "liquid_sky" | "sine_stack" | "helix" | "starfield" | "hot_beam") {
                pools.calm.push(id());
            }
            if matches!(g, "beam_fan" | "beam_circle" | "beam_wave" | "sweep" | "grid_scan" | "tunnel" | "polygon_tunnel" | "spiral_arms" | "starburst" | "beam_v") {
                pools.groove.push(id());
            }
            if matches!(g, "tunnel" | "polygon_tunnel" | "pulse_rings" | "bundle_burst" | "chase_fan" | "beam_wave" | "scissor") {
                pools.build.push(id());
            }
            if matches!(g, "chase_fan" | "scissor" | "fat_beams" | "lightning" | "beam_fan" | "beam_circle" | "beam_v" | "bundle_burst") {
                pools.drop.push(id());
            }
        }
        pools
    }

    fn for_kind(&self, kind: SectionKind) -> &[String] {
        let pool = match kind {
            SectionKind::Intro | SectionKind::Break | SectionKind::Outro => &self.calm,
            SectionKind::Verse => &self.groove,
            SectionKind::Buildup => &self.build,
            SectionKind::Drop => &self.drop,
        };
        // An empty pool borrows the groove's (never empty with the catalogue).
        if pool.is_empty() {
            &self.groove
        } else {
            pool
        }
    }
}

/// SplitMix64: a small seeded generator (no new dependency).
struct Rng(u64);

impl Rng {
    fn new(seed: u64, a: u64, b: u64) -> Self {
        Rng(seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xD1B5_4A32_D192_ED03))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// One of `pool`, not `avoid` when there is a choice.
    fn pick<'a>(&mut self, pool: &'a [String], avoid: Option<&str>) -> Option<&'a String> {
        let choices: Vec<&String> = pool.iter().filter(|id| Some(id.as_str()) != avoid).collect();
        let choices = if choices.is_empty() { pool.iter().collect() } else { choices };
        (!choices.is_empty()).then(|| choices[(self.next() % choices.len() as u64) as usize])
    }
}

/// The phrases of a section: starts on the downbeats every `bars` bars,
/// a tail shorter than 2 bars joined to the phrase before.
fn phrases(start: f64, end: f64, downbeats: &[f64], bars: usize, beat: f64) -> Vec<(f64, f64)> {
    let inner: Vec<f64> = downbeats.iter().copied().filter(|&d| d > start + beat / 2.0 && d < end - beat / 2.0).collect();
    let mut cuts = vec![start];
    cuts.extend(inner.iter().skip(bars - 1).step_by(bars.max(1)).copied());
    cuts.push(end);
    let mut out: Vec<(f64, f64)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
    let min = 2.0 * BEATS_PER_BAR as f64 * beat - beat / 2.0;
    if out.len() > 1 && out.last().is_some_and(|p| p.1 - p.0 < min) {
        let tail = out.pop().unwrap();
        out.last_mut().unwrap().1 = tail.1;
    }
    out
}

/// The proposed show: tempo map, song, a marker per section, a « Looks »
/// track (layer 1) and an « Accents » track (layer 2). Every event plays a
/// cue of `pools`; positions in seconds.
pub fn generate(a: &Analysis, pools: &CuePools, name: &str, file: &str, seed: u64) -> Show {
    let beat = a.beat_s();
    let bar = beat * BEATS_PER_BAR as f64;
    let mut looks: Vec<Event> = Vec::new();
    let mut accents: Vec<Event> = Vec::new();
    let mut markers = Vec::new();
    let mut counts = std::collections::HashMap::new();
    let mut last_id: Option<String> = None;
    let beats = |n: f64| Dur::Beats(n);
    for (i, s) in a.sections.iter().enumerate() {
        let n = counts.entry(s.kind).or_insert(0);
        *n += 1;
        let label = match s.kind {
            SectionKind::Intro | SectionKind::Outro => s.kind.label().to_string(),
            k => format!("{} {n}", k.label()),
        };
        markers.push(Marker { at: s.start_s, name: label, color: s.kind.color() });
        let next = a.sections.get(i + 1).map(|n| n.kind);
        // The beat before a drop is dark.
        let end = if next == Some(SectionKind::Drop) && s.end_s - s.start_s > 2.0 * beat { s.end_s - beat } else { s.end_s };
        let parts = match s.kind {
            SectionKind::Intro | SectionKind::Break | SectionKind::Outro => phrases(s.start_s, end, &a.downbeats_s, 16, beat),
            SectionKind::Verse | SectionKind::Drop => phrases(s.start_s, end, &a.downbeats_s, 8, beat),
            // Two halves: denser in the second one.
            SectionKind::Buildup => {
                let bars_in = ((end - s.start_s) / bar).round().max(1.0) as usize;
                phrases(s.start_s, end, &a.downbeats_s, (bars_in / 2).max(2), beat)
            }
        };
        let brightness = match s.kind {
            k if k.calm() => 0.55 + 0.35 * s.energy,
            SectionKind::Verse => 0.8 + 0.2 * s.energy,
            _ => 1.0,
        };
        let count = parts.len();
        for (j, &(from, to)) in parts.iter().enumerate() {
            if to - from < 0.05 {
                continue;
            }
            let mut rng = Rng::new(seed, i as u64 + 1, j as u64 + 1);
            let Some(id) = rng.pick(pools.for_kind(s.kind), last_id.as_deref()).cloned() else { continue };
            let mut e = Event { start: from, len: to - from, source: EventSource::Cue { id: id.clone() }, time: TimeMode::Beats, ..Default::default() };
            e.modifiers.brightness = brightness;
            if s.kind.calm() {
                // Soft changes: fades, never on a drop.
                if j == 0 {
                    e.fade_in = beats(if s.kind == SectionKind::Intro { 4.0 } else { 2.0 });
                }
                if j + 1 < count {
                    e.to_next = Some(Transition { kind: TransitionKind::Fade, length: beats(2.0) });
                } else if s.kind == SectionKind::Outro {
                    e.fade_out = beats(8.0_f64.min((to - from) / beat));
                } else if next != Some(SectionKind::Drop) {
                    e.fade_out = beats(1.0);
                }
            }
            looks.push(e);
            last_id = Some(id);
        }
        if s.kind == SectionKind::Drop && a.sections.get(i.wrapping_sub(1)).is_none_or(|p| p.kind != SectionKind::Drop) {
            let mut rng = Rng::new(seed, i as u64 + 1, 0);
            if let Some(id) = rng.pick(&pools.hit, None) {
                let len = bar.min(s.end_s - s.start_s);
                accents.push(Event { start: s.start_s, len, source: EventSource::Cue { id: id.clone() }, time: TimeMode::Beats, ..Default::default() });
            }
        }
    }
    markers.truncate(crate::timeline::MAX_MARKERS);
    let mut show = Show {
        name: name.to_string(),
        time_base: TimeBase::Seconds,
        tempo_map: vec![TempoPoint { at_s: 0.0, bpm: a.bpm, beats_per_bar: BEATS_PER_BAR as u8 }],
        audio: Some(AudioRef { file: file.to_string(), duration_s: a.duration_s, ..Default::default() }),
        tracks: vec![
            Track { name: "Looks".into(), layer: 1, events: looks, ..Default::default() },
            Track { name: "Accents".into(), layer: 2, events: accents, ..Default::default() },
        ],
        markers,
        loop_region: None,
        timecode: None,
    };
    for (k, e) in show.tracks.iter_mut().flat_map(|t| t.events.iter_mut()).enumerate() {
        e.id = k as u64 + 1;
    }
    show.sanitize();
    show
}

/// « Auto <titre> », a valid show name not in `taken`.
pub fn show_name(file: &str, taken: impl Fn(&str) -> bool) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let clean: String = stem.chars().map(|c| if c.is_alphanumeric() || "-_".contains(c) { c } else { ' ' }).collect();
    let words: Vec<&str> = clean.split_whitespace().collect();
    let base: String = format!("Auto {}", words.join(" ")).trim().chars().take(56).collect::<String>().trim().to_string();
    let base = if crate::timeline::valid_show_name(&base) { base } else { "Auto".to_string() };
    (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base} {n}") })
        .find(|name| !taken(name))
        .expect("an unused name exists")
}

// ---------- the job ----------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// `GET /api/timeline/autoshow`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct JobView {
    pub state: JobState,
    /// Counts the jobs started.
    pub id: u64,
    pub file: Option<String>,
    /// The seed used (shown; *Autre proposition* sends another).
    pub seed: Option<u64>,
    /// 0..1 while running.
    pub progress: f32,
    /// *décodage*, *analyse*, *création du show*.
    pub step: String,
    pub message: String,
    /// The show opened.
    pub show: Option<String>,
    pub analysis: Option<Analysis>,
    /// Seconds the job took.
    pub seconds: f64,
}

#[derive(Default)]
struct Inner {
    view: JobView,
    ctl: Arc<Control>,
}

/// The one generation job of the studio (`Shared::autoshow`): its own lock,
/// so the job's thread never needs the studio's to report progress.
#[derive(Default)]
pub struct AutoShow {
    inner: Mutex<Inner>,
}

impl AutoShow {
    pub fn view(&self) -> JobView {
        let g = self.inner.lock().unwrap();
        let mut v = g.view.clone();
        if v.state == JobState::Running {
            v.progress = g.ctl.progress();
        }
        v
    }

    /// A new job, unless one is running.
    fn begin(&self, file: &str, seed: Option<u64>) -> Option<(u64, Arc<Control>)> {
        let mut g = self.inner.lock().unwrap();
        if g.view.state == JobState::Running {
            return None;
        }
        let id = g.view.id + 1;
        g.ctl = Arc::new(Control::default());
        g.view = JobView { state: JobState::Running, id, file: Some(file.to_string()), seed, step: "décodage".into(), ..Default::default() };
        Some((id, Arc::clone(&g.ctl)))
    }

    fn step(&self, id: u64, step: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.view.id == id {
            g.view.step = step.to_string();
        }
    }

    fn finish(&self, id: u64, state: JobState, f: impl FnOnce(&mut JobView)) {
        let mut g = self.inner.lock().unwrap();
        if g.view.id == id && g.view.state == JobState::Running {
            g.view.state = state;
            g.view.progress = if state == JobState::Done { 1.0 } else { g.ctl.progress() };
            f(&mut g.view);
        }
    }

    /// *Annuler*: the job stops at its next check; nothing is opened.
    pub fn cancel(&self) {
        self.inner.lock().unwrap().ctl.cancel();
    }
}

/// Marks a job failed if its thread ends without saying how (a panic).
struct Ender {
    hub: Arc<AutoShow>,
    id: u64,
}

impl Drop for Ender {
    fn drop(&mut self) {
        self.hub.finish(self.id, JobState::Failed, |v| v.message = "erreur interne pendant la génération".into());
    }
}

/// `POST /api/timeline/autoshow`: starts generating a show from `file` (a
/// song of the library) on its own thread. Err: (HTTP status, message).
pub fn start(shared: &Arc<Mutex<Shared>>, file: &str, seed: Option<u64>) -> std::result::Result<JobView, (u16, String)> {
    let (media, pools, hub, shows_dir) = {
        let s = shared.lock().unwrap();
        (Arc::clone(&s.media), CuePools::from_presets(&s.presets), Arc::clone(&s.autoshow), s.shows.dir().to_path_buf())
    };
    match media.path(file) {
        Ok(p) if p.is_file() => {}
        _ => return Err((404, format!("morceau introuvable : {}", file.chars().take(80).collect::<String>()))),
    }
    let seed = seed.map(|s| s & 0xFFFF_FFFF);
    let Some((id, ctl)) = hub.begin(file, seed) else { return Err((409, "une génération est déjà en cours".into())) };
    let (shared, file) = (Arc::clone(shared), file.to_string());
    let thread_hub = Arc::clone(&hub);
    let spawned = std::thread::Builder::new().name("autoshow".into()).spawn(move || {
        let hub = thread_hub;
        let _ender = Ender { hub: Arc::clone(&hub), id };
        let started = Instant::now();
        let made = (|| -> Result<(Show, Analysis, u64)> {
            let decoded = media.decode(&file)?;
            ctl.check()?;
            hub.step(id, "analyse");
            let analysis = analyze(&decoded, &ctl, ANALYSIS_BUDGET)?;
            let seed = seed.unwrap_or_else(|| file_seed(&decoded));
            drop(decoded);
            hub.step(id, "création du show");
            // The names in use, read from disk before taking the lock.
            let taken: Vec<String> = crate::timeline::ShowStore::new(shows_dir).list().into_iter().map(|i| i.name).collect();
            let name = show_name(&file, |n| taken.iter().any(|t| t == n));
            Ok((generate(&analysis, &pools, &name, &file, seed), analysis, seed))
        })();
        match made {
            Ok((show, analysis, seed)) => {
                let mut s = shared.lock().unwrap();
                if ctl.cancelled() {
                    drop(s);
                    hub.finish(id, JobState::Cancelled, |v| v.message = Cancelled.to_string());
                    return;
                }
                let name = show.name.clone();
                // Never arms: the show is only loaded, stopped.
                s.timeline.load_unsaved(show);
                drop(s);
                hub.finish(id, JobState::Done, |v| {
                    v.seed = Some(seed);
                    v.show = Some(name);
                    v.analysis = Some(analysis);
                    v.seconds = started.elapsed().as_secs_f64();
                });
            }
            Err(e) if e.is::<Cancelled>() => hub.finish(id, JobState::Cancelled, |v| v.message = Cancelled.to_string()),
            Err(e) => hub.finish(id, JobState::Failed, |v| v.message = format!("{e:#}")),
        }
    });
    if let Err(e) = spawned {
        hub.finish(id, JobState::Failed, |v| v.message = format!("pas de fil d'exécution : {e}"));
    }
    Ok(hub.view())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::bench::signals::Builder;

    const RATE: u32 = 48_000;

    fn decoded(signal: &[f32], rate: u32) -> Decoded {
        Decoded { sample_rate: rate, channels: 1, samples: signal.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect() }
    }

    fn analyse(signal: &[f32], rate: u32) -> Analysis {
        analyze(&decoded(signal, rate), &Control::default(), ANALYSIS_BUDGET).unwrap()
    }

    /// Intro groove, break, build-up, drop, groove: 128 BPM.
    fn arrangement() -> (crate::audio::bench::signals::Case, f64) {
        let case = Builder::new("auto", RATE, 128.0).groove(32, 1.0).breakdown(16).buildup(16).groove(48, 1.0).build();
        let drop = case.truth.drops[0];
        (case, drop)
    }

    fn pools() -> CuePools {
        CuePools::from_presets(&crate::presets::catalog())
    }

    #[test]
    fn the_arrangement_gives_its_tempo_and_a_drop_within_a_beat() {
        let (case, drop) = arrangement();
        let a = analyse(&case.signal, case.rate);
        assert!((a.bpm - 128.0).abs() <= 0.5, "{} BPM", a.bpm);
        assert!(a.confidence > 0.3, "{a:?}");
        let beat = 60.0 / 128.0;
        // The grid lands on the song's beats.
        let off = case.truth.beats.iter().map(|t| a.beats_s.iter().map(|b| (b - t).abs()).fold(f64::MAX, f64::min)).fold(0.0, f64::max);
        assert!(off < 0.05, "grid {off} s off");
        let drops: Vec<&SongSection> = a.sections.iter().filter(|s| s.kind == SectionKind::Drop).collect();
        assert_eq!(drops.len(), 1, "{:#?}", a.sections);
        assert!((drops[0].start_s - drop).abs() <= beat, "drop at {} for {drop}: {:#?}", drops[0].start_s, a.sections);
        // The drop is on a downbeat, the quiet parts before it are not drops.
        assert!(a.downbeats_s.iter().any(|d| (d - drops[0].start_s).abs() < 0.01), "{:?}", &a.downbeats_s[..8]);
        let kinds: Vec<SectionKind> = a.sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds.first(), Some(&SectionKind::Intro), "{kinds:?}");
        let i = kinds.iter().position(|&k| k == SectionKind::Drop).unwrap();
        assert!(matches!(kinds[i - 1], SectionKind::Buildup | SectionKind::Break), "{kinds:?}");
        // Sections tile the song.
        assert_eq!(a.sections[0].start_s, 0.0);
        assert!((a.sections.last().unwrap().end_s - a.duration_s).abs() < 1e-9);
        assert!(a.sections.windows(2).all(|w| w[0].end_s == w[1].start_s && w[0].start_s < w[0].end_s));
        assert!(drops[0].energy > 0.9, "the drop is about the loudest: {:#?}", a.sections);
    }

    #[test]
    fn a_quiet_half_then_a_loud_half_is_split_within_a_bar() {
        // T-167: 16 calm bars then 16 loud ones at 128 BPM.
        let case = Builder::new("calme-fort", RATE, 128.0).groove(64, 0.06).groove(64, 1.0).build();
        let change = 0.1 + 64.0 * 60.0 / 128.0;
        let a = analyse(&case.signal, case.rate);
        assert!((a.bpm - 128.0).abs() <= 0.5, "{} BPM", a.bpm);
        let bar = 4.0 * 60.0 / 128.0;
        assert!(a.sections.len() >= 2, "{:#?}", a.sections);
        let loud = a.sections.iter().find(|s| s.energy > 0.9).unwrap();
        assert!((loud.start_s - change).abs() <= bar, "boundary at {} for {change}: {:#?}", loud.start_s, a.sections);
        assert!(loud.energy > 0.9 && a.sections[0].energy < 0.2, "{:#?}", a.sections);
        assert_eq!(loud.kind, SectionKind::Drop, "much louder than what came before: {:#?}", a.sections);
    }

    #[test]
    fn the_generated_show_is_valid_and_follows_the_sections() {
        let (case, drop) = arrangement();
        let a = analyse(&case.signal, case.rate);
        let pools = pools();
        let show = generate(&a, &pools, "Auto test", "test.wav", 7);
        let catalogue = crate::presets::catalog();
        show.validate_edit(|id| catalogue.iter().any(|p| p.id == id)).unwrap();
        assert_eq!(show.time_base, TimeBase::Seconds);
        assert_eq!(show.tempo_map.len(), 1);
        assert!((show.tempo_map[0].bpm - 128.0).abs() <= 0.5);
        assert_eq!(show.song().map(|s| s.file.as_str()), Some("test.wav"));
        assert!((show.end() - a.duration_s).abs() < 1e-6);
        // A marker per section, the drop's within a beat of the real one.
        assert_eq!(show.markers.len(), a.sections.len());
        let m = show.markers.iter().find(|m| m.name.starts_with("Drop")).expect("a drop marker");
        assert!((m.at - drop).abs() <= 60.0 / 128.0, "{} for {drop}", m.at);
        assert_eq!(show.markers[0].name, "Intro");
        // Every event plays an existing cue of the right pool.
        let looks = &show.tracks[0].events;
        assert!(looks.len() >= a.sections.len());
        for e in looks {
            let EventSource::Cue { id } = &e.source else { panic!("cue events only") };
            let s = a.sections.iter().find(|s| e.start >= s.start_s - 1e-9 && e.start < s.end_s).unwrap();
            assert!(pools.for_kind(s.kind).contains(id), "{id} in a {:?}", s.kind);
            assert!(e.len > 0.0 && e.start + e.len <= a.duration_s + 1e-6);
        }
        // Nothing overlaps on the looks track; the beat before the drop is dark.
        assert!(looks.windows(2).all(|w| w[0].start + w[0].len <= w[1].start + 1e-9));
        let before = looks.iter().rfind(|e| e.start < m.at).unwrap();
        assert!((m.at - (before.start + before.len) - 60.0 / 128.0).abs() < 1e-6, "blackout beat");
        // The accent on the drop's first bar.
        let hits = &show.tracks[1].events;
        assert_eq!(hits.len(), 1);
        assert!((hits[0].start - m.at).abs() < 1e-9 && pools.hit.contains(&match &hits[0].source {
            EventSource::Cue { id } => id.clone(),
            _ => unreachable!(),
        }));
    }

    #[test]
    fn same_file_same_show_another_seed_another_proposal() {
        let (case, _) = arrangement();
        let d = decoded(&case.signal, case.rate);
        let a1 = analyze(&d, &Control::default(), ANALYSIS_BUDGET).unwrap();
        let a2 = analyze(&d, &Control::default(), ANALYSIS_BUDGET).unwrap();
        assert_eq!(a1, a2);
        assert_eq!(file_seed(&d), file_seed(&d.clone()));
        let seed = file_seed(&d);
        assert!(seed <= 0xFFFF_FFFF, "fits a JSON number exactly");
        let p = pools();
        assert_eq!(generate(&a1, &p, "x", "f.wav", seed), generate(&a2, &p, "x", "f.wav", seed));
        let other = generate(&a1, &p, "x", "f.wav", seed + 1);
        assert_ne!(other.tracks, generate(&a1, &p, "x", "f.wav", seed).tracks, "another seed, other cues");
        assert_eq!(other.markers, generate(&a1, &p, "x", "f.wav", seed).markers, "same sections");
    }

    #[test]
    fn cancel_and_budget_stop_the_analysis() {
        let (case, _) = arrangement();
        let d = decoded(&case.signal, case.rate);
        let ctl = Control::default();
        ctl.cancel();
        assert!(analyze(&d, &ctl, ANALYSIS_BUDGET).unwrap_err().is::<Cancelled>());
        let e = analyze(&d, &Control::default(), Duration::ZERO).unwrap_err();
        assert!(e.to_string().contains("trop longue"), "{e}");
        let short = decoded(&vec![0.1; RATE as usize], RATE);
        assert!(analyze(&short, &Control::default(), ANALYSIS_BUDGET).unwrap_err().to_string().contains("trop court"));
    }

    #[test]
    fn silence_and_noise_still_give_a_show_at_the_default_tempo() {
        let case = Builder::new("bruit", RATE, 128.0).noise(12.0, 0.05).build();
        let a = analyse(&case.signal, case.rate);
        assert_eq!(a.confidence, 0.0);
        assert_eq!(a.bpm, DEFAULT_BPM);
        assert!(!a.sections.is_empty());
        let show = generate(&a, &pools(), "Auto bruit", "b.wav", 1);
        assert!(!show.tracks[0].events.is_empty());
        let silent = decoded(&vec![0.0; 10 * RATE as usize], RATE);
        let a = analyze(&silent, &Control::default(), ANALYSIS_BUDGET).unwrap();
        assert_eq!(a.sections.len(), 1);
    }

    /// Measurement for the PR note (T-167: a 4-minute song in < 10 s in
    /// release), not a check:
    /// `cargo test --release -p laser-studio autoshow_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn autoshow_report() {
        let mut b = Builder::new("4 min", 44_100, 126.0);
        for _ in 0..4 {
            b = b.groove(64, 1.0).breakdown(32).buildup(16).groove(13, 1.0);
        }
        let case = b.build();
        let d = decoded(&case.signal, case.rate);
        let t = Instant::now();
        let a = analyze(&d, &Control::default(), ANALYSIS_BUDGET).unwrap();
        println!("{:.1} s of audio analysed in {:?}: {} BPM (confidence {:.2})", a.duration_s, t.elapsed(), a.bpm, a.confidence);
        println!("truth drops: {:?}", case.truth.drops);
        for s in &a.sections {
            println!("  {:7.2} → {:7.2}  {:?} ({:.2})", s.start_s, s.end_s, s.kind, s.energy);
        }
    }

    #[test]
    fn pools_hold_catalogue_cues_by_role() {
        let p = pools();
        let catalogue = crate::presets::catalog();
        for pool in [&p.calm, &p.groove, &p.build, &p.drop, &p.hit] {
            assert!(pool.len() >= 3, "{p:?}");
            for id in pool {
                let c = catalogue.iter().find(|c| &c.id == id).unwrap();
                assert!(!LEFT_OUT.contains(&c.category), "{id}");
            }
        }
        assert!(p.hit.iter().all(|id| !p.drop.contains(id)), "stabs are accents only");
        // The operator's cues and figures are never picked.
        let mut presets = catalogue.clone();
        presets.push(Preset { id: "u-1".into(), name: "mien".into(), category: crate::usercues::CATEGORY, settings: catalogue[0].settings.clone(), tag: None });
        assert_eq!(CuePools::from_presets(&presets), p);
    }

    #[test]
    fn show_names_are_valid_and_new() {
        assert_eq!(show_name("Mon titre (edit).mp3", |_| false), "Auto Mon titre edit");
        assert_eq!(show_name("été.wav", |n| n == "Auto été"), "Auto été 2");
        assert_eq!(show_name("!!!.wav", |_| false), "Auto");
        let long = show_name(&format!("{}.wav", "x".repeat(200)), |_| false);
        assert!(crate::timeline::valid_show_name(&long) && long.chars().count() <= 64);
        assert!(crate::timeline::valid_show_name(&show_name(&format!("{}.wav", "x".repeat(200)), |n| !n.ends_with(" 9"))));
    }

    #[test]
    fn phrases_cut_on_downbeats_and_keep_no_short_tail() {
        let beat = 0.5;
        let downbeats: Vec<f64> = (0..40).map(|k| k as f64 * 2.0).collect();
        // 20 bars in phrases of 8: 8, 8, then 4.
        assert_eq!(phrases(0.0, 40.0, &downbeats, 8, beat), vec![(0.0, 16.0), (16.0, 32.0), (32.0, 40.0)]);
        // 17 bars: the 1-bar tail joins the last phrase.
        assert_eq!(phrases(0.0, 34.0, &downbeats, 8, beat), vec![(0.0, 16.0), (16.0, 34.0)]);
        assert_eq!(phrases(3.0, 5.0, &downbeats, 8, beat), vec![(3.0, 5.0)]);
    }

    /// The whole show through the real engine frame (`Engine::tick`, as
    /// the 60 fps thread runs it), in preview: every event renders within
    /// the point budget and the laser never arms.
    #[test]
    fn the_show_renders_through_the_engine_without_arming() {
        let (case, _) = arrangement();
        let a = analyse(&case.signal, case.rate);
        let mut s = crate::test_support::shared();
        let show = generate(&a, &CuePools::from_presets(&s.presets), "Auto rendu", "r.wav", 3);
        let events: Vec<(f64, f64)> = show.tracks.iter().flat_map(|t| &t.events).map(|e| (e.start, e.len)).collect();
        s.timeline.load_unsaved(show);
        crate::controls::timeline_play(&mut s).unwrap();
        let shared = Mutex::new(s);
        let dir = std::env::temp_dir().join(format!("laser-studio-autoshow-{}", std::process::id()));
        let mut engine = crate::Engine::new(&shared, None, dir.join("live.json"), dir.join("layers.json"));
        let mut now = Instant::now();
        let (mut worst, mut lit_frames, mut ticks) = (Duration::ZERO, 0, 0);
        for (start, len) in events {
            {
                let mut s = shared.lock().unwrap();
                let t = now.saturating_duration_since(s.epoch).as_secs_f64();
                let c = crate::timeline::Clock { t, beat: s.tempo.beat_at(t), bpm: s.tempo.bpm, beats_per_bar: 4 };
                s.timeline.seek(start + len.min(2.0) / 2.0, &c);
            }
            for _ in 0..20 {
                now += crate::FRAME_INTERVAL;
                let t = Instant::now();
                engine.tick(&shared, now);
                worst = worst.max(t.elapsed());
                ticks += 1;
                let s = shared.lock().unwrap();
                assert!(s.frame.len() <= 4000, "{} points", s.frame.len());
                assert!(s.frame.iter().all(|p| p.x.is_finite() && p.y.is_finite() && (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)));
                lit_frames += s.frame.iter().any(|p| p.is_lit()) as usize;
                assert!(!s.gate.is_armed(), "a generated show never arms");
            }
        }
        engine.shutdown(&shared);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(lit_frames > ticks / 2, "the preview shows the show: {lit_frames} / {ticks}");
        // Debug build, tests in parallel: a generous bound on one frame.
        assert!(worst < Duration::from_millis(250), "a frame took {worst:?}");
    }

    #[test]
    fn the_job_decodes_analyses_and_opens_an_unsaved_show() {
        let (case, drop_t) = arrangement();
        let s = crate::test_support::shared();
        let media = Arc::clone(&s.media);
        let wav = crate::audio::decode::testing::wav16(RATE, 1, &case.signal);
        let file = media.import("Arrangement auto.wav", &wav).unwrap().file;
        let shared = Arc::new(Mutex::new(s));
        assert_eq!(start(&shared, "absent.wav", None).unwrap_err().0, 404);
        let v = start(&shared, &file, None).unwrap();
        assert_eq!(v.state, JobState::Running);
        assert_eq!(start(&shared, &file, None).unwrap_err().0, 409, "one job at a time");
        let hub = Arc::clone(&shared.lock().unwrap().autoshow);
        let t0 = Instant::now();
        while hub.view().state == JobState::Running {
            assert!(t0.elapsed() < Duration::from_secs(60));
            // The studio lock stays free while the job runs.
            let s = shared.lock().unwrap();
            assert!(!s.gate.is_armed());
            drop(s);
            std::thread::sleep(Duration::from_millis(20));
        }
        let v = hub.view();
        assert_eq!(v.state, JobState::Done, "{v:?}");
        assert_eq!(v.show.as_deref(), Some("Auto Arrangement auto"));
        let s = shared.lock().unwrap();
        let state = s.timeline.state(&s.timeline_clock());
        assert_eq!(state.name.as_deref(), Some("Auto Arrangement auto"));
        assert!(state.modified, "opened unsaved");
        assert!(!state.playing);
        assert!(s.shows.list().iter().all(|i| i.name != "Auto Arrangement auto"), "not written to disk");
        let show = s.timeline.show.as_ref().unwrap();
        assert_eq!(show.song().unwrap().file, file);
        assert!(show.markers.iter().any(|m| m.name.starts_with("Drop") && (m.at - drop_t).abs() <= 60.0 / 128.0));
        assert!(!s.gate.is_armed());
        drop(s);
        // Same file: same seed, same show.
        let first = shared.lock().unwrap().timeline.show.clone().unwrap();
        start(&shared, &file, v.seed).unwrap();
        while hub.view().state == JobState::Running {
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut again = shared.lock().unwrap().timeline.show.clone().unwrap();
        again.name = first.name.clone();
        assert_eq!(again, first);
        let _ = std::fs::remove_file(media.path(&file).unwrap());
    }

    #[test]
    fn a_cancelled_job_opens_nothing() {
        let (case, _) = arrangement();
        let s = crate::test_support::shared();
        let media = Arc::clone(&s.media);
        let wav = crate::audio::decode::testing::wav16(RATE, 1, &case.signal);
        let file = media.import("Annule auto.wav", &wav).unwrap().file;
        let shared = Arc::new(Mutex::new(s));
        start(&shared, &file, Some(5)).unwrap();
        let hub = Arc::clone(&shared.lock().unwrap().autoshow);
        hub.cancel();
        let t0 = Instant::now();
        while hub.view().state == JobState::Running {
            assert!(t0.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hub.view().state, JobState::Cancelled);
        assert!(shared.lock().unwrap().timeline.show.is_none());
        let _ = std::fs::remove_file(media.path(&file).unwrap());
    }
}
