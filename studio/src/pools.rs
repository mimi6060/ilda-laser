//! Referenced presets (T-422): position, size, colour and motion presets
//! that cues and timeline events *refer to* instead of carrying the
//! values themselves. Change the preset, and every cue that uses it
//! follows from the next frame on, the ones playing included: a show is
//! adapted to a room (a lower ceiling, a screen on the left, the client's
//! colours) by editing a handful of presets, not every cue.
//!
//! The word « preset » is already taken by `presets.rs` (the built-in
//! cues) and `/api/presets`: these are *pools*, `/api/pools`.
//!
//! Each family carries only its own fields of `LiveModifiers`:
//! - **Position**: `pos_x`, `pos_y`, `rot_angle`;
//! - **Size**: `size`, `size_x`, `size_y`;
//! - **Colour**: `color` and `color_params` (a recolouring mode, which may
//!   name a palette by its index; not a palette itself);
//! - **Motion**: `rot_speed`, `rot_sync`, `speed`.
//!
//! A preset holds no brightness, no power cap and no safety setting: it
//! can't loosen anything. Its values are clamped to the master controls'
//! ranges (`controls.rs`) when stored and again when loaded.
//!
//! Order of application, for one cue:
//! cue's own values → references → layer → master → zones → calibration
//! → safety.
//!
//! - A grid cue (`CueSlot::refs`): its cue level (`levels.rs`) is resolved
//!   every frame, the references replacing their families in the working
//!   copy (so a referenced family can't be moved at the cue level while
//!   the reference is set: the preset is what it plays). A user cue's
//!   stored master (`UserCue::master`, T-405) leaves the referenced
//!   families neutral (`strip`), so the preset alone sets them.
//! - A timeline event (`Event::refs`): its own modifiers, resolved every
//!   frame before its envelopes.
//!
//! A reference to an id the store doesn't have (a damaged project, a
//! preset of another project) is ignored: the cue plays its own values,
//! never something brighter or unbounded, and the project open warns.

use crate::cues::CueDeck;
use crate::live::{ColorOverride, ColorParams, LiveModifiers, Rate};
use crate::timeline::{Show, TimeBase};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_PER_FAMILY: usize = 64;
pub const MAX_NAME: usize = 32;
/// Longest id after the family prefix.
const MAX_ID_TAIL: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolFamily {
    Position,
    Size,
    Color,
    Motion,
}

pub const FAMILIES: [PoolFamily; 4] = [PoolFamily::Position, PoolFamily::Size, PoolFamily::Color, PoolFamily::Motion];

impl PoolFamily {
    pub fn key(self) -> &'static str {
        match self {
            PoolFamily::Position => "position",
            PoolFamily::Size => "size",
            PoolFamily::Color => "color",
            PoolFamily::Motion => "motion",
        }
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            PoolFamily::Position => "Position",
            PoolFamily::Size => "Taille",
            PoolFamily::Color => "Couleur",
            PoolFamily::Motion => "Mouvement",
        }
    }

}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum PoolValues {
    Position {
        #[serde(default)]
        pos_x: f32,
        #[serde(default)]
        pos_y: f32,
        #[serde(default)]
        rot_angle: [f32; 3],
    },
    Size {
        #[serde(default = "one")]
        size: f32,
        #[serde(default = "one")]
        size_x: f32,
        #[serde(default = "one")]
        size_y: f32,
    },
    Color {
        #[serde(default)]
        color: ColorOverride,
        #[serde(default)]
        color_params: ColorParams,
    },
    Motion {
        #[serde(default)]
        rot_speed: [f32; 3],
        #[serde(default)]
        rot_sync: bool,
        #[serde(default = "one")]
        speed: f32,
    },
}

/// A non-finite value becomes `default`, a finite one is clamped.
fn fix(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

/// A colour speed within the controls' ranges: free 0..10 Hz, synced
/// 1/16..16 beats a step.
fn fix_rate(r: Rate, default: Rate) -> Rate {
    match r {
        Rate::Hz(hz) if hz.is_finite() => Rate::Hz(hz.clamp(0.0, 10.0)),
        Rate::Beats(b) if b.is_finite() && b > 0.0 => Rate::Beats(b.clamp(1.0 / 16.0, 16.0)),
        _ => default,
    }
}

const MAX_PALETTE: usize = crate::live::PALETTE_LABELS.len() - 1;
const MAX_OFFSET: usize = 15;

fn sanitize_color(c: &mut ColorOverride, p: &mut ColorParams) {
    let d = ColorParams::default();
    p.hue = fix(p.hue, 0.0, 360.0, d.hue);
    p.palette = p.palette.min(MAX_PALETTE);
    p.offset = p.offset.min(MAX_OFFSET);
    p.spread = fix(p.spread, 0.0, 4.0, d.spread);
    p.rainbow_rate = fix_rate(p.rainbow_rate, d.rainbow_rate);
    p.chase_step = fix_rate(p.chase_step, d.chase_step);
    match c {
        ColorOverride::Normal | ColorOverride::Fixed { .. } => {}
        ColorOverride::Hue { hue } => *hue = fix(*hue, 0.0, 360.0, 0.0),
        ColorOverride::Palette { palette, offset, .. } => {
            *palette = (*palette).min(MAX_PALETTE);
            *offset = (*offset).min(MAX_OFFSET);
        }
        ColorOverride::Rainbow { spread, rate } => {
            *spread = fix(*spread, 0.0, 4.0, d.spread);
            *rate = fix_rate(*rate, d.rainbow_rate);
        }
        ColorOverride::Chase { palette, step, .. } => {
            *palette = (*palette).min(MAX_PALETTE);
            *step = fix_rate(*step, d.chase_step);
        }
    }
}

impl PoolValues {
    pub fn family(&self) -> PoolFamily {
        match self {
            PoolValues::Position { .. } => PoolFamily::Position,
            PoolValues::Size { .. } => PoolFamily::Size,
            PoolValues::Color { .. } => PoolFamily::Color,
            PoolValues::Motion { .. } => PoolFamily::Motion,
        }
    }

    /// The family's fields of `m` (« Enregistrer depuis le direct »: the
    /// record filter copies one family only), within range.
    pub fn from_live(family: PoolFamily, m: &LiveModifiers) -> Self {
        let mut v = match family {
            PoolFamily::Position => PoolValues::Position { pos_x: m.pos_x, pos_y: m.pos_y, rot_angle: m.rot_angle },
            PoolFamily::Size => PoolValues::Size { size: m.size, size_x: m.size_x, size_y: m.size_y },
            PoolFamily::Color => PoolValues::Color { color: m.color.clone(), color_params: m.color_params.clone() },
            PoolFamily::Motion => PoolValues::Motion { rot_speed: m.rot_speed, rot_sync: m.rot_sync, speed: m.speed },
        };
        v.sanitize();
        v
    }

    /// The neutral values of a family (what « Valeurs du cue » means for
    /// a cue without its own).
    pub fn neutral(family: PoolFamily) -> Self {
        Self::from_live(family, &LiveModifiers::default())
    }

    /// Into the master controls' ranges (`controls.rs`): positions -1..1,
    /// angles ±180°, sizes 0..2 and ±2, rotation speeds ±720, animation
    /// speed 0..4, colour settings as the colour controls allow.
    pub fn sanitize(&mut self) {
        match self {
            PoolValues::Position { pos_x, pos_y, rot_angle } => {
                *pos_x = fix(*pos_x, -1.0, 1.0, 0.0);
                *pos_y = fix(*pos_y, -1.0, 1.0, 0.0);
                for a in rot_angle.iter_mut() {
                    *a = fix(*a, -180.0, 180.0, 0.0);
                }
            }
            PoolValues::Size { size, size_x, size_y } => {
                *size = fix(*size, 0.0, 2.0, 1.0);
                *size_x = fix(*size_x, -2.0, 2.0, 1.0);
                *size_y = fix(*size_y, -2.0, 2.0, 1.0);
            }
            PoolValues::Color { color, color_params } => sanitize_color(color, color_params),
            PoolValues::Motion { rot_speed, speed, .. } => {
                for s in rot_speed.iter_mut() {
                    *s = fix(*s, -720.0, 720.0, 0.0);
                }
                *speed = fix(*speed, 0.0, 4.0, 1.0);
            }
        }
    }

    /// Replaces the family's fields of `m`; the other fields stay.
    pub fn apply_to(&self, m: &mut LiveModifiers) {
        let mut v = self.clone();
        v.sanitize();
        match v {
            PoolValues::Position { pos_x, pos_y, rot_angle } => {
                (m.pos_x, m.pos_y, m.rot_angle) = (pos_x, pos_y, rot_angle);
            }
            PoolValues::Size { size, size_x, size_y } => {
                (m.size, m.size_x, m.size_y) = (size, size_x, size_y);
            }
            PoolValues::Color { color, color_params } => {
                (m.color, m.color_params) = (color, color_params);
            }
            PoolValues::Motion { rot_speed, rot_sync, speed } => {
                (m.rot_speed, m.rot_sync, m.speed) = (rot_speed, rot_sync, speed);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PoolPreset {
    /// `<family>:<slug>` (`position:plafond`): stays the same when the
    /// preset is renamed, so no reference breaks.
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub values: PoolValues,
}

impl PoolPreset {
    pub fn family(&self) -> PoolFamily {
        self.values.family()
    }
}

/// A cue's or an event's references, one per family; `None` = its own
/// values. Absent in older files: none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresetRefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<String>,
}

impl PresetRefs {
    pub fn is_empty(&self) -> bool {
        FAMILIES.iter().all(|f| self.get(*f).is_none())
    }

    /// The id referenced for `family` (an empty string counts as none).
    pub fn get(&self, family: PoolFamily) -> Option<&str> {
        let r = match family {
            PoolFamily::Position => &self.position,
            PoolFamily::Size => &self.size,
            PoolFamily::Color => &self.color,
            PoolFamily::Motion => &self.motion,
        };
        r.as_deref().filter(|id| !id.is_empty())
    }

    pub fn slot_mut(&mut self, family: PoolFamily) -> &mut Option<String> {
        match family {
            PoolFamily::Position => &mut self.position,
            PoolFamily::Size => &mut self.size,
            PoolFamily::Color => &mut self.color,
            PoolFamily::Motion => &mut self.motion,
        }
    }

    /// Whether any family refers to `id`.
    pub fn uses(&self, id: &str) -> bool {
        FAMILIES.iter().any(|f| self.get(*f) == Some(id))
    }

    /// Empty strings become `None`.
    pub fn normalize(&mut self) {
        for f in FAMILIES {
            let slot = self.slot_mut(f);
            if slot.as_deref().is_some_and(|id| id.trim().is_empty()) {
                *slot = None;
            }
        }
    }
}

/// The presets, saved to `<data-dir>/pools.json` on every change (the
/// same scheme as the user palettes).
pub struct PoolStore {
    path: Option<PathBuf>,
    presets: Vec<PoolPreset>,
}

impl PoolStore {
    /// A missing file starts an empty store; a damaged or invalid one too,
    /// once moved to the quarantine folder (T-402).
    pub fn load_or_create(path: PathBuf) -> Self {
        let mut presets: Vec<PoolPreset> = crate::load_json(&path);
        let presets = match validate(&mut presets) {
            Ok(()) => presets,
            Err(e) => {
                crate::datafile::set_aside(&path, &crate::datafile::beside(&path), &format!("préréglages invalides : {e}"), false);
                Vec::new()
            }
        };
        Self { path: Some(path), presets }
    }

    /// Never written (tests).
    pub fn in_memory() -> Self {
        Self { path: None, presets: Vec::new() }
    }

    pub fn list(&self) -> &[PoolPreset] {
        &self.presets
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self, id: &str) -> Option<&PoolPreset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The preset `id` if it exists and belongs to `family`.
    pub fn get_in(&self, family: PoolFamily, id: &str) -> Option<&PoolPreset> {
        self.get(id).filter(|p| p.family() == family)
    }

    /// Replaces every preset in memory only (already validated); the
    /// caller saves the file (project open, T-286).
    pub fn replace_in_memory(&mut self, presets: Vec<PoolPreset>) {
        self.presets = presets;
    }

    /// Replaces every preset (validated, values clamped) and saves.
    pub fn set(&mut self, mut presets: Vec<PoolPreset>) -> Result<()> {
        validate(&mut presets)?;
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&presets).context("failed to serialize presets")?;
            crate::datafile::replace(path, json.as_bytes()).with_context(|| format!("failed to write {}", path.display()))?;
        }
        self.presets = presets;
        Ok(())
    }

    /// A free id for a new preset of `family` named `name`.
    pub fn new_id(&self, family: PoolFamily, name: &str) -> String {
        let mut slug: String = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| match c {
                'à' | 'â' | 'ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'î' | 'ï' => 'i',
                'ô' | 'ö' => 'o',
                'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                c if c.is_ascii_alphanumeric() => c,
                _ => '-',
            })
            .collect();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let mut slug: String = slug.trim_matches('-').chars().take(MAX_ID_TAIL - 4).collect();
        if slug.is_empty() {
            slug = "p".into();
        }
        let base = format!("{}:{slug}", family.key());
        (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| self.get(id).is_none()).expect("some id is free")
    }
}

fn valid_id(id: &str, family: PoolFamily) -> bool {
    let Some(tail) = id.strip_prefix(family.key()).and_then(|t| t.strip_prefix(':')) else { return false };
    !tail.is_empty() && tail.len() <= MAX_ID_TAIL && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Ids well formed (`<family>:<slug>`, of the preset's own family) and
/// unique; names of 1..=32 characters, unique in their family (trimmed,
/// case ignored); at most 64 per family. Values are clamped, not refused.
pub fn validate(presets: &mut [PoolPreset]) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for f in FAMILIES {
        if presets.iter().filter(|p| p.family() == f).count() > MAX_PER_FAMILY {
            bail!("{MAX_PER_FAMILY} préréglages « {} » au maximum", f.label_fr());
        }
    }
    for p in presets.iter_mut() {
        p.name = p.name.trim().to_string();
        if p.name.is_empty() || p.name.chars().count() > MAX_NAME {
            bail!("nom de préréglage invalide : « {} » (1 à {MAX_NAME} caractères)", p.name);
        }
        if !valid_id(&p.id, p.family()) {
            bail!("identifiant de préréglage invalide : « {} »", p.id);
        }
        if !ids.insert(p.id.clone()) {
            bail!("préréglage en double : « {} »", p.id);
        }
        if !names.insert((p.family(), p.name.to_lowercase())) {
            bail!("nom déjà pris dans « {} » : « {} »", p.family().label_fr(), p.name);
        }
        p.values.sanitize();
    }
    Ok(())
}

/// `base` with the families `refs` points to replaced by their preset's
/// values. An unknown id, or one of another family, leaves that family
/// as it is in `base` (the cue's own values).
pub fn resolve(refs: &PresetRefs, store: &PoolStore, base: &LiveModifiers) -> LiveModifiers {
    let mut m = base.clone();
    for f in FAMILIES {
        if let Some(p) = refs.get(f).and_then(|id| store.get_in(f, id)) {
            p.values.apply_to(&mut m);
        }
    }
    m
}

/// `m` with the families `refs` resolves set neutral: a user cue's stored
/// master leaves them to the preset (applied at the cue level). Unknown
/// references keep the stored values.
pub fn strip(refs: &PresetRefs, store: &PoolStore, m: &LiveModifiers) -> LiveModifiers {
    let mut out = m.clone();
    for f in FAMILIES {
        if refs.get(f).is_some_and(|id| store.get_in(f, id).is_some()) {
            PoolValues::neutral(f).apply_to(&mut out);
        }
    }
    out
}

/// Where preset `id` is used, in French: the grid's cells (`cue_label`
/// names a cue id: its name and page) and the shows' events (show, track,
/// position). Sorted, cells first.
pub fn users(id: &str, deck: &CueDeck, shows: &[Show], cue_label: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut out: Vec<String> = deck.slots.iter().filter(|(_, slot)| slot.refs.uses(id)).map(|(cue, _)| cue_label(cue)).collect();
    out.sort();
    for show in shows {
        for track in &show.tracks {
            for e in track.events.iter().filter(|e| e.refs.uses(id)) {
                let at = match show.time_base {
                    TimeBase::Seconds => format!("{:.1} s", e.start),
                    TimeBase::Beats => format!("temps {:.1}", e.start),
                };
                out.push(format!("show « {} », piste « {} », {at}", show.name, track.name));
            }
        }
    }
    out
}

/// The references to ids `known` doesn't hold, as warnings for the
/// project open: they are ignored when rendering (the cue's own values).
pub fn dangling(deck: &CueDeck, shows: &[Show], known: &[PoolPreset], cue_label: &dyn Fn(&str) -> String) -> Vec<String> {
    let ok = |f: PoolFamily, id: &str| known.iter().any(|p| p.id == id && p.family() == f);
    let mut out = Vec::new();
    for (cue, slot) in &deck.slots {
        for f in FAMILIES {
            if let Some(id) = slot.refs.get(f).filter(|id| !ok(f, id)) {
                out.push(format!("préréglage inconnu « {id} » ({}) ignoré pour {} : valeurs du cue", f.label_fr(), cue_label(cue)));
            }
        }
    }
    for show in shows {
        for track in &show.tracks {
            for e in &track.events {
                for f in FAMILIES {
                    if let Some(id) = e.refs.get(f).filter(|id| !ok(f, id)) {
                        out.push(format!("préréglage inconnu « {id} » ({}) ignoré dans le show « {} », piste « {} » : valeurs de l'événement", f.label_fr(), show.name, track.name));
                    }
                }
            }
        }
    }
    out
}

/// The shows `users` reads in the studio: those on disk
/// (`<data-dir>/shows`), the one in the editor in its current state.
pub fn studio_shows(s: &crate::Shared) -> Vec<Show> {
    let mut shows = s.shows.load_all();
    if let Some(open) = &s.timeline.show {
        shows.retain(|sh| sh.name != open.name);
        shows.push(open.clone());
    }
    shows
}

/// « cue « Nom » (page 2) », or « hors grille » when no page shows it.
pub fn cue_label(s: &crate::Shared, cue: &str) -> String {
    let name = s.presets.iter().find(|p| p.id == cue).map(|p| p.name.clone()).unwrap_or_else(|| cue.to_string());
    match s.grid.view().iter().position(|p| p.cells.iter().any(|c| c.as_deref() == Some(cue))) {
        Some(page) => format!("cue « {name} » (page {})", page + 1),
        None => format!("cue « {name} » (hors grille)"),
    }
}

// ---------- the studio's routes (web.rs) ----------

/// A refusal: HTTP status and JSON body (`error`, and `users` for a
/// preset in use).
pub type Refusal = (u16, serde_json::Value);

fn refuse(status: u16, error: impl std::fmt::Display) -> Refusal {
    (status, serde_json::json!({ "error": error.to_string() }))
}

/// `GET /api/pools`: the presets and where each is used.
pub fn view(s: &crate::Shared) -> serde_json::Value {
    let shows = studio_shows(s);
    let label = |cue: &str| cue_label(s, cue);
    let uses: serde_json::Map<String, serde_json::Value> =
        s.pools.list().iter().map(|p| (p.id.clone(), serde_json::json!(users(&p.id, &s.deck, &shows, &label)))).collect();
    serde_json::json!({
        "presets": s.pools.list(),
        "uses": uses,
        "families": FAMILIES.iter().map(|f| serde_json::json!({ "key": f.key(), "label": f.label_fr() })).collect::<Vec<_>>(),
        "max_per_family": MAX_PER_FAMILY,
        "max_name": MAX_NAME,
    })
}

/// 409 `{ error, users }` if a preset in use is about to go.
fn refuse_removed_in_use(s: &crate::Shared, gone: &[&PoolPreset]) -> Result<(), Refusal> {
    if gone.is_empty() {
        return Ok(());
    }
    let shows = studio_shows(s);
    let label = |cue: &str| cue_label(s, cue);
    for p in gone {
        let u = users(&p.id, &s.deck, &shows, &label);
        if !u.is_empty() {
            return Err((409, serde_json::json!({ "error": format!("Préréglage « {} » utilisé par : {}", p.name, u.join(" ; ")), "users": u })));
        }
    }
    Ok(())
}

/// `POST /api/pools`: the whole list (create, rename, edit, reorder).
/// Ids are kept: a rename never breaks a reference. Removing a preset in
/// use is refused like `delete`.
pub fn set_all(s: &mut crate::Shared, list: Vec<PoolPreset>) -> Result<(), Refusal> {
    let gone: Vec<&PoolPreset> = s.pools.list().iter().filter(|p| !list.iter().any(|q| q.id == p.id)).collect();
    refuse_removed_in_use(s, &gone)?;
    s.pools.set(list).map_err(|e| refuse(400, format!("{e:#}")))
}

/// `POST /api/pools/delete { id }`: 409 with the users while any cue or
/// event refers to it.
pub fn delete(s: &mut crate::Shared, id: &str) -> Result<(), Refusal> {
    let Some(p) = s.pools.get(id) else { return Err(refuse(404, "préréglage inconnu")) };
    refuse_removed_in_use(s, &[p])?;
    let list: Vec<PoolPreset> = s.pools.list().iter().filter(|p| p.id != id).cloned().collect();
    s.pools.set(list).map_err(|e| refuse(500, format!("{e:#}")))
}

#[derive(Debug, Deserialize)]
pub struct CaptureRequest {
    pub family: PoolFamily,
    /// An existing preset to update; absent = a new one.
    #[serde(default)]
    pub id: Option<String>,
    /// The new preset's name, or the updated one's new name (empty: kept).
    #[serde(default)]
    pub name: String,
}

/// `POST /api/pools/capture`: « Enregistrer depuis le direct ». The
/// master's current values of one family only, into a new preset or an
/// existing one of that family.
pub fn capture(s: &mut crate::Shared, req: &CaptureRequest) -> Result<PoolPreset, Refusal> {
    let values = PoolValues::from_live(req.family, &s.live);
    let mut list = s.pools.list().to_vec();
    let name = req.name.trim();
    let preset = match req.id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => {
            let Some(p) = list.iter_mut().find(|p| p.id == id) else { return Err(refuse(404, "préréglage inconnu")) };
            if p.family() != req.family {
                return Err(refuse(400, "ce préréglage est d'une autre famille"));
            }
            p.values = values;
            if !name.is_empty() {
                p.name = name.to_string();
            }
            p.clone()
        }
        None => {
            if name.is_empty() {
                return Err(refuse(400, "donnez un nom au préréglage"));
            }
            let p = PoolPreset { id: s.pools.new_id(req.family, name), name: name.to_string(), values };
            list.push(p.clone());
            p
        }
    };
    s.pools.set(list).map_err(|e| refuse(400, format!("{e:#}")))?;
    Ok(s.pools.get(&preset.id).cloned().unwrap_or(preset))
}

/// The references a cell may take (`POST /api/cues/slot`): each one an id
/// of its family the store knows, or the one the cell already had (a
/// dangling reference survives an edit of the other properties).
pub fn check_refs(store: &PoolStore, before: &PresetRefs, refs: &mut PresetRefs) -> Result<(), String> {
    refs.normalize();
    for f in FAMILIES {
        if let Some(id) = refs.get(f) {
            if store.get_in(f, id).is_none() && before.get(f) != Some(id) {
                return Err(format!("préréglage « {} » inconnu : « {id} »", f.label_fr()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cues::CueSlot;
    use crate::timeline::{Event, Track};

    fn store(presets: Vec<PoolPreset>) -> PoolStore {
        let mut s = PoolStore::in_memory();
        s.set(presets).unwrap();
        s
    }

    fn pos(id: &str, name: &str, x: f32, y: f32) -> PoolPreset {
        PoolPreset { id: id.into(), name: name.into(), values: PoolValues::Position { pos_x: x, pos_y: y, rot_angle: [0.0; 3] } }
    }

    fn size(id: &str, s: f32) -> PoolPreset {
        PoolPreset { id: id.into(), name: id.into(), values: PoolValues::Size { size: s, size_x: 1.0, size_y: 1.0 } }
    }

    #[test]
    fn a_reference_replaces_only_its_own_family() {
        let st = store(vec![pos("position:plafond", "Plafond", 0.2, 0.6), size("size:petit", 0.5)]);
        let base = LiveModifiers { pos_x: -0.3, size: 1.5, color: ColorOverride::Hue { hue: 120.0 }, rot_speed: [0.0, 0.0, 90.0], ..Default::default() };
        let refs = PresetRefs { position: Some("position:plafond".into()), ..Default::default() };
        let m = resolve(&refs, &st, &base);
        assert_eq!((m.pos_x, m.pos_y), (0.2, 0.6));
        assert_eq!(m.size, 1.5, "size untouched");
        assert_eq!(m.color, base.color, "colour untouched");
        assert_eq!(m.rot_speed, base.rot_speed, "motion untouched");
        let refs = PresetRefs { size: Some("size:petit".into()), ..Default::default() };
        let m = resolve(&refs, &st, &base);
        assert_eq!(m.size, 0.5);
        assert_eq!((m.pos_x, m.pos_y), (-0.3, 0.0), "a size preset doesn't touch the position");
        assert_eq!(m.color, base.color);
    }

    #[test]
    fn every_family_resolves() {
        let color = PoolPreset { id: "color:client".into(), name: "Client".into(), values: PoolValues::Color { color: ColorOverride::Fixed { rgb: [0, 0, 255] }, color_params: ColorParams::default() } };
        let motion = PoolPreset { id: "motion:lent".into(), name: "Lent".into(), values: PoolValues::Motion { rot_speed: [0.0, 0.0, 30.0], rot_sync: false, speed: 0.5 } };
        let st = store(vec![color, motion]);
        let refs = PresetRefs { color: Some("color:client".into()), motion: Some("motion:lent".into()), ..Default::default() };
        let m = resolve(&refs, &st, &LiveModifiers::default());
        assert_eq!(m.color, ColorOverride::Fixed { rgb: [0, 0, 255] });
        assert_eq!((m.rot_speed[2], m.speed), (30.0, 0.5));
        assert_eq!(m.brightness, 1.0, "a preset holds no brightness");
    }

    #[test]
    fn an_unknown_or_misfiled_reference_keeps_the_cue_s_values() {
        let st = store(vec![size("size:petit", 0.5)]);
        let base = LiveModifiers { pos_x: 0.4, size: 0.8, ..Default::default() };
        // Unknown id, and a size preset named in the position slot.
        let refs = PresetRefs { position: Some("position:gone".into()), size: Some("position:x".into()), color: Some("size:petit".into()), ..Default::default() };
        assert_eq!(resolve(&refs, &st, &base), base);
        assert_eq!(strip(&refs, &st, &base), base);
    }

    #[test]
    fn strip_leaves_referenced_families_neutral() {
        let st = store(vec![pos("position:a", "A", 0.5, 0.5)]);
        let m = LiveModifiers { pos_x: 0.3, rot_angle: [0.0, 0.0, 45.0], size: 0.5, ..Default::default() };
        let refs = PresetRefs { position: Some("position:a".into()), ..Default::default() };
        let out = strip(&refs, &st, &m);
        assert_eq!((out.pos_x, out.rot_angle), (0.0, [0.0; 3]));
        assert_eq!(out.size, 0.5);
    }

    #[test]
    fn values_are_clamped_on_store_and_on_apply() {
        let wild = PoolPreset { id: "position:loin".into(), name: "Loin".into(), values: PoolValues::Position { pos_x: 9.0, pos_y: f32::NAN, rot_angle: [1e9, -1e9, f32::INFINITY] } };
        let st = store(vec![wild.clone(), PoolPreset { id: "motion:x".into(), name: "X".into(), values: PoolValues::Motion { rot_speed: [1e6; 3], rot_sync: false, speed: 99.0 } }]);
        assert_eq!(st.get("position:loin").unwrap().values, PoolValues::Position { pos_x: 1.0, pos_y: 0.0, rot_angle: [180.0, -180.0, 0.0] });
        assert_eq!(st.get("motion:x").unwrap().values, PoolValues::Motion { rot_speed: [720.0; 3], rot_sync: false, speed: 4.0 });
        // Even values that skipped validation are clamped when applied.
        let mut m = LiveModifiers::default();
        wild.values.apply_to(&mut m);
        assert!(m.pos_x <= 1.0 && m.pos_y.is_finite() && m.rot_angle.iter().all(|a| a.abs() <= 180.0));
        let mut c = PoolValues::Color { color: ColorOverride::Palette { palette: 99, mode: Default::default(), offset: 99 }, color_params: ColorParams { hue: -5.0, spread: 50.0, ..Default::default() } };
        c.sanitize();
        let PoolValues::Color { color: ColorOverride::Palette { palette, offset, .. }, color_params } = c else { panic!() };
        assert!(palette <= MAX_PALETTE && offset <= MAX_OFFSET && color_params.hue == 0.0 && color_params.spread == 4.0);
    }

    #[test]
    fn validation_refuses_bad_names_ids_duplicates_and_too_many() {
        let mut s = PoolStore::in_memory();
        assert!(s.set(vec![pos("position:a", "", 0.0, 0.0)]).is_err(), "empty name");
        assert!(s.set(vec![pos("position:a", &"x".repeat(33), 0.0, 0.0)]).is_err(), "name too long");
        assert!(s.set(vec![pos("position:a", &"é".repeat(32), 0.0, 0.0)]).is_ok(), "32 characters, not bytes");
        assert!(s.set(vec![pos("size:a", "A", 0.0, 0.0)]).is_err(), "id of another family");
        assert!(s.set(vec![pos("position:", "A", 0.0, 0.0)]).is_err(), "empty id");
        assert!(s.set(vec![pos("position:a b", "A", 0.0, 0.0)]).is_err(), "bad id characters");
        assert!(s.set(vec![pos("position:a", "A", 0.0, 0.0), pos("position:a", "B", 0.0, 0.0)]).is_err(), "duplicate id");
        assert!(s.set(vec![pos("position:a", "Plafond", 0.0, 0.0), pos("position:b", " plafond ", 0.0, 0.0)]).is_err(), "duplicate name");
        // The same name in two families is fine.
        assert!(s.set(vec![pos("position:a", "Grand", 0.0, 0.0), PoolPreset { name: "Grand".into(), ..size("size:a", 2.0) }]).is_ok());
        let many: Vec<PoolPreset> = (0..=MAX_PER_FAMILY).map(|i| pos(&format!("position:p{i}"), &format!("P{i}"), 0.0, 0.0)).collect();
        assert!(s.set(many[..MAX_PER_FAMILY].to_vec()).is_ok());
        assert!(s.set(many).is_err(), "65 in a family");
        assert_eq!(s.list().len(), MAX_PER_FAMILY, "a refused set changes nothing");
    }

    #[test]
    fn new_ids_are_slugs_free_and_of_their_family() {
        let s = store(vec![pos("position:plafond", "Plafond", 0.0, 0.0)]);
        assert_eq!(s.new_id(PoolFamily::Position, "Plafond"), "position:plafond-2");
        assert_eq!(s.new_id(PoolFamily::Size, "Écran gauche !"), "size:ecran-gauche");
        assert_eq!(s.new_id(PoolFamily::Color, "  ★  "), "color:p");
        assert!(valid_id(&s.new_id(PoolFamily::Motion, &"long ".repeat(40)), PoolFamily::Motion));
    }

    #[test]
    fn serde_round_trip_and_old_files() {
        let p = pos("position:plafond", "Plafond", 0.0, 0.7);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["family"], "position");
        assert_eq!(json["pos_y"].as_f64().unwrap() as f32, 0.7);
        assert_eq!(serde_json::from_value::<PoolPreset>(json).unwrap(), p);
        // Integers and missing fields read fine.
        let q: PoolPreset = serde_json::from_str(r#"{"id":"size:a","name":"A","family":"size","size":2}"#).unwrap();
        assert_eq!(q.values, PoolValues::Size { size: 2.0, size_x: 1.0, size_y: 1.0 });
        // A slot and an event without refs: none, and none written.
        let slot: CueSlot = serde_json::from_str(r#"{"mode":"flash"}"#).unwrap();
        assert!(slot.refs.is_empty());
        assert!(!serde_json::to_string(&slot).unwrap().contains("refs"));
        let e: Event = serde_json::from_str(r#"{"id":3,"start":1,"len":2}"#).unwrap();
        assert!(e.refs.is_empty());
        assert!(!serde_json::to_string(&e).unwrap().contains("refs"));
        let slot = CueSlot { refs: PresetRefs { size: Some("size:a".into()), ..Default::default() }, ..Default::default() };
        let back: CueSlot = serde_json::from_str(&serde_json::to_string(&slot).unwrap()).unwrap();
        assert_eq!(back.refs, slot.refs);
    }

    #[test]
    fn users_lists_grid_cells_and_show_events() {
        let mut deck = CueDeck::default();
        let r = PresetRefs { position: Some("position:plafond".into()), ..Default::default() };
        deck.slots.insert("b".into(), CueSlot { refs: r.clone(), ..Default::default() });
        deck.slots.insert("a".into(), CueSlot { refs: r.clone(), ..Default::default() });
        deck.slots.insert("c".into(), CueSlot::default());
        let show = Show {
            name: "Concert".into(),
            time_base: TimeBase::Seconds,
            tracks: vec![Track { name: "Piste 1".into(), events: vec![Event { start: 12.0, refs: r, ..Default::default() }, Event::default()], ..Default::default() }],
            ..Default::default()
        };
        let label = |id: &str| format!("cue {id}");
        let u = users("position:plafond", &deck, std::slice::from_ref(&show), &label);
        assert_eq!(u, vec!["cue a", "cue b", "show « Concert », piste « Piste 1 », 12.0 s"]);
        assert!(users("position:autre", &deck, std::slice::from_ref(&show), &label).is_empty());
        // Unknown to the store: one warning per reference.
        let d = dangling(&deck, &[show], &[], &label);
        assert_eq!(d.len(), 3);
        assert!(d[0].contains("position:plafond") && d[0].contains("cue a"));
        let known = [pos("position:plafond", "Plafond", 0.0, 0.0)];
        assert!(dangling(&deck, &[], &known, &label).is_empty());
    }

    // ---------- through the real engine frame (preview only) ----------

    use std::sync::Mutex;
    use std::time::Instant;

    /// Mean x and y of the lit points after a few engine frames.
    fn centroid(shared: &Mutex<crate::Shared>, engine: &mut crate::Engine, now: &mut Instant) -> (f32, f32) {
        for _ in 0..4 {
            *now += crate::FRAME_INTERVAL;
            engine.tick(shared, *now);
        }
        let s = shared.lock().unwrap();
        assert!(!s.gate.is_armed(), "presets never arm");
        let lit: Vec<_> = s.frame.iter().filter(|p| p.is_lit()).collect();
        assert!(!lit.is_empty(), "the preview draws the cue");
        let n = lit.len() as f32;
        (lit.iter().map(|p| p.x).sum::<f32>() / n, lit.iter().map(|p| p.y).sum::<f32>() / n)
    }

    fn engine_for(s: crate::Shared, tag: &str) -> (Mutex<crate::Shared>, crate::Engine, std::path::PathBuf) {
        let shared = Mutex::new(s);
        let dir = std::env::temp_dir().join(format!("laser-studio-pools-{tag}-{}", std::process::id()));
        let engine = crate::Engine::new(&shared, None, dir.join("live.json"), dir.join("layers.json"));
        (shared, engine, dir)
    }

    fn press(s: &mut crate::Shared, id: &str) {
        let look = s.presets.iter().find(|p| p.id == id).unwrap().settings.clone();
        s.deck.press(id, Some(crate::cues::ClickMode::Toggle), crate::cues::At::default(), || look);
    }

    fn stop_all(s: &mut crate::Shared) {
        s.deck.active.clear();
        s.deck.ending.clear();
    }

    #[test]
    fn a_position_preset_moves_the_cues_that_refer_to_it_while_they_play() {
        let mut s = crate::test_support::shared();
        s.pools.set(vec![pos("position:plafond", "Plafond", 0.0, 0.0)]).unwrap();
        let ids: Vec<String> = s.presets.iter().take(3).map(|p| p.id.clone()).collect();
        let r = PresetRefs { position: Some("position:plafond".into()), ..Default::default() };
        for id in &ids[..2] {
            s.deck.set_slot(id, CueSlot { refs: r.clone(), ..Default::default() });
        }
        let (shared, mut engine, dir) = engine_for(s, "move");
        let mut now = Instant::now();
        for (i, id) in ids.iter().enumerate() {
            {
                let mut s = shared.lock().unwrap();
                stop_all(&mut s);
                let list = vec![pos("position:plafond", "Plafond", 0.0, 0.0)];
                set_all(&mut s, list).unwrap();
                press(&mut s, id);
            }
            let before = centroid(&shared, &mut engine, &mut now);
            // Moved while the cue plays: seen at the next frames.
            set_all(&mut shared.lock().unwrap(), vec![pos("position:plafond", "Plafond", 0.0, 0.3)]).unwrap();
            let after = centroid(&shared, &mut engine, &mut now);
            let dy = after.1 - before.1;
            if i < 2 {
                assert!(dy > 0.15, "cue {id} follows its preset: {before:?} → {after:?}");
            } else {
                assert!(dy.abs() < 0.05, "cue {id} keeps its own values: {before:?} → {after:?}");
            }
            assert!((after.0 - before.0).abs() < 0.05, "only y moved");
        }
        engine.shutdown(&shared);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_timeline_event_follows_its_preset_during_playback() {
        let mut s = crate::test_support::shared();
        s.pools.set(vec![pos("position:gauche", "Gauche", 0.0, 0.0)]).unwrap();
        let id = s.presets[0].id.clone();
        let refs = PresetRefs { position: Some("position:gauche".into()), ..Default::default() };
        let show = Show {
            name: "Réf".into(),
            tracks: vec![Track { name: "P".into(), events: vec![Event { start: 0.0, len: 10_000.0, source: crate::timeline::EventSource::Cue { id }, refs, ..Default::default() }], ..Default::default() }],
            ..Default::default()
        };
        s.timeline.load_unsaved(show);
        crate::controls::timeline_play(&mut s).unwrap();
        let (shared, mut engine, dir) = engine_for(s, "event");
        let mut now = Instant::now();
        let before = centroid(&shared, &mut engine, &mut now);
        set_all(&mut shared.lock().unwrap(), vec![pos("position:gauche", "Gauche", -0.3, 0.0)]).unwrap();
        let after = centroid(&shared, &mut engine, &mut now);
        assert!(before.0 - after.0 > 0.15, "{before:?} → {after:?}");
        // In use by the event: refused, with where.
        let (code, v) = delete(&mut shared.lock().unwrap(), "position:gauche").unwrap_err();
        assert_eq!(code, 409);
        assert!(v["users"][0].as_str().unwrap().contains("show « Réf »"), "{v}");
        engine.shutdown(&shared);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_capture_rename_and_slot_checks() {
        let mut s = crate::test_support::shared();
        s.live.pos_x = 0.25;
        s.live.size = 0.4;
        let req = CaptureRequest { family: PoolFamily::Position, id: None, name: "Plafond".into() };
        let p = capture(&mut s, &req).unwrap();
        assert_eq!(p.id, "position:plafond");
        assert_eq!(p.values, PoolValues::Position { pos_x: 0.25, pos_y: 0.0, rot_angle: [0.0; 3] }, "the family only");
        assert_eq!(capture(&mut s, &CaptureRequest { name: "".into(), ..req }).unwrap_err().0, 400);
        // Update from the live, renamed: same id.
        s.live.pos_x = -0.5;
        let up = capture(&mut s, &CaptureRequest { family: PoolFamily::Position, id: Some(p.id.clone()), name: "Écran".into() }).unwrap();
        assert_eq!((up.id.as_str(), up.name.as_str()), ("position:plafond", "Écran"));
        assert_eq!(capture(&mut s, &CaptureRequest { family: PoolFamily::Size, id: Some(p.id.clone()), name: String::new() }).unwrap_err().0, 400);
        // A cell refers to it: delete refused with the cell, then allowed.
        let cue = s.presets[0].id.clone();
        let mut refs = PresetRefs { position: Some(p.id.clone()), ..Default::default() };
        check_refs(&s.pools, &PresetRefs::default(), &mut refs).unwrap();
        let mut bad = PresetRefs { size: Some(p.id.clone()), ..Default::default() };
        assert!(check_refs(&s.pools, &PresetRefs::default(), &mut bad).is_err(), "wrong family");
        let mut gone = PresetRefs { motion: Some("motion:x".into()), ..Default::default() };
        assert!(check_refs(&s.pools, &PresetRefs::default(), &mut gone).is_err(), "unknown");
        assert!(check_refs(&s.pools, &gone.clone(), &mut gone).is_ok(), "a dangling one already there survives");
        s.deck.set_slot(&cue, CueSlot { refs, ..Default::default() });
        let (code, v) = delete(&mut s, &p.id).unwrap_err();
        assert_eq!(code, 409);
        assert_eq!(v["users"].as_array().unwrap().len(), 1);
        assert!(v["error"].as_str().unwrap().starts_with("Préréglage « Écran » utilisé par"), "{v}");
        // Dropping it from the whole list is refused the same way.
        assert_eq!(set_all(&mut s, Vec::new()).unwrap_err().0, 409);
        assert_eq!(view(&s)["uses"][&p.id].as_array().unwrap().len(), 1);
        s.deck.set_slot(&cue, CueSlot::default());
        delete(&mut s, &p.id).unwrap();
        assert!(s.pools.list().is_empty());
        assert_eq!(delete(&mut s, &p.id).unwrap_err().0, 404);
    }
}
