//! Choosing, connecting and reconnecting the output from the UI (T-401).
//!
//! The manager lists what can be driven (the DACs of the last scan,
//! `devices.rs`, plus the PONK target and the fake test output), opens the
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
    /// "helios", "etherdream", "idn" (from the scan), "ponk", "test",
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
}

enum Cmd {
    Select(Option<String>, ArmSource),
    Lost(u64, String),
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
        }
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
        let out = self.backend.open(id, pps)?;
        let mut st = self.status.lock().unwrap();
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
            out.push(OutputInfo {
                state: if selected { st.state } else { LinkState::Available },
                error: if selected { st.error.clone() } else { None },
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
            None => self.on_tick(now),
        }
    }

    fn on_select(&self, id: Option<String>, src: ArmSource, now: Instant) {
        // The old output closes first (a USB DAC can only be opened once).
        let handed = std::mem::take(&mut self.status.lock().unwrap().handed);
        if handed {
            self.hand_over(None, ChangeKind::Disconnected, src, None);
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
        let pps = self.status.lock().unwrap().pps;
        let opened = self.backend.open(id, pps);
        let mut st = self.status.lock().unwrap();
        if st.selected.as_deref() != Some(id) {
            // The operator chose something else meanwhile: close it unused.
            drop(st);
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
        let _ = self.stage_tx.lock().unwrap().send(StageChange { output, kind, source, gen, id });
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
        let backend = FakeBackend::new(vec![dev("a"), dev("b")], None, None, std::env::temp_dir());
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
        let backend = FakeBackend::new(vec![dev("a")], None, Some("127.0.0.1:9".parse().unwrap()), std::env::temp_dir());
        assert!(backend.open("helios:left", 30_000).is_err());
        assert!(backend.open("ponk:127.0.0.1:5583", 30_000).is_err(), "PONK only to --ponk's address");
        assert!(backend.open(TEST_ID, 30_000).is_err(), "no --test-output");
        let out = backend.open("a", 30_000).unwrap();
        assert_eq!((out.name(), out.kind()), ("Ether Dream a", "test"));
        assert_eq!(backend.fixed().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["ponk:127.0.0.1:9"]);
    }

    #[test]
    fn unplugging_a_fake_output_reports_a_fault() {
        let backend = FakeBackend::new(vec![dev("a")], None, None, std::env::temp_dir());
        let out = backend.open("a", 30_000).unwrap();
        assert_eq!(out.fault(), None);
        backend.dropped.store(true, Ordering::SeqCst);
        assert!(out.fault().unwrap().contains("débranchée"));
        assert!(backend.open("a", 30_000).is_err());
    }

    #[test]
    fn the_real_backend_lists_the_ponk_target_without_scanning() {
        let backend = RealBackend { ponk: crate::ponk::DEFAULT_TARGET.parse().unwrap(), data_dir: std::env::temp_dir() };
        let fixed = backend.fixed();
        assert_eq!(fixed.len(), 1);
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
        let mut backend = FakeBackend::new(Vec::new(), None, None, std::env::temp_dir());
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
        assert!(FakeBackend::new(Vec::new(), None, None, std::env::temp_dir()).open("shownet:box-1", 30_000).is_err());
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
}
