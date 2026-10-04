//! T-253: a studio stopped by SIGTERM or Ctrl+C (SIGINT) disarms, sends
//! dark frames and closes its output before it exits.
//!
//! Runs the real binary in a subprocess, preview-only apart from the
//! hidden `--test-output` fake output (a text file, never a laser), with
//! `--no-midi`, `--no-audio`, a free localhost port and a temporary data directory.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Studio {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Studio {
    fn start(tag: &str) -> Self {
        Self::start_with(tag, &[])
    }

    fn start_with(tag: &str, extra: &[&str]) -> Self {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_ne!(port, 8080, "never the user's instance");
        let dir = std::env::temp_dir().join(format!("laser-studio-shutdown-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output.log");
        let child = Command::new(env!("CARGO_BIN_EXE_laser-studio"))
            .args(["--no-midi", "--no-audio", "--port", &port.to_string(), "--data-dir"])
            .arg(&dir)
            .arg("--test-output")
            .arg(&log)
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let studio = Self { child, port, dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        while studio.request("GET", "/api/state", "").is_none() {
            assert!(Instant::now() < deadline, "studio did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        studio
    }

    fn request(&self, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).ok()?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).ok()?;
        let status = raw.split(' ').nth(1)?.parse().ok()?;
        Some((status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()))
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("output.log")).unwrap_or_default().lines().map(String::from).collect()
    }

    fn wait_for_log(&self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.log().iter().any(|l| l == line) {
            assert!(Instant::now() < deadline, "no {line:?} in {:?}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts disarmed, then a page beats and arms: the fake output is lit.
    fn arm(&self) {
        let (_, state) = self.request("GET", "/api/state", "").unwrap();
        assert!(state.contains(r#""armed":false"#), "starts disarmed: {state}");
        assert_eq!(self.request("POST", "/api/heartbeat", r#"{"client_id":"t","visible":true}"#).unwrap().0, 200);
        assert_eq!(self.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard","client_id":"t"}"#).unwrap().0, 200);
        self.wait_for_log("lit");
    }

    /// The kinds of the safety log's lines (T-259), all days.
    fn safety_log(&self) -> Vec<String> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.dir.join("logs"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("safety-"))).collect())
            .unwrap_or_default();
        files.sort();
        let text: String = files.iter().map(|f| std::fs::read_to_string(f).unwrap_or_default()).collect();
        text.lines().map(|l| l.split("\"kind\":\"").nth(1).and_then(|k| k.split('"').next()).unwrap_or("?").to_string()).collect()
    }

    fn signal_and_wait(self, signal: &str) -> Vec<String> {
        self.signal_and_wait_with_safety_log(signal).0
    }

    fn signal_and_wait_with_safety_log(self, signal: &str) -> (Vec<String>, Vec<String>) {
        let (log, safety, _) = self.signal_and_wait_with_logs(signal);
        (log, safety)
    }

    /// The application log (T-403), all `app-*.log` files in order.
    fn app_log(&self) -> String {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.dir.join("logs"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-"))).collect())
            .unwrap_or_default();
        files.sort();
        files.iter().map(|f| std::fs::read_to_string(f).unwrap_or_default()).collect()
    }

    fn signal_and_wait_with_logs(self, signal: &str) -> (Vec<String>, Vec<String>, String) {
        let status = Command::new("kill").args([signal, &self.child.id().to_string()]).status().unwrap();
        assert!(status.success());
        self.wait_with_logs(signal)
    }

    /// Waits for a clean exit (`why` names the cause for the message).
    fn wait_with_logs(mut self, why: &str) -> (Vec<String>, Vec<String>, String) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(code) = self.child.try_wait().unwrap() {
                assert!(code.success(), "clean exit, got {code}");
                break;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("studio did not exit after {why}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let (log, safety, app) = (self.log(), self.safety_log(), self.app_log());
        let _ = std::fs::remove_dir_all(&self.dir);
        (log, safety, app)
    }
}

fn assert_clean_shutdown(log: &[String]) {
    let tail: Vec<&str> = log.iter().rev().take(5).rev().map(String::as_str).collect();
    assert_eq!(tail, ["disarm", "blank", "blank", "blank", "close"], "full log: {log:?}");
}

#[test]
fn sigterm_disarms_sends_dark_frames_and_closes_the_output() {
    let studio = Studio::start("term");
    studio.arm();
    let (log, safety) = studio.signal_and_wait_with_safety_log("-TERM");
    assert_clean_shutdown(&log);
    // The safety log (T-259) has the whole run, the shutdown disarm included.
    assert_eq!(safety, ["app_start", "presence", "arm", "disarm", "app_stop"]);
}

#[test]
fn ctrl_c_disarms_sends_dark_frames_and_closes_the_output() {
    let studio = Studio::start("int");
    studio.arm();
    assert_clean_shutdown(&studio.signal_and_wait("-INT"));
}

/// T-403: the application log on disk has the start and the clean stop,
/// time-stamped, and the arming in between with its source; no path of
/// the data directory leaks into it.
#[test]
fn the_application_log_has_the_start_the_arming_and_the_stop() {
    let studio = Studio::start("applog");
    let dir = studio.dir.to_string_lossy().into_owned();
    studio.arm();
    let (_, _, app) = studio.signal_and_wait_with_logs("-TERM");
    let lines: Vec<&str> = app.lines().collect();
    let find = |what: &str| lines.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("no {what:?} in {app}"));
    let (start, armed, disarmed, stop) = (find("Démarrage de Laser Studio"), find("Laser armé"), find("Laser désarmé"), find("laser_studio: Arrêt du studio"));
    assert!(start < armed && armed < disarmed && disarmed < stop, "{app}");
    assert!(lines[start].contains("--no-midi") && lines[start].contains("sortie : "), "{}", lines[start]);
    // RFC 3339 with milliseconds and offset at the start of every line.
    for i in [start, stop] {
        assert!(lines[i].get(..29).is_some_and(|ts| ts.as_bytes()[10] == b'T' && ts.as_bytes()[19] == b'.'), "{}", lines[i]);
    }
    assert!(!app.contains(&dir), "no data-dir path in the log");
}

/// T-413: « Quitter Laser Studio » in the menu-bar item of the application
/// takes the same path as SIGTERM. The native item can't be clicked by a
/// test: the hidden hook runs the same action.
#[test]
fn quitting_from_the_menu_bar_disarms_sends_dark_frames_and_closes_the_output() {
    let studio = Studio::start_with("menu", &["--app", "--no-open", "--test-hooks"]);
    let (status, menu) = studio.request("GET", "/api/test/menu", "").unwrap();
    assert_eq!(status, 200, "{menu}");
    assert!(menu.contains("Désarmé — "), "starts disarmed: {menu}");
    assert!(menu.contains("Quitter Laser Studio") && menu.contains(r#""action":"estop""#), "{menu}");
    assert!(!menu.contains(r#""action":"arm"#), "nothing arms: {menu}");
    studio.arm();
    let (_, menu) = studio.request("GET", "/api/test/menu", "").unwrap();
    assert!(menu.contains("ARMÉ") && menu.contains(r#""icon":"armed""#), "{menu}");
    assert_eq!(studio.request("POST", "/api/test/menu?action=arm", "").unwrap().0, 400, "no arming entry");
    assert_eq!(studio.request("POST", "/api/test/menu?action=quit", "").unwrap().0, 200);
    let (log, safety, _) = studio.wait_with_logs("Quitter");
    assert_clean_shutdown(&log);
    assert_eq!(safety, ["app_start", "presence", "arm", "disarm", "app_stop"]);
}

/// T-413: « Arrêt » in the menu latches the emergency stop, like Escape;
/// the menu shows it.
#[test]
fn the_menu_bar_stop_latches_the_emergency_stop() {
    let studio = Studio::start_with("menu-estop", &["--app", "--no-open", "--test-hooks"]);
    studio.arm();
    assert_eq!(studio.request("POST", "/api/test/menu?action=estop", "").unwrap().0, 200);
    let (_, state) = studio.request("GET", "/api/state", "").unwrap();
    assert!(state.contains(r#""armed":false"#), "{state}");
    let (_, menu) = studio.request("GET", "/api/test/menu", "").unwrap();
    assert!(menu.contains("Arrêt d'urgence") && menu.contains(r#""icon":"stopped""#), "{menu}");
    studio.signal_and_wait("-TERM");
}
