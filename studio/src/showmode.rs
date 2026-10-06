//! Show mode (T-283): a lock against accidental clicks during a show.
//!
//! While it is on, the studio plays but doesn't edit: cues, pages, the
//! playlist, masters, tempo, timeline transport, blackout, arming and the
//! emergency stop keep working; settings, safety, calibration, power caps,
//! scenes, cue properties, figures, shows, MIDI mappings, test patterns
//! and opening another project are refused with `423 Locked`.
//!
//! The policy is an allow-list (`allowed`): a POST route that isn't known
//! to be playback is refused, so a new editing route is locked by default.
//! GET requests are never refused. The lock never arms anything, never
//! blocks the e-stop (it has its own fast path before routing anyway), a
//! disarm or a brightness cut. MIDI only drives controls (controls.rs),
//! which are all playback; MIDI learn is cancelled and can't bind while
//! the lock is on.
//!
//! The state is a machine preference (`prefs.json` → `show_mode`), like
//! `keep_awake`: the studio comes back locked after a restart if it was
//! locked. Entering and leaving are written to the safety log.

use crate::Shared;
use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The 423 body, also the UI's label.
pub const LOCKED_FR: &str = "Verrouillé en mode spectacle";

#[derive(Debug, Serialize)]
pub struct ShowMode {
    pub on: bool,
    #[serde(skip)]
    prefs_path: Option<PathBuf>,
}

impl ShowMode {
    /// The last state from `<data-dir>/prefs.json` (off when absent).
    pub fn load(data_dir: &Path) -> Self {
        let prefs_path = data_dir.join("prefs.json");
        let prefs: Map<String, Value> = crate::load_json(&prefs_path);
        let on = prefs.get("show_mode").and_then(Value::as_bool).unwrap_or(false);
        Self { on, prefs_path: Some(prefs_path) }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { on: false, prefs_path: None }
    }
}

/// Whether a POST to `path` (no query) may run while show mode is on.
/// Playback and safety only: everything else is an edit.
pub fn allowed(path: &str) -> bool {
    const PLAY: &[&str] = &[
        // Safety: never locked.
        "/api/estop",
        "/api/estop/reset",
        "/api/arm",
        "/api/heartbeat",
        "/api/lock",
        // The tablet remote (T-428): cutting it or a remote is always
        // allowed; the handler refuses opening it in show mode.
        "/api/remote",
        "/api/remote/kick",
        // The external inputs gate (T-408) is a live switch, like arming,
        // and so is TC-IN (T-168); the timecode settings are an edit.
        "/api/external/gate",
        "/api/timecode/arm",
        // Leaving a pattern is allowed; the handler refuses showing one.
        "/api/testpattern",
        "/api/data/warnings/ack",
        // Back above the audience is allowed; the handler refuses unlocking.
        "/api/audience",
        "/api/keep_awake",
        // The guided tour seen or not (T-437): a view, not an edit.
        "/api/onboarding",
        "/api/devices/scan",
        // Playing.
        "/api/cue",
        "/api/control",
        // The command line (T-424): controls, cues, page, tempo, stops only.
        "/api/command",
        "/api/live",
        // Speed groups and the *Cadence* (T-419): played live, like the tempo.
        "/api/speed-groups",
        "/api/scenes/play",
        "/api/playlist/start",
        "/api/playlist/stop",
        "/api/presets/play",
        // « Préparer » (T-274): a cue loaded blind and sent, playback only.
        "/api/prepared",
        "/api/prepared/adjust",
        "/api/prepared/send",
        "/api/prepared/clear",
        // The cue list's transport (T-410); saving lists is an edit.
        "/api/cuelist/load",
        "/api/cuelist/go",
        "/api/cuelist/back",
        "/api/cuelist/pause",
        "/api/cuelist/top",
        "/api/cuelist/goto",
        // « En cours » (T-423): stop buttons, playback only.
        "/api/running/stop",
        // Macros (T-425): running, stopping and Go are playback; saving
        // the list is an edit.
        "/api/macros/run",
        "/api/macros/stop",
        "/api/macros/go",
        // The show player (T-444): its transport and loop are playback;
        // saving the list is an edit.
        "/api/showlist/play",
        "/api/showlist/stop",
        "/api/showlist/next",
        "/api/showlist/prev",
        "/api/showlist/loop",
        "/api/audio",
        "/api/audio/tempo/new_track",
        "/api/timeline/load",
        "/api/timeline/play",
        "/api/timeline/pause",
        "/api/timeline/stop",
        "/api/timeline/seek",
        "/api/timeline/loop",
        "/api/midi/learn/cancel",
        // The on-screen APC40 (T-210) plays like the real one; which model
        // it draws is a view. Removing a mapping is an edit.
        "/api/midi/virtual",
        "/api/midi/virtual/model",
        // Writing the open project to its own file loses nothing.
        "/api/project/save",
        // A copy of the open project to the backup folder (T-450): same.
        "/api/backup/now",
        "/api/project/recover-ignore",
        // Cancelling an ILDA recording (T-442) writes nothing; starting one is
        // a file write, locked.
        "/api/ilda/record/cancel",
        // Ticking the pre-show checklist is not editing it.
        "/api/checklist/operator",
        "/api/checklist/check",
        "/api/checklist/complete",
        "/api/checklist/restart",
    ];
    PLAY.contains(&path) || path.starts_with("/api/test/")
}

/// Turns show mode on or off. Entering cancels a MIDI or DMX learn. Logged in
/// the safety log; `prefs.json` is written with the lock released (other
/// keys kept). Never arms or disarms. `source`: "ui", "keyboard", else
/// logged as "api".
pub fn set(shared: &Mutex<Shared>, on: bool, source: &str) -> Result<()> {
    let path = {
        let mut s = shared.lock().unwrap();
        if s.show_mode.on == on {
            return Ok(());
        }
        s.show_mode.on = on;
        if on {
            crate::midi::learn::cancel(&mut s);
            s.netin.patch.learn = None;
        }
        let source = match source {
            "ui" => "ui",
            "keyboard" => "keyboard",
            _ => "api",
        };
        s.gate.log().record("show_mode", Some(source), json!({ "on": on }));
        log::info!("mode spectacle {}", if on { "activé" } else { "désactivé" });
        s.show_mode.prefs_path.clone()
    };
    let Some(path) = path else { return Ok(()) };
    let mut prefs: Map<String, Value> = crate::load_json(&path);
    prefs.insert("show_mode".into(), Value::Bool(on));
    crate::project::write_atomic(&path, &serde_json::to_vec_pretty(&prefs)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn playback_and_safety_are_allowed_edits_are_not() {
        for p in ["/api/estop", "/api/arm", "/api/cue", "/api/control", "/api/scenes/play", "/api/timeline/play", "/api/cuelist/go", "/api/cuelist/goto", "/api/prepared", "/api/prepared/send", "/api/lock", "/api/test/stall", "/api/external/gate", "/api/timecode/arm", "/api/midi/virtual"] {
            assert!(allowed(p), "{p}");
        }
        for p in [
            "/api/settings",
            "/api/safety",
            "/api/calibration",
            "/api/outputs/limits",
            "/api/presence",
            "/api/scenes/save",
            "/api/scenes/delete",
            "/api/cues/slot",
            "/api/projection",
            "/api/projection/align",
            "/api/checklist/config",
            "/api/project/open",
            "/api/project/new",
            "/api/project/save-as",
            "/api/midi/learn",
            "/api/midi/safety",
            "/api/midi/virtual/unmap",
            "/api/external/scopes",
            "/api/timecode",
            "/api/audio/routes",
            "/api/audio/preset",
            "/api/audio/triggers",
            "/api/dmx/config",
            "/api/dmx/learn",
            "/api/netin",
            "/api/timeline/edit",
            "/api/timeline/markers/parse",
            "/api/timeline/templates",
            "/api/timeline/templates/render",
            "/api/timeline/templates/delete",
            "/api/figures/delete",
            "/api/cuelists",
            "/api/cuelists/delete",
            "/api/fx",
            "/api/some/future/route",
        ] {
            assert!(!allowed(p), "{p}");
        }
    }

    #[test]
    fn the_state_survives_a_restart_and_is_logged() {
        let dir = crate::datafile::tests::temp_dir("showmode");
        std::fs::write(dir.join("prefs.json"), r#"{"keep_awake":false}"#).unwrap();
        assert!(!ShowMode::load(&dir).on, "off by default");
        let (log, sink) = crate::safety_log::testing::memory_log();
        let mut s = test_support::shared();
        s.show_mode = ShowMode::load(&dir);
        s.gate.set_log(log);
        let shared = Mutex::new(s);
        set(&shared, true, "ui").unwrap();
        set(&shared, true, "ui").unwrap(); // no change: not logged twice
        assert!(ShowMode::load(&dir).on);
        let prefs: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("prefs.json")).unwrap()).unwrap();
        assert_eq!(prefs["keep_awake"], false, "other keys kept");
        set(&shared, false, "api").unwrap();
        assert!(!ShowMode::load(&dir).on);
        assert!(!shared.lock().unwrap().gate.is_armed(), "never arms");
        shared.lock().unwrap().gate.log().flush(std::time::Duration::from_secs(2));
        let kinds: Vec<(String, Value)> = sink.events().iter().map(|e| (e.kind.clone(), e.detail.clone())).collect();
        assert_eq!(kinds, vec![("show_mode".to_string(), json!({ "on": true })), ("show_mode".to_string(), json!({ "on": false }))]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
