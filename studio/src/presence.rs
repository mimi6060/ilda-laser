//! Operator presence (T-252): the UI heartbeat and hold-to-run.
//!
//! The engine runs on the server and the operator sits at the browser. If
//! every page is closed, frozen or stops beating, nobody can press Escape
//! any more: the laser disarms with reason « Interface perdue », and held
//! flashes are released. In hold-to-run ("dead man") mode, the output is
//! black unless a key or pad is held; a long release disarms.
//!
//! Everything here is plain state updated with an explicit `now`, so the
//! timing is tested with simulated time. `Shared::sync_presence` applies
//! the verdict to the arming gate.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// `ui_timeout_ms` bounds. Below 1 s, background-tab timer throttling
/// (about one tick a second) would disarm by itself; above 10 s the
/// operator is gone for too long (T-252 acceptance).
pub const UI_TIMEOUT_MIN_MS: u32 = 1_000;
pub const UI_TIMEOUT_MAX_MS: u32 = 10_000;
/// A hold reported by a page counts this long (or `ui_timeout_ms` if
/// shorter): a page that dies with the key down stops holding quickly.
const HOLD_STALE: Duration = Duration::from_millis(1_000);
/// Hold-to-run: every page hidden this long → black output.
const HIDDEN_BLANK: Duration = Duration::from_secs(5);
/// More pages than this is a runaway client: the oldest beat is dropped.
const MAX_CLIENTS: usize = 64;
const DEFAULT_HOLD_KEY: &str = "ShiftRight";
/// Keys that already have a safety meaning cannot be the hold key.
const RESERVED_KEYS: [&str; 2] = ["Space", "Escape"];

/// Saved to presence.json in the data directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceSettings {
    /// No heartbeat from any page for this long → disarm (ms).
    pub ui_timeout_ms: u32,
    /// Hold-to-run ("homme mort"): emit only while the hold key or pad is held.
    pub hold_to_run: bool,
    /// Hold-to-run: released for this long while armed → disarm (s).
    pub hold_release_disarm_s: f32,
    /// `KeyboardEvent.code` of the hold key (a USB pedal that sends a key works too).
    pub hold_key: String,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self { ui_timeout_ms: 2_000, hold_to_run: false, hold_release_disarm_s: 10.0, hold_key: DEFAULT_HOLD_KEY.into() }
    }
}

impl PresenceSettings {
    /// Brings every field into its safe range (from the API or a hand-edited file).
    pub fn sanitize(&mut self) {
        self.ui_timeout_ms = self.ui_timeout_ms.clamp(UI_TIMEOUT_MIN_MS, UI_TIMEOUT_MAX_MS);
        self.hold_release_disarm_s =
            if self.hold_release_disarm_s.is_finite() { self.hold_release_disarm_s.clamp(1.0, 60.0) } else { 10.0 };
        let key = self.hold_key.trim();
        let valid = !key.is_empty() && key.len() <= 32 && key.chars().all(|c| c.is_ascii_alphanumeric()) && !RESERVED_KEYS.contains(&key);
        self.hold_key = if valid { key.to_string() } else { DEFAULT_HOLD_KEY.into() };
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.ui_timeout_ms.clamp(UI_TIMEOUT_MIN_MS, UI_TIMEOUT_MAX_MS) as u64)
    }
}

/// One page's latest heartbeat.
#[derive(Clone, Copy, Debug)]
struct Beat {
    at: Instant,
    visible: bool,
    hold: bool,
}

/// What `Presence::update` decided for this instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Verdict {
    /// At least one page beat within `ui_timeout_ms`.
    pub ui_alive: bool,
    /// The last page just went away (alive → lost): reported once.
    pub ui_lost: bool,
    /// Whether the output may emit. Always true when hold-to-run is off.
    pub hold_ok: bool,
    /// Hold-to-run: released longer than `hold_release_disarm_s` while armed.
    pub hold_expired: bool,
}

pub struct Presence {
    pub settings: PresenceSettings,
    /// Only the real studio enforces presence; unit tests of other modules
    /// build a `Shared` without pages and must still be able to arm.
    pub enforced: bool,
    path: Option<PathBuf>,
    clients: HashMap<String, Beat>,
    /// `safety.hold` from a controller pad (released when it is unplugged).
    midi_hold: bool,
    was_alive: bool,
    hidden_since: Option<Instant>,
    released_since: Option<Instant>,
    last: Verdict,
}

impl Default for Presence {
    /// Not enforced, default settings, not saved.
    fn default() -> Self {
        Self::new(PresenceSettings::default(), false)
    }
}

impl Presence {
    pub fn new(mut settings: PresenceSettings, enforced: bool) -> Self {
        settings.sanitize();
        Self {
            settings,
            enforced,
            path: None,
            clients: HashMap::new(),
            midi_hold: false,
            was_alive: false,
            hidden_since: None,
            released_since: None,
            last: Verdict { hold_ok: true, ..Default::default() },
        }
    }

    /// Enforced presence with the settings saved in `path` (defaults if missing).
    pub fn load(path: PathBuf) -> Self {
        let settings: PresenceSettings = crate::load_json(&path);
        let mut p = Self::new(settings, true);
        p.path = Some(path);
        p
    }

    /// Replaces the settings (sanitized) and saves them.
    pub fn set_settings(&mut self, mut settings: PresenceSettings) {
        settings.sanitize();
        self.settings = settings;
        if let Some(path) = &self.path {
            crate::save_json(path, &self.settings);
        }
    }

    /// `POST /api/heartbeat` from page `id`.
    pub fn beat(&mut self, id: &str, now: Instant, visible: bool, hold: bool) {
        let id: String = id.chars().take(64).collect();
        if !self.clients.contains_key(&id) && self.clients.len() >= MAX_CLIENTS {
            if let Some(oldest) = self.clients.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| k.clone()) {
                self.clients.remove(&oldest);
            }
        }
        self.clients.insert(id, Beat { at: now, visible, hold });
    }

    /// Any other request from a known page (e.g. its arm request) proves it
    /// is alive; its visibility and hold are kept.
    pub fn touch(&mut self, id: &str, now: Instant) {
        match self.clients.get_mut(id) {
            Some(b) => b.at = now,
            None => self.beat(id, now, true, false),
        }
    }

    /// The page is closing (its `pagehide` beacon).
    pub fn leave(&mut self, id: &str) {
        self.clients.remove(id);
    }

    pub fn set_midi_hold(&mut self, on: bool) {
        self.midi_hold = on;
    }

    /// Ages out silent pages and decides, for `now`, whether the UI is
    /// alive and whether hold-to-run lets the output emit.
    pub fn update(&mut self, now: Instant, armed: bool) -> Verdict {
        let timeout = self.settings.timeout();
        let age = |b: &Beat| now.saturating_duration_since(b.at);
        self.clients.retain(|_, b| age(b) < timeout);
        let ui_alive = !self.clients.is_empty();
        let ui_lost = self.was_alive && !ui_alive;
        self.was_alive = ui_alive;

        let (hold_ok, hold_expired) = if self.settings.hold_to_run {
            let hold_stale = timeout.min(HOLD_STALE);
            let held = self.midi_hold || self.clients.values().any(|b| b.hold && age(b) < hold_stale);
            let all_hidden = ui_alive && self.clients.values().all(|b| !b.visible);
            if all_hidden {
                self.hidden_since.get_or_insert(now);
            } else {
                self.hidden_since = None;
            }
            let hidden_long = self.hidden_since.is_some_and(|t| now.saturating_duration_since(t) >= HIDDEN_BLANK);
            let hold_ok = held && !hidden_long;
            if !armed || hold_ok {
                self.released_since = None;
            } else {
                self.released_since.get_or_insert(now);
            }
            let limit = Duration::from_secs_f32(self.settings.hold_release_disarm_s);
            (hold_ok, self.released_since.is_some_and(|t| now.saturating_duration_since(t) >= limit))
        } else {
            self.hidden_since = None;
            self.released_since = None;
            (true, false)
        };
        self.last = Verdict { ui_alive, ui_lost, hold_ok, hold_expired };
        self.last
    }

    /// For `/api/state`, `/api/frame` and `/api/presence`, as of the last update.
    pub fn status(&self) -> PresenceStatus {
        PresenceStatus {
            enforced: self.enforced,
            ui_alive: self.last.ui_alive,
            clients: self.clients.len(),
            hold_to_run: self.settings.hold_to_run,
            hold_key: self.settings.hold_key.clone(),
            hold_ok: self.last.hold_ok,
            midi_hold: self.midi_hold,
        }
    }
}

#[derive(Serialize)]
pub struct PresenceStatus {
    pub enforced: bool,
    pub ui_alive: bool,
    pub clients: usize,
    pub hold_to_run: bool,
    pub hold_key: String,
    pub hold_ok: bool,
    pub midi_hold: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn hold_mode() -> Presence {
        Presence::new(PresenceSettings { hold_to_run: true, ..Default::default() }, true)
    }

    #[test]
    fn a_beating_page_is_alive_until_the_timeout() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        assert!(!p.update(t0, false).ui_alive, "no page yet");
        p.beat("a", t0, true, false);
        assert!(p.update(t0 + ms(1_999), true).ui_alive);
        let v = p.update(t0 + ms(2_000), true);
        assert!(!v.ui_alive);
        assert!(v.ui_lost, "the loss is reported");
        assert!(!p.update(t0 + ms(2_100), true).ui_lost, "only once");
    }

    #[test]
    fn beats_every_500_ms_keep_it_alive_even_with_one_throttled_second() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        // A background tab: timers fire at most once a second.
        for (i, at) in [0, 500, 1_000, 2_000, 3_000, 4_000].iter().enumerate() {
            p.beat("a", t0 + ms(*at), false, false);
            assert!(p.update(t0 + ms(*at + 999), true).ui_alive, "beat {i}");
        }
    }

    #[test]
    fn two_pages_one_closed_stays_alive() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        p.beat("a", t0, true, false);
        p.beat("b", t0, true, false);
        p.leave("a");
        for step in 1..10 {
            let now = t0 + ms(step * 500);
            p.beat("b", now, true, false);
            let v = p.update(now, true);
            assert!(v.ui_alive && !v.ui_lost);
        }
        assert_eq!(p.status().clients, 1);
        // The last one closes: lost at once, without waiting for the timeout.
        p.leave("b");
        let v = p.update(t0 + ms(5_000), true);
        assert!(!v.ui_alive && v.ui_lost);
    }

    #[test]
    fn the_last_silent_page_is_lost_after_its_own_timeout() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        p.beat("a", t0, true, false);
        p.beat("b", t0 + ms(1_500), true, false);
        assert!(p.update(t0 + ms(2_500), true).ui_alive, "b still beats in time");
        assert_eq!(p.status().clients, 1, "a aged out");
        assert!(!p.update(t0 + ms(3_500), true).ui_alive);
    }

    #[test]
    fn touch_keeps_a_page_alive_and_counts_an_unknown_one() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        p.touch("a", t0);
        assert!(p.update(t0, false).ui_alive);
        p.touch("a", t0 + ms(1_500));
        assert!(p.update(t0 + ms(3_000), false).ui_alive);
    }

    #[test]
    fn timeout_is_bounded_and_keys_are_checked() {
        let mut s = PresenceSettings { ui_timeout_ms: 60_000, hold_release_disarm_s: f32::NAN, hold_key: "Escape".into(), ..Default::default() };
        s.sanitize();
        assert_eq!(s.ui_timeout_ms, UI_TIMEOUT_MAX_MS);
        assert_eq!(s.hold_release_disarm_s, 10.0);
        assert_eq!(s.hold_key, "ShiftRight");
        let mut s = PresenceSettings { ui_timeout_ms: 10, hold_release_disarm_s: 0.0, hold_key: " KeyH ".into(), ..Default::default() };
        s.sanitize();
        assert_eq!((s.ui_timeout_ms, s.hold_release_disarm_s, s.hold_key.as_str()), (UI_TIMEOUT_MIN_MS, 1.0, "KeyH"));
        for bad in ["", "Space", "<script>", "Key H"] {
            let mut s = PresenceSettings { hold_key: bad.into(), ..Default::default() };
            s.sanitize();
            assert_eq!(s.hold_key, "ShiftRight", "{bad:?}");
        }
    }

    #[test]
    fn a_saved_file_asking_for_a_long_timeout_is_clamped() {
        let s: PresenceSettings = serde_json::from_str(r#"{"ui_timeout_ms": 999999}"#).unwrap();
        assert_eq!(Presence::new(s, true).settings.ui_timeout_ms, UI_TIMEOUT_MAX_MS);
        let old: PresenceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, PresenceSettings::default());
    }

    #[test]
    fn hold_off_always_lets_the_output_emit() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        p.beat("a", t0, true, false);
        let v = p.update(t0 + ms(20_000), true);
        assert!(v.hold_ok && !v.hold_expired);
    }

    #[test]
    fn hold_to_run_blacks_out_on_release_and_resumes_on_press() {
        let t0 = Instant::now();
        let mut p = hold_mode();
        p.beat("a", t0, true, false);
        assert!(!p.update(t0, true).hold_ok, "not held: black");
        p.beat("a", t0 + ms(100), true, true);
        assert!(p.update(t0 + ms(100), true).hold_ok, "held: emits");
        p.beat("a", t0 + ms(600), true, true);
        assert!(p.update(t0 + ms(900), true).hold_ok);
        p.beat("a", t0 + ms(1_000), true, false);
        let v = p.update(t0 + ms(1_000), true);
        assert!(!v.hold_ok && !v.hold_expired, "released: black but still armed");
        p.beat("a", t0 + ms(1_200), true, true);
        assert!(p.update(t0 + ms(1_200), true).hold_ok, "pressed again: resumes");
    }

    #[test]
    fn released_ten_seconds_while_armed_disarms() {
        let t0 = Instant::now();
        let mut p = hold_mode();
        for step in 0..=21u64 {
            let now = t0 + ms(step * 500);
            p.beat("a", now, true, false);
            let v = p.update(now, true);
            assert_eq!(v.hold_expired, step >= 20, "at {} ms", step * 500);
        }
        // A press in time resets the count.
        let mut p = hold_mode();
        p.beat("a", t0, true, false);
        p.update(t0, true);
        p.beat("a", t0 + ms(9_000), true, true);
        p.update(t0 + ms(9_000), true);
        p.beat("a", t0 + ms(9_100), true, false);
        assert!(!p.update(t0 + ms(18_000), true).hold_expired);
        // Disarmed: never counts.
        let mut p = hold_mode();
        p.beat("a", t0, true, false);
        p.update(t0, false);
        p.beat("a", t0 + ms(15_000), true, false);
        assert!(!p.update(t0 + ms(15_000), false).hold_expired);
    }

    #[test]
    fn a_page_that_dies_with_the_key_down_stops_holding_within_a_second() {
        let t0 = Instant::now();
        let mut p = hold_mode();
        p.beat("a", t0, true, true);
        p.beat("b", t0, true, false);
        assert!(p.update(t0 + ms(900), true).hold_ok);
        p.beat("b", t0 + ms(1_000), true, false); // b keeps beating, a is gone
        assert!(!p.update(t0 + ms(1_000), true).hold_ok);
    }

    #[test]
    fn every_page_hidden_for_five_seconds_blacks_out_even_while_held() {
        let t0 = Instant::now();
        let mut p = hold_mode();
        p.set_midi_hold(true);
        for step in 0..=12u64 {
            let now = t0 + ms(step * 500);
            p.beat("a", now, false, false);
            assert_eq!(p.update(now, true).hold_ok, step < 10, "at {} ms", step * 500);
        }
        p.beat("a", t0 + ms(6_500), true, false);
        assert!(p.update(t0 + ms(6_500), true).hold_ok, "visible again");
        p.set_midi_hold(false);
        assert!(!p.update(t0 + ms(6_600), true).hold_ok);
    }

    #[test]
    fn a_runaway_client_cannot_grow_the_table_forever() {
        let t0 = Instant::now();
        let mut p = Presence::new(PresenceSettings::default(), true);
        for i in 0..200u64 {
            p.beat(&format!("c{i}"), t0 + ms(i), true, false);
        }
        p.update(t0 + ms(200), false);
        assert_eq!(p.status().clients, MAX_CLIENTS);
    }
}
