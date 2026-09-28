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

    fn signal_and_wait(mut self, signal: &str) -> Vec<String> {
        let status = Command::new("kill").args([signal, &self.child.id().to_string()]).status().unwrap();
        assert!(status.success());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(code) = self.child.try_wait().unwrap() {
                assert!(code.success(), "clean exit, got {code}");
                break;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("studio did not exit after {signal}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let log = self.log();
        let _ = std::fs::remove_dir_all(&self.dir);
        log
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
    assert_clean_shutdown(&studio.signal_and_wait("-TERM"));
}

#[test]
fn ctrl_c_disarms_sends_dark_frames_and_closes_the_output() {
    let studio = Studio::start("int");
    studio.arm();
    assert_clean_shutdown(&studio.signal_and_wait("-INT"));
}
