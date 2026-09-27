//! The cue deck: which cues are playing right now, and what a press on a
//! grid cell does to that list. A press can toggle a cue, restart it, or -
//! for flash and solo - hold it only while the key or pad stays down, after
//! which whatever played before comes back untouched.
//!
//! Cues are "latched" (they stay until stopped) or "held" (a flash: gone on
//! release). Exclusive groups and the `max_active` limiter only ever stop
//! latched cues, so a flash can't kill what it temporarily covers.
//!
//! The newest active cue is the *primary*: its look lives in
//! `Shared::settings`, so the look panel and the `look.*` controls edit the
//! cue on top. `controls::press_cue` keeps the two in sync.

use crate::engine::Settings;
use crate::layers::LAYER_COUNT;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Number of exclusive groups a cue can join (1..=MAX_GROUP).
pub const MAX_GROUP: u8 = 8;
pub const MAX_ACTIVE_LIMIT: u8 = 16;

/// What a press on a cue does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickMode {
    /// First press starts, second press stops.
    #[default]
    Toggle,
    /// Plays only while held; in « Un cue » mode it also hides the others.
    Flash,
    /// Plays only while held, alone: every other cue is hidden meanwhile.
    Solo,
    /// Every press starts the cue from the beginning.
    Restart,
}

pub const CLICK_MODES: [ClickMode; 4] = [ClickMode::Toggle, ClickMode::Flash, ClickMode::Solo, ClickMode::Restart];
pub const CLICK_MODE_LABELS: [&str; 4] = ["Basculer", "Flash", "Solo", "Relancer"];

/// Per-cue properties from the grid (« Propriétés du cue »). `None` means
/// "use the deck's setting".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueSlot {
    pub mode: Option<ClickMode>,
    /// Exclusive group 1..=8: starting a cue stops the other cues of its group.
    pub group: Option<u8>,
    /// Layer 1..=4 (layers.rs); `None` = layer 1.
    pub layer: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveCue {
    /// Instance id, unique for the life of the process: a restart gets a
    /// new one, so its animation starts over.
    pub id: u64,
    /// Preset id.
    pub cue: String,
    pub group: Option<u8>,
    /// Layer 1..=4, taken from the grid when the cue started.
    pub layer: u8,
    pub started_s: f64,
    pub started_beat: f64,
    /// Flash/solo: removed on release.
    pub held: bool,
    /// While held, only solo cues are shown.
    pub solo: bool,
    /// The cue's look. For the primary (newest) cue, `Shared::settings` is
    /// the live copy and this one is refreshed from it before every change.
    pub settings: Settings,
}

/// The playing cues plus the grid's trigger settings. Saved to
/// `studio-data/grid.json` without the playing list.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CueDeck {
    #[serde(skip)]
    pub active: Vec<ActiveCue>,
    #[serde(skip)]
    next_id: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
    /// The manual look (scene, playlist, look panel), set aside while only
    /// flashes play, and given back when they end.
    #[serde(skip)]
    pub parked: Option<Settings>,
    /// Default mode for cues without their own.
    pub click_mode: ClickMode,
    /// « Multi »: cues add up. Off (« Un cue »): a new cue replaces the
    /// others of its layer (with one layer in use: all the others).
    pub multi: bool,
    /// Latched cues allowed at once; the oldest is stopped beyond it.
    pub max_active: u8,
    /// Per-cue properties, keyed by preset id.
    pub slots: BTreeMap<String, CueSlot>,
}

impl Default for CueDeck {
    fn default() -> Self {
        Self {
            active: Vec::new(),
            next_id: 1,
            path: None,
            parked: None,
            click_mode: ClickMode::Toggle,
            multi: false,
            max_active: 4,
            slots: BTreeMap::new(),
        }
    }
}

/// When a press happened: wall clock and tempo-clock beat.
#[derive(Clone, Copy, Debug, Default)]
pub struct At {
    pub s: f64,
    pub beat: f64,
}

impl CueDeck {
    /// Loads `grid.json`, or starts empty. Changes are saved back to `path`.
    pub fn load(path: PathBuf) -> Self {
        let mut deck: CueDeck = crate::load_json(&path);
        deck.path = Some(path);
        deck.sanitize();
        deck
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(Path::new(path), self);
        }
    }

    fn sanitize(&mut self) {
        self.max_active = self.max_active.clamp(1, MAX_ACTIVE_LIMIT);
        self.next_id = self.next_id.max(1);
        for slot in self.slots.values_mut() {
            slot.group = slot.group.filter(|g| (1..=MAX_GROUP).contains(g));
            slot.layer = slot.layer.filter(|n| (2..=LAYER_COUNT as u8).contains(n));
        }
        self.slots.retain(|_, slot| *slot != CueSlot::default());
    }

    pub fn set_slot(&mut self, cue: &str, slot: CueSlot) {
        self.slots.insert(cue.to_string(), slot);
        self.sanitize();
    }

    pub fn set_max_active(&mut self, n: u8) {
        self.max_active = n.clamp(1, MAX_ACTIVE_LIMIT);
        self.enforce_limit();
    }

    pub fn slot(&self, cue: &str) -> CueSlot {
        self.slots.get(cue).cloned().unwrap_or_default()
    }

    /// The mode a press on `cue` uses: its own, else the deck's.
    pub fn mode_of(&self, cue: &str) -> ClickMode {
        self.slot(cue).mode.unwrap_or(self.click_mode)
    }

    /// A key or pad went down. `mode` overrides the cue's mode (Shift +
    /// letter flashes). `settings` builds the look of a newly started cue.
    pub fn press(&mut self, cue: &str, mode: Option<ClickMode>, at: At, settings: impl FnOnce() -> Settings) {
        let mode = mode.unwrap_or_else(|| self.mode_of(cue));
        let latched = self.active.iter().position(|a| a.cue == cue && !a.held);
        match mode {
            ClickMode::Toggle => match latched {
                Some(i) => {
                    self.active.remove(i);
                }
                None => self.start(cue, at, settings(), false, false),
            },
            ClickMode::Restart => {
                if let Some(i) = latched {
                    self.active.remove(i);
                }
                self.start(cue, at, settings(), false, false);
            }
            ClickMode::Flash => {
                if !self.active.iter().any(|a| a.cue == cue) {
                    // With a single cue at a time, a flash takes over the output.
                    self.start(cue, at, settings(), true, !self.multi);
                }
            }
            ClickMode::Solo => {
                if !self.active.iter().any(|a| a.cue == cue && a.held) {
                    self.start(cue, at, settings(), true, true);
                }
            }
        }
    }

    /// The key or pad came up: a held flash/solo of `cue` ends. Latched
    /// cues ignore releases.
    pub fn release(&mut self, cue: &str) {
        self.active.retain(|a| !(a.held && a.cue == cue));
    }

    pub fn stop_all(&mut self) {
        self.active.clear();
    }

    /// « Vider » a layer: every cue on it stops, held ones included.
    pub fn clear_layer(&mut self, layer: u8) {
        self.active.retain(|a| a.layer != layer);
    }

    fn start(&mut self, cue: &str, at: At, settings: Settings, held: bool, solo: bool) {
        let slot = self.slot(cue);
        let (group, layer) = (slot.group, slot.layer.unwrap_or(1));
        if !held {
            if !self.multi {
                // One cue per layer: a beam on layer 2 stays over the tunnel on layer 1.
                self.active.retain(|a| a.held || a.layer != layer);
            } else if group.is_some() {
                self.active.retain(|a| a.held || a.group != group);
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.active.push(ActiveCue {
            id,
            cue: cue.to_string(),
            group,
            layer,
            started_s: at.s,
            started_beat: at.beat,
            held,
            solo,
            settings,
        });
        self.enforce_limit();
    }

    /// Stop the oldest latched cues until at most `max_active` remain.
    fn enforce_limit(&mut self) {
        while self.active.iter().filter(|a| !a.held).count() > self.max_active.max(1) as usize {
            let Some(oldest) = self.active.iter().position(|a| !a.held) else { break };
            self.active.remove(oldest);
        }
    }

    /// The cues that actually reach the output, oldest first: held solo
    /// cues hide everything else; a held flash hides the latched cues of
    /// its group.
    pub fn visible(&self) -> Vec<&ActiveCue> {
        if self.active.iter().any(|a| a.held && a.solo) {
            return self.active.iter().filter(|a| a.held && a.solo).collect();
        }
        let flashed_groups: Vec<u8> = self.active.iter().filter(|a| a.held).filter_map(|a| a.group).collect();
        self.active
            .iter()
            .filter(|a| a.held || !a.group.is_some_and(|g| flashed_groups.contains(&g)))
            .collect()
    }

    /// Id the next started cue will get.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn primary(&self) -> Option<&ActiveCue> {
        self.active.last()
    }
}

/// The looks to render this frame, as (animator id, settings). `primary`
/// is `Shared::settings`: the newest cue's live look, or the manual look
/// (scene, playlist, look panel) when no cue plays and `look_on` is set.
/// Instance id 0 is the manual look.
pub fn looks(deck: &CueDeck, primary: &Settings, look_on: bool) -> Vec<(u64, Settings)> {
    let Some(top) = deck.primary() else {
        return if look_on { vec![(0, primary.clone())] } else { Vec::new() };
    };
    deck.visible()
        .into_iter()
        .map(|a| {
            if a.id == top.id {
                (a.id, primary.clone())
            } else {
                // The operator's look brightness applies to every cue.
                (a.id, Settings { brightness: primary.brightness, ..a.settings.clone() })
            }
        })
        .collect()
}

/// `looks` with the layer of each look (the manual look is on layer 1).
pub fn layered_looks(deck: &CueDeck, primary: &Settings, look_on: bool) -> Vec<(u8, u64, Settings)> {
    looks(deck, primary, look_on)
        .into_iter()
        .map(|(id, settings)| (deck.active.iter().find(|a| a.id == id).map_or(1, |a| a.layer), id, settings))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(multi: bool) -> CueDeck {
        CueDeck { multi, ..Default::default() }
    }

    fn press(d: &mut CueDeck, cue: &str) {
        d.press(cue, None, At::default(), Settings::default);
    }

    fn press_as(d: &mut CueDeck, cue: &str, mode: ClickMode) {
        d.press(cue, Some(mode), At::default(), Settings::default);
    }

    fn playing(d: &CueDeck) -> Vec<&str> {
        d.active.iter().map(|a| a.cue.as_str()).collect()
    }

    fn shown(d: &CueDeck) -> Vec<&str> {
        d.visible().iter().map(|a| a.cue.as_str()).collect()
    }

    #[test]
    fn toggle_starts_then_stops() {
        let mut d = deck(false);
        press(&mut d, "a");
        assert_eq!(playing(&d), ["a"]);
        d.release("a");
        assert_eq!(playing(&d), ["a"], "a release does not stop a latched cue");
        press(&mut d, "a");
        assert!(playing(&d).is_empty());
    }

    #[test]
    fn single_mode_replaces_and_multi_adds() {
        let mut d = deck(false);
        press(&mut d, "a");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["b"]);
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["a", "b"]);
    }

    #[test]
    fn flash_plays_only_while_held_and_brings_back_what_played() {
        for multi in [false, true] {
            let mut d = deck(multi);
            press(&mut d, "a");
            let before: Vec<u64> = d.active.iter().map(|a| a.id).collect();
            press_as(&mut d, "f", ClickMode::Flash);
            assert!(shown(&d).contains(&"f"));
            // « Un cue »: the flash takes over; « Multi »: it adds up.
            assert_eq!(shown(&d).contains(&"a"), multi);
            d.release("f");
            assert_eq!(d.active.iter().map(|a| a.id).collect::<Vec<_>>(), before, "multi {multi}");
            assert_eq!(shown(&d), ["a"]);
        }
    }

    #[test]
    fn flash_mode_from_the_deck_and_from_a_slot() {
        let mut d = deck(true);
        d.click_mode = ClickMode::Flash;
        press(&mut d, "a");
        assert_eq!(playing(&d), ["a"]);
        d.release("a");
        assert!(playing(&d).is_empty());
        d.click_mode = ClickMode::Toggle;
        d.set_slot("b", CueSlot { mode: Some(ClickMode::Flash), group: None, layer: None });
        press(&mut d, "b");
        d.release("b");
        assert!(playing(&d).is_empty());
    }

    #[test]
    fn flashing_a_running_cue_changes_nothing() {
        let mut d = deck(true);
        press(&mut d, "a");
        press_as(&mut d, "a", ClickMode::Flash);
        assert_eq!(d.active.len(), 1);
        d.release("a");
        assert_eq!(playing(&d), ["a"]);
    }

    #[test]
    fn solo_hides_the_others_until_released() {
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        press_as(&mut d, "s", ClickMode::Solo);
        assert_eq!(shown(&d), ["s"]);
        assert_eq!(playing(&d), ["a", "b", "s"]);
        d.release("s");
        assert_eq!(shown(&d), ["a", "b"]);
    }

    #[test]
    fn solo_of_a_running_cue_shows_only_it() {
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        press_as(&mut d, "a", ClickMode::Solo);
        assert_eq!(shown(&d), ["a"]);
        d.release("a");
        assert_eq!(shown(&d), ["a", "b"]);
    }

    #[test]
    fn restart_gives_a_new_instance_each_press() {
        let mut d = deck(false);
        press_as(&mut d, "a", ClickMode::Restart);
        let first = d.active[0].id;
        press_as(&mut d, "a", ClickMode::Restart);
        assert_eq!(d.active.len(), 1);
        assert_ne!(d.active[0].id, first);
    }

    #[test]
    fn same_group_replaces_other_groups_stay() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: None, group: Some(1), layer: None });
        d.set_slot("b", CueSlot { mode: None, group: Some(1), layer: None });
        d.set_slot("c", CueSlot { mode: None, group: Some(2), layer: None });
        press(&mut d, "a");
        press(&mut d, "c");
        press(&mut d, "free");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["c", "free", "b"]);
    }

    #[test]
    fn a_flash_in_a_group_hides_its_group_mates_only_while_held() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: None, group: Some(3), layer: None });
        d.set_slot("f", CueSlot { mode: Some(ClickMode::Flash), group: Some(3), layer: None });
        press(&mut d, "a");
        press(&mut d, "other");
        press(&mut d, "f");
        assert_eq!(shown(&d), ["other", "f"]);
        d.release("f");
        assert_eq!(shown(&d), ["a", "other"]);
    }

    #[test]
    fn limiter_stops_the_oldest_latched_cue() {
        let mut d = deck(true);
        for c in ["1", "2", "3", "4", "5"] {
            press(&mut d, c);
        }
        assert_eq!(playing(&d), ["2", "3", "4", "5"]);
        // Flashes don't count and are never the ones stopped.
        press_as(&mut d, "f", ClickMode::Flash);
        assert_eq!(playing(&d), ["2", "3", "4", "5", "f"]);
        press(&mut d, "6");
        assert_eq!(playing(&d), ["3", "4", "5", "f", "6"]);
        d.set_max_active(2);
        assert_eq!(playing(&d), ["5", "f", "6"]);
    }

    #[test]
    fn looks_use_the_live_settings_for_the_newest_cue() {
        let mut d = deck(true);
        d.press("a", None, At::default(), || Settings { scale: 0.1, brightness: 1.0, ..Default::default() });
        d.press("b", None, At::default(), || Settings { scale: 0.2, ..Default::default() });
        let live = Settings { scale: 0.9, brightness: 0.3, ..Default::default() };
        let l = looks(&d, &live, false);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].1.scale, l[0].1.brightness), (0.1, 0.3));
        assert_eq!(l[1].1.scale, 0.9);
        d.stop_all();
        assert!(looks(&d, &live, false).is_empty(), "nothing plays after the last cue stops");
        assert_eq!(looks(&d, &live, true), vec![(0, live.clone())]);
    }

    fn on_layer(d: &mut CueDeck, cue: &str, layer: u8) {
        d.set_slot(cue, CueSlot { layer: Some(layer), ..d.slot(cue) });
    }

    #[test]
    fn single_mode_replaces_only_within_a_layer() {
        let mut d = deck(false);
        on_layer(&mut d, "beam", 2);
        press(&mut d, "a");
        press(&mut d, "beam");
        assert_eq!(playing(&d), ["a", "beam"]);
        press(&mut d, "b");
        assert_eq!(playing(&d), ["beam", "b"], "b replaces a on layer 1, the beam stays");
        let l = layered_looks(&d, &Settings::default(), false);
        assert_eq!(l.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(layered_looks(&deck(false), &Settings::default(), true)[0].0, 1, "the manual look is on layer 1");
    }

    #[test]
    fn clearing_a_layer_stops_its_cues_only() {
        let mut d = deck(true);
        on_layer(&mut d, "x", 3);
        on_layer(&mut d, "f", 3);
        press(&mut d, "a");
        press(&mut d, "x");
        press_as(&mut d, "f", ClickMode::Flash);
        d.clear_layer(3);
        assert_eq!(playing(&d), ["a"]);
        d.release("f");
        assert_eq!(playing(&d), ["a"]);
    }

    #[test]
    fn a_flash_on_another_layer_still_returns_to_what_played() {
        for multi in [false, true] {
            let mut d = deck(multi);
            on_layer(&mut d, "f", 4);
            press(&mut d, "a");
            press_as(&mut d, "f", ClickMode::Flash);
            assert_eq!(shown(&d).contains(&"a"), multi, "« Un cue »: the flash still takes over the output");
            d.release("f");
            assert_eq!(shown(&d), ["a"]);
        }
    }

    #[test]
    fn grid_json_round_trips_without_the_playing_list() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: Some(ClickMode::Solo), group: Some(9), layer: None });
        press(&mut d, "a");
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(back.active.is_empty());
        assert!(back.multi);
        assert_eq!(back.slot("a"), CueSlot { mode: Some(ClickMode::Solo), group: None, layer: None }, "group 9 is out of range");
        on_layer(&mut d, "b", 9);
        on_layer(&mut d, "c", 1);
        assert!(!d.slots.contains_key("b") && !d.slots.contains_key("c"), "layer 9 is out of range, layer 1 is the default");
        on_layer(&mut d, "c", 4);
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.slot("c").layer, Some(4));
        let old: CueDeck = serde_json::from_str("{}").unwrap();
        assert_eq!((old.click_mode, old.multi, old.max_active), (ClickMode::Toggle, false, 4));
    }
}
