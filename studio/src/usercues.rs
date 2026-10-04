//! User cues (T-405): the operator's own looks, stored into a cell of the
//! cue grid with « Enregistrer dans une case » and refreshed with « Mettre
//! à jour », like a console's Store and Update.
//!
//! A user cue holds a whole look (`Settings`) and, on request, the master
//! live modifiers of the moment. It joins `Shared::presets` under the
//! category `CATEGORY`, so it plays exactly like a built-in cue: the same
//! `controls::press_cue`, click modes, groups, layers, MIDI cells, timeline
//! events and thumbnails, through the same layers, master stage, safety
//! chain and gate. Nothing here arms the laser.
//!
//! Ids are `u-<n>`, never reused (the store keeps a counter). Built-in cues
//! are never changed: storing onto one is refused (409) and the UI offers
//! « Copier vers une page utilisateur », which makes a user cue of it.
//!
//! Saved to `<data-dir>/user_cues.json` and in the project (`user_cues`).

use crate::cues::CueSlot;
use crate::engine::{Content, Settings};
use crate::grid::CueRef;
use crate::live::LiveModifiers;
use crate::presets::Preset;
use crate::Shared;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The category of user cues in `Shared::presets`. Not a page of its own:
/// user cues live on the pages the operator stores them into.
pub const CATEGORY: &str = "Mes cues";
/// Name of the page « Copier vers une page utilisateur » creates when
/// there is no user page yet.
pub const DEFAULT_PAGE: &str = "Mes cues";
pub const MAX_CUES: usize = 2000;
const MAX_NAME: usize = 40;
const DEFAULT_COLOR: [u8; 3] = [0, 255, 0];

/// The refusal shown when a store targets a built-in cue.
pub const BUILTIN_REFUSED: &str = "Les cues intégrés ne peuvent pas être remplacés.";
const LIBRARY_REFUSED: &str = "Les figures et les fichiers ILDA ne peuvent pas être remplacés ici.";
const NOT_EDITABLE: &str = "Les cues intégrés ne peuvent pas être modifiés : « Copier vers une page utilisateur » en fait une copie modifiable.";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserCue {
    /// `u-<n>`, unique, never reused.
    pub id: String,
    pub name: String,
    /// The cell's colour band.
    pub color: [u8; 3],
    /// The look.
    pub settings: Settings,
    /// Master live modifiers stored with the look; applied to this cue only
    /// (like a timeline event's), on top of which the master still applies.
    /// `None`: not stored.
    pub master: Option<LiveModifiers>,
}

impl Default for UserCue {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), color: DEFAULT_COLOR, settings: Settings::default(), master: None }
    }
}

/// What « Enregistrer » and « Mettre à jour » take from the current look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreFilter {
    /// The whole look (and the master modifiers when asked).
    #[default]
    All,
    /// `color` (and a generator's colour mode and second colour), plus the
    /// master colour override when asked.
    ColorOnly,
    /// `content`, `scale`, `rotation_speed`, plus the master geometry
    /// (size, position, rotation, perspective) when asked. Never the
    /// colour nor the brightness.
    GeometryOnly,
}

/// Whether `id` is a user cue's id (`u-<n>`), not a built-in cue, a
/// figure or an ILDA file.
#[cfg(test)]
pub fn is_user_id(id: &str) -> bool {
    number(id).is_some()
}

fn number(id: &str) -> Option<u64> {
    let n = id.strip_prefix("u-")?;
    (!n.is_empty() && n.len() <= 12 && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse().ok()).flatten()
}

pub fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

/// The colour of a look's cell: its main colour, green if it is black.
fn color_of(settings: &Settings) -> [u8; 3] {
    if settings.color == [0, 0, 0] {
        DEFAULT_COLOR
    } else {
        settings.color
    }
}

/// The colour part of a generator look: (colour mode, second colour).
fn gen_colors(c: &Content) -> Option<(crate::generators::ColorMode, [u8; 3])> {
    match c {
        Content::Generator { params, .. } => Some((params.color_mode, params.color2)),
        _ => None,
    }
}

fn set_gen_colors(c: &mut Content, colors: Option<(crate::generators::ColorMode, [u8; 3])>) {
    if let (Content::Generator { params, .. }, Some((mode, color2))) = (c, colors) {
        params.color_mode = mode;
        params.color2 = color2;
    }
}

/// A master copy fit to be stored: in range, without the momentary
/// reverse.
fn master_copy(live: &LiveModifiers) -> LiveModifiers {
    let mut m = live.clone();
    m.rot_reverse = false;
    crate::timeline::sanitize_modifiers(&mut m);
    m
}

/// Stores `look` (and `live`) into `cue` through `filter`.
pub fn merge(cue: &mut UserCue, look: &Settings, live: &LiveModifiers, filter: StoreFilter, with_master: bool) {
    let live = master_copy(live);
    match filter {
        StoreFilter::All => {
            cue.settings = look.clone();
            cue.master = with_master.then_some(live);
        }
        StoreFilter::ColorOnly => {
            cue.settings.color = look.color;
            set_gen_colors(&mut cue.settings.content, gen_colors(&look.content));
            if with_master {
                let m = cue.master.get_or_insert_with(LiveModifiers::default);
                m.color = live.color;
                m.color_params = live.color_params;
            }
        }
        StoreFilter::GeometryOnly => {
            let colors = gen_colors(&cue.settings.content);
            cue.settings.content = look.content.clone();
            set_gen_colors(&mut cue.settings.content, colors);
            cue.settings.scale = look.scale;
            cue.settings.rotation_speed = look.rotation_speed;
            if with_master {
                let m = cue.master.get_or_insert_with(LiveModifiers::default);
                m.size = live.size;
                m.size_x = live.size_x;
                m.size_y = live.size_y;
                m.pos_x = live.pos_x;
                m.pos_y = live.pos_y;
                m.rot_angle = live.rot_angle;
                m.rot_speed = live.rot_speed;
                m.rot_sync = live.rot_sync;
                m.perspective = live.perspective;
            }
        }
    }
}

/// Clamps a cue read from a file or a project; false if its id is invalid.
fn sanitize(cue: &mut UserCue) -> bool {
    let Some(n) = number(&cue.id) else { return false };
    cue.name = clean_name(&cue.name).unwrap_or_else(|| format!("Cue {n}"));
    if let Some(m) = &mut cue.master {
        *m = master_copy(m);
    }
    true
}

/// Checks the user cues of a project: valid, unique ids.
pub fn validate(cues: &mut [UserCue]) -> Result<()> {
    if cues.len() > MAX_CUES {
        anyhow::bail!("{MAX_CUES} cues utilisateur au plus");
    }
    let mut seen = std::collections::HashSet::new();
    for cue in cues.iter_mut() {
        if !sanitize(cue) {
            anyhow::bail!("cue utilisateur à l'identifiant invalide : « {} »", cue.id);
        }
        if !seen.insert(cue.id.clone()) {
            anyhow::bail!("cue utilisateur en double : « {} »", cue.id);
        }
    }
    Ok(())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct UserCueFile {
    /// The next id's number: ids are never reused, even after a delete.
    next: u64,
    cues: Vec<UserCue>,
}

#[derive(Clone, Debug)]
pub struct UserCueStore {
    path: Option<PathBuf>,
    next: u64,
    cues: Vec<UserCue>,
}

impl UserCueStore {
    /// `user_cues.json`, or none. A damaged file is set aside (datafile.rs).
    pub fn load_or_create(path: PathBuf) -> Self {
        let file: UserCueFile = crate::load_json(&path);
        let mut store = Self { path: Some(path), next: file.next, cues: Vec::new() };
        store.replace_in_memory(file.cues);
        store
    }

    /// A store that never touches the disk (tests).
    pub fn in_memory() -> Self {
        Self { path: None, next: 1, cues: Vec::new() }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn list(&self) -> &[UserCue] {
        &self.cues
    }

    pub fn get(&self, id: &str) -> Option<&UserCue> {
        self.cues.iter().find(|c| c.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut UserCue> {
        self.cues.iter_mut().find(|c| c.id == id)
    }

    /// Takes `cues` (a project's), in memory only. Invalid and duplicate
    /// ids are dropped; the id counter never goes back.
    pub fn replace_in_memory(&mut self, cues: Vec<UserCue>) {
        let mut seen = std::collections::HashSet::new();
        self.cues = cues.into_iter().filter_map(|mut c| (sanitize(&mut c) && seen.insert(c.id.clone())).then_some(c)).take(MAX_CUES).collect();
        let top = self.cues.iter().filter_map(|c| number(&c.id)).max().unwrap_or(0);
        self.next = self.next.max(top + 1).max(1);
    }

    /// A new cue with the next id. `name` defaults to « Cue <n> ».
    pub fn create(&mut self, name: Option<&str>, color: Option<[u8; 3]>, settings: Settings, master: Option<LiveModifiers>) -> Result<String> {
        if self.cues.len() >= MAX_CUES {
            anyhow::bail!("{MAX_CUES} cues utilisateur au plus");
        }
        let n = self.next;
        self.next += 1;
        let id = format!("u-{n}");
        let name = name.and_then(clean_name).unwrap_or_else(|| format!("Cue {n}"));
        let color = color.unwrap_or_else(|| color_of(&settings));
        self.cues.push(UserCue { id: id.clone(), name, color, settings, master: master.as_ref().map(master_copy) });
        Ok(id)
    }

    /// « Mettre à jour » / storing onto a user cue.
    pub fn store_into(&mut self, id: &str, look: &Settings, live: &LiveModifiers, filter: StoreFilter, with_master: bool) -> Result<()> {
        let cue = self.get_mut(id).ok_or_else(|| anyhow::anyhow!("cue utilisateur inconnu"))?;
        merge(cue, look, live, filter, with_master);
        Ok(())
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        let name = clean_name(name).ok_or_else(|| anyhow::anyhow!("nom de cue vide"))?;
        self.get_mut(id).ok_or_else(|| anyhow::anyhow!("cue utilisateur inconnu"))?.name = name;
        Ok(())
    }

    pub fn set_color(&mut self, id: &str, color: [u8; 3]) -> Result<()> {
        self.get_mut(id).ok_or_else(|| anyhow::anyhow!("cue utilisateur inconnu"))?.color = color;
        Ok(())
    }

    /// A copy under a new id, named « <name> copie ».
    pub fn duplicate(&mut self, id: &str) -> Result<String> {
        let src = self.get(id).ok_or_else(|| anyhow::anyhow!("cue utilisateur inconnu"))?.clone();
        let name: String = src.name.chars().take(MAX_NAME - 6).collect();
        self.create(Some(&format!("{name} copie")), Some(src.color), src.settings, src.master)
    }

    pub fn remove(&mut self, id: &str) {
        self.cues.retain(|c| c.id != id);
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let file = UserCueFile { next: self.next, cues: self.cues.clone() };
        let json = serde_json::to_string_pretty(&file).context("failed to serialize user cues")?;
        crate::datafile::replace(path, json.as_bytes()).with_context(|| format!("failed to write {}", path.display()))
    }

    /// The file's bytes, for a project open (written lock released).
    pub fn file_bytes(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(&UserCueFile { next: self.next, cues: self.cues.clone() })
    }
}

fn preset_of(c: &UserCue) -> Preset {
    Preset { id: c.id.clone(), name: c.name.clone(), category: CATEGORY, settings: c.settings.clone() }
}

/// The user cues into `Shared::presets`, without touching the grid.
pub fn sync_presets(s: &mut Shared) {
    s.presets.retain(|p| p.category != CATEGORY);
    let cues: Vec<Preset> = s.user_cues.list().iter().map(preset_of).collect();
    s.presets.extend(cues);
}

/// After the user cues changed: presets, grid view and grid controls.
pub fn refresh(s: &mut Shared) {
    sync_presets(s);
    crate::grid::refresh(s);
}

/// The master modifiers stored with `cue`, if it is a user cue that has
/// any that do something.
pub fn master_of(s: &Shared, cue: &str) -> Option<LiveModifiers> {
    s.user_cues.get(cue)?.master.clone().filter(|m| *m != LiveModifiers::default())
}

/// An edit refused: HTTP status and French message.
pub type Refusal = (u16, String);

fn bad(e: impl std::fmt::Display) -> Refusal {
    (400, e.to_string())
}

/// Commits the edited store and grid: saved, presets and controls rebuilt.
fn commit(s: &mut Shared, cues: UserCueStore, grid: crate::grid::CueGrid) -> Result<(), Refusal> {
    cues.save().map_err(|e| (500, format!("{e:#}")))?;
    s.user_cues = cues;
    s.grid = grid;
    refresh(s);
    s.grid.save();
    Ok(())
}

/// « Enregistrer dans une case »: the current look into cell `slot` of
/// page `page`. An empty cell gets a new user cue (the whole look, whatever
/// the filter); a user cue is stored into through `filter`; a built-in
/// cue, a figure or an ILDA file is refused with 409. Returns the cue id.
pub fn store(s: &mut Shared, page: usize, slot: usize, name: Option<&str>, filter: StoreFilter, with_master: bool) -> Result<String, Refusal> {
    let mut grid = s.grid.clone();
    grid.materialize(&s.presets);
    let pg = grid.pages.get(page).ok_or_else(|| bad("page inconnue"))?;
    if pg.category.is_some() {
        return Err(bad(format!("la page « {} » suit sa bibliothèque : on ne peut pas y enregistrer de cue", pg.name)));
    }
    if slot >= grid.capacity() {
        return Err(bad("case hors de la grille"));
    }
    // A cell whose cue no longer exists (a deleted figure) counts as empty.
    let current = pg.slots.get(slot).cloned().flatten().filter(|r| s.presets.iter().any(|p| p.id == r.id()));
    let mut cues = s.user_cues.clone();
    let id = match current {
        None => {
            let master = with_master.then(|| s.live.clone());
            let id = cues.create(name, None, s.settings.clone(), master).map_err(bad)?;
            grid.put(page, slot, Some(CueRef::User(id.clone()))).map_err(bad)?;
            id
        }
        Some(CueRef::User(id)) if cues.get(&id).is_some() => {
            cues.store_into(&id, &s.settings, &s.live, filter, with_master).map_err(bad)?;
            if let Some(name) = name.filter(|n| clean_name(n).is_some()) {
                cues.rename(&id, name).map_err(bad)?;
            }
            id
        }
        Some(CueRef::Builtin(_)) => return Err((409, BUILTIN_REFUSED.into())),
        Some(CueRef::User(_)) => return Err((409, LIBRARY_REFUSED.into())),
    };
    commit(s, cues, grid)?;
    Ok(id)
}

/// The user cue « Mettre à jour » refreshes: `id` if given, else the newest
/// playing user cue.
pub fn update_target(s: &Shared, id: Option<&str>) -> Result<String, Refusal> {
    match id {
        Some(id) if s.user_cues.get(id).is_some() => Ok(id.to_string()),
        Some(id) if s.presets.iter().any(|p| p.id == id) => Err((409, BUILTIN_REFUSED.into())),
        Some(_) => Err((404, "cue inconnu".into())),
        None => s
            .deck
            .active
            .iter()
            .rev()
            .map(|a| a.cue.as_str())
            .find(|c| s.user_cues.get(c).is_some())
            .map(str::to_string)
            .ok_or_else(|| (409, "Aucun cue utilisateur en cours : lancer le cue à mettre à jour.".into())),
    }
}

/// « Mettre à jour »: the current look into user cue `id` (or the one
/// playing), through `filter`. Nothing else changes.
pub fn update(s: &mut Shared, id: Option<&str>, filter: StoreFilter, with_master: bool) -> Result<String, Refusal> {
    let id = update_target(s, id)?;
    let mut cues = s.user_cues.clone();
    cues.store_into(&id, &s.settings, &s.live, filter, with_master).map_err(bad)?;
    let grid = s.grid.clone();
    commit(s, cues, grid)?;
    Ok(id)
}

/// `POST /api/grid/edit`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GridEdit {
    /// Drag and drop; onto a cue: the two swap.
    Move { from: (usize, usize), to: (usize, Option<usize>) },
    Swap { from: (usize, usize), to: (usize, usize) },
    /// Alt + drag: a user cue is duplicated (new id, its properties too); a
    /// built-in cue, figure or ILDA file is referenced again.
    Copy { from: (usize, usize), to: (usize, Option<usize>) },
    /// « Copier vers une page utilisateur »: an editable user cue made from
    /// any cue, into `to` or the first user page (made if none).
    CopyToUser {
        from: (usize, usize),
        #[serde(default)]
        to: Option<(usize, Option<usize>)>,
    },
    Rename { from: (usize, usize), name: String },
    Color { from: (usize, usize), color: [u8; 3] },
    /// Empties the cell; a user cue no other cell holds is deleted.
    Delete { from: (usize, usize) },
}

/// The cue shown in a cell (view indices, so library pages work too).
fn cell_cue(s: &Shared, (page, slot): (usize, usize)) -> Result<String, Refusal> {
    s.grid.slot(page, slot).map(str::to_string).ok_or_else(|| bad("case vide"))
}

/// The cell a copy goes to: `to`'s slot, else the page's first empty one.
fn target(grid: &crate::grid::CueGrid, (page, slot): (usize, Option<usize>)) -> Result<(usize, usize), Refusal> {
    let pg = grid.pages.get(page).ok_or_else(|| bad("page inconnue"))?;
    if pg.category.is_some() {
        return Err(bad(format!("la page « {} » suit sa bibliothèque : on ne peut pas y déposer de cue", pg.name)));
    }
    let slot = match slot {
        Some(t) if t < grid.capacity() => t,
        Some(_) => return Err(bad("case hors de la grille")),
        None => grid.first_empty(page).ok_or_else(|| bad(format!("page « {} » pleine", pg.name)))?,
    };
    if grid.pages[page].slots.get(slot).is_some_and(Option::is_some) {
        return Err(bad("case occupée"));
    }
    Ok((page, slot))
}

/// Runs a grid edit. `Ok(Some(n))`: show page n.
pub fn edit(s: &mut Shared, op: GridEdit) -> Result<Option<usize>, Refusal> {
    let mut grid = s.grid.clone();
    grid.materialize(&s.presets);
    let mut cues = s.user_cues.clone();
    let mut show = None;
    let mut deleted = None;
    let mut slot_copy = None;
    match op {
        GridEdit::Move { from, to } => grid.move_cell(from, to, &s.presets).map_err(bad)?,
        GridEdit::Swap { from, to } => {
            if s.grid.slot(to.0, to.1).is_none() {
                return Err(bad("case vide : rien à échanger"));
            }
            grid.move_cell(from, (to.0, Some(to.1)), &s.presets).map_err(bad)?
        }
        GridEdit::Copy { from, to } => {
            let id = cell_cue(s, from)?;
            let (page, slot) = target(&grid, to)?;
            let cue = if cues.get(&id).is_some() {
                let new = cues.duplicate(&id).map_err(bad)?;
                slot_copy = Some((id, new.clone(), false));
                CueRef::User(new)
            } else {
                let p = s.presets.iter().find(|p| p.id == id).ok_or_else(|| bad("cue inconnu"))?;
                CueRef::of(p)
            };
            grid.put(page, slot, Some(cue)).map_err(bad)?;
        }
        GridEdit::CopyToUser { from, to } => {
            let id = cell_cue(s, from)?;
            let p = s.presets.iter().find(|p| p.id == id).ok_or_else(|| bad("cue inconnu"))?;
            let to = match to {
                Some(to) => to,
                None => match grid.pages.iter().position(|pg| pg.user && pg.category.is_none()).filter(|&n| grid.first_empty(n).is_some()) {
                    Some(n) => (n, None),
                    None => (grid.new_page(DEFAULT_PAGE, &s.presets).map_err(bad)?, None),
                },
            };
            let (page, slot) = target(&grid, to)?;
            let new = match cues.get(&id) {
                Some(_) => cues.duplicate(&id).map_err(bad)?,
                // The cell's program (T-157) is baked into the copy's look.
                None => cues.create(Some(&p.name), None, s.deck.with_program(&id, p.settings.clone()), None).map_err(bad)?,
            };
            slot_copy = Some((id, new.clone(), true));
            grid.put(page, slot, Some(CueRef::User(new))).map_err(bad)?;
            show = Some(page);
        }
        GridEdit::Rename { from, name } => {
            let id = cell_cue(s, from)?;
            if cues.get(&id).is_none() {
                return Err((409, NOT_EDITABLE.into()));
            }
            cues.rename(&id, &name).map_err(bad)?;
        }
        GridEdit::Color { from, color } => {
            let id = cell_cue(s, from)?;
            if cues.get(&id).is_none() {
                return Err((409, NOT_EDITABLE.into()));
            }
            cues.set_color(&id, color).map_err(bad)?;
        }
        GridEdit::Delete { from } => {
            let id = cell_cue(s, from).ok();
            grid.clear_cell(from.0, from.1, &s.presets).map_err(bad)?;
            if let Some(id) = id.filter(|id| cues.get(id).is_some() && !grid.references(id)) {
                cues.remove(&id);
                deleted = Some(id);
            }
        }
    }
    commit(s, cues, grid)?;
    // Cell properties: a copy gets its source's (a « copie modifiable »
    // has the program baked in its look); a deleted cue's go, and it stops.
    if let Some((from, to, bake)) = slot_copy {
        let slot = s.deck.slot(&from);
        s.deck.set_slot(&to, if bake { CueSlot { program: None, ..slot } } else { slot });
        s.deck.save();
    }
    if let Some(id) = deleted {
        s.deck.slots.remove(&id);
        s.deck.save();
        crate::controls::stop_cue(s, &id);
    }
    if let Some(n) = show {
        s.cue_page = n.min(s.grid.page_count() - 1);
    }
    Ok(show)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::{ColorMode, GenParams};
    use crate::presets::catalog;

    fn gen(name: &str, mode: ColorMode, color: [u8; 3], scale: f32) -> Settings {
        let params = GenParams { color_mode: mode, color2: [0, 0, 255], ..GenParams::default() };
        Settings { content: Content::Generator { generator: name.into(), params }, color, scale, rotation_speed: 10.0, brightness: 0.8, ..Settings::default() }
    }

    fn of(cat: &str) -> Vec<String> {
        catalog().into_iter().filter(|p| p.category == cat).map(|p| p.id).collect()
    }

    /// A shared state with one empty user page; returns its index.
    fn with_user_page(s: &mut Shared) -> usize {
        let mut g = s.grid.clone();
        let n = g.new_page("Mes looks", &s.presets).unwrap();
        s.grid = g;
        refresh(s);
        n
    }

    #[test]
    fn ids_are_user_ids_only() {
        assert!(is_user_id("u-1") && is_user_id("u-204"));
        for id in ["u-", "u-x", "tunnels-001", "figure:u-1", "u-1a", "U-1", ""] {
            assert!(!is_user_id(id), "{id}");
        }
    }

    #[test]
    fn store_filters_take_what_they_say() {
        let old = gen("tunnel", ColorMode::Solid, [255, 0, 0], 0.3);
        let look = Settings { brightness: 0.4, ..gen("fan", ColorMode::Rainbow, [0, 255, 255], 0.9) };
        let live = LiveModifiers { size: 0.5, pos_x: 0.2, brightness: 0.6, color: crate::live::ColorOverride::Fixed { rgb: [9, 9, 9] }, rot_reverse: true, ..LiveModifiers::default() };
        let base = UserCue { id: "u-1".into(), settings: old.clone(), ..UserCue::default() };

        let mut c = base.clone();
        merge(&mut c, &look, &live, StoreFilter::All, false);
        assert_eq!((c.settings.clone(), c.master.clone()), (look.clone(), None));
        merge(&mut c, &look, &live, StoreFilter::All, true);
        let m = c.master.clone().unwrap();
        assert_eq!((m.size, m.brightness, m.rot_reverse), (0.5, 0.6, false), "the momentary reverse is never stored");

        // Colour only: the colour (and the generator's colour mode), nothing else.
        let mut c = base.clone();
        merge(&mut c, &look, &live, StoreFilter::ColorOnly, false);
        assert_eq!(c.settings.color, look.color);
        assert_eq!(gen_colors(&c.settings.content), Some((ColorMode::Rainbow, [0, 0, 255])));
        assert!(c.settings.content.same_drawing(&old.content));
        assert_eq!((c.settings.scale, c.settings.brightness, c.master.clone()), (0.3, 0.8, None));
        merge(&mut c, &look, &live, StoreFilter::ColorOnly, true);
        let m = c.master.clone().unwrap();
        assert_eq!((m.color, m.size, m.brightness), (crate::live::ColorOverride::Fixed { rgb: [9, 9, 9] }, 1.0, 1.0), "colour only: no geometry, no dimmer");

        // Geometry only: neither the colour nor the brightness.
        let mut c = base.clone();
        merge(&mut c, &look, &live, StoreFilter::GeometryOnly, true);
        assert!(c.settings.content.same_drawing(&look.content));
        assert_eq!((c.settings.scale, c.settings.rotation_speed), (0.9, 10.0));
        assert_eq!((c.settings.color, c.settings.brightness), (old.color, old.brightness));
        assert_eq!(gen_colors(&c.settings.content), Some((ColorMode::Solid, [0, 0, 255])));
        let m = c.master.unwrap();
        assert_eq!((m.size, m.pos_x, m.brightness, m.color), (0.5, 0.2, 1.0, crate::live::ColorOverride::Normal));
    }

    #[test]
    fn ids_are_never_reused_and_survive_a_reload() {
        let dir = std::env::temp_dir().join(format!("laser-studio-usercues-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("user_cues.json");
        let mut store = UserCueStore::load_or_create(path.clone());
        let a = store.create(None, None, Settings::default(), None).unwrap();
        let b = store.create(Some("  Intro  "), None, Settings::default(), None).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("u-1", "u-2"));
        assert_eq!(store.get("u-1").unwrap().name, "Cue 1");
        assert_eq!(store.get("u-2").unwrap().name, "Intro");
        store.remove(&b);
        store.save().unwrap();
        let mut back = UserCueStore::load_or_create(path.clone());
        assert_eq!(back.list().len(), 1);
        assert_eq!(back.create(None, None, Settings::default(), None).unwrap(), "u-3", "u-2 is never given again");
        // A project's cues: invalid or duplicate ids dropped, the counter never goes back.
        back.replace_in_memory(vec![
            UserCue { id: "u-9".into(), ..UserCue::default() },
            UserCue { id: "u-9".into(), ..UserCue::default() },
            UserCue { id: "tunnels-001".into(), ..UserCue::default() },
        ]);
        assert_eq!(back.list().len(), 1);
        assert_eq!(back.create(None, None, Settings::default(), None).unwrap(), "u-10");
        assert_eq!(back.duplicate("u-9").unwrap(), "u-11");
        assert_eq!(back.get("u-11").unwrap().name, "Cue 9 copie");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_is_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("usercues", "user_cues.json");
        let mut store = UserCueStore::load_or_create(path.clone());
        assert!(store.list().is_empty());
        store.create(None, None, Settings::default(), None).unwrap();
        store.save().unwrap();
        assert_eq!(UserCueStore::load_or_create(path).list().len(), 1);
        crate::datafile::tests::check_kept(&dir, "user_cues.json", false);
    }

    #[test]
    fn storing_into_an_empty_cell_makes_a_cue_that_plays() {
        let mut s = crate::test_support::shared();
        let n = with_user_page(&mut s);
        s.settings = gen("tunnel", ColorMode::Solid, [255, 0, 255], 0.4);
        s.live.size = 0.5;
        // Any filter on an empty cell: the whole look.
        let id = store(&mut s, n, 3, Some("Mon look"), StoreFilter::ColorOnly, true).unwrap();
        assert_eq!(id, "u-1");
        let cue = s.user_cues.get(&id).unwrap();
        assert_eq!((cue.name.as_str(), cue.color, cue.settings.clone()), ("Mon look", [255, 0, 255], s.settings.clone()));
        assert_eq!(cue.master.as_ref().unwrap().size, 0.5);
        assert_eq!(s.grid.slot(n, 3), Some("u-1"));
        assert!(s.presets.iter().any(|p| p.id == "u-1" && p.category == CATEGORY));
        assert!(s.controls.get(&format!("grid.{}.1.4", n + 1)).is_some(), "MIDI and controls reach it");
        assert_eq!(master_of(&s, "u-1").unwrap().size, 0.5);
        // It plays like any cue.
        s.settings = Settings::default();
        assert!(crate::controls::press_cue(&mut s, "u-1", None, true));
        assert_eq!(s.active_cue.as_deref(), Some("u-1"));
        assert!(s.settings.content.same_drawing(&gen("tunnel", ColorMode::Solid, [0; 3], 1.0).content));
    }

    #[test]
    fn storing_onto_a_user_cue_replaces_through_the_filter_and_never_a_builtin() {
        let mut s = crate::test_support::shared();
        let n = with_user_page(&mut s);
        s.settings = gen("tunnel", ColorMode::Solid, [255, 0, 0], 0.4);
        let id = store(&mut s, n, 0, None, StoreFilter::All, false).unwrap();
        let other = store(&mut s, n, 1, None, StoreFilter::All, false).unwrap();
        s.settings = gen("fan", ColorMode::Solid, [0, 0, 255], 0.9);
        assert_eq!(store(&mut s, n, 0, None, StoreFilter::ColorOnly, false).unwrap(), id);
        let c = s.user_cues.get(&id).unwrap();
        assert_eq!((c.settings.color, c.settings.scale), ([0, 0, 255], 0.4));
        assert!(c.settings.content.same_drawing(&gen("tunnel", ColorMode::Solid, [0; 3], 1.0).content));
        assert_eq!(s.user_cues.get(&other).unwrap().settings.color, [255, 0, 0], "the other cue is untouched");
        // A built-in cue: refused, nothing changes.
        let before = (s.user_cues.list().to_vec(), s.grid.clone());
        let (code, msg) = store(&mut s, 1, 0, None, StoreFilter::All, false).unwrap_err();
        assert_eq!((code, msg.as_str()), (409, BUILTIN_REFUSED));
        assert_eq!((s.user_cues.list().to_vec(), s.grid.clone()), before);
        assert_eq!(s.grid.slot(1, 0), Some(of("Tunnels")[0].as_str()));
        // A library page: refused.
        let figures = crate::presets::CATEGORIES.iter().position(|c| *c == crate::figures::CATEGORY).unwrap();
        assert_eq!(store(&mut s, figures, 0, None, StoreFilter::All, false).unwrap_err().0, 400);
    }

    #[test]
    fn update_changes_the_playing_user_cue_and_nothing_else() {
        let mut s = crate::test_support::shared();
        let n = with_user_page(&mut s);
        s.settings = gen("tunnel", ColorMode::Solid, [255, 0, 0], 0.4);
        let a = store(&mut s, n, 0, None, StoreFilter::All, false).unwrap();
        let b = store(&mut s, n, 1, None, StoreFilter::All, false).unwrap();
        assert_eq!(update(&mut s, None, StoreFilter::All, false).unwrap_err().0, 409, "nothing plays");
        crate::controls::press_cue(&mut s, &a, None, true);
        s.settings.scale = 0.95;
        s.settings.color = [0, 255, 0];
        assert_eq!(update(&mut s, None, StoreFilter::GeometryOnly, false).unwrap(), a);
        let c = s.user_cues.get(&a).unwrap();
        assert_eq!((c.settings.scale, c.settings.color), (0.95, [255, 0, 0]));
        assert_eq!(s.user_cues.get(&b).unwrap().settings.scale, 0.4);
        // Over a built-in cue on top, the newest *user* cue is the target.
        s.deck.multi = true;
        let tunnel = of("Tunnels")[0].clone();
        crate::controls::press_cue(&mut s, &tunnel, None, true);
        assert_eq!(update_target(&s, None).unwrap(), a);
        assert_eq!(update(&mut s, Some(&tunnel), StoreFilter::All, false).unwrap_err().0, 409);
        assert_eq!(update(&mut s, Some("u-99"), StoreFilter::All, false).unwrap_err().0, 404);
    }

    #[test]
    fn move_copy_and_swap_keep_or_duplicate_the_cell_properties() {
        let mut s = crate::test_support::shared();
        let n = with_user_page(&mut s);
        let m = with_user_page(&mut s);
        s.settings = gen("tunnel", ColorMode::Solid, [255, 0, 0], 0.4);
        let a = store(&mut s, n, 0, None, StoreFilter::All, false).unwrap();
        s.settings = gen("fan", ColorMode::Solid, [0, 0, 255], 0.4);
        let b = store(&mut s, n, 1, None, StoreFilter::All, false).unwrap();
        let props = CueSlot { mode: Some(crate::cues::ClickMode::Flash), group: Some(2), layer: Some(3), ..CueSlot::default() };
        s.deck.set_slot(&a, props.clone());

        // Swap within the page: properties follow their cue.
        edit(&mut s, GridEdit::Swap { from: (n, 0), to: (n, 1) }).unwrap();
        assert_eq!((s.grid.slot(n, 0), s.grid.slot(n, 1)), (Some(b.as_str()), Some(a.as_str())));
        assert_eq!(s.deck.slot(&a), props);
        assert_eq!(s.deck.slot(&b), CueSlot::default());
        assert!(edit(&mut s, GridEdit::Swap { from: (n, 0), to: (n, 7) }).is_err(), "nothing to swap with");
        // Move to another page.
        edit(&mut s, GridEdit::Move { from: (n, 1), to: (m, Some(4)) }).unwrap();
        assert_eq!((s.grid.slot(n, 1), s.grid.slot(m, 4)), (None, Some(a.as_str())));
        assert_eq!(s.deck.slot(&a), props);
        // Copy: a new user cue, new id, same look and properties.
        edit(&mut s, GridEdit::Copy { from: (m, 4), to: (n, None) }).unwrap();
        let copy = s.grid.slot(n, 1).unwrap().to_string();
        assert!(is_user_id(&copy) && copy != a);
        assert_eq!(s.user_cues.get(&copy).unwrap().settings, s.user_cues.get(&a).unwrap().settings);
        assert_eq!(s.deck.slot(&copy), props);
        assert!(edit(&mut s, GridEdit::Copy { from: (m, 4), to: (n, Some(0)) }).is_err(), "occupied");
        // Copying a built-in cue references it.
        let tunnel = of("Tunnels")[0].clone();
        edit(&mut s, GridEdit::Copy { from: (1, 0), to: (m, Some(0)) }).unwrap();
        assert_eq!(s.grid.slot(m, 0), Some(tunnel.as_str()));
        assert_eq!(s.grid.slot(1, 0), Some(tunnel.as_str()));
    }

    #[test]
    fn rename_color_and_delete_only_touch_user_cues() {
        let mut s = crate::test_support::shared();
        let n = with_user_page(&mut s);
        let id = store(&mut s, n, 0, None, StoreFilter::All, false).unwrap();
        s.deck.set_slot(&id, CueSlot { group: Some(1), ..CueSlot::default() });
        edit(&mut s, GridEdit::Rename { from: (n, 0), name: "Final".into() }).unwrap();
        edit(&mut s, GridEdit::Color { from: (n, 0), color: [1, 2, 3] }).unwrap();
        let c = s.user_cues.get(&id).unwrap();
        assert_eq!((c.name.as_str(), c.color), ("Final", [1, 2, 3]));
        assert_eq!(s.presets.iter().find(|p| p.id == id).unwrap().name, "Final");
        assert_eq!(edit(&mut s, GridEdit::Rename { from: (1, 0), name: "X".into() }).unwrap_err().0, 409);
        assert_eq!(edit(&mut s, GridEdit::Color { from: (1, 0), color: [0; 3] }).unwrap_err().0, 409);
        // Delete: the cell, the cue, its properties; a playing one stops.
        crate::controls::press_cue(&mut s, &id, None, true);
        edit(&mut s, GridEdit::Delete { from: (n, 0) }).unwrap();
        assert!(s.user_cues.get(&id).is_none() && !s.presets.iter().any(|p| p.id == id));
        assert!(!s.deck.slots.contains_key(&id));
        assert!(s.deck.active.is_empty());
        // A built-in cell: emptied, the cue stays in the catalogue.
        let tunnel = of("Tunnels")[0].clone();
        edit(&mut s, GridEdit::Delete { from: (1, 0) }).unwrap();
        assert_eq!(s.grid.slot(1, 0), None);
        assert!(s.presets.iter().any(|p| p.id == tunnel));
    }

    #[test]
    fn copy_to_user_makes_an_editable_copy_on_a_user_page() {
        let mut s = crate::test_support::shared();
        let tunnel = catalog().into_iter().find(|p| p.category == "Tunnels").unwrap();
        s.deck.set_slot(&tunnel.id, CueSlot { layer: Some(2), ..CueSlot::default() });
        // No user page yet: « Mes cues » is made.
        let page = edit(&mut s, GridEdit::CopyToUser { from: (1, 0), to: None }).unwrap().unwrap();
        assert_eq!(s.grid.view()[page].name, DEFAULT_PAGE);
        assert!(s.grid.view()[page].user);
        let id = s.grid.slot(page, 0).unwrap().to_string();
        let c = s.user_cues.get(&id).unwrap();
        assert_eq!((c.name.as_str(), &c.settings), (tunnel.name.as_str(), &tunnel.settings));
        assert_eq!(s.deck.slot(&id).layer, Some(2));
        // The next copy lands on the same page.
        assert_eq!(edit(&mut s, GridEdit::CopyToUser { from: (1, 1), to: None }).unwrap(), Some(page));
        assert!(s.grid.slot(page, 1).is_some_and(is_user_id));
        // The built-in cue is untouched.
        assert_eq!(s.grid.slot(1, 0), Some(tunnel.id.as_str()));
        assert_eq!(s.presets.iter().find(|p| p.id == tunnel.id).unwrap().settings, tunnel.settings);
    }
}
