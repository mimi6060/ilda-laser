//! Keep the Mac awake during a show (T-412).
//!
//! A Mac that goes to sleep mid-show stops the engine: the audience sees
//! the laser freeze, then go dark. While the laser is armed or a timeline
//! plays, the studio holds a system power assertion that keeps the machine
//! and the display awake; it lets go 5 s after both have stopped, so a
//! short pause doesn't flip it on and off. Never held otherwise.
//!
//! The system call is behind `SleepBlocker`: on macOS an IOKit power
//! assertion (public API, `IOPMAssertionCreateWithName`), which the system
//! drops by itself if the process dies; a fake one in tests and e2e runs,
//! so a test never touches the machine's power settings. A failed
//! assertion blocks nothing: a warning in the log and in `/api/state`.
//!
//! The setting (`prefs.json` → `keep_awake`, on by default) belongs to
//! this machine, not to a project.

use crate::Shared;
use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the assertion outlives the show.
pub const RELEASE_AFTER: Duration = Duration::from_secs(5);
/// How often the thread looks at the show's state.
const POLL: Duration = Duration::from_millis(250);
/// The name the system shows in `pmset -g assertions` (ASCII: pmset
/// prints it byte by byte).
const REASON: &str = "Laser Studio: show en cours (laser arme ou timeline en lecture)";

/// The system's "don't sleep" switch.
pub trait SleepBlocker: Send {
    fn hold(&mut self, reason: &str) -> Result<()>;
    fn release(&mut self);
}

/// The state machine: holds while wanted, releases `RELEASE_AFTER` later.
pub struct KeepAwake {
    blocker: Box<dyn SleepBlocker>,
    held: bool,
    /// When `wanted` last went false while held.
    idle_since: Option<Instant>,
    /// The last failed `hold`, until one succeeds.
    error: Option<String>,
    /// `hold` failed during this wanted stretch: no retry until it ends.
    failed: bool,
}

impl KeepAwake {
    pub fn new(blocker: Box<dyn SleepBlocker>) -> Self {
        Self { blocker, held: false, idle_since: None, error: None, failed: false }
    }

    /// Called off the engine thread, a few times a second.
    pub fn update(&mut self, wanted: bool, now: Instant) {
        if wanted {
            self.idle_since = None;
            if !self.held && !self.failed {
                match self.blocker.hold(REASON) {
                    Ok(()) => {
                        self.held = true;
                        self.error = None;
                        log::info!("mise en veille bloquée pendant le show");
                    }
                    Err(e) => {
                        // A sleeping Mac is bad; refusing to play is worse.
                        log::warn!("impossible de bloquer la mise en veille : {e:#}");
                        self.error = Some(format!("{e:#}"));
                        self.failed = true;
                    }
                }
            }
            return;
        }
        self.failed = false;
        if !self.held {
            return;
        }
        let since = *self.idle_since.get_or_insert(now);
        if now.duration_since(since) >= RELEASE_AFTER {
            self.release_now();
        }
    }

    /// Lets go at once (setting switched off, shutdown).
    pub fn release_now(&mut self) {
        if self.held {
            self.blocker.release();
            self.held = false;
            log::info!("mise en veille de nouveau autorisée");
        }
        self.idle_since = None;
    }

    /// Setting switched off: let go, forget any failure.
    pub fn disable(&mut self) {
        self.release_now();
        self.error = None;
        self.failed = false;
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.release_now();
    }
}

/// Setting and status, in `Shared` (`/api/state` → `keep_awake`).
#[derive(Clone, Debug, Serialize)]
pub struct KeepAwakeState {
    pub enabled: bool,
    pub held: bool,
    pub error: Option<String>,
    #[serde(skip)]
    prefs_path: Option<PathBuf>,
}

impl KeepAwakeState {
    /// The setting from `<data-dir>/prefs.json` (on when absent).
    pub fn load(data_dir: &std::path::Path) -> Self {
        let prefs_path = data_dir.join("prefs.json");
        let prefs: Map<String, Value> = crate::load_json(&prefs_path);
        let enabled = prefs.get("keep_awake").and_then(Value::as_bool).unwrap_or(true);
        Self { enabled, held: false, error: None, prefs_path: Some(prefs_path) }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { enabled: true, held: false, error: None, prefs_path: None }
    }
}

/// Switches the setting and writes `prefs.json` (other keys kept; lock
/// released for the write). The thread applies it on its next pass.
pub fn set_enabled(shared: &Mutex<Shared>, enabled: bool) -> Result<()> {
    let path = {
        let mut s = shared.lock().unwrap();
        s.keep_awake.enabled = enabled;
        s.keep_awake.prefs_path.clone()
    };
    let Some(path) = path else { return Ok(()) };
    let mut prefs: Map<String, Value> = crate::load_json(&path);
    prefs.insert("keep_awake".into(), Value::Bool(enabled));
    crate::project::write_atomic(&path, &serde_json::to_vec_pretty(&prefs)?)
}

/// What the show wants right now: (setting on, armed or playing).
fn wanted(s: &Shared) -> (bool, bool) {
    (s.keep_awake.enabled, s.gate.is_armed() || s.timeline.is_playing())
}

/// One pass of the thread: read under the lock, call the system without
/// it, publish the status.
fn pass(shared: &Mutex<Shared>, keep: &mut KeepAwake, now: Instant) {
    let (enabled, show) = wanted(&shared.lock().unwrap());
    if enabled {
        keep.update(show, now);
    } else {
        keep.disable();
    }
    let mut s = shared.lock().unwrap();
    s.keep_awake.held = keep.held();
    s.keep_awake.error = keep.error().map(String::from);
}

/// Starts the keep-awake thread; it lets go and ends when `running` goes
/// false. A crash ends the process, and the system drops the assertion.
pub fn spawn(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>, blocker: Box<dyn SleepBlocker>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("keep-awake".into())
        .spawn(move || {
            let mut keep = KeepAwake::new(blocker);
            while running.load(Ordering::SeqCst) {
                pass(&shared, &mut keep, Instant::now());
                std::thread::sleep(POLL);
            }
            keep.release_now();
            if let Ok(mut s) = shared.lock() {
                s.keep_awake.held = false;
            }
        })
        .expect("failed to start the keep-awake thread")
}

/// The machine's real blocker, or none where there is no implementation.
pub fn system_blocker() -> Box<dyn SleepBlocker> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::PowerAssertion::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoBlocker)
    }
}

/// Elsewhere than macOS: nothing to hold, nothing to report.
#[cfg(not(target_os = "macos"))]
pub struct NoBlocker;

#[cfg(not(target_os = "macos"))]
impl SleepBlocker for NoBlocker {
    fn hold(&mut self, _reason: &str) -> Result<()> {
        Ok(())
    }
    fn release(&mut self) {}
}

/// Tests and e2e runs (`--test-keep-awake`, `--test-hooks`): holds nothing
/// on the machine, only counts.
#[derive(Clone, Default)]
pub struct FakeBlocker {
    pub calls: Arc<Mutex<Vec<&'static str>>>,
    pub fail: Arc<AtomicBool>,
}

impl SleepBlocker for FakeBlocker {
    fn hold(&mut self, _reason: &str) -> Result<()> {
        self.calls.lock().unwrap().push("hold");
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("assertion refusée (test)");
        }
        Ok(())
    }
    fn release(&mut self) {
        self.calls.lock().unwrap().push("release");
    }
}

#[cfg(target_os = "macos")]
mod mac {
    //! IOKit power assertions (IOPMLib.h, public API). Both kinds are
    //! taken: the display stays on (the operator's screen) and so does
    //! the machine. The system releases them if the process exits or dies.

    use anyhow::{bail, Result};
    use std::ffi::{c_char, c_void, CString};

    type CFStringRef = *const c_void;
    type IOPMAssertionID = u32;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const K_IO_RETURN_SUCCESS: i32 = 0;
    const TYPES: [&str; 2] = ["PreventUserIdleDisplaySleep", "PreventUserIdleSystemSleep"];

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: *const c_void, c_str: *const c_char, encoding: u32) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut IOPMAssertionID) -> i32;
        fn IOPMAssertionRelease(id: IOPMAssertionID) -> i32;
    }

    /// An owned CFString, released on drop.
    struct CfString(CFStringRef);

    impl CfString {
        fn new(s: &str) -> Result<Self> {
            let c = CString::new(s.replace('\0', ""))?;
            // SAFETY: a valid NUL-terminated UTF-8 string; the result is
            // owned by us (Create rule) and released in Drop.
            let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) };
            if r.is_null() {
                bail!("CFStringCreateWithCString a échoué");
            }
            Ok(Self(r))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: created by CFStringCreateWithCString, released once.
            unsafe { CFRelease(self.0) }
        }
    }

    #[derive(Default)]
    pub struct PowerAssertion {
        ids: Vec<IOPMAssertionID>,
    }

    impl super::SleepBlocker for PowerAssertion {
        fn hold(&mut self, reason: &str) -> Result<()> {
            self.release();
            let name = CfString::new(reason)?;
            for kind in TYPES {
                let kind_cf = CfString::new(kind)?;
                let mut id: IOPMAssertionID = 0;
                // SAFETY: valid CFStrings for the call's duration, `id` is
                // a valid out pointer.
                let ret = unsafe { IOPMAssertionCreateWithName(kind_cf.0, K_IOPM_ASSERTION_LEVEL_ON, name.0, &mut id) };
                if ret != K_IO_RETURN_SUCCESS {
                    self.release();
                    bail!("IOPMAssertionCreateWithName({kind}) : erreur 0x{ret:08x}");
                }
                self.ids.push(id);
            }
            Ok(())
        }

        fn release(&mut self) {
            for id in self.ids.drain(..) {
                // SAFETY: an id returned by IOPMAssertionCreateWithName,
                // released once.
                let ret = unsafe { IOPMAssertionRelease(id) };
                if ret != K_IO_RETURN_SUCCESS {
                    log::warn!("IOPMAssertionRelease : erreur 0x{ret:08x}");
                }
            }
        }
    }

    impl Drop for PowerAssertion {
        fn drop(&mut self) {
            super::SleepBlocker::release(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    fn fake() -> (KeepAwake, FakeBlocker) {
        let b = FakeBlocker::default();
        (KeepAwake::new(Box::new(b.clone())), b)
    }

    fn calls(b: &FakeBlocker) -> Vec<&'static str> {
        b.calls.lock().unwrap().clone()
    }

    fn s(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn armed_holds_once() {
        let (mut k, b) = fake();
        let t0 = Instant::now();
        for i in 0..20 {
            k.update(true, t0 + s(i * 250));
        }
        assert!(k.held());
        assert_eq!(calls(&b), ["hold"]);
    }

    #[test]
    fn released_five_seconds_after_and_not_before() {
        let (mut k, b) = fake();
        let t0 = Instant::now();
        k.update(true, t0);
        k.update(false, t0 + s(1_000));
        k.update(false, t0 + s(5_999));
        assert!(k.held(), "still within 5 s");
        assert_eq!(calls(&b), ["hold"]);
        k.update(false, t0 + s(6_000));
        assert!(!k.held());
        assert_eq!(calls(&b), ["hold", "release"]);
        k.update(false, t0 + s(9_000));
        assert_eq!(calls(&b), ["hold", "release"], "released once");
    }

    #[test]
    fn rearmed_within_the_delay_keeps_holding() {
        let (mut k, b) = fake();
        let t0 = Instant::now();
        k.update(true, t0);
        k.update(false, t0 + s(1_000));
        k.update(true, t0 + s(4_000));
        k.update(false, t0 + s(4_250));
        k.update(false, t0 + s(8_000));
        assert!(k.held(), "the delay starts again from the last stop");
        k.update(false, t0 + s(9_250));
        assert!(!k.held());
        assert_eq!(calls(&b), ["hold", "release"]);
    }

    #[test]
    fn a_failed_hold_does_not_panic_and_retries_on_the_next_show() {
        let (mut k, b) = fake();
        b.fail.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        k.update(true, t0);
        k.update(true, t0 + s(250));
        assert!(!k.held());
        assert!(k.error().is_some());
        assert_eq!(calls(&b), ["hold"], "no retry every pass");
        b.fail.store(false, Ordering::SeqCst);
        k.update(false, t0 + s(500));
        assert!(k.error().is_some(), "still shown until a hold works");
        k.update(true, t0 + s(750));
        assert!(k.held());
        assert_eq!(k.error(), None);
        assert_eq!(calls(&b), ["hold", "hold"]);
    }

    #[test]
    fn dropping_releases() {
        let (mut k, b) = fake();
        k.update(true, Instant::now());
        drop(k);
        assert_eq!(calls(&b), ["hold", "release"]);
    }

    #[test]
    fn the_thread_pass_follows_arming_the_timeline_and_the_setting() {
        let shared = Mutex::new(test_support::shared());
        let (mut k, b) = fake();
        let t0 = Instant::now();
        pass(&shared, &mut k, t0);
        assert!(!shared.lock().unwrap().keep_awake.held, "disarmed and stopped: allowed");

        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        pass(&shared, &mut k, t0 + s(250));
        assert!(shared.lock().unwrap().keep_awake.held);

        // A failure blocks neither arming nor playing.
        shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        pass(&shared, &mut k, t0 + s(500));
        pass(&shared, &mut k, t0 + s(5_500));
        assert!(!shared.lock().unwrap().keep_awake.held);
        b.fail.store(true, Ordering::SeqCst);
        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        pass(&shared, &mut k, t0 + s(6_000));
        {
            let st = shared.lock().unwrap();
            assert!(st.gate.is_armed());
            assert!(!st.keep_awake.held);
            assert!(st.keep_awake.error.is_some());
        }
        b.fail.store(false, Ordering::SeqCst);
        shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        pass(&shared, &mut k, t0 + s(6_250));

        // Setting off: nothing is ever held, even armed.
        let before = calls(&b).len();
        shared.lock().unwrap().keep_awake.enabled = false;
        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        pass(&shared, &mut k, t0 + s(6_500));
        let st = shared.lock().unwrap();
        assert!(!st.keep_awake.held);
        assert_eq!(st.keep_awake.error, None);
        assert_eq!(calls(&b).len(), before, "no assertion taken");
    }

    #[test]
    fn a_playing_timeline_alone_holds() {
        let shared = Mutex::new(test_support::shared());
        let (mut k, _b) = fake();
        {
            let mut st = shared.lock().unwrap();
            let cue = st.presets[0].id.clone();
            let event = crate::timeline::Event { id: 1, start: 0.0, len: 60.0, source: crate::timeline::EventSource::Cue { id: cue }, ..Default::default() };
            let show = crate::timeline::Show { name: "t".into(), tracks: vec![crate::timeline::Track { layer: 1, events: vec![event], ..Default::default() }], ..Default::default() };
            st.timeline.load(show);
            crate::controls::timeline_play(&mut st).unwrap();
            assert!(!st.gate.is_armed());
        }
        pass(&shared, &mut k, Instant::now());
        assert!(shared.lock().unwrap().keep_awake.held);
    }

    #[test]
    fn the_setting_is_kept_in_prefs_with_the_other_keys() {
        let dir = std::env::temp_dir().join(format!("laser-studio-keep-awake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prefs.json"), r#"{"autosave":{"delay_s":30},"other":7}"#).unwrap();
        assert!(KeepAwakeState::load(&dir).enabled, "on by default");
        let mut shared = test_support::shared();
        shared.keep_awake = KeepAwakeState::load(&dir);
        let shared = Mutex::new(shared);
        set_enabled(&shared, false).unwrap();
        assert!(!KeepAwakeState::load(&dir).enabled);
        let prefs: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("prefs.json")).unwrap()).unwrap();
        assert_eq!(prefs["other"], 7);
        assert_eq!(prefs["autosave"]["delay_s"], 30);
        let _ = std::fs::remove_dir_all(dir);
    }
}
