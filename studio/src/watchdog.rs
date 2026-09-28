//! Engine watchdog and failure handling (T-253).
//!
//! The engine publishes a tick at the top of every frame. A separate
//! thread checks it: if no tick came for `stall_ms` while the gate was
//! armed, it disarms the output directly (the DAC kill switch, as for the
//! e-stop) and raises a trip that the engine records as a disarm with
//! reason « Moteur bloqué » as soon as it runs again. The watchdog only
//! uses atomics: it never takes the `Shared` lock, which may be exactly
//! what the engine is stuck on.

use crate::interlock::EStop;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Default stall limit: six frames at 60 fps.
pub const STALL_MS: u32 = 100;
const POLL: Duration = Duration::from_millis(10);

pub struct EngineHealth {
    epoch: Instant,
    /// ms since `epoch` of the engine's last tick.
    last_tick: AtomicU64,
    /// The gate's armed state as of the last tick (or a later arm request).
    armed: AtomicBool,
    /// Set by the watchdog, taken by the engine once it has disarmed the gate.
    tripped: AtomicBool,
    pub stall_ms: u32,
}

impl Default for EngineHealth {
    fn default() -> Self {
        Self::new(STALL_MS)
    }
}

impl EngineHealth {
    pub fn new(stall_ms: u32) -> Self {
        Self { epoch: Instant::now(), last_tick: AtomicU64::new(0), armed: AtomicBool::new(false), tripped: AtomicBool::new(false), stall_ms }
    }

    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// The engine is alive at `now_ms`, with the gate `armed` or not.
    pub fn tick_at(&self, now_ms: u64, armed: bool) {
        self.last_tick.store(now_ms, Ordering::SeqCst);
        self.armed.store(armed, Ordering::SeqCst);
    }

    pub fn tick(&self, armed: bool) {
        self.tick_at(self.now_ms(), armed);
    }

    /// An arm request succeeded between two ticks: watch from now on.
    pub fn note_armed(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    pub fn stalled_at(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_tick.load(Ordering::SeqCst)) > self.stall_ms as u64
    }

    /// For the UI: false while the engine is late.
    pub fn engine_ok(&self) -> bool {
        !self.stalled_at(self.now_ms())
    }

    /// The watchdog's check at `now_ms`: true when it trips right now
    /// (stalled while armed, not already tripped).
    pub fn check(&self, now_ms: u64) -> bool {
        self.armed.load(Ordering::SeqCst) && self.stalled_at(now_ms) && !self.tripped.swap(true, Ordering::SeqCst)
    }

    /// Raised by the watchdog or the panic hook: the output must go dark.
    pub fn trip(&self) {
        self.tripped.store(true, Ordering::SeqCst);
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    /// The engine records the trip (disarm) and clears it.
    pub fn take_trip(&self) -> bool {
        self.tripped.swap(false, Ordering::SeqCst)
    }
}

/// Starts the watchdog thread; it ends when `running` goes false.
pub fn spawn(health: Arc<EngineHealth>, estop: Arc<EStop>, running: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || {
            while running.load(Ordering::SeqCst) {
                if health.check(health.now_ms()) {
                    log::error!("moteur bloqué depuis plus de {} ms : laser coupé", health.stall_ms);
                    estop.kill_output();
                }
                std::thread::sleep(POLL);
            }
        })
        .expect("failed to start the watchdog thread")
}

/// What the panic hook does before the default report: disarm the output
/// directly, mark the trip (the output stage blanks), and ask every loop
/// to stop so the process ends with the output closed. The engine's own
/// output stage blanks and disarms as it unwinds (`OutputStage`'s `Drop`).
pub fn on_panic(estop: &EStop, health: &EngineHealth, running: &AtomicBool) {
    estop.kill_output();
    health.trip();
    running.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn trips_once_when_stalled_while_armed() {
        let h = EngineHealth::new(100);
        h.tick_at(1_000, true);
        assert!(!h.check(1_050));
        assert!(!h.check(1_100), "exactly the limit is still fine");
        assert!(h.check(1_101), "stalled: trips");
        assert!(!h.check(1_200), "once");
        assert!(h.is_tripped());
        assert!(h.take_trip());
        assert!(!h.take_trip(), "taken once");
    }

    #[test]
    fn a_disarmed_stall_does_not_trip_but_is_reported() {
        let h = EngineHealth::new(100);
        h.tick_at(1_000, false);
        assert!(!h.check(5_000));
        assert!(h.stalled_at(5_000));
        assert!(!h.is_tripped());
    }

    #[test]
    fn regular_ticks_never_trip() {
        let h = EngineHealth::new(100);
        for frame in 0..600u64 {
            let now = frame * 17;
            h.tick_at(now, true);
            assert!(!h.check(now + 16));
        }
    }

    #[test]
    fn arming_between_ticks_is_watched() {
        let h = EngineHealth::new(100);
        h.tick_at(1_000, false);
        h.note_armed();
        assert!(h.check(1_200));
    }

    /// With real time and a real thread: an engine that stops ticking while
    /// armed is cut within a few polls of the limit.
    #[test]
    fn the_thread_fires_the_kill_switch_on_a_stall() {
        let health = Arc::new(EngineHealth::new(50));
        let estop = Arc::new(EStop::default());
        let kills = Arc::new(AtomicUsize::new(0));
        let k = Arc::clone(&kills);
        estop.set_kill_switch(Box::new(move || {
            k.fetch_add(1, Ordering::SeqCst);
        }));
        let running = Arc::new(AtomicBool::new(true));
        health.tick(true);
        let thread = spawn(Arc::clone(&health), Arc::clone(&estop), Arc::clone(&running));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !health.is_tripped() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        running.store(false, Ordering::SeqCst);
        thread.join().unwrap();
        assert!(health.is_tripped());
        assert_eq!(kills.load(Ordering::SeqCst), 1);
        assert!(!estop.is_latched(), "a stall is not an e-stop: re-arming stays possible");
    }

    #[test]
    fn panic_handling_kills_the_output_and_stops_the_studio() {
        let estop = EStop::default();
        let kills = Arc::new(AtomicUsize::new(0));
        let k = Arc::clone(&kills);
        estop.set_kill_switch(Box::new(move || {
            k.fetch_add(1, Ordering::SeqCst);
        }));
        let health = EngineHealth::default();
        let running = AtomicBool::new(true);
        on_panic(&estop, &health, &running);
        assert_eq!(kills.load(Ordering::SeqCst), 1);
        assert!(health.is_tripped());
        assert!(!running.load(Ordering::SeqCst));
    }
}
