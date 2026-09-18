//! Named, saved "looks" (shape or text + color + size + speed), each with
//! a playback duration - the same concept as the original laser's own
//! "Scene management" (see the product manual), reimplemented here so the
//! Pi doesn't depend on that app. A `SceneStore` persists them as JSON next
//! to the binary (or wherever `--scenes-file` points).

use crate::font;
use crate::patterns::{self, Point};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SceneContent {
    Shape { shape: String },
    Text { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub content: SceneContent,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub scale: f32,
    pub pps: u32,
    pub duration_secs: f32,
}

impl Scene {
    /// Render this scene's content into laser points, ready to hand to the
    /// player.
    pub fn resolve(&self) -> Vec<Point> {
        let r = self.r as f32 / 255.0;
        let g = self.g as f32 / 255.0;
        let b = self.b as f32 / 255.0;
        match &self.content {
            SceneContent::Shape { shape } => {
                patterns::by_name(shape, self.scale, r, g, b).unwrap_or_default()
            }
            SceneContent::Text { text } => font::text_to_points(text, self.scale, r, g, b),
        }
    }
}

pub struct SceneStore {
    path: PathBuf,
    scenes: Vec<Scene>,
}

impl SceneStore {
    /// Loads scenes from `path` if it exists and parses cleanly; otherwise
    /// starts empty (a missing or corrupt file is not fatal - it's treated
    /// as "no scenes saved yet" and will be overwritten on the next save).
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

    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    /// Replaces any existing scene with the same name, otherwise appends.
    pub fn upsert(&mut self, scene: Scene) -> Result<()> {
        if let Some(existing) = self.scenes.iter_mut().find(|s| s.name == scene.name) {
            *existing = scene;
        } else {
            self.scenes.push(scene);
        }
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.scenes.retain(|s| s.name != name);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.scenes).context("failed to serialize scenes")?;
        fs::write(&self.path, json)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scene(name: &str) -> Scene {
        Scene {
            name: name.to_string(),
            content: SceneContent::Shape { shape: "circle".to_string() },
            r: 255,
            g: 0,
            b: 0,
            scale: 0.8,
            pps: 4000,
            duration_secs: 5.0,
        }
    }

    #[test]
    fn missing_file_starts_with_no_scenes() {
        let store = SceneStore::load_or_create(PathBuf::from("/no/such/dir/scenes.json"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn upsert_then_load_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("pi-laser-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scenes.json");

        let mut store = SceneStore::load_or_create(path.clone());
        store.upsert(sample_scene("Intro")).unwrap();
        store.upsert(sample_scene("Drop")).unwrap();

        let reloaded = SceneStore::load_or_create(path);
        assert_eq!(reloaded.list().len(), 2);
        assert!(reloaded.get("Intro").is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upsert_with_same_name_replaces_not_duplicates() {
        let dir = std::env::temp_dir().join(format!("pi-laser-test-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scenes.json");

        let mut store = SceneStore::load_or_create(path);
        store.upsert(sample_scene("Intro")).unwrap();
        let mut updated = sample_scene("Intro");
        updated.duration_secs = 9.0;
        store.upsert(updated).unwrap();

        assert_eq!(store.list().len(), 1);
        assert_eq!(store.get("Intro").unwrap().duration_secs, 9.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_deletes_by_name() {
        let dir = std::env::temp_dir().join(format!("pi-laser-test-remove-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scenes.json");

        let mut store = SceneStore::load_or_create(path);
        store.upsert(sample_scene("Intro")).unwrap();
        store.remove("Intro").unwrap();
        assert!(store.list().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_shape_scene_produces_points() {
        let scene = sample_scene("Intro");
        assert!(!scene.resolve().is_empty());
    }

    #[test]
    fn resolve_text_scene_produces_points() {
        let mut scene = sample_scene("Title");
        scene.content = SceneContent::Text { text: "HI".to_string() };
        assert!(!scene.resolve().is_empty());
    }
}
