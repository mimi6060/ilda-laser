//! Notification centre, server side (T-432).
//!
//! Events that reach the operator without a request of theirs: a file
//! that could not be written, an autosave that failed (T-287), the audio
//! input lost or refused. They go into a ring of the last `CAPACITY`
//! notices, in memory only (the application log on disk is T-403, the
//! safety log T-259), which the page reads with `GET /api/notices?since=<id>`
//! when `notices_last` in its state moves. Request errors are not here:
//! the page records those itself from the response.
//!
//! A notice with a `key` is a *condition*: pushing it again while it holds
//! changes nothing, and `resolve` ends it (a resolution entry, so a page
//! reading `since` learns it, and the earlier entries are marked resolved).
//!
//! Never called from the engine thread: `push` and `resolve` run on the
//! HTTP and I/O threads, under the usual lock, in constant time. Writes
//! that fail anywhere (`datafile::save_json`, which the engine thread may
//! call) only *report* here through `report_write`: a `try_lock` on a
//! small bounded queue, never a wait; the queue is drained into `Notices`
//! with the state (`Shared::sync_notices`).

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Notices kept: the oldest goes first.
pub const CAPACITY: usize = 200;
/// Write reports waiting to be drained; more are dropped (the first ones say it).
const MAX_PENDING: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub id: u64,
    /// UTC, ISO 8601.
    pub at: String,
    pub level: Level,
    /// « Projet », « Audio », « Fichiers »…
    pub source: &'static str,
    /// French; may hold a file name of the user's (the page escapes it).
    pub text: String,
    /// A condition's key (`autosave`, `audio`, `write:<file>`).
    pub key: Option<String>,
    /// For a keyed notice: the condition is over. A resolution entry has it
    /// set too (same key, same source, the text it resolves).
    pub resolved: bool,
}

#[derive(Default)]
pub struct Notices {
    list: VecDeque<Notice>,
    last: u64,
}

impl Notices {
    /// Records a notice; returns its id. A keyed one whose condition still
    /// holds with the same level and text is not recorded again (its id).
    pub fn push(&mut self, level: Level, source: &'static str, text: String, key: Option<&str>) -> u64 {
        if let Some(k) = key {
            if let Some(n) = self.active(k) {
                if n.level == level && n.text == text {
                    return n.id;
                }
            }
        }
        self.add(Notice { id: 0, at: crate::project::utc_now(), level, source, text, key: key.map(str::to_string), resolved: false })
    }

    /// Ends the condition `key`, if it holds: its entries are marked
    /// resolved and a resolution entry is added.
    pub fn resolve(&mut self, key: &str) {
        let Some(n) = self.active(key).cloned() else { return };
        for e in self.list.iter_mut().filter(|e| e.key.as_deref() == Some(key)) {
            e.resolved = true;
        }
        self.add(Notice { id: 0, at: crate::project::utc_now(), level: Level::Info, resolved: true, ..n });
    }

    /// Sets condition `key` to `Some((level, text))` (pushed if new or
    /// changed) or `None` (resolved if it held).
    pub fn condition(&mut self, key: &str, source: &'static str, now: Option<(Level, String)>) {
        match now {
            Some((level, text)) => {
                self.push(level, source, text, Some(key));
            }
            None => self.resolve(key),
        }
    }

    /// Notices newer than `id`, oldest first.
    pub fn since(&self, id: u64) -> Vec<Notice> {
        self.list.iter().filter(|n| n.id > id).cloned().collect()
    }

    /// Id of the newest notice (0: none yet).
    pub fn last(&self) -> u64 {
        self.last
    }

    /// The newest entry of condition `key`, if it still holds.
    fn active(&self, key: &str) -> Option<&Notice> {
        self.list.iter().rev().find(|n| n.key.as_deref() == Some(key)).filter(|n| !n.resolved)
    }

    fn add(&mut self, mut n: Notice) -> u64 {
        self.last += 1;
        n.id = self.last;
        if self.list.len() == CAPACITY {
            self.list.pop_front();
        }
        self.list.push_back(n);
        self.last
    }
}

/// A file write that failed (`Err`, with the reason) or succeeded after
/// a failure (`Ok`), by file name.
type WriteReport = (String, Result<(), String>);

static PENDING: Mutex<VecDeque<WriteReport>> = Mutex::new(VecDeque::new());
/// Some write failed this run: successes are then reported too (to resolve).
static ANY_FAILED: AtomicBool = AtomicBool::new(false);

/// Reports the outcome of writing `path` (any thread, never waits: if the
/// queue is busy or full, the report is dropped; the next write reports
/// again). Successes cost nothing until a write has failed.
pub fn report_write(path: &std::path::Path, result: Result<(), String>) {
    report_to(&PENDING, &ANY_FAILED, path, result);
}

fn report_to(queue: &Mutex<VecDeque<WriteReport>>, any_failed: &AtomicBool, path: &std::path::Path, result: Result<(), String>) {
    if result.is_ok() && !any_failed.load(Ordering::Relaxed) {
        return;
    }
    if result.is_err() {
        any_failed.store(true, Ordering::Relaxed);
    }
    let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if let Ok(mut q) = queue.try_lock() {
        if q.len() < MAX_PENDING {
            q.push_back((name, result));
        }
    }
}

/// Moves the write reports into `notices`: a failure is condition
/// `write:<file>`, resolved by the next successful write of that file.
pub fn drain_writes(notices: &mut Notices) {
    drain_from(&PENDING, notices);
}

fn drain_from(queue: &Mutex<VecDeque<WriteReport>>, notices: &mut Notices) {
    let reports: Vec<WriteReport> = match queue.try_lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => return,
    };
    for (name, result) in reports {
        let key = format!("write:{name}");
        notices.condition(&key, "Fichiers", result.err().map(|e| (Level::Error, format!("impossible d'enregistrer {name} : {e}"))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_200_with_growing_ids() {
        let mut n = Notices::default();
        assert_eq!(n.last(), 0);
        for i in 0..250 {
            n.push(Level::Info, "Test", format!("n{i}"), None);
        }
        let all = n.since(0);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].text, "n50");
        assert_eq!(all.last().unwrap().id, 250);
        assert_eq!(n.last(), 250);
        assert!(all.windows(2).all(|w| w[0].id + 1 == w[1].id));
    }

    #[test]
    fn since_returns_only_newer_notices() {
        let mut n = Notices::default();
        let a = n.push(Level::Warning, "Projet", "a".into(), None);
        let b = n.push(Level::Error, "Projet", "b".into(), None);
        assert_eq!(n.since(a).iter().map(|x| x.id).collect::<Vec<_>>(), vec![b]);
        assert!(n.since(b).is_empty());
        assert!(n.since(999).is_empty());
        let v = serde_json::to_value(&n.since(0)[1]).unwrap();
        assert_eq!(v["level"], "error");
        assert!(v["at"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn a_condition_is_recorded_once_and_resolves() {
        let mut n = Notices::default();
        let a = n.push(Level::Error, "Audio", "micro refusé".into(), Some("audio"));
        assert_eq!(n.push(Level::Error, "Audio", "micro refusé".into(), Some("audio")), a, "same condition, same entry");
        assert_eq!(n.since(0).len(), 1);
        n.resolve("audio");
        let all = n.since(0);
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|x| x.resolved), "the entry and its resolution");
        assert_eq!(all[1].key.as_deref(), Some("audio"));
        // Resolving again, or an unknown key: nothing.
        n.resolve("audio");
        n.resolve("other");
        assert_eq!(n.since(0).len(), 2);
        // It comes back: a new entry.
        let b = n.push(Level::Error, "Audio", "micro refusé".into(), Some("audio"));
        assert!(b > a);
        assert!(!n.since(0).last().unwrap().resolved);
    }

    #[test]
    fn a_changed_condition_is_recorded_again() {
        let mut n = Notices::default();
        n.condition("audio", "Audio", Some((Level::Warning, "entrée perdue".into())));
        n.condition("audio", "Audio", Some((Level::Warning, "entrée perdue".into())));
        n.condition("audio", "Audio", Some((Level::Error, "micro refusé".into())));
        assert_eq!(n.since(0).len(), 2);
        n.condition("audio", "Audio", None);
        assert_eq!(n.since(0).len(), 3);
        n.condition("audio", "Audio", None);
        assert_eq!(n.since(0).len(), 3);
    }

    #[test]
    fn write_failures_become_conditions_resolved_by_the_next_success() {
        // A queue of its own: other tests may write (and drain) meanwhile.
        static Q: Mutex<VecDeque<WriteReport>> = Mutex::new(VecDeque::new());
        let failed = AtomicBool::new(false);
        let mut n = Notices::default();
        let start = n.last();
        let p = std::path::Path::new("/nowhere/notices-test.json");
        report_to(&Q, &failed, p, Ok(()));
        assert!(Q.lock().unwrap().is_empty(), "no failure yet: a success costs nothing");
        report_to(&Q, &failed, p, Err("disque plein".into()));
        report_to(&Q, &failed, p, Err("disque plein".into()));
        drain_from(&Q, &mut n);
        let fresh: Vec<Notice> = n.since(start).into_iter().filter(|x| x.key.as_deref() == Some("write:notices-test.json")).collect();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].level, Level::Error);
        assert!(fresh[0].text.contains("notices-test.json"));
        report_to(&Q, &failed, p, Ok(()));
        drain_from(&Q, &mut n);
        assert!(n.since(start).iter().filter(|x| x.key.as_deref() == Some("write:notices-test.json")).all(|x| x.resolved));
    }
}
