//! Static beam guard (T-256): minimum drawn size and maximum dwell on one
//! spot, measured on the frame that really goes out.
//!
//! What an eye receives depends on the size, speed and pauses of a figure,
//! not only on its brightness (docs/research/safety-regulation.md §2.2): a
//! figure shrunk to 2 %, a frozen effect or a zoom to 0 is a static beam,
//! and slow parts of a scan (corners, turning points, dwell points) are hot
//! spots. The guard runs on the finished frame (after calibration, the
//! horizon, the zones and the strobe limiter, right before the output
//! gate), so every source is covered: beam cues, figures, text, a shape shrunk
//! to a point by the live size, a frozen timeline event, a held strobe
//! frame.
//!
//! - **Minimum size.** The diagonal of the bounding box of the lit points.
//!   Below `min_extent` the whole frame is dimmed linearly, down to 0 at
//!   `min_extent / 2`.
//! - **Exposure grid.** The field is cut into `grid` × `grid` cells. Each
//!   frame adds, per cell, the share of the scan time spent lit in it
//!   (brightness × the fraction of the frame's samples in the cell × the
//!   frame's duration). Over a sliding `window_ms` window this gives the
//!   cell's **dose**: 1.0 = a full-brightness beam held on that cell for
//!   the whole window. A cell over its cap is held at the cap, and faded
//!   out between 1.5× and 2× the cap (off beyond). This is also the
//!   minimum scan speed: a beam slower than about one cell per
//!   `cap × window` concentrates more than the cap in a cell.
//! - **Profiles.** `Strict`: size check and `max_cell_dose` (0.25)
//!   everywhere. `Beams` (default): static beams above the horizon are
//!   allowed but capped at `max_cell_dose_beams` (0.6); below the horizon,
//!   where the audience is, the strict rules apply (T-255 will add real
//!   audience zones). `Off`: no effect, **only while the output is
//!   disarmed** (preview); armed, it acts as `Beams`.
//!
//! The dose is measured on the guard's input, so the attenuation does not
//! feed back into the measurement (no oscillation), and an attenuation can
//! only rise back slowly (`RISE_PER_S`), so the guard itself never makes a
//! strobe. The dose is a *relative* energy, not mW/cm² (T-257 estimates
//! absolute values).
//!
//! **This is not scan-fail protection.** A stuck galvo turns any figure
//! into a static beam, but our points do not show it: ShowNET, IDN and
//! Ether Dream give no position feedback. Only a hardware scan-fail
//! circuit in the projector can catch that.

use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DwellProfile {
    /// Size check and the strict cap everywhere.
    Strict,
    /// Static beams above the horizon allowed under a cap; strict below.
    #[default]
    Beams,
    /// No effect, and only while disarmed (armed = `Beams`).
    Off,
}

impl DwellProfile {
    /// Higher = safer.
    fn rank(self) -> u8 {
        match self {
            DwellProfile::Off => 0,
            DwellProfile::Beams => 1,
            DwellProfile::Strict => 2,
        }
    }

    fn label_fr(self) -> &'static str {
        match self {
            DwellProfile::Strict => "Stricte",
            DwellProfile::Beams => "Faisceaux",
            DwellProfile::Off => "Désactivée",
        }
    }
}

/// The guard's settings: part of the machine's `SafetySettings`
/// (`safety.json`), never of a look or a project.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DwellGuard {
    pub profile: DwellProfile,
    /// Smallest diagonal of the lit points (normalised units, field = 2).
    pub min_extent: f32,
    /// Highest dose of a cell under the strict rules (fraction of the
    /// window at full brightness).
    pub max_cell_dose: f32,
    /// Highest dose of a cell for static beams above the horizon.
    pub max_cell_dose_beams: f32,
    /// Length of the sliding window, in ms (at most the 0.25 s aversion time).
    pub window_ms: u32,
    /// Cells per side.
    pub grid: u16,
}

impl Default for DwellGuard {
    fn default() -> Self {
        Self { profile: DwellProfile::Beams, min_extent: 0.05, max_cell_dose: 0.25, max_cell_dose_beams: 0.6, window_ms: 250, grid: 32 }
    }
}

const MIN_EXTENT_RANGE: (f32, f32) = (0.02, 0.5);
const CELL_DOSE_RANGE: (f32, f32) = (0.05, 0.5);
const BEAMS_DOSE_RANGE: (f32, f32) = (0.1, 0.8);
const WINDOW_RANGE: (u32, u32) = (100, 250);
const GRID_RANGE: (u16, u16) = (16, 64);
/// How fast an attenuation may lift (factor per second): a full recovery
/// takes at least 250 ms, so the guard can't flash faster than 4 Hz.
const RISE_PER_S: f32 = 4.0;

fn fix(v: f32, (lo, hi): (f32, f32), tight: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        tight
    }
}

impl DwellGuard {
    /// Refuses values out of range (French messages for the UI).
    pub fn validate(&self) -> Result<()> {
        let check = |v: f32, (lo, hi): (f32, f32), what: &str| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("{what} : {v} hors limites ({lo} à {hi})");
            }
            Ok(())
        };
        check(self.min_extent, MIN_EXTENT_RANGE, "Taille minimum")?;
        check(self.max_cell_dose, CELL_DOSE_RANGE, "Plafond par case (strict)")?;
        check(self.max_cell_dose_beams, BEAMS_DOSE_RANGE, "Plafond par case (faisceaux)")?;
        if !(WINDOW_RANGE.0..=WINDOW_RANGE.1).contains(&self.window_ms) {
            bail!("Fenêtre d'exposition : {} ms hors limites ({} à {} ms)", self.window_ms, WINDOW_RANGE.0, WINDOW_RANGE.1);
        }
        if !(GRID_RANGE.0..=GRID_RANGE.1).contains(&self.grid) {
            bail!("Grille d'exposition : {} hors limites ({} à {})", self.grid, GRID_RANGE.0, GRID_RANGE.1);
        }
        Ok(())
    }

    /// Clamped into range, NaN → the tighter end (hand-edited files, and
    /// a copy per frame so the guard never runs on a bad value).
    pub fn sanitized(&self) -> Self {
        Self {
            profile: self.profile,
            min_extent: fix(self.min_extent, MIN_EXTENT_RANGE, MIN_EXTENT_RANGE.1),
            max_cell_dose: fix(self.max_cell_dose, CELL_DOSE_RANGE, CELL_DOSE_RANGE.0),
            max_cell_dose_beams: fix(self.max_cell_dose_beams, BEAMS_DOSE_RANGE, BEAMS_DOSE_RANGE.0),
            window_ms: self.window_ms.clamp(WINDOW_RANGE.0, WINDOW_RANGE.1),
            grid: self.grid.clamp(GRID_RANGE.0, GRID_RANGE.1),
        }
    }

    /// What going from `self` to `new` would loosen (French), for the
    /// operator's confirmation. A longer window or a coarser grid lets more
    /// light gather on one spot before it is seen, so they loosen too.
    pub fn loosenings(&self, new: &DwellGuard) -> Vec<String> {
        let mut out = Vec::new();
        if new.profile.rank() < self.profile.rank() {
            out.push(format!("Garde anti-point fixe : {} → {}", self.profile.label_fr(), new.profile.label_fr()));
        }
        if new.min_extent < self.min_extent {
            out.push(format!("Taille minimum : {:.1} % → {:.1} %", self.min_extent * 100.0, new.min_extent * 100.0));
        }
        if new.max_cell_dose > self.max_cell_dose {
            out.push(format!("Plafond par case (strict) : {:.0} % → {:.0} %", self.max_cell_dose * 100.0, new.max_cell_dose * 100.0));
        }
        if new.max_cell_dose_beams > self.max_cell_dose_beams {
            out.push(format!(
                "Plafond par case (faisceaux) : {:.0} % → {:.0} %",
                self.max_cell_dose_beams * 100.0,
                new.max_cell_dose_beams * 100.0
            ));
        }
        if new.window_ms > self.window_ms {
            out.push(format!("Fenêtre d'exposition : {} → {} ms", self.window_ms, new.window_ms));
        }
        if new.grid < self.grid {
            out.push(format!("Grille d'exposition : {} → {} cases", self.grid, new.grid));
        }
        out
    }
}

/// What the guard did to the last frame, for the UI and `/api/frame`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DwellStatus {
    /// Something is being dimmed or blanked (the « Garde active » light).
    pub active: bool,
    /// The profile in effect (`Off` armed reads `beams`).
    pub profile: DwellProfile,
    /// Diagonal of the lit points of the input frame (0 when dark).
    pub extent: f32,
    /// Brightness factor from the size check (1 = untouched).
    pub extent_factor: f32,
    /// Highest cell dose in the window.
    pub max_dose: f32,
    /// Cells per side.
    pub grid: u16,
    /// Cells being attenuated: `[column from the left, row from the
    /// bottom, factor]`.
    pub cells: Vec<[f32; 3]>,
    /// Lit samples dimmed or blanked in the last frame.
    pub points_dimmed: usize,
}

/// The guard's state: the sliding window of per-cell doses and the
/// attenuations, which rise back slowly.
#[derive(Default)]
pub struct DwellState {
    grid: usize,
    /// (time, [(cell, dose-seconds)]) per frame in the window.
    frames: VecDeque<(f64, Vec<(u32, f32)>)>,
    /// Sum of `frames` per cell, in seconds at full brightness.
    sum: Vec<f64>,
    /// Attenuation applied per cell last frame.
    factor: Vec<f32>,
    extent_factor: f32,
    last_t: Option<f64>,
    scratch: Vec<f32>,
    status: DwellStatus,
}

/// Output dose of a cell whose input dose is `d`, for a cap `cap`: as is up
/// to the cap, held at the cap up to 1.5×, then faded to 0 at 2×.
/// Continuous, so a dose that jitters around a threshold can't flicker.
fn capped_dose(d: f32, cap: f32) -> f32 {
    if d <= cap {
        d
    } else if d <= 1.5 * cap {
        cap
    } else {
        (cap * (2.0 * cap - d) / (0.5 * cap)).max(0.0)
    }
}

fn brightness(p: &Point) -> f32 {
    p.r.max(p.g).max(p.b).clamp(0.0, 1.0)
}

impl DwellState {
    pub fn status(&self) -> &DwellStatus {
        &self.status
    }

    fn cell_of(&self, p: &Point) -> usize {
        let g = self.grid;
        let idx = |v: f32| (((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * g as f32) as usize).min(g - 1);
        idx(p.y) * g + idx(p.x)
    }

    fn reset(&mut self, grid: usize) {
        self.grid = grid;
        self.frames.clear();
        self.sum = vec![0.0; grid * grid];
        self.factor = vec![1.0; grid * grid];
        self.scratch = vec![0.0; grid * grid];
        self.extent_factor = 1.0;
    }

    /// Takes the frame about to go out at time `t` (seconds, monotonic)
    /// and returns it dimmed where it concentrates too much light.
    /// `horizon_y`: below it the strict rules apply in the `Beams` profile.
    pub fn apply(&mut self, mut frame: Vec<Point>, t: f64, cfg: &DwellGuard, horizon_y: f32, armed: bool) -> Vec<Point> {
        let cfg = cfg.sanitized();
        if self.grid != cfg.grid as usize || self.sum.is_empty() {
            self.reset(cfg.grid as usize);
        }
        let g = self.grid;
        let dt = self.last_t.map_or(1.0 / 60.0, |last| (t - last).clamp(0.0, 0.1)) as f32;
        self.last_t = Some(t);
        let window = cfg.window_ms as f64 / 1000.0;

        // Measure the input: each cell's share of this frame's lit time.
        let n = frame.len().max(1) as f32;
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        let mut any_lit = false;
        let mut lit_below = false;
        let mut touched: Vec<(u32, f32)> = Vec::new();
        for p in &frame {
            let b = brightness(p);
            if b <= 0.0 {
                continue;
            }
            any_lit = true;
            lit_below |= p.y < horizon_y;
            lo = [lo[0].min(p.x), lo[1].min(p.y)];
            hi = [hi[0].max(p.x), hi[1].max(p.y)];
            let c = self.cell_of(p);
            if self.scratch[c] == 0.0 {
                touched.push((c as u32, 0.0));
            }
            self.scratch[c] += b;
        }
        for (c, e) in &mut touched {
            *e = self.scratch[*c as usize] / n * dt;
            self.scratch[*c as usize] = 0.0;
            self.sum[*c as usize] += *e as f64;
        }
        self.frames.push_back((t, touched));
        while self.frames.front().is_some_and(|(ft, _)| *ft <= t - window) {
            let (_, cells) = self.frames.pop_front().unwrap();
            for (c, e) in cells {
                let s = &mut self.sum[c as usize];
                *s = (*s - e as f64).max(0.0);
            }
        }
        if self.frames.len() == 1 {
            // Only this frame: wipe the rounding left by the ones dropped.
            self.sum.iter_mut().for_each(|s| *s = 0.0);
            for (c, e) in &self.frames[0].1 {
                self.sum[*c as usize] = *e as f64;
            }
        }
        let extent = if any_lit { ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt() } else { 0.0 };

        let profile = if cfg.profile == DwellProfile::Off && armed { DwellProfile::Beams } else { cfg.profile };
        let rise = dt * RISE_PER_S;
        let cell_h = 2.0 / g as f32;
        let mut max_dose = 0.0f32;
        let mut cells = Vec::new();
        for (c, (&s, f)) in self.sum.iter().zip(self.factor.iter_mut()).enumerate() {
            let d = (s / window) as f32;
            max_dose = max_dose.max(d);
            let target = if profile == DwellProfile::Off || d <= 0.0 {
                1.0
            } else {
                let bottom = -1.0 + (c / g) as f32 * cell_h;
                let strict = profile == DwellProfile::Strict || bottom < horizon_y;
                let cap = if strict { cfg.max_cell_dose } else { cfg.max_cell_dose_beams };
                (capped_dose(d, cap) / d).min(1.0)
            };
            *f = target.min(*f + rise);
            if *f < 1.0 {
                cells.push([(c % g) as f32, (c / g) as f32, *f]);
            }
        }

        // Minimum size: strict everywhere, or wherever something is lit
        // below the horizon; the whole frame fades between min and min/2.
        let size_applies = any_lit && (profile == DwellProfile::Strict || (profile == DwellProfile::Beams && lit_below));
        let half = cfg.min_extent * 0.5;
        let extent_target = if size_applies { ((extent - half) / half).clamp(0.0, 1.0) } else { 1.0 };
        self.extent_factor = extent_target.min(self.extent_factor + rise);

        let mut dimmed = 0;
        let all_one = self.extent_factor >= 1.0 && cells.is_empty();
        if !all_one {
            for p in &mut frame {
                if !p.is_lit() {
                    continue;
                }
                let k = self.extent_factor * self.factor[self.cell_of(p)];
                if k < 1.0 {
                    *p = Point { r: p.r * k, g: p.g * k, b: p.b * k, ..*p };
                    dimmed += 1;
                }
            }
        }
        self.status = DwellStatus {
            active: dimmed > 0,
            profile,
            extent,
            extent_factor: self.extent_factor,
            max_dose,
            grid: g as u16,
            cells,
            points_dimmed: dimmed,
        };
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{densify, Animator, BeatClock, Content, Settings};
    use crate::generators::GenParams;
    use crate::patterns;

    const FPS: f64 = 60.0;

    fn max_level(frame: &[Point]) -> f32 {
        frame.iter().map(brightness).fold(0.0, f32::max)
    }

    fn cfg(profile: DwellProfile) -> DwellGuard {
        DwellGuard { profile, ..Default::default() }
    }

    /// Runs `frame(t)` through a guard at 60 fps for `secs`; returns the
    /// output frames with their times, and the guard.
    fn run(frame: impl Fn(f64) -> Vec<Point>, secs: f64, cfg: &DwellGuard, armed: bool) -> (Vec<(f64, Vec<Point>)>, DwellState) {
        let mut st = DwellState::default();
        let mut out = Vec::new();
        for i in 0..(secs * FPS) as usize {
            let t = i as f64 / FPS;
            out.push((t, st.apply(frame(t), t, cfg, 0.0, armed)));
        }
        (out, st)
    }

    /// A full-brightness spot held at (x, y): `n` samples.
    fn spot(x: f32, y: f32, n: usize) -> Vec<Point> {
        vec![Point::lit(x, y, 1.0, 1.0, 1.0); n]
    }

    /// A circle of radius `r` at (cx, cy), densified like the engine does.
    fn circle(cx: f32, cy: f32, r: f32) -> Vec<Point> {
        densify(&patterns::circle(r, 1.0, 1.0, 1.0)).into_iter().map(|p| Point { x: p.x + cx, y: p.y + cy, ..p }).collect()
    }

    fn render(content: Content, scale: f32, secs: f32) -> Vec<Point> {
        let settings = Settings { content, brightness: 1.0, scale, ..Default::default() };
        let mut a = Animator::default();
        let mut frame = Vec::new();
        for _ in 0..(secs * 60.0) as usize {
            frame = a.render(&settings, Default::default(), 1.0 / 60.0, &BeatClock::default());
        }
        frame
    }

    #[test]
    fn a_single_point_is_blanked_in_strict_at_once() {
        let (out, st) = run(|_| spot(0.3, 0.4, 500), 1.0, &cfg(DwellProfile::Strict), true);
        assert!(out.iter().all(|(_, f)| max_level(f) == 0.0), "no lit frame at all");
        assert!(st.status().active);
        assert_eq!(st.status().extent_factor, 0.0);
        assert!(st.status().max_dose > 0.9, "{:?}", st.status().max_dose);
    }

    #[test]
    fn a_single_point_in_beams_above_the_horizon_is_capped_not_blanked() {
        let (out, st) = run(|_| spot(0.0, 0.5, 500), 1.0, &cfg(DwellProfile::Beams), true);
        let last = &out.last().unwrap().1;
        let level = max_level(last);
        assert!(level > 0.2 && level <= 0.6, "dimmed to the cap region, got {level}");
        assert!(st.status().active);
        assert_eq!(st.status().cells.len(), 1);
        // Below the horizon it is the audience's side: strict.
        let (out, _) = run(|_| spot(0.0, -0.5, 500), 1.0, &cfg(DwellProfile::Beams), true);
        assert!(out.iter().all(|(_, f)| max_level(f) == 0.0));
    }

    #[test]
    fn a_fixed_point_beside_a_big_figure_is_blanked_within_the_window() {
        // A large square (extent fine) plus a spot holding 60 % of the
        // samples: only the grid can catch it. Strict: off in <= 250 ms.
        let square = densify(&patterns::square(0.8, 1.0, 1.0, 1.0));
        let n = square.len();
        let frame = move |t: f64| {
            let mut f = square.clone();
            if t >= 0.5 {
                f.extend(spot(0.1, 0.1, n * 3 / 2));
            }
            f
        };
        let (out, st) = run(frame, 1.5, &cfg(DwellProfile::Strict), true);
        let spot_level = |f: &[Point]| f.iter().filter(|p| (p.x - 0.1).abs() < 1e-6 && (p.y - 0.1).abs() < 1e-6).map(brightness).fold(0.0, f32::max);
        let off_at = out.iter().find(|(t, f)| *t >= 0.5 && spot_level(f) == 0.0).map(|(t, _)| *t).expect("spot blanked");
        assert!(off_at - 0.5 <= 0.25, "blanked after {:.3} s", off_at - 0.5);
        assert!(out.iter().filter(|(t, _)| *t >= off_at).all(|(_, f)| spot_level(f) == 0.0), "stays off");
        // The square stays lit away from the spot.
        let last = &out.last().unwrap().1;
        assert!(last.iter().any(|p| p.is_lit() && p.x.abs() > 0.7));
        assert!(st.status().cells.len() <= 2, "{:?}", st.status().cells);
    }

    #[test]
    fn a_shrinking_circle_goes_dark_below_half_the_minimum_size() {
        // Diagonal of a circle's bounding box = 2 √2 r.
        // Some(true) = untouched, Some(false) = dark, None = halfway.
        for (diag, expect) in [(0.2f32, Some(true)), (0.05, Some(true)), (0.0375, None), (0.025, Some(false)), (0.01, Some(false)), (0.0, Some(false))] {
            let r = diag / (2.0 * 2f32.sqrt());
            let (out, st) = run(|_| circle(0.0, 0.3, r), 0.3, &cfg(DwellProfile::Strict), true);
            let level = max_level(&out.last().unwrap().1);
            match expect {
                Some(false) => assert_eq!(level, 0.0, "diag {diag}"),
                Some(true) => assert!(st.status().extent_factor >= 0.999, "diag {diag}: {:?}", st.status().extent_factor),
                None => {
                    let k = st.status().extent_factor;
                    assert!(k > 0.4 && k < 0.6, "diag {diag}: halfway, got {k}");
                }
            }
        }
        // Shrinking over time (a size LFO to 0): dark by the time it's under 2.5 %.
        let (out, _) = run(|t| circle(0.0, 0.3, (0.2 * (1.0 - t)).max(0.0) as f32), 1.2, &cfg(DwellProfile::Strict), true);
        for (t, f) in &out {
            let diag = 2.0 * 2f64.sqrt() * (0.2 * (1.0 - t)).max(0.0);
            if diag < 0.025 {
                assert_eq!(max_level(f), 0.0, "t {t}: diag {diag}");
            }
        }
    }

    #[test]
    fn a_tiny_circle_is_cut_but_a_moving_line_is_not() {
        // A 3 % circle: under the minimum size and in one or two cells.
        let (out, _) = run(|_| circle(0.1, 0.2, 0.01), 0.5, &cfg(DwellProfile::Strict), true);
        assert_eq!(max_level(&out.last().unwrap().1), 0.0);
        // A long line sweeping across the field: no cell holds light.
        let line = |t: f64| {
            let y = (t * 2.0).sin() as f32 * 0.8;
            densify(&[Point::lit(-0.8, y, 1.0, 1.0, 1.0), Point::lit(0.8, y, 1.0, 1.0, 1.0)])
        };
        let (out, st) = run(line, 2.0, &cfg(DwellProfile::Strict), true);
        for (t, f) in &out {
            assert!(f.iter().filter(|p| p.is_lit()).all(|p| brightness(p) == 1.0), "t {t}: untouched");
        }
        assert!(!st.status().active);
        assert!(st.status().max_dose < 0.25, "{}", st.status().max_dose);
    }

    #[test]
    fn a_slow_moving_spot_is_caught_by_the_grid_a_fast_one_is_not() {
        // Strict, with a big square so the size check stays out of it.
        let square = densify(&patterns::square(0.9, 1.0, 1.0, 1.0));
        let n = square.len();
        let with_spot = move |x: f32| {
            let mut f = square.clone();
            f.extend(spot(x, 0.0, n * 2));
            f
        };
        let at = |speed: f64| {
            let f = with_spot.clone();
            move |t: f64| f(((t * speed).rem_euclid(1.6) - 0.8) as f32)
        };
        let dimmed = |speed: f64| {
            let (out, _) = run(at(speed), 2.0, &cfg(DwellProfile::Strict), true);
            out.iter().skip(30).any(|(_, f)| f.iter().any(|p| p.is_lit() && brightness(p) < 1.0))
        };
        assert!(dimmed(0.1), "a spot crawling at 0.1 field/s is a hot spot");
        assert!(!dimmed(20.0), "sweeping at 20 units/s spreads over the grid");
    }

    #[test]
    fn a_beam_fan_above_the_horizon_stays_lit_in_beams() {
        let fan = render(Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 6, b: 0.6, ..Default::default() } }, 0.8, 0.5);
        assert!(fan.iter().any(|p| p.is_lit()));
        let (out, st) = run(|_| fan.clone(), 2.0, &cfg(DwellProfile::Beams), true);
        assert_eq!(out.last().unwrap().1, fan, "untouched");
        assert!(!st.status().active);
        // A single beam cue is capped (still lit), and blanked in Strict.
        let one = render(Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 1, b: 0.6, ..Default::default() } }, 0.8, 0.5);
        let (out, _) = run(|_| one.clone(), 1.0, &cfg(DwellProfile::Beams), true);
        let level = max_level(&out.last().unwrap().1);
        assert!(level > 0.0 && level < 1.0, "capped: {level}");
        let (out, _) = run(|_| one.clone(), 1.0, &cfg(DwellProfile::Strict), true);
        assert_eq!(max_level(&out.last().unwrap().1), 0.0);
    }

    #[test]
    fn a_full_field_square_is_never_attenuated() {
        // 30 000 pps at 60 fps = 500 samples a frame.
        let sq = densify(&patterns::square(1.0, 1.0, 1.0, 1.0));
        let mut frame = Vec::new();
        while frame.len() < 500 {
            frame.extend_from_slice(&sq);
        }
        frame.truncate(500);
        for profile in [DwellProfile::Strict, DwellProfile::Beams] {
            let (out, st) = run(|_| frame.clone(), 2.0, &cfg(profile), true);
            assert!(out.iter().all(|(_, f)| *f == frame), "{profile:?}");
            assert!(!st.status().active);
        }
        // And every shape of the catalogue at a normal size, in Beams.
        for shape in patterns::SHAPE_NAMES {
            let f = render(Content::Shape { shape: shape.to_string() }, 0.5, 0.2);
            let (out, _) = run(|_| f.clone(), 1.0, &cfg(DwellProfile::Beams), true);
            assert_eq!(out.last().unwrap().1, f, "{shape}");
        }
    }

    #[test]
    fn off_only_while_disarmed() {
        let (out, st) = run(|_| spot(0.0, -0.5, 500), 0.5, &cfg(DwellProfile::Off), false);
        assert!(out.iter().all(|(_, f)| max_level(f) == 1.0), "preview: untouched");
        assert_eq!(st.status().profile, DwellProfile::Off);
        let (out, st) = run(|_| spot(0.0, -0.5, 500), 0.5, &cfg(DwellProfile::Off), true);
        assert_eq!(max_level(&out.last().unwrap().1), 0.0, "armed: guarded as Beams");
        assert_eq!(st.status().profile, DwellProfile::Beams);
    }

    #[test]
    fn the_attenuation_lifts_slowly_and_never_flickers() {
        // A point for 0.5 s, then a big circle: the circle comes back over
        // >= 250 ms, monotonic (no on/off/on).
        let frame = |t: f64| if t < 0.5 { spot(0.0, 0.3, 500) } else { circle(0.0, 0.0, 0.6) };
        let (out, _) = run(frame, 1.5, &cfg(DwellProfile::Strict), true);
        let levels: Vec<(f64, f32)> = out.iter().filter(|(t, _)| *t >= 0.5).map(|(t, f)| (*t, max_level(f))).collect();
        assert!(levels.windows(2).all(|w| w[1].1 >= w[0].1 - 1e-6), "monotonic recovery");
        let full = levels.iter().find(|(_, l)| *l >= 0.999).map(|(t, _)| *t).expect("back to full");
        assert!(full - 0.5 >= 0.23, "recovers over >= 250 ms, got {:.3}", full - 0.5);
        assert!(full - 0.5 <= 0.6, "and not much later, got {:.3}", full - 0.5);
    }

    #[test]
    fn a_dose_jittering_around_a_threshold_does_not_flicker() {
        // Continuous output dose: small input changes, small output changes.
        for cap in [0.25f32, 0.6] {
            let mut prev = capped_dose(0.0, cap);
            for i in 1..=2000 {
                let d = i as f32 / 1000.0;
                let o = capped_dose(d, cap);
                assert!((o - prev).abs() <= 0.01, "cap {cap}, d {d}: {prev} → {o}");
                assert!(o <= cap + 1e-6);
                prev = o;
            }
            assert_eq!(capped_dose(2.0 * cap, cap), 0.0);
        }
    }

    #[test]
    fn dose_does_not_depend_on_point_count() {
        // Same picture, 4× more samples (a higher pps): same doses.
        let a = spot(0.2, 0.2, 100);
        let mut b = spot(0.2, 0.2, 400);
        b.extend(circle(0.0, 0.0, 0.5).into_iter().cycle().take(1200));
        let mut a2 = a.clone();
        a2.extend(circle(0.0, 0.0, 0.5).into_iter().cycle().take(300));
        let (_, sa) = run(|_| a2.clone(), 0.5, &cfg(DwellProfile::Beams), true);
        let (_, sb) = run(|_| b.clone(), 0.5, &cfg(DwellProfile::Beams), true);
        assert!((sa.status().max_dose - sb.status().max_dose).abs() < 0.02, "{} vs {}", sa.status().max_dose, sb.status().max_dose);
    }

    #[test]
    fn validation_loosening_and_sanitizing() {
        let d = DwellGuard::default();
        assert!(d.validate().is_ok());
        assert!(DwellGuard { min_extent: 0.001, ..d }.validate().is_err());
        assert!(DwellGuard { max_cell_dose: f32::NAN, ..d }.validate().is_err());
        assert!(DwellGuard { window_ms: 1000, ..d }.validate().is_err());
        assert!(DwellGuard { grid: 4, ..d }.validate().is_err());
        // Tighter: nothing to confirm.
        let strict = DwellGuard { profile: DwellProfile::Strict, min_extent: 0.1, max_cell_dose: 0.2, max_cell_dose_beams: 0.5, window_ms: 200, grid: 48 };
        assert!(d.loosenings(&strict).is_empty());
        // Back is looser on every field.
        let l = strict.loosenings(&d);
        assert_eq!(l.len(), 6, "{l:?}");
        assert!(l[0].contains("Stricte → Faisceaux"));
        assert_eq!(d.loosenings(&cfg(DwellProfile::Off)).len(), 1);
        // Hand-edited nonsense → the tight end.
        let s = DwellGuard { min_extent: f32::NAN, max_cell_dose: 9.0, max_cell_dose_beams: f32::INFINITY, window_ms: 5000, grid: 1, ..d }.sanitized();
        assert!(s.validate().is_ok());
        assert_eq!((s.min_extent, s.max_cell_dose, s.window_ms, s.grid), (0.5, 0.5, 250, 16));
        // serde: missing fields are the defaults, profile in lowercase.
        let parsed: DwellGuard = serde_json::from_str(r#"{"profile":"strict"}"#).unwrap();
        assert_eq!(parsed, DwellGuard { profile: DwellProfile::Strict, ..d });
    }

    #[test]
    fn two_thousand_points_are_cheap() {
        let frame: Vec<Point> = (0..2000).map(|i| {
            let a = i as f32 * 0.01;
            Point::lit(a.cos() * a / 20.0, a.sin() * a / 20.0, 1.0, 0.5, 0.2)
        }).collect();
        let mut st = DwellState::default();
        let c = cfg(DwellProfile::Strict);
        for i in 0..30 {
            st.apply(frame.clone(), i as f64 / FPS, &c, 0.0, true);
        }
        let n = 200;
        let start = std::time::Instant::now();
        for i in 0..n {
            std::hint::black_box(st.apply(frame.clone(), (30 + i) as f64 / FPS, &c, 0.0, true));
        }
        let per = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("dwell guard: {per:.4} ms per 2000-point frame");
        // < 0.5 ms in release; the debug test build is ~10× slower.
        assert!(per < 5.0, "{per:.3} ms per frame");
    }
}
