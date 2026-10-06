//! The engine's time budget (T-448).
//!
//! A fixed scenario (four layers playing a cue each, about 3 000 points a
//! frame, 8 safety zones, live modifiers on) through the real engine frame
//! (`Engine::tick`, the very function the 60 fps thread runs) on a
//! simulated clock, 600 frames, read from the engine's own monitor
//! (`perf.rs`): the 99th percentile of the safety stage must stay under
//! `SAFETY_BUDGET_MS` and the whole frame under `FRAME_BUDGET_MS`.
//!
//! Marked `#[ignore]`: run it alone, optimised, on the integration
//! machine before a merge:
//!
//! ```sh
//! cargo test -p laser-studio --release -- --ignored perf_budget --nocapture
//! ```
//!
//! It measures the code's own cost, not the machine's load: the scenario
//! runs `RUNS` times and the best run is judged (a busy machine slows
//! some runs; it doesn't make the code faster). The thresholds are never
//! loosened to make it pass: a red budget is a regression to find.

use crate::cues::{ClickMode, CueSlot};
use crate::output::Output;
use crate::patterns::Point;
use crate::safety::SafetySettings;
use crate::zones::{Zone, ZoneKind};
use crate::{controls, live, perf, test_support, Engine, Shared, FRAME_INTERVAL};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 99th percentile of the safety stage, ms.
const SAFETY_BUDGET_MS: f64 = 1.0;
/// 99th percentile of the whole frame, ms.
const FRAME_BUDGET_MS: f64 = 6.0;
/// What measuring may add to a frame, ms.
const OVERHEAD_BUDGET_MS: f64 = 0.05;
const FRAMES: usize = perf::WINDOW;
const RUNS: usize = 3;
const TARGET_POINTS: usize = 3_000;

/// An output that takes every frame and goes nowhere.
struct Sink;

impl Output for Sink {
    fn name(&self) -> &str {
        "budget"
    }
    fn set_armed(&mut self, _armed: bool) -> anyhow::Result<()> {
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        std::hint::black_box(points);
    }
}

/// The scenario's studio: four cues on layers 1-4, the point budget at
/// 3 000, eight zones (Blank and Dim, soft edges and gradients), and the
/// master live modifiers moving (rotation, size, perspective, colour).
fn scenario() -> Shared {
    let mut s = test_support::shared();
    s.tempo.set_bpm_manual(128.0, 0.0);
    s.mixer.point_budget = TARGET_POINTS;
    // The four densest cues of the catalogue (first frame), so the
    // layers ask for more than the budget and the mixer fits them in it.
    let mut by_points: Vec<(usize, String)> = s
        .presets
        .iter()
        .map(|p| {
            let n = crate::engine::Animator::default().render(&p.settings, Default::default(), 1.0 / 60.0, &Default::default()).len();
            (n, p.id.clone())
        })
        .collect();
    by_points.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let ids: Vec<String> = by_points.into_iter().take(4).map(|(_, id)| id).collect();
    for (i, id) in ids.iter().enumerate() {
        s.deck.set_slot(id, CueSlot { layer: Some(i as u8 + 1), ..Default::default() });
        assert!(controls::press_cue(&mut s, id, Some(ClickMode::Toggle), true), "{id} plays");
    }
    let zones = (0..8)
        .map(|i| {
            let (cx, cy) = (-0.75 + 0.5 * (i % 4) as f32, if i < 4 { -0.5 } else { 0.5 });
            let square = vec![[cx - 0.2, cy - 0.2], [cx + 0.2, cy - 0.2], [cx + 0.2, cy + 0.2], [cx - 0.2, cy + 0.2]];
            let dim = i % 2 == 1;
            Zone {
                id: i + 1,
                name: format!("Z{i}"),
                kind: if dim { ZoneKind::Dim } else { ZoneKind::Blank },
                level: 0.4,
                points: square,
                audience: false,
                feather: if dim { 0.05 } else { 0.0 },
                gradient: (i == 3).then_some(crate::zones::ZoneGradient { from: [cx - 0.2, cy], to: [cx + 0.2, cy], level_to: 0.1 }),
            }
        })
        .collect();
    s.safety.set(SafetySettings { zones, ..Default::default() }, true).expect("zones accepted");
    s.live = live::LiveModifiers {
        size: 0.8,
        rot_speed: [0.0, 0.2, 0.5],
        perspective: 0.3,
        color: live::ColorOverride::Hue { hue: 200.0 },
        ..Default::default()
    };
    s
}

/// One run of `FRAMES` frames on a simulated clock: each frame's safety
/// and frame times from the engine's own monitor, each `tick` timed from
/// outside (for the run with the monitor removed, `measure` off), in ms,
/// and the points of the last frame.
struct Run {
    safety: Vec<f64>,
    frame: Vec<f64>,
    outside: Vec<f64>,
    points: usize,
}

fn run(measure: bool) -> Run {
    let dir = std::env::temp_dir().join(format!("laser-studio-perf-budget-{}", std::process::id()));
    let mut s = scenario();
    let mut now = Instant::now();
    s.epoch = now;
    s.sim_now = Some(now);
    s.audio_at = now;
    let shared = Mutex::new(s);
    let mut engine = Engine::new(&shared, Some(Box::new(Sink)), dir.join("live.json"), dir.join("layers.json"));
    engine.last = now;
    if !measure {
        engine.perf = None;
    }
    let mut tick = |engine: &mut Engine| {
        now += FRAME_INTERVAL;
        shared.lock().unwrap().sim_now = Some(now);
        engine.tick(&shared, now);
    };
    // Warm-up: animators, caches, the first allocations.
    for _ in 0..60 {
        tick(&mut engine);
    }
    let (mut safety, mut frame, mut outside) = (Vec::with_capacity(FRAMES), Vec::with_capacity(FRAMES), Vec::with_capacity(FRAMES));
    for _ in 0..FRAMES {
        let start = Instant::now();
        tick(&mut engine);
        outside.push(start.elapsed().as_secs_f64() * 1e3);
        if let Some((_, s, _, f)) = engine.perf.as_ref().map(|m| m.last()) {
            safety.push(s as f64);
            frame.push(f as f64);
        }
    }
    let points = shared.lock().unwrap().frame.len();
    engine.shutdown(&shared);
    let _ = std::fs::remove_dir_all(&dir);
    Run { safety, frame, outside, points }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// The scenario really is the one described: ~3 000 points, 4 layers,
/// 8 zones, live modifiers. Not a timing test: it runs with `cargo test`.
#[test]
fn the_budget_scenario_draws_four_layers_and_about_3000_points() {
    let s = scenario();
    assert_eq!(s.deck.active.len(), 4);
    let layers: std::collections::BTreeSet<u8> = s.deck.active.iter().map(|a| s.deck.slot(&a.cue).layer.unwrap_or(1)).collect();
    assert_eq!(layers.len(), 4, "one cue per layer");
    assert_eq!(s.safety.get().zones.len(), 8);
    drop(s);
    let Run { frame, points, .. } = run(true);
    assert_eq!(frame.len(), FRAMES);
    assert!((TARGET_POINTS * 8 / 10..=TARGET_POINTS * 12 / 10).contains(&points), "{points} points");
}

/// What measuring costs a frame, timed on its own: the four clock reads,
/// `record` and `report` (best of 5 batches, so the machine's load
/// doesn't count).
fn measuring_overhead_ms() -> f64 {
    let mut monitor = perf::PerfMonitor::new();
    const N: usize = 20_000;
    (0..5)
        .map(|_| {
            let start = Instant::now();
            for i in 0..N {
                let a = Instant::now();
                let b = Instant::now();
                let c = Instant::now();
                let d = Instant::now();
                monitor.record(b - a, c - b, d - c, d - a, i);
                std::hint::black_box(monitor.report(0));
            }
            start.elapsed().as_secs_f64() * 1e3 / N as f64
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
#[ignore = "timing budget: cargo test -p laser-studio --release -- --ignored perf_budget"]
fn perf_budget_safety_under_1ms_frame_under_6ms_at_p99() {
    let _guard = test_support::time_guard("perf_budget", Duration::from_secs(600));
    // The thresholds are for the optimised build the studio ships as.
    if cfg!(debug_assertions) {
        panic!("perf_budget se lance en mode optimisé : cargo test -p laser-studio --release -- --ignored perf_budget");
    }
    let mut best: Option<(f64, f64, f64, f64, usize)> = None;
    for i in 0..RUNS {
        let Run { safety, frame, points, .. } = run(true);
        let r = (percentile(&safety, 0.5), percentile(&safety, 0.99), percentile(&frame, 0.5), percentile(&frame, 0.99), points);
        eprintln!("perf_budget run {i}: sécurité médiane {:.3} ms, p99 {:.3} ms ; image médiane {:.3} ms, p99 {:.3} ms ; {} points", r.0, r.1, r.2, r.3, r.4);
        if best.is_none_or(|b| r.1 / SAFETY_BUDGET_MS + r.3 / FRAME_BUDGET_MS < b.1 / SAFETY_BUDGET_MS + b.3 / FRAME_BUDGET_MS) {
            best = Some(r);
        }
    }
    let (without, with) = (run(false).outside, run(true).outside);
    let overhead = measuring_overhead_ms();
    eprintln!(
        "perf_budget: image médiane sans mesure {:.3} ms, avec {:.3} ms ; coût de la mesure {:.4} ms par image",
        percentile(&without, 0.5),
        percentile(&with, 0.5),
        overhead
    );
    let (_, safety_p99, _, frame_p99, _) = best.unwrap();
    assert!(safety_p99 < SAFETY_BUDGET_MS, "étage de sécurité p99 {safety_p99:.3} ms ≥ {SAFETY_BUDGET_MS} ms (meilleur de {RUNS} passages)");
    assert!(frame_p99 < FRAME_BUDGET_MS, "image p99 {frame_p99:.3} ms ≥ {FRAME_BUDGET_MS} ms (meilleur de {RUNS} passages)");
    assert!(overhead < OVERHEAD_BUDGET_MS, "la mesure coûte {overhead:.4} ms par image ≥ {OVERHEAD_BUDGET_MS} ms");
}

/// A stall (the `--test-hooks` one: the engine sleeps holding the lock)
/// shows in `Shared::perf` as a late frame, in the render stage, and
/// changes nothing else: still disarmed, nothing lit sent.
#[test]
fn a_stalled_frame_is_reported_late() {
    let shared = Mutex::new(test_support::shared());
    let dir = std::env::temp_dir().join(format!("laser-studio-perf-stall-{}", std::process::id()));
    let mut engine = Engine::new(&shared, Some(Box::new(Sink)), dir.join("live.json"), dir.join("layers.json"));
    let mut now = Instant::now();
    for stall in [0, 40, 0] {
        shared.lock().unwrap().test_stall_ms = stall;
        now += FRAME_INTERVAL;
        engine.tick(&shared, now);
    }
    {
        let s = shared.lock().unwrap();
        assert_eq!((s.perf.frames, s.perf.late_frames, s.perf.late_in_window), (3, 1, 1), "{:?}", s.perf);
        assert!(s.perf.frame.worst_ms >= 40.0 && s.perf.render.worst_ms >= 40.0, "{:?}", s.perf);
        assert!(!s.gate.is_armed() && s.output_lit == 0);
    }
    engine.shutdown(&shared);
    let _ = std::fs::remove_dir_all(&dir);
}
