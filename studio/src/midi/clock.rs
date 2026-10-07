//! MIDI clock (T-207): the studio's tempo (T-150) can follow a clock that
//! a DJ app or a deck sends, and can send its own clock to a MIDI port
//! (a drum machine, a DJ app, a controller). The tempo clock stays the one
//! source of truth: the MIDI clock is only one of its sources, or a copy.
//!
//! *Suivre l'horloge MIDI* (input), from one port the operator picks:
//! - 24 ticks (`F8`) per beat. The BPM is a least-squares fit over the
//!   last 2 beats of ticks (so a ±1 ms jitter barely moves it), rounded to
//!   0.1, clamped to the tempo's range, and only applied when it moved by
//!   0.1 or more. `TempoClock::set_bpm` keeps the beat continuous: a new
//!   tempo never makes the phase jump.
//! - `Start` (`FA`): the next tick is the "1" (the bar is resynced on it,
//!   like *Sync 1*), then the phase is pulled towards the ticks by at most
//!   1/64 beat per beat. `Stop` (`FC`) freezes the reading; `Continue`
//!   (`FB`) resumes it.
//! - While it is followed, Tap, Nudge, the BPM field and *Tempo auto* are
//!   refused (controls.rs) and the tempo bar says why.
//! - No tick for 2 s: the tempo goes back to *Manuel* at its last BPM and
//!   phase (no jump). The next ticks take it again.
//! - It only ever moves the tempo: no control, no cue, no arming. Strobes
//!   on the beat stay under the flash limiters, which run after.
//!
//! *Envoyer l'horloge MIDI* (output): **off by default**, only to the
//! output port the operator picks, from a "midi-clock" thread of its own:
//! the engine only `try_send`s a snapshot of the tempo (bounded queue)
//! when it changes, the thread extrapolates the beat and sends each tick
//! when due. `Start` goes out on the first bar line, again after a resync;
//! `Stop` when it is switched off or the port changes. Refused (409) on
//! the port the input listens to (the studio would follow itself). An
//! APC40 mkII still gets its own pulse clock from the MIDI worker (T-206)
//! unless it is the chosen port (never two clocks on one port).
//!
//! Tests use the MIDI simulator (`testing.rs`) only.

use super::backend::OutputPort;
use super::{MidiEvent, MidiMsg};
use crate::interlock::ArmSource;
use crate::tempo::{TempoSource, MAX_BPM, MIN_BPM};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// MIDI clock ticks per beat (quarter note).
pub const PPQ: f64 = 24.0;
/// Ticks kept for the BPM: 2 beats (48 intervals).
const WINDOW: usize = 49;
/// Ticks needed for a first reading: one beat.
const MIN_TICKS: usize = 25;
/// Longer between two ticks: a new run (the window starts again).
const GAP_S: f64 = 0.5;
/// No tick for this long: the clock is lost, back to manual.
pub const LOST_AFTER_S: f64 = 2.0;
/// A reading only moves the tempo when this far from it.
const DEADBAND_BPM: f64 = 0.1;
/// Readings outside this range are garbage, not a tempo to clamp.
const PLAUSIBLE_BPM: (f64, f64) = (20.0, 500.0);
/// After a Start: share of the phase error corrected per beat, and its cap.
const PHASE_GAIN: f64 = 0.25;
const MAX_PHASE_STEP: f64 = 1.0 / 64.0;
/// The most ticks sent at once after a late wake-up.
const BURST: i64 = 3;
/// Snapshots waiting for the "midi-clock" thread at most.
pub const QUEUE_LEN: usize = 8;
/// A snapshot is sent again this often even when nothing changed (a
/// dropped one is made up for).
const REPUBLISH: Duration = Duration::from_secs(1);
/// Longest sleep of the sending thread.
const POLL: Duration = Duration::from_millis(20);
/// A port that failed is tried again after this.
const RETRY: Duration = Duration::from_secs(2);

const START: u8 = 0xFA;
const STOP: u8 = 0xFC;
const TICK: u8 = 0xF8;

pub const SELF_DRIVE: &str = "Horloge MIDI : entrée et sortie sur le même port, le studio suivrait sa propre horloge.";
pub const FOLLOWING: &str = "tempo suivi sur l'horloge MIDI : Tap, Nudge et BPM sont ignorés";

/// Saved in `midi/devices.json` (key `clock`), per machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockPrefs {
    /// *Suivre l'horloge MIDI* from `in_port`.
    pub follow: bool,
    pub in_port: Option<String>,
    /// *Envoyer l'horloge MIDI* to `out_port`. Off by default.
    pub send: bool,
    pub out_port: Option<String>,
}

impl ClockPrefs {
    fn validate(&self) -> Result<(), String> {
        for p in [&self.in_port, &self.out_port].into_iter().flatten() {
            if p.trim().is_empty() || p.len() > 128 {
                return Err("port MIDI invalide".into());
            }
        }
        if self.follow && self.in_port.is_none() {
            return Err("choisissez le port d'où vient l'horloge".into());
        }
        if self.send && self.out_port.is_none() {
            return Err("choisissez le port où envoyer l'horloge".into());
        }
        Ok(())
    }

    /// The self-drive trap: the clock sent where it is read.
    pub fn conflict(&self) -> Option<&'static str> {
        (self.follow && self.send && self.in_port.is_some() && self.in_port == self.out_port).then_some(SELF_DRIVE)
    }

    /// The port the output drives now, if any.
    pub fn sending_to(&self) -> Option<&str> {
        if !self.send || self.conflict().is_some() {
            return None;
        }
        self.out_port.as_deref()
    }
}

// ---------------------------------------------------------------- input

/// Where the input stands (`/api/state.tempo.midi_clock.state`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InState {
    /// Not following.
    #[default]
    Off,
    /// Following is on, no tick yet.
    Waiting,
    /// The tempo follows the ticks.
    Following,
    /// `Stop` received: the reading is frozen.
    Stopped,
    /// No tick for 2 s: manual tempo until they come back.
    Lost,
}

/// The BPM from tick times: a least-squares line through the last 2 beats.
#[derive(Clone, Debug, Default)]
pub struct Estimator {
    ticks: VecDeque<f64>,
}

impl Estimator {
    pub fn reset(&mut self) {
        self.ticks.clear();
    }

    /// Adds a tick at `t` (s). From one beat of ticks on, the BPM they
    /// give (unrounded); `None` before, or for an absurd clock.
    pub fn tick(&mut self, t: f64) -> Option<f64> {
        if !t.is_finite() {
            return None;
        }
        if self.ticks.back().is_some_and(|&last| t < last || t - last > GAP_S) {
            self.ticks.clear();
        }
        self.ticks.push_back(t);
        while self.ticks.len() > WINDOW {
            self.ticks.pop_front();
        }
        let n = self.ticks.len();
        if n < MIN_TICKS {
            return None;
        }
        let mean_x = (n - 1) as f64 / 2.0;
        let mean_y = self.ticks.iter().sum::<f64>() / n as f64;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for (i, y) in self.ticks.iter().enumerate() {
            let dx = i as f64 - mean_x;
            sxy += dx * (y - mean_y);
            sxx += dx * dx;
        }
        let per_tick = sxy / sxx;
        let bpm = 60.0 / (per_tick * PPQ);
        (per_tick > 0.0 && bpm.is_finite() && (PLAUSIBLE_BPM.0..=PLAUSIBLE_BPM.1).contains(&bpm)).then_some(bpm)
    }
}

/// The input's runtime state (never saved).
#[derive(Clone, Debug, Default)]
pub struct ClockIn {
    pub state: InState,
    est: Estimator,
    last_tick: Option<f64>,
    /// `Start` received: the next tick is the "1".
    start_pending: bool,
    /// Ticks since that "1", and the clock's beat there.
    since_start: Option<(u64, f64)>,
    /// Last reading (rounded), for the readout.
    pub bpm: Option<f64>,
    pub ticks: u64,
    /// Readings refused as absurd.
    pub refused: u64,
}

impl ClockIn {
    fn reset(&mut self, state: InState) {
        *self = ClockIn { state, ..Default::default() };
    }
}

/// `x` wrapped to -½..½.
fn wrap_half(x: f64) -> f64 {
    x - x.round()
}

/// The MIDI worker's batch (worker.rs, under the lock): the clock messages
/// of the followed port move the tempo.
pub fn input(s: &mut Shared, events: &[MidiEvent]) {
    let prefs = &s.midi.store.devices.clock;
    if !prefs.follow {
        return;
    }
    let Some(port) = prefs.in_port.clone() else { return };
    for ev in events.iter().filter(|e| e.port == port) {
        let t = ev.at.saturating_duration_since(s.epoch).as_secs_f64();
        on_msg(s, &ev.msg, t);
    }
}

/// One clock message of the followed port, at `t` (s, studio clock).
pub fn on_msg(s: &mut Shared, msg: &MidiMsg, t: f64) {
    let c = &mut s.midi.clock.input;
    match msg {
        MidiMsg::Start => {
            c.start_pending = true;
            c.since_start = None;
            c.state = InState::Following;
        }
        MidiMsg::Continue => {
            c.state = InState::Following;
        }
        MidiMsg::Stop => {
            c.state = InState::Stopped;
            c.start_pending = false;
            c.since_start = None;
        }
        MidiMsg::Clock => tick(s, t),
        _ => {}
    }
}

fn tick(s: &mut Shared, t: f64) {
    if !t.is_finite() {
        return;
    }
    let c = &mut s.midi.clock.input;
    c.ticks += 1;
    c.last_tick = Some(t);
    if c.state == InState::Stopped {
        // Frozen: the ticks only say the clock is still there.
        c.est.reset();
        return;
    }
    if c.state != InState::Following {
        log::info!("Horloge MIDI reçue : le tempo la suit");
        c.state = InState::Following;
    }
    let reading = c.est.tick(t);
    if reading.is_none() && c.est.ticks.len() >= MIN_TICKS {
        c.refused += 1;
    }
    let start = std::mem::take(&mut c.start_pending);
    if s.tempo.source != TempoSource::MidiClock {
        s.tempo.set_midi_clock(true);
    }
    if let Some(bpm) = reading {
        let rounded = ((bpm * 10.0).round() / 10.0).clamp(MIN_BPM, MAX_BPM);
        s.midi.clock.input.bpm = Some(rounded);
        if (bpm.clamp(MIN_BPM, MAX_BPM) - s.tempo.bpm).abs() >= DEADBAND_BPM {
            s.tempo.set_bpm(rounded, t);
        }
    }
    if start {
        // The "1": the bar starts here, like *Sync 1* (speed groups too).
        s.tempo.resync(t);
        let bar_beat = s.tempo.beat_at(t);
        s.speed.resync(bar_beat, s.tempo.effect_t(t), s.tempo.beats_per_bar);
        s.speed_dirty = true;
        s.midi.clock.input.since_start = Some((0, bar_beat));
        return;
    }
    let c = &mut s.midi.clock.input;
    if let Some((n, beat0)) = c.since_start.as_mut() {
        *n += 1;
        if *n % PPQ as u64 == 0 {
            // On each beat of the clock: pull our phase gently towards it.
            let expected = *beat0 + *n as f64 / PPQ;
            let err = wrap_half(s.tempo.beat_at(t) - expected);
            s.tempo.nudge((-err * PHASE_GAIN).clamp(-MAX_PHASE_STEP, MAX_PHASE_STEP));
        }
    }
}

// --------------------------------------------------------------- output

/// What the "midi-clock" thread is told: where to send, and the tempo
/// clock as a line (`beat` at `at`, `bpm`).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub port: Option<String>,
    pub bpm: f64,
    pub beat: f64,
    pub at: Instant,
    pub beats_per_bar: u8,
}

impl Snapshot {
    fn beat_at(&self, now: Instant) -> f64 {
        let dt = if now >= self.at { now.duration_since(self.at).as_secs_f64() } else { -self.at.duration_since(now).as_secs_f64() };
        self.beat + dt * self.bpm / 60.0
    }

    /// Same port and the same line (to the µs): no need to send it again.
    fn same(&self, other: &Snapshot) -> bool {
        self.port == other.port && self.bpm == other.bpm && self.beats_per_bar == other.beats_per_bar && (other.beat_at(self.at) - self.beat).abs() < 1e-6
    }
}

/// The thread's report (`/api/midi/clock`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct OutStatus {
    /// The port open now.
    pub open: Option<String>,
    pub ticks: u64,
    pub starts: u64,
    pub errors: u64,
    pub error: Option<String>,
}

/// Opens an output port on the thread (the backend in use): the same
/// as the timecode output's (T-417).
pub type Opener = crate::timecode::out::MidiOpener;

/// The engine's side of the output (never saved).
pub struct OutRuntime {
    tx: SyncSender<Snapshot>,
    rx: Option<Receiver<Snapshot>>,
    pub status: Arc<Mutex<OutStatus>>,
    last: Option<(Snapshot, Instant)>,
    /// Snapshots the full queue refused.
    pub dropped: u64,
}

impl Default for OutRuntime {
    fn default() -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        OutRuntime { tx, rx: Some(rx), status: Arc::default(), last: None, dropped: 0 }
    }
}

impl OutRuntime {
    pub fn take_receiver(&mut self) -> Option<Receiver<Snapshot>> {
        self.rx.take()
    }

    pub fn status(&self) -> OutStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Queues `snap` when it changed (or once a second). Never blocks.
    fn publish(&mut self, snap: Snapshot, now: Instant) {
        if self.last.as_ref().is_some_and(|(last, at)| last.same(&snap) && now < *at + REPUBLISH) {
            return;
        }
        match self.tx.try_send(snap.clone()) {
            Ok(()) => self.last = Some((snap, now)),
            Err(_) => self.dropped += 1,
        }
    }
}

/// `Shared.midi.clock`.
#[derive(Default)]
pub struct ClockState {
    pub input: ClockIn,
    pub out: OutRuntime,
}

/// Once per engine frame (main.rs): the lost clock goes back to manual,
/// and the output thread gets the tempo.
pub fn frame(s: &mut Shared, now: Instant) {
    let t = now.saturating_duration_since(s.epoch).as_secs_f64();
    let follow = s.midi.store.devices.clock.follow;
    let c = &mut s.midi.clock.input;
    if !follow {
        if c.state != InState::Off {
            c.reset(InState::Off);
            s.tempo.set_midi_clock(false);
        }
    } else {
        if c.state == InState::Off {
            c.reset(InState::Waiting);
        }
        let lost = c.last_tick.is_some_and(|last| t - last > LOST_AFTER_S);
        if lost && matches!(c.state, InState::Following | InState::Stopped) {
            log::warn!("Horloge MIDI perdue : retour au tempo manuel ({:.1} BPM)", s.tempo.bpm);
            let bpm = c.bpm;
            c.reset(InState::Lost);
            c.bpm = bpm;
            s.tempo.set_midi_clock(false);
        }
    }
    let port = s.midi.store.devices.clock.sending_to().map(str::to_string);
    let snap = Snapshot { port, bpm: s.tempo.bpm, beat: s.tempo.beat_at(t), at: now, beats_per_bar: s.tempo.beats_per_bar.max(1) };
    s.midi.clock.out.publish(snap, now);
}

/// The worker's pulse clock to an APC40 mkII (T-206) leaves this port to
/// the output, so it never gets two clocks.
pub fn output_drives(s: &Shared, port: &str) -> bool {
    s.midi.store.devices.clock.sending_to() == Some(port)
}

/// Ticks due on the tempo clock: 24 per beat, each when due.
#[derive(Debug, Default)]
pub struct Ticker {
    /// Last tick sent (index on the tempo clock: beat × 24).
    pub last: Option<i64>,
    pub next_at: Option<Instant>,
}

impl Ticker {
    /// At `beat` (tempo `bpm`): the tick indices to send now, whether the
    /// clock restarted (moved back, or far ahead: a resync), and how long
    /// until the next tick is due.
    pub fn advance(&mut self, beat: f64, bpm: f64) -> (std::ops::RangeInclusive<i64>, bool, Duration) {
        let tick = (beat * PPQ).floor() as i64;
        let (ticks, restarted) = match self.last {
            Some(last) if (0..=PPQ as i64).contains(&(tick - last)) => ((tick - last).min(BURST), false),
            Some(_) => (1, true),
            None => (1, false),
        };
        self.last = Some(tick);
        let to_next = ((tick + 1) as f64 / PPQ - beat) * 60.0 / bpm.max(1.0);
        (tick - ticks + 1..=tick, restarted, Duration::from_secs_f64(to_next.clamp(0.001, 0.1)))
    }
}

/// The sending thread's own state.
struct Sender {
    opener: Option<Opener>,
    port: Option<(String, Box<dyn OutputPort>)>,
    retry: Option<(String, Instant)>,
    ticker: Ticker,
    /// `Start` sent since the port opened (or since the last resync).
    started: bool,
}

fn set_error(status: &Mutex<OutStatus>, error: String) {
    let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
    st.errors += 1;
    if st.error.as_ref() != Some(&error) {
        log::warn!("Horloge MIDI sortante : {error}");
    }
    st.error = Some(error);
}

impl Sender {
    /// Opens or closes the port the snapshot asks for (`Stop` on the way out).
    fn reconcile(&mut self, snap: Option<&Snapshot>, status: &Mutex<OutStatus>, now: Instant) {
        let wanted = snap.and_then(|s| s.port.clone());
        if self.port.as_ref().map(|(n, _)| n) == wanted.as_ref() {
            return;
        }
        self.close();
        if let Some(name) = wanted {
            let retry = self.retry.as_ref().is_some_and(|(n, at)| *n == name && now < *at);
            match (&self.opener, retry) {
                (_, true) => {}
                (None, false) => {
                    set_error(status, "MIDI désactivé : pas d'horloge".into());
                    self.retry = Some((name, now + RETRY));
                }
                (Some(open), false) => match open(&name) {
                    Ok(p) => {
                        log::info!("Horloge MIDI envoyée sur {name}");
                        self.port = Some((name, p));
                        self.retry = None;
                        status.lock().unwrap_or_else(|e| e.into_inner()).error = None;
                    }
                    Err(e) => {
                        set_error(status, format!("port MIDI {name} indisponible : {e}"));
                        self.retry = Some((name, now + RETRY));
                    }
                },
            }
        }
        status.lock().unwrap_or_else(|e| e.into_inner()).open = self.port.as_ref().map(|(n, _)| n.clone());
    }

    /// `Stop` to the open port, then closes it.
    fn close(&mut self) {
        if let Some((name, mut port)) = self.port.take() {
            let _ = port.send(&[STOP]);
            log::info!("Horloge MIDI : arrêt sur {name}");
        }
        self.ticker = Ticker::default();
        self.started = false;
    }

    /// The ticks due now. Returns when the next one is.
    fn send_due(&mut self, snap: &Snapshot, status: &Mutex<OutStatus>, now: Instant) -> Option<Instant> {
        self.port.as_ref()?;
        if self.ticker.next_at.is_some_and(|at| now < at) {
            return self.ticker.next_at;
        }
        let (range, restarted, wait) = self.ticker.advance(snap.beat_at(now), snap.bpm);
        self.ticker.next_at = Some(now + wait);
        if restarted {
            self.started = false;
        }
        let bar = PPQ as i64 * snap.beats_per_bar as i64;
        let (mut ticks, mut starts) = (0, 0);
        let mut failed = None;
        if let Some((name, port)) = self.port.as_mut() {
            for i in range {
                let mut msg = Vec::with_capacity(2);
                if !self.started && i.rem_euclid(bar) == 0 {
                    // Start, then the bar's first tick: the receiver's "1".
                    msg.push(START);
                    self.started = true;
                    starts += 1;
                }
                msg.push(TICK);
                let sent = msg.iter().try_for_each(|b| port.send(&[*b]));
                if let Err(e) = sent {
                    failed = Some(format!("envoi vers {name} : {e}"));
                    break;
                }
                ticks += 1;
            }
            if let Some(e) = failed {
                let name = name.clone();
                set_error(status, e);
                self.port = None;
                self.ticker = Ticker::default();
                self.started = false;
                self.retry = Some((name, now + RETRY));
                status.lock().unwrap_or_else(|e| e.into_inner()).open = None;
            }
        }
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        st.ticks += ticks;
        st.starts += starts;
        self.ticker.next_at
    }
}

/// Starts the "midi-clock" thread. `None`: it could not be created
/// (logged); the queue then fills and drops, the engine is not affected.
pub fn spawn(rx: Receiver<Snapshot>, status: Arc<Mutex<OutStatus>>, opener: Option<Opener>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("midi-clock".into())
        .spawn(move || run(rx, &status, opener, &running))
        .map_err(|e| log::warn!("Horloge MIDI : impossible de démarrer le thread : {e}"))
        .ok()
}

fn run(rx: Receiver<Snapshot>, status: &Mutex<OutStatus>, opener: Option<Opener>, running: &AtomicBool) {
    let mut sender = Sender { opener, port: None, retry: None, ticker: Ticker::default(), started: false };
    let mut snap: Option<Snapshot> = None;
    let mut next: Option<Instant> = None;
    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let wait = next.map_or(POLL, |at| at.saturating_duration_since(now).clamp(Duration::from_micros(500), POLL));
        match rx.recv_timeout(wait) {
            Ok(s) => snap = Some(rx.try_iter().last().unwrap_or(s)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        sender.reconcile(snap.as_ref(), status, now);
        next = snap.as_ref().and_then(|s| sender.send_due(s, status, now));
    }
    sender.close();
}

/// The backend the sending threads (this one, the timecode output) open
/// their port with: the simulator with `--midi-test`, none with
/// `--no-midi`, CoreMIDI otherwise.
pub fn opener(sim: Option<super::testing::SimMidi>, no_midi: bool) -> Option<Opener> {
    use super::backend::Backend;
    if let Some(sim) = sim {
        return Some(Arc::new(move |name: &str| sim.clone().open_output(name)));
    }
    if no_midi {
        return None;
    }
    Some(Arc::new(|name: &str| super::backend::MidirBackend.open_output(name)))
}

// ------------------------------------------------------------------ API

/// The tempo bar's readout (`/api/state.tempo.midi_clock`).
pub fn summary(s: &Shared) -> Value {
    let p = &s.midi.store.devices.clock;
    let c = &s.midi.clock.input;
    json!({
        "follow": p.follow,
        "in_port": p.in_port,
        "state": c.state,
        "bpm": c.bpm,
        "send": p.send,
        "out_port": p.out_port,
    })
}

/// `GET /api/midi/clock`.
pub fn view(s: &Shared) -> Value {
    let ports = |input: bool| -> Vec<&str> { s.midi.devices.iter().filter(|d| if input { d.input } else { d.output }).map(|d| d.name.as_str()).collect() };
    json!({
        "config": s.midi.store.devices.clock,
        "input": { "state": s.midi.clock.input.state, "bpm": s.midi.clock.input.bpm, "ticks": s.midi.clock.input.ticks, "refused": s.midi.clock.input.refused },
        "output": s.midi.clock.out.status(),
        "midi_enabled": s.midi.enabled,
        "inputs": ports(true),
        "outputs": ports(false),
    })
}

/// `POST /api/midi/clock` `{ "follow": true, "in_port": "…", "send": …,
/// "out_port": … }`: the fields given replace the current ones. Ports must
/// be listed (input / output); the self-drive trap is refused (409).
/// Returns the HTTP status and a message on refusal.
pub fn update(s: &mut Shared, patch: &Value) -> Result<(), (u16, String)> {
    let obj = patch.as_object().ok_or((400, "objet JSON attendu".to_string()))?;
    let before = s.midi.store.devices.clock.clone();
    let mut v = json!(before);
    for (k, val) in obj {
        if v.get(k).is_none() {
            return Err((400, format!("réglage inconnu : {k}")));
        }
        v[k] = val.clone();
    }
    let cfg: ClockPrefs = serde_json::from_value(v).map_err(|e| (400, format!("réglage invalide : {e}")))?;
    cfg.validate().map_err(|e| (400, e))?;
    if (cfg.follow || cfg.send) && !s.midi.enabled {
        return Err((400, "MIDI désactivé : pas d'horloge".into()));
    }
    let listed = |name: &str, input: bool| s.midi.devices.iter().any(|d| d.name == name && if input { d.input } else { d.output });
    if let Some(p) = cfg.in_port.as_ref().filter(|p| before.in_port.as_ref() != Some(*p)) {
        if !listed(p, true) {
            return Err((400, format!("port MIDI introuvable : {p}")));
        }
    }
    if let Some(p) = cfg.out_port.as_ref().filter(|p| before.out_port.as_ref() != Some(*p)) {
        if !listed(p, false) {
            return Err((400, format!("port MIDI introuvable : {p}")));
        }
    }
    if let Some(why) = cfg.conflict() {
        return Err((409, why.into()));
    }
    if cfg == before {
        return Ok(());
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
        s.gate.log().record("midi_clock_settings", Some(ArmSource::Ui.id()), change);
    }
    if cfg.follow != before.follow || cfg.in_port != before.in_port {
        // A new source starts afresh; leaving it gives the tempo back.
        s.midi.clock.input.reset(if cfg.follow { InState::Waiting } else { InState::Off });
        s.tempo.set_midi_clock(false);
    }
    s.midi.store.set_clock(cfg).map_err(|e| (500, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::testing::{FakeApc, SimMidi};
    use crate::test_support::shared;

    /// Deterministic ±1 ms jitter.
    fn jitter(i: u64) -> f64 {
        let x = (i.wrapping_mul(2654435761) % 2001) as f64;
        (x - 1000.0) / 1_000_000.0
    }

    fn follow(s: &mut Shared, port: &str) {
        s.midi.enabled = true;
        s.midi.device_mut(port).input = true;
        update(s, &json!({ "follow": true, "in_port": port })).unwrap();
        let epoch = s.epoch;
        frame(s, epoch);
    }

    /// `n` ticks at `bpm` from `t0`, with jitter; returns the time after.
    fn ticks(s: &mut Shared, bpm: f64, t0: f64, n: u64, jit: bool) -> f64 {
        let dt = 60.0 / bpm / PPQ;
        for i in 0..n {
            let t = t0 + i as f64 * dt + if jit { jitter(i) } else { 0.0 };
            on_msg(s, &MidiMsg::Clock, t);
        }
        t0 + n as f64 * dt
    }

    #[test]
    fn a_jittery_128_bpm_clock_reads_128_after_two_beats() {
        for bpm in [128.0, 60.0, 174.0, 120.0] {
            let mut e = Estimator::default();
            let dt = 60.0 / bpm / PPQ;
            let mut got = None;
            for i in 0..49u64 {
                got = e.tick(10.0 + i as f64 * dt + jitter(i));
            }
            let got = got.unwrap();
            assert!((got - bpm).abs() <= 0.2, "{bpm}: read {got}");
        }
    }

    #[test]
    fn the_tempo_follows_and_tap_and_nudge_are_refused() {
        let mut s = shared();
        s.tempo.set_bpm_manual(100.0, 0.0);
        follow(&mut s, "Platine");
        assert_eq!(s.midi.clock.input.state, InState::Waiting);
        let t = ticks(&mut s, 128.0, 1.0, 49, true);
        assert_eq!(s.tempo.source, TempoSource::MidiClock);
        assert!((s.tempo.bpm - 128.0).abs() <= 0.2, "{}", s.tempo.bpm);
        assert_eq!(s.midi.clock.input.state, InState::Following);
        let beat = s.tempo.beat_at(t);
        for id in ["tempo.tap", "tempo.nudge_up", "tempo.bpm", "tempo.double", "tempo.auto"] {
            assert!(crate::controls::apply(&mut s, id, crate::controls::ControlInput::Value(1.0), false).is_err(), "{id}");
        }
        assert_eq!((s.tempo.source, s.tempo.beat_at(t)), (TempoSource::MidiClock, beat), "nothing moved");
        // Sync 1 is still the operator's.
        assert!(crate::controls::apply(&mut s, "tempo.resync", crate::controls::ControlInput::Value(1.0), false).is_ok());
    }

    #[test]
    fn small_jitter_never_moves_a_steady_tempo() {
        let mut s = shared();
        follow(&mut s, "Platine");
        let t = ticks(&mut s, 128.0, 1.0, 49, false);
        let bpm = s.tempo.bpm;
        let mut changes = 0;
        let mut last = bpm;
        let dt = 60.0 / 128.0 / PPQ;
        for i in 0..24 * 32u64 {
            on_msg(&mut s, &MidiMsg::Clock, t + i as f64 * dt + jitter(i + 7));
            if s.tempo.bpm != last {
                changes += 1;
                last = s.tempo.bpm;
            }
        }
        assert_eq!(changes, 0, "a jittery clock at a steady tempo leaves the BPM alone ({bpm})");
    }

    #[test]
    fn a_tempo_change_never_makes_the_beat_jump() {
        let mut s = shared();
        follow(&mut s, "Platine");
        let mut t = ticks(&mut s, 120.0, 1.0, 49, false);
        let dt = 60.0 / 140.0 / PPQ;
        let mut prev = s.tempo.beat_at(t);
        for _ in 0..24 * 8 {
            on_msg(&mut s, &MidiMsg::Clock, t);
            let b = s.tempo.beat_at(t);
            assert!((b - prev).abs() < 0.05, "beat jumped {prev} → {b}");
            prev = b;
            t += dt;
            prev += dt * s.tempo.bpm / 60.0;
        }
        assert!((s.tempo.bpm - 140.0).abs() <= 0.2, "{}", s.tempo.bpm);
    }

    #[test]
    fn start_puts_the_one_on_the_next_tick_and_the_phase_follows() {
        let mut s = shared();
        follow(&mut s, "Platine");
        let t = ticks(&mut s, 120.0, 1.0, 30, false);
        s.tempo.nudge(0.3);
        on_msg(&mut s, &MidiMsg::Start, t);
        let t1 = t + 0.01;
        on_msg(&mut s, &MidiMsg::Clock, t1);
        let one = s.tempo.beat_at(t1);
        assert!(wrap_half(one).abs() < 1e-9 && (one.round() as i64).rem_euclid(4) == 0, "the 1: beat {one}");
        // A clock slightly faster than ours: our phase is pulled to its beats.
        let dt = 60.0 / 120.5 / PPQ;
        let mut tt = t1;
        for _ in 0..24 * 64 {
            tt += dt;
            on_msg(&mut s, &MidiMsg::Clock, tt);
        }
        let ph = s.tempo.state(tt).phase;
        assert!(ph < 0.02 || ph > 0.98, "on the clock's beat: phase {ph}");
    }

    #[test]
    fn stop_freezes_and_losing_the_clock_goes_back_to_manual_without_a_jump() {
        let mut s = shared();
        follow(&mut s, "Platine");
        let t = ticks(&mut s, 128.0, 1.0, 49, false);
        on_msg(&mut s, &MidiMsg::Stop, t);
        // Ticks at another tempo while stopped are not followed.
        let t = ticks(&mut s, 90.0, t, 60, false);
        assert!((s.tempo.bpm - 128.0).abs() <= 0.2);
        assert_eq!(s.midi.clock.input.state, InState::Stopped);
        let bpm = s.tempo.bpm;
        let epoch = s.epoch;
        frame(&mut s, epoch + Duration::from_secs_f64(t + 1.0));
        assert_eq!(s.tempo.source, TempoSource::MidiClock, "1 s: still following");
        let lost_at = t + LOST_AFTER_S + 0.1;
        let expect = s.tempo.beat_at(lost_at);
        frame(&mut s, epoch + Duration::from_secs_f64(lost_at));
        assert_eq!(s.midi.clock.input.state, InState::Lost);
        assert_eq!(s.tempo.source, TempoSource::Manual);
        assert_eq!((s.tempo.bpm, s.tempo.beat_at(lost_at)), (bpm, expect), "same tempo, same beat");
        // Tap works again; the clock coming back takes the tempo again.
        assert!(crate::controls::apply(&mut s, "tempo.tap", crate::controls::ControlInput::Value(1.0), false).is_ok());
        ticks(&mut s, 128.0, lost_at + 1.0, 30, false);
        assert_eq!(s.tempo.source, TempoSource::MidiClock);
    }

    #[test]
    fn absurd_clocks_are_refused_and_clamped() {
        let mut e = Estimator::default();
        for i in 0..49 {
            assert_eq!(e.tick(1.0 + i as f64 * 1e-5), None, "thousands of BPM: garbage");
        }
        assert_eq!(Estimator::default().tick(f64::NAN), None);
        let mut s = shared();
        follow(&mut s, "Platine");
        ticks(&mut s, 300.0, 1.0, 49, false);
        assert_eq!(s.tempo.bpm, MAX_BPM, "plausible but too fast: clamped");
        for t in [f64::NAN, f64::INFINITY] {
            on_msg(&mut s, &MidiMsg::Clock, t);
        }
        assert_eq!(s.tempo.bpm, MAX_BPM);
        assert!(s.tempo.bpm.is_finite());
    }

    #[test]
    fn only_the_chosen_port_is_followed_and_never_without_follow() {
        let mut s = shared();
        let at = s.epoch;
        let evs: Vec<MidiEvent> = (0..49).map(|i| MidiEvent { port: "Autre".into(), msg: MidiMsg::Clock, at: at + Duration::from_millis(1000 + i * 21) }).collect();
        input(&mut s, &evs);
        assert_eq!((s.tempo.source, s.midi.clock.input.ticks), (TempoSource::Manual, 0), "off by default");
        follow(&mut s, "Platine");
        input(&mut s, &evs);
        assert_eq!((s.tempo.source, s.midi.clock.input.ticks), (TempoSource::Manual, 0), "another port");
        let evs: Vec<MidiEvent> = evs.into_iter().map(|e| MidiEvent { port: "Platine".into(), ..e }).collect();
        input(&mut s, &evs);
        assert_eq!(s.tempo.source, TempoSource::MidiClock);
        assert!(!s.gate.status(&s.estop).armed, "a clock never arms");
        // Switching it off gives the tempo back at once.
        update(&mut s, &json!({ "follow": false })).unwrap();
        assert_eq!(s.tempo.source, TempoSource::Manual);
    }

    #[test]
    fn the_api_validates_and_refuses_the_self_drive_trap() {
        let mut s = shared();
        assert_eq!(s.midi.store.devices.clock, ClockPrefs::default(), "everything off by default");
        use crate::web::{route_class, RouteClass};
        assert_eq!(route_class("POST", "/api/midi/clock"), Some(RouteClass::Safety), "enabling a clock is a Safety route");
        assert_eq!(route_class("GET", "/api/midi/clock"), Some(RouteClass::Read));
        assert!(!ClockPrefs::default().send);
        s.midi.enabled = true;
        for name in ["Platine", "Boîte à rythmes"] {
            let d = s.midi.device_mut(name);
            d.input = true;
            d.output = true;
        }
        assert_eq!(update(&mut s, &json!({ "follow": true })).unwrap_err().0, 400, "no port");
        assert_eq!(update(&mut s, &json!({ "send": true, "out_port": "Nulle part" })).unwrap_err().0, 400);
        assert_eq!(update(&mut s, &json!({ "bogus": 1 })).unwrap_err().0, 400);
        assert_eq!(update(&mut s, &json!({ "in_port": "" })).unwrap_err().0, 400);
        update(&mut s, &json!({ "follow": true, "in_port": "Platine" })).unwrap();
        let e = update(&mut s, &json!({ "send": true, "out_port": "Platine" })).unwrap_err();
        assert_eq!(e, (409, SELF_DRIVE.to_string()));
        update(&mut s, &json!({ "send": true, "out_port": "Boîte à rythmes" })).unwrap();
        assert_eq!(update(&mut s, &json!({ "in_port": "Boîte à rythmes" })).unwrap_err().0, 409);
        assert!(output_drives(&s, "Boîte à rythmes") && !output_drives(&s, "Platine"));
        s.midi.enabled = false;
        assert_eq!(update(&mut s, &json!({ "follow": true, "in_port": "Platine", "send": false })).unwrap_err().0, 400, "MIDI off");
    }

    #[test]
    fn ticker_sends_each_tick_once_and_restarts_after_a_jump() {
        let mut k = Ticker::default();
        let (r, restarted, wait) = k.advance(0.0, 120.0);
        assert_eq!((r, restarted), (0..=0, false));
        assert!((wait.as_secs_f64() - 0.5 / 24.0).abs() < 1e-6);
        assert_eq!(k.advance(0.01, 120.0).0.count(), 0, "same tick");
        assert_eq!(k.advance(2.0 / 24.0 + 0.001, 120.0).0, 1..=2);
        assert_eq!(k.advance(1.0, 120.0).0, 22..=24, "late: a burst of 3 at most");
        assert!(k.advance(0.5, 120.0).1, "moved back: restart");
        assert!(k.advance(9.0, 120.0).1, "far ahead: restart");
    }

    /// T-209 path: a simulated deck's ticks, decoded by the real worker.
    #[test]
    fn the_worker_reads_a_simulated_deck_clock() {
        let sim = SimMidi::default();
        sim.plug("Platine", FakeApc::generic());
        let shared = Arc::new(Mutex::new(crate::test_support::shared()));
        let mut w = crate::midi::worker::Worker::new(sim.clone(), Arc::clone(&shared));
        w.step(Instant::now(), None);
        follow(&mut shared.lock().unwrap(), "Platine");
        // 48 ticks at 240 BPM (half a second), each at its time.
        let (t0, dt) = (Instant::now(), Duration::from_secs_f64(60.0 / 240.0 / PPQ));
        for i in 0..48 {
            std::thread::sleep((t0 + dt * i).saturating_duration_since(Instant::now()));
            sim.inject("Platine", &[TICK]).unwrap();
        }
        w.step(Instant::now(), None);
        let s = shared.lock().unwrap();
        assert_eq!(s.tempo.source, TempoSource::MidiClock);
        // Sleep is not a deck: a loaded machine may be a little off.
        assert!((s.tempo.bpm - 240.0).abs() < 3.0, "{}", s.tempo.bpm);
        assert_eq!(s.midi.clock.input.ticks, 48);
    }

    fn snap(port: &str, bpm: f64, beat: f64, at: Instant) -> Snapshot {
        Snapshot { port: Some(port.into()), bpm, beat, at, beats_per_bar: 4 }
    }

    #[test]
    fn the_sender_starts_on_the_bar_line_and_stops_when_closed() {
        let sim = SimMidi::default();
        sim.plug("Boîte", FakeApc::generic());
        let status = Mutex::new(OutStatus::default());
        let mut tx = Sender { opener: opener(Some(sim.clone()), true), port: None, retry: None, ticker: Ticker::default(), started: false };
        let t0 = Instant::now();
        // Beat 3.5 at t0, 120 BPM: the bar line (beat 4) is 0.25 s later.
        let sn = snap("Boîte", 120.0, 3.5, t0);
        tx.reconcile(Some(&sn), &status, t0);
        let mut now = t0;
        while now < t0 + Duration::from_secs(1) {
            now = tx.send_due(&sn, &status, now).unwrap();
        }
        let (ticks, sent) = sim.with_device("Boîte", |d| (d.clock_ticks, d.received.clone())).unwrap();
        assert!((47..=49).contains(&ticks), "2 beats of ticks in 1 s at 120: {ticks}");
        assert_eq!(sent, vec![vec![START]], "one Start, on the bar line");
        let st = status.lock().unwrap().clone();
        assert_eq!((st.starts, st.open.as_deref()), (1, Some("Boîte")));
        tx.reconcile(None, &status, now);
        assert_eq!(sim.with_device("Boîte", |d| d.received.back().cloned()).unwrap(), Some(vec![STOP]), "Stop when switched off");
        assert!(status.lock().unwrap().open.is_none());
    }

    #[test]
    fn the_sender_without_midi_reports_and_never_panics() {
        let status = Mutex::new(OutStatus::default());
        let mut tx = Sender { opener: None, port: None, retry: None, ticker: Ticker::default(), started: false };
        let now = Instant::now();
        let sn = snap("X", 120.0, 0.0, now);
        tx.reconcile(Some(&sn), &status, now);
        assert_eq!(tx.send_due(&sn, &status, now), None);
        assert!(status.lock().unwrap().error.is_some());
    }

    #[test]
    fn the_engine_publishes_only_changes_and_never_blocks() {
        let mut s = shared();
        let rx = s.midi.clock.out.take_receiver().unwrap();
        let now = s.epoch + Duration::from_secs(1);
        for i in 0..50 {
            frame(&mut s, now + Duration::from_millis(16 * i));
        }
        assert_eq!(rx.try_iter().count(), 1, "nothing changed: one snapshot");
        for i in 0..100 {
            s.tempo.set_bpm(100.0 + i as f64, 1.0);
            frame(&mut s, now);
        }
        assert_eq!(s.midi.clock.out.dropped, 100 - QUEUE_LEN as u64, "a full queue drops, never waits");
    }

    #[test]
    fn the_thread_sends_the_clock_to_the_simulated_port() {
        let sim = SimMidi::default();
        sim.plug("Boîte", FakeApc::generic());
        let (tx, rx) = sync_channel(QUEUE_LEN);
        let status = Arc::new(Mutex::new(OutStatus::default()));
        let running = Arc::new(AtomicBool::new(true));
        let handle = spawn(rx, Arc::clone(&status), opener(Some(sim.clone()), true), Arc::clone(&running)).unwrap();
        tx.send(snap("Boîte", 240.0, 0.0, Instant::now())).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
        let (ticks, last) = sim.with_device("Boîte", |d| (d.clock_ticks, d.received.back().cloned())).unwrap();
        // 300 ms at 240 BPM = 1.2 beats ≈ 29 ticks (a loaded machine sends fewer).
        assert!((10..=32).contains(&ticks), "{ticks} ticks");
        assert_eq!(last, Some(vec![STOP]), "Stop at shutdown");
    }
}
