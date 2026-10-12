//! Macros (T-425): a named list of command lines (T-424, `command.rs`),
//! each followed by a wait (none, seconds, beats or « attendre Go »),
//! launched from a key, a pad, an executor (T-418) or a cue list step.
//!
//! One language: every step is a command line, parsed by `command::parse`
//! when the macro is saved (and again when a project opens) and run by
//! `command::execute` with the source « Macro ». So a macro has a
//! controller's rights and nothing more: it never arms, never resets the
//! e-stop, never touches a safety setting (the parser's refusals, then the
//! registry's `external` flag again at run time).
//!
//! The player runs in the engine thread, under the `Shared` lock, before
//! the frame is rendered (`frame`): each due step is one command, no I/O.
//! Waits in beats land on the single tempo clock: the target is an
//! absolute beat, `round(start + n)` on the wait's grid (1 beat when
//! n ≥ 1, else n), so a step falls on the beat within a frame even when
//! the macro started between two beats or the tempo changes on the way.
//!
//! Bounds: `MAX_MACROS` macros of `MAX_STEPS` steps, `MAX_RUNNING` running
//! together, waits within `SECONDS` / `BEATS`, no cycle (refused when
//! saved) and at most `MAX_DEPTH` macros launching one another.
//!
//! Stops: the e-stop (and blackout, which is the e-stop), a disarm and
//! « Tout arrêter » end the running macros; no step runs after them.
//! Timed steps can't outrun the strobe limiter: it acts on the output,
//! after everything a macro can change.

use crate::command::{self, Command, CueAction};
use crate::controls::{ControlRegistry, ControlSource};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const MAX_MACROS: usize = 64;
pub const MAX_STEPS: usize = 64;
pub const MAX_RUNNING: usize = 8;
/// Macros launching macros: A → B → C → D at most.
pub const MAX_DEPTH: usize = 4;
/// Name length, in characters.
pub const MAX_NAME: usize = 32;
const MAX_ID: usize = 16;
pub const SECONDS: (f32, f32) = (0.05, 600.0);
pub const BEATS: (f32, f32) = (0.125, 256.0);
/// Steps run in one frame, all macros together (a constant bound under
/// the lock).
const STEPS_PER_FRAME: usize = MAX_RUNNING * MAX_STEPS;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Macro {
    /// Stable, "m1", "m2"…: the control `macro.<id>.go`.
    pub id: String,
    /// 1 to 32 characters, unique.
    pub name: String,
    pub steps: Vec<MacroStep>,
    /// A letter A–Z: Alt+letter launches it.
    pub key: Option<String>,
    pub enabled: bool,
}

impl Default for Macro {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), steps: Vec::new(), key: None, enabled: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MacroStep {
    /// A command line (T-424); empty = only the wait.
    pub command: String,
    pub wait: Wait,
    pub enabled: bool,
}

impl Default for MacroStep {
    fn default() -> Self {
        Self { command: String::new(), wait: Wait::None, enabled: true }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    #[default]
    None,
    /// 0.05..=600 s.
    Seconds(f32),
    /// 0.125..=256 beats.
    Beats(f32),
    /// Until `macro.go`.
    Go,
}

/// When a run's next step is due.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Due {
    Now,
    AtSecond(f64),
    AtBeat(f64),
    OnGo,
}

#[derive(Clone, Debug, Serialize)]
pub struct MacroRun {
    pub macro_id: String,
    /// The next step to run (0-based).
    pub step: usize,
    pub due: Due,
    pub started_s: f64,
    /// Unique per launch: a restart is a new run.
    pub serial: u64,
    /// 1 for a macro launched from outside, +1 per macro launching it.
    pub depth: usize,
    pub source: ControlSource,
    /// Started by a script, or by a macro a script started: its steps may
    /// not launch a script (T-458: no script → macro → script loop, which
    /// would be an every-frame trigger).
    #[serde(skip)]
    pub from_script: bool,
    /// The last step that failed (a cue deleted since…), in French.
    pub last_error: Option<String>,
}

/// What runs now. Memory only: every start with nothing running.
#[derive(Debug, Default)]
pub struct MacroPlayer {
    pub runs: Vec<MacroRun>,
    next_serial: u64,
    was_armed: bool,
    /// The run whose step is executing (a macro launched from it is one
    /// level deeper; « Tout arrêter » from it keeps it), its depth and
    /// whether a script is at its origin.
    current: Option<(u64, usize, bool)>,
}

impl MacroPlayer {
    /// A step is executing in a run a script started (directly or through
    /// other macros): `script::request_run` refuses to launch from it.
    pub fn in_script_run(&self) -> bool {
        self.current.is_some_and(|(_, _, from_script)| from_script)
    }

    /// Every run ends.
    pub fn stop_all(&mut self) {
        self.runs.clear();
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.runs.iter().any(|r| r.macro_id == id)
    }
}

/// The library (`macros.json`, the project's `macros`) and the player.
#[derive(Debug, Default)]
pub struct Macros {
    list: Vec<Macro>,
    path: Option<PathBuf>,
    pub player: MacroPlayer,
    /// Bumped on every change of the list, so the page reloads it.
    pub rev: u64,
    /// What loading found wrong (macros disabled), for the page and the
    /// project's open report; taken by whoever reports it.
    pub warnings: Vec<String>,
}

impl Macros {
    /// `macros.json` (sanitised; lines are checked by `revalidate` once the
    /// control registry exists).
    pub fn load(path: PathBuf) -> Self {
        let mut list: Vec<Macro> = crate::load_json(&path);
        sanitize(&mut list);
        Self { list, path: Some(path), ..Self::default() }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn list(&self) -> &[Macro] {
        &self.list
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self, id: &str) -> Option<&Macro> {
        self.list.iter().find(|m| m.id == id)
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.list);
        }
    }

    /// A list already checked (or from a project, revalidated after).
    /// Runs of macros that are gone or disabled end.
    pub fn replace_in_memory(&mut self, mut list: Vec<Macro>) {
        sanitize(&mut list);
        self.list = list;
        let list = &self.list;
        self.player.runs.retain(|r| list.iter().any(|m| m.id == r.macro_id && m.enabled));
        self.rev += 1;
    }
}

// ---------- checks ----------

/// Why a list can't be saved: which macro and step (1-based), in French.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MacroError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macro_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    pub message_fr: String,
    /// A safety refusal (arming, reset, safety settings), not a typo.
    pub refused: bool,
}

impl MacroError {
    fn list(message_fr: String) -> Self {
        Self { macro_id: None, step: None, message_fr, refused: false }
    }

    fn of(m: &Macro, message_fr: String) -> Self {
        Self { macro_id: Some(m.id.clone()), step: None, message_fr: format!("Macro « {} » : {message_fr}", m.name), refused: false }
    }
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message_fr)
    }
}

fn clip(text: &str, n: usize) -> String {
    text.trim().chars().take(n).collect()
}

/// `m` and a number: the control id `macro.<id>.go` can never hold a
/// word the command line refuses.
fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID && id.strip_prefix('m').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The key as stored: one upper-case letter A–Z, or none.
fn norm_key(key: &Option<String>) -> Result<Option<String>, ()> {
    match key.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(k) if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphabetic()) => Ok(Some(k.to_ascii_uppercase())),
        Some(_) => Err(()),
    }
}

fn wait_ok(w: Wait) -> bool {
    match w {
        Wait::None | Wait::Go => true,
        Wait::Seconds(s) => s.is_finite() && (SECONDS.0..=SECONDS.1).contains(&s),
        Wait::Beats(b) => b.is_finite() && (BEATS.0..=BEATS.1).contains(&b),
    }
}

/// A file or a project, repaired rather than refused: the counts cut,
/// names and keys clipped, ids made valid and unique, waits clamped.
/// Lines are not checked here (`revalidate`).
pub fn sanitize(list: &mut Vec<Macro>) {
    list.truncate(MAX_MACROS);
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut keys = HashSet::new();
    let taken: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
    let mut n = 0;
    for m in list.iter_mut() {
        if !valid_id(&m.id) || !ids.insert(m.id.clone()) {
            m.id = loop {
                n += 1;
                let id = format!("m{n}");
                if !taken.contains(&id) && !ids.contains(&id) {
                    break id;
                }
            };
            ids.insert(m.id.clone());
        }
        m.name = clip(&m.name, MAX_NAME);
        if m.name.is_empty() || !names.insert(command::fold(&m.name)) {
            let base = if m.name.is_empty() { "Macro".to_string() } else { clip(&m.name, MAX_NAME - 4) };
            m.name = (2..).map(|i| format!("{base} {i}")).find(|c| !names.contains(&command::fold(c))).expect("a free name");
            names.insert(command::fold(&m.name));
        }
        m.key = norm_key(&m.key).ok().flatten().filter(|k| keys.insert(k.clone()));
        m.steps.truncate(MAX_STEPS);
        for st in &mut m.steps {
            st.command = st.command.chars().take(command::MAX_LINE).collect();
            st.wait = match st.wait {
                Wait::Seconds(s) if s.is_finite() => Wait::Seconds(s.clamp(SECONDS.0, SECONDS.1)),
                Wait::Beats(b) if b.is_finite() => Wait::Beats(b.clamp(BEATS.0, BEATS.1)),
                Wait::Seconds(_) | Wait::Beats(_) => Wait::None,
                w => w,
            };
        }
    }
}

/// The macro a parsed line launches, if it does.
fn launches(cmd: &Command) -> Option<&str> {
    match cmd {
        Command::Macro { id, action: CueAction::Go } => Some(id),
        Command::Set { id, .. } => id.strip_prefix("macro.").and_then(|r| r.strip_suffix(".go")),
        _ => None,
    }
}

/// `reg` with the macros of `list` (the ones being saved) in it.
fn registry_for(s: &Shared, list: &[Macro]) -> ControlRegistry {
    let mut reg = ControlRegistry::build_for(&s.presets, &s.grid);
    reg.add_macros(list);
    reg.add_scripts(s.scripts.list());
    reg
}

/// One step's line: parsed, refusals and unknown words as errors. Empty
/// lines are fine (a wait only).
fn check_line(m: &Macro, i: usize, line: &str, reg: &ControlRegistry, presets: &[crate::presets::Preset]) -> Result<Option<Command>, MacroError> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    command::parse(line, reg, presets).map(Some).map_err(|e| MacroError {
        macro_id: Some(m.id.clone()),
        step: Some(i + 1),
        message_fr: format!("Macro « {} », étape {} : {}", m.name, i + 1, e.message_fr),
        refused: e.refused,
    })
}

/// `POST /api/macros`: the whole list, refused rather than repaired.
/// Every line parses (none refused), names and keys unique, waits in
/// range, no cycle and no chain deeper than `MAX_DEPTH`. Ids left empty
/// get the next free `mN`. A disabled macro may keep lines that don't
/// parse any more (a cue deleted since), never a refused one.
pub fn validate(s: &Shared, list: &mut [Macro]) -> Result<(), MacroError> {
    if list.len() > MAX_MACROS {
        return Err(MacroError::list(format!("{MAX_MACROS} macros au plus")));
    }
    let given: HashSet<String> = list.iter().map(|m| m.id.clone()).collect();
    let mut n = 0;
    for m in list.iter_mut() {
        if m.id.trim().is_empty() {
            m.id = loop {
                n += 1;
                let id = format!("m{n}");
                if !given.contains(&id) {
                    break id;
                }
            };
        }
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut keys = HashSet::new();
    for m in list.iter_mut() {
        if !valid_id(&m.id) || !ids.insert(m.id.clone()) {
            return Err(MacroError::list(format!("identifiant de macro invalide ou en double : « {} »", m.id)));
        }
        m.name = m.name.trim().to_string();
        let len = m.name.chars().count();
        if len == 0 || len > MAX_NAME {
            return Err(MacroError::of(m, format!("nom de 1 à {MAX_NAME} caractères")));
        }
        if !names.insert(command::fold(&m.name)) {
            return Err(MacroError::of(m, "nom déjà pris".into()));
        }
        m.key = norm_key(&m.key).map_err(|_| MacroError::of(m, "touche invalide (une lettre de A à Z)".into()))?;
        if let Some(k) = &m.key {
            if !keys.insert(k.clone()) {
                return Err(MacroError::of(m, format!("la touche {k} est déjà prise")));
            }
        }
        if m.steps.len() > MAX_STEPS {
            return Err(MacroError::of(m, format!("{MAX_STEPS} étapes au plus")));
        }
        for (i, st) in m.steps.iter().enumerate() {
            if !wait_ok(st.wait) {
                let msg = format!("Macro « {} », étape {} : attente hors limites (0,05 à 600 s, 0,125 à 256 temps)", m.name, i + 1);
                return Err(MacroError { macro_id: Some(m.id.clone()), step: Some(i + 1), message_fr: msg, refused: false });
            }
        }
    }
    let reg = registry_for(s, list);
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for m in list.iter() {
        for (i, st) in m.steps.iter().enumerate() {
            match check_line(m, i, &st.command, &reg, &s.presets) {
                Ok(Some(cmd)) => {
                    if let Some(to) = launches(&cmd) {
                        edges.entry(m.id.clone()).or_default().push(to.to_string());
                    }
                }
                Ok(None) => {}
                Err(e) if e.refused || m.enabled => return Err(e),
                Err(_) => {}
            }
        }
    }
    check_graph(list, &edges)
}

/// No cycle (A → B → A, or A → A) and no chain longer than `MAX_DEPTH`.
fn check_graph(list: &[Macro], edges: &HashMap<String, Vec<String>>) -> Result<(), MacroError> {
    let name = |id: &str| list.iter().find(|m| m.id == id).map_or(id.to_string(), |m| m.name.clone());
    // Depth-first from every macro, the path kept for the message.
    fn walk(id: &str, path: &mut Vec<String>, edges: &HashMap<String, Vec<String>>) -> Result<usize, Vec<String>> {
        if let Some(at) = path.iter().position(|p| p == id) {
            let mut cycle = path[at..].to_vec();
            cycle.push(id.to_string());
            return Err(cycle);
        }
        path.push(id.to_string());
        let mut deepest = 0;
        for to in edges.get(id).map(Vec::as_slice).unwrap_or(&[]) {
            deepest = deepest.max(walk(to, path, edges)?);
        }
        path.pop();
        Ok(deepest + 1)
    }
    for m in list {
        match walk(&m.id, &mut Vec::new(), edges) {
            Err(cycle) => {
                let names: Vec<String> = cycle.iter().map(|id| format!("« {} »", name(id))).collect();
                let message_fr = if cycle.len() == 2 {
                    format!("Cycle refusé : la macro {} se lance elle-même", names[0])
                } else {
                    format!("Cycle refusé : {}", names.join(" lance "))
                };
                return Err(MacroError { macro_id: Some(m.id.clone()), step: None, message_fr, refused: false });
            }
            Ok(depth) if depth > MAX_DEPTH => {
                return Err(MacroError::of(m, format!("trop de macros imbriquées ({MAX_DEPTH} niveaux au plus)")));
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// After a load (startup, project): every line checked against the
/// registry as it is now. A macro with a bad line is disabled, a refused
/// line emptied (never kept, even disabled), a cycle disabled too; one
/// French warning each, kept in `warnings`. Returns them as well.
pub fn revalidate(s: &mut Shared) -> Vec<String> {
    let mut list = s.macros.list.clone();
    let mut warnings = Vec::new();
    {
        let reg = &s.controls;
        for m in list.iter_mut() {
            let mut bad: Option<String> = None;
            for i in 0..m.steps.len() {
                if let Err(e) = check_line(m, i, &m.steps[i].command.clone(), reg, &s.presets) {
                    if e.refused {
                        m.steps[i].command.clear();
                        m.steps[i].enabled = false;
                    }
                    bad.get_or_insert(e.message_fr);
                }
            }
            if let Some(why) = bad {
                if m.enabled {
                    m.enabled = false;
                }
                warnings.push(format!("{why} : macro désactivée"));
            }
        }
    }
    // Cycles and depth: disable the macros involved until none is left.
    loop {
        let reg = registry_for(s, &list);
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        for m in list.iter().filter(|m| m.enabled) {
            for st in &m.steps {
                if let Ok(cmd) = command::parse(&st.command, &reg, &s.presets) {
                    if let Some(to) = launches(&cmd) {
                        edges.entry(m.id.clone()).or_default().push(to.to_string());
                    }
                }
            }
        }
        match check_graph(&list, &edges) {
            Ok(()) => break,
            Err(e) => {
                let Some(m) = e.macro_id.as_ref().and_then(|id| list.iter_mut().find(|m| &m.id == id && m.enabled)) else { break };
                m.enabled = false;
                warnings.push(format!("{} : macro « {} » désactivée", e.message_fr, m.name));
            }
        }
    }
    if list != s.macros.list {
        s.macros.list = list;
        s.macros.rev += 1;
        let macros = &s.macros.list;
        s.macros.player.runs.retain(|r| macros.iter().any(|m| m.id == r.macro_id && m.enabled));
    }
    for w in &warnings {
        log::warn!("{w}");
    }
    s.macros.warnings = warnings.clone();
    warnings
}

/// The control registry follows the list (`macro.<id>.go`), like the
/// figures do.
pub fn refresh(s: &mut Shared) {
    crate::grid::refresh(s);
}

/// `POST /api/macros` (and an undo): checked, swapped in, saved, the
/// registry rebuilt.
pub fn set(s: &mut Shared, mut list: Vec<Macro>) -> Result<(), MacroError> {
    validate(s, &mut list)?;
    s.macros.replace_in_memory(list);
    s.macros.warnings.clear();
    s.macros.save();
    refresh(s);
    Ok(())
}

// ---------- the player ----------

/// Why a launch is refused (controls need a static text).
pub fn start(s: &mut Shared, id: &str, source: ControlSource) -> Result<(), &'static str> {
    s.gate.sync_estop(&s.estop);
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : aucune macro ne se lance avant la réinitialisation");
    }
    let Some(m) = s.macros.get(id) else { return Err("macro inconnue") };
    if !m.enabled {
        return Err("macro désactivée");
    }
    let depth = s.macros.player.current.map_or(1, |(_, d, _)| d + 1);
    // A script, a macro it started or a cue list it moved: see `script_origin`.
    let from_script = crate::script::script_origin(s, source);
    if depth > MAX_DEPTH {
        return Err("trop de macros imbriquées");
    }
    let now = s.now_s();
    let p = &mut s.macros.player;
    p.next_serial += 1;
    let run = MacroRun { macro_id: id.to_string(), step: 0, due: Due::Now, started_s: now, serial: p.next_serial, depth, source, from_script, last_error: None };
    let full = p.runs.len() >= MAX_RUNNING;
    match p.runs.iter_mut().find(|r| r.macro_id == id) {
        // Launched again: back to its first step.
        Some(r) => *r = run,
        None if full => return Err("8 macros au plus en même temps"),
        None => p.runs.push(run),
    }
    Ok(())
}

/// Stops one macro; false when it wasn't running.
pub fn stop(s: &mut Shared, id: &str) -> bool {
    let p = &mut s.macros.player;
    let before = p.runs.len();
    p.runs.retain(|r| r.macro_id != id);
    p.runs.len() != before
}

/// « Arrêter les macros », « Tout arrêter »: every run ends, except the
/// one whose step asks it (it goes on with its next step).
pub fn stop_all(s: &mut Shared) {
    let p = &mut s.macros.player;
    match p.current {
        Some((serial, _, _)) => p.runs.retain(|r| r.serial == serial),
        None => p.runs.clear(),
    }
}

/// `macro.go`: every run waiting for Go goes on. Returns how many.
pub fn go(s: &mut Shared) -> usize {
    let mut n = 0;
    for r in &mut s.macros.player.runs {
        if r.due == Due::OnGo {
            r.due = Due::Now;
            n += 1;
        }
    }
    n
}

/// When a step's wait ends, from `now_s` / `beat` (the moment it ran).
pub fn next_due(wait: Wait, now_s: f64, beat: f64) -> Due {
    match wait {
        Wait::None => Due::Now,
        Wait::Seconds(n) => Due::AtSecond(now_s + n as f64),
        Wait::Beats(n) => {
            let n = n as f64;
            let grid = if n >= 1.0 { 1.0 } else { n };
            Due::AtBeat(((beat + n) / grid).round() * grid)
        }
        Wait::Go => Due::OnGo,
    }
}

fn is_due(due: Due, now_s: f64, beat: f64) -> bool {
    match due {
        Due::Now => true,
        Due::AtSecond(t) => now_s >= t - 1e-9,
        Due::AtBeat(b) => beat >= b - 1e-9,
        Due::OnGo => false,
    }
}

/// One frame of the player, in the engine thread before rendering: every
/// step due runs, as the source « Macro ». The e-stop (latched now or by
/// a step) and a disarm end every run; nothing runs past them.
pub fn frame(s: &mut Shared, now_s: f64, beat: f64) {
    let armed = s.gate.is_armed();
    let disarmed = std::mem::replace(&mut s.macros.player.was_armed, armed) && !armed;
    if s.estop.is_latched() || disarmed {
        s.macros.player.stop_all();
        return;
    }
    let mut budget = STEPS_PER_FRAME;
    let mut i = 0;
    while i < s.macros.player.runs.len() && budget > 0 {
        let serial = s.macros.player.runs[i].serial;
        while let Some(idx) = s.macros.player.runs.iter().position(|r| r.serial == serial) {
            let run = &s.macros.player.runs[idx];
            if !is_due(run.due, now_s, beat) {
                break;
            }
            let Some(step) = s.macros.get(&run.macro_id).filter(|m| m.enabled).map(|m| m.steps.get(run.step).cloned()) else {
                s.macros.player.runs.remove(idx);
                break;
            };
            let Some(step) = step else {
                // Past its last step and its last wait: done.
                s.macros.player.runs.remove(idx);
                break;
            };
            let (depth, from_script) = (run.depth, run.from_script);
            let run = &mut s.macros.player.runs[idx];
            run.step += 1;
            if !step.enabled {
                run.due = Due::Now;
                continue;
            }
            run.due = next_due(step.wait, now_s, beat);
            if step.command.trim().is_empty() {
                continue;
            }
            budget -= 1;
            s.macros.player.current = Some((serial, depth, from_script));
            let r = run_line(s, &step.command);
            s.macros.player.current = None;
            if s.estop.is_latched() || (armed && !s.gate.is_armed()) {
                s.macros.player.stop_all();
                return;
            }
            if let Err(e) = r {
                if let Some(run) = s.macros.player.runs.iter_mut().find(|r| r.serial == serial) {
                    run.last_error = Some(e);
                }
            }
            if budget == 0 {
                break;
            }
        }
        i = s.macros.player.runs.iter().position(|r| r.serial == serial).map_or(i, |p| p + 1);
    }
}

/// One step's line, parsed against the registry as it is now.
fn run_line(s: &mut Shared, line: &str) -> Result<(), String> {
    let cmd = command::parse(line, &s.controls, &s.presets).map_err(|e| e.message_fr)?;
    command::execute(s, &cmd, ControlSource::Macro).map(|_| ()).map_err(|e| e.message_fr)
}

// ---------- views ----------

fn wait_label(due: Due, now_s: f64, beat: f64) -> Value {
    match due {
        Due::Now => Value::Null,
        Due::AtSecond(t) => json!({ "kind": "seconds", "left": ((t - now_s).max(0.0) * 100.0).round() / 100.0 }),
        Due::AtBeat(b) => json!({ "kind": "beats", "left": ((b - beat).max(0.0) * 100.0).round() / 100.0 }),
        Due::OnGo => json!({ "kind": "go" }),
    }
}

/// The running macros: id, name, the step reached (1-based, of `steps`),
/// what it waits for, who launched it.
pub fn running(s: &Shared) -> Vec<Value> {
    let now = s.now_s();
    let beat = s.tempo.beat_at(now);
    shown_runs(s)
        .iter()
        .map(|r| {
            let m = s.macros.get(&r.macro_id);
            json!({
                "id": r.macro_id,
                "name": m.map_or(r.macro_id.as_str(), |m| m.name.as_str()),
                "step": r.step,
                "steps": m.map_or(0, |m| m.steps.len()),
                "waiting": wait_label(r.due, now, beat),
                "source": r.source,
                "source_label": r.source.label_fr(),
                "error": r.last_error,
            })
        })
        .collect()
}

/// The runs the views report (T-480). With the e-stop latched, none:
/// no step runs while it is (`frame` ends every run first), but the
/// HTTP fast path leaves the clearing to the engine's next frame when the
/// lock is busy, and a view read in between must not show a macro still
/// running next to `estop: true`.
fn shown_runs(s: &Shared) -> &[MacroRun] {
    if s.estop.is_latched() {
        &[]
    } else {
        &s.macros.player.runs
    }
}

/// `GET /api/macros`.
pub fn view(s: &Shared) -> Value {
    json!({
        "macros": s.macros.list,
        "running": running(s),
        "rev": s.macros.rev,
        "warnings": s.macros.warnings,
        "limits": { "macros": MAX_MACROS, "steps": MAX_STEPS, "running": MAX_RUNNING, "depth": MAX_DEPTH, "seconds": [0.05, 600.0], "beats": [0.125, 256.0] },
    })
}

/// `/api/frame` › `macros`: what the LIVE buttons need every frame.
pub fn live_view(s: &Shared) -> Value {
    let runs: Vec<Value> = shown_runs(s)
        .iter()
        .map(|r| json!({ "id": r.macro_id, "step": r.step, "go": r.due == Due::OnGo, "error": r.last_error.is_some() }))
        .collect();
    json!({ "rev": s.macros.rev, "running": runs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{self, ControlInput};
    use crate::interlock::ArmSource;
    use crate::test_support;

    fn step(command: &str, wait: Wait) -> MacroStep {
        MacroStep { command: command.into(), wait, enabled: true }
    }

    fn mac(id: &str, name: &str, steps: Vec<MacroStep>) -> Macro {
        Macro { id: id.into(), name: name.into(), steps, ..Default::default() }
    }

    fn with(list: Vec<Macro>) -> Shared {
        let mut s = test_support::shared();
        set(&mut s, list).unwrap_or_else(|e| panic!("{e:?}"));
        s
    }

    fn size(s: &Shared) -> f32 {
        (s.live.size * 100.0).round()
    }

    #[test]
    fn a_macros_tempo_is_never_proposed_to_ableton_link() {
        use crate::ableton_link::fake::{on, proposals, run, Fake};
        let mut s = with(vec![mac("m1", "Tempo", vec![step("tempo.bpm 95", Wait::None), step("tempo.double", Wait::None)])]);
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        let mut now = s.epoch + std::time::Duration::from_secs(2);
        run(&mut s, &fake, &mut now, 2);
        // Started by the operator, but its steps are the macro's.
        start(&mut s, "m1", ControlSource::Keyboard).unwrap();
        frame(&mut s, 0.0, 0.0);
        assert!(s.macros.player.runs.is_empty());
        run(&mut s, &fake, &mut now, 3);
        assert!(proposals(&fake).is_empty(), "{:?}", proposals(&fake));
        assert!((s.tempo.bpm - 120.0).abs() <= 0.01, "the session wins");
    }

    #[test]
    fn steps_without_waits_run_in_one_frame_in_order() {
        let mut s = with(vec![mac("m1", "Deux", vec![step("master.size 50", Wait::None), step("", Wait::None), step("master.size 70", Wait::None)])]);
        start(&mut s, "m1", ControlSource::Keyboard).unwrap();
        // Nothing before the engine's frame.
        assert_eq!(size(&s), 100.0);
        frame(&mut s, 0.0, 0.0);
        assert_eq!(size(&s), 70.0);
        assert!(s.macros.player.runs.is_empty(), "done after its last step");
        assert_eq!(s.touches["master.size"].source, ControlSource::Macro);
    }

    #[test]
    fn waits_in_seconds() {
        let mut s = with(vec![mac("m1", "S", vec![step("master.size 50", Wait::Seconds(2.0)), step("master.size 60", Wait::None)])]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 10.0, 0.0);
        assert_eq!(size(&s), 50.0);
        frame(&mut s, 11.98, 0.0);
        assert_eq!(size(&s), 50.0);
        frame(&mut s, 12.0, 0.0);
        assert_eq!(size(&s), 60.0);
    }

    /// The engine at 60 fps on a tempo clock: the step lands on the beat
    /// within a frame, launched off the beat (next beat of the grid) and
    /// across a tempo change during the wait.
    #[test]
    fn waits_in_beats_land_on_the_grid_within_a_frame() {
        let mut s = with(vec![mac("m1", "B", vec![step("master.size 50", Wait::Beats(1.0)), step("master.size 100", Wait::Beats(2.0)), step("master.size 30", Wait::None)])]);
        s.tempo.set_bpm_manual(120.0, 0.0);
        let dt = 1.0 / 60.0;
        // Launched half a beat after a beat: rounds to the beat after next
        // grid point (beat + 1, rounded half up).
        let mut t = 0.0;
        while s.tempo.beat_at(t) < 3.5 {
            t += dt;
        }
        let b0 = s.tempo.beat_at(t);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        let mut fired = Vec::new();
        let mut last = size(&s);
        for i in 0..600 {
            if i == 60 {
                // The tempo changes while it waits.
                s.tempo.set_bpm_manual(90.0, t);
            }
            let beat = s.tempo.beat_at(t);
            frame(&mut s, t, beat);
            if size(&s) != last {
                last = size(&s);
                fired.push((last, beat));
            }
            t += dt;
        }
        assert_eq!(fired.len(), 3, "{fired:?}");
        assert!((fired[0].1 - b0).abs() < 0.05);
        let target1 = (b0 + 1.0).round();
        assert_eq!(target1, 5.0, "off-beat launch at {b0}: next grid beat");
        let frame_beats = |bpm: f64| bpm / 60.0 * dt;
        assert!(fired[1].1 >= target1 - 1e-6 && fired[1].1 - target1 <= frame_beats(120.0) + 1e-9, "{fired:?}");
        let target2 = (fired[1].1 + 2.0).round();
        assert!(fired[2].1 >= target2 - 1e-6 && fired[2].1 - target2 <= frame_beats(120.0) + 1e-9, "{fired:?}");
    }

    #[test]
    fn next_due_rounds_on_the_wait_grid() {
        assert_eq!(next_due(Wait::Beats(1.0), 0.0, 10.5), Due::AtBeat(12.0));
        assert_eq!(next_due(Wait::Beats(1.0), 0.0, 10.02), Due::AtBeat(11.0));
        assert_eq!(next_due(Wait::Beats(4.0), 0.0, 10.98), Due::AtBeat(15.0));
        assert_eq!(next_due(Wait::Beats(0.5), 0.0, 10.3), Due::AtBeat(11.0));
        assert_eq!(next_due(Wait::Beats(0.25), 0.0, 10.01), Due::AtBeat(10.25));
        assert_eq!(next_due(Wait::Seconds(1.5), 2.0, 0.0), Due::AtSecond(3.5));
        assert_eq!(next_due(Wait::Go, 0.0, 0.0), Due::OnGo);
        // Always strictly ahead of where it started.
        for b in [0.0, 0.49, 0.5, 0.51, 7.999] {
            for n in [0.125, 0.3, 0.5, 1.0, 1.5, 3.0] {
                let Due::AtBeat(at) = next_due(Wait::Beats(n), 0.0, b) else { unreachable!() };
                assert!(at > b, "{n} from {b}: {at}");
            }
        }
    }

    #[test]
    fn wait_for_go_holds_until_macro_go() {
        let mut s = with(vec![mac("m1", "G", vec![step("master.size 50", Wait::Go), step("master.size 80", Wait::None)])]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        for i in 0..100 {
            frame(&mut s, i as f64, i as f64);
        }
        assert_eq!(size(&s), 50.0);
        assert_eq!(live_view(&s)["running"][0]["go"], true);
        controls::apply(&mut s, "macro.go", ControlInput::Value(1.0), true).unwrap();
        frame(&mut s, 200.0, 200.0);
        assert_eq!(size(&s), 80.0);
        assert!(s.macros.player.runs.is_empty());
        // Go with nothing waiting does nothing.
        assert_eq!(go(&mut s), 0);
    }

    #[test]
    fn a_second_launch_restarts_and_eight_run_at_most() {
        let list: Vec<Macro> = (1..=9).map(|i| mac(&format!("m{i}"), &format!("M{i}"), vec![step("master.size 50", Wait::Go), step("master.size 80", Wait::None)])).collect();
        let mut s = with(list);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        go(&mut s);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        assert_eq!(s.macros.player.runs.len(), 1);
        assert_eq!(s.macros.player.runs[0].step, 0, "back to its first step");
        for i in 2..=8 {
            start(&mut s, &format!("m{i}"), ControlSource::Ui).unwrap();
        }
        assert_eq!(start(&mut s, "m9", ControlSource::Ui), Err("8 macros au plus en même temps"));
        assert_eq!(s.macros.player.runs.len(), MAX_RUNNING);
        // A restart is not a ninth.
        start(&mut s, "m3", ControlSource::Ui).unwrap();
        assert!(stop(&mut s, "m3"));
        start(&mut s, "m9", ControlSource::Ui).unwrap();
        controls::apply(&mut s, "macro.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert!(s.macros.player.runs.is_empty());
    }

    #[test]
    fn a_macro_launches_another_and_cycles_are_refused() {
        let a = mac("m1", "Alpha", vec![step("macro \"Bêta\" go", Wait::None)]);
        let b = mac("m2", "Bêta", vec![step("master.size 40", Wait::None)]);
        let mut s = with(vec![a.clone(), b.clone()]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        assert_eq!(size(&s), 40.0, "B ran in the same frame");
        // B launching A back: refused when saved, naming both.
        let b2 = mac("m2", "Bêta", vec![step("macro.m1.go", Wait::None)]);
        let e = set(&mut s, vec![a.clone(), b2]).unwrap_err();
        assert!(e.message_fr.starts_with("Cycle refusé : « Alpha » lance « Bêta » lance « Alpha »"), "{e:?}");
        let selfish = mac("m1", "Alpha", vec![step("macro alpha go", Wait::None)]);
        let e = set(&mut s, vec![selfish]).unwrap_err();
        assert_eq!(e.message_fr, "Cycle refusé : la macro « Alpha » se lance elle-même");
        // A chain deeper than MAX_DEPTH.
        let chain: Vec<Macro> = (0..=MAX_DEPTH).map(|i| mac(&format!("m{}", 10 + i), &format!("C{i}"), if i < MAX_DEPTH { vec![step(&format!("macro.m{}.go", 11 + i), Wait::None)] } else { vec![] })).collect();
        assert!(set(&mut s, chain[1..].to_vec()).is_ok());
        assert!(set(&mut s, chain).unwrap_err().message_fr.contains("imbriquées"));
        // The list in memory is the last good one.
        assert_eq!(s.macros.list().len(), MAX_DEPTH);
    }

    #[test]
    fn refused_and_invalid_lines_cannot_be_saved() {
        let mut s = test_support::shared();
        for (line, refused) in [("armer", true), ("transport.arm on", true), ("safety.hold on", true), ("estop.reset", true), ("Réinitialiser", true), ("SAFETY.HOLD off", true), ("mastr.size 3", false)] {
            let m = mac("m1", "X", vec![step("tempo 120", Wait::None), step("", Wait::Beats(1.0)), step(line, Wait::None)]);
            let e = set(&mut s, vec![m.clone()]).unwrap_err();
            assert_eq!(e.step, Some(3), "{line}");
            assert_eq!(e.refused, refused, "{line}: {e:?}");
            assert!(e.message_fr.starts_with("Macro « X », étape 3 : "), "{e:?}");
            // Even disabled, a refused line is never stored.
            let off = Macro { enabled: false, ..m };
            assert_eq!(set(&mut s, vec![off]).is_err(), refused, "{line}");
        }
        assert!(set(&mut s, vec![mac("m1", "X", vec![step("", Wait::Seconds(0.01))])]).unwrap_err().message_fr.contains("attente hors limites"));
        assert!(set(&mut s, vec![mac("m1", "X", vec![step("", Wait::Beats(f32::NAN))])]).is_err());
        assert!(set(&mut s, vec![mac("m1", "", vec![])]).is_err());
        assert!(set(&mut s, vec![mac("m1", "X", vec![]), mac("m2", "x", vec![])]).is_err(), "names unique, any case");
        let keyed = |id: &str, k: &str| Macro { key: Some(k.into()), ..mac(id, id, vec![]) };
        assert!(set(&mut s, vec![keyed("m1", "m"), keyed("m2", "M")]).is_err());
        assert!(set(&mut s, vec![keyed("m1", "1")]).is_err());
        set(&mut s, vec![keyed("m1", "m")]).unwrap();
        assert_eq!(s.macros.list()[0].key.as_deref(), Some("M"));
        assert!(set(&mut s, vec![mac("m1", "X", vec![MacroStep::default(); MAX_STEPS + 1])]).is_err());
        // Empty ids get the next free one.
        set(&mut s, vec![mac("m1", "A", vec![]), mac("", "B", vec![])]).unwrap();
        assert_eq!(s.macros.list()[1].id, "m2");
        assert!(s.controls.get("macro.m2.go").is_some());
        assert_eq!(s.controls.get("macro.m2.go").unwrap().label_fr, "B");
    }

    #[test]
    fn the_estop_stops_every_macro_and_no_step_runs_after() {
        let mut s = with(vec![
            mac("m1", "Un", vec![step("master.size 50", Wait::Seconds(1.0)), step("master.size 90", Wait::None)]),
            mac("m2", "Deux", vec![step("noir", Wait::None), step("master.size 10", Wait::None)]),
        ]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        s.emergency_stop(ArmSource::Keyboard);
        assert!(s.macros.player.runs.is_empty());
        for i in 0..200 {
            frame(&mut s, i as f64 * 0.1, 0.0);
        }
        assert_eq!(size(&s), 50.0);
        // Refused while latched.
        assert!(start(&mut s, "m1", ControlSource::Ui).unwrap_err().starts_with("arrêt d'urgence"));
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        // A step that latches the stop ends its own macro at once.
        start(&mut s, "m2", ControlSource::Ui).unwrap();
        frame(&mut s, 30.0, 0.0);
        assert!(s.estop.is_latched());
        assert_eq!(size(&s), 50.0, "nothing after « noir »");
        assert!(s.macros.player.runs.is_empty());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_latch_not_yet_seen_by_the_engine_already_hides_the_runs() {
        // T-480: the HTTP fast path trips the latch but, with the lock
        // busy, leaves the runs to the engine's next frame.
        let mut s = with(vec![mac("m1", "Un", vec![step("master.size 50", Wait::Seconds(1.0)), step("master.size 90", Wait::None)])]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        assert_eq!(running(&s).len(), 1);
        s.estop.trip(ArmSource::Keyboard);
        assert_eq!(s.macros.player.runs.len(), 1, "not cleared yet");
        assert!(running(&s).is_empty());
        assert_eq!(live_view(&s)["running"], json!([]));
        assert_eq!(view(&s)["running"], json!([]));
        // And the engine's next frame clears them before any step.
        frame(&mut s, 5.0, 0.0);
        assert!(s.macros.player.runs.is_empty());
        assert_eq!(size(&s), 50.0);
    }

    #[test]
    fn a_disarm_stops_every_macro() {
        let mut s = with(vec![mac("m1", "Un", vec![step("master.size 50", Wait::Seconds(1.0)), step("master.size 90", Wait::None)])]);
        s.request_arm(ArmSource::Ui).unwrap();
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        s.gate.disarm(crate::interlock::DisarmReason::User, ArmSource::Ui);
        frame(&mut s, 5.0, 0.0);
        assert!(s.macros.player.runs.is_empty());
        assert_eq!(size(&s), 50.0);
    }

    #[test]
    fn stop_all_from_a_step_keeps_the_macro_that_asked() {
        let mut s = with(vec![
            mac("m1", "Long", vec![step("", Wait::Go)]),
            mac("m2", "Propre", vec![step("tout arrêter", Wait::None), step("master.size 60", Wait::None)]),
        ]);
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        start(&mut s, "m2", ControlSource::Ui).unwrap();
        frame(&mut s, 0.1, 0.0);
        assert_eq!(size(&s), 60.0);
        assert!(!s.macros.player.is_running("m1"));
        // « Tout arrêter » from the page stops them all.
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        controls::apply(&mut s, "running.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert!(s.macros.player.runs.is_empty());
    }

    #[test]
    fn a_macro_never_arms() {
        let mut s = with(vec![mac("m1", "Tout", vec![step("macro.go", Wait::None), step("transport.freeze on", Wait::None), step("transport.freeze off", Wait::None)])]);
        // Hand-written past the checks (a file): the runtime refuses again.
        s.macros.list[0].steps.push(step("transport.arm on", Wait::None));
        s.macros.list[0].steps.push(step("armer", Wait::None));
        start(&mut s, "m1", ControlSource::Ui).unwrap();
        frame(&mut s, 0.0, 0.0);
        assert!(!s.gate.is_armed());
        assert!(s.macros.player.runs.is_empty());
        // And reloading disables it, the refused lines emptied.
        let w = revalidate(&mut s);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].ends_with("macro désactivée"), "{w:?}");
        let m = &s.macros.list()[0];
        assert!(!m.enabled);
        assert!(m.steps[3].command.is_empty() && m.steps[4].command.is_empty());
        assert_eq!(start(&mut s, "m1", ControlSource::Ui), Err("macro désactivée"));
    }

    #[test]
    fn sanitize_repairs_a_mangled_file() {
        let mut list = vec![
            Macro { id: "bad id".into(), name: "  ".into(), key: Some("ab".into()), ..Default::default() },
            Macro { id: "m1".into(), name: "Même".into(), key: Some("q".into()), ..Default::default() },
            Macro { id: "m1".into(), name: "même".into(), key: Some("Q".into()), steps: vec![step("x", Wait::Seconds(9999.0)), step("", Wait::Beats(f32::INFINITY))], ..Default::default() },
        ];
        sanitize(&mut list);
        let ids: HashSet<&str> = list.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), 3);
        assert!(list.iter().all(|m| valid_id(&m.id) && !m.name.is_empty()));
        assert_eq!(list[1].key.as_deref(), Some("Q"));
        assert_eq!(list[2].key, None);
        assert_ne!(command::fold(&list[1].name), command::fold(&list[2].name));
        assert_eq!(list[2].steps[0].wait, Wait::Seconds(600.0));
        assert_eq!(list[2].steps[1].wait, Wait::None);
        // An old file without the optional fields loads.
        let m: Macro = serde_json::from_str(r#"{"id":"m1","name":"A","steps":[{"command":"tap"}]}"#).unwrap();
        assert!(m.enabled && m.steps[0].enabled && m.steps[0].wait == Wait::None);
        let w: Wait = serde_json::from_str(r#"{"beats":2}"#).unwrap();
        assert_eq!(w, Wait::Beats(2.0));
    }

    #[test]
    fn running_macros_show_in_en_cours_and_stop_there() {
        let mut s = with(vec![mac("m1", "Montée", vec![step("master.size 50", Wait::Beats(4.0)), step("master.size 90", Wait::None)])]);
        start(&mut s, "m1", ControlSource::Keyboard).unwrap();
        frame(&mut s, 0.0, 0.0);
        let snap = crate::running::snapshot(&s);
        let row = &snap["macros"][0];
        assert_eq!(row["name"], "Montée");
        assert_eq!(row["step"], 1);
        assert_eq!(row["steps"], 2);
        assert_eq!(row["waiting"]["kind"], "beats");
        assert_eq!(row["source_label"], "Clavier");
        let req: crate::running::StopRequest = serde_json::from_value(json!({ "kind": "macro", "id": "m1" })).unwrap();
        crate::running::stop(&mut s, &req).unwrap();
        assert!(s.macros.player.runs.is_empty());
        assert!(crate::running::stop(&mut s, &req).is_err());
    }
}
