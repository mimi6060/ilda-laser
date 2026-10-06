//! Engine performance monitor (T-448).
//!
//! The engine thread times three stages of every frame, **rendu** (from
//! the top of the frame to the safety stage: the shared bookkeeping,
//! animators, layers, modifiers, mixer, zones and calibration),
//! **sécurité** (`safety_chain`) and **sortie** (the main output's gate
//! and emit, then the venue's other projectors), and the whole frame (top
//! of the frame to the closing critical section). `PerfMonitor` keeps the
//! last `WINDOW` frames in fixed rings allocated once: recording a frame
//! allocates nothing and takes no lock. Its report (a plain copy, no heap)
//! is stored in `Shared::perf` inside the critical section that already
//! ends every frame, and `GET /api/perf` reads it.
//!
//! The monitor only informs: `watchdog::EngineHealth` stays the one judge
//! of a stalled engine (no tick for `STALL_MS` while armed: the output is
//! killed and the gate disarmed). A late or slow frame changes nothing in
//! what the engine sends. Nothing is saved to disk.

use serde::Serialize;
use std::time::Duration;

/// 10 s at 60 frames a second.
pub const WINDOW: usize = 600;
/// The panel turns orange when the worst frame of the window took longer.
pub const WARN_WORST_MS: f32 = 8.0;
/// A frame that took longer than one frame interval (60 fps) is late.
pub const LATE_MS: f32 = 1000.0 / 60.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct StageStat {
    pub mean_ms: f32,
    pub worst_ms: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PerfReport {
    pub render: StageStat,
    pub safety: StageStat,
    pub output: StageStat,
    pub frame: StageStat,
    /// Points of the last frame sent to the main output.
    pub points: usize,
    /// Late frames (longer than `LATE_MS`) since start-up.
    pub late_frames: u64,
    /// Late frames among the frames of the window.
    pub late_in_window: u32,
    /// Pages connected (heartbeats, presence.rs); filled in by the route.
    pub clients: usize,
    /// Seconds of frames the window holds right now (≤ 10).
    pub window_s: f32,
    /// Frames measured since start-up.
    pub frames: u64,
}

impl PerfReport {
    /// For the page: 2 = late frames in the window (red), 1 = the worst
    /// frame of the window over `WARN_WORST_MS` (orange), 0 = normal.
    pub fn level(&self) -> u8 {
        if self.late_in_window > 0 {
            2
        } else if self.frame.worst_ms > WARN_WORST_MS {
            1
        } else {
            0
        }
    }
}

/// One stage's last `WINDOW` durations (ms), with a running sum.
struct Ring {
    values: Box<[f32; WINDOW]>,
    /// f64: no drift from adding and taking away for days.
    sum: f64,
}

impl Ring {
    fn new() -> Self {
        Self { values: Box::new([0.0; WINDOW]), sum: 0.0 }
    }

    /// Writes `ms` at `slot`, returning the value it replaces.
    fn put(&mut self, slot: usize, ms: f32, full: bool) -> f32 {
        let old = if full { self.values[slot] } else { 0.0 };
        self.values[slot] = ms;
        self.sum += ms as f64 - old as f64;
        old
    }

    fn stat(&self, len: usize) -> StageStat {
        if len == 0 {
            return StageStat::default();
        }
        let worst = self.values[..len].iter().copied().fold(0.0f32, f32::max);
        StageStat { mean_ms: (self.sum / len as f64).max(0.0) as f32, worst_ms: worst }
    }
}

pub struct PerfMonitor {
    render: Ring,
    safety: Ring,
    output: Ring,
    frame: Ring,
    /// Next slot to write; `len` slots hold values (≤ WINDOW).
    next: usize,
    len: usize,
    points: usize,
    late_frames: u64,
    late_in_window: u32,
    frames: u64,
}

impl Default for PerfMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

impl PerfMonitor {
    /// The rings are allocated here, once.
    pub fn new() -> Self {
        Self { render: Ring::new(), safety: Ring::new(), output: Ring::new(), frame: Ring::new(), next: 0, len: 0, points: 0, late_frames: 0, late_in_window: 0, frames: 0 }
    }

    /// One frame's stages and total, and the points it sent. No allocation.
    pub fn record(&mut self, render: Duration, safety: Duration, output: Duration, frame: Duration, points: usize) {
        let full = self.len == WINDOW;
        let slot = self.next;
        self.render.put(slot, ms(render), full);
        self.safety.put(slot, ms(safety), full);
        self.output.put(slot, ms(output), full);
        let frame_ms = ms(frame);
        let old = self.frame.put(slot, frame_ms, full);
        if full && old > LATE_MS {
            self.late_in_window -= 1;
        }
        if frame_ms > LATE_MS {
            self.late_in_window += 1;
            self.late_frames += 1;
        }
        self.next = (slot + 1) % WINDOW;
        self.len = (self.len + 1).min(WINDOW);
        self.points = points;
        self.frames += 1;
    }

    /// The last frame's (render, safety, output, frame) times, ms.
    #[cfg(test)]
    pub fn last(&self) -> (f32, f32, f32, f32) {
        let slot = (self.next + WINDOW - 1) % WINDOW;
        (self.render.values[slot], self.safety.values[slot], self.output.values[slot], self.frame.values[slot])
    }

    /// Means and worst cases over the window. A copy with no heap.
    pub fn report(&self, clients: usize) -> PerfReport {
        PerfReport {
            render: self.render.stat(self.len),
            safety: self.safety.stat(self.len),
            output: self.output.stat(self.len),
            frame: self.frame.stat(self.len),
            points: self.points,
            late_frames: self.late_frames,
            late_in_window: self.late_in_window,
            clients,
            window_s: self.len as f32 / 60.0,
            frames: self.frames,
        }
    }
}

/// The start-up check (one log line): whether the data folder takes a
/// file, and how much space is left on its disk.
pub fn startup_check(data_dir: &std::path::Path) -> String {
    let probe = data_dir.join(format!(".perf-probe-{}", std::process::id()));
    let writable = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    let free = match crate::backup::free_bytes(data_dir) {
        Some(b) => format!("{:.1} Go libres", b as f64 / 1e9),
        None => "espace libre inconnu".to_string(),
    };
    format!("contrôle au démarrage : dossier de données {} ({}), {free}", if writable { "inscriptible" } else { "NON inscriptible" }, data_dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(ms: f32) -> Duration {
        Duration::from_secs_f32(ms / 1000.0)
    }

    #[test]
    fn an_empty_monitor_reports_zeros() {
        let r = PerfMonitor::new().report(2);
        assert_eq!(r, PerfReport { clients: 2, ..Default::default() });
    }

    #[test]
    fn mean_and_worst_over_the_window() {
        let mut m = PerfMonitor::new();
        m.record(d(1.0), d(0.2), d(0.1), d(1.5), 100);
        m.record(d(3.0), d(0.4), d(0.3), d(4.0), 200);
        let r = m.report(0);
        assert!((r.render.mean_ms - 2.0).abs() < 1e-4, "{r:?}");
        assert!((r.render.worst_ms - 3.0).abs() < 1e-4);
        assert!((r.safety.mean_ms - 0.3).abs() < 1e-4);
        assert!((r.output.worst_ms - 0.3).abs() < 1e-4);
        assert!((r.frame.mean_ms - 2.75).abs() < 1e-4);
        assert_eq!((r.points, r.frames, r.late_frames), (200, 2, 0));
        assert_eq!(r.level(), 0);
        m.record(d(8.5), d(0.1), d(0.1), d(9.0), 1);
        assert_eq!(m.report(0).level(), 1, "worst over 8 ms: orange");
        assert!((r.window_s - 2.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn old_values_leave_the_window() {
        let mut m = PerfMonitor::new();
        m.record(d(9.0), d(5.0), d(1.0), d(12.0), 10);
        for _ in 0..WINDOW {
            m.record(d(1.0), d(0.5), d(0.25), d(2.0), 10);
        }
        let r = m.report(0);
        assert!((r.render.worst_ms - 1.0).abs() < 1e-4, "the 9 ms frame left: {r:?}");
        assert!((r.render.mean_ms - 1.0).abs() < 1e-4);
        assert!((r.safety.mean_ms - 0.5).abs() < 1e-4);
        assert!((r.frame.worst_ms - 2.0).abs() < 1e-4);
        assert!((r.window_s - 10.0).abs() < 1e-4);
        assert_eq!(r.frames, WINDOW as u64 + 1);
    }

    #[test]
    fn late_frames_count_since_start_and_in_the_window() {
        let mut m = PerfMonitor::new();
        m.record(d(1.0), d(0.1), d(0.1), d(16.0), 1);
        assert_eq!(m.report(0).late_frames, 0, "16 ms is on time");
        m.record(d(200.0), d(0.1), d(0.1), d(200.0), 1);
        m.record(d(20.0), d(0.1), d(0.1), d(20.0), 1);
        let r = m.report(0);
        assert_eq!((r.late_frames, r.late_in_window), (2, 2));
        assert_eq!(r.level(), 2, "late: red");
        // Three frames in, WINDOW - 1 more: the first two left.
        for _ in 0..WINDOW - 1 {
            m.record(d(1.0), d(0.1), d(0.1), d(1.0), 1);
        }
        assert_eq!(m.report(0).late_in_window, 1, "the 200 ms frame left the window");
        m.record(d(1.0), d(0.1), d(0.1), d(1.0), 1);
        let r = m.report(0);
        assert_eq!((r.late_frames, r.late_in_window), (2, 0));
    }

    #[test]
    fn the_mean_does_not_drift_over_many_windows() {
        let mut m = PerfMonitor::new();
        for i in 0..100 * WINDOW {
            let v = if i % 7 == 0 { 3.3 } else { 0.7 };
            m.record(d(v), d(v), d(v), d(v), 1);
        }
        for _ in 0..WINDOW {
            m.record(d(1.0), d(1.0), d(1.0), d(1.0), 1);
        }
        let r = m.report(0);
        assert!((r.render.mean_ms - 1.0).abs() < 1e-4, "{r:?}");
    }

    #[test]
    fn the_startup_check_reports_a_writable_folder_and_free_space() {
        let dir = std::env::temp_dir().join(format!("laser-studio-perf-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let line = startup_check(&dir);
        assert!(line.contains("inscriptible") && !line.contains("NON"), "{line}");
        assert!(line.contains("Go libres"), "{line}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe file is removed");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(startup_check(&dir.join("missing")).contains("NON inscriptible"));
    }
}
