//! Isolated audio decoding (T-298): a damaged or crafted song file must
//! not be able to stop the studio.
//!
//! The decoders read the user's files, and `nanomp3` is a machine
//! translation of C with `unsafe` inside. Since T-253 a panic anywhere
//! stops the whole studio (the global hook cuts the output and shuts down),
//! which is right for the engine, the output or MIDI, but not for a song
//! import in the middle of a show. So the decoders never run inside the
//! studio's own threads:
//!
//! - **Child process** (the real studio, `Isolation::child`): the studio
//!   runs its own executable again as `laser-studio --decode <file>
//!   --data-dir <dir> [--decode-peaks]`. The child reads one file of
//!   `media/audio/` (a song name or an import in progress, never a path:
//!   `media::source_path`), decodes it and writes the result on its stdout
//!   (`write_reply`). A panic, an abort, a segfault or memory corruption in
//!   `unsafe` code end the child only; a decode that takes longer than the
//!   time limit is killed. The parent checks every size in the reply
//!   before it allocates anything (`read_reply`).
//! - **Contained thread** (`Isolation::Thread`: unit tests, or when the
//!   executable can't be found): the decoder runs on its own thread, marked
//!   so that the global panic hook lets its panic through
//!   (`panic_is_contained`), and the panic becomes an error. It only
//!   catches panics: not an abort, a segfault or a hang.
//!
//! Either way a failure is a French error for the UI and the studio (and
//! the laser) carries on.

use super::decode::{self, Decoded, WaveformPeaks, MAX_SECONDS, PEAK_BLOCK};
use super::media;
use anyhow::{anyhow, bail, Context, Result};
use std::cell::Cell;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Longest a decode may take (a 30 min MP3 takes a few seconds in a
/// release build): past it the child is killed and the file refused.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(120);
/// First bytes of every reply on the child's stdout.
const MAGIC: &[u8; 8] = b"LSDECOD1";
/// Longest error message a child may send back.
const MAX_MESSAGE: usize = 4096;

/// What the caller needs: the samples (playback) or only the waveform and
/// the format (import, waveform view), which is much less to send back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Samples,
    Peaks,
}

#[derive(Debug, PartialEq)]
pub enum Reply {
    Samples(Decoded),
    Peaks { peaks: WaveformPeaks, channels: u16 },
}

impl Reply {
    fn from_decoded(d: Decoded, want: Want) -> Self {
        match want {
            Want::Samples => Reply::Samples(d),
            Want::Peaks => Reply::Peaks { peaks: WaveformPeaks::compute(&d, PEAK_BLOCK), channels: d.channels },
        }
    }
}

/// How `MediaStore` runs the decoders.
#[derive(Clone, Debug)]
pub enum Isolation {
    /// This executable in `--decode` mode, killed after `timeout`.
    Child { exe: PathBuf, data_dir: PathBuf, test_hooks: bool, timeout: Duration },
    /// A dedicated thread whose panic becomes an error.
    Thread,
}

impl Isolation {
    /// For the real studio: decode in a child process of this executable.
    /// If the executable can't be found, fall back to a contained thread.
    pub fn child(data_dir: &Path, test_hooks: bool, timeout: Duration) -> Self {
        match std::env::current_exe() {
            Ok(exe) => Isolation::Child { exe, data_dir: data_dir.to_path_buf(), test_hooks, timeout },
            Err(e) => {
                log::error!("audio decoding not isolated in a child process ({e}): panics are still contained");
                Isolation::Thread
            }
        }
    }

    /// Decodes `name` of the media folder `dir` (`media::source_path`).
    pub fn run(&self, dir: &Path, name: &str, want: Want) -> Result<Reply> {
        match self {
            Isolation::Child { exe, data_dir, test_hooks, timeout } => {
                let mut cmd = Command::new(exe);
                cmd.arg("--decode").arg(name).arg("--data-dir").arg(data_dir);
                if want == Want::Peaks {
                    cmd.arg("--decode-peaks");
                }
                if *test_hooks {
                    cmd.arg("--test-hooks");
                }
                run_child(cmd, want, *timeout)
            }
            Isolation::Thread => contained(|| decode_file(dir, name, want)),
        }
    }

    pub fn samples(&self, dir: &Path, name: &str) -> Result<Decoded> {
        match self.run(dir, name, Want::Samples)? {
            Reply::Samples(d) => Ok(d),
            Reply::Peaks { .. } => bail!("réponse du décodeur inattendue"),
        }
    }

    /// The waveform and the channel count.
    pub fn peaks(&self, dir: &Path, name: &str) -> Result<(WaveformPeaks, u16)> {
        match self.run(dir, name, Want::Peaks)? {
            Reply::Peaks { peaks, channels } => Ok((peaks, channels)),
            Reply::Samples(_) => bail!("réponse du décodeur inattendue"),
        }
    }
}

/// Reads and decodes one file of the media folder. What the child runs,
/// and the contained thread.
fn decode_file(dir: &Path, name: &str, want: Want) -> Result<Reply> {
    let path = media::source_path(dir, name)?;
    let len = std::fs::symlink_metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len > media::MAX_IMPORT_BYTES {
        bail!("fichier trop gros ({} Mo au plus)", media::MAX_IMPORT_BYTES >> 20);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("lecture impossible : {name}"))?;
    Ok(Reply::from_decoded(decode::decode(&bytes)?, want))
}

/// `laser-studio --decode <file> --data-dir <dir> [--decode-peaks]`: the
/// child's whole life. The reply (or the decoder's error) goes to stdout;
/// the exit code is 0 when it was written. A panic is not caught here: it
/// ends this process (code 101), which the parent reports.
pub fn child_main(data_dir: &Path, name: &str, want: Want, test_hooks: bool) -> i32 {
    if test_hooks {
        enable_test_hooks();
    }
    let result = decode_file(&media::audio_dir(data_dir), name, want).map_err(|e| format!("{e:#}"));
    let mut out = BufWriter::new(std::io::stdout().lock());
    match write_reply(&mut out, &result).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("--decode: reply not written: {e}");
            1
        }
    }
}

/// The reply format, little-endian: `MAGIC`, then a kind byte.
/// 0 = error: u32 length, UTF-8 message.
/// 1 = samples: rate u32, channels u16, frames u64, frames × channels i16.
/// 2 = peaks: rate u32, channels u16, frames u64, block u32, count u64,
///     count × f32 min, count × f32 max.
pub fn write_reply(out: &mut impl Write, result: &std::result::Result<Reply, String>) -> std::io::Result<()> {
    out.write_all(MAGIC)?;
    match result {
        Err(message) => {
            let mut end = message.len().min(MAX_MESSAGE);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            out.write_all(&[0])?;
            out.write_all(&(end as u32).to_le_bytes())?;
            out.write_all(&message.as_bytes()[..end])?;
        }
        Ok(Reply::Samples(d)) => {
            out.write_all(&[1])?;
            out.write_all(&d.sample_rate.to_le_bytes())?;
            out.write_all(&d.channels.to_le_bytes())?;
            out.write_all(&(d.frames() as u64).to_le_bytes())?;
            let mut buf = Vec::with_capacity(64 * 1024);
            for chunk in d.samples[..d.frames() * d.channels as usize].chunks(32 * 1024) {
                buf.clear();
                buf.extend(chunk.iter().flat_map(|s| s.to_le_bytes()));
                out.write_all(&buf)?;
            }
        }
        Ok(Reply::Peaks { peaks, channels }) => {
            out.write_all(&[2])?;
            out.write_all(&peaks.sample_rate.to_le_bytes())?;
            out.write_all(&channels.to_le_bytes())?;
            out.write_all(&peaks.frames.to_le_bytes())?;
            out.write_all(&(peaks.block as u32).to_le_bytes())?;
            out.write_all(&(peaks.min.len() as u64).to_le_bytes())?;
            for v in peaks.min.iter().chain(&peaks.max) {
                out.write_all(&v.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn read_array<const N: usize>(r: &mut impl Read) -> Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b).context("réponse du décodeur tronquée")?;
    Ok(b)
}

/// Reads a reply. The outer error: the reply is missing or malformed (the
/// child died, or wrote garbage); the inner one: the decoder's own error.
/// Every size is checked against the decoder's limits before anything is
/// allocated, and nothing may follow the reply.
pub fn read_reply(r: &mut impl Read, want: Want) -> Result<std::result::Result<Reply, String>> {
    if &read_array::<8>(r)? != MAGIC {
        bail!("réponse du décodeur illisible");
    }
    let kind = read_array::<1>(r)?[0];
    if kind == 0 {
        let len = u32::from_le_bytes(read_array(r)?) as usize;
        if len > MAX_MESSAGE {
            bail!("réponse du décodeur trop longue");
        }
        let mut msg = vec![0u8; len];
        r.read_exact(&mut msg).context("réponse du décodeur tronquée")?;
        expect_end(r)?;
        return Ok(Err(String::from_utf8_lossy(&msg).into_owned()));
    }
    let expected = match want {
        Want::Samples => 1,
        Want::Peaks => 2,
    };
    if kind != expected {
        bail!("réponse du décodeur inattendue");
    }
    let sample_rate = u32::from_le_bytes(read_array(r)?);
    let channels = u16::from_le_bytes(read_array(r)?);
    let frames = u64::from_le_bytes(read_array(r)?);
    if !(1_000..=384_000).contains(&sample_rate) || !(1..=2).contains(&channels) || frames == 0 || frames > MAX_SECONDS * sample_rate as u64 {
        bail!("réponse du décodeur hors limites");
    }
    let reply = if want == Want::Samples {
        let n = frames as usize * channels as usize;
        let mut samples = Vec::with_capacity(n);
        let mut buf = vec![0u8; 64 * 1024];
        while samples.len() < n {
            let want_bytes = ((n - samples.len()) * 2).min(buf.len());
            r.read_exact(&mut buf[..want_bytes]).context("réponse du décodeur tronquée")?;
            samples.extend(buf[..want_bytes].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)));
        }
        Reply::Samples(Decoded { sample_rate, channels, samples })
    } else {
        let block = u32::from_le_bytes(read_array(r)?) as usize;
        let count = u64::from_le_bytes(read_array(r)?);
        if block != PEAK_BLOCK || count != frames.div_ceil(block as u64) {
            bail!("réponse du décodeur hors limites");
        }
        let mut floats = Vec::with_capacity(2 * count as usize);
        for _ in 0..2 * count {
            let v = f32::from_le_bytes(read_array(r)?);
            if !(-1.0..=1.0).contains(&v) {
                bail!("réponse du décodeur hors limites");
            }
            floats.push(v);
        }
        let max = floats.split_off(count as usize);
        Reply::Peaks { peaks: WaveformPeaks { block, min: floats, max, sample_rate, frames }, channels }
    };
    expect_end(r)?;
    Ok(Ok(reply))
}

fn expect_end(r: &mut impl Read) -> Result<()> {
    let mut extra = [0u8; 1];
    match r.read(&mut extra) {
        Ok(0) => Ok(()),
        _ => bail!("réponse du décodeur trop longue"),
    }
}

/// Runs a decoder child and reads its reply, killing it after `timeout`.
/// Whatever the child does (crash, garbage, silence, hang), the result is
/// a reply or a French error; it never panics and never waits longer.
pub fn run_child(mut cmd: Command, want: Want, timeout: Duration) -> Result<Reply> {
    let deadline = Instant::now() + timeout;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().context("impossible de lancer le décodeur")?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new().name("decode-reply".into()).spawn(move || {
        let _ = tx.send(read_reply(&mut BufReader::new(stdout), want));
    });
    if let Err(e) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e).context("impossible de lire le décodeur");
    }
    let reply = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    // The reply is in (or it's too late): the child must be gone by the deadline.
    let status = wait_or_kill(&mut child, deadline);
    match (reply, status) {
        (Ok(Ok(Ok(reply))), Some(status)) if status.success() => Ok(reply),
        (Ok(Ok(Err(message))), _) => Err(anyhow!(message)),
        (Err(_), _) | (_, None) => {
            log::warn!("audio decoder killed after {timeout:?}");
            bail!("décodage trop long (plus de {} s) : fichier refusé, le studio continue", timeout.as_secs_f32().ceil() as u64)
        }
        (Ok(broken), Some(status)) => {
            if let Err(e) = broken {
                log::warn!("audio decoder ({status}): {e:#}");
            }
            Err(anyhow!(crash_message(status)))
        }
    }
}

/// Waits for the child until `deadline`, then kills it. `None`: killed.
fn wait_or_kill(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Why the child gave no reply, for the UI.
fn crash_message(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.signal(), status.code()) {
        (Some(sig), _) => format!("le décodeur s'est arrêté brutalement (signal {sig}) sur ce fichier, abîmé ou piégé : fichier refusé, le studio continue"),
        (_, Some(101)) => "le décodeur a planté sur ce fichier, abîmé ou piégé : fichier refusé, le studio continue".to_string(),
        (_, code) => format!("le décodeur n'a pas répondu (code {}) : fichier refusé, le studio continue", code.unwrap_or(-1)),
    }
}

thread_local! {
    static CONTAINED: Cell<bool> = const { Cell::new(false) };
}

/// True on a contained decoder thread: its panic becomes an error for the
/// caller, so the global panic hook must not stop the studio for it.
pub fn panic_is_contained() -> bool {
    CONTAINED.with(Cell::get)
}

/// Runs `f` on a dedicated, contained thread: a panic in it is an error.
pub fn contained<T: Send>(f: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("decode".into())
            .spawn_scoped(scope, || {
                CONTAINED.with(|c| c.set(true));
                f()
            })
            .context("impossible de lancer le décodeur")?;
        handle.join().unwrap_or_else(|panic| {
            let why = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
            log::warn!("audio decoder panicked: {why}");
            Err(anyhow!("le décodeur a planté sur ce fichier, abîmé ou piégé : fichier refusé, le studio continue"))
        })
    })
}

static TEST_HOOKS: AtomicBool = AtomicBool::new(false);

/// `--test-hooks`: files starting with `LSPANIC!`, `LSABORT!` or `LSHANG!!`
/// make the decoder panic, abort the process or hang (tests of the
/// isolation). Never enabled otherwise.
pub fn enable_test_hooks() {
    TEST_HOOKS.store(true, Ordering::SeqCst);
}

/// Called by `decode::decode` first: the simulated decoder failures.
pub fn test_crash(bytes: &[u8]) {
    if !TEST_HOOKS.load(Ordering::SeqCst) {
        return;
    }
    if bytes.starts_with(b"LSPANIC!") {
        panic!("décodeur : panique simulée (--test-hooks)");
    }
    if bytes.starts_with(b"LSABORT!") {
        std::process::abort();
    }
    if bytes.starts_with(b"LSHANG!!") {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::decode::testing::{sine, wav16};
    use super::*;

    fn temp_media(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-isolate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn roundtrip(reply: std::result::Result<Reply, String>, want: Want) -> Result<std::result::Result<Reply, String>> {
        let mut bytes = Vec::new();
        write_reply(&mut bytes, &reply).unwrap();
        read_reply(&mut bytes.as_slice(), want)
    }

    #[test]
    fn replies_round_trip() {
        let d = decode::decode(&wav16(8_000, 2, &sine(8_000, 0.5, 100.0, 0.5))).unwrap();
        let peaks = WaveformPeaks::compute(&d, PEAK_BLOCK);
        assert_eq!(roundtrip(Ok(Reply::Samples(d.clone())), Want::Samples).unwrap().unwrap(), Reply::Samples(d.clone()));
        assert_eq!(roundtrip(Ok(Reply::from_decoded(d, Want::Peaks)), Want::Peaks).unwrap().unwrap(), Reply::Peaks { peaks, channels: 2 });
        assert_eq!(roundtrip(Err("fichier MP3 illisible".into()), Want::Peaks).unwrap().unwrap_err(), "fichier MP3 illisible");
        // A long message is cut on a character boundary.
        let long = roundtrip(Err("é".repeat(5000)), Want::Samples).unwrap().unwrap_err();
        assert!(long.len() <= MAX_MESSAGE && long.chars().all(|c| c == 'é'));
    }

    #[test]
    fn malformed_replies_are_refused_before_allocating() {
        let d = decode::decode(&wav16(8_000, 1, &[0.25; 3000])).unwrap();
        let mut good = Vec::new();
        write_reply(&mut good, &Ok(Reply::Samples(d.clone()))).unwrap();
        let bad = |bytes: &[u8], want| read_reply(&mut &bytes[..], want).is_err();
        assert!(bad(b"", Want::Samples), "nothing (the child died at once)");
        assert!(bad(b"garbage!garbage!", Want::Samples));
        assert!(bad(&good[..good.len() - 1], Want::Samples), "truncated");
        assert!(bad(&[&good[..], b"x"].concat(), Want::Samples), "trailing bytes");
        assert!(bad(&good, Want::Peaks), "not what was asked");
        // Header fields past the limits: a huge sample count, 0 Hz, 3 channels,
        // 31 minutes, a huge error message. Refused from the header alone.
        let header = |rate: u32, ch: u16, frames: u64| [&MAGIC[..], &[1], &rate.to_le_bytes(), &ch.to_le_bytes(), &frames.to_le_bytes()].concat();
        assert!(bad(&header(48_000, 2, u64::MAX), Want::Samples));
        assert!(bad(&header(0, 1, 10), Want::Samples));
        assert!(bad(&header(48_000, 3, 10), Want::Samples));
        assert!(bad(&header(48_000, 1, 31 * 60 * 48_000), Want::Samples));
        assert!(bad(&header(48_000, 1, 0), Want::Samples));
        assert!(bad(&[&MAGIC[..], &[0], &u32::MAX.to_le_bytes()].concat(), Want::Samples));
        // Peaks with a wrong count, block, or a value out of -1..1.
        let mut peaks = Vec::new();
        write_reply(&mut peaks, &Ok(Reply::from_decoded(d, Want::Peaks))).unwrap();
        assert!(read_reply(&mut peaks.as_slice(), Want::Peaks).unwrap().is_ok());
        let mut wrong_block = peaks.clone();
        wrong_block[23] = 7;
        assert!(bad(&wrong_block, Want::Peaks));
        let mut wrong_count = peaks.clone();
        wrong_count[27] = 9;
        assert!(bad(&wrong_count, Want::Peaks));
        let mut out_of_range = peaks.clone();
        let n = out_of_range.len();
        out_of_range[n - 4..].copy_from_slice(&2.0f32.to_le_bytes());
        assert!(bad(&out_of_range, Want::Peaks));
        // Random garbage behind the magic: an error, never a panic.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..2000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 64) as usize;
            let mut bytes = MAGIC.to_vec();
            bytes.push((seed >> 8) as u8 % 3);
            bytes.extend((0..len).map(|i| (seed >> (i % 8 * 8)) as u8));
            let _ = read_reply(&mut bytes.as_slice(), if seed & 1 == 0 { Want::Samples } else { Want::Peaks });
        }
    }

    /// Stand-ins for a misbehaving child: a shell that crashes, answers
    /// garbage, says nothing, or hangs. Each is a French error in time.
    #[test]
    fn a_child_that_crashes_lies_or_hangs_is_an_error_in_time() {
        let sh = |script: &str| {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", script]);
            c
        };
        let err = |script: &str, timeout_ms: u64| format!("{:#}", run_child(sh(script), Want::Peaks, Duration::from_millis(timeout_ms)).unwrap_err());
        let e = err("kill -SEGV $$", 5_000);
        assert!(e.contains("arrêté brutalement (signal 11)") && e.contains("le studio continue"), "{e}");
        let e = err("kill -ABRT $$", 5_000);
        assert!(e.contains("arrêté brutalement (signal 6)"), "{e}");
        let e = err("exit 101", 5_000);
        assert!(e.contains("a planté"), "{e}");
        let e = err("printf garbage; exit 0", 5_000);
        assert!(e.contains("n'a pas répondu (code 0)"), "{e}");
        let e = err("exit 3", 5_000);
        assert!(e.contains("code 3"), "{e}");
        let t = Instant::now();
        let e = err("sleep 30", 300);
        assert!(e.contains("décodage trop long"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(3), "killed at the limit: {:?}", t.elapsed());
        // Garbage then a hang: still bounded by the limit.
        let t = Instant::now();
        let e = err("printf LSDECOD1; sleep 30", 300);
        assert!(e.contains("trop long"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(3));
        // A child that can't be started at all.
        let e = format!("{:#}", run_child(Command::new("/nonexistent/decoder"), Want::Peaks, Duration::from_secs(1)).unwrap_err());
        assert!(e.contains("impossible de lancer le décodeur"), "{e}");
        // A child whose error reply is passed on as it is.
        let mut reply = Vec::new();
        write_reply(&mut reply, &Err("format audio non reconnu".into())).unwrap();
        let file = temp_media("reply").join("reply.bin");
        std::fs::write(&file, &reply).unwrap();
        let e = format!("{:#}", run_child(sh(&format!("cat '{}'", file.display())), Want::Peaks, Duration::from_secs(5)).unwrap_err());
        assert_eq!(e, "format audio non reconnu");
    }

    /// The contained thread: a decoder panic is an error for the caller,
    /// the thread is marked for the global hook, and the caller's is not.
    #[test]
    fn a_contained_panic_is_an_error() {
        let dir = temp_media("contained");
        std::fs::write(dir.join("piège.mp3"), b"LSPANIC! crafted").unwrap();
        std::fs::write(dir.join("bon.wav"), wav16(8_000, 1, &[0.5; 800])).unwrap();
        enable_test_hooks();
        assert!(!panic_is_contained());
        let e = format!("{:#}", Isolation::Thread.peaks(&dir, "piège.mp3").unwrap_err());
        assert!(e.contains("le décodeur a planté") && e.contains("le studio continue"), "{e}");
        assert!(contained(|| Ok(panic_is_contained())).unwrap(), "the decoder thread is marked");
        assert!(!panic_is_contained(), "the caller's thread is not");
        // Other files still decode, and names stay confined.
        let (p, ch) = Isolation::Thread.peaks(&dir, "bon.wav").unwrap();
        assert_eq!((ch, p.frames), (1, 800));
        assert_eq!(Isolation::Thread.samples(&dir, "bon.wav").unwrap().frames(), 800);
        assert!(Isolation::Thread.peaks(&dir, "../bon.wav").is_err());
        assert!(Isolation::Thread.peaks(&dir, "absent.wav").is_err());
    }

    #[test]
    fn test_hooks_are_off_unless_enabled() {
        // Without hooks a `LSPANIC!` file is just an unknown format. (Hooks
        // are process-wide once enabled, so this checks the gate itself.)
        let dir = temp_media("hooks");
        std::fs::write(dir.join("x.mp3"), b"LSPANIC! crafted").unwrap();
        if !TEST_HOOKS.load(Ordering::SeqCst) {
            assert!(format!("{:#}", Isolation::Thread.peaks(&dir, "x.mp3").unwrap_err()).contains("format audio non reconnu"));
        }
        test_crash(b"harmless bytes");
    }
}
