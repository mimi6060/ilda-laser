//! Named, saved looks with a playback duration, persisted as JSON. A scene
//! stores a whole `Settings` value, so anything the UI can set - including
//! music reactions - can be saved and replayed.

use crate::engine::Settings;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub settings: Settings,
    pub duration_secs: f32,
}

pub struct SceneStore {
    path: PathBuf,
    scenes: Vec<Scene>,
}

impl SceneStore {
    /// A missing or unreadable file starts an empty store; it's overwritten
    /// on the next save.
    pub fn load_or_create(path: PathBuf) -> Self {
        let scenes = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, scenes }
    }

    pub fn list(&self) -> &[Scene] {
        &self.scenes
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replaces every scene in memory only; the caller saves the file
    /// (project open, T-286: outside the engine lock).
    pub fn replace_in_memory(&mut self, scenes: Vec<Scene>) {
        self.scenes = scenes;
    }

    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    /// Replaces a scene with the same name, otherwise appends.
    pub fn upsert(&mut self, scene: Scene) -> Result<()> {
        match self.scenes.iter_mut().find(|s| s.name == scene.name) {
            Some(existing) => *existing = scene,
            None => self.scenes.push(scene),
        }
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.scenes.retain(|s| s.name != name);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.scenes).context("failed to serialize scenes")?;
        fs::write(&self.path, json).with_context(|| format!("failed to write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("scenes.json")
    }

    fn scene(name: &str, duration_secs: f32) -> Scene {
        Scene { name: name.into(), settings: Settings::default(), duration_secs }
    }

    #[test]
    fn missing_file_starts_empty() {
        assert!(SceneStore::load_or_create(PathBuf::from("/no/such/dir/scenes.json")).list().is_empty());
    }

    #[test]
    fn upsert_round_trips_and_replaces_by_name() {
        let path = temp_path("upsert");
        let mut store = SceneStore::load_or_create(path.clone());
        store.upsert(scene("Intro", 5.0)).unwrap();
        store.upsert(scene("Drop", 8.0)).unwrap();
        store.upsert(scene("Intro", 9.0)).unwrap();

        let reloaded = SceneStore::load_or_create(path.clone());
        assert_eq!(reloaded.list().len(), 2);
        assert_eq!(reloaded.get("Intro").unwrap().duration_secs, 9.0);
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_evolving_cue_saved_in_a_scene_reloads() {
        let path = temp_path("evolving");
        let mut store = SceneStore::load_or_create(path.clone());
        let settings = Settings { content: crate::engine::Content::Evolving(crate::evolving::test_cue(true)), ..Settings::default() };
        store.upsert(Scene { name: "Montée".into(), settings: settings.clone(), duration_secs: 8.0 }).unwrap();
        let reloaded = SceneStore::load_or_create(path.clone());
        assert_eq!(reloaded.get("Montée").unwrap().settings, settings);
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn remove_deletes_by_name() {
        let path = temp_path("remove");
        let mut store = SceneStore::load_or_create(path.clone());
        store.upsert(scene("Intro", 5.0)).unwrap();
        store.remove("Intro").unwrap();
        assert!(store.list().is_empty());
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
