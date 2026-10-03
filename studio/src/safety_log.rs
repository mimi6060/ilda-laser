//! Safety event log (T-259): who armed, when, what played, when the
//! emergency stop was used and which protections stepped in, for an
//! incident report, an inspection or a permit application.
//!
//! One JSON Lines file per local day, `<data-dir>/logs/safety-AAAA-MM-JJ.jsonl`,
//! append-only, flushed after every line. Files older than a year are
//! deleted at startup.
//!
//! Recording never touches the disk: `SafetyLog::record` stamps the time
//! and hands the event to a bounded channel (`try_send`), and a writer
//! thread formats and appends it. A slow, full or failing disk can make
//! the writer fall behind or fail, never the caller: when the queue is
//! full the event is dropped and counted, and the next line written says
//! how many were lost. Disarms and the e-stop happen before they are
//! logged, so logging can never stop or delay them.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Log files are kept this many days.
pub const KEEP_DAYS: i32 = 365;
/// Events waiting for the writer. Far more than a burst ever produces; a
/// disk stuck long enough to fill it loses events, never time.
const QUEUE: usize = 1024;

/// One line of the log, as written and read back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SafetyEvent {
    /// RFC 3339, local time with its offset, milliseconds.
    pub ts: String,
    pub kind: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub detail: Value,
    /// Which run of the studio wrote it.
    #[serde(default)]
    pub session: String,
}

/// Where the writer puts its lines: files by day, or a test double.
pub trait Sink: Send + 'static {
    /// Appends one line (no newline) to the file of `day` (`AAAA-MM-JJ`).
    fn append(&mut self, day: &str, line: &str) -> std::io::Result<()>;
}

/// The real sink: `<dir>/safety-<day>.jsonl`, opened in append mode, kept
/// open while the day lasts, flushed after every line.
pub struct DirSink {
    dir: PathBuf,
    day: String,
    file: Option<std::fs::File>,
}

impl DirSink {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, day: String::new(), file: None }
    }
}

impl Sink for DirSink {
    fn append(&mut self, day: &str, line: &str) -> std::io::Result<()> {
        if self.file.is_none() || self.day != day {
            // Rotation: a new day, a new file.
            self.file = None;
            std::fs::create_dir_all(&self.dir)?;
            self.file = Some(OpenOptions::new().create(true).append(true).open(file_of(&self.dir, day))?);
            self.day = day.to_string();
        }
        let file = self.file.as_mut().expect("opened above");
        let written = writeln!(file, "{line}").and_then(|_| file.flush());
        if written.is_err() {
            // Reopen next time (the disk may come back, the file may be gone).
            self.file = None;
        }
        written
    }
}

pub fn file_of(dir: &Path, day: &str) -> PathBuf {
    dir.join(format!("safety-{day}.jsonl"))
}

struct Pending {
    at: SystemTime,
    kind: &'static str,
    source: Option<&'static str>,
    detail: Value,
}

enum Msg {
    Event(Pending),
    /// Answered once every event queued before it is written.
    Flush(SyncSender<()>),
}

#[derive(Default, Serialize)]
struct Health {
    written: u64,
    failed: u64,
    last_error: Option<String>,
}

struct Inner {
    tx: SyncSender<Msg>,
    /// Events dropped because the queue was full, not yet reported.
    dropped: AtomicU64,
    dropped_total: AtomicU64,
    health: Mutex<Health>,
    session: String,
    dir: Option<PathBuf>,
}

/// Handle to the log; cheap to clone. The default one records nothing
/// (unit tests, and anything built before the studio starts its log).
#[derive(Clone, Default)]
pub struct SafetyLog {
    inner: Option<Arc<Inner>>,
}

impl SafetyLog {
    /// The studio's log in `dir` (`<data-dir>/logs`): deletes the files
    /// older than `KEEP_DAYS`, then starts the writer thread.
    pub fn start(dir: PathBuf) -> Self {
        let pruned = prune(&dir, today(), KEEP_DAYS);
        if pruned > 0 {
            log::info!("safety log: deleted {pruned} file(s) older than {KEEP_DAYS} days");
        }
        Self::spawn(DirSink::new(dir.clone()), Some(dir))
    }

    /// A log writing to `sink` (tests: slow, failing or in-memory disks).
    pub fn with_sink(sink: impl Sink) -> Self {
        Self::spawn(sink, None)
    }

    fn spawn(sink: impl Sink, dir: Option<PathBuf>) -> Self {
        let (tx, rx) = sync_channel(QUEUE);
        static STARTED: AtomicU64 = AtomicU64::new(0);
        let n = STARTED.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        let mut session = format!("{:x}-{:x}", now.as_secs(), std::process::id());
        if n > 0 {
            // Only unit tests start several logs in one process.
            session.push_str(&format!("-{n}"));
        }
        let inner = Arc::new(Inner {
            tx,
            dropped: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            health: Mutex::new(Health::default()),
            session,
            dir,
        });
        let writer = Arc::clone(&inner);
        let spawned = std::thread::Builder::new().name("safety-log".into()).spawn(move || write_loop(rx, sink, &writer));
        if let Err(e) = spawned {
            log::error!("safety log: no writer thread: {e}");
        }
        Self { inner: Some(inner) }
    }

    /// Records an event now. Never blocks, never touches the disk.
    pub fn record(&self, kind: &'static str, source: Option<&'static str>, detail: Value) {
        self.record_at(SystemTime::now(), kind, source, detail);
    }

    /// Records an event that happened at `at` (an e-stop tripped on the
    /// HTTP fast path, seen by the gate a frame later).
    pub fn record_at(&self, at: SystemTime, kind: &'static str, source: Option<&'static str>, detail: Value) {
        let Some(inner) = &self.inner else { return };
        match inner.tx.try_send(Msg::Event(Pending { at, kind, source, detail })) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                inner.dropped.fetch_add(1, Ordering::Relaxed);
                inner.dropped_total.fetch_add(1, Ordering::Relaxed);
            }
            // The writer is gone (it only ends with the process).
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Waits up to `timeout` for everything recorded so far to be written.
    /// For readers (the HTTP view, shutdown, tests), never the engine.
    pub fn flush(&self, timeout: Duration) -> bool {
        let Some(inner) = &self.inner else { return true };
        let (tx, rx) = sync_channel(1);
        let deadline = Instant::now() + timeout;
        let mut msg = Msg::Flush(tx);
        loop {
            match inner.tx.try_send(msg) {
                Ok(()) => break,
                Err(TrySendError::Full(m)) if Instant::now() < deadline => {
                    msg = m;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return false,
            }
        }
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_ok()
    }

    pub fn dir(&self) -> Option<&Path> {
        self.inner.as_ref()?.dir.as_deref()
    }

    /// `{session, written, failed, dropped, last_error}` for the UI: a
    /// failing disk shows up in the « Journal » panel.
    pub fn status(&self) -> Value {
        let Some(inner) = &self.inner else { return json!({ "enabled": false }) };
        let health = inner.health.lock().unwrap_or_else(|e| e.into_inner());
        json!({
            "enabled": true,
            "session": inner.session,
            "written": health.written,
            "failed": health.failed,
            "last_error": health.last_error,
            "dropped": inner.dropped_total.load(Ordering::Relaxed),
        })
    }
}

fn write_loop(rx: Receiver<Msg>, mut sink: impl Sink, inner: &Inner) {
    for msg in rx {
        match msg {
            Msg::Event(event) => {
                let lost = inner.dropped.swap(0, Ordering::Relaxed);
                if lost > 0 {
                    let note = Pending { at: event.at, kind: "log_overflow", source: Some("system"), detail: json!({ "lost": lost }) };
                    write_one(&mut sink, inner, &note);
                }
                write_one(&mut sink, inner, &event);
            }
            Msg::Flush(done) => {
                let _ = done.try_send(());
            }
        }
    }
}

fn write_one(sink: &mut impl Sink, inner: &Inner, event: &Pending) {
    let (day, line) = format_line(event, &inner.session);
    let result = sink.append(&day, &line);
    let mut health = inner.health.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(()) => {
            health.written += 1;
            health.last_error = None;
        }
        Err(e) => {
            if health.last_error.is_none() {
                log::error!("safety log: cannot write: {e}");
            }
            health.failed += 1;
            health.last_error = Some(e.to_string());
        }
    }
}

/// The local day (`AAAA-MM-JJ`) and the JSON line of an event.
fn format_line(event: &Pending, session: &str) -> (String, String) {
    let zoned = local(event.at);
    let line = SafetyEvent {
        ts: zoned.strftime("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string(),
        kind: event.kind.to_string(),
        source: event.source.map(str::to_string),
        detail: event.detail.clone(),
        session: session.to_string(),
    };
    let json = serde_json::to_string(&line).unwrap_or_else(|_| format!("{{\"kind\":\"{}\"}}", event.kind));
    (zoned.strftime("%Y-%m-%d").to_string(), json)
}

fn local(at: SystemTime) -> jiff::Zoned {
    let ts = jiff::Timestamp::try_from(at).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    ts.to_zoned(jiff::tz::TimeZone::system())
}

pub fn today() -> jiff::civil::Date {
    local(SystemTime::now()).date()
}

/// The day of a log file name, if it is one of ours.
fn day_of(name: &str) -> Option<jiff::civil::Date> {
    parse_day(name.strip_prefix("safety-")?.strip_suffix(".jsonl")?)
}

/// `AAAA-MM-JJ` only: the date goes into a file name.
pub fn parse_day(s: &str) -> Option<jiff::civil::Date> {
    let b = s.as_bytes();
    let shape = b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shape {
        return None;
    }
    s.parse().ok()
}

/// Deletes the log files of days more than `keep_days` before `today`.
/// Other files are left alone. Returns how many were deleted.
pub fn prune(dir: &Path, today: jiff::civil::Date, keep_days: i32) -> usize {
    let Ok(oldest) = today.checked_sub(jiff::Span::new().days(keep_days)) else { return 0 };
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut deleted = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(day) = name.to_str().and_then(day_of) else { continue };
        if day < oldest {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => deleted += 1,
                Err(e) => log::warn!("safety log: cannot delete {}: {e}", entry.path().display()),
            }
        }
    }
    deleted
}

/// Days that have a log file, newest first.
pub fn days(dir: &Path) -> Vec<String> {
    let mut days: Vec<jiff::civil::Date> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().to_str().and_then(day_of)).collect())
        .unwrap_or_default();
    days.sort_unstable_by(|a, b| b.cmp(a));
    days.into_iter().map(|d| d.to_string()).collect()
}

/// The events of one day, in the order they were written. A torn last
/// line (power cut mid-write) is skipped.
pub fn read_day(dir: &Path, day: &str) -> Vec<SafetyEvent> {
    let Some(date) = parse_day(day) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(file_of(dir, &date.to_string())) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn source_fr(source: Option<&str>) -> &'static str {
    match source {
        Some("ui") => "interface",
        Some("keyboard") => "clavier",
        Some("midi") => "MIDI",
        Some("api") => "API",
        Some("system") => "système",
        _ => "",
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One line of French for the « Journal » table and the CSV export.
pub fn describe(e: &SafetyEvent) -> String {
    let d = &e.detail;
    let changed = || {
        let keys: Vec<&str> = d.get("changed").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if keys.is_empty() {
            String::new()
        } else {
            format!(" : {}", keys.join(", "))
        }
    };
    match e.kind.as_str() {
        "app_start" => format!("Démarrage du studio ({})", str_of(d, "output")),
        "app_stop" => "Arrêt du studio".into(),
        "arm" => "Armement".into(),
        "arm_refused" => {
            let why: Vec<&str> = d.get("blocking").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            format!("Armement refusé : {}", why.join(", "))
        }
        "disarm" => format!("Désarmement — {}", str_of(d, "reason_fr")),
        "estop" => {
            if d.get("was_armed").and_then(Value::as_bool).unwrap_or(false) {
                "Arrêt d'urgence (laser désarmé)".into()
            } else {
                "Arrêt d'urgence".into()
            }
        }
        "estop_reset" => "Arrêt d'urgence réinitialisé".into(),
        "interlock" => {
            let state = if d.get("ok").and_then(Value::as_bool).unwrap_or(false) { "refermé" } else { "ouvert" };
            format!("Verrou « {} » {state}", str_of(d, "label"))
        }
        "presence" => {
            if d.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                "Interface présente".into()
            } else {
                "Interface perdue (plus aucune page ne répond)".into()
            }
        }
        "watchdog" => "Chien de garde : moteur bloqué, sortie coupée".into(),
        "limiter" => {
            let name = match str_of(d, "limiter") {
                "strobe" => "stroboscope",
                other => other,
            };
            if d.get("active").and_then(Value::as_bool).unwrap_or(false) {
                format!("Limiteur {name} actif")
            } else {
                format!("Limiteur {name} relâché")
            }
        }
        "cue" => format!("Cue lancé (armé) : {}", str_of(d, "name")),
        "show" => format!("Show lancé (armé) : {}", str_of(d, "name")),
        "scene" => format!("Scène lancée (armé) : {}", str_of(d, "name")),
        "safety_settings" => format!("Réglages de sécurité modifiés{}", changed()),
        "presence_settings" => format!("Réglages de présence modifiés{}", changed()),
        "midi_safety" => {
            let allow = d.pointer("/after/allow_arm").and_then(Value::as_bool);
            let before = d.pointer("/before/allow_arm").and_then(Value::as_bool);
            match (before, allow) {
                (Some(b), Some(a)) if a != b && a => "Armement MIDI autorisé".into(),
                (Some(b), Some(a)) if a != b => "Armement MIDI interdit".into(),
                _ => format!("Sécurité MIDI modifiée{}", changed()),
            }
        }
        "project_open" => format!("Projet ouvert : {}", str_of(d, "name")),
        "log_overflow" => format!("{} événement(s) perdu(s) : journal saturé", d.get("lost").and_then(Value::as_u64).unwrap_or(0)),
        other => other.to_string(),
    }
}

/// The table the UI shows: each event plus `source_fr` and `text`.
pub fn view(events: &[SafetyEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|e| {
            let mut v = json!(e);
            v["source_fr"] = json!(source_fr(e.source.as_deref()));
            v["text"] = json!(describe(e));
            v
        })
        .collect()
}

/// CSV export (RFC 4180, UTF-8 with a BOM so spreadsheets get the accents
/// right): heure, type, source, événement, détail (JSON), session.
pub fn to_csv(events: &[SafetyEvent]) -> String {
    fn field(s: &str) -> String {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let mut out = String::from("\u{feff}heure,type,source,événement,détail,session\r\n");
    for e in events {
        let row = [e.ts.as_str(), e.kind.as_str(), source_fr(e.source.as_deref()), &describe(e), &e.detail.to_string(), e.session.as_str()];
        out.push_str(&row.iter().map(|s| field(s)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

/// Before/after of a settings change: the top-level fields that changed,
/// for the log. `None` when nothing changed (no line).
pub fn settings_change(before: &impl Serialize, after: &impl Serialize) -> Option<Value> {
    let (before, after) = (json!(before), json!(after));
    if before == after {
        return None;
    }
    let changed: Vec<&String> = match (before.as_object(), after.as_object()) {
        (Some(b), Some(a)) => a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))).filter(|k| b.get(*k) != a.get(*k)).collect(),
        _ => Vec::new(),
    };
    Some(json!({ "changed": changed, "before": before, "after": after }))
}

/// Aggregates one limiter's engagements (T-259: at most one line per
/// limiter per second). Called every frame with the limiter's state;
/// allocates only on the frames that write a line.
pub struct LimiterTracker {
    name: &'static str,
    reported: bool,
    last_line: Option<Instant>,
    /// Engagements since the last line (a flickering limiter).
    engagements: u32,
    was_active: bool,
}

impl LimiterTracker {
    pub fn new(name: &'static str) -> Self {
        Self { name, reported: false, last_line: None, engagements: 0, was_active: false }
    }

    /// Returns true when a line was recorded.
    pub fn update(&mut self, active: bool, now: Instant, log: &SafetyLog, detail: impl FnOnce() -> Value) -> bool {
        if active && !self.was_active {
            self.engagements += 1;
        }
        self.was_active = active;
        if active == self.reported || self.last_line.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return false;
        }
        self.reported = active;
        self.last_line = Some(now);
        let mut d = detail();
        d["limiter"] = json!(self.name);
        d["active"] = json!(active);
        d["engagements"] = json!(std::mem::take(&mut self.engagements));
        log.record("limiter", Some("system"), d);
        true
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// Lines kept in memory: `(day, line)`.
    #[derive(Clone, Default)]
    pub struct MemorySink(pub Arc<Mutex<Vec<(String, String)>>>);

    impl Sink for MemorySink {
        fn append(&mut self, day: &str, line: &str) -> std::io::Result<()> {
            self.0.lock().unwrap().push((day.to_string(), line.to_string()));
            Ok(())
        }
    }

    impl MemorySink {
        pub fn events(&self) -> Vec<SafetyEvent> {
            self.0.lock().unwrap().iter().map(|(_, l)| serde_json::from_str(l).unwrap()).collect()
        }

        pub fn kinds(&self) -> Vec<String> {
            self.events().into_iter().map(|e| e.kind).collect()
        }
    }

    /// A log in memory, and its sink to read back.
    pub fn memory_log() -> (SafetyLog, MemorySink) {
        let sink = MemorySink::default();
        (SafetyLog::with_sink(sink.clone()), sink)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-safety-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn events_are_written_in_order_with_their_fields() {
        let (log, sink) = memory_log();
        log.record("arm", Some("keyboard"), json!({}));
        log.record("cue", Some("ui"), json!({ "id": "tunnels-001", "name": "Tunnel" }));
        log.record("estop", Some("keyboard"), json!({ "was_armed": true }));
        assert!(log.flush(Duration::from_secs(2)));
        let events = sink.events();
        assert_eq!(sink.kinds(), ["arm", "cue", "estop"]);
        assert_eq!(events[0].source.as_deref(), Some("keyboard"));
        assert_eq!(events[1].detail["id"], "tunnels-001");
        assert!(events.iter().all(|e| e.session == events[0].session && !e.session.is_empty()));
        // RFC 3339 with milliseconds and the local offset: 2026-09-29T21:42:10.123+02:00
        let ts = &events[0].ts;
        assert_eq!(ts.len(), 29, "{ts}");
        assert!(ts.parse::<jiff::Timestamp>().is_ok(), "{ts}");
        assert_eq!(&ts[..10], sink.0.lock().unwrap()[0].0, "the file's day is the event's local day");
        assert_eq!(log.status()["written"], 3);
    }

    #[test]
    fn the_time_is_when_it_happened_not_when_it_was_written() {
        let (log, sink) = memory_log();
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_790_000_000_123);
        log.record_at(at, "estop", Some("ui"), json!({}));
        log.flush(Duration::from_secs(2));
        let ts: jiff::Timestamp = sink.events()[0].ts.parse().unwrap();
        assert_eq!(ts.as_millisecond(), 1_790_000_000_123);
    }

    #[test]
    fn files_rotate_by_local_day_and_append() {
        let dir = temp_dir("rotate");
        let log = SafetyLog::with_sink(DirSink::new(dir.clone()));
        // Noon UTC: the same calendar day in every time zone from -11 to +11.
        let noon = |day: &str| -> SystemTime { format!("{day}T12:00:00Z").parse::<jiff::Timestamp>().unwrap().into() };
        log.record_at(noon("2026-03-01"), "arm", Some("ui"), json!({}));
        log.record_at(noon("2026-03-01"), "disarm", Some("ui"), json!({ "reason": "user" }));
        log.record_at(noon("2026-03-02"), "arm", Some("keyboard"), json!({}));
        assert!(log.flush(Duration::from_secs(2)));
        assert_eq!(read_day(&dir, "2026-03-01").iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["arm", "disarm"]);
        assert_eq!(read_day(&dir, "2026-03-02").len(), 1);
        assert_eq!(days(&dir), ["2026-03-02", "2026-03-01"]);
        // A second run appends to the same day's file.
        let again = SafetyLog::with_sink(DirSink::new(dir.clone()));
        again.record_at(noon("2026-03-02"), "app_start", Some("system"), json!({}));
        again.flush(Duration::from_secs(2));
        let day2 = read_day(&dir, "2026-03-02");
        assert_eq!(day2.len(), 2);
        assert_ne!(day2[0].session, day2[1].session);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_torn_line_is_skipped_and_bad_days_are_refused() {
        let dir = temp_dir("torn");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file_of(&dir, "2026-01-05"), "{\"ts\":\"x\",\"kind\":\"arm\",\"session\":\"s\"}\n{\"ts\":\"y\",\"ki").unwrap();
        assert_eq!(read_day(&dir, "2026-01-05").len(), 1);
        for bad in ["../etc", "2026-1-05", "2026-01-05/../../x", "2026-13-01", ""] {
            assert!(parse_day(bad).is_none(), "{bad}");
            assert!(read_day(&dir, bad).is_empty());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_older_than_a_year_are_deleted_and_nothing_else() {
        let dir = temp_dir("prune");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["safety-2025-09-28.jsonl", "safety-2025-09-29.jsonl", "safety-2026-09-29.jsonl", "safety-2020-01-01.jsonl", "notes.txt", "safety-old.jsonl"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let today: jiff::civil::Date = "2026-09-29".parse().unwrap();
        assert_eq!(prune(&dir, today, KEEP_DAYS), 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["notes.txt", "safety-2025-09-29.jsonl", "safety-2026-09-29.jsonl", "safety-old.jsonl"]);
        assert_eq!(prune(&dir.join("missing"), today, KEEP_DAYS), 0);
        // `start` prunes too.
        std::fs::write(dir.join("safety-2001-01-01.jsonl"), "").unwrap();
        let _log = SafetyLog::start(dir.clone());
        assert!(!dir.join("safety-2001-01-01.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A disk that takes 200 ms per line.
    struct SlowSink(Arc<Mutex<Vec<String>>>);
    impl Sink for SlowSink {
        fn append(&mut self, _: &str, line: &str) -> std::io::Result<()> {
            std::thread::sleep(Duration::from_millis(200));
            self.0.lock().unwrap().push(line.to_string());
            Ok(())
        }
    }

    #[test]
    fn a_slow_disk_never_slows_the_caller() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log = SafetyLog::with_sink(SlowSink(Arc::clone(&lines)));
        // A 60 fps loop that logs 10 times a second (far more than the
        // engine ever does) keeps its frame time.
        let mut worst = Duration::ZERO;
        for frame in 0..60u32 {
            let t = Instant::now();
            if frame % 6 == 0 {
                log.record("limiter", Some("system"), json!({ "frame": frame }));
            }
            worst = worst.max(t.elapsed());
            std::thread::sleep(Duration::from_micros(16_667).saturating_sub(t.elapsed()));
        }
        assert!(worst < Duration::from_millis(5), "record took {worst:?}");
        assert!(lines.lock().unwrap().len() < 10, "the writer is behind, the caller is not");
        assert!(log.flush(Duration::from_secs(10)));
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 10, "nothing lost while the queue had room");
        assert!(lines[9].contains("\"frame\":54"), "in order");
    }

    /// A disk that is stuck (full, unplugged, hung NFS): every write blocks.
    struct StuckSink(Arc<Mutex<()>>);
    impl Sink for StuckSink {
        fn append(&mut self, _: &str, _: &str) -> std::io::Result<()> {
            let _stuck = self.0.lock().unwrap();
            Ok(())
        }
    }

    #[test]
    fn a_stuck_disk_drops_events_instead_of_blocking_and_says_so() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let log = SafetyLog::with_sink(StuckSink(Arc::clone(&gate)));
        let t = Instant::now();
        for _ in 0..QUEUE + 500 {
            log.record("arm", Some("ui"), json!({}));
        }
        assert!(t.elapsed() < Duration::from_millis(500), "{:?} for {} events", t.elapsed(), QUEUE + 500);
        assert!(!log.flush(Duration::from_millis(50)), "a reader gives up; it never hangs");
        let dropped = log.status()["dropped"].as_u64().unwrap();
        assert!(dropped >= 499, "{dropped}");
        drop(held);
        assert!(log.flush(Duration::from_secs(5)));
    }

    #[test]
    fn the_overflow_is_reported_in_the_log() {
        let (log, sink) = memory_log();
        let inner = log.inner.as_ref().unwrap();
        inner.dropped.store(7, Ordering::Relaxed);
        log.record("arm", Some("ui"), json!({}));
        log.flush(Duration::from_secs(2));
        let events = sink.events();
        assert_eq!(sink.kinds(), ["log_overflow", "arm"]);
        assert_eq!(describe(&events[0]), "7 événement(s) perdu(s) : journal saturé");
    }

    struct FailingSink;
    impl Sink for FailingSink {
        fn append(&mut self, _: &str, _: &str) -> std::io::Result<()> {
            Err(std::io::Error::other("No space left on device"))
        }
    }

    #[test]
    fn a_failing_disk_is_reported_not_fatal() {
        let log = SafetyLog::with_sink(FailingSink);
        for _ in 0..10 {
            log.record("disarm", Some("ui"), json!({}));
        }
        assert!(log.flush(Duration::from_secs(2)));
        let status = log.status();
        assert_eq!(status["failed"], 10);
        assert_eq!(status["last_error"], "No space left on device");
        // A directory that cannot be created (a file is in the way).
        let dir = temp_dir("blocked");
        std::fs::write(&dir, "not a directory").unwrap();
        let log = SafetyLog::with_sink(DirSink::new(dir.join("logs")));
        log.record("arm", Some("ui"), json!({}));
        log.flush(Duration::from_secs(2));
        assert_eq!(log.status()["failed"], 1);
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn the_default_log_records_nothing() {
        let log = SafetyLog::default();
        log.record("arm", None, json!({}));
        assert!(log.flush(Duration::from_millis(10)));
        assert_eq!(log.status()["enabled"], false);
    }

    #[test]
    fn a_limiter_writes_at_most_one_line_a_second() {
        let (log, sink) = memory_log();
        let mut tracker = LimiterTracker::new("strobe");
        let t0 = Instant::now();
        // 10 s at 60 fps of a limiter that flickers on and off every frame.
        for frame in 0..600u64 {
            tracker.update(frame % 2 == 0, t0 + Duration::from_micros(frame * 16_667), &log, || json!({}));
        }
        log.flush(Duration::from_secs(2));
        let n = sink.events().len();
        assert!((1..=10).contains(&n), "{n} lines");
        // Held on for 10 s: one line, then one when it lets go.
        let (log, sink) = memory_log();
        let mut tracker = LimiterTracker::new("strobe");
        for frame in 0..600u64 {
            tracker.update(frame < 590, t0 + Duration::from_micros(frame * 16_667), &log, || json!({ "rate_hz": 8.0 }));
        }
        log.flush(Duration::from_secs(2));
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].detail["active"].as_bool(), events[1].detail["active"].as_bool()), (Some(true), Some(false)));
        assert_eq!(events[0].detail["rate_hz"], 8.0);
        assert_eq!(describe(&events[0]), "Limiteur stroboscope actif");
    }

    #[test]
    fn settings_changes_list_what_changed() {
        #[derive(Serialize)]
        struct S {
            a: u8,
            b: bool,
        }
        assert!(settings_change(&S { a: 1, b: true }, &S { a: 1, b: true }).is_none());
        let d = settings_change(&S { a: 1, b: true }, &S { a: 2, b: true }).unwrap();
        assert_eq!(d["changed"], json!(["a"]));
        assert_eq!((d["before"]["a"].as_u64(), d["after"]["a"].as_u64()), (Some(1), Some(2)));
    }

    #[test]
    fn csv_quotes_and_describes() {
        let e = SafetyEvent {
            ts: "2026-09-29T21:42:10.000+02:00".into(),
            kind: "arm_refused".into(),
            source: Some("ui".into()),
            detail: json!({ "blocking": ["Porte, \"ouverte\""] }),
            session: "s1".into(),
        };
        let csv = to_csv(&[e]);
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "\u{feff}heure,type,source,événement,détail,session");
        assert_eq!(
            lines.next().unwrap(),
            r#"2026-09-29T21:42:10.000+02:00,arm_refused,interface,"Armement refusé : Porte, ""ouverte""","{""blocking"":[""Porte, \""ouverte\""""]}",s1"#
        );
    }
}
