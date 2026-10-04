//! Scan presets (T-415): named sets of optimiser settings (T-002) that a
//! cue of the grid, a figure or a timeline event picks for its own
//! rendering. Beams, graphics and rasters don't want the same tracing:
//! beams few points and firm stops at their ends, graphics a fine step and
//! crisp corners, a raster an even step with no corner stops.
//!
//! The three built-in presets are ours, measured on the preview (see
//! `builtin_scan_presets`), not taken from any other software. Nothing
//! chosen anywhere = « Auto »: the output's optimiser settings as they are
//! (`Mixer::optimizer`), so looks saved before T-415 draw exactly as before.
//!
//! A preset only changes how a look is traced, each look on its own,
//! **before** the layer mixer: the point budget, the zones, the
//! calibration and the whole safety stage (static-beam guard included)
//! still run after, on the finished frame. Travel stays dark whatever the
//! preset (`optimize::travel`). A preset is bounded by the output's scan
//! limits (`ScanLimits`), never the other way round.

use crate::engine::{Content, Settings};
use crate::layers::{Mixer, MAX_POINT_BUDGET, MIN_POINT_BUDGET};
use crate::optimize::OptimizeParams;
use crate::timeline::{EventSource, TimelineCue};
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const BEAMS: &str = "beams";
pub const GRAPHICS: &str = "graphics";
pub const RASTER: &str = "raster";

/// Scan speed range of a preset, in % of the preset's own steps.
pub const MIN_SCAN_PCT: f32 = 50.0;
pub const MAX_SCAN_PCT: f32 = 150.0;

pub const MAX_USER_PRESETS: usize = 32;
const MAX_NAME_CHARS: usize = 40;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanPreset {
    /// `beams` | `graphics` | `raster` | `u-<n>` (user presets).
    pub id: String,
    pub name: String,
    pub builtin: bool,
    /// The optimiser's settings (`enabled` is ignored: a preset always
    /// uses the optimiser).
    pub params: OptimizeParams,
    /// Scan speed, % (50..150): scales the lit and blanked steps, so 150 %
    /// draws the same picture with a third fewer points, faster.
    pub scan_pct: f32,
}

impl Default for ScanPreset {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), builtin: false, params: OptimizeParams { enabled: true, ..Default::default() }, scan_pct: 100.0 }
    }
}

impl ScanPreset {
    /// The optimiser settings this preset draws with: its steps scaled by
    /// its scan speed, sanitised. Call on an `effective` preset.
    pub fn optimize_params(&self) -> OptimizeParams {
        let k = if self.scan_pct.is_finite() { self.scan_pct.clamp(MIN_SCAN_PCT, MAX_SCAN_PCT) / 100.0 } else { 1.0 };
        let mut p = self.params.clone();
        p.sanitize();
        p.enabled = true;
        p.lit_step *= k;
        p.blank_step *= k;
        p.sanitize();
        p
    }
}

/// The three built-in presets. Values measured on the preview with our own
/// generators (a square, a star, the beam fans): *Faisceaux* about 40 %
/// fewer points than *Graphismes* on the same outline, *Trame* no stop at
/// corners. *Graphismes* is T-002's default.
pub fn builtin_scan_presets() -> Vec<ScanPreset> {
    let d = OptimizeParams { enabled: true, ..Default::default() };
    vec![
        ScanPreset {
            id: BEAMS.into(),
            name: "Faisceaux".into(),
            builtin: true,
            params: OptimizeParams {
                lit_step: 0.05,
                blank_step: 0.2,
                corner_min_deg: 45.0,
                max_corner_dwell: 4,
                endpoint_dwell: 6,
                max_points: 600,
                ..d.clone()
            },
            scan_pct: 100.0,
        },
        ScanPreset { id: GRAPHICS.into(), name: "Graphismes".into(), builtin: true, params: d.clone(), scan_pct: 100.0 },
        ScanPreset {
            id: RASTER.into(),
            name: "Trame".into(),
            builtin: true,
            params: OptimizeParams {
                lit_step: 0.02,
                blank_step: 0.1,
                corner_min_deg: 180.0,
                max_corner_dwell: 0,
                endpoint_dwell: 1,
                // A raster is drawn in its own line order.
                reorder: false,
                ..d
            },
            scan_pct: 100.0,
        },
    ]
}

/// What the output allows a preset (T-171 will give every output its
/// own; until then the same for all): the fastest scan and the range of
/// the per-look point cap.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ScanLimits {
    pub max_scan_pct: f32,
    pub min_points: usize,
    pub max_points: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self { max_scan_pct: MAX_SCAN_PCT, min_points: MIN_POINT_BUDGET, max_points: MAX_POINT_BUDGET }
    }
}

/// `preset` within `limits`: the scan speed no faster than the output
/// allows (and within 50..150 %), the point cap within its range, every
/// optimiser value sanitised. What is drawn, and what the editor shows.
pub fn effective(preset: &ScanPreset, limits: &ScanLimits) -> ScanPreset {
    let mut p = preset.clone();
    p.params.sanitize();
    p.params.enabled = true;
    let max_pct = if limits.max_scan_pct.is_finite() { limits.max_scan_pct.clamp(MIN_SCAN_PCT, MAX_SCAN_PCT) } else { 100.0 };
    p.scan_pct = if p.scan_pct.is_finite() { p.scan_pct.clamp(MIN_SCAN_PCT, max_pct) } else { 100.0f32.min(max_pct) };
    let min = limits.min_points.max(MIN_POINT_BUDGET);
    let max = limits.max_points.clamp(min, MAX_POINT_BUDGET);
    p.params.max_points = p.params.max_points.clamp(min, max);
    p
}

/// The preset a look asked for: the timeline event's, else the grid
/// cell's, else the figure's (empty = none).
pub fn chosen<'a>(event: Option<&'a str>, slot: Option<&'a str>, figure: Option<&'a str>) -> Option<&'a str> {
    [event, slot, figure].into_iter().flatten().find(|id| !id.is_empty())
}

/// The figure's own preset, when the look plays a figure.
pub fn figure_scan(look: &Settings) -> Option<&str> {
    match &look.content {
        Content::Figure(f) => f.scan.as_deref(),
        _ => None,
    }
}

/// What a look of the cue deck (or the manual look, id 0) draws with: its
/// cell's preset, else its figure's, else the output's default.
pub fn for_look(s: &Shared, id: u64, look: &Settings) -> OptimizeParams {
    let slot = s.deck.find(id).and_then(|a| s.deck.slot_scan(&a.cue));
    s.scan_presets.resolve(chosen(None, slot, figure_scan(look)), &s.mixer, &ScanLimits::default())
}

/// What a timeline event draws with: its own preset, else its cue's cell's,
/// else its figure's, else the output's default.
pub fn for_event(s: &Shared, cue: &TimelineCue, look: &Settings) -> OptimizeParams {
    let slot = match &cue.source {
        EventSource::Cue { id } => s.deck.slot_scan(id),
        EventSource::Look { .. } => None,
    };
    s.scan_presets.resolve(chosen(cue.scan.as_deref(), slot, figure_scan(look)), &s.mixer, &ScanLimits::default())
}

/// User presets, saved to `scan_presets.json` on every change.
pub struct ScanPresetStore {
    path: PathBuf,
    user: Vec<ScanPreset>,
}

impl ScanPresetStore {
    /// A missing file starts an empty store; a damaged or invalid one too,
    /// once moved to the quarantine folder (T-402).
    pub fn load_or_create(path: PathBuf) -> Self {
        let mut user: Vec<ScanPreset> = crate::load_json(&path);
        let user = match validate(&mut user) {
            Ok(()) => user,
            Err(e) => {
                crate::datafile::set_aside(&path, &crate::datafile::beside(&path), &format!("préréglages de balayage invalides : {e}"), false);
                Vec::new()
            }
        };
        Self { path, user }
    }

    pub fn user(&self) -> &[ScanPreset] {
        &self.user
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Built-in presets first, then the user's.
    pub fn all(&self) -> Vec<ScanPreset> {
        let mut all = builtin_scan_presets();
        all.extend(self.user.iter().cloned());
        all
    }

    pub fn find(&self, id: &str) -> Option<ScanPreset> {
        builtin_scan_presets().into_iter().find(|p| p.id == id).or_else(|| self.user.iter().find(|p| p.id == id).cloned())
    }

    pub fn is_known(&self, id: &str) -> bool {
        self.find(id).is_some()
    }

    /// Replaces every user preset in memory only (already validated); the
    /// caller saves the file (project open).
    pub fn replace_in_memory(&mut self, user: Vec<ScanPreset>) {
        self.user = user;
    }

    /// Replaces every user preset and saves. Built-in ones can't be
    /// changed: an id of theirs is refused.
    pub fn set(&mut self, mut user: Vec<ScanPreset>) -> Result<()> {
        validate(&mut user)?;
        let json = serde_json::to_string_pretty(&user).context("failed to serialize scan presets")?;
        crate::datafile::replace(&self.path, json.as_bytes()).with_context(|| format!("failed to write {}", self.path.display()))?;
        self.user = user;
        Ok(())
    }

    /// The optimiser settings a look draws with: the preset it chose, else
    /// the output's default preset, else (« Auto », or a preset since
    /// deleted) the output's optimiser settings as they are.
    pub fn resolve(&self, chosen: Option<&str>, mixer: &Mixer, limits: &ScanLimits) -> OptimizeParams {
        chosen
            .and_then(|id| self.find(id))
            .or_else(|| mixer.scan_default.as_deref().and_then(|id| self.find(id)))
            .map_or_else(|| mixer.optimizer.clone(), |p| effective(&p, limits).optimize_params())
    }
}

/// User presets: at most `MAX_USER_PRESETS`, ids `u-<n>` unique, names
/// 1..40 characters. Values are brought into range, `builtin` cleared.
pub fn validate(user: &mut [ScanPreset]) -> Result<()> {
    if user.len() > MAX_USER_PRESETS {
        bail!("{MAX_USER_PRESETS} préréglages de balayage utilisateur au maximum");
    }
    let mut ids = HashSet::new();
    for p in user.iter_mut() {
        let n = p.id.strip_prefix("u-").unwrap_or("");
        if n.is_empty() || n.len() > 9 || !n.bytes().all(|b| b.is_ascii_digit()) {
            bail!("identifiant de préréglage invalide : « {} » (u-<n> ; les préréglages intégrés ne se modifient pas)", p.id);
        }
        if !ids.insert(p.id.clone()) {
            bail!("préréglage de balayage en double : « {} »", p.id);
        }
        p.name = p.name.trim().to_string();
        if p.name.is_empty() || p.name.chars().count() > MAX_NAME_CHARS {
            bail!("nom de préréglage invalide : « {} »", p.name);
        }
        p.builtin = false;
        p.params.sanitize();
        p.params.enabled = true;
        p.scan_pct = if p.scan_pct.is_finite() { p.scan_pct.clamp(MIN_SCAN_PCT, MAX_SCAN_PCT) } else { 100.0 };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::Optimizer;
    use crate::patterns::{self, Point};

    fn store(user: Vec<ScanPreset>) -> ScanPresetStore {
        ScanPresetStore { path: PathBuf::new(), user }
    }

    fn preset(id: &str) -> ScanPreset {
        builtin_scan_presets().into_iter().find(|p| p.id == id).unwrap()
    }

    fn draw(id: &str, pts: &[Point]) -> Vec<Point> {
        Optimizer::new(effective(&preset(id), &ScanLimits::default()).optimize_params()).optimize(pts)
    }

    /// Runs of identical consecutive points longer than 1.
    fn holds(points: &[Point]) -> Vec<usize> {
        let mut r: Vec<(Point, usize)> = Vec::new();
        for &q in points {
            match r.last_mut() {
                Some((p, n)) if *p == q => *n += 1,
                _ => r.push((q, 1)),
            }
        }
        r.into_iter().map(|x| x.1).filter(|&n| n > 1).collect()
    }

    #[test]
    fn builtins_order_point_counts_and_raster_has_no_corner_stop() {
        for shape in [patterns::square(0.5, 1.0, 1.0, 1.0), patterns::by_name("star", 0.8, 1.0, 1.0, 1.0).unwrap()] {
            let (beams, graphics, raster) = (draw(BEAMS, &shape), draw(GRAPHICS, &shape), draw(RASTER, &shape));
            assert!(beams.len() < graphics.len(), "beams {} vs graphics {}", beams.len(), graphics.len());
            // Graphics = T-002's default settings.
            assert_eq!(graphics, Optimizer::new(OptimizeParams { enabled: true, ..Default::default() }).optimize(&shape));
            // Raster: the ends' single extra point, no corner held.
            let h = holds(&raster);
            assert!(h.iter().all(|&n| n <= 2) && h.len() <= 2, "{h:?}");
            assert!(holds(&graphics).iter().filter(|&&n| n > 2).count() >= 4);
        }
        // Beams hold their ends longer.
        let line = [Point::lit(-0.5, 0.0, 1.0, 1.0, 1.0), Point::lit(0.5, 0.0, 1.0, 1.0, 1.0)];
        assert!(holds(&draw(BEAMS, &line))[0] > holds(&draw(GRAPHICS, &line))[0]);
    }

    #[test]
    fn effective_clamps_scan_speed_and_points_to_the_output_limits() {
        let fast = ScanPreset { scan_pct: 150.0, params: OptimizeParams { max_points: 50, ..Default::default() }, ..preset(GRAPHICS) };
        let limits = ScanLimits { max_scan_pct: 110.0, min_points: 300, max_points: 800 };
        let e = effective(&fast, &limits);
        assert_eq!(e.scan_pct, 110.0);
        assert_eq!(e.params.max_points, 300);
        let big = ScanPreset { params: OptimizeParams { max_points: 5000, ..Default::default() }, ..fast.clone() };
        assert_eq!(effective(&big, &limits).params.max_points, 800);
        // Nonsense never gets through.
        let nan = ScanPreset { scan_pct: f32::NAN, params: OptimizeParams { lit_step: f32::INFINITY, ..Default::default() }, ..fast };
        let e = effective(&nan, &ScanLimits::default());
        assert_eq!(e.scan_pct, 100.0);
        assert!(e.params.enabled && e.params.lit_step <= 0.2);
        // A faster scan = longer steps = fewer points, within sanitize's range.
        let sq = patterns::square(0.5, 1.0, 1.0, 1.0);
        let at = |pct: f32| Optimizer::new(ScanPreset { scan_pct: pct, ..preset(GRAPHICS) }.optimize_params()).optimize(&sq).len();
        assert!(at(150.0) < at(100.0) && at(100.0) < at(50.0));
    }

    #[test]
    fn priority_is_event_then_cell_then_figure_then_the_output_default() {
        assert_eq!(chosen(Some("raster"), Some("beams"), Some("graphics")), Some("raster"));
        assert_eq!(chosen(None, Some("beams"), Some("graphics")), Some("beams"));
        assert_eq!(chosen(Some(""), None, Some("graphics")), Some("graphics"));
        assert_eq!(chosen(None, None, None), None);

        let s = store(vec![ScanPreset { id: "u-1".into(), name: "Mien".into(), scan_pct: 80.0, ..Default::default() }]);
        let limits = ScanLimits::default();
        let auto = Mixer::default();
        // Nothing chosen, no output default: « Auto » = the mixer's optimiser (off).
        assert_eq!(s.resolve(None, &auto, &limits), auto.optimizer);
        assert!(!s.resolve(None, &auto, &limits).enabled);
        let beams = effective(&preset(BEAMS), &limits).optimize_params();
        assert_eq!(s.resolve(Some(BEAMS), &auto, &limits), beams);
        assert_eq!(s.resolve(Some("u-1"), &auto, &limits).lit_step, 0.025 * 0.8);
        // An unknown (deleted) preset falls back to the output default.
        let with_default = Mixer { scan_default: Some(RASTER.into()), ..Mixer::default() };
        assert_eq!(s.resolve(Some("u-9"), &with_default, &limits), effective(&preset(RASTER), &limits).optimize_params());
        assert_eq!(s.resolve(Some("u-9"), &auto, &limits), auto.optimizer);
        // A deleted output default = Auto.
        let gone = Mixer { scan_default: Some("u-7".into()), ..Mixer::default() };
        assert_eq!(s.resolve(None, &gone, &limits), gone.optimizer);
    }

    #[test]
    fn builtins_cannot_be_changed_or_deleted_and_user_presets_are_checked() {
        let dir = std::env::temp_dir().join(format!("ls-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = ScanPresetStore::load_or_create(dir.join("scan_presets.json"));
        assert!(s.user().is_empty());
        // A built-in id (or any non-user id) is refused, so it can't be overridden.
        let hijack = ScanPreset { name: "Faisceaux".into(), params: OptimizeParams { lit_step: 0.2, ..Default::default() }, ..preset(BEAMS) };
        assert!(s.set(vec![hijack]).is_err());
        assert_eq!(s.find(BEAMS), Some(preset(BEAMS)));
        let dup = ScanPreset { id: "u-1".into(), name: "A".into(), ..Default::default() };
        assert!(s.set(vec![dup.clone(), dup.clone()]).is_err());
        assert!(s.set(vec![ScanPreset { name: " ".into(), ..dup.clone() }]).is_err());
        // Out-of-range values are brought into range, `builtin` cleared.
        let wild = ScanPreset { builtin: true, scan_pct: 900.0, params: OptimizeParams { max_points: 0, enabled: false, ..Default::default() }, ..dup };
        s.set(vec![wild]).unwrap();
        let u = &s.user()[0];
        assert!(!u.builtin && u.scan_pct == MAX_SCAN_PCT && u.params.enabled && u.params.max_points == MIN_POINT_BUDGET);
        // Survives a restart; the built-ins are always there.
        let again = ScanPresetStore::load_or_create(dir.join("scan_presets.json"));
        assert_eq!(again.user(), s.user());
        assert_eq!(again.all().len(), 4);
        // Deleting = saving the list without it; built-ins stay.
        s.set(Vec::new()).unwrap();
        assert!(s.is_known(BEAMS) && s.is_known(GRAPHICS) && s.is_known(RASTER) && !s.is_known("u-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_layers_with_two_presets_are_traced_separately_in_one_frame() {
        use crate::engine::{Animator, AudioFeatures, BeatClock};
        let look = Settings { content: Content::Shape { shape: "square".into() }, ..Settings::default() };
        let limits = ScanLimits::default();
        let render = |id: &str| {
            let mut a = Animator::default();
            a.set_optimizer(&effective(&preset(id), &limits).optimize_params());
            a.render(&look, AudioFeatures::default(), 1.0 / 60.0, &BeatClock::default())
        };
        let (beams, graphics) = (render(BEAMS), render(GRAPHICS));
        assert!(beams.len() < graphics.len());
        let mixer = Mixer { point_budget: MAX_POINT_BUDGET, ..Mixer::default() };
        let (frame, report) = crate::layers::mix(vec![(1, beams.clone()), (2, graphics.clone())], &mixer);
        assert_eq!(report.per_layer[0], beams.len());
        assert_eq!(report.per_layer[1], graphics.len());
        // Both looks are in the frame as traced, joined by dark travel.
        assert!(frame.windows(beams.len()).any(|w| w == beams.as_slice()));
        assert!(frame.windows(graphics.len()).any(|w| w == graphics.as_slice()));
        let lit_step = effective(&preset(BEAMS), &limits).optimize_params().lit_step;
        for w in frame.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()) {
            assert!(((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt() <= lit_step + 1e-5, "lit jump {w:?}");
        }
    }

    #[test]
    fn looks_resolve_their_preset_from_the_cell_the_event_and_the_figure() {
        use crate::cues::{At, CueSlot};
        let mut s = crate::test_support::shared();
        let limits = ScanLimits::default();
        let params = |id: &str| effective(&preset(id), &limits).optimize_params();
        let fig = Settings { content: Content::Figure(crate::figures::Figure { scan: Some(RASTER.into()), ..Default::default() }), ..Settings::default() };
        // The manual look (id 0) and a cue with no preset anywhere: Auto.
        assert_eq!(for_look(&s, 0, &Settings::default()), s.mixer.optimizer);
        assert_eq!(for_look(&s, 0, &fig), params(RASTER));
        s.deck.press("c", None, At::default(), Settings::default);
        let id = s.deck.active[0].id;
        assert_eq!(for_look(&s, id, &fig), params(RASTER));
        s.deck.set_slot("c", CueSlot { scan: Some(BEAMS.into()), ..CueSlot::default() });
        assert_eq!(for_look(&s, id, &fig), params(BEAMS));
        // A timeline event: its own first, then its cue's cell.
        let mut ev = TimelineCue {
            instance: 1,
            event: 1,
            layer: 1,
            source: EventSource::Cue { id: "c".into() },
            gain: 1.0,
            content_beat: 0.0,
            frozen: false,
            modifiers: Default::default(),
            envelopes: Vec::new(),
            elapsed_s: 0.0,
            scan: None,
        };
        assert_eq!(for_event(&s, &ev, &fig), params(BEAMS));
        ev.scan = Some(GRAPHICS.into());
        assert_eq!(for_event(&s, &ev, &fig), params(GRAPHICS));
        // A preset deleted while still referenced: the output's default.
        s.deck.set_slot("c", CueSlot { scan: Some("u-4".into()), ..CueSlot::default() });
        assert_eq!(for_look(&s, id, &Settings::default()), s.mixer.optimizer);
        s.mixer.scan_default = Some(BEAMS.into());
        assert_eq!(for_look(&s, id, &Settings::default()), params(BEAMS));
    }

    #[test]
    fn files_saved_before_scan_presets_load_without_one() {
        let slot: crate::cues::CueSlot = serde_json::from_str(r#"{"layer":2}"#).unwrap();
        assert_eq!(slot.scan, None);
        assert!(!serde_json::to_string(&slot).unwrap().contains("scan"));
        let fig: crate::figures::Figure = serde_json::from_str(r#"{"name":"logo","rate":2.0}"#).unwrap();
        assert_eq!(fig.scan, None);
        assert!(!serde_json::to_string(&fig).unwrap().contains("scan"));
        let ev: crate::timeline::Event = serde_json::from_str(r#"{"id":3,"start":1.0,"len":4.0}"#).unwrap();
        assert_eq!(ev.scan, None);
        assert!(!serde_json::to_string(&ev).unwrap().contains("scan"));
        let m: Mixer = serde_json::from_str(r#"{"point_budget":750}"#).unwrap();
        assert_eq!(m.scan_default, None);
        assert!(!serde_json::to_string(&m).unwrap().contains("scan_default"));
        let slot: crate::cues::CueSlot = serde_json::from_str(r#"{"scan":"beams"}"#).unwrap();
        assert_eq!(slot.scan.as_deref(), Some(BEAMS));
    }

    #[test]
    fn a_beam_preset_does_not_get_past_the_static_beam_guard() {
        use crate::dwell::{DwellGuard, DwellProfile, DwellState, StrictArea};
        // A static beam (one lit point, held) below the horizon, traced with
        // every preset (Faisceaux holds its ends longest), then the guard.
        let beam = vec![Point::lit(0.1, -0.4, 1.0, 1.0, 1.0); 3];
        for p in builtin_scan_presets() {
            let traced = Optimizer::new(effective(&p, &ScanLimits::default()).optimize_params()).optimize(&beam);
            let cfg = DwellGuard { profile: DwellProfile::Beams, ..Default::default() };
            let mut st = DwellState::default();
            for i in 0..60 {
                let mut frame = Vec::new();
                while frame.len() < 500 {
                    frame.extend(traced.iter().copied());
                }
                let out = st.apply(frame, i as f64 / 60.0, &cfg, &StrictArea::below(0.0), true);
                assert!(out.iter().all(|q| q.r == 0.0 && q.g == 0.0 && q.b == 0.0), "{}: lit at frame {i}", p.id);
            }
        }
    }

    #[test]
    fn travel_stays_dark_with_every_preset() {
        let strokes = {
            let mut v = patterns::square(0.3, 1.0, 0.0, 0.0);
            let last = *v.last().unwrap();
            v.push(Point::blanked(last.x, last.y));
            v.push(Point::blanked(0.8, 0.8));
            v.extend(patterns::by_name("star", 0.2, 0.0, 1.0, 0.0).unwrap().into_iter().map(|p| Point { x: p.x + 0.6, y: p.y + 0.6, ..p }));
            v
        };
        for p in builtin_scan_presets() {
            let out = draw(&p.id, &strokes);
            let lit_step = effective(&p, &ScanLimits::default()).optimize_params().lit_step;
            for w in out.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()) {
                let d = ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt();
                assert!(d <= 2.0 * lit_step + 1e-5, "{}: a lit jump of {d}", p.id);
            }
        }
    }
}
