//! Ableton Link (T-154): the tempo clock (T-150) can follow a Link
//! session (Ableton Live, Rekordbox, Traktor, phone apps…) on the LAN,
//! tempo and phase, and a BPM set here goes to the session.
//!
//! Licence. Ableton's Link library is GPLv2+ (or a proprietary licence
//! from Ableton). It is only compiled with the Cargo feature
//! `ableton-link`, **off by default**: a default build contains no GPL
//! code and this module then only answers "not built" (the UI greys the
//! control). A build with the feature is a GPL combined work, for the
//! operator's own use, never distributed, and never together with the
//! proprietary ShowNET SDK (`shownet`, refused at compile time below).
//!
//! Behaviour, when the operator turns it on (*Ableton Link : Activé*):
//! - Link is never on at start-up and the choice is not saved: the
//!   multicast discovery (224.76.78.75:20808, every interface) only runs
//!   between the operator's "on" and "off".
//! - Each engine frame reads the session (tempo, beat at "now" with the
//!   quantum, 4 beats by default) and `TempoClock::follow_link` takes it:
//!   same BPM, beat aligned modulo the quantum, so the bar's "1" is the
//!   session's. A session tempo outside 40–250 BPM is not taken (shown).
//! - A BPM the operator sets here (field, Tap, ×2, ÷2, the same controls
//!   from OSC, MIDI, the keyboard or the command line) is proposed to the
//!   session, which every peer then follows: `operator_tempo`, called by
//!   controls.rs, is the only way in. Any other BPM change (opening a
//!   project, a macro, a show list, a timeline, the autopilot…) stays
//!   local and the session's tempo overwrites it on the next frame: the
//!   studio never changes a whole session's tempo on its own. Nudge,
//!   *Sync 1* and *Tempo auto* are refused meanwhile (controls.rs): the
//!   phase belongs to the session. Following the MIDI clock and Link at
//!   once is refused (409), either way round.
//! - It only ever moves the tempo: no arming, no cue, and the flash
//!   limiters and every safety stage still run after.
//!
//! The session sits behind `LinkSession`, so the logic is tested with a
//! fake in every build; the real one (`native`) exists with the feature
//! only. Tests never start the real one, except one opt-in LAN test.

use crate::controls::ControlSource;
use crate::interlock::ArmSource;
use crate::tempo::TempoSource;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

#[cfg(all(feature = "ableton-link", feature = "shownet"))]
compile_error!("`ableton-link` (GPL) and `shownet` (proprietary SDK under NDA) can't be in the same build: such a binary could never be distributed");

/// Beats over which phase is aligned with the session (a 4/4 bar).
pub const DEFAULT_QUANTUM: f64 = 4.0;
pub const MAX_QUANTUM: f64 = 16.0;

pub const NOT_BUILT: &str = "Ableton Link n'est pas compilé dans cette version (fonction « ableton-link », désactivée par défaut à cause de sa licence GPL).";
pub const FOLLOWING: &str = "tempo suivi sur Ableton Link : Nudge, Sync 1 et Tempo auto sont ignorés (le BPM, lui, est envoyé à la session)";
pub const MIDI_CLOCK_ON: &str = "Le tempo suit déjà l'horloge MIDI : arrêtez-la avant d'activer Ableton Link.";
pub const LINK_ON: &str = "Le tempo suit Ableton Link : désactivez-le avant de suivre l'horloge MIDI.";

/// One Link session, as the studio sees it. Times are the session's
/// host clock, in microseconds.
pub trait LinkSession: Send {
    fn enable(&mut self, on: bool);
    fn peers(&self) -> usize;
    fn clock_micros(&self) -> i64;
    /// The session's tempo, and its beat at `at` for this `quantum`.
    fn capture(&mut self, at: i64, quantum: f64) -> (f64, f64);
    /// Proposes `bpm` from `at` on: every peer follows.
    fn propose_tempo(&mut self, bpm: f64, at: i64);
}

/// Where Link stands (`/api/link.state`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    #[default]
    Off,
    /// The clock follows the session.
    Following,
    /// The session's tempo is outside 40–250 BPM: the clock keeps its own.
    OutOfRange,
}

/// What the operator chose (logged on change, never saved).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct LinkPrefs {
    enabled: bool,
    quantum: f64,
}

/// `Shared.link`.
pub struct LinkState {
    pub quantum: f64,
    pub peers: usize,
    pub status: LinkStatus,
    /// The session's tempo at the last frame.
    pub session_bpm: Option<f64>,
    /// BPM changes sent to the session.
    pub proposed: u64,
    session: Option<Box<dyn LinkSession>>,
    /// An operator's BPM waiting for the next frame (`operator_tempo`).
    pending: Option<f64>,
}

impl Default for LinkState {
    fn default() -> Self {
        Self { quantum: DEFAULT_QUANTUM, peers: 0, status: LinkStatus::Off, session_bpm: None, proposed: 0, session: None, pending: None }
    }
}

impl LinkState {
    pub fn enabled(&self) -> bool {
        self.session.is_some()
    }

    fn prefs(&self) -> LinkPrefs {
        LinkPrefs { enabled: self.enabled(), quantum: self.quantum }
    }
}

/// Whether this build has the real Link library.
pub fn available() -> bool {
    cfg!(feature = "ableton-link")
}

/// A new real session at `bpm` (not yet enabled), or why there is none.
fn open_session(bpm: f64) -> Result<Box<dyn LinkSession>, &'static str> {
    #[cfg(feature = "ableton-link")]
    {
        Ok(Box::new(native::Native::new(bpm)))
    }
    #[cfg(not(feature = "ableton-link"))]
    {
        let _ = bpm;
        Err(NOT_BUILT)
    }
}

/// Turns Link on with `session` (the real one, or a fake in tests).
fn start(s: &mut Shared, mut session: Box<dyn LinkSession>) {
    session.enable(true);
    log::info!("Ableton Link activé ({:.1} BPM)", s.tempo.bpm);
    s.link.session = Some(session);
    s.link.pending = None;
    s.link.status = LinkStatus::Off;
}

/// Turns Link off: the tempo goes back to manual at its BPM and phase.
fn stop(s: &mut Shared) {
    if let Some(mut session) = s.link.session.take() {
        session.enable(false);
        log::info!("Ableton Link désactivé : tempo manuel ({:.1} BPM)", s.tempo.bpm);
    }
    s.link.peers = 0;
    s.link.status = LinkStatus::Off;
    s.link.session_bpm = None;
    s.link.pending = None;
    s.tempo.set_link(false);
}

/// Once per engine frame (main.rs), after the MIDI clock.
pub fn frame(s: &mut Shared, now: Instant) {
    let t = now.saturating_duration_since(s.epoch).as_secs_f64();
    let (link, tempo) = (&mut s.link, &mut s.tempo);
    let Some(session) = link.session.as_mut() else { return };
    // The session's clock at `now` (the frame started a moment ago).
    let lag = Instant::now().saturating_duration_since(now).as_micros() as i64;
    let at = session.clock_micros() - lag;
    link.peers = session.peers();
    // The operator's BPM goes to the session; any other local change is
    // overwritten by the session's tempo just below.
    if let Some(bpm) = link.pending.take() {
        session.propose_tempo(bpm, at);
        link.proposed += 1;
    }
    let (bpm, beat) = session.capture(at, link.quantum);
    link.session_bpm = bpm.is_finite().then_some(bpm);
    link.status = if tempo.follow_link(bpm, beat, link.quantum, t) {
        LinkStatus::Following
    } else {
        // Not taken: the clock keeps its tempo, still as the Link source
        // (a BPM set here still goes to the session).
        if tempo.source != TempoSource::Link {
            tempo.set_link(true);
        }
        LinkStatus::OutOfRange
    };
}

/// Whether a control from `source` is the operator's own hand (proposes a
/// BPM to the session) rather than a programmed action.
fn is_operator(source: ControlSource) -> bool {
    match source {
        ControlSource::Ui
        | ControlSource::Keyboard
        | ControlSource::Midi
        | ControlSource::Dmx
        | ControlSource::Osc
        | ControlSource::Network
        | ControlSource::Remote
        | ControlSource::Api
        | ControlSource::Command => true,
        ControlSource::Timeline | ControlSource::Audio | ControlSource::Macro | ControlSource::System | ControlSource::Autopilot => false,
    }
}

/// The operator's tempo controls (controls.rs: field, Tap, ×2, ÷2) after
/// they changed the BPM: with Link on, the clock's BPM is proposed to the
/// session at the next frame. The only way a BPM reaches the session;
/// `TempoClock::set_bpm_manual` and the other setters stay local.
pub fn operator_tempo(s: &mut Shared, source: ControlSource) {
    if s.link.enabled() && is_operator(source) {
        s.link.pending = Some(s.tempo.bpm);
    }
}

/// The tempo bar's readout (`/api/state.tempo.link`).
pub fn summary(s: &Shared) -> Value {
    let l = &s.link;
    json!({ "enabled": l.enabled(), "peers": l.peers, "state": l.status, "session_bpm": l.session_bpm })
}

/// `GET /api/link`.
pub fn view(s: &Shared) -> Value {
    let l = &s.link;
    json!({
        "available": available(),
        "why": (!available()).then_some(NOT_BUILT),
        "enabled": l.enabled(),
        "quantum": l.quantum,
        "peers": l.peers,
        "state": l.status,
        "session_bpm": l.session_bpm,
        "proposed": l.proposed,
        "following": s.tempo.source == TempoSource::Link,
    })
}

/// `POST /api/link` `{ "enabled": bool, "quantum": 1..16 }` (both optional).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    pub enabled: Option<bool>,
    pub quantum: Option<f64>,
}

/// Applies a `POST /api/link`; the status and message on refusal.
pub fn update(s: &mut Shared, req: LinkRequest) -> Result<(), (u16, String)> {
    update_with(s, req, open_session)
}

fn update_with(s: &mut Shared, req: LinkRequest, open: impl FnOnce(f64) -> Result<Box<dyn LinkSession>, &'static str>) -> Result<(), (u16, String)> {
    let before = s.link.prefs();
    if let Some(q) = req.quantum {
        if !q.is_finite() || !(1.0..=MAX_QUANTUM).contains(&q) || q.fract() != 0.0 {
            return Err((400, format!("quantum : un nombre entier de temps de 1 à {MAX_QUANTUM} est attendu")));
        }
    }
    match req.enabled {
        Some(true) if !s.link.enabled() => {
            if s.midi.store.devices.clock.follow {
                return Err((409, MIDI_CLOCK_ON.into()));
            }
            let session = open(s.tempo.bpm).map_err(|e| (501, e.to_string()))?;
            start(s, session);
        }
        Some(false) if s.link.enabled() => stop(s),
        _ => {}
    }
    if let Some(q) = req.quantum {
        s.link.quantum = q;
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &s.link.prefs()) {
        s.gate.log().record("link_settings", Some(ArmSource::Ui.id()), change);
    }
    Ok(())
}

/// The real session: Ableton's Link library through `rusty_link` (both
/// GPLv2+). Only in a build with the `ableton-link` feature.
#[cfg(feature = "ableton-link")]
mod native {
    use super::LinkSession;
    use rusty_link::{AblLink, SessionState};

    pub struct Native {
        link: AblLink,
        state: SessionState,
    }

    impl Native {
        /// Created disabled: nothing goes on the network before `enable`.
        pub fn new(bpm: f64) -> Self {
            Self { link: AblLink::new(bpm), state: SessionState::new() }
        }
    }

    impl LinkSession for Native {
        fn enable(&mut self, on: bool) {
            self.link.enable(on);
        }

        fn peers(&self) -> usize {
            self.link.num_peers() as usize
        }

        fn clock_micros(&self) -> i64 {
            self.link.clock_micros()
        }

        fn capture(&mut self, at: i64, quantum: f64) -> (f64, f64) {
            self.link.capture_app_session_state(&mut self.state);
            (self.state.tempo(), self.state.beat_at_time(at, quantum))
        }

        fn propose_tempo(&mut self, bpm: f64, at: i64) {
            self.link.capture_app_session_state(&mut self.state);
            self.state.set_tempo(bpm, at);
            self.link.commit_app_session_state(&self.state);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{Duration, Instant};

        /// Two real sessions in this process find each other and share a
        /// tempo. Link's discovery is multicast on **every interface**
        /// (the LAN too), so this never runs in `cargo test`: only by hand,
        /// on a machine whose network may see it, with
        /// `LASER_STUDIO_LINK_LAN_TEST=1 cargo test -p laser-studio
        /// --features ableton-link -- --ignored two_real_sessions`.
        #[test]
        #[ignore = "Link multicast on the LAN: run by hand only"]
        fn two_real_sessions_share_the_tempo() {
            if std::env::var("LASER_STUDIO_LINK_LAN_TEST").as_deref() != Ok("1") {
                eprintln!("skipped: set LASER_STUDIO_LINK_LAN_TEST=1 to allow Link multicast");
                return;
            }
            let (mut a, mut b) = (Native::new(120.0), Native::new(120.0));
            a.enable(true);
            b.enable(true);
            let until = Instant::now() + Duration::from_secs(5);
            while (a.peers() == 0 || b.peers() == 0) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(a.peers() >= 1 && b.peers() >= 1, "the two sessions found each other");
            a.propose_tempo(133.0, a.clock_micros());
            let until = Instant::now() + Duration::from_secs(3);
            while (b.capture(b.clock_micros(), 4.0).0 - 133.0).abs() > 0.01 && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            let at = a.clock_micros();
            let ((ta, ba), (tb, bb)) = (a.capture(at, 4.0), b.capture(at, 4.0));
            assert!((tb - 133.0).abs() <= 0.01 && (ta - tb).abs() <= 0.01, "{ta} / {tb}");
            let phase_ms = ((ba - bb).rem_euclid(4.0) * 60_000.0 / 133.0).min((bb - ba).rem_euclid(4.0) * 60_000.0 / 133.0);
            assert!(phase_ms <= 5.0, "phase {phase_ms} ms");
            a.enable(false);
            b.enable(false);
        }
    }
}

/// A fake Link session for tests here and in the modules whose BPM
/// changes must not reach the session (project, show list, macros).
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// One simulated Link timeline that several fake peers share: a tempo
    /// and a beat that is a pure function of the host time, kept
    /// continuous when the tempo changes (as Link does).
    pub struct Timeline {
        base: Instant,
        bpm: f64,
        /// (host µs, beat) the tempo last changed at.
        anchor: (i64, f64),
        now: Instant,
        pub peers: usize,
        pub proposals: Vec<f64>,
    }

    impl Timeline {
        fn micros(&self, at: Instant) -> i64 {
            // An arbitrary offset: the host clock is not the studio's epoch.
            1_000_000_000 + at.saturating_duration_since(self.base).as_micros() as i64
        }
        fn beat(&self, at: i64) -> f64 {
            self.anchor.1 + (at - self.anchor.0) as f64 / 60e6 * self.bpm
        }
    }

    #[derive(Clone)]
    pub struct Fake(pub Arc<Mutex<Timeline>>);

    impl Fake {
        pub fn new(bpm: f64, base: Instant) -> Self {
            // A beat origin that is not a whole bar: alignment is tested.
            Self(Arc::new(Mutex::new(Timeline { base, bpm, anchor: (1_000_000_000, 1.3), now: base, peers: 0, proposals: Vec::new() })))
        }
        pub fn at(&self, now: Instant) {
            self.0.lock().unwrap().now = now;
        }
        pub fn set_tempo(&self, bpm: f64) {
            let mut tl = self.0.lock().unwrap();
            let at = tl.micros(tl.now);
            tl.anchor = (at, tl.beat(at));
            tl.bpm = bpm;
        }
        pub fn beat_at(&self, now: Instant) -> f64 {
            let tl = self.0.lock().unwrap();
            tl.beat(tl.micros(now))
        }
    }

    impl LinkSession for Fake {
        fn enable(&mut self, on: bool) {
            self.0.lock().unwrap().peers += if on { 1 } else { 0 };
        }
        fn peers(&self) -> usize {
            self.0.lock().unwrap().peers.saturating_sub(1)
        }
        fn clock_micros(&self) -> i64 {
            let tl = self.0.lock().unwrap();
            tl.micros(tl.now)
        }
        fn capture(&mut self, at: i64, _quantum: f64) -> (f64, f64) {
            let tl = self.0.lock().unwrap();
            (tl.bpm, tl.beat(at))
        }
        fn propose_tempo(&mut self, bpm: f64, at: i64) {
            let mut tl = self.0.lock().unwrap();
            tl.anchor = (at, tl.beat(at));
            tl.bpm = bpm;
            tl.proposals.push(bpm);
        }
    }

    pub fn on(s: &mut Shared, fake: &Fake) {
        let f = fake.clone();
        update_with(s, LinkRequest { enabled: Some(true), quantum: None }, move |_| Ok(Box::new(f))).unwrap();
    }

    /// Runs `frames` engine frames of 1/60 s from `*now`.
    pub fn run(s: &mut Shared, fake: &Fake, now: &mut Instant, frames: u32) {
        for _ in 0..frames {
            *now += Duration::from_micros(16_667);
            s.sim_now = Some(*now);
            fake.at(*now);
            frame(s, *now);
        }
    }

    /// What the studio proposed to the session so far.
    pub fn proposals(fake: &Fake) -> Vec<f64> {
        fake.0.lock().unwrap().proposals.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{on, run, Fake};
    use super::*;
    use crate::controls::{apply, ControlInput};
    use crate::test_support::shared;
    use std::time::Duration;

    /// Phase error between the clock and the session, in ms, modulo the bar.
    fn phase_error_ms(s: &Shared, fake: &Fake, now: Instant) -> f64 {
        let t = now.saturating_duration_since(s.epoch).as_secs_f64();
        let d = (s.tempo.beat_at(t) - fake.beat_at(now)).rem_euclid(DEFAULT_QUANTUM);
        d.min(DEFAULT_QUANTUM - d) * 60_000.0 / s.tempo.bpm
    }

    #[test]
    fn the_default_build_says_it_is_not_built() {
        let mut s = shared();
        if available() {
            return;
        }
        let err = update(&mut s, LinkRequest { enabled: Some(true), quantum: None }).unwrap_err();
        assert_eq!(err, (501, NOT_BUILT.to_string()));
        assert!(!s.link.enabled());
        assert_eq!(view(&s)["available"], json!(false));
    }

    #[test]
    fn off_by_default_and_nothing_runs() {
        let mut s = shared();
        let epoch = s.epoch;
        frame(&mut s, epoch);
        assert!(!s.link.enabled());
        assert_eq!(s.tempo.source, TempoSource::Manual);
        assert_eq!(summary(&s)["enabled"], json!(false));
    }

    #[test]
    fn follows_the_session_tempo_and_bar() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(10);
        let fake = Fake::new(128.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 3);
        assert_eq!(s.tempo.source, TempoSource::Link);
        assert_eq!(s.link.status, LinkStatus::Following);
        assert!((s.tempo.bpm - 128.0).abs() <= 0.01);
        assert!(phase_error_ms(&s, &fake, now) <= 5.0, "{} ms", phase_error_ms(&s, &fake, now));
        // Another peer changes the tempo: the clock follows, on the beat.
        fake.set_tempo(140.5);
        run(&mut s, &fake, &mut now, 30);
        assert!((s.tempo.bpm - 140.5).abs() <= 0.01);
        assert!(phase_error_ms(&s, &fake, now) <= 5.0);
    }

    #[test]
    fn the_clock_never_jumps_more_than_half_a_bar() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(3);
        let t = now.saturating_duration_since(s.epoch).as_secs_f64();
        let before = s.tempo.beat_at(t);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 1);
        let t = now.saturating_duration_since(s.epoch).as_secs_f64();
        let moved = s.tempo.beat_at(t) - (before + 1.0 / 30.0);
        assert!(moved.abs() <= DEFAULT_QUANTUM / 2.0 + 1e-9, "moved {moved} beats");
    }

    #[test]
    fn a_local_bpm_goes_to_the_session() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(5);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 2);
        apply(&mut s, "tempo.bpm", ControlInput::Value(126.0), false).unwrap();
        run(&mut s, &fake, &mut now, 2);
        assert_eq!(fake.0.lock().unwrap().proposals, vec![126.0]);
        assert_eq!((s.tempo.source, s.link.proposed), (TempoSource::Link, 1));
        assert!((s.tempo.bpm - 126.0).abs() <= 0.01);
        // Nothing changed here: nothing more is proposed.
        run(&mut s, &fake, &mut now, 20);
        assert_eq!(s.link.proposed, 1);
    }

    #[test]
    fn only_the_operators_tempo_controls_reach_the_session() {
        use crate::controls::{apply_as, apply_from};
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(5);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 2);
        // Programmed changes: a macro or the command line of a macro, a cue
        // list action (System), the timeline, and the local setter that
        // project and show-list loads use. All stay local, and the
        // session's tempo wins at the next frame.
        apply_from(&mut s, "tempo.bpm", ControlInput::Value(90.0), ControlSource::Macro).unwrap();
        run(&mut s, &fake, &mut now, 2);
        apply_from(&mut s, "tempo.double", ControlInput::Value(1.0), ControlSource::Timeline).unwrap();
        run(&mut s, &fake, &mut now, 2);
        apply_as(&mut s, "tempo.half", ControlInput::Value(1.0), ArmSource::System).unwrap();
        run(&mut s, &fake, &mut now, 2);
        let t = s.now_s();
        s.tempo.set_bpm_manual(100.0, t);
        run(&mut s, &fake, &mut now, 2);
        assert!(super::fake::proposals(&fake).is_empty(), "{:?}", super::fake::proposals(&fake));
        assert_eq!((s.tempo.source, s.link.proposed), (TempoSource::Link, 0));
        assert!((s.tempo.bpm - 120.0).abs() <= 0.01, "the session won: {}", s.tempo.bpm);
        // The operator's: the field (UI), ×2 from MIDI, ÷2 from OSC.
        apply(&mut s, "tempo.bpm", ControlInput::Value(110.0), false).unwrap();
        run(&mut s, &fake, &mut now, 1);
        apply(&mut s, "tempo.double", ControlInput::Value(1.0), true).unwrap();
        run(&mut s, &fake, &mut now, 1);
        apply_as(&mut s, "tempo.half", ControlInput::Value(1.0), ArmSource::Osc).unwrap();
        run(&mut s, &fake, &mut now, 1);
        assert_eq!(super::fake::proposals(&fake), vec![110.0, 220.0, 110.0]);
        assert!((s.tempo.bpm - 110.0).abs() <= 0.01);
        // The same BPM again changes nothing: nothing proposed.
        apply(&mut s, "tempo.bpm", ControlInput::Value(110.0), false).unwrap();
        run(&mut s, &fake, &mut now, 1);
        assert_eq!(s.link.proposed, 3);
    }

    #[test]
    fn an_operator_bpm_with_link_off_proposes_nothing_later() {
        let mut s = shared();
        apply(&mut s, "tempo.bpm", ControlInput::Value(100.0), false).unwrap();
        let mut now = s.epoch + Duration::from_secs(5);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 2);
        assert!(super::fake::proposals(&fake).is_empty());
        assert!((s.tempo.bpm - 120.0).abs() <= 0.01);
    }

    #[test]
    fn two_studios_on_one_session_agree() {
        // Two instances, one session (the fake stands for the network).
        let (mut a, mut b) = (shared(), shared());
        let base = a.epoch.min(b.epoch);
        let fake = Fake::new(120.0, base);
        on(&mut a, &fake);
        on(&mut b, &fake);
        let mut now = base + Duration::from_secs(7);
        for _ in 0..10 {
            now += Duration::from_micros(16_667);
            fake.at(now);
            frame(&mut a, now);
            frame(&mut b, now);
        }
        assert_eq!(a.link.peers, 1);
        apply(&mut a, "tempo.bpm", ControlInput::Value(97.3), false).unwrap();
        for _ in 0..3 {
            now += Duration::from_micros(16_667);
            fake.at(now);
            frame(&mut a, now);
            frame(&mut b, now);
        }
        assert!((a.tempo.bpm - b.tempo.bpm).abs() <= 0.01 && (b.tempo.bpm - 97.3).abs() <= 0.01);
        assert!(phase_error_ms(&a, &fake, now) <= 5.0 && phase_error_ms(&b, &fake, now) <= 5.0);
    }

    #[test]
    fn out_of_range_tempo_is_not_taken() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(1);
        let fake = Fake::new(300.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 2);
        assert_eq!(s.link.status, LinkStatus::OutOfRange);
        assert_eq!(s.tempo.bpm, 120.0);
        assert!(fake.0.lock().unwrap().proposals.is_empty(), "nothing sent back on its own");
    }

    #[test]
    fn phase_controls_are_refused_while_following() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(2);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 1);
        for id in ["tempo.nudge_up", "tempo.nudge_down", "tempo.resync"] {
            assert!(apply(&mut s, id, ControlInput::Value(1.0), false).is_err(), "{id}");
        }
        assert!(apply(&mut s, "tempo.auto", ControlInput::Value(1.0), false).is_err());
        assert_eq!(s.tempo.source, TempoSource::Link);
    }

    #[test]
    fn off_gives_the_tempo_back_without_a_jump() {
        let mut s = shared();
        let mut now = s.epoch + Duration::from_secs(4);
        let fake = Fake::new(133.0, s.epoch);
        on(&mut s, &fake);
        run(&mut s, &fake, &mut now, 5);
        let t = now.saturating_duration_since(s.epoch).as_secs_f64();
        let beat = s.tempo.beat_at(t);
        update(&mut s, LinkRequest { enabled: Some(false), quantum: None }).unwrap();
        assert!(!s.link.enabled());
        assert_eq!((s.tempo.source, s.tempo.bpm, s.tempo.beat_at(t)), (TempoSource::Manual, 133.0, beat));
        assert_eq!(fake.0.lock().unwrap().peers, 1, "the session was disabled once");
    }

    #[test]
    fn never_arms_and_refuses_bad_input() {
        let mut s = shared();
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        let mut now = s.epoch + Duration::from_secs(1);
        run(&mut s, &fake, &mut now, 10);
        assert!(!s.gate.status(&s.estop).armed);
        for q in [0.0, 2.5, 17.0, f64::NAN] {
            assert_eq!(update(&mut s, LinkRequest { enabled: None, quantum: Some(q) }).unwrap_err().0, 400);
        }
        update(&mut s, LinkRequest { enabled: None, quantum: Some(8.0) }).unwrap();
        assert_eq!(s.link.quantum, 8.0);
    }

    #[test]
    fn midi_clock_and_link_exclude_each_other() {
        let mut s = shared();
        s.midi.store.devices.clock.follow = true;
        let fake = Fake::new(120.0, s.epoch);
        let f = fake.clone();
        let err = update_with(&mut s, LinkRequest { enabled: Some(true), quantum: None }, move |_| Ok(Box::new(f))).unwrap_err();
        assert_eq!(err.0, 409);
        s.midi.store.devices.clock.follow = false;
        on(&mut s, &fake);
        s.midi.enabled = true;
        s.midi.device_mut("Deck").input = true;
        let err = crate::midi::clock::update(&mut s, &json!({ "follow": true, "in_port": "Deck" })).unwrap_err();
        assert_eq!(err, (409, LINK_ON.to_string()));
    }
}
