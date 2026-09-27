//! Arming: the single place where the laser's armed state changes.
//!
//! Arming is a *request*: `ArmGate::request_arm` succeeds only when every
//! interlock is satisfied, and never for a controller (MIDI) or the system.
//! Every disarm records a reason and a source, and an interlock that drops
//! while armed disarms on the spot.
//!
//! The emergency stop (`EStop`) is a lock-free latch shared by the HTTP
//! fast path, the engine and the output: tripping it blanks the output on
//! the engine's next frame without waiting for the `Shared` lock, and it
//! stays latched until an explicit reset (which never re-arms).

use crate::patterns::Point;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Who asked for an arm or disarm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmSource {
    Ui,
    Keyboard,
    Midi,
    Api,
    System,
}

impl ArmSource {
    pub fn label_fr(self) -> &'static str {
        match self {
            ArmSource::Ui => "interface",
            ArmSource::Keyboard => "clavier",
            ArmSource::Midi => "MIDI",
            ArmSource::Api => "API",
            ArmSource::System => "système",
        }
    }

    /// Lenient parse for request bodies and query strings: anything unknown
    /// is the API, so a disarm is never rejected over its source.
    pub fn parse(s: &str) -> Self {
        match s {
            "ui" => ArmSource::Ui,
            "keyboard" => ArmSource::Keyboard,
            "midi" => ArmSource::Midi,
            "system" => ArmSource::System,
            _ => ArmSource::Api,
        }
    }

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Self {
        [ArmSource::Ui, ArmSource::Keyboard, ArmSource::Midi, ArmSource::Api, ArmSource::System].get(v as usize).copied().unwrap_or(ArmSource::System)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisarmReason {
    Startup,
    User,
    EStop,
    UiLost,
    EngineStall,
    Interlock(String),
    ProfileChange,
    Shutdown,
}

impl DisarmReason {
    pub fn id(&self) -> String {
        match self {
            DisarmReason::Startup => "startup".into(),
            DisarmReason::User => "user".into(),
            DisarmReason::EStop => "estop".into(),
            DisarmReason::UiLost => "ui_lost".into(),
            DisarmReason::EngineStall => "engine_stall".into(),
            DisarmReason::Interlock(id) => format!("interlock:{id}"),
            DisarmReason::ProfileChange => "profile_change".into(),
            DisarmReason::Shutdown => "shutdown".into(),
        }
    }

    pub fn label_fr(&self) -> String {
        match self {
            DisarmReason::Startup => "Démarrage".into(),
            DisarmReason::User => "Désarmement".into(),
            DisarmReason::EStop => "Arrêt d'urgence".into(),
            DisarmReason::UiLost => "Interface perdue".into(),
            DisarmReason::EngineStall => "Moteur bloqué".into(),
            DisarmReason::Interlock(label) => format!("Verrou : {label}"),
            DisarmReason::ProfileChange => "Changement de profil".into(),
            DisarmReason::Shutdown => "Arrêt du studio".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Interlock {
    pub id: &'static str,
    pub label_fr: String,
    pub ok: bool,
}

/// Id of the interlock held open by a latched emergency stop.
pub const ESTOP: &str = "estop";
/// Id of the always-blocking interlock enabled by `--test-interlock`.
pub const TEST: &str = "test";

pub struct ArmGate {
    armed: bool,
    since: Option<SystemTime>,
    source: Option<ArmSource>,
    last_disarm: Option<(DisarmReason, ArmSource, SystemTime)>,
    interlocks: Vec<Interlock>,
}

impl Default for ArmGate {
    /// Always disarmed, reason « Démarrage ».
    fn default() -> Self {
        let mut gate = Self { armed: false, since: None, source: None, last_disarm: Some((DisarmReason::Startup, ArmSource::System, SystemTime::now())), interlocks: Vec::new() };
        gate.register(ESTOP, "Arrêt d'urgence enclenché", true);
        gate
    }
}

impl ArmGate {
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Adds an interlock (or updates its label and state).
    pub fn register(&mut self, id: &'static str, label_fr: &str, ok: bool) {
        match self.interlocks.iter_mut().find(|i| i.id == id) {
            Some(i) => i.label_fr = label_fr.to_string(),
            None => self.interlocks.push(Interlock { id, label_fr: label_fr.to_string(), ok: true }),
        }
        self.set_interlock(id, ok);
    }

    /// Labels of the interlocks that currently block arming.
    pub fn blocking(&self) -> Vec<String> {
        self.interlocks.iter().filter(|i| !i.ok).map(|i| i.label_fr.clone()).collect()
    }

    pub fn interlock_ok(&self, id: &str) -> bool {
        self.interlocks.iter().find(|i| i.id == id).is_none_or(|i| i.ok)
    }

    /// Arms only if every interlock is satisfied. A controller (MIDI) or
    /// the system can never arm: only a person at the UI, keyboard or API.
    pub fn request_arm(&mut self, src: ArmSource) -> Result<(), Vec<String>> {
        if matches!(src, ArmSource::Midi | ArmSource::System) {
            return Err(vec![format!("L'armement depuis « {} » n'est pas autorisé", src.label_fr())]);
        }
        let blocking = self.blocking();
        if !blocking.is_empty() {
            return Err(blocking);
        }
        if !self.armed {
            self.armed = true;
            self.since = Some(SystemTime::now());
            self.source = Some(src);
        }
        Ok(())
    }

    /// Always accepted. Records the reason even when already disarmed, so
    /// the UI shows the latest cause (e.g. an e-stop over a disarm).
    pub fn disarm(&mut self, reason: DisarmReason, src: ArmSource) {
        self.armed = false;
        self.since = None;
        self.source = None;
        self.last_disarm = Some((reason, src, SystemTime::now()));
    }

    /// Unknown ids are added (with the id as label), so a failing
    /// interlock blocks even if it was never registered.
    pub fn set_interlock(&mut self, id: &'static str, ok: bool) {
        let index = match self.interlocks.iter().position(|i| i.id == id) {
            Some(i) => i,
            None => {
                self.interlocks.push(Interlock { id, label_fr: id.to_string(), ok: true });
                self.interlocks.len() - 1
            }
        };
        let lock = &mut self.interlocks[index];
        let dropped = lock.ok && !ok;
        lock.ok = ok;
        if dropped && self.armed {
            let label = lock.label_fr.clone();
            self.disarm(DisarmReason::Interlock(label), ArmSource::System);
        }
    }

    /// Blanks every point unless armed. The preview keeps the unblanked frame.
    pub fn gate(&self, frame: &mut [Point]) {
        blank_unless(self.armed, frame);
    }

    /// Brings the gate in line with the e-stop latch: a new trip disarms
    /// with reason `EStop` and holds the `estop` interlock open; a reset
    /// closes it again (without arming).
    pub fn sync_estop(&mut self, estop: &EStop) {
        match estop.state() {
            Some((src, _)) if self.interlock_ok(ESTOP) => {
                self.disarm(DisarmReason::EStop, src);
                self.set_interlock(ESTOP, false);
            }
            None if !self.interlock_ok(ESTOP) => self.set_interlock(ESTOP, true),
            _ => {}
        }
    }

    /// `POST /api/estop/reset`: records a trip the gate had not seen yet,
    /// then releases the latch. Never arms.
    pub fn reset_estop(&mut self, estop: &EStop) {
        self.sync_estop(estop);
        estop.reset();
        self.sync_estop(estop);
    }

    pub fn status(&self, estop: &EStop) -> ArmStatus {
        let ms = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        ArmStatus {
            armed: self.armed && !estop.is_latched(),
            since: self.since.map(ms),
            source: self.source,
            source_fr: self.source.map(ArmSource::label_fr),
            last_disarm: self.last_disarm.as_ref().map(|(reason, src, at)| LastDisarm {
                reason: reason.id(),
                reason_fr: reason.label_fr(),
                source: *src,
                source_fr: src.label_fr(),
                at: ms(*at),
            }),
            blocking: self.blocking(),
            estop: estop.state().map(|(src, at)| EStopStatus { source: src, source_fr: src.label_fr(), at }),
        }
    }
}

/// `GET /api/arm`. Times are milliseconds since the Unix epoch.
#[derive(Serialize)]
pub struct ArmStatus {
    pub armed: bool,
    pub since: Option<u64>,
    pub source: Option<ArmSource>,
    pub source_fr: Option<&'static str>,
    pub last_disarm: Option<LastDisarm>,
    pub blocking: Vec<String>,
    pub estop: Option<EStopStatus>,
}

#[derive(Serialize)]
pub struct LastDisarm {
    pub reason: String,
    pub reason_fr: String,
    pub source: ArmSource,
    pub source_fr: &'static str,
    pub at: u64,
}

#[derive(Serialize)]
pub struct EStopStatus {
    pub source: ArmSource,
    pub source_fr: &'static str,
    pub at: u64,
}

pub fn blank_unless(armed: bool, frame: &mut [Point]) {
    if !armed {
        for p in frame {
            p.r = 0.0;
            p.g = 0.0;
            p.b = 0.0;
        }
    }
}

type KillSwitch = Box<dyn Fn() + Send + Sync>;

/// The latched emergency stop. Lock-free to read and to trip, so neither
/// a busy HTTP handler nor the `Shared` lock can delay it. Not saved: a
/// restart is disarmed anyway.
#[derive(Default)]
pub struct EStop {
    latched: AtomicBool,
    source: AtomicU8,
    at_ms: AtomicU64,
    /// Disarms the output directly (DAC-level), called on every trip.
    kill: Mutex<Option<KillSwitch>>,
}

impl EStop {
    pub fn set_kill_switch(&self, kill: KillSwitch) {
        *self.kill.lock().unwrap() = Some(kill);
    }

    pub fn trip(&self, src: ArmSource) {
        if !self.latched.load(Ordering::SeqCst) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            self.source.store(src.to_u8(), Ordering::SeqCst);
            self.at_ms.store(now, Ordering::SeqCst);
        }
        self.latched.store(true, Ordering::SeqCst);
        // A poisoned lock still holds the switch: use it anyway.
        let kill = self.kill.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(kill) = kill.as_ref() {
            kill();
        }
    }

    pub fn is_latched(&self) -> bool {
        self.latched.load(Ordering::SeqCst)
    }

    /// Source and time (ms since the epoch) of the trip, while latched.
    pub fn state(&self) -> Option<(ArmSource, u64)> {
        self.is_latched().then(|| (ArmSource::from_u8(self.source.load(Ordering::SeqCst)), self.at_ms.load(Ordering::SeqCst)))
    }

    /// Only through `ArmGate::reset_estop`, so the gate sees the trip first.
    fn reset(&self) {
        self.latched.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn lit_frame() -> Vec<Point> {
        vec![Point::lit(0.0, 0.0, 1.0, 0.5, 0.2), Point::lit(0.5, 0.5, 0.0, 1.0, 0.0), Point::blanked(0.1, 0.1)]
    }

    #[test]
    fn starts_disarmed_with_reason_startup() {
        let gate = ArmGate::default();
        assert!(!gate.is_armed());
        let status = gate.status(&EStop::default());
        assert!(!status.armed);
        assert_eq!(status.last_disarm.unwrap().reason_fr, "Démarrage");
        assert!(status.blocking.is_empty());
    }

    #[test]
    fn arm_and_disarm_record_source_and_reason() {
        let mut gate = ArmGate::default();
        gate.request_arm(ArmSource::Keyboard).unwrap();
        assert!(gate.is_armed());
        assert_eq!(gate.status(&EStop::default()).source, Some(ArmSource::Keyboard));
        gate.disarm(DisarmReason::User, ArmSource::Ui);
        assert!(!gate.is_armed());
        let last = gate.status(&EStop::default()).last_disarm.unwrap();
        assert_eq!((last.reason.as_str(), last.source), ("user", ArmSource::Ui));
    }

    /// Transition table: (interlock ok?, armed before?) × arm request.
    #[test]
    fn arming_needs_every_interlock() {
        for (lock_ok, armed_before) in [(true, false), (true, true), (false, false)] {
            let mut gate = ArmGate::default();
            gate.register("door", "Porte ouverte", true);
            if armed_before {
                gate.request_arm(ArmSource::Ui).unwrap();
            }
            gate.set_interlock("door", lock_ok);
            let result = gate.request_arm(ArmSource::Ui);
            assert_eq!(result.is_ok(), lock_ok, "lock_ok={lock_ok} armed_before={armed_before}");
            assert_eq!(gate.is_armed(), lock_ok);
            if !lock_ok {
                assert_eq!(result.unwrap_err(), vec!["Porte ouverte".to_string()]);
            }
        }
    }

    #[test]
    fn a_dropping_interlock_disarms_at_once_with_its_label() {
        let mut gate = ArmGate::default();
        gate.register("door", "Porte ouverte", true);
        gate.request_arm(ArmSource::Ui).unwrap();
        gate.set_interlock("door", false);
        assert!(!gate.is_armed());
        let last = gate.status(&EStop::default()).last_disarm.unwrap();
        assert_eq!(last.reason_fr, "Verrou : Porte ouverte");
        assert_eq!(last.source, ArmSource::System);
        // Coming back does not re-arm.
        gate.set_interlock("door", true);
        assert!(!gate.is_armed());
    }

    #[test]
    fn unregistered_failing_interlocks_still_block() {
        let mut gate = ArmGate::default();
        gate.set_interlock("mystery", false);
        assert_eq!(gate.request_arm(ArmSource::Ui).unwrap_err(), vec!["mystery".to_string()]);
    }

    #[test]
    fn the_test_interlock_blocks_arming() {
        let mut gate = ArmGate::default();
        gate.register(TEST, "Verrou de test", false);
        assert!(gate.request_arm(ArmSource::Ui).is_err());
        assert_eq!(gate.status(&EStop::default()).blocking, vec!["Verrou de test".to_string()]);
    }

    #[test]
    fn controllers_and_the_system_can_never_arm() {
        let mut gate = ArmGate::default();
        assert!(gate.request_arm(ArmSource::Midi).is_err());
        assert!(gate.request_arm(ArmSource::System).is_err());
        assert!(!gate.is_armed());
    }

    #[test]
    fn disarm_is_always_accepted() {
        let mut gate = ArmGate::default();
        gate.set_interlock("door", false);
        gate.disarm(DisarmReason::User, ArmSource::Midi);
        assert!(!gate.is_armed());
    }

    #[test]
    fn gate_blanks_every_colour_only_when_disarmed() {
        let mut gate = ArmGate::default();
        let mut frame = lit_frame();
        gate.gate(&mut frame);
        assert!(frame.iter().all(|p| !p.is_lit()));
        assert_eq!((frame[1].x, frame[1].y), (0.5, 0.5), "positions are kept");
        gate.request_arm(ArmSource::Ui).unwrap();
        let mut frame = lit_frame();
        gate.gate(&mut frame);
        assert_eq!(frame.iter().filter(|p| p.is_lit()).count(), 2);
    }

    #[test]
    fn estop_disarms_latches_and_blocks_arming() {
        let mut gate = ArmGate::default();
        let estop = EStop::default();
        gate.request_arm(ArmSource::Ui).unwrap();
        estop.trip(ArmSource::Keyboard);
        // Before the gate syncs, the status already reports disarmed.
        assert!(!gate.status(&estop).armed);
        gate.sync_estop(&estop);
        assert!(!gate.is_armed());
        let status = gate.status(&estop);
        assert_eq!(status.last_disarm.unwrap().reason, "estop");
        assert_eq!(status.estop.unwrap().source, ArmSource::Keyboard);
        assert_eq!(gate.request_arm(ArmSource::Keyboard).unwrap_err(), vec!["Arrêt d'urgence enclenché".to_string()]);
    }

    #[test]
    fn reset_releases_the_latch_but_never_arms() {
        let mut gate = ArmGate::default();
        let estop = EStop::default();
        gate.request_arm(ArmSource::Ui).unwrap();
        estop.trip(ArmSource::Ui);
        // Reset before the engine ever synced: the trip is still recorded.
        gate.reset_estop(&estop);
        assert!(!estop.is_latched());
        assert!(!gate.is_armed());
        assert_eq!(gate.status(&estop).last_disarm.unwrap().reason, "estop");
        assert!(gate.blocking().is_empty());
        gate.request_arm(ArmSource::Keyboard).unwrap();
        assert!(gate.is_armed());
    }

    #[test]
    fn a_second_trip_keeps_the_first_source_and_time() {
        let estop = EStop::default();
        estop.trip(ArmSource::Keyboard);
        let first = estop.state().unwrap();
        estop.trip(ArmSource::Midi);
        assert_eq!(estop.state().unwrap(), first);
    }

    #[test]
    fn trip_fires_the_kill_switch_every_time() {
        let estop = EStop::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        estop.set_kill_switch(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        estop.trip(ArmSource::Api);
        estop.trip(ArmSource::Api);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn sources_parse_leniently() {
        assert_eq!(ArmSource::parse("keyboard"), ArmSource::Keyboard);
        assert_eq!(ArmSource::parse("ui"), ArmSource::Ui);
        assert_eq!(ArmSource::parse("whatever"), ArmSource::Api);
        for s in [ArmSource::Ui, ArmSource::Keyboard, ArmSource::Midi, ArmSource::Api, ArmSource::System] {
            assert_eq!(ArmSource::from_u8(s.to_u8()), s);
        }
    }
}
