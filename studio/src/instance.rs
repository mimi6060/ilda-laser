//! One studio per data folder (T-412).
//!
//! Two instances on the same `--data-dir` would each write the same files.
//! At startup the studio takes an exclusive advisory lock on
//! `<data-dir>/.lock` (`flock`, through the standard library) and writes
//! its PID, port and start time in it. The system drops the lock when the
//! process ends, crash included, so there is no "is that PID still alive?"
//! guesswork: a `.lock` file left by a crash is simply not locked any more
//! and is taken over. A clean exit empties the file, so a non-empty file
//! found unlocked means the last run did not end cleanly.
//!
//! Not taken by the decoder child (`--decode`, run by the studio itself on
//! the same folder) nor by `--list-controls`.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const LOCK_FILE: &str = ".lock";

pub struct InstanceLock {
    file: File,
    path: PathBuf,
    /// What a previous run that did not exit cleanly left in the file.
    pub previous: Option<LockInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub port: u16,
    pub started_at: String,
}

#[derive(Debug)]
pub enum LockError {
    /// Another studio holds the folder (with what it wrote, if readable).
    Held(Option<LockInfo>),
    /// No lock possible here (e.g. some network volumes): the caller warns
    /// and carries on.
    Io(std::io::Error),
}

impl LockError {
    /// The message for the second instance, in French.
    pub fn message(&self, data_dir: &Path) -> String {
        match self {
            LockError::Held(Some(info)) => format!(
                "Laser Studio utilise déjà ce dossier ({}) : PID {}, port {}, démarré le {}. Fermez-le ou choisissez un autre --data-dir.",
                data_dir.display(),
                info.pid,
                info.port,
                info.started_at
            ),
            LockError::Held(None) => {
                format!("Laser Studio utilise déjà ce dossier ({}). Fermez-le ou choisissez un autre --data-dir.", data_dir.display())
            }
            LockError::Io(e) => format!("Verrou du dossier {} impossible : {e}", data_dir.display()),
        }
    }
}

fn read_info(file: &mut File) -> Option<LockInfo> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn now_local() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string()
}

/// Takes `<data_dir>/.lock` for this process, serving on `port`.
pub fn acquire(data_dir: &Path, port: u16) -> Result<InstanceLock, LockError> {
    let path = data_dir.join(LOCK_FILE);
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(LockError::Held(read_info(&mut file))),
        Err(TryLockError::Error(e)) => return Err(LockError::Io(e)),
    }
    let previous = read_info(&mut file);
    if let Some(old) = &previous {
        log::warn!("verrou orphelin repris dans {} (PID {}, démarré le {}) : la dernière session ne s'est pas terminée proprement", data_dir.display(), old.pid, old.started_at);
    }
    let info = LockInfo { pid: std::process::id(), port, started_at: now_local() };
    let write = |file: &mut File| -> std::io::Result<()> {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(serde_json::to_string(&info).map_err(std::io::Error::other)?.as_bytes())?;
        file.sync_all()
    };
    if let Err(e) = write(&mut file) {
        // The lock itself is what counts; the details are for the message.
        log::warn!("{} : {e}", path.display());
    }
    Ok(InstanceLock { file, path, previous })
}

impl Drop for InstanceLock {
    /// Clean exit: empty the file (so the next start doesn't report a
    /// crash), then closing it releases the lock. The file stays: removing
    /// it would let a racing start lock a deleted file.
    fn drop(&mut self) {
        if let Err(e) = self.file.set_len(0) {
            log::warn!("{} : {e}", self.path.display());
        }
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-instance-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_wins_second_is_refused_with_the_pid() {
        let dir = temp_dir("second");
        let first = acquire(&dir, 8123).unwrap();
        assert_eq!(first.previous, None);
        match acquire(&dir, 8124) {
            Err(LockError::Held(Some(info))) => {
                assert_eq!((info.pid, info.port), (std::process::id(), 8123));
                let msg = LockError::Held(Some(info)).message(&dir);
                assert!(msg.starts_with("Laser Studio utilise déjà ce dossier"), "{msg}");
                assert!(msg.contains("port 8123") && msg.contains("--data-dir"), "{msg}");
            }
            Err(e) => panic!("expected Held with info, got {e:?}"),
            Ok(_) => panic!("a second lock on the same folder"),
        }
        drop(first);
        let again = acquire(&dir, 8125).unwrap();
        assert_eq!(again.previous, None, "a clean exit is not reported as a crash");
        drop(again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_lock_file_is_taken_over() {
        let dir = temp_dir("stale");
        // What a crashed run leaves: the file with its details, no lock.
        std::fs::write(dir.join(LOCK_FILE), r#"{"pid":4321,"port":8080,"started_at":"2026-10-01 20:00:00"}"#).unwrap();
        let lock = acquire(&dir, 8126).unwrap();
        assert_eq!(lock.previous.as_ref().map(|i| i.pid), Some(4321));
        let written: LockInfo = serde_json::from_str(&std::fs::read_to_string(dir.join(LOCK_FILE)).unwrap()).unwrap();
        assert_eq!((written.pid, written.port), (std::process::id(), 8126));
        drop(lock);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn released_on_drop_and_the_file_is_emptied() {
        let dir = temp_dir("drop");
        let lock = acquire(&dir, 8127).unwrap();
        assert!(dir.join(LOCK_FILE).exists());
        drop(lock);
        assert_eq!(std::fs::read_to_string(dir.join(LOCK_FILE)).unwrap(), "");
        assert!(acquire(&dir, 8128).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn two_folders_two_locks() {
        let (a, b) = (temp_dir("a"), temp_dir("b"));
        let la = acquire(&a, 8129).unwrap();
        let lb = acquire(&b, 8130).unwrap();
        drop((la, lb));
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
