//! Choosing, connecting and reconnecting the output from the UI (T-401).
//!
//! The manager lists what can be driven (the DACs of the last scan,
//! `devices.rs`, plus the PONK target, the ShowNET SD card box the
//! operator typed (Art-Net, T-301) and the fake test output), opens the
//! operator's choice on its own thread and hands it to the engine's
//! `OutputStage` through a channel: nothing here ever runs on the engine
//! thread or under the `Shared` lock, so a slow scan or open never delays
//! a frame.
//!
//! Safety: every transition disarms. The HTTP handler disarms the gate
//! (« Changement de sortie ») before it asks for a new output; the engine
//! disarms again if needed when the change lands, and disarms with
//! « Sortie perdue » when the output stops answering. A reconnection
//! installs the output disarmed: only the operator re-arms. The `output`
//! interlock (« Aucune sortie connectée ») refuses arming while nothing is
//! connected. The engine closes the old output (disarm, dark frames)
//! before the new one gets a frame.
//!
//! Scanning happens only when the operator asks (*Rechercher*); a
//! remembered output is reconnected at start-up by opening its id, which
//! for an Ether Dream or IDN DAC means `laser-dac` looks it up. Tests and
//! e2e use `FakeBackend` (`--test-devices`, `--test-output`): nothing then
//! can open a real DAC.

use crate::devices::{DeviceInfo, DeviceList, Scope};
use crate::interlock::ArmSource;
use crate::output::{DacOutput, FileLogOutput, Output, TEST_OUTPUT_NAME};
use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the selected output is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Nothing selected (preview only), or a device that is not selected.
    #[default]
    Available,
    Connecting,
    Connected,
    /// Selected but not answering: retried while auto-reconnect is on.
    Lost,
}

/// The connection part of `outputs.json` (key `connection`). Machine
/// settings, never in a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputPrefs {
    /// None: preview only.
    pub selected: Option<String>,
    pub auto_reconnect: bool,
    pub pps: u32,
}

impl Default for OutputPrefs {
    fn default() -> Self {
        Self { selected: None, auto_reconnect: true, pps: 30_000 }
    }
}

/// One entry of `GET /api/outputs`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputInfo {
    pub id: String,
    pub name: String,
    /// "helios", "etherdream", "idn" (from the scan), "ponk", "artnet", "test",
    /// "shownet" (`shownet` feature with the SDK binding installed).
    pub kind: String,
    pub state: LinkState,
    pub error: Option<String>,
}

/// What can be driven, and how to open it.
pub trait OutputBackend: Send + Sync {
    /// The devices a scan finds (runs on the scan thread, may take seconds).
    fn scan(&self, scope: Scope) -> Result<Vec<DeviceInfo>>;
    /// Entries listed without a scan (the PONK target, the test output).
    fn fixed(&self) -> Vec<DeviceInfo>;
    /// Opens an output, disarmed. Runs on the manager's thread only.
    fn open(&self, id: &str, pps: u32) -> Result<Box<dyn Output>>;
}

/// Delays between reconnection attempts: 1 s, 2 s, 4 s, 8 s, then 15 s.
pub const BACKOFF: [Duration; 5] = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8), Duration::from_secs(15)];

/// The wait before attempt `n + 1` after `n` failed ones.
pub fn backoff(failed: u32) -> Duration {
    BACKOFF[(failed as usize).min(BACKOFF.len() - 1)]
}

/// Why the engine gets a new output (or none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The operator connected this output.
    Connected,
    /// The operator disconnected, or is switching: back to preview first.
    Disconnected,
    /// The lost output answers again. Installed disarmed.
    Reconnected,
}

/// A new output for the engine's stage, from the manager's thread.
pub struct StageChange {
    pub output: Option<Box<dyn Output>>,
    pub kind: ChangeKind,
    pub source: ArmSource,
    /// Which hand-over this is: the engine reports a loss with it, so a
    /// late report never marks a newer output lost.
    pub gen: u64,
    pub id: Option<String>,
    /// None: the main output. Some: the output of that projector of the
    /// venue (venue.rs, T-277), with its own generations.
    pub projector: Option<String>,
}

enum Cmd {
    Select(Option<String>, ArmSource),
    Lost(u64, String),
    /// A venue projector's output (T-277): (projector id, output id).
    SelectProjector(String, Option<String>, ArmSource),
    LostProjector(String, u64, String),
}

/// The output of one more projector of the venue (T-277): the same
/// connect / lose / retry cycle as the main output, with its own
/// generations. Auto-reconnect is the main output's setting.
#[derive(Default)]
struct Slot {
    selected: Option<String>,
    name: Option<String>,
    kind: Option<&'static str>,
    state: LinkState,
    error: Option<String>,
    retry_at: Option<Instant>,
    failed: u32,
    gen: u64,
    handed: bool,
    /// The exclusive claim on this output (T-456), while selected.
    lock: Option<DacLock>,
}

/// One venue projector's link, for the UI and the `projectors` interlock.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectorLink {
    pub projector: String,
    pub selected: Option<String>,
    pub name: Option<String>,
    pub kind: Option<&'static str>,
    pub state: LinkState,
    pub error: Option<String>,
}

#[derive(Default)]
struct Status {
    selected: Option<String>,
    name: Option<String>,
    kind: Option<&'static str>,
    state: LinkState,
    error: Option<String>,
    retry_at: Option<Instant>,
    /// Failed attempts since the loss.
    failed: u32,
    auto_reconnect: bool,
    pps: u32,
    /// The latest hand-over to the engine.
    gen: u64,
    /// The engine holds (or is about to hold) an output from us, or still
    /// shows a lost one.
    handed: bool,
    /// The selection `outputs.json` keeps: the operator's last choice,
    /// never the command line's.
    remembered: Option<String>,
    /// `connection.pps` as read (a `--pps` for this run is not saved).
    saved_pps: u32,
    /// The venue's other projectors (T-277), by projector id.
    projectors: std::collections::BTreeMap<String, Slot>,
    /// Why no output may be connected now (T-456: a backup machine in
    /// standby). Every open is refused with it while set.
    hold: Option<String>,
    /// The main output's exclusive claim (T-456), while selected.
    lock: Option<DacLock>,
}

/// The refusal when another studio holds the output (T-456).
pub const IN_USE_FR: &str = "Sortie déjà utilisée par une autre instance du studio";

/// An exclusive claim on one output (T-456): an advisory `flock` on a
/// file named after the output id in the lock folder, taken before the
/// output is opened. A second studio on this Mac (or a second claim in
/// this one) asking for the same output is refused; the system lets go
/// when the file closes, a crash included. Between two Macs the backup
/// role does the same job (standby.rs): a backup in standby opens nothing.
#[derive(Debug)]
pub struct DacLock {
    id: String,
    _file: std::fs::File,
}

impl DacLock {
    /// Claims `id` in `dir`, or fails at once (never waits).
    pub fn acquire(dir: &std::path::Path, id: &str) -> Result<Self> {
        use std::os::fd::AsRawFd;
        std::fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("dossier des verrous de sortie {} : {e}", dir.display()))?;
        // The id in hex: any id is a safe file name; a long one is hashed.
        let hex: String = id.bytes().map(|b| format!("{b:02x}")).collect();
        let name = if hex.len() <= 128 {
            hex
        } else {
            use sha2::Digest;
            sha2::Sha256::digest(id.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
        };
        let path = dir.join(format!("{name}.lock"));
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(|e| anyhow::anyhow!("verrou de sortie {} : {e}", path.display()))?;
        // SAFETY: a valid descriptor owned by `file` for the call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            bail!("{IN_USE_FR} ({id})");
        }
        Ok(DacLock { id: id.to_string(), _file: file })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

pub struct OutputManager {
    backend: Arc<dyn OutputBackend>,
    devices: Arc<DeviceList>,
    status: Mutex<Status>,
    cmd_tx: Mutex<Sender<Cmd>>,
    cmd_rx: Mutex<Option<Receiver<Cmd>>>,
    stage_tx: Mutex<Sender<StageChange>>,
    stage_rx: Mutex<Receiver<StageChange>>,
    /// `FakeBackend` only: unplugs / plugs back the fake outputs.
    test_drop: Option<Arc<AtomicBool>>,
    /// Where the exclusive claims live (T-456); None: no claim (unit tests).
    lock_dir: Option<PathBuf>,
}

impl OutputManager {
    pub fn new(backend: Arc<dyn OutputBackend>, devices: Arc<DeviceList>, prefs: &OutputPrefs, pps: u32) -> Self {
        let (cmd_tx, cmd_rx) = channel();
        let (stage_tx, stage_rx) = channel();
        let status = Status { auto_reconnect: prefs.auto_reconnect, pps, remembered: prefs.selected.clone(), saved_pps: prefs.pps, ..Default::default() };
        Self {
            backend,
            devices,
            status: Mutex::new(status),
            cmd_tx: Mutex::new(cmd_tx),
            cmd_rx: Mutex::new(Some(cmd_rx)),
            stage_tx: Mutex::new(stage_tx),
            stage_rx: Mutex::new(stage_rx),
            test_drop: None,
            lock_dir: None,
        }
    }

    /// Claims every output in `dir` before opening it (T-456): two studios
    /// sharing `dir` never drive the same output at once.
    pub fn with_lock_dir(mut self, dir: PathBuf) -> Self {
        self.lock_dir = Some(dir);
        self
    }

    /// Claims `id`: `have` when it is already that one, else a new claim
    /// (`have` let go first). Always succeeds without a lock folder.
    fn claim(&self, id: &str, have: Option<DacLock>) -> Result<Option<DacLock>> {
        if let Some(l) = have.filter(|l| l.id() == id) {
            return Ok(Some(l));
        }
        match &self.lock_dir {
            Some(dir) => DacLock::acquire(dir, id).map(Some),
            None => Ok(None),
        }
    }

    /// Opens `id` after the hold and the claim: (the claim, the output).
    fn open_claimed(&self, id: &str, pps: u32, have: Option<DacLock>) -> (Option<DacLock>, Result<Box<dyn Output>>) {
        if let Some(reason) = self.status.lock().unwrap().hold.clone() {
            return (None, Err(anyhow::anyhow!(reason)));
        }
        match self.claim(id, have) {
            Ok(lock) => (lock, self.backend.open(id, pps)),
            Err(e) => (None, Err(e)),
        }
    }

    /// T-456: `Some(reason)` refuses every output (a backup machine in
    /// standby) and lets go of the connected ones, without forgetting the
    /// remembered choice; `None` allows them again (nothing reconnects by
    /// itself: the operator connects).
    pub fn set_hold(&self, reason: Option<String>) {
        let (drop_main, drop_projectors) = {
            let mut st = self.status.lock().unwrap();
            let was = st.hold.is_some();
            st.hold = reason.clone();
            if reason.is_none() || was {
                return;
            }
            let main = st.selected.is_some() || st.handed;
            st.selected = None;
            st.state = LinkState::Available;
            st.name = None;
            st.kind = None;
            st.error = None;
            st.retry_at = None;
            st.failed = 0;
            let projectors: Vec<String> = st.projectors.iter().filter(|(_, p)| p.selected.is_some() || p.handed).map(|(id, _)| id.clone()).collect();
            for pid in &projectors {
                if let Some(slot) = st.projectors.get_mut(pid) {
                    slot.selected = None;
                    slot.state = LinkState::Available;
                    slot.retry_at = None;
                }
            }
            (main, projectors)
        };
        log::info!("Sorties retenues : {}", reason.as_deref().unwrap_or_default());
        if drop_main {
            let _ = self.cmd_tx.lock().unwrap().send(Cmd::Select(None, ArmSource::System));
        }
        for pid in drop_projectors {
            let _ = self.cmd_tx.lock().unwrap().send(Cmd::SelectProjector(pid, None, ArmSource::System));
        }
    }

    /// Why outputs are refused now (T-456), if they are.
    pub fn hold(&self) -> Option<String> {
        self.status.lock().unwrap().hold.clone()
    }

    /// Whether this studio drives (or is about to drive) any output: the
    /// main one or a venue projector's (T-456, `holds_output`).
    pub fn holds_any(&self) -> bool {
        let st = self.status.lock().unwrap();
        st.selected.is_some() || st.handed || st.projectors.values().any(|p| p.selected.is_some() || p.handed)
    }

    /// With the switch that simulates unplugging the fake outputs.
    pub fn with_test_drop(mut self, dropped: Arc<AtomicBool>) -> Self {
        self.test_drop = Some(dropped);
        self
    }

    /// The DAC list (`GET /api/devices`, *Rechercher*).
    pub fn devices(&self) -> &Arc<DeviceList> {
        &self.devices
    }

    /// Opens the output named on the command line (`--device`, `--ponk`,
    /// `--test-output`), on the calling thread, before the engine starts.
    /// It is the selection for this run only (never saved).
    pub fn open_at_start(&self, id: &str) -> Result<Box<dyn Output>> {
        let pps = self.status.lock().unwrap().pps;
        let (lock, out) = self.open_claimed(id, pps, None);
        let out = out?;
        let mut st = self.status.lock().unwrap();
        st.lock = lock;
        st.selected = Some(id.to_string());
        st.name = Some(out.name().to_string());
        st.kind = Some(out.kind());
        st.state = LinkState::Connected;
        st.handed = true;
        Ok(out)
    }

    /// Every id the operator may pick: the fixed entries, the last scan,
    /// and the current selection.
    pub fn list(&self) -> Vec<OutputInfo> {
        let st = self.status.lock().unwrap();
        let mut out: Vec<OutputInfo> = Vec::new();
        for d in self.backend.fixed().into_iter().chain(self.devices.devices()) {
            if out.iter().any(|o| o.id == d.id) {
                continue;
            }
            let selected = st.selected.as_deref() == Some(d.id.as_str());
            // Driven by another projector of the venue (T-277).
            let slot = st.projectors.values().find(|p| p.selected.as_deref() == Some(d.id.as_str()));
            out.push(OutputInfo {
                state: if selected { st.state } else { slot.map_or(LinkState::Available, |p| p.state) },
                error: if selected { st.error.clone() } else { slot.and_then(|p| p.error.clone()) },
                id: d.id,
                name: d.name,
                kind: d.kind,
            });
        }
        if let Some(id) = &st.selected {
            if !out.iter().any(|o| &o.id == id) {
                out.push(OutputInfo {
                    id: id.clone(),
                    name: st.name.clone().unwrap_or_else(|| id.clone()),
                    kind: st.kind.unwrap_or("dac").to_string(),
                    state: st.state,
                    error: st.error.clone(),
                });
            }
        }
        out
    }

    /// Whether `id` can be selected.
    pub fn knows(&self, id: &str) -> bool {
        self.list().iter().any(|o| o.id == id)
    }

    /// `POST /api/outputs/select`: connect `id`, or go back to preview
    /// with `None`. The caller has disarmed the gate already. Returns at
    /// once: the manager's thread closes the old output and opens the new.
    pub fn select(&self, id: Option<String>, src: ArmSource) {
        {
            let mut st = self.status.lock().unwrap();
            st.state = if id.is_some() { LinkState::Connecting } else { LinkState::Available };
            st.selected = id.clone();
            st.remembered = id.clone();
            st.name = None;
            st.kind = None;
            st.error = None;
            st.retry_at = None;
            st.failed = 0;
        }
        log::info!("Sortie choisie ({}) : {}", src.label_fr(), id.as_deref().unwrap_or("aperçu seul"));
        let _ = self.cmd_tx.lock().unwrap().send(Cmd::Select(id, src));
    }

    /// Turns automatic reconnection on or off. Off stops the retries; on
    /// while lost schedules the next one.
    pub fn set_auto_reconnect(&self, on: bool, now: Instant) {
        let mut st = self.status.lock().unwrap();
        st.auto_reconnect = on;
        st.retry_at = (on && st.state == LinkState::Lost).then(|| now + backoff(st.failed));
    }

    /// What `outputs.json` keeps (`connection`).
    pub fn prefs(&self) -> OutputPrefs {
        let st = self.status.lock().unwrap();
        OutputPrefs { selected: st.remembered.clone(), auto_reconnect: st.auto_reconnect, pps: st.saved_pps }
    }

    /// The selected output's id (None: preview only).
    pub fn selected(&self) -> Option<String> {
        self.status.lock().unwrap().selected.clone()
    }

    pub fn state(&self) -> LinkState {
        self.status.lock().unwrap().state
    }

    /// The engine's question when a change lands: is hand-over `gen` still
    /// the connected output (else the interlock stays open)?
    pub fn is_current(&self, gen: u64) -> bool {
        let st = self.status.lock().unwrap();
        st.gen == gen && st.state == LinkState::Connected
    }

    /// The engine thread saw hand-over `gen` fail (it has disarmed and
    /// closed it already).
    pub fn report_lost(&self, gen: u64, error: String) {
        let _ = self.cmd_tx.lock().unwrap().send(Cmd::Lost(gen, error));
    }

    /// `POST /api/venue` (T-277): projector `projector` now drives `id`
    /// (None: no output, or the projector was removed). The caller checked
    /// the laser is disarmed. Returns at once, like `select`.
    pub fn select_projector(&self, projector: &str, id: Option<String>, src: ArmSource) {
        {
            let mut st = self.status.lock().unwrap();
            let slot = st.projectors.entry(projector.to_string()).or_default();
            if slot.selected == id && slot.state != LinkState::Lost {
                if id.is_none() && !slot.handed {
                    st.projectors.remove(projector);
                }
                return;
            }
            slot.state = if id.is_some() { LinkState::Connecting } else { LinkState::Available };
            slot.selected = id.clone();
            slot.name = None;
            slot.kind = None;
            slot.error = None;
            slot.retry_at = None;
            slot.failed = 0;
        }
        log::info!("Projecteur {projector} ({}) : sortie {}", src.label_fr(), id.as_deref().unwrap_or("aucune"));
        let _ = self.cmd_tx.lock().unwrap().send(Cmd::SelectProjector(projector.to_string(), id, src));
    }

    /// The venue projectors' links (T-277), in projector id order.
    pub fn projector_links(&self) -> Vec<ProjectorLink> {
        let st = self.status.lock().unwrap();
        st.projectors
            .iter()
            .filter(|(_, p)| p.selected.is_some() || p.handed)
            .map(|(id, p)| ProjectorLink { projector: id.clone(), selected: p.selected.clone(), name: p.name.clone(), kind: p.kind, state: p.state, error: p.error.clone() })
            .collect()
    }

    /// Which output slot drives output `id`: `Some(None)` the main one,
    /// `Some(Some(projector))` a venue projector, None nobody.
    pub fn driver_of(&self, id: &str) -> Option<Option<String>> {
        let st = self.status.lock().unwrap();
        if st.selected.as_deref() == Some(id) {
            return Some(None);
        }
        st.projectors.iter().find(|(_, p)| p.selected.as_deref() == Some(id)).map(|(pid, _)| Some(pid.clone()))
    }

    /// The engine's question when a projector's change lands (T-277).
    pub fn is_current_projector(&self, projector: &str, gen: u64) -> bool {
        let st = self.status.lock().unwrap();
        st.projectors.get(projector).is_some_and(|p| p.gen == gen && p.state == LinkState::Connected)
    }

    /// The engine thread saw projector `projector`'s hand-over `gen` fail.
    pub fn report_lost_projector(&self, projector: &str, gen: u64, error: String) {
        let _ = self.cmd_tx.lock().unwrap().send(Cmd::LostProjector(projector.to_string(), gen, error));
    }

    /// Unit tests (safety invariants): hands `output` to the engine as
    /// projector `projector`'s, as if the manager had opened it.
    #[cfg(test)]
    pub fn inject_projector(&self, projector: &str, id: &str, output: Box<dyn Output>) {
        {
            let mut st = self.status.lock().unwrap();
            let slot = st.projectors.entry(projector.to_string()).or_default();
            slot.selected = Some(id.to_string());
            slot.name = Some(output.name().to_string());
            slot.kind = Some(output.kind());
            slot.state = LinkState::Connected;
            slot.handed = true;
        }
        self.hand_over_projector(projector, Some(output), ChangeKind::Connected, ArmSource::Ui, Some(id.to_string()));
    }

    /// The engine thread, once per frame: the next output to install.
    pub fn engine_take(&self) -> Option<StageChange> {
        self.stage_rx.lock().unwrap().try_recv().ok()
    }

    /// `--test-hooks`: unplugs (true) or plugs back the fake outputs.
    /// False when the backend has no such switch (a real one).
    pub fn simulate_drop(&self, dropped: bool) -> bool {
        match &self.test_drop {
            Some(flag) => {
                flag.store(dropped, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// `GET /api/outputs`.
    pub fn view(&self, now: Instant) -> serde_json::Value {
        let list = self.list();
        let scan = self.devices.view();
        let st = self.status.lock().unwrap();
        let retry_in_ms = st.retry_at.filter(|_| st.state == LinkState::Lost).map(|t| t.saturating_duration_since(now).as_millis() as u64);
        json!({
            "devices": list,
            "selected": st.selected,
            "name": st.name,
            "kind": st.kind,
            "state": st.state,
            "error": st.error,
            "retry_in_ms": retry_in_ms,
            "auto_reconnect": st.auto_reconnect,
            "pps": st.pps,
            "scanning": scan["scanning"],
            "scanned": scan["scanned"],
            "scan_error": scan["error"],
            "usb": scan["usb"],
            "projectors": st.projectors.iter().filter(|(_, p)| p.selected.is_some() || p.handed).map(|(id, p)| json!({
                "projector": id, "selected": p.selected, "name": p.name, "kind": p.kind, "state": p.state, "error": p.error,
            })).collect::<Vec<_>>(),
        })
    }

    /// Starts the manager's thread: it handles the selections and the
    /// losses in order and retries a lost output on time.
    pub fn spawn(self: &Arc<Self>, running: Arc<std::sync::atomic::AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
        let rx = self.cmd_rx.lock().unwrap().take()?;
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("output-manager".into())
            .spawn(move || {
                while running.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(cmd) => me.step(Some(cmd), Instant::now()),
                        Err(RecvTimeoutError::Timeout) => me.step(None, Instant::now()),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| log::error!("output manager thread: {e}"))
            .ok()
    }

    /// One step of the state machine: a command, or a tick (`None`) that
    /// retries a lost output when its time has come. Never holds the
    /// status lock while opening.
    fn step(&self, cmd: Option<Cmd>, now: Instant) {
        match cmd {
            Some(Cmd::Select(id, src)) => self.on_select(id, src, now),
            Some(Cmd::Lost(gen, error)) => self.on_lost(gen, error, now),
            Some(Cmd::SelectProjector(pid, id, src)) => self.on_select_projector(&pid, id, src, now),
            Some(Cmd::LostProjector(pid, gen, error)) => self.on_lost_projector(&pid, gen, error, now),
            None => {
                self.on_tick(now);
                self.on_tick_projectors(now);
            }
        }
    }

    fn on_select_projector(&self, pid: &str, id: Option<String>, src: ArmSource, now: Instant) {
        let handed = {
            let mut st = self.status.lock().unwrap();
            if let Some(p) = st.projectors.get_mut(pid).filter(|p| p.lock.as_ref().is_some_and(|l| Some(l.id()) != id.as_deref())) {
                p.lock = None;
            }
            st.projectors.get_mut(pid).is_some_and(|p| std::mem::take(&mut p.handed))
        };
        if handed {
            self.hand_over_projector(pid, None, ChangeKind::Disconnected, src, None);
        }
        let Some(id) = id else {
            // Nothing to drive: forget the slot once the engine is told.
            let mut st = self.status.lock().unwrap();
            if st.projectors.get(pid).is_some_and(|p| p.selected.is_none()) {
                st.projectors.remove(pid);
            }
            return;
        };
        if self.status.lock().unwrap().projectors.get(pid).and_then(|p| p.selected.as_deref()) != Some(id.as_str()) {
            return; // a newer choice is queued
        }
        self.attempt_projector(pid, &id, ChangeKind::Connected, src, now);
    }

    fn on_lost_projector(&self, pid: &str, gen: u64, error: String, now: Instant) {
        let mut st = self.status.lock().unwrap();
        let auto = st.auto_reconnect;
        let Some(slot) = st.projectors.get_mut(pid) else { return };
        if slot.gen != gen || slot.state != LinkState::Connected {
            return;
        }
        log::warn!("Projecteur {pid} : sortie perdue : {error}");
        slot.state = LinkState::Lost;
        slot.error = Some(error);
        slot.failed = 0;
        slot.retry_at = auto.then(|| now + backoff(0));
    }

    fn on_tick_projectors(&self, now: Instant) {
        let due: Vec<(String, String)> = {
            let st = self.status.lock().unwrap();
            if !st.auto_reconnect {
                return;
            }
            st.projectors
                .iter()
                .filter(|(_, p)| p.state == LinkState::Lost && p.retry_at.is_some_and(|at| at <= now))
                .filter_map(|(pid, p)| Some((pid.clone(), p.selected.clone()?)))
                .collect()
        };
        for (pid, id) in due {
            self.attempt_projector(&pid, &id, ChangeKind::Reconnected, ArmSource::System, now);
        }
    }

    /// `attempt` for a venue projector's output.
    fn attempt_projector(&self, pid: &str, id: &str, kind: ChangeKind, src: ArmSource, now: Instant) {
        let (pps, have) = {
            let mut st = self.status.lock().unwrap();
            let have = st.projectors.get_mut(pid).and_then(|p| p.lock.take());
            (st.pps, have)
        };
        let (lock, opened) = self.open_claimed(id, pps, have);
        let mut st = self.status.lock().unwrap();
        let auto = st.auto_reconnect;
        let Some(slot) = st.projectors.get_mut(pid).filter(|p| p.selected.as_deref() == Some(id)) else {
            drop(st);
            drop(lock);
            if let Ok(mut out) = opened {
                let _ = out.blank_now();
            }
            return;
        };
        match opened {
            Ok(out) => {
                log::info!("Projecteur {pid} : sortie {} {}", out.name(), if kind == ChangeKind::Reconnected { "retrouvée (laser désarmé)" } else { "connectée" });
                slot.name = Some(out.name().to_string());
                slot.kind = Some(out.kind());
                slot.state = LinkState::Connected;
                slot.error = None;
                slot.retry_at = None;
                slot.failed = 0;
                slot.handed = true;
                slot.lock = lock;
                drop(st);
                self.hand_over_projector(pid, Some(out), kind, src, Some(id.to_string()));
            }
            Err(e) => {
                let error = format!("{e:#}");
                if kind == ChangeKind::Reconnected {
                    slot.failed += 1;
                } else {
                    log::warn!("Projecteur {pid} : connexion de la sortie {id} impossible : {error}");
                    slot.failed = 0;
                }
                slot.state = LinkState::Lost;
                slot.error = Some(error);
                slot.retry_at = auto.then(|| now + backoff(slot.failed));
                slot.lock = lock;
            }
        }
    }

    /// `hand_over` for a venue projector, under that projector's generation.
    fn hand_over_projector(&self, pid: &str, output: Option<Box<dyn Output>>, kind: ChangeKind, source: ArmSource, id: Option<String>) {
        let gen = {
            let mut st = self.status.lock().unwrap();
            let slot = st.projectors.entry(pid.to_string()).or_default();
            slot.gen += 1;
            slot.gen
        };
        let _ = self.stage_tx.lock().unwrap().send(StageChange { output, kind, source, gen, id, projector: Some(pid.to_string()) });
    }

    fn on_select(&self, id: Option<String>, src: ArmSource, now: Instant) {
        // The old output closes first (a USB DAC can only be opened once).
        let handed = std::mem::take(&mut self.status.lock().unwrap().handed);
        if handed {
            self.hand_over(None, ChangeKind::Disconnected, src, None);
        }
        {
            // The old output is let go: so is its claim (T-456), unless it
            // is chosen again.
            let mut st = self.status.lock().unwrap();
            if st.lock.as_ref().is_some_and(|l| Some(l.id()) != id.as_deref()) {
                st.lock = None;
            }
        }
        let Some(id) = id else { return };
        if self.status.lock().unwrap().selected.as_deref() != Some(id.as_str()) {
            return; // a newer choice is queued
        }
        self.attempt(&id, ChangeKind::Connected, src, now);
    }

    fn on_lost(&self, gen: u64, error: String, now: Instant) {
        let mut st = self.status.lock().unwrap();
        if st.gen != gen || st.state != LinkState::Connected {
            return; // about an output that was already replaced
        }
        log::warn!("Sortie perdue : {error}");
        // `handed` stays: the engine still shows the lost output (its name
        // and error) until a new choice clears it.
        st.state = LinkState::Lost;
        st.error = Some(error);
        st.failed = 0;
        st.retry_at = st.auto_reconnect.then(|| now + backoff(0));
    }

    fn on_tick(&self, now: Instant) {
        let id = {
            let st = self.status.lock().unwrap();
            match (&st.selected, st.state, st.retry_at) {
                (Some(id), LinkState::Lost, Some(at)) if st.auto_reconnect && at <= now => id.clone(),
                _ => return,
            }
        };
        self.attempt(&id, ChangeKind::Reconnected, ArmSource::System, now);
    }

    /// Opens `id` and hands it to the engine, or records the failure and
    /// when to try again.
    fn attempt(&self, id: &str, kind: ChangeKind, src: ArmSource, now: Instant) {
        let (pps, have) = {
            let mut st = self.status.lock().unwrap();
            (st.pps, st.lock.take())
        };
        let (lock, opened) = self.open_claimed(id, pps, have);
        let mut st = self.status.lock().unwrap();
        if st.selected.as_deref() != Some(id) {
            // The operator chose something else meanwhile: close it unused.
            drop(st);
            drop(lock);
            if let Ok(mut out) = opened {
                let _ = out.blank_now();
            }
            return;
        }
        match opened {
            Ok(out) => {
                if kind == ChangeKind::Reconnected {
                    log::info!("Sortie retrouvée : {} (laser désarmé)", out.name());
                } else {
                    log::info!("Sortie connectée : {}", out.name());
                }
                st.name = Some(out.name().to_string());
                st.kind = Some(out.kind());
                st.state = LinkState::Connected;
                st.error = None;
                st.retry_at = None;
                st.failed = 0;
                st.handed = true;
                st.lock = lock;
                drop(st);
                self.hand_over(Some(out), kind, src, Some(id.to_string()));
            }
            Err(e) => {
                let error = format!("{e:#}");
                if kind == ChangeKind::Reconnected {
                    st.failed += 1;
                } else {
                    log::warn!("Connexion de la sortie {id} impossible : {error}");
                    st.failed = 0;
                }
                st.state = LinkState::Lost;
                st.error = Some(error);
                st.retry_at = st.auto_reconnect.then(|| now + backoff(st.failed));
                // Still chosen: the claim is kept for the next attempt.
                st.lock = lock;
            }
        }
    }

    /// Sends a change to the engine, under a new generation number.
    fn hand_over(&self, output: Option<Box<dyn Output>>, kind: ChangeKind, source: ArmSource, id: Option<String>) {
        let gen = {
            let mut st = self.status.lock().unwrap();
            st.gen += 1;
            st.gen
        };
        let _ = self.stage_tx.lock().unwrap().send(StageChange { output, kind, source, gen, id, projector: None });
    }

    /// The generation of the output opened by `open_at_start` (0).
    pub fn current_gen(&self) -> u64 {
        self.status.lock().unwrap().gen
    }
}

fn ponk_entry(target: SocketAddr) -> DeviceInfo {
    DeviceInfo { kind: "ponk".into(), id: format!("ponk:{target}"), name: format!("PONK → MadMapper ({target})") }
}

/// The real outputs: DACs through `laser-dac`, and PONK to MadMapper.
/// With the `shownet` feature, also the ShowNET boxes of the installed SDK
/// binding (`shownet.rs`); none without it.
pub struct RealBackend {
    /// `--ponk`'s target, else MadMapper on this Mac.
    pub ponk: SocketAddr,
    pub data_dir: PathBuf,
    /// The ShowNET SD card box the operator typed (T-301), listed once set.
    pub artnet: Arc<crate::artnet::ShowNetDmxStore>,
}

/// Opens the Art-Net output of `id` with the stored profile settings.
fn open_artnet(store: &crate::artnet::ShowNetDmxStore, id: &str) -> Result<Box<dyn Output>> {
    let target = crate::artnet::id_target(id).ok_or_else(|| anyhow::anyhow!("adresse Art-Net invalide : « {id} »"))?;
    Ok(Box::new(crate::artnet::ShowNetDmxOutput::open(target, &store.get())?))
}

impl OutputBackend for RealBackend {
    fn scan(&self, scope: Scope) -> Result<Vec<DeviceInfo>> {
        #[allow(unused_mut)]
        let mut found = crate::devices::scan_dacs(scope)?;
        #[cfg(feature = "shownet")]
        if let Some(backend) = crate::shownet::installed_backend() {
            match crate::shownet::scan(&*backend) {
                Ok(boxes) => found.extend(boxes),
                Err(e) => log::warn!("Recherche des ShowNET impossible : {e:#}"),
            }
        }
        Ok(found)
    }
    fn fixed(&self) -> Vec<DeviceInfo> {
        #[allow(unused_mut)]
        let mut out = vec![ponk_entry(self.ponk)];
        out.extend(self.artnet.entry());
        #[cfg(feature = "shownet")]
        if crate::shownet::installed_backend().is_some() {
            out.push(crate::shownet::fixed_entry());
        }
        out
    }
    fn open(&self, id: &str, pps: u32) -> Result<Box<dyn Output>> {
        #[cfg(feature = "shownet")]
        if id.starts_with(crate::shownet::ID_PREFIX) {
            return crate::shownet::open_id(crate::shownet::installed_backend().as_deref(), id, pps);
        }
        if let Some(target) = id.strip_prefix("ponk:") {
            let target = crate::ponk::parse_target(target).map_err(anyhow::Error::msg)?;
            return Ok(Box::new(crate::ponk::PonkOutput::open(target, &self.data_dir)?));
        }
        if id.starts_with(crate::artnet::ID_PREFIX) {
            return open_artnet(&self.artnet, id);
        }
        Ok(Box::new(DacOutput::open(id, pps)?))
    }
}

/// Tests and e2e (`--test-devices`, `--test-output`): never a real DAC.
/// The listed devices open as fake outputs that only count frames; `test`
/// is the `--test-output` log file; PONK only towards `--ponk`'s address
/// (a loopback receiver in the e2e). `dropped` simulates a lost link.
pub struct FakeBackend {
    pub devices: Vec<DeviceInfo>,
    pub log_file: Option<PathBuf>,
    pub ponk: Option<SocketAddr>,
    pub data_dir: PathBuf,
    pub dropped: Arc<AtomicBool>,
    /// Successful and failed opens, for tests.
    pub opens: Arc<AtomicUsize>,
    /// The Art-Net box (T-301): opened only towards a loopback address.
    pub artnet: Arc<crate::artnet::ShowNetDmxStore>,
    /// A fake ShowNET box (unit tests only): opens `shownet:` ids.
    #[cfg(feature = "shownet")]
    pub shownet: Option<Arc<dyn crate::shownet::ShowNetBackend>>,
}

/// The id of the `--test-output` entry.
pub const TEST_ID: &str = "test";

impl FakeBackend {
    pub fn new(devices: Vec<DeviceInfo>, log_file: Option<PathBuf>, ponk: Option<SocketAddr>, data_dir: PathBuf) -> Self {
        Self {
            devices,
            log_file,
            ponk,
            data_dir,
            dropped: Arc::default(),
            opens: Arc::default(),
            artnet: Arc::new(crate::artnet::ShowNetDmxStore::in_memory()),
            #[cfg(feature = "shownet")]
            shownet: None,
        }
    }
}

impl OutputBackend for FakeBackend {
    fn scan(&self, _: Scope) -> Result<Vec<DeviceInfo>> {
        Ok(self.devices.clone())
    }
    fn fixed(&self) -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        if self.log_file.is_some() {
            out.push(DeviceInfo { kind: "test".into(), id: TEST_ID.into(), name: TEST_OUTPUT_NAME.into() });
        }
        out.extend(self.ponk.map(ponk_entry));
        out.extend(self.artnet.entry());
        out
    }
    #[cfg_attr(not(feature = "shownet"), allow(unused_variables))]
    fn open(&self, id: &str, pps: u32) -> Result<Box<dyn Output>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if self.dropped.load(Ordering::SeqCst) {
            bail!("{id} injoignable (simulation)");
        }
        #[cfg(feature = "shownet")]
        if let Some(shownet) = self.shownet.as_ref().filter(|_| id.starts_with(crate::shownet::ID_PREFIX)) {
            let inner = crate::shownet::open_id(Some(&**shownet), id, pps)?;
            return Ok(Box::new(Unpluggable { inner, dropped: Arc::clone(&self.dropped) }));
        }
        let inner: Box<dyn Output> = if let (TEST_ID, Some(path)) = (id, &self.log_file) {
            Box::new(FileLogOutput::append(path)?)
        } else if id.starts_with(crate::artnet::ID_PREFIX) {
            // Tests and e2e: a local receiver only, never a box.
            match crate::artnet::id_target(id) {
                Some(t) if t.ip().is_loopback() => open_artnet(&self.artnet, id)?,
                _ => bail!("Art-Net vers « {id} » refusé en test (127.0.0.1 seulement)"),
            }
        } else if let Some(target) = id.strip_prefix("ponk:").filter(|t| self.ponk.is_some_and(|p| p.to_string() == *t)) {
            Box::new(crate::ponk::PonkOutput::open(target.parse()?, &self.data_dir)?)
        } else if let Some(d) = self.devices.iter().find(|d| d.id == id) {
            Box::new(MemoryOutput { name: d.name.clone() })
        } else {
            bail!("Sortie inconnue : « {id} »");
        };
        Ok(Box::new(Unpluggable { inner, dropped: Arc::clone(&self.dropped) }))
    }
}

/// A fake output standing in for a listed test device: no laser at all.
struct MemoryOutput {
    name: String,
}

impl Output for MemoryOutput {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> &'static str {
        "test"
    }
    fn set_armed(&mut self, _: bool) -> Result<()> {
        Ok(())
    }
    fn send(&mut self, _: &[Point]) {}
}

/// Wraps a fake output so the test hook can "unplug" it: it then reports
/// a fault, as a DAC whose session ended would.
struct Unpluggable {
    inner: Box<dyn Output>,
    dropped: Arc<AtomicBool>,
}

impl Output for Unpluggable {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn set_armed(&mut self, armed: bool) -> Result<()> {
        self.inner.set_armed(armed)
    }
    fn send(&mut self, points: &[Point]) {
        self.inner.send(points)
    }
    fn blank_now(&mut self) -> Result<()> {
        self.inner.blank_now()
    }
    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        self.inner.kill_switch()
    }
    fn max_points(&self) -> Option<usize> {
        self.inner.max_points()
    }
    fn cue_signal(&mut self, signal: &crate::output::CueSignal) {
        self.inner.cue_signal(signal)
    }
    fn status(&self) -> Option<serde_json::Value> {
        self.inner.status()
    }
    fn fault(&self) -> Option<String> {
        if self.dropped.load(Ordering::SeqCst) {
            return Some(format!("{} débranchée (simulation)", self.inner.name()));
        }
        self.inner.fault()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str) -> DeviceInfo {
        DeviceInfo { kind: "etherdream".into(), id: id.into(), name: format!("Ether Dream {id}") }
    }

    /// A manager on a fake backend with two devices, already scanned.
    fn manager(auto: bool) -> (OutputManager, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let backend = FakeBackend::new(vec![dev("a"), dev("b")], None, None, crate::test_support::scratch_dir());
        let (dropped, opens) = (Arc::clone(&backend.dropped), Arc::clone(&backend.opens));
        let list = Arc::new(DeviceList::fixed(backend.devices.clone()));
        list.refresh();
        let deadline = Instant::now() + Duration::from_secs(5);
        while list.view()["scanned"] != true {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let prefs = OutputPrefs { auto_reconnect: auto, ..Default::default() };
        let mgr = OutputManager::new(Arc::new(backend), list, &prefs, 30_000).with_test_drop(Arc::clone(&dropped));
        (mgr, dropped, opens)
    }

    /// Runs what the thread would: every queued command, in order.
    fn drain(mgr: &OutputManager, now: Instant) {
        let rx = mgr.cmd_rx.lock().unwrap();
        let cmds: Vec<Cmd> = rx.as_ref().unwrap().try_iter().collect();
        drop(rx);
        for c in cmds {
            mgr.step(Some(c), now);
        }
    }

    fn changes(mgr: &OutputManager) -> Vec<(ChangeKind, Option<String>, bool)> {
        std::iter::from_fn(|| mgr.engine_take()).map(|c| (c.kind, c.id, c.output.is_some())).collect()
    }

    #[test]
    fn prefs_default_to_preview_with_auto_reconnect() {
        let p: OutputPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, OutputPrefs { selected: None, auto_reconnect: true, pps: 30_000 });
        let p: OutputPrefs = serde_json::from_str(r#"{"selected":"idn:x","auto_reconnect":false}"#).unwrap();
        assert_eq!((p.selected.as_deref(), p.auto_reconnect, p.pps), (Some("idn:x"), false, 30_000));
    }

    #[test]
    fn the_backoff_doubles_up_to_fifteen_seconds() {
        let s: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(s, [1, 2, 4, 8, 15, 15, 15, 15]);
    }

    #[test]
    fn selecting_connects_on_the_manager_thread_and_hands_the_output_over() {
        let (mgr, _, _) = manager(true);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        assert_eq!(mgr.state(), LinkState::Connecting, "nothing opened by the caller");
        assert!(mgr.engine_take().is_none());
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Connected);
        assert_eq!(changes(&mgr), [(ChangeKind::Connected, Some("a".into()), true)]);
        let list = mgr.list();
        assert_eq!(list.iter().map(|o| (o.id.as_str(), o.state)).collect::<Vec<_>>(), [("a", LinkState::Connected), ("b", LinkState::Available)]);
        assert!(mgr.is_current(mgr.current_gen()));
    }

    #[test]
    fn two_claims_on_one_output_only_one_wins() {
        let dir = crate::test_support::scratch_dir().join("locks");
        let first = DacLock::acquire(&dir, "etherdream:a").unwrap();
        let second = DacLock::acquire(&dir, "etherdream:a").unwrap_err();
        assert!(format!("{second:#}").contains(IN_USE_FR), "{second:#}");
        assert!(DacLock::acquire(&dir, "etherdream:b").is_ok(), "another output is free");
        drop(first);
        assert!(DacLock::acquire(&dir, "etherdream:a").is_ok(), "let go with the claim");
        let long = "x".repeat(300);
        assert!(DacLock::acquire(&dir, &long).is_ok());
    }

    #[test]
    fn two_studios_sharing_a_lock_folder_never_drive_the_same_output() {
        let dir = crate::test_support::scratch_dir().join("locks");
        let (a, _, _) = manager(false);
        let (b, _, _) = manager(false);
        let (a, b) = (a.with_lock_dir(dir.clone()), b.with_lock_dir(dir));
        let now = Instant::now();
        a.select(Some("a".into()), ArmSource::Ui);
        drain(&a, now);
        b.select(Some("a".into()), ArmSource::Ui);
        drain(&b, now);
        assert_eq!(a.state(), LinkState::Connected);
        assert_eq!(b.state(), LinkState::Lost);
        assert!(b.view(now)["error"].as_str().unwrap().contains(IN_USE_FR));
        assert!(changes(&b).is_empty(), "nothing handed to the second engine");
        // The first lets go: the second may connect.
        a.select(None, ArmSource::Ui);
        drain(&a, now);
        b.select(Some("a".into()), ArmSource::Ui);
        drain(&b, now);
        assert_eq!(b.state(), LinkState::Connected);
    }

    #[test]
    fn a_hold_drops_the_outputs_and_refuses_new_ones_without_forgetting_the_choice() {
        let (mgr, _, _) = manager(false);
        let now = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, now);
        let _ = changes(&mgr);
        mgr.set_hold(Some("Machine de secours en veille".into()));
        drain(&mgr, now);
        assert_eq!(changes(&mgr), [(ChangeKind::Disconnected, None, false)]);
        assert!(!mgr.holds_any());
        assert_eq!(mgr.prefs().selected.as_deref(), Some("a"), "the remembered choice stays");
        mgr.select(Some("b".into()), ArmSource::Ui);
        drain(&mgr, now);
        assert_eq!(mgr.state(), LinkState::Lost);
        assert_eq!(mgr.view(now)["error"], "Machine de secours en veille");
        assert!(changes(&mgr).is_empty());
        mgr.set_hold(None);
        assert!(mgr.hold().is_none());
    }

    #[test]
    fn switching_closes_the_old_output_before_opening_the_new_one() {
        let (mgr, _, _) = manager(true);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        changes(&mgr);
        mgr.select(Some("b".into()), ArmSource::Ui);
        drain(&mgr, t0);
        assert_eq!(changes(&mgr), [(ChangeKind::Disconnected, None, false), (ChangeKind::Connected, Some("b".into()), true)]);
        mgr.select(None, ArmSource::Ui);
        drain(&mgr, t0);
        assert_eq!(changes(&mgr), [(ChangeKind::Disconnected, None, false)]);
        assert_eq!(mgr.state(), LinkState::Available);
        assert_eq!(mgr.prefs().selected, None);
    }

    /// Two quick choices: only the last one is opened and handed over.
    #[test]
    fn a_newer_choice_wins() {
        let (mgr, _, opens) = manager(true);
        mgr.select(Some("a".into()), ArmSource::Ui);
        mgr.select(Some("b".into()), ArmSource::Ui);
        drain(&mgr, Instant::now());
        assert_eq!(changes(&mgr), [(ChangeKind::Connected, Some("b".into()), true)]);
        assert_eq!(opens.load(Ordering::SeqCst), 1, "a was never opened");
    }

    #[test]
    fn a_lost_output_is_retried_with_backoff_and_capped() {
        let (mgr, dropped, opens) = manager(true);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        changes(&mgr);
        dropped.store(true, Ordering::SeqCst);
        mgr.report_lost(mgr.current_gen(), "a ne répond plus".into());
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Lost);
        assert_eq!(mgr.view(t0)["retry_in_ms"], 1000);
        // Attempts at 1, 1+2, 3+4, 7+8, 15+15, 30+15 s.
        let mut t = t0;
        let mut attempts = Vec::new();
        let before = opens.load(Ordering::SeqCst);
        for _ in 0..(50 * 10) {
            t += Duration::from_millis(100);
            let n = opens.load(Ordering::SeqCst);
            mgr.step(None, t);
            if opens.load(Ordering::SeqCst) > n {
                attempts.push((t - t0).as_millis() as u64 / 100);
            }
        }
        assert_eq!(attempts, [10, 30, 70, 150, 300, 450], "{attempts:?}");
        assert_eq!(opens.load(Ordering::SeqCst) - before, 6);
        assert!(mgr.engine_take().is_none(), "nothing handed over while unreachable");
        // Back: the next attempt reconnects, flagged as a reconnection.
        dropped.store(false, Ordering::SeqCst);
        for _ in 0..200 {
            t += Duration::from_millis(100);
            mgr.step(None, t);
        }
        assert_eq!(mgr.state(), LinkState::Connected);
        assert_eq!(changes(&mgr), [(ChangeKind::Reconnected, Some("a".into()), true)]);
        assert_eq!(mgr.view(t)["retry_in_ms"], serde_json::Value::Null);
    }

    #[test]
    fn no_retry_when_auto_reconnect_is_off() {
        let (mgr, dropped, opens) = manager(false);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        changes(&mgr);
        dropped.store(true, Ordering::SeqCst);
        mgr.report_lost(mgr.current_gen(), "perdue".into());
        drain(&mgr, t0);
        let n = opens.load(Ordering::SeqCst);
        for s in 1..60 {
            mgr.step(None, t0 + Duration::from_secs(s));
        }
        assert_eq!(opens.load(Ordering::SeqCst), n);
        assert_eq!(mgr.state(), LinkState::Lost);
        // Turned on: tried after the backoff from now.
        dropped.store(false, Ordering::SeqCst);
        let t1 = t0 + Duration::from_secs(60);
        mgr.set_auto_reconnect(true, t1);
        mgr.step(None, t1 + Duration::from_millis(900));
        assert_eq!(mgr.state(), LinkState::Lost);
        mgr.step(None, t1 + Duration::from_secs(1));
        assert_eq!(mgr.state(), LinkState::Connected);
        // And turned off while lost: the retries stop.
        mgr.report_lost(mgr.current_gen(), "perdue".into());
        drain(&mgr, t1);
        mgr.set_auto_reconnect(false, t1);
        let n = opens.load(Ordering::SeqCst);
        mgr.step(None, t1 + Duration::from_secs(30));
        assert_eq!(opens.load(Ordering::SeqCst), n);
        // Disconnecting a lost output clears it from the engine too.
        changes(&mgr);
        mgr.select(None, ArmSource::Ui);
        drain(&mgr, t1);
        assert_eq!(changes(&mgr), [(ChangeKind::Disconnected, None, false)]);
        assert_eq!(mgr.state(), LinkState::Available);
    }

    /// A loss reported for an output that was replaced since is ignored.
    #[test]
    fn a_late_loss_report_never_marks_the_new_output_lost() {
        let (mgr, _, _) = manager(true);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        let old = mgr.current_gen();
        mgr.select(Some("b".into()), ArmSource::Ui);
        drain(&mgr, t0);
        mgr.report_lost(old, "a ne répond plus".into());
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Connected);
        assert!(!mgr.is_current(old));
    }

    #[test]
    fn a_failed_first_connection_is_lost_and_retried() {
        let (mgr, dropped, _) = manager(true);
        dropped.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Lost);
        assert!(mgr.view(t0)["error"].as_str().unwrap().contains("injoignable"));
        dropped.store(false, Ordering::SeqCst);
        mgr.step(None, t0 + Duration::from_secs(1));
        assert_eq!(mgr.state(), LinkState::Connected);
    }

    #[test]
    fn the_fake_backend_never_opens_anything_unknown() {
        let backend = FakeBackend::new(vec![dev("a")], None, Some("127.0.0.1:9".parse().unwrap()), crate::test_support::scratch_dir());
        assert!(backend.open("helios:left", 30_000).is_err());
        assert!(backend.open("ponk:127.0.0.1:5583", 30_000).is_err(), "PONK only to --ponk's address");
        assert!(backend.open(TEST_ID, 30_000).is_err(), "no --test-output");
        let out = backend.open("a", 30_000).unwrap();
        assert_eq!((out.name(), out.kind()), ("Ether Dream a", "test"));
        assert_eq!(backend.fixed().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["ponk:127.0.0.1:9"]);
    }

    #[test]
    fn unplugging_a_fake_output_reports_a_fault() {
        let backend = FakeBackend::new(vec![dev("a")], None, None, crate::test_support::scratch_dir());
        let out = backend.open("a", 30_000).unwrap();
        assert_eq!(out.fault(), None);
        backend.dropped.store(true, Ordering::SeqCst);
        assert!(out.fault().unwrap().contains("débranchée"));
        assert!(backend.open("a", 30_000).is_err());
    }

    #[test]
    fn the_real_backend_lists_the_ponk_target_without_scanning() {
        let backend = RealBackend { ponk: crate::ponk::DEFAULT_TARGET.parse().unwrap(), data_dir: crate::test_support::scratch_dir(), artnet: Arc::new(crate::artnet::ShowNetDmxStore::in_memory()) };
        let fixed = backend.fixed();
        assert_eq!(fixed.len(), 1, "the Art-Net box is off by default");
        assert_eq!((fixed[0].kind.as_str(), fixed[0].id.as_str()), ("ponk", "ponk:127.0.0.1:5583"));
    }

    /// A ShowNET output goes through the manager like any other (T-471):
    /// opened on its thread, handed over disarmed, reported lost when the
    /// box goes away. Its kind is what the checklist (T-472) looks at.
    #[cfg(feature = "shownet")]
    #[test]
    fn a_shownet_output_connects_and_is_lost_like_a_dac() {
        use crate::interlock::EStop;
        use crate::output::OutputStage;
        let fake = crate::shownet::fake::FakeShowNet::new(&[("box-1", 100_000)]);
        let mut backend = FakeBackend::new(Vec::new(), None, None, crate::test_support::scratch_dir());
        backend.shownet = Some(Arc::new(fake.clone()));
        let list = Arc::new(DeviceList::fixed(Vec::new()));
        let mgr = OutputManager::new(Arc::new(backend), list, &OutputPrefs::default(), 150_000);
        let t0 = Instant::now();
        mgr.select(Some("shownet:box-1".into()), ArmSource::Ui);
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Connected);
        assert_eq!(mgr.view(t0)["kind"], "shownet");
        let change = mgr.engine_take().unwrap();
        let mut stage = OutputStage::new(change.output);
        assert!(stage.kill_switch().is_some(), "the e-stop can reach it");
        let estop = EStop::default();
        let lit = [Point::lit(0.0, 0.0, 1.0, 1.0, 1.0)];
        assert_eq!(stage.emit(&lit, true, true, &estop).lit, 1);
        fake.lost.store(true, Ordering::SeqCst);
        stage.emit(&lit, true, true, &estop);
        let deadline = Instant::now() + Duration::from_secs(2);
        while stage.fault().is_none() {
            assert!(Instant::now() < deadline, "loss reported");
            std::thread::sleep(Duration::from_millis(2));
        }
        mgr.report_lost(change.gen, stage.fault().unwrap());
        drain(&mgr, t0);
        assert_eq!(mgr.state(), LinkState::Lost);
        drop(stage.replace(None));
        assert!(fake.events.lock().unwrap().iter().all(|e| !matches!(e, crate::shownet::fake::Event::Frame { pps, .. } if *pps != 100_000)), "rate capped");
        // An unknown box fails to open, without an SDK nothing does.
        assert!(FakeBackend::new(Vec::new(), None, None, crate::test_support::scratch_dir()).open("shownet:box-1", 30_000).is_err());
    }

    /// The ShowNET SD card output (T-301): listed once an address is set,
    /// connected through the manager like any output, and in tests only
    /// towards a loopback receiver.
    #[test]
    fn the_artnet_box_is_listed_once_typed_and_opens_on_loopback_only() {
        use crate::artnet::{ShowNetDmxConfig, ShowNetDmxStore};
        let (rx, addr) = crate::artnet::tests::receiver();
        let store = Arc::new(ShowNetDmxStore::in_memory());
        let mut backend = FakeBackend::new(Vec::new(), None, None, crate::test_support::scratch_dir());
        backend.artnet = Arc::clone(&store);
        assert!(backend.fixed().is_empty(), "off by default");
        store.set(ShowNetDmxConfig { target: Some(addr.to_string()), ..Default::default() }).unwrap();
        let id = format!("artnet:{addr}");
        assert_eq!(backend.fixed().iter().map(|d| (d.kind.as_str(), d.id.as_str())).collect::<Vec<_>>(), [("artnet", id.as_str())]);
        assert!(backend.open("artnet:192.168.129.51:6454", 30_000).is_err(), "never a real box in tests");
        assert!(backend.open("artnet:nonsense", 30_000).is_err());
        let list = Arc::new(DeviceList::fixed(Vec::new()));
        let mgr = OutputManager::new(Arc::new(backend), list, &OutputPrefs::default(), 30_000);
        mgr.select(Some(id.clone()), ArmSource::Ui);
        drain(&mgr, Instant::now());
        assert_eq!(mgr.state(), LinkState::Connected);
        assert_eq!(mgr.view(Instant::now())["kind"], "artnet");
        let change = mgr.engine_take().unwrap();
        assert_eq!(change.output.as_ref().unwrap().name(), crate::artnet::output_name(addr));
        let (_, d) = crate::artnet::tests::next_dmx(&rx);
        assert_eq!((d[0], d[1]), (0, 0), "opened dark");
    }

    #[test]
    fn the_list_keeps_a_selection_missing_from_the_scan() {
        let (mgr, _, _) = manager(true);
        {
            let mut st = mgr.status.lock().unwrap();
            st.selected = Some("auto".into());
            st.name = Some("Ether Dream (etherdream:1)".into());
            st.kind = Some("dac");
            st.state = LinkState::Connected;
        }
        let list = mgr.list();
        assert_eq!(list.last().unwrap().id, "auto");
        assert_eq!(list.last().unwrap().state, LinkState::Connected);
        assert!(mgr.knows("auto") && mgr.knows("b") && !mgr.knows("c"));
    }

    fn projector_changes(mgr: &OutputManager) -> Vec<(Option<String>, ChangeKind, Option<String>, bool)> {
        std::iter::from_fn(|| mgr.engine_take()).map(|c| (c.projector, c.kind, c.id, c.output.is_some())).collect()
    }

    #[test]
    fn a_venue_projector_gets_its_own_output_and_generations() {
        let (mgr, _, _) = manager(true);
        let t0 = Instant::now();
        mgr.select(Some("a".into()), ArmSource::Ui);
        drain(&mgr, t0);
        let main_gen = mgr.current_gen();
        mgr.select_projector("p2", Some("b".into()), ArmSource::Ui);
        drain(&mgr, t0);
        let got = projector_changes(&mgr);
        assert_eq!(got.last().unwrap(), &(Some("p2".into()), ChangeKind::Connected, Some("b".into()), true));
        assert!(mgr.is_current(main_gen), "a projector's hand-over leaves the main generation alone");
        assert!(mgr.is_current_projector("p2", 1));
        assert_eq!(mgr.driver_of("b"), Some(Some("p2".into())));
        assert_eq!(mgr.driver_of("a"), Some(None));
        assert_eq!(mgr.list().iter().find(|o| o.id == "b").unwrap().state, LinkState::Connected);
        assert_eq!(mgr.projector_links().len(), 1);
        // Removing it: the engine gets a dark hand-over and the slot goes.
        mgr.select_projector("p2", None, ArmSource::Ui);
        drain(&mgr, t0);
        assert_eq!(projector_changes(&mgr), [(Some("p2".into()), ChangeKind::Disconnected, None, false)]);
        assert!(mgr.projector_links().is_empty());
        assert_eq!(mgr.driver_of("b"), None);
    }

    #[test]
    fn a_lost_projector_output_is_retried_and_comes_back_disarmed() {
        let (mgr, dropped, _) = manager(true);
        let t0 = Instant::now();
        mgr.select_projector("p2", Some("b".into()), ArmSource::Ui);
        drain(&mgr, t0);
        projector_changes(&mgr);
        dropped.store(true, Ordering::SeqCst);
        mgr.report_lost_projector("p2", 1, "b ne répond plus".into());
        drain(&mgr, t0);
        assert_eq!(mgr.projector_links()[0].state, LinkState::Lost);
        mgr.step(None, t0 + Duration::from_millis(1500));
        assert_eq!(mgr.projector_links()[0].state, LinkState::Lost, "still unplugged");
        dropped.store(false, Ordering::SeqCst);
        mgr.step(None, t0 + Duration::from_secs(10));
        assert_eq!(mgr.projector_links()[0].state, LinkState::Connected);
        assert_eq!(projector_changes(&mgr), [(Some("p2".into()), ChangeKind::Reconnected, Some("b".into()), true)]);
        // A late report about an older hand-over changes nothing.
        mgr.report_lost_projector("p2", 1, "vieux".into());
        drain(&mgr, t0);
        assert_eq!(mgr.projector_links()[0].state, LinkState::Connected);
    }
}
