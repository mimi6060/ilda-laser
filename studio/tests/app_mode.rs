//! T-413: the application mode (`--app`) of the real binary, in a
//! subprocess: preview only, `--no-open` (the user's browser is never
//! opened), `--no-midi`, `--no-audio`, explicit free localhost ports and a
//! temporary data directory (never the user's Application Support).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_ne!(port, 8080, "never the user's instance");
    port
}

fn app(dir: &Path, port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_laser-studio"));
    cmd.args(["--app", "--no-open", "--no-midi", "--no-audio", "--port", &port.to_string(), "--data-dir"])
        .arg(dir)
        .arg("--exit-with-parent")
        .arg(std::process::id().to_string());
    cmd
}

fn get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    Some(raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-TERM", &self.0.id().to_string()]).status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.0.kill();
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laser-studio-app-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Starts disarmed after a run that did not end cleanly, and a second
/// launch on the same data shows the first one's address and exits
/// without starting a second engine.
#[test]
fn starts_disarmed_after_a_crash_and_a_second_launch_defers_to_the_first() {
    let dir = temp_dir("second");
    // What a crashed run leaves: its lock details, no lock.
    std::fs::write(dir.join(".lock"), r#"{"pid":4321,"port":8080,"started_at":"2026-10-01 20:00:00"}"#).unwrap();
    let port = free_port();
    let first = Running(app(&dir, port).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        if let Some(state) = get(port, "/api/state") {
            break state;
        }
        assert!(Instant::now() < deadline, "studio did not start");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(state.contains(r#""armed":false"#), "starts disarmed: {state}");

    let other = free_port();
    let second = app(&dir, other).output().unwrap();
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(second.status.success(), "{stdout} {}", String::from_utf8_lossy(&second.stderr));
    assert!(stdout.contains(&format!("Laser Studio tourne déjà (PID {}) : http://127.0.0.1:{port}/", first.0.id())), "{stdout}");
    assert!(get(other, "/api/state").is_none(), "no second studio");
    assert!(get(port, "/api/state").is_some(), "the first one still runs");
    drop(first);
    let _ = std::fs::remove_dir_all(dir);
}
