//! First launch (T-437): whether the guided tour was seen.
//!
//! Kept in `<data-dir>/onboarding.json`, outside any project, so a second
//! browser (or a reload) doesn't replay it. A missing file means a fresh
//! data folder: the tour is offered. A damaged one goes to quarantine like
//! every data file (datafile.rs) and the tour is offered again.
//!
//! The tour is a view: it never arms, never connects an output and calls
//! no arming route (the page only highlights the buttons).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bumped only when the tour changes a lot. A seen tour of an older
/// version is not replayed by itself (it stays reachable from « Aide »).
pub const TOUR_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Onboarding {
    pub tour_done: bool,
    pub tour_version: u32,
}

/// `onboarding.json` and its contents, in `Shared`.
#[derive(Debug, Default)]
pub struct OnboardingStore {
    pub state: Onboarding,
    /// `None` in unit tests: memory only.
    path: Option<PathBuf>,
}

impl OnboardingStore {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("onboarding.json");
        let state = crate::datafile::load_or_default(&path, &crate::datafile::beside(&path), false);
        Self { state, path: Some(path) }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// True until the tour was finished or skipped once.
    pub fn first_run(&self) -> bool {
        !self.state.tour_done
    }

    /// Marks the tour seen (or not, to offer it again) and saves.
    pub fn set_tour_done(&mut self, done: bool) -> Result<()> {
        self.state = Onboarding { tour_done: done, tour_version: if done { TOUR_VERSION } else { 0 } };
        match &self.path {
            Some(path) => crate::datafile::replace(path, &serde_json::to_vec_pretty(&self.state)?),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_data_folder_offers_the_tour() {
        let dir = crate::datafile::tests::temp_dir("onboarding-fresh");
        let store = OnboardingStore::load(&dir);
        assert!(store.first_run());
        assert_eq!(store.state, Onboarding::default());
        assert!(!dir.join("onboarding.json").exists(), "loading writes nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_seen_tour_survives_a_restart() {
        let dir = crate::datafile::tests::temp_dir("onboarding-roundtrip");
        let mut store = OnboardingStore::load(&dir);
        store.set_tour_done(true).unwrap();
        let again = OnboardingStore::load(&dir);
        assert!(!again.first_run());
        assert_eq!(again.state, Onboarding { tour_done: true, tour_version: TOUR_VERSION });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_harness_file_without_a_version_still_counts_as_seen() {
        let dir = crate::datafile::tests::temp_dir("onboarding-partial");
        std::fs::write(dir.join("onboarding.json"), br#"{"tour_done":true}"#).unwrap();
        assert!(!OnboardingStore::load(&dir).first_run());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_the_tour_offered() {
        let (dir, _) = crate::datafile::tests::with_damaged("onboarding-damaged", "onboarding.json");
        let mut store = OnboardingStore::load(&dir);
        assert!(store.first_run());
        store.set_tour_done(true).unwrap();
        crate::datafile::tests::check_kept(&dir, "onboarding.json", false);
    }

    #[test]
    fn memory_only_store_never_writes() {
        let mut store = OnboardingStore::in_memory();
        store.set_tour_done(true).unwrap();
        assert!(!store.first_run());
        store.set_tour_done(false).unwrap();
        assert!(store.first_run());
    }
}
