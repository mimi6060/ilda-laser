//! T-412: one studio per data folder. A second studio on the same folder
//! exits with code 2 and a French message; one killed with SIGKILL (a
//! crash) leaves a lock that the next start takes over; two folders, two
//! studios.
//!
//! Runs the real binary, preview only (no --device), with `--no-midi`,
//! `--no-audio`, `--test-keep-awake`, free localhost ports and temporary
//! data directories. Nothing is armed.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_ne!(port, 8080, "never the user's instance");
    port
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laser-studio-lock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn command(dir: &Path, port: u16) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_laser-studio"));
    c.args(["--no-midi", "--no-audio", "--test-keep-awake", "--port", &port.to_string(), "--data-dir"]).arg(dir);
    c
}

fn answers(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else { return false };
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    if write!(stream, "GET /api/state HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut raw = String::new();
    stream.read_to_string(&mut raw).is_ok() && raw.starts_with("HTTP/1.1 200")
}

/// A studio serving on `port`, killed on drop.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(dir: &Path) -> (Running, u16) {
    let port = free_port();
    let child = command(dir, port).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut running = Running(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !answers(port) {
        assert!(running.0.try_wait().unwrap().is_none(), "studio exited early");
        assert!(Instant::now() < deadline, "studio did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    (running, port)
}

/// Runs a studio that is expected to exit by itself (within 10 s).
fn run_to_exit(dir: &Path) -> Output {
    let child = command(dir, free_port()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(10);
    let waiter = std::thread::spawn(move || child.wait_with_output().unwrap());
    while !waiter.is_finished() {
        if Instant::now() > deadline {
            let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            panic!("the second studio did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    waiter.join().unwrap()
}

#[test]
fn a_second_studio_on_the_same_folder_exits_with_code_2() {
    let dir = temp_dir("same");
    let (first, port) = start(&dir);
    let second = run_to_exit(&dir);
    assert_eq!(second.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("Laser Studio utilise déjà ce dossier"), "{stderr}");
    assert!(stderr.contains(&format!("PID {}", first.0.id())) && stderr.contains(&format!("port {port}")), "{stderr}");
    assert!(stderr.contains("--data-dir"), "{stderr}");
    assert!(answers(port), "the first one is untouched");
    drop(first);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_crashed_studio_does_not_block_a_restart() {
    let dir = temp_dir("crash");
    let (mut first, _) = start(&dir);
    // SIGKILL: no clean shutdown, the .lock file keeps its details.
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    assert!(!std::fs::read_to_string(dir.join(".lock")).unwrap().is_empty());
    let (again, port) = start(&dir);
    assert!(answers(port));
    drop(again);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn two_folders_two_studios() {
    let (a, b) = (temp_dir("a"), temp_dir("b"));
    let (sa, pa) = start(&a);
    let (sb, pb) = start(&b);
    assert!(answers(pa) && answers(pb));
    drop((sa, sb));
    let _ = std::fs::remove_dir_all(a);
    let _ = std::fs::remove_dir_all(b);
}

#[test]
fn list_controls_does_not_need_the_lock() {
    let dir = temp_dir("list");
    let (studio, _) = start(&dir);
    let out = Command::new(env!("CARGO_BIN_EXE_laser-studio")).arg("--list-controls").arg("--data-dir").arg(&dir).output().unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
    drop(studio);
    let _ = std::fs::remove_dir_all(dir);
}
