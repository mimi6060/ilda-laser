//! Application log on disk and diagnostic report (T-403).
//!
//! Every `log::` line keeps going to the terminal as before (`env_logger`,
//! `RUST_LOG`), and from `info` up (prefs.json → `log.level`) it is also
//! written to `<data-dir>/logs/app-AAAA-MM-JJ.log`, one file per UTC day,
//! kept 14 days, at most 20 MB a day. The safety log (T-259) stays a
//! separate set of files (`safety-*.jsonl`); nothing here touches them.
//!
//! The caller never touches the disk: a line is formatted in memory and
//! handed to the safety log's bounded queue (`safety_log::Queue`,
//! `try_send`); a full queue drops and counts it. A writer thread stamps,
//! redacts (the data directory becomes `<data-dir>`, the home folder `~`)
//! and appends. A log directory that can't be written is reported once on
//! the terminal and in `/api/state` (`applog.write_failed`); the studio
//! runs on.
//!
//! The diagnostic report (`report`) is a zip the user downloads and sends
//! themselves: logs, about.json, devices.json, settings.json, state.json
//! and, only on request, the project without its figures. Never a media
//! file (audio, ILDA, figure).

use crate::safety_log::{self, DirSink, Naming, Queue, Sink, SAFETY_FILES};
use log::{Level, LevelFilter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// `app-AAAA-MM-JJ.log`.
pub const APP_FILES: Naming = Naming { prefix: "app-", ext: ".log" };
/// At most this much per day; then one « journal tronqué » line and
/// silence until the next day.
pub const DAY_CAP: u64 = 20 * 1024 * 1024;
/// Logs put in a diagnostic report, newest first, at most this much.
const REPORT_LOG_BYTES: u64 = 64 * 1024 * 1024;
/// Extensions a report must never carry (audio, ILDA, figures, images).
pub const MEDIA_EXTENSIONS: &[&str] = &[".wav", ".mp3", ".flac", ".ogg", ".aiff", ".aif", ".m4a", ".ild", ".svg", ".png", ".jpg", ".jpeg"];

/// Log preferences (`prefs.json` → `log`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    pub enabled: bool,
    pub keep_days: u32,
    /// "error", "warn", "info", "debug" or "trace".
    pub level: String,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self { enabled: true, keep_days: 14, level: "info".into() }
    }
}

impl LogSettings {
    /// From `<data-dir>/prefs.json`; defaults when missing or unreadable.
    pub fn load(data_dir: &Path) -> Self {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Prefs {
            log: LogSettings,
        }
        crate::load_json::<Prefs>(&data_dir.join("prefs.json")).log
    }

    fn level_filter(&self) -> LevelFilter {
        self.level.parse().unwrap_or(LevelFilter::Info)
    }

    fn keep(&self) -> u32 {
        self.keep_days.clamp(1, 365)
    }
}

/// One line on its way to the writer.
struct Line {
    at: SystemTime,
    level: Level,
    target: String,
    text: String,
}

#[derive(Default)]
struct Health {
    written: u64,
    failed: u64,
    last_error: Option<String>,
    /// The day whose file reached `DAY_CAP`.
    truncated: Option<String>,
}

/// The application log. Built once by `init`; the default one writes
/// nothing (unit tests).
pub struct AppLog {
    queue: Option<Queue<Line>>,
    health: Arc<Mutex<Health>>,
    dir: Option<PathBuf>,
    level: LevelFilter,
    keep_days: u32,
    started: Instant,
    /// Replacements applied to everything written or exported.
    redactions: Vec<(String, String)>,
}

impl Default for AppLog {
    fn default() -> Self {
        Self { queue: None, health: Arc::default(), dir: None, level: LevelFilter::Off, keep_days: 14, started: Instant::now(), redactions: Vec::new() }
    }
}

/// Starts the log in `<data-dir>/logs` and installs it as the `log`
/// logger (replaces `env_logger::init()`; the terminal output is kept).
pub fn init(data_dir: &Path, settings: &LogSettings) -> Arc<AppLog> {
    let app = Arc::new(AppLog::start(data_dir.join("logs"), settings, redactions(data_dir)));
    let term = env_logger::Builder::from_default_env().build();
    let max = term.filter().max(app.level);
    if log::set_boxed_logger(Box::new(Logger { term: Some(term), app: Arc::clone(&app) })).is_ok() {
        log::set_max_level(max);
    }
    app
}

/// The data directory (absolute) → `<data-dir>`, the home folder → `~`.
fn redactions(data_dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(abs) = std::fs::canonicalize(data_dir) {
        out.push((abs.to_string_lossy().into_owned(), "<data-dir>".into()));
    }
    if let Ok(abs) = std::path::absolute(data_dir) {
        out.push((abs.to_string_lossy().into_owned(), "<data-dir>".into()));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| h.len() > 1) {
        out.push((home.to_string_lossy().into_owned(), "~".into()));
    }
    out
}

fn redact(text: &str, redactions: &[(String, String)]) -> String {
    let mut text = text.to_string();
    for (from, to) in redactions {
        if !from.is_empty() && text.contains(from.as_str()) {
            text = text.replace(from.as_str(), to);
        }
    }
    text
}

impl AppLog {
    /// The log in `dir`: deletes the files past `keep_days`, checks the
    /// folder can be written (a warning once if not), starts the writer.
    pub fn start(dir: PathBuf, settings: &LogSettings, redactions: Vec<(String, String)>) -> Self {
        if !settings.enabled {
            return Self::default();
        }
        prune(&dir, utc_day(SystemTime::now()), settings.keep());
        let log = Self::spawn(DirSink::named(dir.clone(), APP_FILES), Some(dir.clone()), settings, DAY_CAP, redactions);
        let probe = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(APP_FILES.file(&dir, &utc_day(SystemTime::now()).to_string())));
        if let Err(e) = probe {
            eprintln!("Journal de l'application : écriture impossible dans {} ({e}) ; le studio continue sans journal sur disque.", dir.display());
            log.health.lock().unwrap_or_else(|e| e.into_inner()).last_error = Some(e.to_string());
        }
        log
    }

    fn spawn(sink: impl Sink, dir: Option<PathBuf>, settings: &LogSettings, cap: u64, redactions: Vec<(String, String)>) -> Self {
        let health: Arc<Mutex<Health>> = Arc::default();
        let mut writer = Writer {
            sink,
            dir: dir.clone(),
            keep_days: settings.keep(),
            cap,
            day: String::new(),
            bytes: 0,
            truncated: false,
            health: Arc::clone(&health),
            warned: false,
            redactions: redactions.clone(),
        };
        let queue = Queue::spawn("app-log", move |line: Line, lost| writer.write(line, lost));
        Self { queue: Some(queue), health, dir, level: settings.level_filter(), keep_days: settings.keep(), started: Instant::now(), redactions }
    }

    /// Whether lines of `level` go to the file.
    pub fn wants(&self, level: Level) -> bool {
        self.queue.is_some() && level <= self.level
    }

    /// Queues one line, stamped now. Never blocks, never touches the disk.
    pub fn push(&self, level: Level, target: &str, text: String) {
        self.push_at(SystemTime::now(), level, target, text);
    }

    pub fn push_at(&self, at: SystemTime, level: Level, target: &str, text: String) {
        if let Some(queue) = &self.queue {
            let target = target.strip_prefix("laser_studio::").unwrap_or(target).to_string();
            queue.push(Line { at, level, target, text });
        }
    }

    /// From the panic hook, after the output is cut: one line, `try_send`.
    pub fn panic(&self, info: &std::panic::PanicHookInfo) {
        self.push(Level::Error, "panic", format!("panique : {info}"));
    }

    /// Waits up to `timeout` for the queued lines to be written (readers,
    /// shutdown; never the engine).
    pub fn flush(&self, timeout: Duration) -> bool {
        self.queue.as_ref().is_none_or(|q| q.flush(timeout))
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// For `/api/state` and the RÉGLAGES › Diagnostic panel.
    pub fn status(&self) -> Value {
        let Some(queue) = &self.queue else { return json!({ "enabled": false, "write_failed": false }) };
        let health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        json!({
            "enabled": true,
            "dir": self.dir,
            "level": self.level.to_string().to_lowercase(),
            "keep_days": self.keep_days,
            "written": health.written,
            "failed": health.failed,
            "dropped": queue.dropped_total(),
            "last_error": health.last_error,
            "write_failed": health.last_error.is_some(),
            "truncated": health.truncated,
        })
    }

    /// The last `n` lines (today's file, then yesterday's if needed).
    pub fn tail(&self, n: usize) -> Vec<String> {
        let Some(dir) = &self.dir else { return Vec::new() };
        self.flush(Duration::from_millis(200));
        tail_lines(dir, n)
    }
}

/// The writer thread's side: rotation, purge, size cap, redaction.
struct Writer<S: Sink> {
    sink: S,
    dir: Option<PathBuf>,
    keep_days: u32,
    cap: u64,
    day: String,
    bytes: u64,
    truncated: bool,
    health: Arc<Mutex<Health>>,
    /// The terminal has been told the disk fails (once until it works).
    warned: bool,
    redactions: Vec<(String, String)>,
}

impl<S: Sink> Writer<S> {
    fn write(&mut self, line: Line, lost: u64) {
        let day = utc_day(line.at);
        let day_s = day.to_string();
        if day_s != self.day {
            // A new day (or the first line): new file, purge the old ones.
            self.day = day_s;
            self.bytes = self.sink.size(&self.day);
            self.truncated = false;
            if let Some(dir) = &self.dir {
                prune(dir, day, self.keep_days);
            }
        }
        if lost > 0 {
            let note = Line { at: line.at, level: Level::Warn, target: "applog".into(), text: format!("{lost} ligne(s) perdue(s) : journal saturé") };
            self.append(&note);
        }
        self.append(&line);
    }

    fn append(&mut self, line: &Line) {
        if self.truncated {
            return;
        }
        let text = redact(&format_line(line), &self.redactions);
        if self.bytes + text.len() as u64 + 1 > self.cap {
            self.truncated = true;
            let note = Line { at: line.at, level: Level::Warn, target: "applog".into(), text: "journal tronqué : taille maximale du jour atteinte, reprise demain".into() };
            self.put(&format_line(&note));
            self.health.lock().unwrap_or_else(|e| e.into_inner()).truncated = Some(self.day.clone());
            return;
        }
        self.put(&text);
    }

    fn put(&mut self, text: &str) {
        let result = self.sink.append(&self.day, text);
        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(()) => {
                self.bytes += text.len() as u64 + 1;
                health.written += 1;
                health.last_error = None;
                self.warned = false;
            }
            Err(e) => {
                // Not through `log::`: that would come back here.
                if !self.warned {
                    eprintln!("Journal de l'application : écriture impossible ({e})");
                    self.warned = true;
                }
                health.failed += 1;
                health.last_error = Some(e.to_string());
            }
        }
    }
}

/// `2026-10-03T21:42:10.123+02:00 INFO  midi::worker: MIDI : … connecté`;
/// a multi-line message is indented so every line starts with a stamp or
/// spaces.
fn format_line(line: &Line) -> String {
    let ts = safety_log::local(line.at).strftime("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string();
    let text = line.text.trim_end().replace('\n', "\n    ");
    format!("{ts} {:<5} {}: {text}", line.level, line.target)
}

pub fn utc_day(at: SystemTime) -> jiff::civil::Date {
    let ts = jiff::Timestamp::try_from(at).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    ts.to_zoned(jiff::tz::TimeZone::UTC).date()
}

/// Keeps the `keep` newest days up to `today`: older files go, and never
/// more than `keep` `app-*.log` files remain (a clock set back leaves no
/// pile of « future » files). Returns how many were deleted.
pub fn prune(dir: &Path, today: jiff::civil::Date, keep: u32) -> usize {
    let keep = keep.max(1);
    let mut deleted = safety_log::prune_files(dir, APP_FILES, today, keep as i32 - 1);
    for day in safety_log::days_of(dir, APP_FILES).iter().skip(keep as usize) {
        if std::fs::remove_file(APP_FILES.file(dir, day)).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// The last `n` lines of the newest log files (at most two days, the end
/// of each file only).
fn tail_lines(dir: &Path, n: usize) -> Vec<String> {
    const WINDOW: u64 = 512 * 1024;
    let mut out: Vec<String> = Vec::new();
    for day in safety_log::days_of(dir, APP_FILES).iter().take(2) {
        let Ok(mut file) = std::fs::File::open(APP_FILES.file(dir, day)) else { continue };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let start = len.saturating_sub(WINDOW);
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_to_end(&mut buf)).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0); // cut mid-line
        }
        lines.append(&mut out);
        out = lines;
        if out.len() >= n {
            break;
        }
    }
    let skip = out.len().saturating_sub(n);
    out.split_off(skip)
}

/// The `log` logger: the terminal as before, plus the file.
pub struct Logger {
    term: Option<env_logger::Logger>,
    app: Arc<AppLog>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.term.as_ref().is_some_and(|t| t.enabled(metadata)) || self.app.wants(metadata.level())
    }

    fn log(&self, record: &log::Record) {
        if let Some(term) = &self.term {
            if term.matches(record) {
                term.log(record);
            }
        }
        if self.app.wants(record.level()) {
            self.app.push(record.level(), record.target(), record.args().to_string());
        }
    }

    fn flush(&self) {
        if let Some(term) = &self.term {
            term.flush();
        }
    }
}

/// The macOS version (`sw_vers`), read once.
pub fn os_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        if cfg!(target_os = "macos") {
            std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        }
    })
}

/// What a diagnostic report holds besides the logs, gathered by the HTTP
/// handler (a snapshot taken under the lock, then released).
pub struct ReportParts {
    pub devices: Value,
    pub settings: Value,
    pub state: Value,
    /// Only with « Inclure le projet (sans les médias) ».
    pub project: Option<Value>,
}

/// What the report contains, for about.json and the UI's list.
pub const REPORT_CONTENTS_FR: &[&str] = &[
    "logs/ : journal de l'application et journal de sécurité des 14 derniers jours",
    "about.json : version, système, architecture, durée de fonctionnement",
    "devices.json : entrées audio, ports MIDI, état de la sortie laser",
    "settings.json : réglages de sécurité, présence, calibration, plafonds de puissance, préférences",
    "state.json : l'état courant du studio (/api/state)",
    "project.json : le projet courant sans ses figures, seulement si la case est cochée",
];

/// The diagnostic report, a zip (stored, no compression). Every text is
/// redacted like the log; nothing but the files listed in
/// `REPORT_CONTENTS_FR` goes in.
pub fn report(app: &AppLog, parts: ReportParts) -> Vec<u8> {
    app.flush(Duration::from_millis(300));
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut logs_omitted: Vec<String> = Vec::new();
    if let Some(dir) = app.dir() {
        let mut budget = REPORT_LOG_BYTES;
        for naming in [APP_FILES, SAFETY_FILES] {
            for day in safety_log::days_of(dir, naming).iter().take(app.keep_days as usize) {
                let name = format!("{}{day}{}", naming.prefix, naming.ext);
                match std::fs::read(naming.file(dir, day)) {
                    Ok(bytes) if (bytes.len() as u64) <= budget => {
                        budget -= bytes.len() as u64;
                        let text = redact(&String::from_utf8_lossy(&bytes), &app.redactions);
                        entries.push((format!("logs/{name}"), text.into_bytes()));
                    }
                    _ => logs_omitted.push(name),
                }
            }
        }
    }
    let json_entry = |name: &str, v: &Value| (name.to_string(), redact(&serde_json::to_string_pretty(v).unwrap_or_default(), &app.redactions).into_bytes());
    let mut status = app.status();
    if let Some(o) = status.as_object_mut() {
        o.remove("dir");
    }
    let about = json!({
        "application": "Laser Studio",
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "os_version": os_version(),
        "arch": std::env::consts::ARCH,
        "uptime_s": app.uptime().as_secs(),
        "created": safety_log::local(SystemTime::now()).strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "log": status,
        "contents": REPORT_CONTENTS_FR,
        "project_included": parts.project.is_some(),
        "logs_omitted_size": logs_omitted,
        "never_included": "aucun fichier audio, ILDA, figure ou image ; les chemins du dossier de données et du dossier personnel sont masqués",
    });
    entries.push(json_entry("about.json", &about));
    entries.push(json_entry("devices.json", &parts.devices));
    entries.push(json_entry("settings.json", &parts.settings));
    entries.push(json_entry("state.json", &parts.state));
    if let Some(project) = &parts.project {
        entries.push(json_entry("project.json", project));
    }
    entries.retain(|(name, _)| !is_media(name));
    zip_stored(&entries, SystemTime::now())
}

pub fn is_media(name: &str) -> bool {
    let lower = name.to_lowercase();
    MEDIA_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// CRC-32 (IEEE, the zip one).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// A zip archive of `entries` (name, bytes), stored without compression:
/// the logs are small, and no compression code is needed (PKWARE APPNOTE
/// 6.3: local headers, central directory, end record; UTF-8 names).
pub fn zip_stored(entries: &[(String, Vec<u8>)], at: SystemTime) -> Vec<u8> {
    let local = safety_log::local(at);
    let dos_time = ((local.hour() as u16) << 11) | ((local.minute() as u16) << 5) | (local.second() as u16 / 2);
    let dos_date = (((local.year() - 1980).clamp(0, 127) as u16) << 9) | ((local.month() as u16) << 5) | local.day() as u16;
    let (mut out, mut central) = (Vec::new(), Vec::new());
    let u16le = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let u32le = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for (name, data) in entries {
        let (crc, size, offset) = (crc32(data), data.len() as u32, out.len() as u32);
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20); // version needed
        u16le(&mut out, 0x0800); // UTF-8 names
        u16le(&mut out, 0); // stored
        u16le(&mut out, dos_time);
        u16le(&mut out, dos_date);
        u32le(&mut out, crc);
        u32le(&mut out, size);
        u32le(&mut out, size);
        u16le(&mut out, name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        u32le(&mut central, 0x0201_4b50);
        u16le(&mut central, 20); // made by
        u16le(&mut central, 20);
        u16le(&mut central, 0x0800);
        u16le(&mut central, 0);
        u16le(&mut central, dos_time);
        u16le(&mut central, dos_date);
        u32le(&mut central, crc);
        u32le(&mut central, size);
        u32le(&mut central, size);
        u16le(&mut central, name.len() as u16);
        u16le(&mut central, 0); // extra
        u16le(&mut central, 0); // comment
        u16le(&mut central, 0); // disk
        u16le(&mut central, 0); // internal attributes
        u32le(&mut central, 0); // external attributes
        u32le(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let (cd_offset, cd_size) = (out.len() as u32, central.len() as u32);
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, entries.len() as u16);
    u16le(&mut out, entries.len() as u16);
    u32le(&mut out, cd_size);
    u32le(&mut out, cd_offset);
    u16le(&mut out, 0);
    out
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// A disk that never returns: every write blocks forever.
    pub struct StuckSink;
    impl Sink for StuckSink {
        fn append(&mut self, _: &str, _: &str) -> std::io::Result<()> {
            loop {
                std::thread::park();
            }
        }
    }

    /// A log whose writer is stuck for good.
    pub fn stuck() -> AppLog {
        AppLog::spawn(StuckSink, None, &LogSettings::default(), DAY_CAP, Vec::new())
    }

    /// Installs a stuck log as the process's logger (file side only, no
    /// terminal), once. For the engine cadence test.
    pub fn install_stuck_global() -> Arc<AppLog> {
        static APP: OnceLock<Arc<AppLog>> = OnceLock::new();
        Arc::clone(APP.get_or_init(|| {
            let app = Arc::new(stuck());
            if log::set_boxed_logger(Box::new(Logger { term: None, app: Arc::clone(&app) })).is_ok() {
                log::set_max_level(LevelFilter::Info);
            }
            app
        }))
    }

    /// The entries of a stored zip: (name, data), checking every CRC and
    /// that the central directory matches the local headers.
    pub fn read_zip(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let u16at = |i: usize| u16::from_le_bytes([zip[i], zip[i + 1]]) as usize;
        let u32at = |i: usize| u32::from_le_bytes([zip[i], zip[i + 1], zip[i + 2], zip[i + 3]]) as usize;
        let end = zip.len() - 22;
        assert_eq!(u32at(end), 0x0605_4b50, "end of central directory");
        let (count, mut at) = (u16at(end + 10), u32at(end + 16));
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32at(at), 0x0201_4b50);
            let (crc, size, name_len, offset) = (u32at(at + 16) as u32, u32at(at + 20), u16at(at + 28), u32at(at + 42));
            let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32at(offset), 0x0403_4b50);
            assert_eq!(u16at(offset + 8), 0, "stored");
            let data_at = offset + 30 + u16at(offset + 26) + u16at(offset + 28);
            let data = zip[data_at..data_at + size].to_vec();
            assert_eq!(crc32(&data), crc, "CRC of {name}");
            out.push((name, data));
            at += 46 + name_len + u16at(at + 30) + u16at(at + 32);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use log::Log;
    use std::sync::atomic::Ordering;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-applog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn noon(day: &str) -> SystemTime {
        format!("{day}T12:00:00Z").parse::<jiff::Timestamp>().unwrap().into()
    }

    fn file_log(dir: &Path, cap: u64) -> AppLog {
        AppLog::spawn(DirSink::named(dir.to_path_buf(), APP_FILES), Some(dir.to_path_buf()), &LogSettings::default(), cap, Vec::new())
    }

    fn read(dir: &Path, day: &str) -> String {
        std::fs::read_to_string(APP_FILES.file(dir, day)).unwrap_or_default()
    }

    #[test]
    fn lines_are_stamped_and_rotate_by_utc_day() {
        let dir = temp_dir("rotate");
        let log = file_log(&dir, DAY_CAP);
        log.push_at(noon("2026-03-01"), Level::Info, "laser_studio::midi::worker", "MIDI : APC40 connecté".into());
        log.push_at(noon("2026-03-01"), Level::Warn, "laser_studio::output", "erreur\nsur deux lignes".into());
        log.push_at(noon("2026-03-02"), Level::Info, "laser_studio", "Arrêt du studio".into());
        assert!(log.flush(Duration::from_secs(2)));
        let day1 = read(&dir, "2026-03-01");
        let lines: Vec<&str> = day1.lines().collect();
        assert_eq!(lines.len(), 3, "{day1}");
        assert!(lines[0].ends_with(" INFO  midi::worker: MIDI : APC40 connecté"), "{}", lines[0]);
        assert!(lines[0][..29].parse::<jiff::Timestamp>().is_ok(), "{}", lines[0]);
        assert!(lines[1].contains("WARN  output: erreur") && lines[2] == "    sur deux lignes");
        assert!(read(&dir, "2026-03-02").contains("Arrêt du studio"));
        assert_eq!(log.status()["written"], 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_files_are_purged_at_startup_and_on_a_new_day_never_more_than_fourteen() {
        let dir = temp_dir("prune");
        std::fs::create_dir_all(&dir).unwrap();
        let start: jiff::civil::Date = "2026-03-01".parse().unwrap();
        for i in 0..20 {
            let day = start.checked_add(jiff::Span::new().days(i)).unwrap();
            std::fs::write(APP_FILES.file(&dir, &day.to_string()), "x\n").unwrap();
        }
        // Not ours: kept, the safety log included.
        for name in ["safety-2020-01-01.jsonl", "notes.txt", "app-old.log"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let count = || safety_log::days_of(&dir, APP_FILES).len();
        assert_eq!(prune(&dir, "2026-03-20".parse().unwrap(), 14), 6);
        assert_eq!(count(), 14);
        assert_eq!(safety_log::days_of(&dir, APP_FILES).last().unwrap(), "2026-03-07");
        // The writer purges when the day changes (a simulated date).
        let log = file_log(&dir, DAY_CAP);
        log.push_at(noon("2026-03-25"), Level::Info, "t", "nouveau jour".into());
        log.flush(Duration::from_secs(2));
        // 2026-03-12..20 kept, 03-25 new: the 14-day window starts at 03-12.
        assert_eq!(count(), 10);
        assert_eq!(safety_log::days_of(&dir, APP_FILES).last().unwrap(), "2026-03-12");
        // A clock set back: « future » files count too, never more than keep.
        assert_eq!(prune(&dir, "2026-01-01".parse().unwrap(), 4), 6);
        assert_eq!(safety_log::days_of(&dir, APP_FILES), ["2026-03-25", "2026-03-20", "2026-03-19", "2026-03-18"]);
        for name in ["safety-2020-01-01.jsonl", "notes.txt", "app-old.log"] {
            assert!(dir.join(name).exists(), "{name}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_day_is_capped_with_one_truncation_line() {
        let dir = temp_dir("cap");
        let log = file_log(&dir, 1_000);
        for i in 0..100 {
            log.push_at(noon("2026-03-01"), Level::Info, "t", format!("ligne {i}"));
        }
        log.push_at(noon("2026-03-02"), Level::Info, "t", "le lendemain".into());
        log.flush(Duration::from_secs(2));
        let text = read(&dir, "2026-03-01");
        assert!(text.len() < 1_200, "{}", text.len());
        assert_eq!(text.matches("journal tronqué").count(), 1);
        assert!(text.trim_end().ends_with("reprise demain"));
        assert!(read(&dir, "2026-03-02").contains("le lendemain"));
        assert_eq!(log.status()["truncated"], "2026-03-01");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_queue_drops_and_counts_without_waiting() {
        let app = Arc::new(stuck());
        let logger = Logger { term: None, app: Arc::clone(&app) };
        let mut took: Vec<Duration> = (0..3_000)
            .map(|i| {
                let t = Instant::now();
                logger.log(&log::Record::builder().level(Level::Info).target("laser_studio::t").args(format_args!("ligne {i}")).build());
                t.elapsed()
            })
            .collect();
        took.sort_unstable();
        // Under 1 ms each; a busy test machine may preempt a rare call, so
        // the 99th percentile is held to it and the worst to a frame.
        assert!(took[2_970] < Duration::from_millis(1), "99 % of the calls under 1 ms, got {:?}", took[2_970]);
        assert!(took[2_999] < Duration::from_millis(16), "worst {:?}", took[2_999]);
        let dropped = app.status()["dropped"].as_u64().unwrap();
        assert!(dropped >= 3_000 - 1_025, "{dropped}");
        assert!(!app.flush(Duration::from_millis(20)), "a reader gives up, never hangs");
        // Below the level: not even queued.
        assert!(!logger.enabled(&log::Metadata::builder().level(Level::Debug).build()));
    }

    #[test]
    fn the_overflow_is_written_when_the_disk_comes_back() {
        let dir = temp_dir("overflow");
        let log = file_log(&dir, DAY_CAP);
        log.queue.as_ref().unwrap().dropped.store(5, Ordering::Relaxed);
        log.push_at(noon("2026-03-01"), Level::Info, "t", "après".into());
        log.flush(Duration::from_secs(2));
        let text = read(&dir, "2026-03-01");
        assert!(text.lines().next().unwrap().contains("5 ligne(s) perdue(s)"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_read_only_folder_is_reported_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("readonly");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let log = AppLog::start(dir.clone(), &LogSettings::default(), Vec::new());
        let t = Instant::now();
        for i in 0..100 {
            log.push(Level::Info, "t", format!("ligne {i}"));
        }
        assert!(t.elapsed() < Duration::from_millis(50));
        assert!(log.flush(Duration::from_secs(2)));
        let status = log.status();
        assert_eq!(status["write_failed"], true, "{status}");
        assert_eq!(status["failed"], 100);
        assert!(log.tail(10).is_empty());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_disabled_log_writes_nothing() {
        let dir = temp_dir("off");
        let log = AppLog::start(dir.clone(), &LogSettings { enabled: false, ..Default::default() }, Vec::new());
        assert!(!log.wants(Level::Error));
        log.push(Level::Error, "t", "x".into());
        assert!(log.flush(Duration::from_millis(10)));
        assert!(!dir.exists());
        assert_eq!(log.status()["enabled"], false);
    }

    #[test]
    fn settings_come_from_prefs_json_with_defaults() {
        let dir = temp_dir("prefs");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(LogSettings::load(&dir).keep_days, 14);
        std::fs::write(dir.join("prefs.json"), r#"{"autosave":{"delay_s":60},"log":{"level":"warn"}}"#).unwrap();
        let s = LogSettings::load(&dir);
        assert_eq!((s.enabled, s.keep_days, s.level_filter()), (true, 14, LevelFilter::Warn));
        assert_eq!(LogSettings { level: "bavard".into(), ..Default::default() }.level_filter(), LevelFilter::Info);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paths_are_redacted_in_the_file() {
        let dir = temp_dir("redact");
        let redactions = vec![("/Users/someone/shows/data".to_string(), "<data-dir>".to_string()), ("/Users/someone".to_string(), "~".to_string())];
        let log = AppLog::spawn(DirSink::named(dir.clone(), APP_FILES), Some(dir.clone()), &LogSettings::default(), DAY_CAP, redactions);
        log.push_at(noon("2026-03-01"), Level::Warn, "t", "failed to save /Users/someone/shows/data/live.json and /Users/someone/x".into());
        log.flush(Duration::from_secs(2));
        assert!(read(&dir, "2026-03-01").contains("failed to save <data-dir>/live.json and ~/x"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tail_returns_the_last_lines_across_two_days() {
        let dir = temp_dir("tail");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(APP_FILES.file(&dir, "2026-03-01"), "a\nb\nc\n").unwrap();
        std::fs::write(APP_FILES.file(&dir, "2026-03-02"), "d\ne\n").unwrap();
        assert_eq!(tail_lines(&dir, 3), ["c", "d", "e"]);
        assert_eq!(tail_lines(&dir, 1), ["e"]);
        assert_eq!(tail_lines(&dir, 50).len(), 5);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crc32_matches_the_reference() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    fn parts(project: bool) -> ReportParts {
        ReportParts {
            devices: json!({ "audio": [], "midi": [] }),
            settings: json!({ "safety": { "max_brightness": 0.5 } }),
            state: json!({ "armed": false, "dir": "/Users/someone/shows/data/autosave" }),
            project: project.then(|| json!({ "name": "Soirée" })),
        }
    }

    #[test]
    fn the_report_is_a_valid_zip_without_media_and_the_project_only_on_request() {
        let dir = temp_dir("report");
        std::fs::create_dir_all(&dir).unwrap();
        // Media lying around in the logs folder are never picked up.
        for name in ["song.wav", "show.ild", "logo.svg", "pic.png", "app-2026-03-01.log.mp3"] {
            std::fs::write(dir.join(name), "media").unwrap();
        }
        std::fs::write(safety_log::file_of(&dir, "2026-03-01"), "{\"kind\":\"arm\"}\n").unwrap();
        let redactions = vec![("/Users/someone/shows/data".to_string(), "<data-dir>".to_string())];
        let log = AppLog::spawn(DirSink::named(dir.clone(), APP_FILES), Some(dir.clone()), &LogSettings::default(), DAY_CAP, redactions);
        log.push(Level::Info, "t", "Démarrage".into());
        let entries = read_zip(&report(&log, parts(false)));
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        let today = format!("logs/app-{}.log", utc_day(SystemTime::now()));
        assert!(names.contains(&today.as_str()), "{names:?}");
        assert!(names.contains(&"logs/safety-2026-03-01.jsonl"));
        for name in ["about.json", "devices.json", "settings.json", "state.json"] {
            assert!(names.contains(&name), "{name} in {names:?}");
        }
        assert!(!names.contains(&"project.json"));
        assert!(names.iter().all(|n| !is_media(n)), "{names:?}");
        let about: Value = serde_json::from_slice(&entries.iter().find(|(n, _)| n == "about.json").unwrap().1).unwrap();
        assert_eq!(about["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(about["project_included"], false);
        assert!(about["log"].get("dir").is_none(), "no absolute path");
        let state = String::from_utf8(entries.iter().find(|(n, _)| n == "state.json").unwrap().1.clone()).unwrap();
        assert!(state.contains("<data-dir>/autosave") && !state.contains("/Users/someone"), "{state}");

        let with = read_zip(&report(&log, parts(true)));
        assert!(with.iter().any(|(n, d)| n == "project.json" && String::from_utf8_lossy(d).contains("Soirée")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
