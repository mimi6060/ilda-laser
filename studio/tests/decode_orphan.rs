//! T-359: an audio decoder child (`--decode`, T-298) never outlives its
//! studio: killed without warning, the studio leaves no decoder behind
//! after 5 s; stopped cleanly, it kills and reaps its decoders first; and a
//! decoder with no studio at all still stops at its own time limit.
//!
//! Preview-only: `--no-midi`, `--no-audio`, a free localhost port and a
//! temporary data directory. `--test-hooks` makes a file starting with
//! `LSHANG!!` hang the decoder.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_laser-studio");

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laser-studio-decode-orphan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request(port: u16, method: &str, path: &str, body: &[u8]) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).ok()?;
    stream.write_all(body).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let status = raw.split(' ').nth(1)?.parse().ok()?;
    Some((status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()))
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
}

fn kill_9(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).stderr(Stdio::null()).status();
}

/// A studio whose decoding limit (60 s) is far beyond the test: a hanging
/// decoder only stops through the T-359 mechanisms.
struct Studio {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Studio {
    fn start(tag: &str) -> Self {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_ne!(port, 8080, "never the user's instance");
        let dir = temp_dir(tag);
        let child = Command::new(BIN)
            .args(["--no-midi", "--no-audio", "--test-hooks", "--test-decode-timeout-ms", "60000", "--port", &port.to_string(), "--data-dir"])
            .arg(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while request(port, "GET", "/api/state", b"").is_none() {
            assert!(Instant::now() < deadline, "studio did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Self { child, port, dir }
    }

    /// Imports a « slow » song in the background and returns the PID of
    /// the hanging decoder child.
    fn start_slow_decode(&self) -> u32 {
        let port = self.port;
        std::thread::spawn(move || request(port, "POST", "/api/media/audio?name=lent.mp3", b"LSHANG!! crafted file"));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = Command::new("pgrep").args(["-P", &self.child.id().to_string(), "-f", "laser-studio --decode"]).output().unwrap();
            if let Some(pid) = String::from_utf8_lossy(&out.stdout).lines().next().and_then(|l| l.trim().parse().ok()) {
                return pid;
            }
            assert!(Instant::now() < deadline, "no decoder child started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_decoder_whose_studio_is_killed_exits_within_5_s() {
    let mut studio = Studio::start("killed");
    let decoder = studio.start_slow_decode();
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(decoder), "the decoder hangs while its studio lives");

    studio.child.kill().unwrap(); // SIGKILL: no chance to stop its child
    studio.child.wait().unwrap();
    let killed = Instant::now();
    while alive(decoder) {
        if killed.elapsed() > Duration::from_secs(5) {
            kill_9(decoder);
            panic!("the decoder outlived its studio by 5 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_clean_shutdown_kills_and_reaps_the_decoders() {
    let mut studio = Studio::start("clean");
    let decoder = studio.start_slow_decode();
    Command::new("kill").args(["-TERM", &studio.child.id().to_string()]).status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while studio.child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            kill_9(decoder);
            panic!("the studio did not stop on SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Killed and reaped before the studio exited, not later by its own watch.
    let still_there = alive(decoder);
    kill_9(decoder);
    assert!(!still_there, "the decoder survived the studio's clean shutdown");
}

/// A decoder started by hand (no studio watching it) with a 500 ms limit
/// hangs, and still stops by itself shortly after its limit.
#[test]
fn a_decoder_without_a_studio_keeps_its_time_limit() {
    let dir = temp_dir("limit");
    std::fs::create_dir_all(dir.join("media/audio")).unwrap();
    std::fs::write(dir.join("media/audio/lent.mp3"), b"LSHANG!! crafted file").unwrap();
    let mut child = Command::new(BIN)
        .args(["--decode", "lent.mp3", "--test-hooks", "--test-decode-timeout-ms", "500", "--data-dir"])
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the decoder ignored its time limit");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success(), "stopped, not answered: {status}");
    assert!(started.elapsed() >= Duration::from_millis(500), "not before its limit");
    let _ = std::fs::remove_dir_all(&dir);
}
