//! Test bench of the audio analysis (T-244): synthetic cases with their
//! ground truth (`signals.rs`) run through the whole `Analyzer` hop by hop,
//! as the analysis thread does (no audio device, no network), and scored
//! (`metrics.rs`):
//!
//! - onset F-measure (±50 ms) on every transient, and per kind (kick,
//!   snare, hi-hat);
//! - tempo: Acc1 (±4 %), Acc2 (octave errors allowed) and the share of
//!   *Locked* hops, over every hop once the detector has had 8 s of a new
//!   tempo; beat F-measure (±70 ms) on the tracked beats; lock time (first
//!   *Locked* after the pulse starts or comes back from a silence) and how
//!   many pulses never lock; re-tempo time (after a tempo change, until
//!   the estimate stays within 4 %);
//! - latency: the end of the hop that reports a kick (or any onset) minus
//!   the sample of its transient. Analysis only: the CoreAudio buffer, the
//!   engine tick and the DAC come on top (research note § 2.7);
//! - sections: share of the hops whose section is right (a beat of slack
//!   after each change), drop F-measure (±100 ms on `last_drop_t`);
//! - no drums: false kicks per minute under a bass line alone; under white
//!   noise, false kicks and onsets per minute, the highest tempo
//!   confidence, and never *Locked*.
//!
//! The default tests check each group of cases against floors set just
//! under the baseline measured when the bench was written (see `Limits`):
//! they fail on a regression, not on today's weaknesses. Raise a floor
//! when the analysis improves. The full table:
//!
//! ```sh
//! cargo test -p laser-studio audio_bench -- --ignored --nocapture
//! ```
//!
//! and on a private corpus of WAV files, each with a `<name>.csv` of
//! `time_s,label` rows (labels `onset`, `kick`, `snare`, `hat`, `beat`,
//! `drop`, `section:<silence|normal|break|buildup|drop>`), never committed
//! unless its licence allows it (docs/CONTENT_SOURCES.md):
//!
//! ```sh
//! AUDIO_BENCH_CORPUS=/path/to/dir cargo test -p laser-studio audio_bench_corpus -- --ignored --nocapture
//! ```

mod metrics;
mod signals;

use super::analysis::{Analyzer, HOP};
use super::bpm::DetectState;
use super::onsets::Onsets;
use super::spectrum::AnalysisConfig;
use crate::engine::Section;
use anyhow::{Context, Result};
use metrics::{acc1, acc2, delays, f_measure, mean, quantile};
use signals::{Builder, Case, TempoSpan, Truth};
use std::fmt::Write as _;
use std::path::Path;

/// Onsets match within ±50 ms, beats within ±70 ms, drops within ±100 ms.
const ONSET_TOL_S: f64 = 0.05;
const BEAT_TOL_S: f64 = 0.07;
const DROP_TOL_S: f64 = 0.1;
/// A report counts for an event from 5 ms before it to 100 ms after it.
const REPORT_EARLY_S: f64 = 0.005;
const REPORT_LATE_S: f64 = 0.1;
/// No-pulse confidence is judged once the estimator has 3 s of sound.
const NO_PULSE_FROM_S: f64 = 3.0;

/// One hop's output, at the end of the hop (s).
#[derive(Clone, Copy, Debug)]
struct HopRec {
    t: f64,
    bpm: f32,
    confidence: f32,
    state: DetectState,
    section: Section,
}

/// An event: when it was reported (end of the hop) and the time the
/// analysis gives it.
#[derive(Clone, Copy, Debug, Default)]
struct Event {
    at: f64,
    t: f64,
}

#[derive(Debug, Default)]
struct Trace {
    hops: Vec<HopRec>,
    onset: Vec<Event>,
    kick: Vec<Event>,
    snare: Vec<Event>,
    hat: Vec<Event>,
    beats: Vec<f64>,
    drops: Vec<f64>,
}

/// Runs `signal` through a fresh analyser, hop by hop.
fn run(signal: &[f32], rate: u32) -> Trace {
    let mut a = Analyzer::with_config(rate, Onsets::default(), AnalysisConfig::default());
    let mut tr = Trace::default();
    let (mut prev_o, mut prev_beat, mut prev_drop) = (Onsets::default(), 0.0, 0);
    for (h, hop) in signal.as_chunks::<HOP>().0.iter().enumerate() {
        let at = ((h + 1) * HOP) as f64 / rate as f64;
        let m = a.process(hop, at);
        let o = m.onsets;
        let note = |now: u64, before: u64, t: f64, v: &mut Vec<Event>| {
            if now > before {
                v.push(Event { at, t });
            }
        };
        note(o.onset, prev_o.onset, o.last_onset_t, &mut tr.onset);
        note(o.kick, prev_o.kick, o.last_kick_t, &mut tr.kick);
        note(o.snare, prev_o.snare, o.last_snare_t, &mut tr.snare);
        note(o.hat, prev_o.hat, o.last_hat_t, &mut tr.hat);
        prev_o = o;
        if m.tempo.beat_time > 0.0 && m.tempo.beat_time != prev_beat {
            tr.beats.push(m.tempo.beat_time);
            prev_beat = m.tempo.beat_time;
        }
        if m.sections.drop > prev_drop {
            tr.drops.push(m.sections.last_drop_t);
            prev_drop = m.sections.drop;
        }
        tr.hops.push(HopRec { t: at, bpm: m.tempo.bpm, confidence: m.tempo.confidence, state: m.tempo.state, section: m.features.section });
    }
    tr
}

/// One case's scores; `None` where the case says nothing about it.
#[derive(Clone, Debug, Default)]
struct CaseResult {
    name: String,
    onset_f: Option<f64>,
    kick_f: Option<f64>,
    snare_f: Option<f64>,
    hat_f: Option<f64>,
    bpm_acc1: Option<f64>,
    bpm_acc2: Option<f64>,
    /// Share of the judged hops in the *Locked* state.
    locked_share: Option<f64>,
    beat_f: Option<f64>,
    /// From the pulse starting (or coming back after a silence) to *Locked*;
    /// infinite when it never locks.
    lock_s: Vec<f64>,
    /// After a change of tempo, until the estimate stays within 4 %.
    retempo_s: Vec<f64>,
    kick_delays: Vec<f64>,
    onset_delays: Vec<f64>,
    section_acc: Option<f64>,
    drop_f: Option<f64>,
    false_kicks_per_min: Option<f64>,
    false_onsets_per_min: Option<f64>,
    noise_confidence: Option<f64>,
    noise_locked: bool,
}

fn times(v: &[Event]) -> Vec<f64> {
    v.iter().map(|e| e.t).collect()
}

fn reports(v: &[Event]) -> Vec<f64> {
    v.iter().map(|e| e.at).collect()
}

fn within(v: &[f64], from: f64, to: f64) -> Vec<f64> {
    v.iter().copied().filter(|&t| t >= from && t <= to).collect()
}

fn evaluate(name: &str, truth: &Truth, tr: &Trace, duration: f64) -> CaseResult {
    let mut r = CaseResult { name: name.into(), ..Default::default() };
    if truth.onsets_labelled {
        r.onset_f = Some(f_measure(&truth.onsets, &times(&tr.onset), ONSET_TOL_S) as f64);
        r.onset_delays = delays(&truth.onsets, &reports(&tr.onset), REPORT_EARLY_S, REPORT_LATE_S);
    }
    if truth.drums_labelled {
        r.kick_f = Some(f_measure(&truth.kicks, &times(&tr.kick), ONSET_TOL_S) as f64);
        r.snare_f = (!truth.snares.is_empty()).then(|| f_measure(&truth.snares, &times(&tr.snare), ONSET_TOL_S) as f64);
        r.hat_f = (!truth.hats.is_empty()).then(|| f_measure(&truth.hats, &times(&tr.hat), ONSET_TOL_S) as f64);
        r.kick_delays = delays(&truth.kicks, &reports(&tr.kick), REPORT_EARLY_S, REPORT_LATE_S);
    }

    // Tempo, span by span.
    let (mut n, mut a1, mut a2, mut locked) = (0usize, 0usize, 0usize, 0usize);
    let (mut beat_truth, mut beat_found) = (Vec::new(), Vec::new());
    for (i, span) in truth.tempo.iter().enumerate() {
        let &TempoSpan { from, to, bpm, settle } = span;
        let judged = from + settle;
        for h in tr.hops.iter().filter(|h| h.t >= judged && h.t <= to) {
            n += 1;
            a1 += acc1(h.bpm as f64, bpm) as usize;
            a2 += acc2(h.bpm as f64, bpm) as usize;
            locked += (h.state == DetectState::Locked) as usize;
        }
        beat_truth.extend(within(&truth.beats, judged, to));
        beat_found.extend(within(&tr.beats, judged, to));
        let after_silence = i == 0 || truth.section_at(from - 0.01) == Some(Section::Silence);
        let mut in_span = tr.hops.iter().filter(|h| h.t >= from && h.t <= to);
        if after_silence {
            r.lock_s.push(in_span.clone().find(|h| h.state == DetectState::Locked).map_or(f64::INFINITY, |h| h.t - from));
        } else if truth.tempo[i - 1].bpm != bpm {
            // The last hop that is off: the estimate is right from the next one.
            let off = in_span.rfind(|h| !acc1(h.bpm as f64, bpm)).map(|h| h.t);
            r.retempo_s.push(match off {
                None => 0.0,
                Some(t) if t >= to - 0.01 => f64::INFINITY,
                Some(t) => t - from,
            });
        }
    }
    if n > 0 {
        r.bpm_acc1 = Some(a1 as f64 / n as f64);
        r.bpm_acc2 = Some(a2 as f64 / n as f64);
        r.locked_share = Some(locked as f64 / n as f64);
        r.beat_f = Some(f_measure(&beat_truth, &beat_found, BEAT_TOL_S) as f64);
    }

    if let Some((from, to)) = truth.sections_eval {
        let beat = 60.0 / truth.tempo.first().map_or(120.0, |s| s.bpm);
        let slack = (beat + 0.05).max(0.4);
        let (mut n, mut ok) = (0usize, 0usize);
        for h in tr.hops.iter().filter(|h| h.t >= from && h.t <= to) {
            if truth.sections.iter().any(|c| h.t >= c.0 && h.t < c.0 + slack) {
                continue;
            }
            n += 1;
            ok += (truth.section_at(h.t) == Some(h.section)) as usize;
        }
        r.section_acc = (n > 0).then(|| ok as f64 / n as f64);
        r.drop_f = Some(f_measure(&truth.drops, &tr.drops, DROP_TOL_S) as f64);
    }

    if truth.no_kicks {
        r.false_kicks_per_min = Some(tr.kick.len() as f64 * 60.0 / duration);
    }
    if truth.no_onsets {
        r.false_onsets_per_min = Some(tr.onset.len() as f64 * 60.0 / duration);
    }
    if truth.no_pulse {
        r.noise_confidence = Some(tr.hops.iter().filter(|h| h.t >= NO_PULSE_FROM_S).map(|h| h.confidence as f64).fold(0.0, f64::max));
        r.noise_locked = tr.hops.iter().any(|h| h.state == DetectState::Locked);
    }
    r
}

fn bench_case(c: &Case) -> CaseResult {
    let tr = run(&c.signal, c.rate);
    evaluate(&c.name, &c.truth, &tr, c.signal.len() as f64 / c.rate as f64)
}

/// A group's scores (the T-244 `BenchResult`, plus what the bench also
/// measures). Times in seconds, latencies in milliseconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BenchResult {
    pub onset_f: Option<f64>,
    pub kick_f: Option<f64>,
    pub snare_f: Option<f64>,
    pub hat_f: Option<f64>,
    pub bpm_acc1: Option<f64>,
    pub bpm_acc2: Option<f64>,
    pub locked_share: Option<f64>,
    pub beat_f: Option<f64>,
    /// Mean and worst lock time of the pulses that lock, and how many
    /// never do.
    pub lock_time_s: Option<f64>,
    pub lock_time_max_s: Option<f64>,
    pub never_locked: Option<f64>,
    /// Worst re-tempo time.
    pub retempo_s: Option<f64>,
    /// Kick report delay: median and worst.
    pub latency_ms: Option<f64>,
    pub latency_max_ms: Option<f64>,
    /// Any onset's report delay, median.
    pub onset_latency_ms: Option<f64>,
    pub section_acc: Option<f64>,
    pub drop_f: Option<f64>,
    pub false_kicks_per_min: Option<f64>,
    pub false_onsets_per_min: Option<f64>,
    /// Highest tempo confidence without a pulse; and whether it locked.
    pub noise_confidence: Option<f64>,
    pub noise_locked: Option<bool>,
}

fn aggregate(rs: &[CaseResult]) -> BenchResult {
    let avg = |f: fn(&CaseResult) -> Option<f64>| mean(rs.iter().map(f));
    let worst = |f: fn(&CaseResult) -> Option<f64>| rs.iter().filter_map(f).reduce(f64::max);
    let all = |f: fn(&CaseResult) -> &Vec<f64>| rs.iter().flat_map(f).copied().collect::<Vec<f64>>();
    let (locks, retempo, kd, od) = (all(|r| &r.lock_s), all(|r| &r.retempo_s), all(|r| &r.kick_delays), all(|r| &r.onset_delays));
    let no_pulse = rs.iter().any(|r| r.noise_confidence.is_some());
    BenchResult {
        onset_f: avg(|r| r.onset_f),
        kick_f: avg(|r| r.kick_f),
        snare_f: avg(|r| r.snare_f),
        hat_f: avg(|r| r.hat_f),
        bpm_acc1: avg(|r| r.bpm_acc1),
        bpm_acc2: avg(|r| r.bpm_acc2),
        locked_share: avg(|r| r.locked_share),
        beat_f: avg(|r| r.beat_f),
        lock_time_s: mean(locks.iter().map(|&x| x.is_finite().then_some(x))),
        lock_time_max_s: locks.iter().copied().filter(|x| x.is_finite()).reduce(f64::max),
        never_locked: (!locks.is_empty()).then(|| locks.iter().filter(|x| x.is_infinite()).count() as f64),
        retempo_s: retempo.iter().copied().reduce(f64::max),
        latency_ms: quantile(&kd, 0.5).map(|x| x * 1e3),
        latency_max_ms: quantile(&kd, 1.0).map(|x| x * 1e3),
        onset_latency_ms: quantile(&od, 0.5).map(|x| x * 1e3),
        section_acc: avg(|r| r.section_acc),
        drop_f: avg(|r| r.drop_f),
        false_kicks_per_min: worst(|r| r.false_kicks_per_min),
        false_onsets_per_min: worst(|r| r.false_onsets_per_min),
        noise_confidence: worst(|r| r.noise_confidence),
        noise_locked: no_pulse.then(|| rs.iter().any(|r| r.noise_locked)),
    }
}

// ---------------------------------------------------------------- cases

const RATE: u32 = 48_000;

/// Steady pulses: clicks, grooves from 90 to 174 BPM, 30 dB quieter, 44.1 kHz.
fn steady_cases() -> Vec<Case> {
    vec![
        Builder::new("clicks_128", RATE, 128.0).clicks(32).build(),
        Builder::new("groove_90", RATE, 90.0).groove(24, 1.0).build(),
        Builder::new("groove_128", RATE, 128.0).groove(32, 1.0).build(),
        Builder::new("groove_174", RATE, 174.0).groove(44, 1.0).build(),
        Builder::new("groove_128_-30dB", RATE, 128.0).groove(24, 0.0316).build(),
        Builder::new("groove_140_44k1", 44_100, 140.0).groove(32, 1.0).build(),
    ]
}

/// Tempo changes (120 → 130, outside the ±4 % follow band; a DJ nudging
/// 124 → 128, inside it) and grooves around silences (the second one long
/// enough to be a new track).
fn change_cases() -> Vec<Case> {
    vec![
        Builder::new("tempo_120_to_130", RATE, 120.0).groove(24, 1.0).tempo(130.0).groove(32, 1.0).build(),
        Builder::new("nudge_124_to_128", RATE, 124.0).groove(24, 1.0).tempo(128.0).groove(24, 1.0).build(),
        Builder::new("silence_gaps", RATE, 128.0).silence(1.5).groove(20, 1.0).silence(4.0).tempo(124.0).groove(20, 1.0).build(),
    ]
}

/// Harder material: a broken beat (kicks off the beat), a groove under
/// white noise 26 dB under full scale.
fn hard_cases() -> Vec<Case> {
    vec![Builder::new("breakbeat_96", RATE, 96.0).breakbeat(24).build(), Builder::new("groove_128_noisy", RATE, 128.0).groove(32, 1.0).bed(0.05).build()]
}

/// Groove → break → build-up → drop → groove.
fn arrangement_cases() -> Vec<Case> {
    vec![Builder::new("arrangement_128", RATE, 128.0).groove(16, 1.0).breakdown(8).buildup(8).groove(24, 1.0).build()]
}

/// No drums: white noise at −20 dBFS, a bass line and pad.
fn no_pulse_cases() -> Vec<Case> {
    vec![Builder::new("white_noise", RATE, 128.0).noise(10.0, 0.1).build(), Builder::new("bass_line", RATE, 128.0).bass_line(24).build()]
}

/// Runs the cases on their own threads (each is independent).
fn bench(cases: Vec<Case>) -> Vec<CaseResult> {
    std::thread::scope(|s| cases.iter().map(|c| s.spawn(move || bench_case(c))).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect())
}

// --------------------------------------------------------------- limits

/// Floors (higher is better) and ceilings (lower is better) for a group.
/// `None` = not checked.
#[derive(Clone, Copy, Debug, Default)]
struct Limits {
    onset_f: Option<f64>,
    kick_f: Option<f64>,
    snare_f: Option<f64>,
    hat_f: Option<f64>,
    bpm_acc1: Option<f64>,
    bpm_acc2: Option<f64>,
    locked_share: Option<f64>,
    beat_f: Option<f64>,
    lock_time_max_s: Option<f64>,
    never_locked: Option<f64>,
    retempo_s: Option<f64>,
    latency_ms: Option<f64>,
    latency_max_ms: Option<f64>,
    onset_latency_ms: Option<f64>,
    section_acc: Option<f64>,
    drop_f: Option<f64>,
    false_kicks_per_min: Option<f64>,
    false_onsets_per_min: Option<f64>,
    noise_confidence: Option<f64>,
}

const NO_LIMITS: Limits = Limits {
    onset_f: None,
    kick_f: None,
    snare_f: None,
    hat_f: None,
    bpm_acc1: None,
    bpm_acc2: None,
    locked_share: None,
    beat_f: None,
    lock_time_max_s: None,
    never_locked: None,
    retempo_s: None,
    latency_ms: None,
    latency_max_ms: None,
    onset_latency_ms: None,
    section_acc: None,
    drop_f: None,
    false_kicks_per_min: None,
    false_onsets_per_min: None,
    noise_confidence: None,
};

/// What `r` breaks of `l`, one line each.
fn violations(r: &BenchResult, l: &Limits) -> Vec<String> {
    let mut out = Vec::new();
    let mut floor = |name: &str, v: Option<f64>, min: Option<f64>| {
        if let Some(min) = min {
            match v {
                Some(v) if v >= min => {}
                v => out.push(format!("{name} = {v:?}, floor {min}")),
            }
        }
    };
    floor("onset_f", r.onset_f, l.onset_f);
    floor("kick_f", r.kick_f, l.kick_f);
    floor("snare_f", r.snare_f, l.snare_f);
    floor("hat_f", r.hat_f, l.hat_f);
    floor("bpm_acc1", r.bpm_acc1, l.bpm_acc1);
    floor("bpm_acc2", r.bpm_acc2, l.bpm_acc2);
    floor("locked_share", r.locked_share, l.locked_share);
    floor("beat_f", r.beat_f, l.beat_f);
    floor("section_acc", r.section_acc, l.section_acc);
    floor("drop_f", r.drop_f, l.drop_f);
    let mut ceiling = |name: &str, v: Option<f64>, max: Option<f64>| {
        if let Some(max) = max {
            match v {
                Some(v) if v <= max => {}
                v => out.push(format!("{name} = {v:?}, ceiling {max}")),
            }
        }
    };
    ceiling("lock_time_max_s", r.lock_time_max_s, l.lock_time_max_s);
    ceiling("never_locked", r.never_locked, l.never_locked);
    ceiling("retempo_s", r.retempo_s, l.retempo_s);
    ceiling("latency_ms", r.latency_ms, l.latency_ms);
    ceiling("latency_max_ms", r.latency_max_ms, l.latency_max_ms);
    ceiling("onset_latency_ms", r.onset_latency_ms, l.onset_latency_ms);
    ceiling("false_kicks_per_min", r.false_kicks_per_min, l.false_kicks_per_min);
    ceiling("false_onsets_per_min", r.false_onsets_per_min, l.false_onsets_per_min);
    ceiling("noise_confidence", r.noise_confidence, l.noise_confidence);
    out
}

fn check(group: &str, rs: &[CaseResult], l: &Limits) -> BenchResult {
    let r = aggregate(rs);
    let v = violations(&r, l);
    assert!(v.is_empty(), "audio bench, {group}: regression\n  {}\n{}", v.join("\n  "), table(rs, &r));
    r
}

// --------------------------------------------------------------- report

fn cell(v: Option<f64>, scale: f64, prec: usize) -> String {
    match v {
        Some(x) if x.is_infinite() => "never".into(),
        Some(x) => format!("{:.*}", prec, x * scale),
        None => "-".into(),
    }
}

fn worst_of(v: &[f64]) -> Option<f64> {
    v.iter().copied().reduce(f64::max)
}

/// The results as a fixed-width table, one row per case and a total.
fn table(rs: &[CaseResult], total: &BenchResult) -> String {
    let mut s = String::new();
    let head = ["case", "onsetF", "kickF", "snareF", "hatF", "acc1", "acc2", "locked", "beatF", "lock s", "retempo", "kick ms", "kick max", "onset ms", "sect", "dropF", "fk/min", "fo/min", "noiseC"];
    let w = [20, 7, 7, 7, 7, 7, 7, 7, 7, 7, 8, 8, 9, 9, 7, 7, 7, 7, 7];
    let row = |s: &mut String, cells: &[String]| {
        for (c, w) in cells.iter().zip(w) {
            let _ = write!(s, "{c:>w$} ");
        }
        s.push('\n');
    };
    row(&mut s, &head.map(String::from));
    for r in rs {
        row(
            &mut s,
            &[
                r.name.clone(),
                cell(r.onset_f, 1.0, 3),
                cell(r.kick_f, 1.0, 3),
                cell(r.snare_f, 1.0, 3),
                cell(r.hat_f, 1.0, 3),
                cell(r.bpm_acc1, 1.0, 3),
                cell(r.bpm_acc2, 1.0, 3),
                cell(r.locked_share, 1.0, 3),
                cell(r.beat_f, 1.0, 3),
                cell(worst_of(&r.lock_s), 1.0, 2),
                cell(worst_of(&r.retempo_s), 1.0, 2),
                cell(quantile(&r.kick_delays, 0.5), 1e3, 1),
                cell(quantile(&r.kick_delays, 1.0), 1e3, 1),
                cell(quantile(&r.onset_delays, 0.5), 1e3, 1),
                cell(r.section_acc, 1.0, 3),
                cell(r.drop_f, 1.0, 3),
                cell(r.false_kicks_per_min, 1.0, 1),
                cell(r.false_onsets_per_min, 1.0, 1),
                cell(r.noise_confidence, 1.0, 2),
            ],
        );
    }
    let t = total;
    row(
        &mut s,
        &[
            "TOTAL".into(),
            cell(t.onset_f, 1.0, 3),
            cell(t.kick_f, 1.0, 3),
            cell(t.snare_f, 1.0, 3),
            cell(t.hat_f, 1.0, 3),
            cell(t.bpm_acc1, 1.0, 3),
            cell(t.bpm_acc2, 1.0, 3),
            cell(t.locked_share, 1.0, 3),
            cell(t.beat_f, 1.0, 3),
            cell(t.lock_time_max_s, 1.0, 2),
            cell(t.retempo_s, 1.0, 2),
            cell(t.latency_ms, 1.0, 1),
            cell(t.latency_max_ms, 1.0, 1),
            cell(t.onset_latency_ms, 1.0, 1),
            cell(t.section_acc, 1.0, 3),
            cell(t.drop_f, 1.0, 3),
            cell(t.false_kicks_per_min, 1.0, 1),
            cell(t.false_onsets_per_min, 1.0, 1),
            cell(t.noise_confidence, 1.0, 2),
        ],
    );
    let _ = writeln!(s, "mean lock time {} s; pulses never locked: {}; noise ever locked: {:?}", cell(t.lock_time_s, 1.0, 2), cell(t.never_locked, 1.0, 0), t.noise_locked);
    s
}

// --------------------------------------------------------------- corpus

/// A WAV file of the private corpus, mixed to mono.
fn load_wav(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("{}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|s| s as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    Ok((interleaved.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect(), spec.sample_rate))
}

/// `time_s,label` rows (header, blank and `#` lines skipped).
fn parse_annotations(text: &str, duration: f64) -> Result<Truth> {
    let mut t = Truth::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (time, label) = line.split_once(',').with_context(|| format!("ligne {}: time,label attendu", n + 1))?;
        let Ok(time) = time.trim().parse::<f64>() else {
            if n == 0 {
                continue; // header
            }
            anyhow::bail!("ligne {}: temps illisible", n + 1);
        };
        match label.trim() {
            "onset" => t.onsets.push(time),
            "kick" => t.kicks.push(time),
            "snare" => t.snares.push(time),
            "hat" => t.hats.push(time),
            "beat" => t.beats.push(time),
            "drop" => t.drops.push(time),
            l => {
                let name = l.strip_prefix("section:").with_context(|| format!("ligne {}: étiquette inconnue {l:?}", n + 1))?;
                let s: Section = serde_json::from_value(serde_json::Value::String(name.into())).with_context(|| format!("ligne {}: section {name:?}", n + 1))?;
                t.sections.push((time, s));
            }
        }
    }
    for v in [&mut t.onsets, &mut t.kicks, &mut t.snares, &mut t.hats, &mut t.beats, &mut t.drops] {
        v.sort_by(f64::total_cmp);
    }
    t.sections.sort_by(|a, b| a.0.total_cmp(&b.0));
    t.drums_labelled = !(t.kicks.is_empty() && t.snares.is_empty() && t.hats.is_empty());
    let mut all: Vec<f64> = t.onsets.iter().chain(&t.kicks).chain(&t.snares).chain(&t.hats).copied().collect();
    all.sort_by(f64::total_cmp);
    t.onsets_labelled = !all.is_empty();
    t.onsets.clear();
    for x in all {
        if t.onsets.last().is_none_or(|&l| x - l > 0.03) {
            t.onsets.push(x);
        }
    }
    if t.beats.len() >= 2 {
        let gaps: Vec<f64> = t.beats.windows(2).map(|w| w[1] - w[0]).collect();
        let p = quantile(&gaps, 0.5).unwrap_or(0.5);
        t.tempo.push(TempoSpan { from: t.beats[0], to: *t.beats.last().unwrap(), bpm: 60.0 / p, settle: 8.0 });
    }
    if let Some(&(start, _)) = t.sections.first() {
        t.sections_eval = Some((start, duration));
    }
    Ok(t)
}

fn corpus(dir: &Path) -> Result<Vec<Case>> {
    let mut cases = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))).collect();
    paths.sort();
    for wav in paths {
        let csv = wav.with_extension("csv");
        if !csv.exists() {
            eprintln!("{}: pas d'annotations ({}), ignoré", wav.display(), csv.display());
            continue;
        }
        let (signal, rate) = load_wav(&wav)?;
        let truth = parse_annotations(&std::fs::read_to_string(&csv)?, signal.len() as f64 / rate.max(1) as f64).with_context(|| csv.display().to_string())?;
        cases.push(Case { name: wav.file_stem().unwrap_or_default().to_string_lossy().into(), rate, signal, truth });
    }
    Ok(cases)
}

// ---------------------------------------------------------------- tests

/// Floors and ceilings per group, from the baseline measured on
/// 2026-10-04 (develop at ff67bf1; the numbers are in the comments and in
/// docs/prs/audio-bench.md). Floors sit a little under it, ceilings a
/// little over, so float noise across machines passes and a real
/// regression doesn't. Known weaknesses are recorded as they are:
/// tighten the limit when the analysis gets better. Raised by T-356 (the
/// 90 BPM groove and the 96 BPM breakbeat lock) and T-357 (a hat with a
/// kick or a snare, the snare roll under the riser); the comments give the
/// numbers then.
const STEADY: Limits = Limits {
    onset_f: Some(0.98),       // 0.997
    kick_f: Some(0.98),        // 1.000
    snare_f: Some(0.95),       // 0.983
    hat_f: Some(0.94),         // 0.973 (0.806 before T-357)
    bpm_acc1: Some(0.97),      // 1.000
    bpm_acc2: Some(0.97),      // 1.000
    locked_share: Some(0.97),  // 1.000 (0.833 before T-356: groove_90 never locked)
    beat_f: Some(0.95),        // 0.993
    lock_time_max_s: Some(7.5), // 6.39 (mean 5.99)
    never_locked: Some(0.0),   // 0
    latency_ms: Some(22.0),    // 18.0
    latency_max_ms: Some(28.0), // 23.2
    onset_latency_ms: Some(20.0), // 16.2
    section_acc: Some(0.97),   // 1.000: no false break in a steady groove
    drop_f: Some(0.99),        // 1.000: no false drop
    ..NO_LIMITS
};

const CHANGES: Limits = Limits {
    onset_f: Some(0.98),       // 0.996
    kick_f: Some(0.98),        // 1.000
    snare_f: Some(0.94),       // 0.977
    hat_f: Some(0.93),         // 0.958 (0.801)
    bpm_acc1: Some(0.97),      // 1.000
    bpm_acc2: Some(0.97),      // 1.000
    locked_share: Some(0.95),  // 1.000
    beat_f: Some(0.92),        // 0.955
    lock_time_max_s: Some(7.5), // 6.00
    never_locked: Some(0.0),   // 0
    retempo_s: Some(5.0),      // 3.93 (120 → 130); the 124 → 128 nudge: 0
    latency_ms: Some(22.0),    // 17.7
    latency_max_ms: Some(28.0), // 20.8
    section_acc: Some(0.97),   // 1.000 (silences included)
    drop_f: Some(0.99),        // 1.000
    ..NO_LIMITS
};

const HARD: Limits = Limits {
    onset_f: Some(0.97),       // 0.991
    kick_f: Some(0.97),        // 1.000
    snare_f: Some(0.93),       // 0.965
    hat_f: Some(0.93),         // 0.958 (0.767)
    bpm_acc1: Some(0.97),      // 1.000 (0.500: the 96 BPM breakbeat read 192...)
    bpm_acc2: Some(0.97),      // 1.000
    locked_share: Some(0.95),  // 1.000 (0.500: ... and never locked)
    beat_f: Some(0.95),        // 1.000 (0.833)
    lock_time_max_s: Some(7.5), // 6.41 (mean 6.16)
    never_locked: Some(0.0),   // 0
    latency_ms: Some(22.0),    // 17.3
    latency_max_ms: Some(28.0), // 21.3
    ..NO_LIMITS
};

const ARRANGEMENT: Limits = Limits {
    onset_f: Some(0.91),      // 0.940: the roll under the riser, any onset
    kick_f: Some(0.97),       // 1.000
    snare_f: Some(0.90),      // 0.912 (0.767): 11 of the 16 roll snares, the last 16ths lost in the riser
    hat_f: Some(0.92),        // 0.947 (0.806)
    bpm_acc1: Some(0.97),     // 1.000
    bpm_acc2: Some(0.97),     // 1.000
    locked_share: Some(0.95), // 1.000 (coasting through the break)
    beat_f: Some(0.94),       // 0.980
    section_acc: Some(0.95),  // 1.000
    drop_f: Some(0.99),       // 1.000: one drop, on time
    ..NO_LIMITS
};

/// One false event in the 10–12 s cases is 5–6 per minute; two fail.
const NO_PULSE: Limits = Limits {
    false_kicks_per_min: Some(6.0),  // 5.1 (one kick under the bass line), 0 under noise
    false_onsets_per_min: Some(6.0), // 5.7 (one onset in white noise)
    noise_confidence: Some(0.3),     // 0.00
    ..NO_LIMITS
};

#[test]
fn steady_pulses_hold_their_baseline() {
    check("steady", &bench(steady_cases()), &STEADY);
}

#[test]
fn tempo_changes_and_silences_hold_their_baseline() {
    check("changes", &bench(change_cases()), &CHANGES);
}

#[test]
fn hard_material_holds_its_baseline() {
    check("hard", &bench(hard_cases()), &HARD);
}

#[test]
fn the_arrangement_holds_its_baseline() {
    check("arrangement", &bench(arrangement_cases()), &ARRANGEMENT);
}

#[test]
fn no_pulse_holds_its_baseline() {
    let r = check("no pulse", &bench(no_pulse_cases()), &NO_PULSE);
    assert_eq!(r.noise_locked, Some(false), "noise or a bass line alone must never lock a tempo");
}

#[test]
fn the_bench_is_deterministic() {
    let make = || Builder::new("short", RATE, 128.0).groove(8, 1.0).build();
    let (a, b) = (bench_case(&make()), bench_case(&make()));
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

#[test]
fn annotations_parse() {
    let text = "time,label\n# comment\n0.5,kick\n0.5,hat\n1.0,beat\n1.5,beat\n2.0,beat\n2.0,section:break\n3.0,drop\n0.75,onset\n";
    let t = parse_annotations(text, 10.0).unwrap();
    assert_eq!(t.onsets, [0.5, 0.75]);
    assert_eq!((t.kicks.len(), t.hats.len(), t.beats.len(), t.drops.len()), (1, 1, 3, 1));
    assert!(t.drums_labelled && t.onsets_labelled);
    assert_eq!(t.tempo, [TempoSpan { from: 1.0, to: 2.0, bpm: 120.0, settle: 8.0 }]);
    assert_eq!(t.sections, [(2.0, Section::Break)]);
    assert_eq!(t.sections_eval, Some((2.0, 10.0)));
    assert!(parse_annotations("1.0,cowbell\n", 1.0).is_err());
    assert!(parse_annotations("1.0,section:chorus\n", 1.0).is_err());
    assert!(parse_annotations("0.0,kick\nx,kick\n", 1.0).is_err());
}

#[test]
fn a_wav_file_loads_as_mono() {
    let dir = std::env::temp_dir().join(format!("audio-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("two.wav");
    let spec = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..10 {
        w.write_sample(16_384i16).unwrap();
        w.write_sample(0i16).unwrap();
    }
    w.finalize().unwrap();
    std::fs::write(dir.join("two.csv"), "0.0,kick\n").unwrap();
    let cases = corpus(&dir).unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].rate, 44_100);
    assert_eq!(cases[0].signal, vec![0.25; 10]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The whole table: `cargo test -p laser-studio audio_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn audio_bench_report() {
    type Group = (&'static str, fn() -> Vec<Case>, Limits);
    let groups: [Group; 5] = [("steady", steady_cases, STEADY), ("changes", change_cases, CHANGES), ("hard", hard_cases, HARD), ("arrangement", arrangement_cases, ARRANGEMENT), ("no drums", no_pulse_cases, NO_PULSE)];
    let mut failed = Vec::new();
    for (name, cases, limits) in groups {
        let rs = bench(cases());
        let total = aggregate(&rs);
        println!("\n== {name} ==\n{}", table(&rs, &total));
        println!("{total:?}");
        failed.extend(violations(&total, &limits).into_iter().map(|v| format!("{name}: {v}")));
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The private corpus: `AUDIO_BENCH_CORPUS=<dir> cargo test -p laser-studio
/// audio_bench_corpus -- --ignored --nocapture`. Reports, never fails on
/// the scores.
#[test]
#[ignore]
fn audio_bench_corpus() {
    let Some(dir) = std::env::var_os("AUDIO_BENCH_CORPUS") else {
        println!("AUDIO_BENCH_CORPUS n'est pas défini: rien à mesurer");
        return;
    };
    let cases = corpus(Path::new(&dir)).expect("corpus");
    let rs = bench(cases);
    println!("{}", table(&rs, &aggregate(&rs)));
}

