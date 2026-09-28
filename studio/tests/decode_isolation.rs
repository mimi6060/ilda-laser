//! T-298: an audio decoder that panics, aborts or hangs can't stop the
//! studio or touch the laser gate. Runs the real binary, preview-only apart
//! from the hidden `--test-output` fake output (a text file, never a
//! laser), with `--no-midi`, `--no-audio`, a free localhost port and a
//! temporary data directory. `--test-hooks` makes files starting with
//! `LSPANIC!`, `LSABORT!` or `LSHANG!!` crash the decoder on purpose.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_laser-studio");

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laser-studio-decode-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 16-bit mono WAV of a ramp, `frames` long.
fn wav(frames: usize) -> Vec<u8> {
    let mut b = b"RIFF".to_vec();
    b.extend((36 + 2 * frames as u32).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(8_000u32.to_le_bytes());
    b.extend(16_000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((2 * frames as u32).to_le_bytes());
    for i in 0..frames {
        b.extend(((i % 200) as i16 * 100).to_le_bytes());
    }
    b
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

struct Studio {
    child: Child,
    port: u16,
    dir: PathBuf,
    beating: Arc<AtomicBool>,
}

impl Studio {
    /// Started with a 2 s decoding limit, and a page beating every 300 ms.
    fn start(tag: &str) -> Self {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_ne!(port, 8080, "never the user's instance");
        let dir = temp_dir(tag);
        let child = Command::new(BIN)
            .args(["--no-midi", "--no-audio", "--test-hooks", "--test-decode-timeout-ms", "2000", "--port", &port.to_string(), "--data-dir"])
            .arg(&dir)
            .arg("--test-output")
            .arg(dir.join("output.log"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while request(port, "GET", "/api/state", b"").is_none() {
            assert!(Instant::now() < deadline, "studio did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        let beating = Arc::new(AtomicBool::new(true));
        std::thread::spawn({
            let beating = Arc::clone(&beating);
            move || {
                while beating.load(Ordering::SeqCst) {
                    request(port, "POST", "/api/heartbeat", br#"{"client_id":"t","visible":true}"#);
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        });
        Self { child, port, dir, beating }
    }

    fn get(&self, path: &str) -> String {
        let t = Instant::now();
        let (status, body) = request(self.port, "GET", path, b"").expect("the studio answers");
        assert_eq!(status, 200, "{path}: {body}");
        assert!(t.elapsed() < Duration::from_millis(1500), "{path} answered in {:?}", t.elapsed());
        body
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("output.log")).unwrap_or_default().lines().map(String::from).collect()
    }

    fn arm(&self) {
        assert!(self.get("/api/state").contains(r#""armed":false"#), "starts disarmed");
        assert_eq!(request(self.port, "POST", "/api/arm", br#"{"on":true,"source":"keyboard","client_id":"t"}"#).unwrap().0, 200);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.log().iter().any(|l| l == "lit") {
            assert!(Instant::now() < deadline, "not lit: {:?}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Still running, still armed, the output never disarmed or blanked.
    fn assert_unaffected(&mut self) {
        assert!(self.child.try_wait().unwrap().is_none(), "the studio is still running");
        let state = self.get("/api/state");
        assert!(state.contains(r#""armed":true"#), "still armed: {state}");
        let log = self.log();
        assert!(!log.iter().any(|l| l == "disarm" || l == "blank" || l == "close"), "the output was never touched: {log:?}");
        assert_eq!(log.last().map(String::as_str), Some("lit"), "the output still gets lit frames: {log:?}");
        let frame = self.get("/api/frame");
        assert!(frame.contains(r#""output_lit":"#) && !frame.contains(r#""output_lit":0,"#) && !frame.contains(r#""output_lit":0}"#), "lit points sent to the output");
    }

    fn import(&self, name: &str, bytes: &[u8]) -> (u16, String) {
        request(self.port, "POST", &format!("/api/media/audio?name={name}"), bytes).expect("an answer")
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        self.beating.store(false, Ordering::SeqCst);
        let _ = request(self.port, "POST", "/api/arm", br#"{"on":false}"#);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn media_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("media/audio"))
        .map(|d| d.filter_map(|e| e.ok()?.file_name().to_str().map(String::from)).filter(|f| f != ".peaks").collect())
        .unwrap_or_default()
}

#[test]
fn a_decoder_that_panics_aborts_or_hangs_leaves_the_armed_studio_running() {
    let mut studio = Studio::start("crash");
    studio.arm();

    for (magic, reason) in [(&b"LSPANIC!"[..], "le décodeur a planté"), (b"LSABORT!", "arrêté brutalement (signal 6)")] {
        let (status, body) = studio.import("piege.mp3", &[magic, b" crafted file"].concat());
        assert_eq!(status, 400, "{body}");
        assert!(body.contains(reason) && body.contains("le studio continue"), "{body}");
        assert!(media_files(&studio.dir).is_empty(), "nothing kept: {:?}", media_files(&studio.dir));
        studio.assert_unaffected();
    }

    // A hanging decoder: killed at the limit (2 s here). Meanwhile the API
    // answers and the heartbeats keep flowing, so the laser stays armed.
    let port = studio.port;
    let pending = std::thread::spawn(move || request(port, "POST", "/api/media/audio?name=lent.mp3", b"LSHANG!! crafted file"));
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(1500) {
        studio.get("/api/state");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (status, body) = pending.join().unwrap().expect("an answer");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("décodage trop long (plus de 2 s)"), "{body}");
    assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
    assert!(media_files(&studio.dir).is_empty());
    studio.assert_unaffected();

    // A real song still imports, through the same child, and its waveform works.
    let (status, body) = studio.import("bon.wav", &wav(8_000));
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""duration_s":1.0"#), "{body}");
    assert!(studio.get("/api/timeline/waveform?file=bon.wav&from=0&to=1&px=10").contains(r#""duration_s":1.0"#));
    assert_eq!(media_files(&studio.dir), vec!["bon.wav".to_string()]);
    studio.assert_unaffected();
}

/// The `--decode` child itself: reads only songs of `media/audio/`, and
/// answers in the reply format (an error reply, never a crash, for a bad name).
#[test]
fn the_decoder_child_reads_only_the_media_folder() {
    let dir = temp_dir("child");
    let media = dir.join("media/audio");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(dir.join("secret.wav"), wav(100)).unwrap();
    std::fs::write(media.join("bon.wav"), wav(1000)).unwrap();
    std::os::unix::fs::symlink(dir.join("secret.wav"), media.join("lien.wav")).unwrap();
    let run = |args: &[&str]| Command::new(BIN).args(args).arg("--data-dir").arg(&dir).output().unwrap();
    let reply = |name: &str| {
        let out = run(&["--decode", name]);
        assert!(out.status.success(), "{name}: {out:?}");
        assert_eq!(&out.stdout[..8], b"LSDECOD1", "{name}");
        out.stdout
    };
    for (name, why) in [
        ("../secret.wav", "nom de morceau invalide"),
        ("/etc/passwd", "nom de morceau invalide"),
        (".peaks", "nom de morceau invalide"),
        ("lien.wav", "pas un fichier ordinaire"),
        ("absent.wav", "morceau introuvable"),
    ] {
        let r = reply(name);
        assert_eq!(r[8], 0, "{name}: an error reply");
        assert!(String::from_utf8_lossy(&r[13..]).contains(why), "{name}: {}", String::from_utf8_lossy(&r[13..]));
    }
    let samples = reply("bon.wav");
    assert_eq!(samples[8], 1);
    assert_eq!(samples.len(), 8 + 1 + 4 + 2 + 8 + 2 * 1000);
    let peaks = run(&["--decode", "bon.wav", "--decode-peaks"]);
    assert_eq!(peaks.stdout[8], 2);
    // Without --test-hooks a crafted file is just not a song.
    std::fs::write(media.join("piege.mp3"), b"LSPANIC! crafted").unwrap();
    let r = reply("piege.mp3");
    assert!(r[8] == 0 && String::from_utf8_lossy(&r[13..]).contains("format audio non reconnu"));
    // Test-only flags don't go alone.
    assert!(!run(&["--decode-peaks"]).status.success());
    assert!(!run(&["--test-decode-timeout-ms", "10", "--list-controls"]).status.success());
    let _ = std::fs::remove_dir_all(&dir);
}
