//! Safety lock (T-261): the safety officer locks the safety settings with
//! a code, so a guest at the controls (VJ, DJ) can play cues but can't
//! widen a zone, raise a cap or open audience scanning by mistake.
//!
//! What the lock does, while it is on:
//! - every route of class `Safety` (`web::ROUTES`) is refused with `423
//!   Locked`, except the ones listed in `ALWAYS` (stopping, neutral) and
//!   in `HANDLER` (the handler lets through what only tightens);
//! - what lowers the risk always runs: the e-stop, disarming (`/api/arm`
//!   is `Arm`, never locked), « Ciel coupé » on, back above the audience,
//!   leaving a test pattern, stopping the zone trace, cutting the tablet
//!   remote, closing the external inputs, show mode on or off, a tighter
//!   safety setting (`/api/safety`, profile load, power caps, colour, scan
//!   speed: anything their own `loosenings` check would flag is 423, and
//!   `confirm_loosen` is ignored), the master brightness (an `Edit`/`Play`
//!   route, never locked);
//! - undo/redo of a projection or venue change is refused (history.rs).
//!
//! It is a protection against mistakes, not against an attacker: the API
//! listens on 127.0.0.1 only. Still, the code is never stored nor logged:
//! `<data-dir>/safety-lock.json` holds a PBKDF2-HMAC-SHA256 hash with a
//! random salt; the comparison is constant-time; 5 wrong codes in a row
//! block attempts for 60 s. Unlocking is an HTTP route only (the UI, the
//! keyboard, the local API): MIDI, OSC, DMX and the tablet remote can't
//! reach it (controls never unlock; the remote serves its own whitelist).
//! After `relock_min` minutes (10) without a safety edit, it locks again.
//!
//! Forgotten code: quit the studio, delete `safety-lock.json` from the
//! data directory, restart. The studio starts unlocked and writes it to
//! the safety log (`prefs.json` remembers that a code was set). That needs
//! the Mac itself: nothing over the network removes the code.

use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The 423 body, also the UI's tooltip.
pub const LOCKED_FR: &str = "Verrouillé par le responsable sécurité";
pub const FILE: &str = "safety-lock.json";
/// Key in `prefs.json`: a code was set (to log its removal by hand).
const PREFS_KEY: &str = "safety_lock_set";
/// Wrong codes in a row before the wait.
pub const MAX_FAILURES: u32 = 5;
pub const WAIT: Duration = Duration::from_secs(60);
pub const DEFAULT_RELOCK_MIN: u32 = 10;
/// PBKDF2 rounds for a new code (fewer in unit tests, for speed). The
/// file keeps its own count, so raising it later keeps old codes working.
const ITERATIONS: u32 = if cfg!(test) { 1_000 } else { 100_000 };
const FORMAT: u32 = 1;

/// Safety-class POST routes that always run, locked or not: they stop,
/// tighten or change no safety setting.
pub const ALWAYS: &[&str] = &[
    "/api/estop",
    "/api/safety/lock",
    "/api/safety/unlock",
    // Show mode (T-283): an independent lock, both ways.
    "/api/lock",
    "/api/keep_awake",
    // Listing the DACs changes nothing (opening one is `/api/outputs/select`).
    "/api/devices/scan",
    "/api/outputs/scan",
    "/api/remote/kick",
    // Taking over on a backup (T-456) leaves the laser disarmed.
    "/api/standby/takeover",
    // TC-IN follows the timecode like arming follows the button.
    "/api/timecode/arm",
    // Ticking the pre-show checklist is operating, not editing it.
    "/api/checklist/operator",
    "/api/checklist/check",
    "/api/checklist/complete",
    "/api/checklist/restart",
];

/// Safety-class routes whose handler refuses only the loosening direction
/// while locked (see the module doc).
pub const HANDLER: &[&str] = &[
    "/api/safety",
    "/api/safety/profiles/load",
    "/api/safety/sky",
    "/api/audience",
    "/api/outputs/limits",
    "/api/outputs/color",
    "/api/outputs/scan_limits",
    "/api/testpattern",
    "/api/safety/trace",
    "/api/remote",
    "/api/external/gate",
    // Removing or renaming a mirror target (T-109) only tightens.
    "/api/targets",
];

/// Whether the route gate lets `path` (a Safety-class POST) through while
/// locked; the `HANDLER` ones check the direction themselves.
pub fn passes_gate(path: &str) -> bool {
    ALWAYS.contains(&path) || HANDLER.contains(&path)
}

/// `safety-lock.json`. Never the code itself.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LockFile {
    pub format: u32,
    /// PBKDF2-HMAC-SHA256 of the code, hex.
    pub hash: Option<String>,
    /// 16 random bytes, hex.
    pub salt: Option<String>,
    pub iterations: u32,
    /// Minutes without a safety edit before locking again (1 to 240).
    pub relock_min: u32,
}

/// The lock in memory.
#[derive(Debug, Default)]
pub struct SafetyLock {
    /// A valid code is set.
    code: Option<LockFile>,
    /// The file exists but can't be read: locked, no code unlocks it.
    damaged: bool,
    /// Unlocked, with the last activity (unlock or safety edit).
    unlocked_at: Option<Instant>,
    failures: u32,
    wait_until: Option<Instant>,
    path: Option<PathBuf>,
    prefs_path: Option<PathBuf>,
    /// What `startup` writes to the safety log.
    startup_note: Option<&'static str>,
}

/// Why an unlock attempt was refused.
#[derive(Debug, PartialEq)]
pub enum UnlockError {
    /// No code set: nothing to unlock.
    NotSet,
    /// The file is damaged: only deleting it unlocks.
    Damaged,
    /// Too many wrong codes: seconds to wait.
    Wait(u64),
    /// Wrong code: attempts left before the wait.
    Wrong(u32),
}

impl UnlockError {
    pub fn message_fr(&self) -> String {
        match self {
            Self::NotSet => "Aucun code de sécurité n'est défini".into(),
            Self::Damaged => format!("Fichier {FILE} illisible : supprimez-le à la main (studio arrêté) pour déverrouiller"),
            Self::Wait(s) => format!("Trop d'essais : réessayez dans {s} s"),
            Self::Wrong(0) => format!("Code incorrect : attendez {} s", WAIT.as_secs()),
            Self::Wrong(n) => format!("Code incorrect ({n} essai{} avant une attente de {} s)", if *n > 1 { "s" } else { "" }, WAIT.as_secs()),
        }
    }
}

/// What an attempt needs, read under the lock; the slow hash runs
/// outside it (`hash_code`), then `finish_unlock` compares.
pub struct Attempt {
    salt: Vec<u8>,
    iterations: u32,
}

impl SafetyLock {
    /// From `<data-dir>/safety-lock.json`: a valid code starts locked; a
    /// damaged file starts locked with no code; no file starts unlocked.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        let prefs_path = data_dir.join("prefs.json");
        let mut lock = Self { path: Some(path.clone()), prefs_path: Some(prefs_path.clone()), ..Self::default() };
        match std::fs::read(&path) {
            Ok(bytes) => match parse(&bytes) {
                Ok(file) => lock.code = Some(file),
                Err(e) => {
                    log::warn!("{FILE} illisible ({e:#}) : sécurité verrouillée, supprimez le fichier pour la déverrouiller");
                    lock.damaged = true;
                    lock.startup_note = Some("damaged");
                }
            },
            Err(_) => {
                let prefs: Map<String, Value> = crate::load_json(&prefs_path);
                if prefs.get(PREFS_KEY).and_then(Value::as_bool).unwrap_or(false) {
                    log::warn!("{FILE} supprimé : le studio démarre avec la sécurité déverrouillée");
                    lock.startup_note = Some("file_removed");
                }
            }
        }
        lock
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Whether a code (or a damaged file) protects the settings.
    pub fn is_set(&self) -> bool {
        self.code.is_some() || self.damaged
    }

    pub fn relock_after(&self) -> Duration {
        Duration::from_secs(60 * u64::from(self.code.as_ref().map_or(DEFAULT_RELOCK_MIN, |c| c.relock_min)))
    }

    /// Whether safety edits are refused at `now`.
    pub fn locked(&self, now: Instant) -> bool {
        self.is_set() && self.unlocked_at.is_none_or(|t| now.saturating_duration_since(t) >= self.relock_after())
    }

    /// Ends an unlock that timed out. True when it just locked again (the
    /// caller logs it).
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.unlocked_at.is_some() && self.locked(now) {
            self.unlocked_at = None;
            return true;
        }
        false
    }

    /// A safety edit while unlocked: the relock countdown starts again.
    pub fn touch(&mut self, now: Instant) {
        if self.unlocked_at.is_some() && !self.locked(now) {
            self.unlocked_at = Some(now);
        }
    }

    /// Locks now. Refused without a code.
    pub fn lock_now(&mut self) -> Result<()> {
        if !self.is_set() {
            bail!("Définissez d'abord un code");
        }
        self.unlocked_at = None;
        Ok(())
    }

    /// Starts an unlock attempt: refused while waiting, with no code or a
    /// damaged file.
    pub fn begin_unlock(&self, now: Instant) -> Result<Attempt, UnlockError> {
        if self.damaged {
            return Err(UnlockError::Damaged);
        }
        let Some(file) = &self.code else { return Err(UnlockError::NotSet) };
        if let Some(until) = self.wait_until.filter(|&u| u > now) {
            return Err(UnlockError::Wait(until.duration_since(now).as_secs().max(1)));
        }
        Ok(Attempt { salt: from_hex(file.salt.as_deref().unwrap_or("")).unwrap_or_default(), iterations: file.iterations })
    }

    /// Ends an attempt with the code's hash (`hash_code`). A wrong code
    /// counts; the fifth in a row starts the wait.
    pub fn finish_unlock(&mut self, hash: &[u8; 32], now: Instant) -> Result<(), UnlockError> {
        // Checked again: the file may have changed while hashing.
        self.begin_unlock(now)?;
        let expected = self.code.as_ref().and_then(|c| from_hex(c.hash.as_deref().unwrap_or(""))).unwrap_or_default();
        if constant_time_eq(&expected, hash) {
            self.failures = 0;
            self.wait_until = None;
            self.unlocked_at = Some(now);
            return Ok(());
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.failures = 0;
            self.wait_until = Some(now + WAIT);
            return Err(UnlockError::Wrong(0));
        }
        Err(UnlockError::Wrong(MAX_FAILURES - self.failures))
    }

    /// Sets (or changes) the code and locks. Changing it needs the lock
    /// open; a damaged file can't be replaced from here.
    pub fn set_code(&mut self, file: LockFile, now: Instant) -> Result<()> {
        if self.damaged {
            bail!("Fichier {FILE} illisible : supprimez-le à la main (studio arrêté)");
        }
        if self.locked(now) {
            bail!("{LOCKED_FR} : déverrouillez pour changer le code");
        }
        if let Some(path) = &self.path {
            crate::project::write_atomic(path, &serde_json::to_vec_pretty(&file)?).context("écriture du code impossible")?;
            self.write_marker(true)?;
        }
        self.code = Some(file);
        self.unlocked_at = None;
        self.failures = 0;
        Ok(())
    }

    /// Removes the code (the lock must be open).
    pub fn remove(&mut self, now: Instant) -> Result<()> {
        if !self.is_set() {
            return Ok(());
        }
        if self.locked(now) {
            bail!("{LOCKED_FR} : déverrouillez pour retirer le code");
        }
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("suppression du code impossible"),
            }
            self.write_marker(false)?;
        }
        self.code = None;
        self.unlocked_at = None;
        Ok(())
    }

    /// Changes the relock delay (the lock must be open).
    pub fn set_relock_min(&mut self, minutes: u32, now: Instant) -> Result<()> {
        let Some(mut file) = self.code.clone() else { bail!("Définissez d'abord un code") };
        if self.locked(now) {
            bail!("{LOCKED_FR}");
        }
        file.relock_min = minutes.clamp(1, 240);
        if let Some(path) = &self.path {
            crate::project::write_atomic(path, &serde_json::to_vec_pretty(&file)?).context("écriture impossible")?;
        }
        self.code = Some(file);
        self.unlocked_at = Some(now);
        Ok(())
    }

    fn write_marker(&self, on: bool) -> Result<()> {
        let Some(path) = &self.prefs_path else { return Ok(()) };
        let mut prefs: Map<String, Value> = crate::load_json(path);
        prefs.insert(PREFS_KEY.into(), Value::Bool(on));
        crate::project::write_atomic(path, &serde_json::to_vec_pretty(&prefs)?)
    }

    /// `safety_lock` in `/api/lock`, `/api/state`: never the hash.
    pub fn view(&self, now: Instant) -> Value {
        let locked = self.locked(now);
        let relock_in = self.unlocked_at.filter(|_| !locked).map(|t| self.relock_after().saturating_sub(now.saturating_duration_since(t)).as_secs());
        let wait = self.wait_until.filter(|&u| u > now).map(|u| u.duration_since(now).as_secs().max(1));
        json!({
            "set": self.is_set(),
            "locked": locked,
            "damaged": self.damaged,
            "relock_min": self.code.as_ref().map_or(DEFAULT_RELOCK_MIN, |c| c.relock_min),
            "relock_in_s": relock_in,
            "wait_s": wait,
            "message": LOCKED_FR,
        })
    }
}

/// Logs what `load` found (a code removed by hand, a damaged file).
pub fn startup(s: &mut Shared) {
    match s.safety_lock.startup_note.take() {
        Some("file_removed") => {
            s.gate.log().record("safety_lock", Some("api"), json!({ "event": "file_removed" }));
            if let Err(e) = s.safety_lock.write_marker(false) {
                log::warn!("prefs.json : {e:#}");
            }
        }
        Some(note) => s.gate.log().record("safety_lock", Some("api"), json!({ "event": note })),
        None => {}
    }
}

/// Whether the safety settings are locked now; logs a timed-out unlock.
pub fn check(s: &mut Shared, now: Instant) -> bool {
    if s.safety_lock.tick(now) {
        log::info!("sécurité reverrouillée (inactivité)");
        s.gate.log().record("safety_lock", Some("api"), json!({ "event": "relocked" }));
    }
    s.safety_lock.locked(now)
}

/// A code is 4 to 8 digits.
pub fn valid_code(code: &str) -> bool {
    (4..=8).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_digit())
}

/// A new lock file for `code` (random salt, slow hash). Slow: call it
/// with the studio lock released.
pub fn new_file(code: &str, relock_min: u32) -> Result<LockFile> {
    if !valid_code(code) {
        bail!("Le code doit compter 4 à 8 chiffres");
    }
    let salt = random_salt()?;
    let hash = pbkdf2_sha256(code.as_bytes(), &salt, ITERATIONS);
    Ok(LockFile { format: FORMAT, hash: Some(to_hex(&hash)), salt: Some(to_hex(&salt)), iterations: ITERATIONS, relock_min: relock_min.clamp(1, 240) })
}

/// The hash of a typed code for an attempt. Slow: lock released.
pub fn hash_code(code: &str, a: &Attempt) -> [u8; 32] {
    pbkdf2_sha256(code.as_bytes(), &a.salt, a.iterations)
}

fn parse(bytes: &[u8]) -> Result<LockFile> {
    let file: LockFile = serde_json::from_slice(bytes)?;
    if file.format > FORMAT {
        bail!("format {} plus récent", file.format);
    }
    let ok_hex = |h: &Option<String>, len: usize| h.as_deref().and_then(from_hex).is_some_and(|b| b.len() == len);
    if !ok_hex(&file.hash, 32) || !ok_hex(&file.salt, 16) || file.iterations == 0 || file.relock_min == 0 {
        bail!("hachage, sel ou délai invalide");
    }
    Ok(file)
}

fn random_salt() -> Result<[u8; 16]> {
    use std::io::Read;
    let mut salt = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut salt)).context("pas d'aléa système")?;
    Ok(salt)
}

/// HMAC-SHA256 key state: the hashers after the inner and outer pads.
fn hmac_keyed(key: &[u8]) -> (Sha256, Sha256) {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let (mut inner, mut outer) = (Sha256::new(), Sha256::new());
    inner.update(block.map(|b| b ^ 0x36));
    outer.update(block.map(|b| b ^ 0x5c));
    (inner, outer)
}

fn hmac(keyed: &(Sha256, Sha256), msg: &[&[u8]]) -> [u8; 32] {
    let mut inner = keyed.0.clone();
    for m in msg {
        inner.update(m);
    }
    let mut outer = keyed.1.clone();
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA256 (RFC 8018), one 32-byte block.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let keyed = hmac_keyed(password);
    let mut u = hmac(&keyed, &[salt, &1u32.to_be_bytes()]);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(&keyed, &[&u]);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// Equal length and bytes, in time independent of where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        from_hex(s).unwrap()
    }

    #[test]
    fn pbkdf2_matches_the_rfc_7914_vectors() {
        // RFC 7914 §11, PBKDF2-HMAC-SHA256, first 32 bytes.
        assert_eq!(pbkdf2_sha256(b"passwd", b"salt", 1).to_vec(), hex("55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"));
        assert_eq!(pbkdf2_sha256(b"Password", b"NaCl", 80000).to_vec(), hex("4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"));
    }

    #[test]
    fn codes_are_4_to_8_digits() {
        for ok in ["1234", "00000000", "987654"] {
            assert!(valid_code(ok), "{ok}");
        }
        for bad in ["123", "123456789", "12a4", "", "12 34", "١٢٣٤"] {
            assert!(!valid_code(bad), "{bad}");
        }
    }

    #[test]
    fn the_file_holds_a_salted_hash_never_the_code() {
        let a = new_file("482913", 10).unwrap();
        let b = new_file("482913", 10).unwrap();
        assert_ne!(a.salt, b.salt, "a fresh salt each time");
        assert_ne!(a.hash, b.hash);
        let text = serde_json::to_string(&a).unwrap();
        assert!(!text.contains("482913"));
        assert_eq!(parse(text.as_bytes()).unwrap(), a);
        assert!(new_file("12", 10).is_err());
    }

    fn locked_with(code: &str) -> SafetyLock {
        let mut l = SafetyLock::in_memory();
        l.set_code(new_file(code, 10).unwrap(), Instant::now()).unwrap();
        l
    }

    fn attempt(l: &mut SafetyLock, code: &str, now: Instant) -> Result<(), UnlockError> {
        let a = l.begin_unlock(now)?;
        l.finish_unlock(&hash_code(code, &a), now)
    }

    #[test]
    fn the_right_code_unlocks_and_it_locks_again_after_inactivity() {
        let t0 = Instant::now();
        let mut l = locked_with("2468");
        assert!(l.locked(t0));
        attempt(&mut l, "2468", t0).unwrap();
        assert!(!l.locked(t0));
        // A safety edit at 9 min restarts the countdown.
        let t9 = t0 + Duration::from_secs(9 * 60);
        l.touch(t9);
        assert!(!l.locked(t9 + Duration::from_secs(9 * 60)));
        let late = t9 + Duration::from_secs(10 * 60);
        assert!(l.locked(late));
        assert!(l.tick(late), "the relock is reported once");
        assert!(!l.tick(late));
        assert!(l.locked(late));
    }

    #[test]
    fn five_wrong_codes_make_it_wait_60_s() {
        let t0 = Instant::now();
        let mut l = locked_with("2468");
        for left in (1..MAX_FAILURES).rev() {
            assert_eq!(attempt(&mut l, "1111", t0), Err(UnlockError::Wrong(left)));
        }
        assert_eq!(attempt(&mut l, "1111", t0), Err(UnlockError::Wrong(0)));
        // Even the right code waits; nothing is hashed meanwhile.
        assert_eq!(attempt(&mut l, "2468", t0 + Duration::from_secs(1)), Err(UnlockError::Wait(59)));
        assert!(matches!(l.begin_unlock(t0 + Duration::from_secs(59)), Err(UnlockError::Wait(_))));
        assert!(l.locked(t0 + Duration::from_secs(59)));
        let after = t0 + WAIT;
        attempt(&mut l, "2468", after).unwrap();
        assert!(!l.locked(after));
    }

    #[test]
    fn changing_or_removing_the_code_needs_the_lock_open() {
        let t0 = Instant::now();
        let mut l = locked_with("2468");
        assert!(l.set_code(new_file("1357", 10).unwrap(), t0).is_err());
        assert!(l.remove(t0).is_err());
        assert!(l.set_relock_min(5, t0).is_err());
        attempt(&mut l, "2468", t0).unwrap();
        l.set_relock_min(5, t0).unwrap();
        assert!(l.locked(t0 + Duration::from_secs(5 * 60)));
        attempt(&mut l, "2468", t0).unwrap();
        l.remove(t0).unwrap();
        assert!(!l.is_set() && !l.locked(t0));
        assert!(l.lock_now().is_err(), "no code, nothing to lock");
    }

    #[test]
    fn a_damaged_file_starts_locked_and_a_removed_one_unlocked_and_logged() {
        let dir = std::env::temp_dir().join(format!("safetylock-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A code set: on disk, hash only; the next start is locked.
        let mut l = SafetyLock::load(&dir);
        assert!(!l.is_set());
        l.set_code(new_file("97531", 10).unwrap(), Instant::now()).unwrap();
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("97531"));
        let l = SafetyLock::load(&dir);
        assert!(l.locked(Instant::now()) && l.startup_note.is_none());
        // Damaged: locked, no code opens it.
        std::fs::write(dir.join(FILE), b"{ pas du json").unwrap();
        let l = SafetyLock::load(&dir);
        assert!(l.locked(Instant::now()));
        assert_eq!(l.begin_unlock(Instant::now()).err(), Some(UnlockError::Damaged));
        assert_eq!(l.startup_note, Some("damaged"));
        // Deleted by hand: unlocked, noted for the log.
        std::fs::remove_file(dir.join(FILE)).unwrap();
        let l = SafetyLock::load(&dir);
        assert!(!l.locked(Instant::now()));
        assert_eq!(l.startup_note, Some("file_removed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn comparison_is_by_value_and_length() {
        assert!(constant_time_eq(&[1, 2, 3], &[1, 2, 3]));
        assert!(!constant_time_eq(&[1, 2, 3], &[1, 2, 4]));
        assert!(!constant_time_eq(&[1, 2], &[1, 2, 3]));
    }
}
