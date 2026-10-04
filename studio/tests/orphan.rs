//! T-358: a test studio started with the hidden `--exit-with-parent`
//! stops by itself, through the clean shutdown (disarm, dark frames,
//! close), when the process that started it dies or its temporary data
//! directory is deleted: an interrupted e2e run leaves no studio behind.
//!
//! Preview-only apart from the hidden `--test-output` fake output (a text
//! file, never a laser), with `--no-midi`, `--no-audio`, a free localhost
//! port and a temporary data directory.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_ne!(port, 8080, "never the user's instance");
    port
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laser-studio-orphan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request(port: u16, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let status = raw.split(' ').nth(1)?.parse().ok()?;
    Some((status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()))
}

fn wait_listening(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while request(port, "GET", "/api/state", "").is_none() {
        assert!(Instant::now() < deadline, "studio did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts disarmed, then a page beats and arms: the fake output is lit.
fn arm(port: u16, log: &Path) {
    let (_, state) = request(port, "GET", "/api/state", "").unwrap();
    assert!(state.contains(r#""armed":false"#), "starts disarmed: {state}");
    assert_eq!(request(port, "POST", "/api/heartbeat", r#"{"client_id":"t","visible":true}"#).unwrap().0, 200);
    assert_eq!(request(port, "POST", "/api/arm", r#"{"on":true,"source":"keyboard","client_id":"t"}"#).unwrap().0, 200);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !read_log(log).iter().any(|l| l == "lit") {
        assert!(Instant::now() < deadline, "never lit: {:?}", read_log(log));
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_log(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(String::from).collect()
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
}

fn assert_clean_shutdown(log: &[String]) {
    let tail: Vec<&str> = log.iter().rev().take(5).rev().map(String::as_str).collect();
    assert_eq!(tail, ["disarm", "blank", "blank", "blank", "close"], "full log: {log:?}");
}

/// The harness (here a shell standing in for a Playwright worker) is
/// killed without warning: its studio is gone within 5 s, after a clean
/// shutdown.
#[test]
fn a_studio_whose_parent_dies_exits_within_5_s() {
    let dir = temp_dir("parent");
    let data = dir.join("data");
    let log = dir.join("output.log");
    let port = free_port();
    // The shell starts the studio in the background with its own PID ($$),
    // prints the studio's PID and waits: it is the studio's parent.
    let mut shell = Command::new("sh")
        .arg("-c")
        .arg(r#"bin="$1"; shift; "$bin" "$@" --exit-with-parent $$ >/dev/null 2>&1 & echo $!; wait"#)
        .arg("sh")
        .arg(env!("CARGO_BIN_EXE_laser-studio"))
        .args(["--no-midi", "--no-audio", "--test-keep-awake", "--port", &port.to_string(), "--data-dir"])
        .arg(&data)
        .arg("--test-output")
        .arg(&log)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(shell.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let studio: u32 = line.trim().parse().unwrap_or_else(|_| panic!("no studio PID: {line:?}"));
    wait_listening(port);
    arm(port, &log);

    shell.kill().unwrap(); // SIGKILL: no chance to stop its child
    shell.wait().unwrap();
    let killed = Instant::now();
    while alive(studio) {
        if killed.elapsed() > Duration::from_secs(5) {
            let _ = Command::new("kill").args(["-9", &studio.to_string()]).status();
            panic!("the studio outlived its parent by 5 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_clean_shutdown(&read_log(&log));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The harness deleted the temporary data directory (its cleanup ran but
/// the stop did not): the studio stops too.
#[test]
fn a_studio_whose_data_dir_is_deleted_exits_within_5_s() {
    let dir = temp_dir("datadir");
    let data = dir.join("data");
    let log = dir.join("output.log");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_laser-studio"))
        .args(["--no-midi", "--no-audio", "--test-keep-awake", "--port", &port.to_string(), "--exit-with-parent", &std::process::id().to_string(), "--data-dir"])
        .arg(&data)
        .arg("--test-output")
        .arg(&log)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_listening(port);
    arm(port, &log);

    std::fs::remove_dir_all(&data).unwrap();
    let deleted = Instant::now();
    loop {
        if let Some(code) = child.try_wait().unwrap() {
            assert!(code.success(), "clean exit, got {code}");
            break;
        }
        if deleted.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("the studio outlived its data directory by 5 s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_clean_shutdown(&read_log(&log));
    let _ = std::fs::remove_dir_all(&dir);
}

/// While its parent lives and its data directory is there, a studio with
/// the option keeps running (it is stopped by SIGTERM here).
#[test]
fn a_studio_with_a_live_parent_keeps_running() {
    let dir = temp_dir("alive");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_laser-studio"))
        .args(["--no-midi", "--no-audio", "--test-keep-awake", "--port", &port.to_string(), "--exit-with-parent", &std::process::id().to_string(), "--data-dir"])
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_listening(port);
    std::thread::sleep(Duration::from_millis(1_000));
    assert!(child.try_wait().unwrap().is_none(), "still running");
    assert!(request(port, "GET", "/api/state", "").is_some());
    Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("did not stop on SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
