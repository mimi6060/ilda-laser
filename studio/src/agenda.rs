//! Agenda (T-455): scheduled playback for fixed installations.
//!
//! A list of entries, each a name, a target and a slot: a local time
//! `HH:MM` on some days of the week, or on one date. At the slot, the
//! target starts as if the operator had pressed *Lecture*: a saved show
//! (`controls::timeline_play`), or the show player's list from its first
//! item (`showlist::play_once`). It plays to its end once: the agenda
//! turns the loop off for that playback and puts it back afterwards. An
//! *Arrêt* entry stops the playback and disarms at its slot (a curfew).
//!
//! What it never does:
//! - **It never arms.** Disarmed at the slot, the target plays in the
//!   preview and a notification says so. Light only reaches the output
//!   if the operator armed through the normal path, with every interlock,
//!   presence, hold-to-run and checklist rule still holding.
//! - It never interrupts the operator: a playback already running (a
//!   show, the list, a cue list) skips the slot, noted in the history.
//! - With the e-stop latched, nothing starts. An e-stop, or a disarm
//!   during a playback it started, ends that playback and suspends the
//!   agenda: no slot starts anything until the operator arms again (or
//!   switches *Agenda actif* off and on). An *Arrêt* entry still runs.
//! - A slot missed by more than `LATE_LIMIT` (studio closed, Mac asleep)
//!   is not caught up: one history line, no burst. At a change of time,
//!   an hour that does not exist that day is skipped; an hour that exists
//!   twice starts once, at the first one.
//! - No control in the registry: MIDI, DMX, OSC, the remote and the
//!   external gate can't switch it or edit it. Edits go through
//!   `POST /api/agenda` (an edit: history, locked in show mode).
//!
//! The agenda is the machine's and the venue's, not the project's
//! (`<data-dir>/agenda.json`): opening a project schedules nothing.
//! Off by default. The time is the system's, in the system time zone;
//! tests pass the time in (`tick`), the `--test-hooks` route shifts it.
//!
//! It runs on its own thread (`spawn`), four times a second, like a
//! request of the operator's: loading a show reads the disk, which the
//! engine thread never does.

use crate::Shared;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{SignedDuration, Span, Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Entries kept at most.
pub const MAX_ENTRIES: usize = 100;
/// History lines kept, newest first.
pub const HISTORY: usize = 20;
/// A slot later than this is missed, never caught up.
pub const LATE_LIMIT: SignedDuration = SignedDuration::from_secs(60);
/// How often the agenda looks at the clock.
pub const TICK: Duration = Duration::from_millis(250);
const MAX_NAME: usize = 64;
const DAYS_FR: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agenda {
    /// *Agenda actif*. Off by default.
    pub enabled: bool,
    pub entries: Vec<AgendaEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgendaEntry {
    /// Unique; 0 (a new entry) gets one when the agenda is set.
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub target: AgendaTarget,
    /// Local time, `HH:MM`.
    pub time: String,
    /// Monday to Sunday (weekly).
    pub days: [bool; 7],
    /// One date, `YYYY-MM-DD`: the days are then ignored.
    pub date: Option<String>,
}

impl Default for AgendaEntry {
    fn default() -> Self {
        Self { id: 0, name: String::new(), enabled: true, target: AgendaTarget::ShowList, time: "20:30".into(), days: [false; 7], date: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgendaTarget {
    /// A show of `studio-data/shows/`, by name.
    Show { name: String },
    /// The show player's list (T-444), from its first item.
    #[default]
    ShowList,
    /// *Arrêt et noir*: stops the playback and disarms. Only ever takes
    /// light away, so it runs even when the agenda is suspended.
    Stop,
}

impl AgendaTarget {
    fn label_fr(&self) -> String {
        match self {
            AgendaTarget::Show { name } => format!("show « {name} »"),
            AgendaTarget::ShowList => "lecteur de shows".into(),
            AgendaTarget::Stop => "arrêt et noir".into(),
        }
    }
}

/// One history line.
#[derive(Clone, Debug, Serialize)]
pub struct AgendaRun {
    /// The slot, local, RFC 3339.
    pub at: String,
    /// The slot in French (« ven. 09/10 20:30 »).
    pub when: String,
    pub entry: String,
    pub result: String,
}

/// `HH:MM` → hour and minute.
fn hm(time: &str) -> Option<(i8, i8)> {
    let (h, m) = time.trim().split_once(':')?;
    let ok = |p: &str| (1..=2).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
    if !ok(h) || m.len() != 2 || !ok(m) {
        return None;
    }
    let (h, m): (i8, i8) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

fn date_of(s: &str) -> Option<Date> {
    let s = s.trim();
    (s.len() == 10).then(|| s.parse().ok()).flatten()
}

/// The instant of `date` at `h:m` in `tz`. A time that does not exist
/// that day (clocks forward) has none; one that exists twice (clocks
/// back) is the first.
fn instant(tz: &TimeZone, date: Date, h: i8, m: i8) -> Option<Timestamp> {
    let dt = date.at(h, m, 0, 0);
    match tz.to_ambiguous_timestamp(dt).offset() {
        AmbiguousOffset::Unambiguous { offset } => offset.to_timestamp(dt).ok(),
        AmbiguousOffset::Gap { .. } => None,
        AmbiguousOffset::Fold { before, .. } => before.to_timestamp(dt).ok(),
    }
}

impl AgendaEntry {
    /// Its slots on the days around `now` (a week either side; a date
    /// entry: its one slot, however far), in order.
    fn slots(&self, now: &Zoned) -> Vec<Timestamp> {
        let Some((h, m)) = hm(&self.time).filter(|_| self.enabled) else { return Vec::new() };
        let tz = now.time_zone();
        let dates: Vec<Date> = match &self.date {
            Some(d) => date_of(d).into_iter().collect(),
            None => (-8..=8)
                .filter_map(|k| now.date().checked_add(Span::new().days(k)).ok())
                .filter(|d| self.days[d.weekday().to_monday_zero_offset() as usize])
                .collect(),
        };
        dates.into_iter().filter_map(|d| instant(tz, d, h, m)).collect()
    }

    fn next_after(&self, now: &Zoned) -> Option<Timestamp> {
        let ts = now.timestamp();
        self.slots(now).into_iter().find(|&t| t > ts)
    }

    fn last_until(&self, now: &Zoned) -> Option<Timestamp> {
        let ts = now.timestamp();
        self.slots(now).into_iter().rev().find(|&t| t <= ts)
    }
}

/// The next slot after `now`, of the enabled entries (whether or not the
/// agenda is on): the entry's id and the slot, in `now`'s zone. A tie
/// goes to the entry listed first.
pub fn next_due(agenda: &Agenda, now: &Zoned) -> Option<(u32, Zoned)> {
    agenda
        .entries
        .iter()
        .filter_map(|e| e.next_after(now).map(|t| (t, e.id)))
        .min_by_key(|&(t, _)| t)
        .map(|(t, id)| (id, t.to_zoned(now.time_zone().clone())))
}

/// « ven. 09/10 20:30 ».
pub fn when_fr(z: &Zoned) -> String {
    format!("{} {}", DAYS_FR[z.weekday().to_monday_zero_offset() as usize], z.strftime("%d/%m %H:%M"))
}

impl Agenda {
    /// At most `MAX_ENTRIES`, names and times trimmed, an empty date
    /// gone, ids unique and non-zero.
    pub fn sanitize(&mut self) {
        self.entries.truncate(MAX_ENTRIES);
        let mut seen = std::collections::HashSet::new();
        let mut next = self.entries.iter().map(|e| e.id).max().unwrap_or(0);
        for e in &mut self.entries {
            e.name = e.name.trim().chars().take(MAX_NAME).collect();
            e.time = e.time.trim().to_string();
            if e.date.as_deref().is_some_and(|d| d.trim().is_empty()) {
                e.date = None;
            }
            if e.id == 0 || !seen.insert(e.id) {
                next = next.saturating_add(1);
                e.id = next;
                seen.insert(next);
            }
        }
    }

    /// Why an agenda sent by the page can't be saved, in French.
    pub fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(format!("{MAX_ENTRIES} entrées au plus dans l'agenda"));
        }
        for e in &self.entries {
            let who = if e.name.trim().is_empty() { "entrée sans nom".to_string() } else { format!("« {} »", e.name.trim()) };
            if e.name.trim().is_empty() {
                return Err("chaque entrée de l'agenda a besoin d'un nom".into());
            }
            if hm(&e.time).is_none() {
                return Err(format!("{who} : heure invalide « {} » (HH:MM)", e.time));
            }
            if let Some(d) = e.date.as_deref().filter(|d| !d.trim().is_empty()) {
                if date_of(d).is_none() {
                    return Err(format!("{who} : date invalide « {d} » (AAAA-MM-JJ)"));
                }
            }
            if let AgendaTarget::Show { name } = &e.target {
                if !crate::timeline::valid_show_name(name.trim()) {
                    return Err(format!("{who} : nom de show invalide « {name} »"));
                }
            }
        }
        Ok(())
    }
}

/// The playback an entry started, while it lasts.
#[derive(Clone, Debug)]
struct ActiveRun {
    entry: String,
    target: AgendaTarget,
    /// The timeline's *Boucle* before the agenda turned it off.
    prev_loop: bool,
    started: SystemTime,
    /// The laser was armed at some point of the playback.
    seen_armed: bool,
}

/// `Shared::agenda`: the entries and what the agenda did.
#[derive(Default)]
pub struct AgendaRunner {
    agenda: Agenda,
    path: Option<PathBuf>,
    /// `--test-hooks`: the clock shifted from the system's, and its zone.
    offset: SignedDuration,
    zone: Option<TimeZone>,
    /// The time of the last look: a slot counts once it is past it.
    last_check: Option<Timestamp>,
    /// Per entry, the last slot handled (a clock set back never plays it again).
    done: HashMap<u32, Timestamp>,
    runs: VecDeque<AgendaRun>,
    active: Option<ActiveRun>,
    /// Why nothing starts until the operator arms again.
    suspended: Option<String>,
    /// Suspended at this very look (an arm seen now came before it).
    suspended_now: bool,
    trips: Option<u64>,
    next: Option<(u32, Zoned)>,
    rev: u64,
}

impl AgendaRunner {
    /// `agenda.json` (missing or damaged: an empty agenda, off).
    pub fn load(path: PathBuf) -> Self {
        let mut agenda: Agenda = crate::load_json(&path);
        agenda.sanitize();
        Self { agenda, path: Some(path), ..Self::default() }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn agenda(&self) -> &Agenda {
        &self.agenda
    }

    /// The agenda's clock: the system's (shifted by the test hook).
    pub fn now(&self) -> Zoned {
        let tz = self.zone.clone().unwrap_or_else(TimeZone::system);
        let now = Timestamp::now();
        now.checked_add(self.offset).unwrap_or(now).to_zoned(tz)
    }

    fn record(&mut self, at: &Zoned, entry: &str, result: String) {
        log::info!("agenda : « {entry} » {} : {result}", when_fr(at));
        self.runs.push_front(AgendaRun { at: at.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(), when: when_fr(at), entry: entry.to_string(), result });
        self.runs.truncate(HISTORY);
        self.rev += 1;
    }

    fn suspend(&mut self, why: &str) {
        if self.suspended.is_none() {
            log::warn!("agenda suspendu : {why}");
            self.rev += 1;
        }
        self.suspended = Some(why.to_string());
        self.suspended_now = true;
    }

    fn resume(&mut self) {
        if self.suspended.take().is_some() {
            self.rev += 1;
        }
    }

    /// A playback the agenda started still plays.
    pub fn is_playing(&self) -> bool {
        self.active.is_some()
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended.is_some()
    }

    #[cfg(test)]
    pub fn runs(&self) -> Vec<AgendaRun> {
        self.runs.iter().cloned().collect()
    }
}

// ---------- operations on the studio ----------

/// Replace the agenda (the page, the history): checked, saved. Switching
/// it on lifts a suspension (the operator's own gesture).
pub fn set(s: &mut Shared, mut agenda: Agenda) -> Result<(), String> {
    agenda.validate()?;
    agenda.sanitize();
    let a = &mut s.agenda;
    let switched_on = agenda.enabled && !a.agenda.enabled;
    // While off, the agenda kept looking at the clock: nothing before
    // this moment starts.
    if switched_on {
        a.resume();
    }
    // A slot handled stays handled while its entry is unchanged.
    let old = std::mem::replace(&mut a.agenda, agenda);
    a.done.retain(|id, _| a.agenda.entries.iter().find(|e| e.id == *id) == old.entries.iter().find(|e| e.id == *id));
    a.rev += 1;
    if let Some(path) = &a.path {
        crate::save_json(path, &a.agenda);
    }
    let now = a.now();
    a.next = next_due(&a.agenda, &now);
    Ok(())
}

/// Something is playing that the agenda must not interrupt.
fn busy(s: &Shared) -> bool {
    s.timeline.transport != crate::timeline::Transport::Stopped || s.showlist.is_active() || s.cue_list.is_active()
}

/// The agenda's playback still plays.
fn still_playing(s: &Shared, run: &ActiveRun) -> bool {
    match &run.target {
        AgendaTarget::Show { name } => s.timeline.transport != crate::timeline::Transport::Stopped && s.timeline.show.as_ref().is_some_and(|sh| &sh.name == name),
        AgendaTarget::ShowList => s.showlist.is_active() || (s.timeline.is_playing() && s.showlist.transport != crate::showlist::ListTransport::Stopped),
        AgendaTarget::Stop => false,
    }
}

/// The agenda's playback ended: the loop goes back as it was.
fn finish(s: &mut Shared) {
    if let Some(run) = s.agenda.active.take() {
        if run.prev_loop {
            s.timeline.loop_on = true;
        }
        s.showlist.set_once(false);
        s.agenda.rev += 1;
    }
}

/// An e-stop or a disarm ended the agenda's playback: stopped, noted.
fn cancel(s: &mut Shared, now: &Zoned, why: &str) {
    let Some(run) = s.agenda.active.clone() else { return };
    if still_playing(s, &run) {
        match run.target {
            AgendaTarget::ShowList => crate::showlist::stop(s),
            _ => s.timeline.stop(),
        }
    }
    finish(s);
    s.agenda.record(now, &run.entry, format!("interrompu ({why})"));
}

/// Stops every playback (a show, the list, a cue list) and disarms.
fn stop_all(s: &mut Shared) {
    finish(s);
    crate::showlist::stop(s);
    s.timeline.stop();
    if s.cue_list.list.is_some() {
        let _ = crate::cuelist::top(s, crate::controls::ControlSource::System);
    }
    if s.gate.is_armed() {
        s.gate.disarm(crate::interlock::DisarmReason::Schedule, crate::interlock::ArmSource::System);
    }
}

fn notify(s: &mut Shared, level: crate::notices::Level, text: String) {
    s.notices.push(level, "Agenda", text, None);
}

/// Start `entry`'s target at its slot `due` (or say why not).
fn fire(s: &mut Shared, entry: &AgendaEntry, due: &Zoned) {
    use crate::notices::Level;
    let name = entry.name.clone();
    if entry.target == AgendaTarget::Stop {
        stop_all(s);
        s.agenda.record(due, &name, "arrêt et noir effectués".into());
        notify(s, Level::Info, format!("Agenda : « {name} » : lecture arrêtée, laser désarmé."));
        return;
    }
    let refused = if s.estop.is_latched() {
        Some(("non lancé (arrêt d'urgence)".to_string(), Level::Warning))
    } else if let Some(why) = s.agenda.suspended.clone() {
        Some((format!("non lancé : agenda suspendu ({why}), réarmer le laser pour le reprendre"), Level::Warning))
    } else if busy(s) {
        Some(("non lancé : une lecture était en cours".to_string(), Level::Info))
    } else {
        None
    };
    if let Some((why, level)) = refused {
        s.agenda.record(due, &name, why.clone());
        notify(s, level, format!("Agenda : « {name} » {why}."));
        return;
    }
    let prev_loop = s.timeline.loop_on;
    let started = match &entry.target {
        AgendaTarget::Show { name: show } => match (s.timeline.unsaved(), s.shows.load(show)) {
            (Some(open), _) => Err(format!("le show « {open} » a des modifications non enregistrées")),
            (None, Err(e)) => Err(format!("show « {show} » introuvable ({e:#})")),
            (None, Ok(loaded)) => {
                s.timeline.loop_on = false;
                s.timeline.load(loaded);
                crate::controls::timeline_play(s).map_err(str::to_string)
            }
        },
        AgendaTarget::ShowList => {
            s.timeline.loop_on = false;
            crate::showlist::play_once(s)
        }
        AgendaTarget::Stop => unreachable!("handled above"),
    };
    match started {
        Ok(()) => {
            let armed = s.gate.is_armed();
            s.agenda.active = Some(ActiveRun { entry: name.clone(), target: entry.target.clone(), prev_loop, started: SystemTime::now(), seen_armed: armed });
            if armed {
                s.agenda.record(due, &name, "lancé (laser armé)".into());
                notify(s, Level::Info, format!("Agenda : « {name} » lancé."));
            } else {
                s.agenda.record(due, &name, "joué en aperçu (laser désarmé)".into());
                notify(s, Level::Info, format!("Agenda : « {name} » joue en aperçu (laser désarmé)."));
            }
        }
        Err(why) => {
            s.timeline.loop_on = prev_loop;
            s.agenda.record(due, &name, format!("non lancé : {why}"));
            notify(s, Level::Warning, format!("Agenda : « {name} » non lancé : {why}."));
        }
    }
}

/// One look at the clock, `now` (the agenda thread, four times a second;
/// tests pass a simulated time). Never arms.
pub fn tick(s: &mut Shared, now: &Zoned) {
    let ts = now.timestamp();
    let armed = s.gate.is_armed();
    // The operator armed again since a suspension: slots start again.
    if armed && !s.agenda.suspended_now {
        s.agenda.resume();
    }
    s.agenda.suspended_now = false;
    // An e-stop (even one already reset) ends the agenda's playback and
    // suspends it.
    let trips = s.estop.trips();
    let tripped = s.agenda.trips.is_some_and(|t| t != trips) || s.estop.is_latched();
    s.agenda.trips = Some(trips);
    if tripped && (s.agenda.agenda.enabled || s.agenda.active.is_some()) {
        cancel(s, now, "arrêt d'urgence");
        s.agenda.suspend("arrêt d'urgence");
    }
    if let Some(run) = s.agenda.active.as_mut() {
        run.seen_armed |= armed;
        let disarmed = run.seen_armed && s.gate.last_disarm().is_some_and(|(_, at)| at >= run.started);
        if disarmed {
            cancel(s, now, "laser désarmé");
            s.agenda.suspend("laser désarmé pendant une lecture de l'agenda");
        }
    }
    if s.agenda.active.as_ref().is_some_and(|run| !still_playing(s, run)) {
        finish(s);
    }
    let lower = s.agenda.last_check.unwrap_or(ts - LATE_LIMIT);
    s.agenda.last_check = Some(ts);
    if !s.agenda.agenda.enabled {
        return;
    }
    // The slots passed since the last look, oldest first.
    let mut due: Vec<(Timestamp, AgendaEntry)> = s
        .agenda
        .agenda
        .entries
        .iter()
        .filter_map(|e| e.last_until(now).map(|t| (t, e.clone())))
        .filter(|(t, e)| *t > lower && s.agenda.done.get(&e.id).is_none_or(|d| t > d))
        .collect();
    due.sort_by_key(|(t, _)| *t);
    for (t, entry) in due {
        s.agenda.done.insert(entry.id, t);
        let at = t.to_zoned(now.time_zone().clone());
        let late = ts.duration_since(t);
        if late > LATE_LIMIT {
            let minutes = late.as_secs() / 60;
            s.agenda.record(&at, &entry.name, format!("manqué : studio fermé ou en veille ({minutes} min de retard), non rattrapé"));
        } else {
            fire(s, &entry, &at);
        }
    }
    let next = next_due(&s.agenda.agenda, now);
    if next.as_ref().map(|(id, z)| (id, z.timestamp())) != s.agenda.next.as_ref().map(|(id, z)| (id, z.timestamp())) {
        s.agenda.rev += 1;
    }
    s.agenda.next = next;
}

/// The agenda's thread: a look every `TICK`, until the studio stops.
pub fn spawn(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("agenda".into())
        .spawn(move || {
            while running.load(Ordering::SeqCst) {
                {
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let now = s.agenda.now();
                    tick(&mut s, &now);
                }
                std::thread::sleep(TICK);
            }
        })
        .expect("failed to start the agenda thread")
}

/// `--test-hooks`: the agenda's clock reads `at` (local, `YYYY-MM-DDTHH:MM:SS`)
/// from now on, in zone `tz` (IANA or POSIX; none: the system's).
pub fn set_test_clock(s: &mut Shared, at: &str, tz: Option<&str>) -> Result<(), String> {
    let zone = match tz {
        Some(name) => TimeZone::get(name).or_else(|_| TimeZone::posix(name)).map_err(|e| format!("fuseau inconnu « {name} » : {e}"))?,
        None => TimeZone::system(),
    };
    let dt: DateTime = at.parse().map_err(|e| format!("heure invalide « {at} » : {e}"))?;
    let target = zone.to_ambiguous_timestamp(dt).compatible().map_err(|e| format!("{e}"))?;
    s.agenda.offset = target.duration_since(Timestamp::now());
    s.agenda.zone = tz.map(|_| zone);
    s.agenda.rev += 1;
    Ok(())
}

fn next_view(s: &Shared) -> Value {
    match &s.agenda.next {
        Some((id, at)) => {
            let e = s.agenda.agenda.entries.iter().find(|e| e.id == *id);
            let (name, target) = e.map_or((String::new(), String::new()), |e| (e.name.clone(), e.target.label_fr()));
            json!({ "id": id, "name": name, "target": target, "at": at.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(), "when": when_fr(at), "time": at.strftime("%H:%M").to_string() })
        }
        None => Value::Null,
    }
}

/// With every frame (`/api/frame`): small.
pub fn summary(s: &Shared) -> Value {
    json!({
        "enabled": s.agenda.agenda.enabled,
        "next": next_view(s),
        "suspended": s.agenda.suspended,
        "playing": s.agenda.active.as_ref().map(|r| r.entry.clone()),
        "rev": s.agenda.rev,
    })
}

/// In `/api/state`: the summary and the last runs.
pub fn state(s: &Shared) -> Value {
    let mut v = summary(s);
    v["last_runs"] = json!(s.agenda.runs);
    v
}

/// `GET /api/agenda`: the entries, the state, the clock.
pub fn view(s: &Shared) -> Value {
    let now = s.agenda.now();
    json!({
        "agenda": s.agenda.agenda,
        "state": state(s),
        "now": now.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "now_when": when_fr(&now),
        "time_zone": now.time_zone().iana_name().unwrap_or(""),
        "max_entries": MAX_ENTRIES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;
    use crate::timeline::{Event, EventSource, Show, ShowStore, Track};

    /// Brussels' rules, fixed: clocks forward on the last Sunday of March
    /// at 02:00, back on the last Sunday of October at 03:00.
    fn tz() -> TimeZone {
        TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap()
    }

    fn at(s: &str) -> Zoned {
        let dt: DateTime = s.parse().unwrap();
        tz().to_ambiguous_zoned(dt).compatible().unwrap()
    }

    fn show(name: &str, len: f64) -> Show {
        let event = Event { id: 1, start: 0.0, len, source: EventSource::Cue { id: "nope".into() }, ..Default::default() };
        Show { name: name.into(), tracks: vec![Track { layer: 1, events: vec![event], ..Default::default() }], ..Default::default() }
    }

    /// A studio with its own shows and a simulated engine clock.
    fn studio(tag: &str, shows: &[(&str, f64)]) -> Shared {
        let mut s = shared();
        let dir = std::env::temp_dir().join(format!("laser-studio-agenda-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        s.shows = ShowStore::new(dir.join("shows"));
        let now = std::time::Instant::now();
        s.epoch = now;
        s.sim_now = Some(now);
        for (name, len) in shows {
            s.shows.save(&show(name, *len)).unwrap();
        }
        s
    }

    fn entry(name: &str, target: AgendaTarget, time: &str, days: &[usize], date: Option<&str>) -> AgendaEntry {
        let mut d = [false; 7];
        for &i in days {
            d[i] = true;
        }
        AgendaEntry { name: name.into(), target, time: time.into(), days: d, date: date.map(str::to_string), ..Default::default() }
    }

    fn show_target(name: &str) -> AgendaTarget {
        AgendaTarget::Show { name: name.into() }
    }

    fn agenda(entries: Vec<AgendaEntry>) -> Agenda {
        let mut a = Agenda { enabled: true, entries };
        a.sanitize();
        a
    }

    /// The engine's part: the timeline and the list move on by `dt`.
    fn engine(s: &mut Shared, dt: f64) {
        let at = s.sim_now.unwrap() + Duration::from_secs_f64(dt);
        s.sim_now = Some(at);
        let c = s.timeline_clock();
        if s.estop.is_latched() {
            s.timeline.halt(&c);
        }
        crate::showlist::frame(s, &c);
        s.timeline.frame(&c);
    }

    /// From `from`, `secs` seconds of studio: the engine at 60 fps, the
    /// agenda every 250 ms. Returns the time reached.
    fn run(s: &mut Shared, from: &Zoned, secs: f64) -> Zoned {
        let mut now = from.clone();
        let steps = (secs * 4.0).round() as usize;
        for _ in 0..steps {
            for _ in 0..15 {
                engine(s, 1.0 / 60.0);
            }
            now = now.checked_add(SignedDuration::from_millis(250)).unwrap();
            tick(s, &now);
        }
        now
    }

    fn last(s: &Shared) -> String {
        s.agenda.runs.front().map(|r| r.result.clone()).unwrap_or_default()
    }

    fn arm(s: &mut Shared) {
        crate::controls::apply(s, "transport.arm", crate::controls::ControlInput::Value(1.0), false).unwrap();
        assert!(s.gate.is_armed());
    }

    #[test]
    fn next_due_weekly_single_date_and_midnight() {
        // 2026-10-09 is a Friday (index 4).
        let a = agenda(vec![entry("Soir", show_target("A"), "20:30", &[4], None), entry("Réveillon", show_target("A"), "00:00", &[], Some("2027-01-01"))]);
        let (id, z) = next_due(&a, &at("2026-10-07T12:00:00")).unwrap();
        assert_eq!((id, z.strftime("%F %R").to_string()), (1, "2026-10-09 20:30".into()));
        // Exactly at the slot: the next one is a week later.
        let (_, z) = next_due(&a, &at("2026-10-09T20:30:00")).unwrap();
        assert_eq!(z.strftime("%F %R").to_string(), "2026-10-16 20:30");
        // Late December: midnight of the first of January comes first.
        let (id, z) = next_due(&a, &at("2026-12-31T23:59:00")).unwrap();
        assert_eq!((id, z.strftime("%F %R").to_string()), (2, "2027-01-01 00:00".into()));
        // A weekly entry at midnight on Saturday: from Friday 23:59.
        let m = agenda(vec![entry("Minuit", AgendaTarget::ShowList, "00:00", &[5], None)]);
        let (_, z) = next_due(&m, &at("2026-10-09T23:59:00")).unwrap();
        assert_eq!(z.strftime("%F %R").to_string(), "2026-10-10 00:00");
        assert_eq!(when_fr(&z), "sam. 10/10 00:00");
    }

    #[test]
    fn a_disabled_entry_a_past_date_and_no_days_have_no_slot() {
        let mut off = entry("Off", show_target("A"), "20:30", &[0, 1, 2, 3, 4, 5, 6], None);
        off.enabled = false;
        let a = agenda(vec![off, entry("Passé", show_target("A"), "20:30", &[], Some("2025-01-01")), entry("Jamais", show_target("A"), "20:30", &[], None)]);
        assert_eq!(next_due(&a, &at("2026-10-07T12:00:00")), None);
    }

    #[test]
    fn the_change_of_time_skips_a_missing_hour_and_plays_a_double_one_once() {
        // 2027-03-28: 02:30 does not exist; 2026-10-25: 02:30 exists twice.
        let a = agenda(vec![entry("Nuit", AgendaTarget::ShowList, "02:30", &[6], None)]);
        let (_, z) = next_due(&a, &at("2027-03-27T12:00:00")).unwrap();
        assert_eq!(z.strftime("%F %R").to_string(), "2027-04-04 02:30", "the missing hour is skipped");
        let (_, first) = next_due(&a, &at("2026-10-24T12:00:00")).unwrap();
        assert_eq!(first.strftime("%F %R %:z").to_string(), "2026-10-25 02:30 +02:00", "the first of the two");
        let (_, after) = next_due(&a, &first).unwrap();
        assert_eq!(after.strftime("%F %R").to_string(), "2026-11-01 02:30", "never the second one");
        // Through the night of the change, the agenda starts it once.
        let mut s = studio("fold", &[]);
        s.agenda.agenda = a;
        let mut n = 0;
        let mut now = at("2026-10-25T01:59:00");
        tick(&mut s, &now);
        for _ in 0..(3 * 3600 / 30) {
            now = now.checked_add(SignedDuration::from_secs(30)).unwrap();
            let before = s.agenda.runs.len();
            tick(&mut s, &now);
            n += s.agenda.runs.len() - before;
        }
        assert_eq!(n, 1, "{:?}", s.agenda.runs());
    }

    #[test]
    fn a_show_starts_at_its_time_once_and_stops_at_its_end_without_the_loop() {
        let mut s = studio("show", &[("Soir", 2.0)]);
        set(&mut s, agenda(vec![entry("Show du soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        s.timeline.loop_on = true;
        let t0 = at("2026-10-09T20:29:58");
        tick(&mut s, &t0);
        assert!(!s.timeline.is_playing());
        let t = run(&mut s, &t0, 2.5);
        assert!(s.timeline.is_playing(), "started at 20:30");
        assert_eq!(s.timeline.show.as_ref().unwrap().name, "Soir");
        assert!(!s.timeline.loop_on, "no loop for the agenda's playback");
        assert_eq!(last(&s), "joué en aperçu (laser désarmé)");
        assert_eq!(s.agenda.runs.len(), 1);
        let t = run(&mut s, &t, 2.5);
        assert!(!s.timeline.is_playing(), "stopped at its end");
        assert!(s.timeline.loop_on, "the operator's loop is back");
        assert_eq!(s.agenda.runs.len(), 1, "once per slot");
        run(&mut s, &t, 10.0);
        assert!(!s.timeline.is_playing());
        assert!(!s.gate.is_armed(), "the agenda never arms");
        let n = s.notices.since(0);
        assert!(n.iter().any(|n| n.text == "Agenda : « Show du soir » joue en aperçu (laser désarmé)."), "{n:?}");
    }

    #[test]
    fn armed_by_the_operator_the_show_plays_and_stays_armed() {
        let mut s = studio("armed", &[("Soir", 5.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        arm(&mut s);
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        run(&mut s, &t0, 1.5);
        assert!(s.timeline.is_playing());
        assert!(s.gate.is_armed());
        assert_eq!(last(&s), "lancé (laser armé)");
    }

    #[test]
    fn the_show_list_plays_once_without_its_loop() {
        let mut s = studio("list", &[("A", 1.0), ("B", 1.0)]);
        let list = crate::showlist::ShowList { items: ["A", "B"].iter().map(|n| crate::showlist::ShowListItem { show: n.to_string(), ..Default::default() }).collect(), loop_list: true };
        crate::showlist::set(&mut s, list).unwrap();
        set(&mut s, agenda(vec![entry("Liste", AgendaTarget::ShowList, "20:30", &[4], None)])).unwrap();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        let t = run(&mut s, &t0, 1.5);
        assert!(s.showlist.is_active());
        let t = run(&mut s, &t, 4.0);
        assert!(!s.showlist.is_active(), "the list played once");
        assert!(!s.timeline.is_playing());
        assert!(s.showlist.list().loop_list, "the saved list keeps its loop");
        assert!(s.agenda.active.is_none());
        run(&mut s, &t, 3.0);
        assert!(!s.showlist.is_active());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn late_by_59_s_starts_late_by_61_s_does_not() {
        for (late, plays) in [(59, true), (61, false)] {
            let mut s = studio(&format!("late{late}"), &[("Soir", 5.0)]);
            set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
            tick(&mut s, &at("2026-10-09T20:29:00"));
            // Asleep, then awake `late` seconds after the slot.
            tick(&mut s, &at("2026-10-09T20:30:00").checked_add(SignedDuration::from_secs(late)).unwrap());
            assert_eq!(s.timeline.is_playing(), plays, "late by {late} s");
            assert_eq!(s.agenda.runs.len(), 1);
            if !plays {
                assert!(last(&s).starts_with("manqué"), "{}", last(&s));
            }
        }
    }

    #[test]
    fn a_week_asleep_is_one_line_never_a_burst() {
        let mut s = studio("week", &[("Soir", 5.0)]);
        set(&mut s, agenda(vec![entry("Tous", show_target("Soir"), "20:30", &[0, 1, 2, 3, 4, 5, 6], None)])).unwrap();
        tick(&mut s, &at("2026-10-01T12:00:00"));
        tick(&mut s, &at("2026-10-09T12:00:00"));
        assert_eq!(s.agenda.runs.len(), 1);
        assert!(!s.timeline.is_playing());
        // At start-up, an old slot is not even a line.
        let mut s = studio("startup", &[("Soir", 5.0)]);
        set(&mut s, agenda(vec![entry("Tous", show_target("Soir"), "20:30", &[0, 1, 2, 3, 4, 5, 6], None)])).unwrap();
        s.agenda.last_check = None;
        tick(&mut s, &at("2026-10-09T22:00:00"));
        assert!(s.agenda.runs.is_empty());
    }

    #[test]
    fn a_playback_already_running_is_never_interrupted() {
        let mut s = studio("busy", &[("Opérateur", 60.0), ("Soir", 5.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        let mine = s.shows.load("Opérateur").unwrap();
        s.timeline.load(mine);
        crate::controls::timeline_play(&mut s).unwrap();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        run(&mut s, &t0, 2.0);
        assert_eq!(s.timeline.show.as_ref().unwrap().name, "Opérateur");
        assert!(s.timeline.is_playing());
        assert_eq!(last(&s), "non lancé : une lecture était en cours");
    }

    #[test]
    fn a_latched_estop_starts_nothing_and_the_agenda_waits_for_an_arm() {
        let mut s = studio("estop", &[("Soir", 5.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None), entry("Plus tard", show_target("Soir"), "21:00", &[4], None)])).unwrap();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        s.emergency_stop(crate::interlock::ArmSource::Keyboard);
        let t = run(&mut s, &t0, 2.0);
        assert!(!s.timeline.is_playing());
        assert_eq!(last(&s), "non lancé (arrêt d'urgence)");
        assert!(s.notices.since(0).iter().any(|n| n.text == "Agenda : « Soir » non lancé (arrêt d'urgence)."));
        // Reset, but not armed again: the next slot starts nothing either.
        s.gate.reset_estop(&s.estop.clone());
        let t = run(&mut s, &t, 1.0);
        assert!(s.agenda.is_suspended());
        let t = run(&mut s, &t.checked_add(SignedDuration::from_secs(29 * 60 + 55)).unwrap(), 6.0);
        assert!(!s.timeline.is_playing());
        assert!(last(&s).contains("agenda suspendu"), "{}", last(&s));
        // The operator arms: the agenda takes over again (it still never arms).
        arm(&mut s);
        run(&mut s, &t, 0.5);
        assert!(!s.agenda.is_suspended());
    }

    #[test]
    fn an_estop_or_a_disarm_during_the_agenda_playback_ends_it() {
        let mut s = studio("cancel", &[("Soir", 30.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        arm(&mut s);
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        let t = run(&mut s, &t0, 2.0);
        assert!(s.timeline.is_playing());
        s.gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        run(&mut s, &t, 0.5);
        assert!(!s.timeline.is_playing());
        assert_eq!(last(&s), "interrompu (laser désarmé)");
        assert!(s.agenda.is_suspended());
        assert!(!s.gate.is_armed());
        // And an e-stop.
        let mut s = studio("cancel2", &[("Soir", 30.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        tick(&mut s, &t0);
        let t = run(&mut s, &t0, 2.0);
        assert!(s.timeline.is_playing());
        s.emergency_stop(crate::interlock::ArmSource::Ui);
        s.gate.reset_estop(&s.estop.clone());
        run(&mut s, &t, 3.0);
        assert!(!s.timeline.is_playing(), "never by itself");
        assert_eq!(last(&s), "interrompu (arrêt d'urgence)");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_stop_entry_stops_and_disarms_even_when_suspended() {
        let mut s = studio("stop", &[("Opérateur", 600.0)]);
        set(&mut s, agenda(vec![entry("Couvre-feu", AgendaTarget::Stop, "23:00", &[4], None)])).unwrap();
        arm(&mut s);
        let mine = s.shows.load("Opérateur").unwrap();
        s.timeline.load(mine);
        crate::controls::timeline_play(&mut s).unwrap();
        s.agenda.suspend("test");
        s.agenda.suspended_now = true;
        let t0 = at("2026-10-09T22:59:59");
        tick(&mut s, &t0);
        run(&mut s, &t0, 2.0);
        assert!(!s.timeline.is_playing());
        assert!(!s.gate.is_armed());
        assert_eq!(last(&s), "arrêt et noir effectués");
        assert_eq!(s.gate.last_disarm().unwrap().0, &crate::interlock::DisarmReason::Schedule);
    }

    #[test]
    fn presence_and_the_gate_are_left_alone() {
        let mut s = studio("presence", &[("Soir", 3.0)]);
        s.presence.enforced = true;
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        let blocking = s.gate.blocking();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        run(&mut s, &t0, 2.0);
        assert!(s.timeline.is_playing());
        assert!(!s.gate.is_armed());
        assert_eq!(s.gate.blocking(), blocking, "no interlock moved");
        assert!(s.presence.enforced);
    }

    #[test]
    fn off_by_default_and_off_starts_nothing() {
        let mut s = studio("off", &[("Soir", 3.0)]);
        assert!(!s.agenda.agenda().enabled);
        let mut a = agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)]);
        a.enabled = false;
        set(&mut s, a.clone()).unwrap();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        let t = run(&mut s, &t0, 2.0);
        assert!(!s.timeline.is_playing());
        // Switched on after the slot: not caught up.
        a.enabled = true;
        set(&mut s, a).unwrap();
        run(&mut s, &t, 2.0);
        assert!(!s.timeline.is_playing());
        assert!(s.agenda.runs.is_empty());
    }

    #[test]
    fn a_clock_set_back_never_plays_a_slot_twice() {
        let mut s = studio("back", &[("Soir", 1.0)]);
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        let t0 = at("2026-10-09T20:29:59");
        tick(&mut s, &t0);
        run(&mut s, &t0, 3.0);
        run(&mut s, &t0, 3.0);
        assert_eq!(s.agenda.runs.len(), 1);
    }

    #[test]
    fn a_missing_file_or_fields_give_the_defaults_and_bad_entries_are_refused() {
        let dir = std::env::temp_dir().join(format!("laser-studio-agenda-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = AgendaRunner::load(dir.join("agenda.json"));
        assert_eq!(r.agenda(), &Agenda::default());
        std::fs::write(dir.join("agenda.json"), r#"{"entries":[{"name":"X"}]}"#).unwrap();
        let r = AgendaRunner::load(dir.join("agenda.json"));
        assert!(!r.agenda().enabled);
        let e = &r.agenda().entries[0];
        assert_eq!((e.id, e.enabled, &e.target, e.time.as_str(), e.days, &e.date), (1, true, &AgendaTarget::ShowList, "20:30", [false; 7], &None));
        let bad = |e: AgendaEntry| Agenda { enabled: true, entries: vec![e] }.validate();
        assert!(bad(entry("", AgendaTarget::ShowList, "20:30", &[0], None)).is_err());
        assert!(bad(entry("X", AgendaTarget::ShowList, "24:00", &[0], None)).is_err());
        assert!(bad(entry("X", AgendaTarget::ShowList, "8:5", &[0], None)).is_err());
        assert!(bad(entry("X", AgendaTarget::ShowList, "20:30", &[], Some("2026-02-30"))).is_err());
        assert!(bad(entry("X", show_target("../x"), "20:30", &[0], None)).is_err());
        assert!(bad(entry("X", AgendaTarget::ShowList, "8:05", &[0], Some(""))).is_ok());
        let too_many = Agenda { enabled: true, entries: vec![entry("X", AgendaTarget::ShowList, "20:30", &[0], None); MAX_ENTRIES + 1] };
        assert!(too_many.validate().is_err());
        // Saved and read back.
        let mut s = shared();
        s.agenda = AgendaRunner::load(dir.join("agenda.json"));
        set(&mut s, agenda(vec![entry("Soir", show_target("Soir"), "20:30", &[4], None)])).unwrap();
        assert_eq!(AgendaRunner::load(dir.join("agenda.json")).agenda(), s.agenda.agenda());
    }

    #[test]
    fn the_test_clock_shifts_the_agenda_time() {
        let mut s = shared();
        set_test_clock(&mut s, "2026-10-09T20:29:30", Some("Europe/Paris")).unwrap();
        let now = s.agenda.now();
        assert_eq!(now.date(), jiff::civil::date(2026, 10, 9));
        assert!((now.hour(), now.minute()) == (20, 29) || (now.hour(), now.minute()) == (20, 30));
        assert!(set_test_clock(&mut s, "demain", None).is_err());
    }

    #[test]
    fn no_control_drives_the_agenda() {
        let s = shared();
        assert!(s.controls.list().iter().all(|c| !c.id.starts_with("agenda")));
    }
}
