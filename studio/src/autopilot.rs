//! Pilote automatique (T-148): a « virtual LJ » that keeps the show moving
//! on the tempo when the operator steps away, or gives them a base they
//! play over by hand (« comme deux laseristes »).
//!
//! What it does: every `every_bars` bars (1, 2, 4 or 8) of the tempo clock
//! it presses one cue of the grid, *Dans l'ordre* or *Aléatoire* (never
//! the same cue twice in a row when there is a choice). The cues come from
//! the page shown, or from a selection of pages (`pages`): in *Aléatoire*
//! each change draws among all their cells, so the page changes at random
//! too; in *Dans l'ordre* it walks the pages one after the other. The page
//! the operator sees is never moved.
//!
//! The changes land on the bar grid: on the bars that are a multiple of
//! `every_bars` from bar 0 of the clock, which *Temps 1* (T-235) puts on
//! the music's downbeat, so a change every 4 or 8 bars follows the
//! phrases. The press is made on that boundary on the deck's own clock
//! (`controls::deck_clock`), inside the deck's catch-up window: a
//! quantised deck (T-159) starts it at once, with its transition (T-158).
//!
//! What it never does (copy the idea, never the content: our own logic):
//!
//! - **Never arms**, never touches a safety setting: it only presses cues
//!   (`controls::press_cue_as_at`, source `autopilot`). Disarmed, the cues
//!   change in the preview. What it starts goes through the mixer and the
//!   whole output safety stage like any cue.
//! - **Stops for good** on an emergency stop (the blackout is one) and on
//!   a disarm, logged; it does not come back by itself, and it can't be
//!   switched on while the stop is latched. The switch (`vlj.enabled`,
//!   MIDI-assignable) is memory only and off at every start.
//! - **Operator first**: a cue started by hand (grid, keyboard, MIDI,
//!   macro…) or the autopilot's cue stopped by hand restarts its count:
//!   the next change waits `every_bars` full bars from that bar.
//! - **Doesn't fight a playback**: while the timeline, the show player
//!   (T-444) or a cue list (T-410) plays, it waits (*En attente*); that
//!   includes what the agenda (T-455) starts, and an agenda *Arrêt*
//!   disarms, which switches it off.
//! - **Content it skips**: cells marked *Ignorer par le pilote*
//!   (`CueSlot::vlj_skip`), cells holding a show, flash and solo cues
//!   (held content), cues whose own strobe would flash faster than the
//!   strobe limiter allows at the current tempo, and cues tagged « public »
//!   (or « audience ») unless *Autoriser le contenu public* is ticked.
//! - **Bounded**: at most one change per bar and never two within
//!   `MIN_CHANGE_S`, whatever the tempo.
//!
//! The settings (pages, public content) are content: `autopilot.json`,
//! the history, locked in show mode. The switch, *Toutes les N mesures*
//! and *Ordre / Aléatoire* are live controls (`vlj.*`), like the grid's
//! quantisation. Picks are drawn from a seeded generator: tests are
//! deterministic.

use crate::controls::{self, ControlSource};
use crate::cues::ClickMode;
use crate::interlock::ArmSource;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// *Toutes les N mesures*.
pub const EVERY_BARS: [u8; 4] = [1, 2, 4, 8];
pub const EVERY_LABELS: [&str; 4] = ["Toutes les mesures", "Toutes les 2 mesures", "Toutes les 4 mesures", "Toutes les 8 mesures"];
pub const ORDER_LABELS: [&str; 2] = ["Dans l'ordre", "Aléatoire"];
/// Never two changes closer than this (s), whatever the tempo.
pub const MIN_CHANGE_S: f64 = 1.0;
/// Pages a selection may name.
pub const MAX_PAGES: usize = 64;
/// Cell tags that mark content made for the audience.
pub const PUBLIC_TAGS: [&str; 2] = ["public", "audience"];

/// What is saved (`autopilot.json`). The switch is not: off at start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Autopilot {
    /// One of `EVERY_BARS`.
    pub every_bars: u8,
    /// *Aléatoire* (else *Dans l'ordre*).
    pub random: bool,
    /// Pages (1-based) to draw from; empty = the page shown.
    pub pages: Vec<u8>,
    /// Cues tagged « public » may be picked.
    pub allow_audience: bool,
}

impl Default for Autopilot {
    fn default() -> Self {
        Self { every_bars: 4, random: false, pages: Vec::new(), allow_audience: false }
    }
}

impl Autopilot {
    /// `every_bars` one of `EVERY_BARS` (the nearest not shorter, else
    /// 4), pages in 1..=255, sorted, once each, at most `MAX_PAGES`.
    pub fn sanitized(mut self) -> Self {
        self.every_bars = EVERY_BARS.into_iter().find(|&n| n >= self.every_bars.max(1)).unwrap_or(8);
        self.pages.retain(|&p| p >= 1);
        self.pages.sort_unstable();
        self.pages.dedup();
        self.pages.truncate(MAX_PAGES);
        self
    }
}

/// The settings route's body (`POST /api/autopilot`): the content part
/// only; the rhythm and order are live controls.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AutopilotEdit {
    pub pages: Vec<u8>,
    pub allow_audience: bool,
}

/// SplitMix64: a small seeded generator (no new dependency).
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The next cue among `candidates` after `last`: the following one in
/// order, or a random one that isn't `last` when there is a choice.
fn choose(candidates: &[String], last: Option<&str>, random: bool, rng: &mut Rng) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let at = last.and_then(|l| candidates.iter().position(|c| c == l));
    if !random {
        return Some(at.map_or(0, |i| (i + 1) % candidates.len()));
    }
    match at {
        Some(i) if candidates.len() > 1 => {
            let k = rng.below(candidates.len() - 1);
            Some(if k >= i { k + 1 } else { k })
        }
        _ => Some(rng.below(candidates.len())),
    }
}

/// Why it waits instead of changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Waiting {
    /// The timeline, the show player or a cue list plays.
    Playback,
    /// No cue of the pages can be picked.
    NoCue,
}

impl Waiting {
    pub fn label_fr(self) -> &'static str {
        match self {
            Waiting::Playback => "En attente : une lecture est en cours",
            Waiting::NoCue => "En attente : aucun cue utilisable sur ces pages",
        }
    }
}

/// `Shared.autopilot`: the settings, the switch and the running state.
#[derive(Debug)]
pub struct AutopilotStore {
    path: Option<PathBuf>,
    config: Autopilot,
    enabled: bool,
    rng: Rng,
    /// `EStop::trips` when last looked at, and the arm state of the last
    /// frame (a disarm switches it off).
    estop_trips: u64,
    was_armed: bool,
    /// The bar the count runs from (the last change, by it or by hand),
    /// and the last bar seen.
    anchor_bar: Option<i64>,
    last_bar: Option<i64>,
    last_change_t: Option<f64>,
    /// Its last pick, and the instance it started.
    last_pick: Option<String>,
    last_instance: Option<u64>,
    /// The deck's next instance id when last looked at: newer instances
    /// were started since.
    seen_id: u64,
    waiting: Option<Waiting>,
    changes: u64,
    yields: u64,
}

impl AutopilotStore {
    fn with(path: Option<PathBuf>, config: Autopilot) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x0148, |d| d.as_nanos() as u64);
        Self {
            path,
            config: config.sanitized(),
            enabled: false,
            rng: Rng(seed),
            estop_trips: 0,
            was_armed: false,
            anchor_bar: None,
            last_bar: None,
            last_change_t: None,
            last_pick: None,
            last_instance: None,
            seen_id: 0,
            waiting: None,
            changes: 0,
            yields: 0,
        }
    }

    /// From `autopilot.json` (missing or damaged: the defaults). Off.
    pub fn load(path: PathBuf) -> Self {
        let config: Autopilot = crate::load_json(&path);
        Self::with(Some(path), config)
    }

    pub fn in_memory() -> Self {
        Self::with(None, Autopilot::default())
    }

    /// The picks from now on are drawn from `seed` (tests, fuzz).
    pub fn seed(&mut self, seed: u64) {
        self.rng = Rng(seed);
    }

    pub fn config(&self) -> &Autopilot {
        &self.config
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The cue it pressed last.
    pub fn last_pick(&self) -> Option<&str> {
        self.last_pick.as_deref()
    }

    /// Cues it started since the studio started.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.config);
        }
    }

    /// The count starts afresh (switched on, settings changed).
    fn restart(&mut self) {
        self.anchor_bar = None;
        self.last_bar = None;
        self.waiting = None;
    }
}

/// Switches it off after an emergency stop (however it was tripped: the
/// HTTP fast path trips the latch without the lock) or a disarm. Called
/// by `Shared::emergency_stop` and every engine frame.
pub fn sync_stops(s: &mut Shared) {
    let trips = s.estop.trips();
    let armed = s.gate.is_armed();
    let disarmed = std::mem::replace(&mut s.autopilot.was_armed, armed) && !armed;
    let estop = trips != s.autopilot.estop_trips || s.estop.is_latched();
    s.autopilot.estop_trips = trips;
    if (estop || disarmed) && s.autopilot.enabled {
        s.autopilot.enabled = false;
        s.autopilot.restart();
        let reason = if estop { "estop" } else { "disarm" };
        log::info!("Pilote automatique coupé ({})", if estop { "arrêt d'urgence" } else { "laser désarmé" });
        s.gate.log().record("autopilot", Some(ArmSource::System.id()), json!({ "on": false, "reason": reason }));
    }
}

/// The switch (`vlj.enabled`). Switching on is refused while the
/// emergency stop is latched. Logged; never arms.
pub fn set_enabled(s: &mut Shared, on: bool, src: ArmSource) -> Result<(), &'static str> {
    sync_stops(s);
    if on == s.autopilot.enabled {
        return Ok(());
    }
    if on && s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réinitialisez-le avant d'activer le pilote automatique");
    }
    s.autopilot.enabled = on;
    s.autopilot.restart();
    s.autopilot.seen_id = s.deck.next_id();
    s.autopilot.last_instance = None;
    log::info!("Pilote automatique {}", if on { "activé" } else { "coupé" });
    s.gate.log().record("autopilot", Some(src.id()), json!({ "on": on, "reason": "operator" }));
    Ok(())
}

/// *Toutes les N mesures* (`vlj.every`), by index into `EVERY_BARS`.
pub fn set_every(s: &mut Shared, index: usize) {
    let n = EVERY_BARS[index.min(EVERY_BARS.len() - 1)];
    if s.autopilot.config.every_bars != n {
        s.autopilot.config.every_bars = n;
        s.autopilot.save();
    }
}

/// *Ordre / Aléatoire* (`vlj.order`).
pub fn set_random(s: &mut Shared, random: bool) {
    if s.autopilot.config.random != random {
        s.autopilot.config.random = random;
        s.autopilot.save();
    }
}

/// `POST /api/autopilot`: the pages and the public content. Pages must
/// exist. Saved; the count starts afresh.
pub fn set_config(s: &mut Shared, edit: AutopilotEdit) -> Result<(), String> {
    let pages = s.grid.page_count();
    if let Some(p) = edit.pages.iter().find(|&&p| p == 0 || p as usize > pages) {
        return Err(format!("page {p} inconnue (1 à {pages})"));
    }
    let config = Autopilot { pages: edit.pages, allow_audience: edit.allow_audience, ..s.autopilot.config.clone() }.sanitized();
    if config != s.autopilot.config {
        s.autopilot.config = config;
        s.autopilot.save();
        s.autopilot.restart();
    }
    Ok(())
}

/// The history's copy (T-285): the content part only.
pub fn capture(s: &Shared) -> Value {
    json!({ "pages": s.autopilot.config.pages, "allow_audience": s.autopilot.config.allow_audience })
}

/// Whether cue `id` may be picked: see the module's « Content it skips ».
pub fn eligible(s: &Shared, id: &str, bpm: f64) -> bool {
    let slot = s.deck.slot(id);
    if slot.vlj_skip || slot.show.is_some() {
        return false;
    }
    if matches!(slot.mode.unwrap_or(s.deck.click_mode), ClickMode::Flash | ClickMode::Solo) {
        return false;
    }
    if !s.autopilot.config.allow_audience && slot.tags.iter().any(|t| PUBLIC_TAGS.contains(&t.trim().to_lowercase().as_str())) {
        return false;
    }
    let Some(settings) = controls::cue_settings(s, id) else { return false };
    // Its own strobe at this tempo, against the limiter's sustained rate.
    let div = if settings.strobe_div.is_finite() { settings.strobe_div.max(0.0) as f64 } else { f64::INFINITY };
    div * bpm.max(0.0) / 60.0 <= s.effective_safety().strobe_max_hz as f64 + 1e-6
}

/// The cues it may pick now, in order: the cells of its pages (the page
/// shown when none), row by row, each once.
pub fn candidates(s: &Shared, bpm: f64) -> Vec<String> {
    let view = s.grid.view();
    let pages: Vec<usize> = if s.autopilot.config.pages.is_empty() { vec![s.cue_page] } else { s.autopilot.config.pages.iter().map(|&p| p as usize - 1).collect() };
    let mut out: Vec<String> = Vec::new();
    for p in pages {
        let Some(page) = view.get(p) else { continue };
        for id in page.cells.iter().flatten() {
            if !out.contains(id) && eligible(s, id, bpm) {
                out.push(id.clone());
            }
        }
    }
    out
}

/// A source the operator drives (a press from it wins over the autopilot).
fn by_operator(src: ControlSource) -> bool {
    !matches!(src, ControlSource::Autopilot | ControlSource::Audio | ControlSource::System | ControlSource::Timeline)
}

/// Whether a playback it must not fight is running.
fn playback_running(s: &Shared) -> bool {
    s.timeline.is_playing() || s.showlist.is_active() || s.cue_list.is_active()
}

/// One engine frame at `t` (studio clock), under the lock, before the
/// deck's quantised launches: on the bar it is due, one cue is pressed.
pub fn frame(s: &mut Shared, t: f64) {
    sync_stops(s);
    if !s.autopilot.enabled {
        return;
    }
    let beat = s.tempo.effect_beat_at(t);
    let bpb = s.tempo.beats_per_bar.max(1) as f64;
    let bar = ((beat + 1e-9) / bpb).floor() as i64;
    let every = s.autopilot.config.every_bars.max(1) as i64;

    // The operator's hand: a cue started (or waiting to start) by them
    // since the last frame, or its own cue stopped.
    let seen = s.autopilot.seen_id;
    let started_by_hand = s.deck.active.iter().any(|a| a.id >= seen && by_operator(a.source)) || s.deck.pending.iter().any(|p| by_operator(p.source));
    let own_stopped = s.autopilot.last_instance.is_some_and(|id| !s.deck.active.iter().any(|a| a.id == id));
    s.autopilot.seen_id = s.deck.next_id();
    if started_by_hand || own_stopped {
        s.autopilot.last_instance = None;
        s.autopilot.anchor_bar = Some(bar);
        s.autopilot.yields += 1;
    }

    if playback_running(s) {
        s.autopilot.waiting = Some(Waiting::Playback);
        s.autopilot.last_bar = Some(bar);
        return;
    }
    if s.autopilot.waiting == Some(Waiting::Playback) {
        s.autopilot.waiting = None;
    }
    // Only on the first frame of a bar; the first frame after switching
    // on only takes note (it may be mid-bar).
    match s.autopilot.last_bar.replace(bar) {
        None => return,
        Some(b) if b == bar => return,
        _ => {}
    }
    let due_bar = |anchor: i64| {
        let from = anchor + every;
        from + (every - from.rem_euclid(every)) % every
    };
    let aligned = bar.rem_euclid(every) == 0;
    let due = match s.autopilot.anchor_bar {
        Some(anchor) => bar >= due_bar(anchor),
        None => aligned,
    };
    if !due || !aligned || s.autopilot.last_change_t.is_some_and(|last| t - last < MIN_CHANGE_S) {
        return;
    }
    let bpm = s.tempo.bpm;
    let pool = candidates(s, bpm);
    let last = s.autopilot.last_pick.clone();
    let random = s.autopilot.config.random;
    let Some(i) = choose(&pool, last.as_deref(), random, &mut s.autopilot.rng) else {
        s.autopilot.waiting = Some(Waiting::NoCue);
        return;
    };
    s.autopilot.waiting = None;
    let id = pool[i].clone();
    let previous = s.autopilot.last_instance;
    let first_new = s.deck.next_id();
    controls::press_cue_as_at(s, &id, Some(ClickMode::Restart), true, Some("autopilot"), ControlSource::Autopilot, t);
    // Several cues at once (*Multi*): its previous cue makes way.
    if s.deck.multi {
        if let Some(prev) = previous.filter(|p| *p < first_new) {
            controls::stop_instance(s, prev);
        }
    }
    // None while a coarser quantisation holds it: nothing to watch then.
    s.autopilot.last_instance = s.deck.active.iter().find(|a| a.id >= first_new && a.cue == id).map(|a| a.id);
    s.autopilot.seen_id = s.deck.next_id();
    s.autopilot.last_pick = Some(id);
    s.autopilot.anchor_bar = Some(bar);
    s.autopilot.last_change_t = Some(t);
    s.autopilot.changes += 1;
}

/// The bar of the next change (none while off or waiting).
fn next_bar(s: &Shared, bar: i64) -> i64 {
    let every = s.autopilot.config.every_bars.max(1) as i64;
    match s.autopilot.anchor_bar {
        Some(anchor) => {
            let from = (anchor + every).max(bar + 1);
            from + (every - from.rem_euclid(every)) % every
        }
        None => {
            let from = bar + 1;
            from + (every - from.rem_euclid(every)) % every
        }
    }
}

/// What the page shows (`/api/frame`, the pushed state): only values that
/// change on a change, never every frame.
pub fn status(s: &Shared) -> Value {
    let a = &s.autopilot;
    let bar = a.last_bar;
    json!({
        "enabled": a.enabled,
        "every_bars": a.config.every_bars,
        "random": a.config.random,
        "pages": a.config.pages,
        "allow_audience": a.config.allow_audience,
        "last_pick": a.last_pick,
        "changes": a.changes,
        "yields": a.yields,
        "next_bar": if a.enabled && a.waiting.is_none() { bar.map(|b| next_bar(s, b)) } else { None },
        "waiting": a.waiting,
        "waiting_label": a.waiting.map(Waiting::label_fr),
    })
}

/// `GET /api/autopilot`: the status, the labels and the cues it may pick now.
pub fn view(s: &Shared) -> Value {
    let mut v = status(s);
    v["candidates"] = json!(candidates(s, s.tempo.bpm));
    v["every_options"] = json!(EVERY_BARS);
    v["pages_count"] = json!(s.grid.page_count());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cues::CueSlot;
    use crate::test_support;

    const FPS: f64 = 60.0;

    /// A studio at 120 BPM, bar 0 at t = 0, the autopilot on page 1 with
    /// a fixed seed.
    fn studio() -> Shared {
        let mut s = test_support::shared();
        s.tempo.bpm = 120.0;
        s.tempo.beats_per_bar = 4;
        s.autopilot.seed(7);
        s.cue_page = 0;
        s
    }

    fn page_cells(s: &Shared) -> Vec<String> {
        s.grid.view()[0].cells.iter().flatten().cloned().collect()
    }

    /// Runs the autopilot and the deck from `from` to `to` seconds at 60
    /// frames a second; returns (time, cue) of every change.
    fn run(s: &mut Shared, from: f64, to: f64) -> Vec<(f64, String)> {
        let mut out = Vec::new();
        let mut k = (from * FPS).ceil() as i64;
        while (k as f64) / FPS < to {
            let t = k as f64 / FPS;
            let before = s.autopilot.changes;
            frame(s, t);
            controls::deck_frame(s, t);
            if s.autopilot.changes > before {
                out.push((t, s.autopilot.last_pick.clone().unwrap()));
            }
            k += 1;
        }
        out
    }

    #[test]
    fn every_four_bars_at_120_bpm_is_a_change_every_eight_seconds() {
        let mut s = studio();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let changes = run(&mut s, 0.5, 60.0);
        assert!(changes.len() >= 6, "{changes:?}");
        for w in changes.windows(2) {
            let gap = w[1].0 - w[0].0;
            assert!((gap - 8.0).abs() <= 1.0 / FPS + 1e-9, "gap {gap}");
        }
        // On the phrase grid: bars 4, 8, 12… (every 8 s from t = 0).
        for (t, _) in &changes {
            assert!((t / 8.0 - (t / 8.0).round()).abs() * 8.0 <= 1.0 / FPS + 1e-9, "change at {t}");
        }
        assert!(!s.gate.is_armed(), "never arms");
    }

    #[test]
    fn every_bar_option_and_order_walk_the_page() {
        let mut s = studio();
        set_every(&mut s, 0);
        let cells = page_cells(&s);
        let eligible: Vec<String> = cells.iter().filter(|c| eligible(&s, c, 120.0)).cloned().collect();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let changes = run(&mut s, 0.1, 2.0 * 5.0 + 0.1);
        let picked: Vec<&str> = changes.iter().map(|(_, c)| c.as_str()).collect();
        assert_eq!(picked, eligible.iter().take(picked.len()).map(String::as_str).collect::<Vec<_>>());
        assert_eq!(picked.len(), 5, "one a bar (2 s)");
        assert_eq!(s.deck.primary().map(|a| a.cue.as_str()), picked.last().copied());
        assert_eq!(s.deck.active.last().unwrap().source, ControlSource::Autopilot);
    }

    #[test]
    fn random_never_plays_the_same_cue_twice_in_a_row_and_is_seeded() {
        let picks = |seed: u64| {
            let mut s = studio();
            s.autopilot.seed(seed);
            set_every(&mut s, 0);
            set_random(&mut s, true);
            set_enabled(&mut s, true, ArmSource::Ui).unwrap();
            run(&mut s, 0.1, 400.0).into_iter().map(|(_, c)| c).collect::<Vec<_>>()
        };
        let a = picks(11);
        assert!(a.len() > 150);
        assert!(a.windows(2).all(|w| w[0] != w[1]), "a repeat in {a:?}");
        assert_eq!(a, picks(11), "same seed, same picks");
        assert_ne!(a, picks(12));
        let distinct: std::collections::HashSet<_> = a.iter().collect();
        assert!(distinct.len() > 3);
        // The pure draw, on two candidates: always the other one.
        let two = vec!["x".to_string(), "y".to_string()];
        let mut rng = Rng(3);
        for _ in 0..50 {
            assert_eq!(choose(&two, Some("x"), true, &mut rng), Some(1));
        }
        assert_eq!(choose(&two[..1], Some("x"), true, &mut rng), Some(0), "one cue: it plays again");
        assert_eq!(choose(&[], None, true, &mut rng), None);
    }

    #[test]
    fn skipped_flash_show_strobe_and_public_cells_are_never_picked() {
        let mut s = studio();
        let cells = page_cells(&s);
        let (skip, flash, public, strobe, show) = (&cells[0], &cells[1], &cells[2], &cells[3], &cells[4]);
        s.deck.set_slot(skip, CueSlot { vlj_skip: true, ..Default::default() });
        s.deck.set_slot(flash, CueSlot { mode: Some(ClickMode::Flash), ..Default::default() });
        s.deck.set_slot(public, CueSlot { tags: vec!["Public".into()], ..Default::default() });
        s.deck.set_slot(show, CueSlot { show: Some("Un show".into()), ..Default::default() });
        // 8 flashes a beat at 120 BPM = 16 Hz, over the limiter's rate.
        let mut look = s.presets.iter().find(|p| &p.id == strobe).unwrap().settings.clone();
        look.strobe_div = 8.0;
        s.presets.iter_mut().find(|p| &p.id == strobe).unwrap().settings = look;
        assert!(16.0 > s.effective_safety().strobe_max_hz);
        set_every(&mut s, 0);
        set_random(&mut s, true);
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let picked: Vec<String> = run(&mut s, 0.1, 300.0).into_iter().map(|(_, c)| c).collect();
        assert!(picked.len() > 100);
        for never in [skip, flash, public, strobe, show] {
            assert!(!picked.contains(never), "{never} was picked");
        }
        // Slower tempo: the same strobe is within the limiter (8 × 30/60 = 4 Hz).
        assert!(!eligible(&s, strobe, 120.0));
        assert!(eligible(&s, strobe, 30.0));
        // Public content when the operator allows it.
        set_config(&mut s, AutopilotEdit { pages: vec![], allow_audience: true }).unwrap();
        assert!(eligible(&s, public, 120.0));
    }

    #[test]
    fn a_selection_of_pages_draws_from_them_only() {
        let mut s = studio();
        assert!(s.grid.page_count() >= 3);
        assert!(set_config(&mut s, AutopilotEdit { pages: vec![99], allow_audience: false }).is_err());
        set_config(&mut s, AutopilotEdit { pages: vec![2, 3], allow_audience: false }).unwrap();
        let allowed: Vec<String> = s.grid.view()[1..3].iter().flat_map(|p| p.cells.iter().flatten().cloned()).collect();
        set_every(&mut s, 0);
        set_random(&mut s, true);
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let picked: Vec<String> = run(&mut s, 0.1, 200.0).into_iter().map(|(_, c)| c).collect();
        assert!(picked.iter().all(|c| allowed.contains(c)));
        let page2 = &s.grid.view()[1].cells;
        assert!(picked.iter().any(|c| page2.iter().flatten().any(|x| x == c)), "page 2 drawn");
        let page3 = &s.grid.view()[2].cells;
        assert!(picked.iter().any(|c| page3.iter().flatten().any(|x| x == c)), "page 3 drawn");
        assert_eq!(s.cue_page, 0, "the page shown never moves");
    }

    #[test]
    fn a_manual_press_wins_and_restarts_the_count() {
        let mut s = studio();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let first = run(&mut s, 0.1, 8.5);
        assert_eq!(first.len(), 1, "bar 4 at 8 s");
        // The operator presses a cue at 10 s (bar 5).
        let manual = page_cells(&s).last().unwrap().clone();
        controls::press_cue_as_at(&mut s, &manual, Some(ClickMode::Restart), true, Some("ui"), ControlSource::Ui, 10.0);
        let after = run(&mut s, 10.0, 30.0);
        // Not at bar 8 (16 s): the first multiple of 4 at least 4 bars
        // after bar 5 is bar 12 (24 s).
        assert_eq!(after.first().map(|c| (c.0 * 60.0).round() / 60.0), Some(24.0), "{after:?}");
        assert!(s.autopilot.yields >= 1);
        // Its own cue stopped by hand: the same.
        let own = s.autopilot.last_instance.unwrap();
        controls::stop_instance(&mut s, own);
        let later = run(&mut s, 30.0, 40.0);
        assert!(later.is_empty(), "bar 15 stop: next change at bar 20 (40 s) at the earliest: {later:?}");
    }

    #[test]
    fn it_waits_while_a_playback_runs() {
        let mut s = studio();
        set_every(&mut s, 0);
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let event = crate::timeline::Event { id: 1, start: 0.0, len: 60.0, source: crate::timeline::EventSource::Cue { id: "nope".into() }, ..Default::default() };
        s.timeline.load(crate::timeline::Show { name: "Long".into(), tracks: vec![crate::timeline::Track { layer: 1, events: vec![event], ..Default::default() }], ..Default::default() });
        controls::timeline_play(&mut s).unwrap();
        assert!(run(&mut s, 0.1, 10.0).is_empty());
        assert_eq!(status(&s)["waiting"], "playback");
        s.timeline.stop();
        assert!(!run(&mut s, 10.0, 14.1).is_empty(), "resumes when the playback ends");
    }

    #[test]
    fn the_estop_and_a_disarm_switch_it_off_for_good() {
        let mut s = studio();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        s.emergency_stop(ArmSource::Ui);
        assert!(!s.autopilot.enabled());
        assert!(set_enabled(&mut s, true, ArmSource::Ui).is_err(), "not while latched");
        assert!(run(&mut s, 0.1, 20.0).is_empty());
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        frame(&mut s, 20.0);
        assert!(!s.autopilot.enabled(), "the reset doesn't switch it back on");

        let mut s = studio();
        controls::apply(&mut s, "transport.arm", controls::ControlInput::Value(1.0), false).unwrap();
        assert!(s.gate.is_armed());
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        frame(&mut s, 0.1);
        assert!(s.autopilot.enabled());
        s.gate.disarm(crate::interlock::DisarmReason::User, ArmSource::Ui);
        frame(&mut s, 0.2);
        assert!(!s.autopilot.enabled(), "a disarm ends it");
        assert!(run(&mut s, 0.3, 20.0).is_empty());
    }

    #[test]
    fn the_rate_is_bounded_whatever_the_tempo() {
        let mut s = studio();
        s.tempo.bpm = crate::tempo::MAX_BPM;
        s.tempo.beats_per_bar = 1;
        set_every(&mut s, 0);
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let changes = run(&mut s, 0.1, 30.0);
        assert!(changes.windows(2).all(|w| w[1].0 - w[0].0 >= MIN_CHANGE_S - 1e-9));
        assert!(changes.len() as f64 <= 30.0 / MIN_CHANGE_S + 1.0);
    }

    #[test]
    fn settings_load_with_defaults_and_are_sanitised() {
        let a: Autopilot = serde_json::from_str("{}").unwrap();
        assert_eq!(a, Autopilot::default());
        let a = Autopilot { every_bars: 3, pages: vec![3, 0, 1, 3], ..Default::default() }.sanitized();
        assert_eq!((a.every_bars, a.pages), (4, vec![1, 3]));
        assert_eq!(Autopilot { every_bars: 200, ..Default::default() }.sanitized().every_bars, 8);
        assert_eq!(Autopilot { every_bars: 0, ..Default::default() }.sanitized().every_bars, 1);
        let s = studio();
        assert!(!s.autopilot.enabled(), "off at start");
    }
}
