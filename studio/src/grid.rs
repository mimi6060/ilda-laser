//! The cue grid's layout (T-272): a fixed number of columns and rows (8×5
//! by default, one APC40 page) and pages of cells that may be empty. A cue
//! keeps its cell whatever the window's width, so the operator's hands
//! (and a controller's pads) always find it in the same place.
//!
//! Until the operator edits a page, the pages are built from the cue
//! categories (`presets::CATEGORIES`), in order: one page per category,
//! and the cues that don't fit go on pages added after the last one
//! (« Abstraits 2 »), so `grid.<page>.<row>.<col>` and `page.N` keep
//! meaning what they did before. The « Figures » and « ILDA » pages follow
//! the operator's libraries: their cells are always that library's items.
//! Pages added to the catalogue later (`presets::LATE_PAGES`: « Festival »,
//! « Festival évolutifs ») come after those overflow pages, and are
//! appended once to the operator's own pages, so no page they had changes
//! number.
//!
//! Saved to `studio-data/pages.json` and in the project (`pages`). A page
//! keeps every cue it holds when the grid gets smaller: cells past the last
//! one are kept, not shown, and come back when it grows again.

use crate::presets::{Preset, CATEGORIES, LATE_PAGES};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MIN_COLS: u8 = 4;
pub const MAX_COLS: u8 = 12;
pub const MIN_ROWS: u8 = 2;
pub const MAX_ROWS: u8 = 8;
pub const DEFAULT_COLS: u8 = 8;
pub const DEFAULT_ROWS: u8 = 5;
pub const MAX_PAGES: usize = 64;
const MAX_SLOTS: usize = MAX_COLS as usize * MAX_ROWS as usize;
const MAX_NAME: usize = 40;

/// What a cell plays: a built-in cue, or one of the operator's own
/// (figure, ILDA file), by id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueRef {
    Builtin(String),
    User(String),
}

impl CueRef {
    pub fn id(&self) -> &str {
        match self {
            CueRef::Builtin(id) | CueRef::User(id) => id,
        }
    }

    /// How a cell refers to `p`: user (library) or built-in.
    pub fn of(p: &Preset) -> Self {
        if is_library(p.category) {
            CueRef::User(p.id.clone())
        } else {
            CueRef::Builtin(p.id.clone())
        }
    }
}

/// The categories filled by the operator's libraries, not by the catalogue.
fn is_library(category: &str) -> bool {
    category == crate::figures::CATEGORY || category == crate::ilda_library::CATEGORY
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CuePage {
    pub name: String,
    /// Row-major cells; `None` = empty. May be longer than the grid.
    pub slots: Vec<Option<CueRef>>,
    /// A library page (Figures, ILDA): its cells are that category's cues,
    /// in order, and `slots` is unused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// A page the operator made (« Nouvelle page », T-405), after the
    /// built-in ones: where « Copier vers une page utilisateur » goes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub user: bool,
}

/// A page as shown: the cue id in each cell of the current grid.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageView {
    pub name: String,
    /// `cols × rows` cells, row-major; `None` = empty (or a cue that no
    /// longer exists, e.g. a deleted figure).
    pub cells: Vec<Option<String>>,
    /// Cues the page holds past its last cell (a smaller grid).
    pub hidden: usize,
    /// A library page: cells can be dragged from it, not onto it.
    pub auto: bool,
    /// False for the extra pages of a library longer than one page: they
    /// exist only on screen and can't be renamed or deleted.
    pub editable: bool,
    /// A page the operator made (T-405).
    pub user: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CueGrid {
    pub cols: u8,
    pub rows: u8,
    /// Empty: the default pages, built from the categories (see above).
    pub pages: Vec<CuePage>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    view: Vec<PageView>,
    /// The late catalogue pages (`presets::LATE_PAGES`) already given to
    /// the operator's own pages: each is appended once, and stays deleted
    /// if the operator deletes it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub late_pages: Vec<String>,
    /// Bumped by every `resolve`, so the page knows when to reload the grid.
    #[serde(skip)]
    pub rev: u64,
}

impl Default for CueGrid {
    fn default() -> Self {
        Self { cols: DEFAULT_COLS, rows: DEFAULT_ROWS, pages: Vec::new(), late_pages: Vec::new(), path: None, view: Vec::new(), rev: 0 }
    }
}

impl PartialEq for CueGrid {
    fn eq(&self, other: &Self) -> bool {
        (self.cols, self.rows, &self.pages) == (other.cols, other.rows, &other.pages)
    }
}

impl CueGrid {
    /// Loads `pages.json`, or the defaults. Changes are saved back to `path`.
    pub fn load(path: PathBuf) -> Self {
        let mut grid: CueGrid = crate::load_json(&path);
        grid.path = Some(path);
        grid.sanitize();
        grid.add_late_pages(&crate::presets::catalog());
        grid
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(Path::new(path), self);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Cells per page.
    pub fn capacity(&self) -> usize {
        self.cols as usize * self.rows as usize
    }

    /// Whether the pages are still the default ones.
    pub fn is_default(&self) -> bool {
        self.pages.is_empty()
    }

    /// Clamps the size, names the unnamed pages, drops what is out of range.
    pub fn sanitize(&mut self) {
        self.cols = self.cols.clamp(MIN_COLS, MAX_COLS);
        self.rows = self.rows.clamp(MIN_ROWS, MAX_ROWS);
        self.pages.truncate(MAX_PAGES);
        for (i, page) in self.pages.iter_mut().enumerate() {
            page.name = clean_name(&page.name).unwrap_or_else(|| format!("Page {}", i + 1));
            page.category = page.category.take().filter(|c| is_library(c));
            if page.category.is_some() {
                page.slots.clear();
            }
            page.slots.truncate(MAX_SLOTS);
            while page.slots.last() == Some(&None) {
                page.slots.pop();
            }
        }
    }

    /// Takes the layout of `other` (a project's, T-286). Memory only;
    /// `resolve` must follow.
    pub fn set_config(&mut self, other: &CueGrid) {
        self.set_config_with(other, &crate::presets::catalog());
    }

    /// `set_config` with the built-in catalogue made beforehand (a project
    /// open makes it with the studio lock released, T-488).
    pub fn set_config_with(&mut self, other: &CueGrid, catalog: &[Preset]) {
        self.cols = other.cols;
        self.rows = other.rows;
        self.pages = other.pages.clone();
        self.late_pages = other.late_pages.clone();
        self.sanitize();
        self.add_late_pages(catalog);
    }

    /// The operator's own pages get the catalogue pages added since they
    /// were made (« Festival », T-110), at the end, once. Default pages
    /// already have them. Memory only; the next save keeps them.
    pub fn add_late_pages(&mut self, presets: &[Preset]) {
        if self.pages.is_empty() {
            return;
        }
        let cap = self.capacity();
        for &category in late_categories() {
            if self.late_pages.iter().any(|c| c == category) {
                continue;
            }
            self.late_pages.push(category.to_string());
            if self.pages.iter().any(|p| p.name == category) {
                continue;
            }
            for page in category_pages(presets, category, cap) {
                if self.pages.len() < MAX_PAGES {
                    self.pages.push(page);
                }
            }
        }
    }

    /// The pages as shown, from the last `resolve`.
    pub fn view(&self) -> &[PageView] {
        &self.view
    }

    pub fn page_count(&self) -> usize {
        self.view.len().max(1)
    }

    /// The cue in cell `slot` (row-major) of page `page`, 0-based.
    pub fn slot(&self, page: usize, slot: usize) -> Option<&str> {
        self.view.get(page)?.cells.get(slot)?.as_deref()
    }

    /// The cue at `row`, `col` (1-based) of page `page` (1-based), as in
    /// the control id `grid.<page>.<row>.<col>`.
    pub fn cell(&self, page: usize, row: usize, col: usize) -> Option<&str> {
        let (cols, rows) = (self.cols as usize, self.rows as usize);
        if !(1..=rows).contains(&row) || !(1..=cols).contains(&col) {
            return None;
        }
        self.slot(page.checked_sub(1)?, (row - 1) * cols + (col - 1))
    }

    /// Rebuilds the pages as shown from the cues that exist now. Called
    /// when the cues (figures, ILDA files) or the layout change.
    pub fn resolve(&mut self, presets: &[Preset]) {
        let cap = self.capacity();
        let known: HashMap<&str, &Preset> = presets.iter().map(|p| (p.id.as_str(), p)).collect();
        let defaults;
        let pages = if self.pages.is_empty() {
            defaults = default_pages(presets, cap);
            &defaults
        } else {
            &self.pages
        };
        let mut view = Vec::new();
        let mut extra = Vec::new();
        for page in pages {
            if let Some(category) = &page.category {
                let ids: Vec<Option<String>> = presets.iter().filter(|p| p.category == category).map(|p| Some(p.id.clone())).collect();
                let mut chunks = ids.chunks(cap);
                view.push(page_view(&page.name, chunks.next().unwrap_or_default(), cap, 0, true, true));
                for (n, chunk) in chunks.enumerate() {
                    extra.push(page_view(&format!("{} {}", page.name, n + 2), chunk, cap, 0, true, false));
                }
            } else {
                let cells: Vec<Option<String>> = page
                    .slots
                    .iter()
                    .map(|s| s.as_ref().filter(|r| known.contains_key(r.id())).map(|r| r.id().to_string()))
                    .collect();
                let hidden = page.slots.iter().skip(cap).filter(|s| s.is_some()).count();
                let mut v = page_view(&page.name, &cells[..cells.len().min(cap)], cap, hidden, false, true);
                v.user = page.user;
                view.push(v);
            }
        }
        view.extend(extra);
        self.view = view;
        self.rev += 1;
    }

    /// The default pages become the operator's own, before an edit.
    pub fn materialize(&mut self, presets: &[Preset]) {
        if self.pages.is_empty() {
            self.pages = default_pages(presets, self.capacity());
            self.late_pages = late_categories().iter().map(|c| c.to_string()).collect();
        }
    }

    /// « Taille de grille ». Default pages are rebuilt for the new size;
    /// the operator's pages keep every cue in its slot (some may be hidden).
    pub fn set_size(&mut self, cols: u8, rows: u8) -> Result<()> {
        if !(MIN_COLS..=MAX_COLS).contains(&cols) || !(MIN_ROWS..=MAX_ROWS).contains(&rows) {
            bail!("taille de grille hors limites ({MIN_COLS}–{MAX_COLS} colonnes, {MIN_ROWS}–{MAX_ROWS} rangées)");
        }
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// « Nouvelle page »: an empty page at the end of the operator's pages.
    /// Returns its index.
    pub fn new_page(&mut self, name: &str, presets: &[Preset]) -> Result<usize> {
        self.materialize(presets);
        if self.pages.len() >= MAX_PAGES {
            bail!("{MAX_PAGES} pages au plus");
        }
        let name = clean_name(name).unwrap_or_else(|| format!("Page {}", self.pages.len() + 1));
        self.pages.push(CuePage { name, user: true, ..CuePage::default() });
        Ok(self.pages.len() - 1)
    }

    /// The first empty cell of one of the operator's pages, within the grid.
    /// Call after `materialize`.
    pub fn first_empty(&self, page: usize) -> Option<usize> {
        let p = self.pages.get(page)?;
        (0..self.capacity()).find(|&i| p.slots.get(i).is_none_or(|s| s.is_none()))
    }

    /// Puts `cue` in a cell of one of the operator's pages (T-405). Call
    /// after `materialize`.
    pub fn put(&mut self, page: usize, slot: usize, cue: Option<CueRef>) -> Result<()> {
        let cap = self.capacity();
        let p = self.pages.get_mut(page).ok_or_else(|| anyhow::anyhow!("page inconnue"))?;
        if p.category.is_some() {
            bail!("la page « {} » suit sa bibliothèque", p.name);
        }
        if slot >= cap {
            bail!("case hors de la grille");
        }
        set_slot(p, slot, cue);
        Ok(())
    }

    /// Whether a cell of the operator's pages holds `id` (hidden ones too).
    pub fn references(&self, id: &str) -> bool {
        self.pages.iter().flat_map(|p| &p.slots).flatten().any(|r| r.id() == id)
    }

    /// « Renommer la page ».
    pub fn rename_page(&mut self, page: usize, name: &str, presets: &[Preset]) -> Result<()> {
        let name = clean_name(name).ok_or_else(|| anyhow::anyhow!("nom de page vide"))?;
        self.materialize(presets);
        self.pages.get_mut(page).ok_or_else(|| anyhow::anyhow!("page inconnue"))?.name = name;
        Ok(())
    }

    /// Deletes a page; the last one stays.
    pub fn delete_page(&mut self, page: usize, presets: &[Preset]) -> Result<()> {
        self.materialize(presets);
        if page >= self.pages.len() {
            bail!("page inconnue");
        }
        if self.pages.len() == 1 {
            bail!("la dernière page reste");
        }
        self.pages.remove(page);
        Ok(())
    }

    /// Drag and drop: the cue in `from` (page, slot) goes to `to`; `None`
    /// as the slot = the page's first empty cell. Onto a cue of the same
    /// kind of page, the two swap. From a library page it is a copy (the
    /// library page keeps its item); onto a library page is refused.
    pub fn move_cell(&mut self, from: (usize, usize), to: (usize, Option<usize>), presets: &[Preset]) -> Result<()> {
        self.materialize(presets);
        let cap = self.capacity();
        let (fp, fs) = from;
        let (tp, ts) = to;
        let src = self.pages.get(fp).ok_or_else(|| anyhow::anyhow!("page inconnue"))?;
        let dst = self.pages.get(tp).ok_or_else(|| anyhow::anyhow!("page inconnue"))?;
        if dst.category.is_some() {
            bail!("la page « {} » suit sa bibliothèque : on ne peut pas y déposer de cue", dst.name);
        }
        let copy = src.category.is_some();
        let cue = if copy {
            let id = self.view.get(fp).and_then(|v| v.cells.get(fs)).cloned().flatten().ok_or_else(|| anyhow::anyhow!("case vide"))?;
            let p = presets.iter().find(|p| p.id == id).ok_or_else(|| anyhow::anyhow!("cue inconnu"))?;
            CueRef::of(p)
        } else {
            src.slots.get(fs).cloned().flatten().ok_or_else(|| anyhow::anyhow!("case vide"))?
        };
        let ts = match ts {
            Some(t) if t < cap => t,
            Some(_) => bail!("case hors de la grille"),
            None => (0..cap).find(|&i| dst.slots.get(i).is_none_or(|s| s.is_none())).ok_or_else(|| anyhow::anyhow!("page « {} » pleine", dst.name))?,
        };
        if (fp, fs) == (tp, ts) {
            return Ok(());
        }
        let target = self.pages[tp].slots.get(ts).cloned().flatten();
        if copy && target.is_some() {
            bail!("case occupée");
        }
        if !copy {
            set_slot(&mut self.pages[fp], fs, target);
        }
        set_slot(&mut self.pages[tp], ts, Some(cue));
        Ok(())
    }

    /// Empties a cell of one of the operator's pages.
    pub fn clear_cell(&mut self, page: usize, slot: usize, presets: &[Preset]) -> Result<()> {
        self.materialize(presets);
        let p = self.pages.get_mut(page).ok_or_else(|| anyhow::anyhow!("page inconnue"))?;
        if p.category.is_some() {
            bail!("la page « {} » suit sa bibliothèque", p.name);
        }
        set_slot(p, slot, None);
        Ok(())
    }
}

fn set_slot(page: &mut CuePage, slot: usize, cue: Option<CueRef>) {
    if page.slots.len() <= slot {
        page.slots.resize(slot + 1, None);
    }
    page.slots[slot] = cue;
    while page.slots.last() == Some(&None) {
        page.slots.pop();
    }
}

fn page_view(name: &str, cells: &[Option<String>], cap: usize, hidden: usize, auto: bool, editable: bool) -> PageView {
    let mut cells = cells.to_vec();
    cells.resize(cap, None);
    PageView { name: name.to_string(), cells, hidden, auto, editable, user: false }
}

fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

/// One page per category, in order, filled with its cues; what doesn't fit
/// goes on pages after the last category, so the first pages keep their
/// numbers. Library categories follow their library.
pub fn default_pages(presets: &[Preset], cap: usize) -> Vec<CuePage> {
    let (early, late) = CATEGORIES.split_at(CATEGORIES.len() - LATE_PAGES);
    let mut pages = Vec::new();
    let mut extra = Vec::new();
    for &category in early {
        let mut own = category_pages(presets, category, cap).into_iter();
        pages.extend(own.next());
        extra.extend(own);
    }
    pages.extend(extra);
    // Pages added later, each with its own overflow right after it.
    for &category in late {
        pages.extend(category_pages(presets, category, cap));
    }
    pages
}

/// The catalogue pages added later (`presets::LATE_PAGES`).
fn late_categories() -> &'static [&'static str] {
    &CATEGORIES[CATEGORIES.len() - LATE_PAGES..]
}

/// A category's pages: its first page, then « <name> 2 »… for the cues
/// that don't fit. A library category is one page that follows it.
fn category_pages(presets: &[Preset], category: &str, cap: usize) -> Vec<CuePage> {
    if is_library(category) {
        return vec![CuePage { name: category.to_string(), slots: Vec::new(), category: Some(category.to_string()), user: false }];
    }
    let cues: Vec<Option<CueRef>> = presets.iter().filter(|p| p.category == category).map(|p| Some(CueRef::of(p))).collect();
    let mut chunks = cues.chunks(cap.max(1));
    let mut pages = vec![CuePage { name: category.to_string(), slots: chunks.next().unwrap_or_default().to_vec(), category: None, user: false }];
    for (n, chunk) in chunks.enumerate() {
        pages.push(CuePage { name: format!("{category} {}", n + 2), slots: chunk.to_vec(), category: None, user: false });
    }
    pages
}

/// After the cues or the layout changed: the pages as shown, the grid
/// controls (`grid.<page>.<row>.<col>`, `page.N`) and the current page.
pub fn refresh(s: &mut crate::Shared) {
    s.grid.resolve(&s.presets);
    s.controls = crate::controls::ControlRegistry::build_for(&s.presets, &s.grid);
    s.controls.add_macros(s.macros.list());
    s.cue_page = s.cue_page.min(s.grid.page_count() - 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::catalog;

    fn resolved(grid: &mut CueGrid) -> &CueGrid {
        grid.resolve(&catalog());
        grid
    }

    fn of(cat: &str) -> Vec<String> {
        catalog().into_iter().filter(|p| p.category == cat).map(|p| p.id).collect()
    }

    #[test]
    fn defaults_are_8_by_5_and_an_empty_file_loads_them() {
        let g: CueGrid = serde_json::from_str("{}").unwrap();
        assert_eq!((g.cols, g.rows, g.pages.len()), (8, 5, 0));
        assert_eq!(g, CueGrid::default());
        // Saved by an older studio or by hand: missing fields take their defaults.
        let g: CueGrid = serde_json::from_str(r#"{"pages":[{"name":"A","slots":[{"builtin":"tunnels-001"},null]}]}"#).unwrap();
        assert_eq!((g.cols, g.rows), (8, 5));
        assert_eq!(g.pages[0].slots, vec![Some(CueRef::Builtin("tunnels-001".into())), None]);
    }

    #[test]
    fn round_trips_and_sanitizes() {
        let mut g = CueGrid { cols: 30, rows: 0, ..CueGrid::default() };
        g.pages = vec![CuePage { name: "  ".into(), slots: vec![Some(CueRef::User("figure:Logo".into())), None, None], category: None, user: false }];
        g.sanitize();
        assert_eq!((g.cols, g.rows), (MAX_COLS, MIN_ROWS));
        assert_eq!(g.pages[0].name, "Page 1");
        assert_eq!(g.pages[0].slots.len(), 1, "trailing empty cells are trimmed");
        let back: CueGrid = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        assert!(serde_json::to_string(&g).unwrap().contains(r#"{"user":"figure:Logo"}"#));
    }

    #[test]
    fn default_pages_follow_the_categories_in_order() {
        let mut g = CueGrid::default();
        let g = resolved(&mut g);
        let view = g.view();
        // The category pages keep their numbers (MIDI mappings, page.N).
        let early = CATEGORIES.len() - LATE_PAGES;
        for (i, cat) in CATEGORIES[..early].iter().enumerate() {
            assert_eq!(view[i].name, *cat);
            assert_eq!(view[i].cells.len(), 40);
        }
        let abstraits = of("Abstraits");
        assert!(abstraits.len() > 40, "the test needs an overflowing page");
        assert_eq!(view[0].cells.iter().flatten().cloned().collect::<Vec<_>>(), abstraits[..40]);
        // The rest comes after the last category, in order.
        let more = view.iter().position(|v| v.name == "Abstraits 2").unwrap();
        assert!(more >= early);
        assert_eq!(view[more].cells.iter().flatten().cloned().collect::<Vec<_>>(), abstraits[40..]);
        // A short page: its cues first, then empty cells.
        let tunnels = of("Tunnels");
        assert_eq!(view[1].cells[..tunnels.len()].iter().flatten().cloned().collect::<Vec<_>>(), tunnels);
        assert!(view[1].cells[tunnels.len()..].iter().all(Option::is_none));
        // Library pages are there, empty, and follow their library.
        let figures = CATEGORIES.iter().position(|c| *c == crate::figures::CATEGORY).unwrap();
        assert!(view[figures].auto && view[figures].cells.iter().all(Option::is_none));
        assert_eq!(g.cell(2, 1, 1), Some(tunnels[0].as_str()));
        assert_eq!(g.cell(1, 2, 3), Some(abstraits[10].as_str()));
        assert_eq!(g.cell(1, 6, 1), None);
        assert_eq!(g.cell(1, 1, 9), None);
        assert_eq!(g.cell(0, 1, 1), None);
    }

    #[test]
    fn the_festival_page_comes_after_every_older_page() {
        let mut g = CueGrid::default();
        let names: Vec<String> = resolved(&mut g).view().iter().map(|v| v.name.clone()).collect();
        // Pages 1 to 12 are what they were before T-110 (page.N, F-keys,
        // MIDI, DMX and APC page mappings); « Festival » is page 13.
        let before = ["Abstraits", "Tunnels", "Faisceaux", "Balayages", "Vagues", "Géométrie", "Audio", "Texte & horloge", "Figures", "ILDA", "Abstraits 2", "Faisceaux 2"];
        assert_eq!(names[..before.len()], before);
        assert_eq!(names[before.len()], crate::presets::FESTIVAL);
        assert_eq!(g.view()[before.len()].cells.iter().flatten().cloned().collect::<Vec<_>>(), of(crate::presets::FESTIVAL));
        assert_eq!(g.cell(13, 1, 1), Some("festival-001"));
        // « Festival évolutifs » (T-112 to T-123) came after it: page 14.
        assert_eq!(names[before.len() + 1], crate::presets::EVOLVING);
        assert_eq!(g.cell(14, 1, 1), Some("festival-evolutifs-001"));
        assert_eq!(names.len(), before.len() + 2);
    }

    #[test]
    fn the_operators_pages_get_the_festival_page_once() {
        // Pages made before T-110 (no « Festival », no `late_pages`).
        let dir = std::env::temp_dir().join(format!("studio-grid-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pages.json");
        let old = r#"{"cols":8,"rows":5,"pages":[{"name":"Mes cues","slots":[{"builtin":"tunnels-001"}]},{"name":"Figures","category":"Figures"}]}"#;
        std::fs::write(&path, old).unwrap();
        let g = CueGrid::load(path.clone());
        let names: Vec<&str> = g.pages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Mes cues", "Figures", crate::presets::FESTIVAL, crate::presets::EVOLVING], "appended, the others keep their numbers");
        assert_eq!((g.pages[2].slots.len(), g.pages[3].slots.len()), (40, 12));
        // Saved, then loaded again: still once.
        g.save();
        let mut g = CueGrid::load(path.clone());
        assert_eq!(g.pages.len(), 4);
        // Deleted by the operator: it stays deleted.
        g.delete_page(2, &catalog()).unwrap();
        g.save();
        assert_eq!(CueGrid::load(path.clone()).pages.len(), 3);
        // Pages that already got « Festival » (T-110) get only the later page.
        let festival = r#"{"cols":8,"rows":5,"pages":[{"name":"Mes cues","slots":[{"builtin":"tunnels-001"}]}],"late_pages":["Festival"]}"#;
        std::fs::write(&path, festival).unwrap();
        let names: Vec<String> = CueGrid::load(path.clone()).pages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Mes cues", crate::presets::EVOLVING]);
        // The default grid has it already, and doesn't get it twice once edited.
        let mut d = CueGrid::default();
        d.materialize(&catalog());
        let n = d.pages.len();
        d.add_late_pages(&catalog());
        assert_eq!(d.pages.len(), n);
        assert_eq!(d.pages.iter().filter(|p| p.name == crate::presets::FESTIVAL).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_6_by_4_grid_keeps_the_cues_in_order() {
        // Default pages: rebuilt for the new size, nothing lost.
        let mut g = CueGrid::default();
        g.set_size(6, 4).unwrap();
        g.resolve(&catalog());
        let shown: Vec<String> = g.view().iter().flat_map(|v| v.cells.iter().flatten().cloned()).collect();
        let abstraits = of("Abstraits");
        assert_eq!(g.view()[0].cells.len(), 24);
        assert_eq!(g.view()[0].cells.iter().flatten().cloned().collect::<Vec<_>>(), abstraits[..24]);
        assert!(abstraits.iter().all(|id| shown.contains(id)));
        assert!(g.set_size(13, 4).is_err() && g.set_size(6, 1).is_err());

        // The operator's pages: same order, the end hidden, back when it grows.
        let mut g = CueGrid::default();
        let presets = catalog();
        g.new_page("Mes cues", &presets).unwrap();
        g.set_size(6, 4).unwrap();
        g.resolve(&presets);
        assert_eq!(g.view()[0].cells.iter().flatten().cloned().collect::<Vec<_>>(), abstraits[..24]);
        assert_eq!(g.view()[0].hidden, 16);
        g.set_size(8, 5).unwrap();
        g.resolve(&presets);
        assert_eq!(g.view()[0].cells.iter().flatten().cloned().collect::<Vec<_>>(), abstraits[..40]);
        assert_eq!(g.view()[0].hidden, 0);
    }

    #[test]
    fn edits_turn_the_defaults_into_pages_of_their_own() {
        let presets = catalog();
        let mut g = CueGrid::default();
        g.resolve(&presets);
        let before = g.view().to_vec();
        let n = g.new_page("", &presets).unwrap();
        assert_eq!(n, before.len());
        g.resolve(&presets);
        // Same pages as before, plus the new empty one.
        assert_eq!(g.view()[..n], before[..]);
        assert_eq!(g.view()[n].name, format!("Page {}", n + 1));
        assert!(g.view()[n].cells.iter().all(Option::is_none));

        g.rename_page(n, "Final", &presets).unwrap();
        assert!(g.rename_page(n, "  ", &presets).is_err());
        // Tunnels #1 to the new page's first empty cell, then swap within Tunnels.
        let tunnels = of("Tunnels");
        g.move_cell((1, 0), (n, None), &presets).unwrap();
        g.move_cell((1, 1), (1, Some(0)), &presets).unwrap();
        g.resolve(&presets);
        assert_eq!(g.slot(n, 0), Some(tunnels[0].as_str()));
        assert_eq!(g.slot(1, 0), Some(tunnels[1].as_str()));
        assert_eq!(g.slot(1, 1), None);
        assert!(g.move_cell((1, 1), (n, None), &presets).is_err(), "empty cell");
        assert!(g.move_cell((1, 0), (n, Some(40)), &presets).is_err(), "outside the grid");
        // Onto a library page: refused.
        let figures = CATEGORIES.iter().position(|c| *c == crate::figures::CATEGORY).unwrap();
        assert!(g.move_cell((1, 0), (figures, None), &presets).is_err());
        g.clear_cell(n, 0, &presets).unwrap();
        g.resolve(&presets);
        assert_eq!(g.slot(n, 0), None);
        g.delete_page(n, &presets).unwrap();
        assert!(g.delete_page(999, &presets).is_err());
    }

    #[test]
    fn library_pages_follow_their_items_and_copy_out() {
        let mut presets = catalog();
        let mut g = CueGrid::default();
        let figures = CATEGORIES.iter().position(|c| *c == crate::figures::CATEGORY).unwrap();
        for i in 0..45 {
            presets.push(Preset { id: format!("figure:F{i:02}"), name: format!("F{i:02}"), category: crate::figures::CATEGORY, settings: Default::default(), tag: None });
        }
        g.resolve(&presets);
        assert_eq!(g.slot(figures, 0), Some("figure:F00"));
        // The rest of a long library: an extra page at the very end.
        let last = g.view().last().unwrap();
        assert_eq!((last.name.as_str(), last.editable), ("Figures 2", false));
        assert_eq!(last.cells.iter().flatten().count(), 5);
        // Dragging a figure out copies it, as a user cue.
        let mine = g.new_page("Mix", &presets).unwrap();
        g.move_cell((figures, 2), (mine, Some(3)), &presets).unwrap();
        assert_eq!(g.pages[mine].slots[3], Some(CueRef::User("figure:F02".into())));
        g.resolve(&presets);
        assert_eq!(g.slot(figures, 2), Some("figure:F02"));
        // A figure that disappears leaves an empty cell (the slot stays).
        presets.retain(|p| p.id != "figure:F02");
        g.resolve(&presets);
        assert_eq!(g.slot(mine, 3), None);
        assert_eq!(g.pages[mine].slots[3], Some(CueRef::User("figure:F02".into())));
    }

    #[test]
    fn refresh_keeps_the_controls_and_the_page_in_range() {
        let mut s = crate::test_support::shared();
        let tunnels = of("Tunnels");
        assert_eq!(crate::controls::grid_cell_preset(&s.grid, "2.1.1").as_deref(), Some(tunnels[0].as_str()));
        s.cue_page = 500;
        refresh(&mut s);
        assert_eq!(s.cue_page, s.grid.page_count() - 1);
        // A smaller grid: cells past it are gone from the controls.
        s.grid.set_size(4, 2).unwrap();
        refresh(&mut s);
        assert!(s.controls.get("grid.2.1.4").is_some());
        assert!(s.controls.get("grid.2.1.5").is_none());
        assert!(s.controls.get("grid.2.3.1").is_none());
    }
}
