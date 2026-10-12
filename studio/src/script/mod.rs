//! Sandboxed scripts (T-458): a little logic on top of the command line
//! (T-424) and the macros (T-425): « above 140 BPM go to page 3 », « a
//! random cue out of six every four bars ».
//!
//! A script reads a snapshot of the state (`etat()`) and queues command
//! lines (`commande("…")`); nothing else reaches the studio (see `lang.rs`
//! for the sandbox and `docs/design/scripts.md` for the frozen surface).
//!
//! Threads: scripts run in their own thread (`Worker`). The engine only
//! posts a job (the source, a snapshot, the budgets) and, at the top of a
//! later frame, reads the finished runs without waiting (`try_recv`). A run
//! that loops costs its own thread up to its time budget, never a frame.
//!
//! Rights: a controller's. The queued lines are parsed again by
//! `command::parse` on the registry of the moment and run by
//! `command::execute` with the source « Script », so the command line's
//! refusals apply, and `controls::apply_from` checks the `external` flag
//! again. Lines are also checked when a script is saved or loaded (its
//! literal `commande("…")`), and in the script's thread before queueing
//! (`command::refusal`). A run whose lines don't all parse applies none.
//!
//! Stops: the e-stop (blackout included), a disarm and « Tout arrêter »
//! cancel every run (their commands dropped) and suspend the automatic
//! triggers until *Reprendre les scripts*: nothing resumes by itself. A
//! run past its budget stops its script's triggers until the operator
//! acts on it. The strobe limiter acts on the output, after everything a
//! script can change.
//!
//! Trust: a list approved on this machine (its fingerprint in
//! `scripts_trust.json`) runs; a list that came with a project and was
//! never approved here runs nothing until *Activer les scripts de ce
//! projet*.

pub mod lang;

pub use lang::ScriptLimits;

use crate::controls::{self, ControlSource};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

pub const MAX_SCRIPTS: usize = 32;
pub const MAX_SOURCE: usize = lang::MAX_SOURCE;
pub const MAX_NAME: usize = 32;
/// Console lines kept per script.
const MAX_CONSOLE: usize = 100;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Script {
    pub id: u32,
    pub name: String,
    /// At most `MAX_SOURCE` bytes.
    pub source: String,
    pub trigger: ScriptTrigger,
    /// The automatic trigger is on (a manual run doesn't need it).
    pub enabled: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self { id: 0, name: String::new(), source: String::new(), trigger: ScriptTrigger::Manual, enabled: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptTrigger {
    #[default]
    Manual,
    EveryBeat,
    EveryBar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Ready,
    Running,
    Error,
    Budget,
    Stopped,
}

impl RunState {
    pub fn label_fr(self) -> &'static str {
        match self {
            RunState::Ready => "prêt",
            RunState::Running => "en cours",
            RunState::Error => "erreur",
            RunState::Budget => "arrêté : budget dépassé",
            RunState::Stopped => "arrêté",
        }
    }
}

/// What the page shows of one script.
#[derive(Clone, Debug, Serialize)]
pub struct ScriptStatus {
    pub id: u32,
    pub state: String,
    pub kind: RunState,
    pub last_error: Option<String>,
    pub runs: u64,
    pub last_ms: f32,
    /// Its automatic trigger waits for an operator's action (budget, stop).
    pub blocked: bool,
    pub console: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Status {
    kind: RunState,
    last_error: Option<String>,
    runs: u64,
    last_ms: f32,
    blocked: bool,
    console: VecDeque<String>,
}

impl Status {
    fn log(&mut self, line: String) {
        if self.console.len() >= MAX_CONSOLE {
            self.console.pop_front();
        }
        self.console.push_back(line);
    }

    fn fail(&mut self, kind: RunState, msg: String) {
        self.log(msg.clone());
        self.kind = kind;
        self.last_error = Some(msg);
    }
}

/// Why a list can't be saved, in French: which script and line.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScriptSaveError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message_fr: String,
    /// A safety refusal (arming, reset, safety settings), not a typo.
    pub refused: bool,
}

impl std::fmt::Display for ScriptSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message_fr)
    }
}

// ---------- the worker ----------

struct Job {
    serial: u64,
    source: String,
    state: Value,
    seed: u64,
    limits: ScriptLimits,
    cancel: Arc<AtomicBool>,
}

struct Outcome {
    serial: u64,
    result: Result<(), lang::ScriptError>,
    out: lang::RunOutput,
    ms: f32,
}

struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Outcome>,
}

/// The refusals a script meets before a line is even queued: the command
/// line's own (`command::refusal`), and no script launching a script.
fn refusal(line: &str) -> Option<String> {
    if let Some(why) = crate::command::refusal(line) {
        return Some(why.trim_start_matches("Commande refusée : ").to_string());
    }
    let first = line.split_whitespace().next().map(crate::command::fold).unwrap_or_default();
    first.starts_with("script.").then(|| "un script ne lance pas de script".to_string())
}

fn execute(job: &Job) -> Outcome {
    let start = Instant::now();
    let host = lang::Host { state: &job.state, seed: job.seed, cancel: Some(&job.cancel), refusal: &refusal };
    let (result, out) = lang::run(&job.source, job.limits, &host);
    Outcome { serial: job.serial, result, out, ms: start.elapsed().as_secs_f32() * 1000.0 }
}

impl Worker {
    fn spawn() -> Option<Worker> {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (done, rx) = mpsc::channel::<Outcome>();
        let spawned = std::thread::Builder::new().name("scripts".into()).stack_size(4 << 20).spawn(move || {
            for job in jobs {
                let o = execute(&job);
                if done.send(o).is_err() {
                    break;
                }
            }
        });
        match spawned {
            Ok(_) => Some(Worker { tx, rx }),
            Err(e) => {
                log::error!("scripts : fil impossible à lancer ({e})");
                None
            }
        }
    }
}

struct Pending {
    serial: u64,
    id: u32,
    cancel: Arc<AtomicBool>,
}

/// The audio levels the last frame saw, for `etat()`.
#[derive(Clone, Copy, Debug, Default)]
struct AudioSeen {
    level: f32,
    bass: f32,
    mid: f32,
    high: f32,
    silent: bool,
}

// ---------- the library and its runner ----------

#[derive(Default)]
pub struct Scripts {
    list: Vec<Script>,
    path: Option<PathBuf>,
    trust_path: Option<PathBuf>,
    approved: Vec<String>,
    /// Bumped on every change of the list, so the page reloads it.
    pub rev: u64,
    status: BTreeMap<u32, Status>,
    pending: Vec<Pending>,
    next_serial: u64,
    /// The automatic triggers are suspended (a stop) until *Reprendre*.
    pub halted: bool,
    /// Bumped by every stop: a run being applied stops at it.
    stops: u64,
    last_beat: Option<i64>,
    last_bar: Option<i64>,
    was_armed: bool,
    audio: AudioSeen,
    worker: Option<Worker>,
    /// Tests: runs on the calling thread, applied at the next frame like
    /// the worker's, so a session replays exactly.
    inline: bool,
    inline_done: Vec<Outcome>,
    pub limits: ScriptLimits,
    /// `hasard()`'s base seed (each run mixes in its serial).
    pub seed: u64,
    /// What loading found wrong (scripts disabled), for the page and the
    /// project's open report.
    pub warnings: Vec<String>,
    /// The banner: the list came with a project and isn't approved here.
    pub ask_trust: bool,
}

fn fingerprint(list: &[Script]) -> String {
    use sha2::Digest;
    let key: Vec<(&str, &str, ScriptTrigger)> = list.iter().map(|s| (s.name.as_str(), s.source.as_str(), s.trigger)).collect();
    let bytes = serde_json::to_vec(&key).unwrap_or_default();
    sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn clip_source(src: &mut String) {
    if src.len() > MAX_SOURCE {
        let mut end = MAX_SOURCE;
        while !src.is_char_boundary(end) {
            end -= 1;
        }
        src.truncate(end);
    }
}

/// A file or a project, repaired rather than refused: the count cut, ids
/// made unique, names clipped and unique, sources clipped.
pub fn sanitize(list: &mut Vec<Script>) {
    list.truncate(MAX_SCRIPTS);
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let taken: Vec<u32> = list.iter().map(|s| s.id).collect();
    let mut n = 0;
    for sc in list.iter_mut() {
        if sc.id == 0 || !ids.insert(sc.id) {
            sc.id = loop {
                n += 1;
                if !taken.contains(&n) && !ids.contains(&n) {
                    break n;
                }
            };
            ids.insert(sc.id);
        }
        sc.name = sc.name.trim().chars().take(MAX_NAME).collect();
        if sc.name.is_empty() || !names.insert(crate::command::fold(&sc.name)) {
            let base: String = if sc.name.is_empty() { "Script".into() } else { sc.name.chars().take(MAX_NAME - 4).collect() };
            sc.name = (2..).map(|i| format!("{base} {i}")).find(|c| !names.contains(&crate::command::fold(c))).expect("a free name");
            names.insert(crate::command::fold(&sc.name));
        }
        clip_source(&mut sc.source);
    }
}

impl Scripts {
    /// `<dir>/scripts.json` and `<dir>/scripts_trust.json`.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("scripts.json");
        let trust_path = dir.join("scripts_trust.json");
        let mut list: Vec<Script> = crate::load_json(&path);
        sanitize(&mut list);
        let approved: Vec<String> = crate::load_json(&trust_path);
        let mut s = Self { list, path: Some(path), trust_path: Some(trust_path), approved, ..Self::default() };
        s.ask_trust = !s.trusted();
        s
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Tests: run on the calling thread (see `inline`).
    #[cfg(test)]
    pub fn set_inline(&mut self, on: bool) {
        self.inline = on;
    }

    pub fn list(&self) -> &[Script] {
        &self.list
    }

    pub fn get(&self, id: u32) -> Option<&Script> {
        self.list.iter().find(|s| s.id == id)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The list runs on this machine: empty, or its fingerprint approved.
    pub fn trusted(&self) -> bool {
        self.list.is_empty() || self.approved.contains(&fingerprint(&self.list))
    }

    pub fn is_running(&self, id: u32) -> bool {
        self.pending.iter().any(|p| p.id == id)
    }

    pub fn running_ids(&self) -> Vec<u32> {
        self.pending.iter().map(|p| p.id).collect()
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.list);
        }
    }

    fn approve_current(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let f = fingerprint(&self.list);
        if !self.approved.contains(&f) {
            self.approved.push(f);
            if let Some(path) = &self.trust_path {
                crate::save_json(path, &self.approved);
            }
        }
    }

    /// A list from a project (or an undo): swapped in, every run cancelled.
    pub fn replace_in_memory(&mut self, mut list: Vec<Script>) {
        sanitize(&mut list);
        self.cancel_all();
        self.list = list;
        let list = &self.list;
        self.status.retain(|id, _| list.iter().any(|s| s.id == *id));
        self.ask_trust = !self.trusted();
        self.rev += 1;
    }

    fn cancel_all(&mut self) {
        for p in self.pending.drain(..) {
            p.cancel.store(true, Ordering::Relaxed);
            if let Some(st) = self.status.get_mut(&p.id) {
                st.kind = RunState::Stopped;
            }
        }
        self.stops += 1;
    }

    fn status_mut(&mut self, id: u32) -> &mut Status {
        self.status.entry(id).or_default()
    }
}

// ---------- checks ----------

fn save_error(sc: &Script, e: &lang::ScriptError) -> ScriptSaveError {
    ScriptSaveError { id: Some(sc.id), line: e.line, message_fr: format!("Script « {} », {}", sc.name, lower_first(&e.to_string())), refused: e.kind == lang::ErrorKind::Refused }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// The registry with the scripts being saved in it (a macro or a script
/// may name `script.<id>.run`).
fn registry_for(s: &Shared, list: &[Script]) -> controls::ControlRegistry {
    let mut reg = controls::ControlRegistry::build_for(&s.presets, &s.grid);
    reg.add_macros(s.macros.list());
    reg.add_scripts(list);
    reg
}

/// One script: parsed, every literal `commande("…")` parsed on `reg`.
/// `Err` with the line; `refused` for a safety refusal.
fn check_one(sc: &Script, reg: &controls::ControlRegistry, presets: &[crate::presets::Preset]) -> Result<(), lang::ScriptError> {
    let program = lang::parse(&sc.source)?;
    for (line, cmd) in lang::literal_commands(&program) {
        let refused = |why: String| lang::ScriptError { line: Some(line), message_fr: format!("commande refusée « {} » : {why}", cmd.trim()), kind: lang::ErrorKind::Refused };
        if let Some(why) = refusal(&cmd) {
            return Err(refused(why));
        }
        if let Err(e) = crate::command::parse(&cmd, reg, presets) {
            if e.refused {
                return Err(refused(e.message_fr.trim_start_matches("Commande refusée : ").to_string()));
            }
            return Err(lang::ScriptError { line: Some(line), message_fr: format!("commande « {} » : {}", cmd.trim(), e.message_fr), kind: lang::ErrorKind::Runtime });
        }
    }
    Ok(())
}

/// `POST /api/scripts`: the whole list, refused rather than repaired.
/// Ids left at 0 get the next free one. A refused line is never saved; a
/// syntax error or an unknown command only in a script left off.
pub fn validate(s: &Shared, list: &mut [Script]) -> Result<(), ScriptSaveError> {
    let whole = |m: String| ScriptSaveError { id: None, line: None, message_fr: m, refused: false };
    if list.len() > MAX_SCRIPTS {
        return Err(whole(format!("{MAX_SCRIPTS} scripts au plus")));
    }
    let given: Vec<u32> = list.iter().map(|s| s.id).collect();
    let mut n = 0;
    for sc in list.iter_mut().filter(|s| s.id == 0) {
        sc.id = loop {
            n += 1;
            if !given.contains(&n) {
                break n;
            }
        };
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for sc in list.iter_mut() {
        if !ids.insert(sc.id) {
            return Err(whole(format!("identifiant de script en double : {}", sc.id)));
        }
        sc.name = sc.name.trim().to_string();
        let len = sc.name.chars().count();
        if len == 0 || len > MAX_NAME {
            return Err(ScriptSaveError { id: Some(sc.id), line: None, message_fr: format!("nom de script de 1 à {MAX_NAME} caractères"), refused: false });
        }
        if !names.insert(crate::command::fold(&sc.name)) {
            return Err(ScriptSaveError { id: Some(sc.id), line: None, message_fr: format!("Script « {} » : nom déjà pris", sc.name), refused: false });
        }
        if sc.source.len() > MAX_SOURCE {
            return Err(ScriptSaveError { id: Some(sc.id), line: None, message_fr: format!("Script « {} » : {} Ko au plus", sc.name, MAX_SOURCE / 1024), refused: false });
        }
    }
    let reg = registry_for(s, list);
    for sc in list.iter() {
        if let Err(e) = check_one(sc, &reg, &s.presets) {
            if e.kind == lang::ErrorKind::Refused || sc.enabled {
                return Err(save_error(sc, &e));
            }
        }
    }
    Ok(())
}

/// `POST /api/scripts/check`: one source, nothing saved. `None` when fine.
pub fn check_source(s: &Shared, source: &str) -> Option<ScriptSaveError> {
    let sc = Script { id: 0, name: "?".into(), source: source.to_string(), ..Script::default() };
    let reg = registry_for(s, s.scripts.list());
    check_one(&sc, &reg, &s.presets).err().map(|e| ScriptSaveError { id: None, line: e.line, message_fr: e.to_string(), refused: e.kind == lang::ErrorKind::Refused })
}

/// `POST /api/scripts` (and an undo): checked, swapped in, saved, the
/// registry rebuilt. A list approved before stays approved once edited
/// here; saving lifts the budget blocks.
pub fn set(s: &mut Shared, mut list: Vec<Script>) -> Result<(), ScriptSaveError> {
    validate(s, &mut list)?;
    let trusted = s.scripts.trusted();
    let sc = &mut s.scripts;
    // Runs of scripts whose source changed or that are gone end.
    for p in &sc.pending {
        if !list.iter().any(|n| n.id == p.id && sc.get(p.id).is_some_and(|o| o.source == n.source)) {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }
    sc.pending.retain(|p| !p.cancel.load(Ordering::Relaxed));
    sc.list = list;
    let ids: Vec<u32> = sc.list.iter().map(|s| s.id).collect();
    sc.status.retain(|id, _| ids.contains(id));
    for st in sc.status.values_mut() {
        st.blocked = false;
        if st.kind == RunState::Budget {
            st.kind = RunState::Ready;
        }
    }
    if trusted {
        sc.approve_current();
    }
    sc.ask_trust = !sc.trusted();
    sc.warnings.clear();
    sc.rev += 1;
    sc.save();
    crate::grid::refresh(s);
    Ok(())
}

/// After a load (startup, project): each script checked on the registry
/// as it is now; a bad one is switched off with a French warning.
pub fn revalidate(s: &mut Shared) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut list = s.scripts.list.clone();
    for sc in list.iter_mut() {
        if let Err(e) = check_one(sc, &s.controls, &s.presets) {
            if sc.enabled {
                sc.enabled = false;
            }
            let msg = format!("{} : script désactivé", save_error(sc, &e).message_fr);
            s.scripts.status_mut(sc.id).fail(RunState::Error, e.to_string());
            warnings.push(msg);
        }
    }
    if list != s.scripts.list {
        s.scripts.list = list;
        s.scripts.rev += 1;
    }
    for w in &warnings {
        log::warn!("{w}");
    }
    s.scripts.warnings = warnings.clone();
    warnings
}

/// *Activer les scripts de ce projet* (`yes`) or *Laisser désactivés*.
pub fn approve(s: &mut Shared, yes: bool) {
    if yes {
        s.scripts.approve_current();
    }
    s.scripts.ask_trust = false;
    s.scripts.rev += 1;
}

// ---------- running ----------

/// The read-only copy `etat()` returns. Never a safety setting, never a path.
fn snapshot(s: &Shared) -> Value {
    let t = s.now_s();
    let tempo = s.tempo.state(t);
    let mut controls = serde_json::Map::new();
    for d in s.controls.list().iter().filter(|d| d.external && d.group != "safety") {
        if let Some(v) = controls::current(s, d) {
            if v.is_number() || v.is_boolean() || v.is_string() {
                controls.insert(d.id.clone(), v);
            }
        }
    }
    let cues: Vec<String> = s.deck.active.iter().filter_map(|a| s.presets.iter().find(|p| p.id == a.cue).map(|p| p.name.clone())).collect();
    let a = s.scripts.audio;
    json!({
        "tempo": {
            "bpm": tempo.bpm,
            "temps": tempo.beat_in_bar as u32 + 1,
            "mesure": tempo.bar,
            "phase": tempo.phase,
            "battement": tempo.beat,
        },
        "page": s.cue_page + 1,
        "cues": cues,
        "controles": controls,
        "timeline": { "position": s.timeline.position_at(&s.timeline_clock()), "lecture": s.timeline.is_playing() },
        "audio": { "niveau": a.level, "basses": a.bass, "mediums": a.mid, "aigus": a.high, "silence": a.silent },
    })
}

/// Posts a run of `id` (already checked). Never waits.
fn enqueue(s: &mut Shared, id: u32, state: &Value) {
    let Some(source) = s.scripts.get(id).map(|sc| sc.source.clone()) else { return };
    let sc = &mut s.scripts;
    sc.next_serial += 1;
    let serial = sc.next_serial;
    let cancel = Arc::new(AtomicBool::new(false));
    let job = Job { serial, source, state: state.clone(), seed: sc.seed ^ serial.wrapping_mul(0x9E37_79B9_7F4A_7C15), limits: sc.limits, cancel: Arc::clone(&cancel) };
    if sc.inline {
        sc.inline_done.push(execute(&job));
    } else {
        if sc.worker.is_none() {
            sc.worker = Worker::spawn();
        }
        let sent = sc.worker.as_ref().is_some_and(|w| w.tx.send(job).is_ok());
        if !sent {
            sc.worker = None;
            sc.status_mut(id).fail(RunState::Error, "le fil des scripts ne répond pas".into());
            return;
        }
    }
    sc.pending.push(Pending { serial, id, cancel });
    sc.status_mut(id).kind = RunState::Running;
}

/// Checked again before every run (the registry may have changed since
/// the save): a refused or broken line puts the script in error.
fn runnable(s: &mut Shared, id: u32) -> bool {
    let Some(sc) = s.scripts.get(id).cloned() else { return false };
    match check_one(&sc, &s.controls, &s.presets) {
        Ok(()) => true,
        Err(e) => {
            s.scripts.status_mut(id).fail(RunState::Error, e.to_string());
            false
        }
    }
}

/// A run on demand (button, `script.<id>.run`, a macro, a cue list).
/// Lifts the script's block: it is the operator's action.
pub fn request_run(s: &mut Shared, id: u32, source: ControlSource) -> Result<(), &'static str> {
    if source == ControlSource::Script {
        return Err("un script ne lance pas de script");
    }
    s.gate.sync_estop(&s.estop);
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : aucun script ne se lance avant la réinitialisation");
    }
    if s.scripts.get(id).is_none() {
        return Err("script inconnu");
    }
    if !s.scripts.trusted() {
        return Err("scripts de ce projet non approuvés : « Activer les scripts de ce projet »");
    }
    if s.scripts.is_running(id) {
        return Err("ce script tourne déjà");
    }
    if !runnable(s, id) {
        return Err("script en erreur : voir sa console");
    }
    s.scripts.status_mut(id).blocked = false;
    let state = snapshot(s);
    enqueue(s, id, &state);
    Ok(())
}

/// Stops one script: its run cancelled, its automatic trigger held until
/// the operator acts on it. False when nothing ran or was armed.
pub fn stop(s: &mut Shared, id: u32) -> bool {
    let sc = &mut s.scripts;
    let mut hit = false;
    sc.pending.retain(|p| {
        if p.id == id {
            p.cancel.store(true, Ordering::Relaxed);
            hit = true;
            false
        } else {
            true
        }
    });
    let auto = sc.get(id).is_some_and(|x| x.enabled && x.trigger != ScriptTrigger::Manual);
    if hit || auto {
        let st = sc.status_mut(id);
        st.kind = RunState::Stopped;
        st.blocked = auto;
        sc.stops += 1;
    }
    hit || auto
}

/// E-stop, disarm, « Tout arrêter »: every run cancelled, the automatic
/// triggers suspended until *Reprendre les scripts*.
pub fn stop_all(s: &mut Shared) {
    s.scripts.cancel_all();
    s.scripts.halted = true;
}

/// *Reprendre les scripts*: the automatic triggers on again (operator).
pub fn resume(s: &mut Shared) -> Result<(), &'static str> {
    s.gate.sync_estop(&s.estop);
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réinitialiser d'abord");
    }
    s.scripts.halted = false;
    Ok(())
}

/// A finished run, in the engine thread: its lines parsed on the registry
/// now (one bad line and none runs), then run one by one as « Script ».
fn apply(s: &mut Shared, o: Outcome) {
    let Some(at) = s.scripts.pending.iter().position(|p| p.serial == o.serial) else { return };
    let p = s.scripts.pending.remove(at);
    if p.cancel.load(Ordering::Relaxed) {
        return;
    }
    let id = p.id;
    {
        let st = s.scripts.status_mut(id);
        st.runs += 1;
        st.last_ms = o.ms;
        for l in &o.out.journal {
            st.log(l.clone());
        }
    }
    if let Err(e) = o.result {
        let st = s.scripts.status_mut(id);
        match e.kind {
            lang::ErrorKind::Budget => {
                st.fail(RunState::Budget, e.to_string());
                st.blocked = true;
            }
            lang::ErrorKind::Cancelled => st.kind = RunState::Stopped,
            _ => st.fail(RunState::Error, e.to_string()),
        }
        return;
    }
    let mut cmds = Vec::with_capacity(o.out.commands.len());
    for (line, text) in &o.out.commands {
        match crate::command::parse(text, &s.controls, &s.presets) {
            Ok(c) => cmds.push((*line, c)),
            Err(e) => {
                let why = if e.refused { format!("commande refusée « {} » : {}", text.trim(), e.message_fr.trim_start_matches("Commande refusée : ")) } else { format!("commande « {} » : {}", text.trim(), e.message_fr) };
                s.scripts.status_mut(id).fail(RunState::Error, format!("Ligne {line} : {why}"));
                return;
            }
        }
    }
    let armed = s.gate.is_armed();
    let stops = s.scripts.stops;
    for (line, cmd) in cmds {
        let r = crate::command::execute(s, &cmd, ControlSource::Script);
        if s.estop.is_latched() || (armed && !s.gate.is_armed()) {
            stop_all(s);
            return;
        }
        if let Err(e) = r {
            s.scripts.status_mut(id).fail(RunState::Error, format!("Ligne {line} : {}", e.message_fr));
            return;
        }
        if s.scripts.stops != stops {
            // « Tout arrêter » from this very run: nothing after it.
            return;
        }
    }
    let st = s.scripts.status_mut(id);
    st.kind = RunState::Ready;
    st.last_error = None;
}

/// One frame, in the engine thread before the deck: the stops, the runs
/// finished since the last frame, then the beat and bar triggers.
pub fn frame(s: &mut Shared, t: f64, audio: &crate::engine::AudioFeatures) {
    s.scripts.audio = AudioSeen { level: audio.level, bass: audio.bands.bass, mid: audio.bands.mid, high: audio.bands.high, silent: audio.silent };
    let armed = s.gate.is_armed();
    let disarmed = std::mem::replace(&mut s.scripts.was_armed, armed) && !armed;
    if s.estop.is_latched() || disarmed {
        if !s.scripts.pending.is_empty() || !s.scripts.halted {
            stop_all(s);
        }
        return;
    }
    let mut done = std::mem::take(&mut s.scripts.inline_done);
    if let Some(w) = &s.scripts.worker {
        done.extend(w.rx.try_iter());
    }
    for o in done {
        apply(s, o);
        if s.estop.is_latched() {
            stop_all(s);
            return;
        }
    }
    // The triggers: on the tempo clock, when the beat or the bar changes.
    let beat = s.tempo.beat_at(t).floor() as i64;
    let bar = beat.div_euclid(s.tempo.beats_per_bar.max(1) as i64);
    let new_beat = s.scripts.last_beat.is_some_and(|b| b != beat);
    let new_bar = s.scripts.last_bar.is_some_and(|b| b != bar);
    s.scripts.last_beat = Some(beat);
    s.scripts.last_bar = Some(bar);
    if !(new_beat || new_bar) || s.scripts.halted || !s.scripts.trusted() {
        return;
    }
    let due: Vec<u32> = s
        .scripts
        .list
        .iter()
        .filter(|sc| sc.enabled && ((new_beat && sc.trigger == ScriptTrigger::EveryBeat) || (new_bar && sc.trigger == ScriptTrigger::EveryBar)))
        .map(|sc| sc.id)
        .filter(|id| !s.scripts.is_running(*id) && !s.scripts.status.get(id).is_some_and(|st| st.blocked))
        .collect();
    let mut state = None;
    for id in due {
        if !runnable(s, id) {
            continue;
        }
        let st = state.get_or_insert_with(|| snapshot(s)).clone();
        enqueue(s, id, &st);
    }
}

// ---------- views ----------

pub fn statuses(s: &Shared) -> Vec<ScriptStatus> {
    s.scripts
        .list
        .iter()
        .map(|sc| {
            let st = s.scripts.status.get(&sc.id).cloned().unwrap_or_default();
            ScriptStatus { id: sc.id, state: st.kind.label_fr().to_string(), kind: st.kind, last_error: st.last_error, runs: st.runs, last_ms: st.last_ms, blocked: st.blocked, console: st.console.into_iter().collect() }
        })
        .collect()
}

/// « En cours » (T-423): the runs in flight and the automatic scripts armed.
pub fn running(s: &Shared) -> Vec<Value> {
    if s.estop.is_latched() {
        return Vec::new();
    }
    let sc = &s.scripts;
    sc.list
        .iter()
        .filter(|x| sc.is_running(x.id) || (x.enabled && x.trigger != ScriptTrigger::Manual && !sc.halted && sc.trusted() && !sc.status.get(&x.id).is_some_and(|st| st.blocked)))
        .map(|x| json!({ "id": x.id, "name": x.name, "trigger": x.trigger, "running": sc.is_running(x.id) }))
        .collect()
}

/// `GET /api/scripts`.
pub fn view(s: &Shared) -> Value {
    json!({
        "scripts": s.scripts.list,
        "status": statuses(s),
        "rev": s.scripts.rev,
        "trusted": s.scripts.trusted(),
        "ask_trust": s.scripts.ask_trust,
        "halted": s.scripts.halted,
        "warnings": s.scripts.warnings,
        "limits": { "scripts": MAX_SCRIPTS, "source_bytes": MAX_SOURCE, "ms": s.scripts.limits.max_ms, "ops": s.scripts.limits.max_ops, "commands": s.scripts.limits.max_commands, "memory_kb": s.scripts.limits.max_memory_kb },
        "functions": lang::Builtin::ALL.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "examples": EXAMPLES.iter().map(|(name, trigger, source)| json!({ "name": name, "trigger": trigger, "source": source })).collect::<Vec<_>>(),
    })
}

/// `/api/frame` › `scripts`: what the header and the tab need every frame.
pub fn live_view(s: &Shared) -> Value {
    let running: Vec<u32> = if s.estop.is_latched() { Vec::new() } else { s.scripts.running_ids() };
    json!({ "rev": s.scripts.rev, "running": running, "halted": s.scripts.halted, "ask_trust": s.scripts.ask_trust, "count": s.scripts.list.len() })
}

/// Our own examples (the help shows them too).
pub const EXAMPLES: [(&str, ScriptTrigger, &str); 3] = [
    (
        "Page selon le tempo",
        ScriptTrigger::EveryBar,
        "# Au-dessus de 140 BPM, page 3 ; sinon page 1.\nlet e = etat()\nif e.tempo.bpm > 140 {\n  if e.page != 3 { commande(\"page 3\") }\n} else if e.page != 1 {\n  commande(\"page 1\")\n}\n",
    ),
    (
        "Cue au hasard",
        ScriptTrigger::EveryBar,
        "# Toutes les quatre mesures, une case au hasard parmi les six premières de la page 1.\nlet e = etat()\nif e.tempo.mesure % 4 == 0 {\n  let n = hasard(6)\n  journal(\"case 1.\" + n)\n  commande(\"cue 1.\" + n + \" go\")\n}\n",
    ),
    (
        "Taille sur les basses",
        ScriptTrigger::EveryBeat,
        "# À chaque temps, la taille suit les basses (de 50 à 100 %).\nlet b = etat().audio.basses\ncommande(\"master.size \" + arrondi(50 + 50 * min(b, 1)))\n",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::ControlInput;
    use crate::interlock::ArmSource;
    use crate::test_support;

    fn sc(id: u32, name: &str, source: &str, trigger: ScriptTrigger) -> Script {
        Script { id, name: name.into(), source: source.into(), trigger, enabled: trigger != ScriptTrigger::Manual }
    }

    fn with(list: Vec<Script>) -> Shared {
        let mut s = test_support::shared();
        s.scripts.set_inline(true);
        s.scripts.limits.max_ms = 2_000;
        set(&mut s, list).expect("saved");
        s
    }

    fn tick(s: &mut Shared) {
        let audio = s.audio;
        let t = s.now_s();
        frame(s, t, &audio);
    }

    fn status(s: &Shared, id: u32) -> ScriptStatus {
        statuses(s).into_iter().find(|x| x.id == id).unwrap()
    }

    /// `master.size` as a command reads it: in per cent.
    fn size(s: &Shared) -> f32 {
        s.live.size * 100.0
    }

    #[test]
    fn a_run_reads_the_state_and_its_commands_apply_next_frame() {
        let mut s = with(vec![sc(1, "Page", "let e = etat()\njournal(e.page)\ncommande(\"page \" + (e.page + 1))", ScriptTrigger::Manual)]);
        let page = s.cue_page;
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        assert_eq!(s.cue_page, page, "nothing applied before the frame");
        tick(&mut s);
        assert_eq!(s.cue_page, page + 1);
        let st = status(&s, 1);
        assert_eq!(st.kind, RunState::Ready);
        assert_eq!(st.console, vec![(page + 1).to_string()]);
        assert_eq!(s.touches.get(&format!("page.{}", page + 2)).map(|t| t.source), Some(ControlSource::Script));
    }

    #[test]
    fn refused_lines_cannot_be_saved_and_never_arm() {
        let mut s = with(Vec::new());
        for line in ["armer", "transport.arm on", "safety.hold on", "estop.reset", "réinitialiser", "ARMER", "Réarmer"] {
            let r = set(&mut s, vec![sc(1, "X", &format!("commande(\"{line}\")"), ScriptTrigger::Manual)]);
            let e = r.expect_err(line);
            assert!(e.refused, "{line}: {e:?}");
            assert_eq!(e.line, Some(1));
        }
        // Built at run time: refused in the script's thread, nothing applied.
        set(&mut s, vec![sc(1, "X", "commande(\"tempo 99\")\ncommande(\"ar\" + \"mer\")", ScriptTrigger::Manual)]).unwrap();
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        tick(&mut s);
        let st = status(&s, 1);
        assert_eq!(st.kind, RunState::Error);
        assert_eq!(st.last_error.as_deref(), Some("Ligne 2 : commande refusée « armer » : le laser ne s'arme qu'à l'écran ou avec Espace"));
        assert_ne!(s.tempo.bpm, 99.0, "a failed run applies none of its lines");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn every_refused_control_of_the_registry_is_refused_from_a_script() {
        let s = test_support::shared();
        let mut checked = 0;
        for d in s.controls.list() {
            let refused = d.id == "transport.arm" || (d.group == "safety" && d.id != "safety.estop") || !d.external;
            if !refused {
                continue;
            }
            let script = sc(1, "X", &format!("commande(\"{} on\")", d.id), ScriptTrigger::Manual);
            let e = check_one(&script, &s.controls, &s.presets).expect_err(&d.id);
            assert_eq!(e.kind, lang::ErrorKind::Refused, "{}: {e}", d.id);
            // Even when the save is skipped, the engine refuses it.
            assert!(crate::command::parse(&format!("{} on", d.id), &s.controls, &s.presets).is_err());
            checked += 1;
        }
        assert!(checked >= 3, "{checked}");
        // The refused words, written out or built at run time: never saved,
        // never applied, never armed.
        let mut s = with(Vec::new());
        for line in ["armer", "réarmer", "allumer", "arm", "réinitialiser", "estop.reset", "safety.estop.reset", "tout armer", "master.size 80 arm", "safety.sky_off", "safety.hold on"] {
            let literal = sc(1, "X", &format!("commande(\"{line}\")"), ScriptTrigger::Manual);
            assert!(set(&mut s, vec![literal]).expect_err(line).refused, "{line}");
            let built = sc(1, "X", &format!("let l = \"\" + \"{line}\"\ncommande(l)"), ScriptTrigger::Manual);
            set(&mut s, vec![built]).unwrap();
            request_run(&mut s, 1, ControlSource::Ui).unwrap();
            tick(&mut s);
            let st = status(&s, 1);
            assert_eq!(st.kind, RunState::Error, "{line}");
            assert!(st.last_error.as_deref().is_some_and(|e| e.starts_with("Ligne 2 : commande refusée")), "{line}: {st:?}");
            assert!(!s.gate.is_armed());
        }
    }

    #[test]
    fn at_most_32_commands_per_run() {
        let mut s = with(vec![sc(1, "Tap", "for i in 0..50 { commande(\"master.size \" + i) }", ScriptTrigger::Manual)]);
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        tick(&mut s);
        assert!((size(&s) - 31.0).abs() < 1e-4, "{}", size(&s));
        assert!(status(&s, 1).console.iter().any(|l| l.contains("commande ignorée")));
    }

    #[test]
    fn a_looping_script_is_stopped_and_blocked_until_the_operator_acts() {
        let mut s = with(vec![sc(1, "Boucle", "commande(\"tempo 99\")\nwhile true {}", ScriptTrigger::EveryBeat), sc(2, "Bon", "commande(\"master.size 40\")", ScriptTrigger::EveryBeat)]);
        s.scripts.limits.max_ms = 5;
        // A beat passes.
        s.tempo.set_bpm_manual(120.0, s.now_s());
        let beat_s = 0.5;
        for _ in 0..2 {
            tick(&mut s);
            s.sim_now = Some(s.sim_now.unwrap_or(s.epoch) + std::time::Duration::from_secs_f64(beat_s));
            tick(&mut s);
        }
        let st = status(&s, 1);
        assert_eq!(st.kind, RunState::Budget, "{st:?}");
        assert_eq!(st.state, "arrêté : budget dépassé");
        assert!(st.blocked);
        assert_ne!(s.tempo.bpm, 99.0);
        // The other script ran.
        assert!((size(&s) - 40.0).abs() < 1e-4);
        let runs = st.runs;
        s.sim_now = Some(s.sim_now.unwrap() + std::time::Duration::from_secs_f64(beat_s));
        tick(&mut s);
        tick(&mut s);
        assert_eq!(status(&s, 1).runs, runs, "blocked: no automatic run");
        // The operator's run lifts the block.
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        assert!(!status(&s, 1).blocked);
    }

    #[test]
    fn every_bar_runs_once_per_bar_within_a_frame() {
        let mut s = with(vec![sc(1, "Mesure", "journal(etat().tempo.mesure)", ScriptTrigger::EveryBar)]);
        s.tempo.set_bpm_manual(120.0, s.now_s());
        let start = s.sim_now.unwrap_or(s.epoch);
        let frame = std::time::Duration::from_micros(16_667);
        let mut ran_at = Vec::new();
        let mut last_runs = 0;
        for f in 0..(60 * 9) {
            s.sim_now = Some(start + frame * f);
            tick(&mut s);
            let runs = status(&s, 1).runs;
            if runs != last_runs {
                ran_at.push(f);
                last_runs = runs;
            }
        }
        // 120 BPM, 4/4: a bar every 2 s = 120 frames.
        assert!(ran_at.len() >= 4, "{ran_at:?}");
        for w in ran_at.windows(2) {
            assert!((w[1] - w[0]).abs_diff(120) <= 1, "{ran_at:?}");
        }
        // Each run is the bar it landed on: consecutive bars, once each.
        let bars: Vec<i64> = status(&s, 1).console.iter().map(|l| l.parse().unwrap()).collect();
        for w in bars.windows(2) {
            assert_eq!(w[1], w[0] + 1, "{bars:?}");
        }
    }

    #[test]
    fn the_estop_a_disarm_and_stop_all_stop_scripts_and_nothing_resumes() {
        let mut s = with(vec![sc(1, "Temps", "commande(\"master.size 30\")", ScriptTrigger::EveryBeat)]);
        // A run in flight when the stop comes: its commands are dropped.
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        s.emergency_stop(ArmSource::Ui);
        tick(&mut s);
        assert!(s.scripts.pending.is_empty());
        assert!((size(&s) - 30.0).abs() > 1e-3);
        assert!(s.scripts.halted);
        assert_eq!(request_run(&mut s, 1, ControlSource::Ui).unwrap_err(), "arrêt d'urgence enclenché : aucun script ne se lance avant la réinitialisation");
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        // Reset: still halted, the beats don't run it.
        for _ in 0..3 {
            s.sim_now = Some(s.sim_now.unwrap_or(s.epoch) + std::time::Duration::from_secs_f64(0.6));
            tick(&mut s);
        }
        assert!((size(&s) - 30.0).abs() > 1e-3);
        resume(&mut s).unwrap();
        s.sim_now = Some(s.sim_now.unwrap() + std::time::Duration::from_secs_f64(0.6));
        tick(&mut s);
        tick(&mut s);
        assert!((size(&s) - 30.0).abs() < 1e-4);
        // A disarm halts them too.
        s.request_arm(ArmSource::Ui).unwrap();
        tick(&mut s);
        s.gate.disarm(crate::interlock::DisarmReason::User, ArmSource::Ui);
        tick(&mut s);
        assert!(s.scripts.halted);
        // « Tout arrêter » too.
        resume(&mut s).unwrap();
        controls::apply(&mut s, "running.stop_all", ControlInput::Value(1.0), false).unwrap();
        assert!(s.scripts.halted);
    }

    #[test]
    fn a_script_cannot_launch_a_script_and_a_control_can() {
        let mut s = with(vec![sc(1, "A", "commande(\"master.size 20\")", ScriptTrigger::Manual)]);
        assert!(set(&mut s, vec![sc(1, "A", "commande(\"script.1.run\")", ScriptTrigger::Manual)]).unwrap_err().refused);
        set(&mut s, vec![sc(1, "A", "commande(\"master.size 20\")", ScriptTrigger::Manual)]).unwrap();
        controls::apply(&mut s, "script.1.run", ControlInput::Value(1.0), true).unwrap();
        tick(&mut s);
        assert!((size(&s) - 20.0).abs() < 1e-4);
        assert_eq!(request_run(&mut s, 1, ControlSource::Script).unwrap_err(), "un script ne lance pas de script");
    }

    #[test]
    fn an_error_breaks_neither_other_scripts_nor_the_engine() {
        let mut s = with(vec![sc(1, "Casse", "let a = 1\n\njournal(b)", ScriptTrigger::Manual), sc(2, "Bon", "commande(\"master.size 70\")", ScriptTrigger::Manual)]);
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        request_run(&mut s, 2, ControlSource::Ui).unwrap();
        tick(&mut s);
        assert_eq!(status(&s, 1).last_error.as_deref(), Some("Ligne 3 : variable inconnue « b » (la déclarer avec let)"));
        assert!((size(&s) - 70.0).abs() < 1e-4);
    }

    #[test]
    fn a_project_list_runs_nothing_until_approved() {
        let dir = std::env::temp_dir().join(format!("laser-studio-scripts-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = test_support::shared();
        s.scripts = Scripts::load(&dir);
        s.scripts.set_inline(true);
        // Written here: approved as saved.
        set(&mut s, vec![sc(1, "A", "commande(\"master.size 20\")", ScriptTrigger::Manual)]).unwrap();
        assert!(s.scripts.trusted());
        // A project brings another list.
        s.scripts.replace_in_memory(vec![sc(1, "A", "commande(\"master.size 25\")", ScriptTrigger::EveryBeat)]);
        assert!(!s.scripts.trusted() && s.scripts.ask_trust);
        assert!(request_run(&mut s, 1, ControlSource::Ui).unwrap_err().contains("non approuvés"));
        // An edit here doesn't approve it either.
        set(&mut s, vec![sc(1, "A", "commande(\"master.size 26\")", ScriptTrigger::Manual)]).unwrap();
        assert!(!s.scripts.trusted());
        approve(&mut s, true);
        assert!(s.scripts.trusted() && !s.scripts.ask_trust);
        request_run(&mut s, 1, ControlSource::Ui).unwrap();
        // Remembered on this machine by fingerprint.
        let again = Scripts::load(&dir);
        assert!(again.trusted());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_engine_never_waits_for_a_looping_script() {
        let shared = std::sync::Mutex::new(test_support::shared());
        {
            let mut s = shared.lock().unwrap();
            s.scripts.limits.max_ms = 300;
            s.scripts.limits.max_ops = u64::MAX;
            set(&mut s, vec![sc(1, "Boucle", "while true {}", ScriptTrigger::Manual)]).unwrap();
            request_run(&mut s, 1, ControlSource::Ui).unwrap();
        }
        // Frames go on at once while the script spins in its own thread.
        let mut worst = std::time::Duration::ZERO;
        let start = Instant::now();
        while shared.lock().unwrap().scripts.is_running(1) {
            let t0 = Instant::now();
            tick(&mut shared.lock().unwrap());
            worst = worst.max(t0.elapsed());
            std::thread::sleep(std::time::Duration::from_millis(2));
            assert!(start.elapsed().as_secs() < 10, "the run never ended");
        }
        assert!(start.elapsed().as_millis() >= 250, "{:?}", start.elapsed());
        assert!(worst.as_millis() < 50, "a frame waited {worst:?}");
        assert_eq!(status(&shared.lock().unwrap(), 1).kind, RunState::Budget);
    }

    #[test]
    fn generated_scripts_never_arm() {
        let mut rng = crate::safety_invariants::Rng::new(458);
        let words = ["armer", "tempo 140", "page 2", "noir", "tout arrêter", "master.size 50", "transport.arm on", "safety.estop on", "tap", "réarmer", "estop.reset"];
        for case in 0..150 {
            let mut s = with(Vec::new());
            s.scripts.limits.max_ms = 50;
            if rng.chance(0.5) {
                s.request_arm(ArmSource::Ui).ok();
            }
            let mut src = String::new();
            for _ in 0..1 + rng.below(6) {
                let w = rng.pick(&words);
                src.push_str(&match rng.below(4) {
                    0 => format!("commande(\"{w}\")\n"),
                    1 => format!("let x = \"{w}\"\ncommande(x)\n"),
                    2 => format!("if hasard() < 0.5 {{ commande(\"{w}\") }}\n"),
                    _ => format!("for i in 0..{} {{ commande(\"{w}\") }}\n", rng.below(40)),
                });
            }
            let armed_before = s.gate.is_armed();
            let script = sc(1, "Fuzz", &src, ScriptTrigger::Manual);
            if set(&mut s, vec![script.clone()]).is_err() {
                // Saved anyway through a project: refused at run time.
                s.scripts.replace_in_memory(vec![script]);
                approve(&mut s, true);
            }
            let _ = request_run(&mut s, 1, ControlSource::Ui);
            for _ in 0..3 {
                tick(&mut s);
                assert!(!s.gate.is_armed() || armed_before, "case {case} armed: {src}");
            }
        }
    }

    #[test]
    fn old_lists_load_and_mangled_ones_are_repaired() {
        let mut list: Vec<Script> = serde_json::from_str(r#"[{"name":"A"},{"id":1,"name":"A","source":"x"},{"id":1}]"#).unwrap();
        sanitize(&mut list);
        let ids: Vec<u32> = list.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|&i| i > 0));
        assert_ne!(list[0].name, list[1].name);
        assert!(list.iter().all(|s| !s.enabled || s.name.is_empty()));
    }

    #[test]
    fn the_examples_are_valid() {
        let s = test_support::shared();
        for (name, trigger, source) in EXAMPLES {
            let script = Script { id: 1, name: name.into(), source: source.into(), trigger, enabled: true };
            check_one(&script, &s.controls, &s.presets).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
