//! Colour rendering of one output (T-414): white balance, response curve
//! per colour and colour delay.
//!
//! Every projector has its own white, its diodes don't answer linearly,
//! and the lag between the colour and the galvos depends on the machine.
//! These are **rendering** settings of the output, set in front of the
//! test patterns (T-404), not safety limits:
//!
//! - **Curve** per channel: a gamma (`v^gamma`, 1.0 = linear) or five
//!   points (outputs for the inputs 0, 25, 50, 75 and 100 %, linear in
//!   between, never decreasing). The curve starts at 0: a dark channel
//!   stays dark (the diode threshold is the safety stage's minimum diode
//!   level, which applies after this).
//! - **White balance**: a gain 0..1 per channel, after the curve.
//! - **Colour delay** (-8..+8 points): point *i* takes the colour of point
//!   *i - n* (positive: the colour comes later than the position, the
//!   same sense as `laser-dac`'s own delay, to which ours adds). The shift
//!   stays **inside each lit stroke**: a point that was dark in the frame
//!   stays dark (blanked travel never receives colour), and at the ends
//!   of a stroke the colour is held (the stroke's own first or last
//!   colour), so the number of points never changes and no light moves
//!   from one stroke to another.
//!
//! **Order in the engine** (`Engine::tick`): content, calibration,
//! output size limit → **these colours** → `safety::apply` (zones,
//! horizon, safety gains, minimum diode level, strobe limiter) → static
//! beam guard → power caps (T-254) → the arming gate. Every channel leaves
//! this stage within 0..1, and the safety stages and the caps come after:
//! nothing set here can get past a Blank zone, the horizon or a cap.
//!
//! Identity settings (the default, and a missing file) leave the frame
//! strictly untouched. Raising a gain, or a curve that gives more light at
//! the same input, is a loosening: it needs the operator's confirmation
//! (`loosenings`), like the safety settings. None of this is reachable
//! from MIDI, OSC, DMX, cues or projects: machine settings in
//! `outputs.json`, edited from RÉGLAGES › Sorties et projecteurs only.

use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Colour delay limit, in points either way.
pub const MAX_DELAY: i8 = 8;
/// Gamma range accepted (and clamped to in a hand-edited file).
pub const GAMMA_RANGE: (f32, f32) = (0.2, 5.0);
/// The inputs of the five-point curve.
pub const NODES: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

const COLOURS: [&str; 3] = ["rouge", "vert", "bleu"];
/// Inputs where two curves are compared for `loosenings`.
const PROBES: usize = 40;

/// The response of one colour channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelCurve {
    /// `out = in^gamma`.
    Gamma { gamma: f32 },
    /// Outputs for the inputs `NODES`, linear in between.
    Points { out: [f32; 5] },
}

impl Default for ChannelCurve {
    fn default() -> Self {
        Self::Gamma { gamma: 1.0 }
    }
}

impl ChannelCurve {
    /// The output for `v`: 0 for a dark (≤ 0) or broken value, within
    /// 0..1 otherwise.
    pub fn eval(&self, v: f32) -> f32 {
        // Dark, negative or not a number: dark.
        if v.is_nan() || v <= 0.0 {
            return 0.0;
        }
        let v = v.min(1.0);
        let out = match self {
            Self::Gamma { gamma } => v.powf(*gamma),
            Self::Points { out } => {
                let x = v * 4.0;
                let i = (x.floor() as usize).min(3);
                let f = x - i as f32;
                out[i] + (out[i + 1] - out[i]) * f
            }
        };
        if out.is_finite() {
            out.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn is_identity(&self) -> bool {
        match self {
            Self::Gamma { gamma } => *gamma == 1.0,
            Self::Points { out } => *out == NODES,
        }
    }

    /// In range: gamma within `GAMMA_RANGE` (not a number: linear), points
    /// within 0..1, starting at 0 and never decreasing (each point at
    /// least the one before; not a number: the linear value).
    pub fn sanitized(&self) -> Self {
        match *self {
            Self::Gamma { gamma } => Self::Gamma { gamma: if gamma.is_finite() { gamma.clamp(GAMMA_RANGE.0, GAMMA_RANGE.1) } else { 1.0 } },
            Self::Points { out } => {
                let mut fixed = [0.0; 5];
                for i in 1..5 {
                    let v = if out[i].is_finite() { out[i].clamp(0.0, 1.0) } else { NODES[i] };
                    fixed[i] = v.max(fixed[i - 1]);
                }
                Self::Points { out: fixed }
            }
        }
    }

    fn validate(&self, name: &str) -> Result<()> {
        match *self {
            Self::Gamma { gamma } => {
                if !gamma.is_finite() || gamma < GAMMA_RANGE.0 || gamma > GAMMA_RANGE.1 {
                    bail!("Gamma {name} : {gamma} hors limites ({} à {})", GAMMA_RANGE.0, GAMMA_RANGE.1);
                }
            }
            Self::Points { out } => {
                if out.iter().any(|v| !v.is_finite() || *v < 0.0 || *v > 1.0) {
                    bail!("Courbe {name} : chaque point entre 0 et 1");
                }
            }
        }
        Ok(())
    }
}

/// The colour settings of one output (`outputs.json`, key `color`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorCalibration {
    /// Gain per channel (red, green, blue), 0..1, after the curve.
    pub white_balance: [f32; 3],
    /// Response curve per channel.
    pub curve: [ChannelCurve; 3],
    /// Colour shift relative to the position, in points (-8..+8).
    pub color_delay_points: i8,
}

impl Default for ColorCalibration {
    fn default() -> Self {
        Self { white_balance: [1.0; 3], curve: [ChannelCurve::default(); 3], color_delay_points: 0 }
    }
}

impl ColorCalibration {
    /// Leaves every frame strictly as it is.
    pub fn is_identity(&self) -> bool {
        self.white_balance == [1.0; 3] && self.curve.iter().all(ChannelCurve::is_identity) && self.color_delay_points == 0
    }

    /// Refuses what the API should not take (400); a decreasing curve is
    /// accepted and straightened by `sanitized`.
    pub fn validate(&self) -> Result<()> {
        for (g, name) in self.white_balance.iter().zip(COLOURS) {
            if !g.is_finite() || *g < 0.0 || *g > 1.0 {
                bail!("Balance {name} : {g} hors limites (0 à 1)");
            }
        }
        for (c, name) in self.curve.iter().zip(COLOURS) {
            c.validate(name)?;
        }
        if self.color_delay_points.abs() > MAX_DELAY {
            bail!("Retard couleur : {} points hors limites (-{MAX_DELAY} à {MAX_DELAY})", self.color_delay_points);
        }
        Ok(())
    }

    /// Clamped into range, for a hand-edited file. A gain that is not a
    /// number becomes 0 (the darker end).
    pub fn sanitized(&self) -> Self {
        Self {
            white_balance: self.white_balance.map(|g| if g.is_finite() { g.clamp(0.0, 1.0) } else { 0.0 }),
            curve: self.curve.map(|c| c.sanitized()),
            color_delay_points: self.color_delay_points.clamp(-MAX_DELAY, MAX_DELAY),
        }
    }

    /// Curve, then balance, then colour delay, in place (see the module
    /// docs). Positions and the number of points never change; a point
    /// dark on the way in is dark on the way out; every channel ends
    /// within 0..1.
    pub fn apply(&self, frame: &mut [Point]) {
        if self.is_identity() {
            return;
        }
        let c = self.sanitized();
        let delay = c.color_delay_points as isize;
        // Which points were lit before anything here: the strokes.
        let lit: Vec<bool> = if delay != 0 { frame.iter().map(Point::is_lit).collect() } else { Vec::new() };
        for p in frame.iter_mut() {
            p.r = c.curve[0].eval(p.r) * c.white_balance[0];
            p.g = c.curve[1].eval(p.g) * c.white_balance[1];
            p.b = c.curve[2].eval(p.b) * c.white_balance[2];
        }
        if delay == 0 {
            return;
        }
        let mut colours: Vec<[f32; 3]> = Vec::new();
        let mut i = 0;
        while i < frame.len() {
            if !lit[i] {
                // Dark on the way in: dark on the way out.
                let p = &mut frame[i];
                (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
                i += 1;
                continue;
            }
            let start = i;
            while i < frame.len() && lit[i] {
                i += 1;
            }
            let end = i; // exclusive
            colours.clear();
            colours.extend(frame[start..end].iter().map(|p| [p.r, p.g, p.b]));
            let last = (end - start - 1) as isize;
            for (k, p) in frame[start..end].iter_mut().enumerate() {
                let [r, g, b] = colours[(k as isize - delay).clamp(0, last) as usize];
                (p.r, p.g, p.b) = (r, g, b);
            }
        }
    }

    /// What going from `self` to `new` would brighten, in French, for the
    /// confirmation the operator must give. Empty = only darker or equal.
    /// The colour delay never counts: it moves light, it adds none.
    pub fn loosenings(&self, new: &ColorCalibration) -> Vec<String> {
        let (old, new) = (self.sanitized(), new.sanitized());
        let mut out = Vec::new();
        for (i, name) in COLOURS.iter().enumerate() {
            if new.white_balance[i] > old.white_balance[i] {
                out.push(format!("Balance {name} : {:.0} % → {:.0} %", old.white_balance[i] * 100.0, new.white_balance[i] * 100.0));
            }
            let brighter = (1..=PROBES).any(|k| {
                let v = k as f32 / PROBES as f32;
                new.curve[i].eval(v) > old.curve[i].eval(v) + 1e-4
            });
            if brighter {
                out.push(format!("Courbe {name} : plus de lumière à entrée égale"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey_ramp(n: usize) -> Vec<Point> {
        (0..=n).map(|k| {
            let v = k as f32 / n as f32;
            Point::lit(v * 2.0 - 1.0, 0.0, v, v, v)
        }).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn gamma_2_2_on_a_grey_ramp() {
        let mut cal = ColorCalibration::default();
        cal.curve[1] = ChannelCurve::Gamma { gamma: 2.2 };
        let before = grey_ramp(10);
        let mut f = before.clone();
        cal.apply(&mut f);
        for (a, b) in f.iter().zip(&before) {
            assert_eq!((a.x, a.y, a.r, a.b), (b.x, b.y, b.r, b.b), "red, blue and positions untouched");
            assert!(close(a.g, b.g.powf(2.2)), "{} → {}", b.g, a.g);
        }
        assert_eq!((f[0].g, f[10].g), (0.0, 1.0), "the ends don't move");
        assert!(f[5].g < 0.25, "mid grey darker: {}", f[5].g);
    }

    #[test]
    fn five_point_curve_hits_its_nodes_and_interpolates() {
        let out = [0.0, 0.1, 0.3, 0.6, 0.9];
        let c = ChannelCurve::Points { out };
        for (x, y) in NODES.iter().zip(out) {
            assert!(close(c.eval(*x), y), "node {x}: {}", c.eval(*x));
        }
        assert!(close(c.eval(0.125), 0.05));
        assert!(close(c.eval(0.625), 0.45));
        // A neutral grey ramp on all three channels comes out at the nodes.
        let cal = ColorCalibration { curve: [c; 3], ..Default::default() };
        let mut f = grey_ramp(4);
        cal.apply(&mut f);
        for (p, y) in f.iter().zip(out) {
            assert!(close(p.r, y) && close(p.g, y) && close(p.b, y), "{p:?} vs {y}");
        }
    }

    #[test]
    fn a_decreasing_curve_is_straightened_and_starts_at_zero() {
        let c = ChannelCurve::Points { out: [0.3, 0.5, 0.2, f32::NAN, 2.0] }.sanitized();
        assert_eq!(c, ChannelCurve::Points { out: [0.0, 0.5, 0.5, 0.75, 1.0] });
        assert_eq!(ChannelCurve::Gamma { gamma: 0.01 }.sanitized(), ChannelCurve::Gamma { gamma: 0.2 });
        assert_eq!(ChannelCurve::Gamma { gamma: f32::NAN }.sanitized(), ChannelCurve::Gamma { gamma: 1.0 });
        let cal = ColorCalibration { white_balance: [f32::NAN, 1.5, -1.0], color_delay_points: 100, ..Default::default() }.sanitized();
        assert_eq!((cal.white_balance, cal.color_delay_points), ([0.0, 1.0, 0.0], MAX_DELAY));
        // A point dark on the way in stays dark, whatever the first point says.
        let raw = ColorCalibration { curve: [ChannelCurve::Points { out: [0.5, 0.6, 0.7, 0.8, 1.0] }; 3], ..Default::default() };
        let mut f = vec![Point::blanked(0.0, 0.0), Point::lit(0.0, 0.0, 0.0, 1e-6, 0.0)];
        raw.apply(&mut f);
        assert!(!f[0].is_lit() && f[1].r == 0.0 && f[1].b == 0.0 && f[1].g < 1e-5, "{f:?}");
    }

    #[test]
    fn validation() {
        assert!(ColorCalibration::default().validate().is_ok());
        for bad in [
            ColorCalibration { white_balance: [1.1, 1.0, 1.0], ..Default::default() },
            ColorCalibration { white_balance: [f32::NAN, 1.0, 1.0], ..Default::default() },
            ColorCalibration { curve: [ChannelCurve::Gamma { gamma: 6.0 }; 3], ..Default::default() },
            ColorCalibration { curve: [ChannelCurve::Points { out: [0.0, 0.2, 0.4, 0.6, 1.2] }; 3], ..Default::default() },
            ColorCalibration { color_delay_points: -9, ..Default::default() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn white_balance_scales_each_channel() {
        let cal = ColorCalibration { white_balance: [0.5, 1.0, 0.25], ..Default::default() };
        let mut f = vec![Point::lit(0.1, 0.2, 1.0, 0.8, 0.4)];
        cal.apply(&mut f);
        assert!(close(f[0].r, 0.5) && close(f[0].g, 0.8) && close(f[0].b, 0.1), "{:?}", f[0]);
        assert_eq!((f[0].x, f[0].y), (0.1, 0.2));
    }

    /// Travel, a 20-point stroke whose colour changes every point, travel.
    fn stroke() -> Vec<Point> {
        let mut f: Vec<Point> = (0..5).map(|i| Point::blanked(-1.0 + i as f32 * 0.01, 0.5)).collect();
        f.extend((0..20).map(|k| Point::lit(k as f32 * 0.05 - 0.5, 0.0, k as f32 / 19.0, 1.0 - k as f32 / 19.0, 0.5)));
        f.extend((0..5).map(|i| Point::blanked(0.8 + i as f32 * 0.01, -0.5)));
        f
    }

    #[test]
    fn colour_delay_shifts_inside_the_stroke_and_never_lights_travel() {
        let before = stroke();
        for n in [3i8, -3] {
            let mut f = before.clone();
            ColorCalibration { color_delay_points: n, ..Default::default() }.apply(&mut f);
            assert_eq!(f.len(), before.len(), "point count unchanged");
            for (i, (a, b)) in f.iter().zip(&before).enumerate() {
                assert_eq!((a.x, a.y), (b.x, b.y), "positions unchanged");
                if !b.is_lit() {
                    assert!(!a.is_lit(), "delay {n}: travel point {i} lit: {a:?}");
                }
            }
            for k in 0..20usize {
                let src = (k as isize - n as isize).clamp(0, 19) as usize;
                let (got, want) = (f[5 + k], before[5 + src]);
                assert_eq!((got.r, got.g, got.b), (want.r, want.g, want.b), "delay {n}, stroke point {k}");
            }
        }
        // Positive: the colour comes later (the stroke's first colour held).
        let mut f = before.clone();
        ColorCalibration { color_delay_points: 3, ..Default::default() }.apply(&mut f);
        assert_eq!(f[5 + 3].r, before[5].r);
    }

    #[test]
    fn identity_leaves_the_frame_exactly_as_it_was() {
        let before = {
            let mut f = stroke();
            f.push(Point::lit(0.0, 0.0, f32::NAN, 2.0, -1.0));
            f
        };
        let mut f = before.clone();
        ColorCalibration::default().apply(&mut f);
        assert_eq!(format!("{f:?}"), format!("{before:?}"), "bit for bit, odd values included");
        assert!(ColorCalibration { curve: [ChannelCurve::Points { out: NODES }; 3], ..Default::default() }.is_identity());
    }

    #[test]
    fn brighter_is_a_loosening_darker_is_not() {
        let base = ColorCalibration { white_balance: [0.8, 0.8, 0.8], curve: [ChannelCurve::Gamma { gamma: 2.0 }; 3], ..Default::default() };
        assert!(base.loosenings(&base).is_empty());
        let darker = ColorCalibration { white_balance: [0.5, 0.8, 0.8], curve: [ChannelCurve::Gamma { gamma: 2.5 }; 3], color_delay_points: 4 };
        assert!(base.loosenings(&darker).is_empty(), "{:?}", base.loosenings(&darker));
        let up = ColorCalibration { white_balance: [1.0, 0.8, 0.8], ..base };
        assert_eq!(base.loosenings(&up), vec!["Balance rouge : 80 % → 100 %".to_string()]);
        let mut curve = base;
        curve.curve[2] = ChannelCurve::Gamma { gamma: 1.5 };
        assert_eq!(base.loosenings(&curve), vec!["Courbe bleu : plus de lumière à entrée égale".to_string()]);
        let mut points = base;
        points.curve[1] = ChannelCurve::Points { out: [0.0, 0.0, 0.0, 0.0, 1.0] };
        assert!(base.loosenings(&points).is_empty(), "under v^2 everywhere");
        points.curve[1] = ChannelCurve::Points { out: [0.0, 0.1, 0.3, 0.6, 1.0] };
        assert_eq!(base.loosenings(&points).len(), 1, "above v^2 near 0.25");
    }

    #[test]
    fn serde_defaults_and_shape() {
        let c: ColorCalibration = serde_json::from_str("{}").unwrap();
        assert!(c.is_identity());
        let c: ColorCalibration = serde_json::from_str(r#"{"curve":[{"type":"gamma","gamma":2.2},{"type":"points","out":[0,0.2,0.4,0.7,1]},{"type":"gamma","gamma":1}],"color_delay_points":-2}"#).unwrap();
        assert_eq!(c.curve[0], ChannelCurve::Gamma { gamma: 2.2 });
        assert_eq!(c.white_balance, [1.0; 3]);
        let back: ColorCalibration = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
