//! Exposure estimator (T-257): maximum permissible exposure (MPE, « EMP »)
//! and nominal ocular hazard distance (NOHD, « DNRO ») of a fixed beam.
//!
//! **An estimate, never a measurement nor a certification.** It turns the
//! projector sheet (T-254) into orders of magnitude so the operator sees
//! how far over the MPE a beam is at the audience; it does not replace a
//! calibrated meter, a qualified laser safety officer or the law, and the
//! real stop stays hardware (interlock, E-stop, key switch). Nothing here
//! touches the output: no cap is raised or lowered, nothing is armed. The
//! UI may only *suggest* a lower cap.
//!
//! Model (docs/research/safety-regulation.md §2.2, public sources):
//! - beam diameter at distance d: `D(d) = a + d·φ` (a = diameter at the
//!   aperture, φ = full-angle divergence; 1 m × 1 mrad = 1 mm);
//! - mean irradiance of a uniform beam: `E = 4P / (π·D²)`, averaged over the
//!   7 mm limiting aperture (the pupil) when `D` is smaller, as IEC
//!   60825-1 measures it for visible light;
//! - corneal MPE, 400–700 nm, 18 µs ≤ t ≤ 10 s, small source (C6 = 1):
//!   `H = 18·t^0.75 J/m²` (IEC 60825-1:2014, table A.1), so
//!   `E_MPE = H / t = 18·t^-0.25 W/m²`; with the aversion time t = 0.25 s,
//!   ≈ 25.5 W/m² = 2.55 mW/cm²;
//! - NOHD: the distance where `E = E_MPE`, i.e.
//!   `d = (sqrt(4P / (π·E_MPE)) − a) / φ` (IEC TR 60825-14 user's guide).
//!
//! Reference values from ILDA (<https://www.ilda.com/audiencescanningsafety.htm>)
//! are shown beside it: 2.5 mW/cm² for a static beam or hot spot, 10 mW/cm²
//! for a continuously scanned beam without hot spots.
//!
//! Conservative on purpose (every unknown errs towards more danger): all
//! colours added at full power, unknown aperture = 0 mm, unknown
//! divergence = no divergence (the NOHD is then « never »), a single pulse
//! of a static beam. Out of scope (said in the UI): scanned effects, which
//! ILDA asks to assess for single pulse, repeated pulses and average power;
//! the peak of a gaussian profile (up to twice the mean); atmosphere,
//! optics and reflections.

use crate::power::{OutputLimits, ProjectorInfo};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Aversion response time (blink reflex), s.
pub const EXPOSURE_S: f32 = 0.25;
/// Limiting aperture for visible light (the pupil), mm.
pub const MEASURE_APERTURE_MM: f64 = 7.0;
/// ILDA reference, static beam / hot spot, mW/cm².
pub const ILDA_STATIC_MW_CM2: f32 = 2.5;
/// ILDA reference, continuously scanned beam without hot spots, mW/cm².
pub const ILDA_SCANNED_MW_CM2: f32 = 10.0;
/// Range of the visible MPE formula, nm.
pub const VISIBLE_NM: (u16, u16) = (400, 700);
/// Range of exposure times the formula covers, s.
const EXPOSURE_RANGE_S: (f32, f32) = (18e-6, 10.0);
/// Audience distance the operator may type, m.
pub const DISTANCE_M: (f32, f32) = (0.1, 10_000.0);
/// Attenuation in the audience zone the operator may type, %. Never 100 %:
/// an attenuated zone always lets some light through in this estimate.
pub const ATTENUATION_PCT: (f32, f32) = (0.0, 99.9);

pub const DISCLAIMER_FR: &str = "Estimation théorique — ne remplace pas une mesure avec un appareil étalonné, \
l'évaluation d'une personne compétente en sécurité laser ni le respect de la réglementation. \
L'arrêt réel reste matériel (interlock, arrêt d'urgence, clé).";

/// One estimate's inputs (all already checked).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ExposureInput {
    /// Optical power, all colours added, mW.
    pub power_mw: f32,
    pub aperture_mm: f32,
    pub divergence_mrad: f32,
    pub distance_m: f32,
    /// Fraction of the light removed in the audience zone, 0..1.
    pub attenuation: f32,
    /// Exposure time, s (0.25 = aversion response).
    pub exposure_s: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ExposureResult {
    /// Beam diameter at the distance, mm.
    pub beam_mm: f32,
    /// Mean irradiance at the distance, attenuation applied, mW/cm².
    pub irradiance_mw_cm2: f32,
    pub mpe_mw_cm2: f32,
    /// Irradiance / MPE: above 1 = over the MPE.
    pub ratio: f32,
    /// NOHD without the zone attenuation (a fixed beam outside the zone),
    /// m. None = never under the MPE (no divergence declared).
    pub nohd_m: Option<f32>,
}

/// Beam diameter at `distance_m`, mm: `D = a + d·φ`.
pub fn beam_diameter_mm(aperture_mm: f64, divergence_mrad: f64, distance_m: f64) -> f64 {
    aperture_mm + distance_m * divergence_mrad
}

/// Mean irradiance of `power_mw` spread on a beam of `beam_mm`, mW/cm²:
/// `E = 4P / (π·D²)`, D at least the 7 mm limiting aperture.
pub fn irradiance_mw_cm2(power_mw: f64, beam_mm: f64) -> f64 {
    let d_cm = beam_mm.max(MEASURE_APERTURE_MM) / 10.0;
    4.0 * power_mw / (std::f64::consts::PI * d_cm * d_cm)
}

/// Corneal MPE of visible light for an exposure of `t_s`, mW/cm²:
/// `H = 18·t^0.75 J/m²`, `E = H / t`; 1 W/m² = 0.1 mW/cm².
pub fn mpe_mw_cm2(t_s: f64) -> f64 {
    18.0 * t_s.powf(0.75) / t_s * 0.1
}

/// Distance where a fixed beam falls to `mpe` (mW/cm²), m. Some(0) = under
/// it from the aperture; None = never (no divergence).
pub fn nohd_m(power_mw: f64, aperture_mm: f64, divergence_mrad: f64, mpe: f64) -> Option<f64> {
    if irradiance_mw_cm2(power_mw, aperture_mm) <= mpe {
        return Some(0.0);
    }
    if divergence_mrad <= 0.0 {
        return None;
    }
    // Diameter whose mean irradiance is the MPE (cm → mm); above 7 mm here
    // since the 7 mm aperture is over it.
    let needed_mm = (4.0 * power_mw / (std::f64::consts::PI * mpe)).sqrt() * 10.0;
    Some(((needed_mm - aperture_mm) / divergence_mrad).max(0.0))
}

impl ExposureInput {
    /// Refuses what is not a number, negative or out of range.
    pub fn validate(&self) -> Result<()> {
        let check = |v: f32, lo: f32, hi: f32, what: &str| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("{what} : {v} hors limites ({lo} à {hi})");
            }
            Ok(())
        };
        check(self.power_mw, 0.0, 1_000_000.0 * 3.0, "Puissance (mW)")?;
        check(self.aperture_mm, 0.0, 100.0, "Diamètre de sortie (mm)")?;
        check(self.divergence_mrad, 0.0, 100.0, "Divergence (mrad)")?;
        check(self.distance_m, DISTANCE_M.0, DISTANCE_M.1, "Distance (m)")?;
        check(self.attenuation, 0.0, ATTENUATION_PCT.1 / 100.0, "Atténuation")?;
        check(self.exposure_s, EXPOSURE_RANGE_S.0, EXPOSURE_RANGE_S.1, "Durée d'exposition (s)")?;
        Ok(())
    }
}

/// The estimate for `input` (refused when out of range).
pub fn estimate(input: &ExposureInput) -> Result<ExposureResult> {
    input.validate()?;
    let (p, a, phi, d) = (input.power_mw as f64, input.aperture_mm as f64, input.divergence_mrad as f64, input.distance_m as f64);
    let mpe = mpe_mw_cm2(input.exposure_s as f64);
    let beam = beam_diameter_mm(a, phi, d);
    let e = irradiance_mw_cm2(p * (1.0 - input.attenuation as f64), beam);
    Ok(ExposureResult {
        beam_mm: beam as f32,
        irradiance_mw_cm2: e as f32,
        mpe_mw_cm2: mpe as f32,
        ratio: (e / mpe) as f32,
        nohd_m: nohd_m(p, a, phi, mpe).map(|v| v as f32),
    })
}

/// What the operator types for one output (saved with it in
/// `outputs.json`; absent in older files: the defaults).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExposureSettings {
    /// Shortest distance between the projector and the audience, m.
    pub distance_m: f32,
    /// Light removed in the audience zone, % (T-003 « Atténuer »).
    pub attenuation_pct: f32,
}

impl Default for ExposureSettings {
    fn default() -> Self {
        // A close audience and no attenuation: the cautious end.
        Self { distance_m: 3.0, attenuation_pct: 0.0 }
    }
}

impl ExposureSettings {
    pub fn validate(&self) -> Result<()> {
        let (lo, hi) = DISTANCE_M;
        if !self.distance_m.is_finite() || self.distance_m < lo || self.distance_m > hi {
            bail!("Distance minimale du public : {} m hors limites ({lo} à {hi} m)", self.distance_m);
        }
        let (lo, hi) = ATTENUATION_PCT;
        if !self.attenuation_pct.is_finite() || self.attenuation_pct < lo || self.attenuation_pct > hi {
            bail!("Atténuation dans la zone public : {} % hors limites ({lo} à {hi} %)", self.attenuation_pct);
        }
        Ok(())
    }

    /// Clamped into range, for a file edited by hand; not a number → the
    /// default (the cautious end).
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        let fix = |v: f32, (lo, hi): (f32, f32), def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        Self { distance_m: fix(self.distance_m, DISTANCE_M, d.distance_m), attenuation_pct: fix(self.attenuation_pct, ATTENUATION_PCT, d.attenuation_pct) }
    }
}

/// The page « Estimation d'exposition » for one output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExposureReport {
    pub settings: ExposureSettings,
    /// Every colour at full power, from the sheet, mW.
    pub power_full_mw: f32,
    /// The same through the output caps (T-254), mW.
    pub power_capped_mw: f32,
    /// None: the sheet gives no power, nothing to estimate.
    pub result: Option<ExposureResult>,
    /// Ratio and NOHD with the projector at full power (no software cap:
    /// another program, a fault), same distance and attenuation.
    pub ratio_full: Option<f32>,
    pub nohd_full_m: Option<f32>,
    /// Irradiance against the ILDA references.
    pub ratio_ilda_static: Option<f32>,
    pub ratio_ilda_scanned: Option<f32>,
    /// The cap (0..1, like `max_power`) that would bring this estimate
    /// down to the MPE, when it is over. A suggestion only, never applied.
    pub suggested_max_power: Option<f32>,
    /// Inputs the estimate had to assume, in French.
    pub assumptions: Vec<String>,
    /// Why the estimate may not apply at all (wavelength out of range...).
    pub warnings: Vec<String>,
    pub exposure_s: f32,
    pub ilda_static_mw_cm2: f32,
    pub ilda_scanned_mw_cm2: f32,
    pub disclaimer: &'static str,
}

const COLOURS: [&str; 3] = ["rouge", "vert", "bleu"];

/// The estimate for an output's projector sheet, caps and typed settings.
/// Pure: reads only, changes nothing.
pub fn report(projector: &ProjectorInfo, limits: &OutputLimits, settings: &ExposureSettings) -> ExposureReport {
    let p = projector.sanitized();
    let l = limits.sanitized();
    let s = settings.sanitized();
    let mut assumptions = vec![
        "Toutes les couleurs à pleine puissance, additionnées (cas le plus défavorable).".to_string(),
        format!("Faisceau fixe, une seule exposition de {} s (réflexe de clignement), profil uniforme : le centre d'un faisceau gaussien peut être jusqu'à 2 fois plus intense.", EXPOSURE_S),
        "Effets balayés non évalués (impulsion unique, impulsions répétées et puissance moyenne, selon l'ILDA) : hors du périmètre de cette estimation.".to_string(),
    ];
    let mut warnings = Vec::new();
    for (i, name) in COLOURS.iter().enumerate() {
        if p.power_mw[i] <= 0.0 {
            assumptions.push(format!("Puissance {name} non renseignée : comptée 0 mW."));
        } else {
            let w = p.wavelength_nm[i];
            if w == 0 {
                assumptions.push(format!("Longueur d'onde {name} non renseignée : supposée visible (400 à 700 nm)."));
            } else if w < VISIBLE_NM.0 || w > VISIBLE_NM.1 {
                warnings.push(format!("Longueur d'onde {name} ({w} nm) hors de 400 à 700 nm : la formule de l'EMP visible ne s'applique pas, estimation non valable."));
            }
        }
    }
    if p.aperture_mm <= 0.0 {
        assumptions.push("Diamètre de sortie non renseigné : 0 mm supposé (cas le plus défavorable).".into());
    }
    if p.divergence_mrad <= 0.0 {
        assumptions.push("Divergence non renseignée : faisceau supposé sans divergence (cas le plus défavorable, distance de danger illimitée).".into());
    }
    let power_full: f32 = p.power_mw.iter().sum();
    let power_capped: f32 = (0..3).map(|i| p.power_mw[i] * l.ceiling(i)).sum();
    let input = |power_mw: f32| ExposureInput {
        power_mw,
        aperture_mm: p.aperture_mm,
        divergence_mrad: p.divergence_mrad,
        distance_m: s.distance_m,
        attenuation: s.attenuation_pct / 100.0,
        exposure_s: EXPOSURE_S,
    };
    let (result, full) = if power_full > 0.0 { (estimate(&input(power_capped)).ok(), estimate(&input(power_full)).ok()) } else { (None, None) };
    if power_full <= 0.0 {
        warnings.push("Aucune puissance dans la fiche projecteur (RÉGLAGES › Sorties et projecteurs) : rien à estimer.".into());
    }
    let suggested = result.filter(|r| r.ratio > 1.0).map(|r| l.max_power / r.ratio);
    ExposureReport {
        settings: s,
        power_full_mw: power_full,
        power_capped_mw: power_capped,
        ratio_full: full.map(|r| r.ratio),
        nohd_full_m: full.and_then(|r| r.nohd_m),
        ratio_ilda_static: result.map(|r| r.irradiance_mw_cm2 / ILDA_STATIC_MW_CM2),
        ratio_ilda_scanned: result.map(|r| r.irradiance_mw_cm2 / ILDA_SCANNED_MW_CM2),
        suggested_max_power: suggested,
        result,
        assumptions,
        warnings,
        exposure_s: EXPOSURE_S,
        ilda_static_mw_cm2: ILDA_STATIC_MW_CM2,
        ilda_scanned_mw_cm2: ILDA_SCANNED_MW_CM2,
        disclaimer: DISCLAIMER_FR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= b.abs() * rel
    }

    #[test]
    fn mpe_at_the_aversion_time_is_2_55_mw_per_cm2() {
        // 18 × 0.25^0.75 = 18 × 0.353553 = 6.36396 J/m²; / 0.25 s =
        // 25.4558 W/m² = 2.54558 mW/cm².
        let mpe = mpe_mw_cm2(0.25);
        assert!(close(mpe, 2.55, 0.01), "{mpe}");
        assert!(close(mpe, 2.545_58, 1e-5), "{mpe}");
        // t = 1 s: 18 J/m² over 1 s = 18 W/m² = 1.8 mW/cm².
        assert!(close(mpe_mw_cm2(1.0), 1.8, 1e-9));
    }

    #[test]
    fn one_watt_5mm_1mrad_at_10m_matches_the_hand_calculation() {
        // D = 5 mm + 10 m × 1 mrad = 15 mm = 1.5 cm.
        // E = 4 × 1000 mW / (π × 1.5²) = 4000 / 7.068583 = 565.884 mW/cm².
        // ratio = 565.884 / 2.545584 = 222.30.
        // NOHD: D needed = sqrt(4 × 1000 / (π × 2.545584)) = sqrt(500.17) cm
        //   = 22.3645 cm = 223.645 mm; d = (223.645 − 5) / 1 = 218.64 m.
        let r = estimate(&ExposureInput { power_mw: 1000.0, aperture_mm: 5.0, divergence_mrad: 1.0, distance_m: 10.0, attenuation: 0.0, exposure_s: 0.25 }).unwrap();
        assert!(close(r.beam_mm as f64, 15.0, 1e-6));
        assert!(close(r.irradiance_mw_cm2 as f64, 565.884, 1e-4), "{r:?}");
        assert!(close(r.ratio as f64, 222.30, 1e-3), "{r:?}");
        assert!(close(r.nohd_m.unwrap() as f64, 218.64, 1e-3), "{r:?}");
    }

    #[test]
    fn attenuation_scales_the_irradiance_but_not_the_nohd() {
        let base = ExposureInput { power_mw: 1000.0, aperture_mm: 5.0, divergence_mrad: 1.0, distance_m: 10.0, attenuation: 0.0, exposure_s: 0.25 };
        let a = estimate(&base).unwrap();
        let b = estimate(&ExposureInput { attenuation: 0.9, ..base }).unwrap();
        assert!(close(b.irradiance_mw_cm2 as f64, a.irradiance_mw_cm2 as f64 * 0.1, 1e-4));
        assert_eq!(a.nohd_m, b.nohd_m);
    }

    #[test]
    fn small_beams_are_averaged_over_the_7mm_aperture() {
        // 1 mW on a 2 mm beam: averaged over 7 mm = 4 / (π × 0.49) =
        // 2.598 mW/cm², just over the MPE (Class 2 ≈ 1 mW is the limit).
        assert!(close(irradiance_mw_cm2(1.0, 2.0), 2.598_4, 1e-4));
        assert_eq!(irradiance_mw_cm2(1.0, 2.0), irradiance_mw_cm2(1.0, 7.0));
        // 0.9 mW through 7 mm: 2.339 mW/cm², under the MPE from the start.
        assert_eq!(nohd_m(0.9, 2.0, 1.0, mpe_mw_cm2(0.25)), Some(0.0));
    }

    #[test]
    fn no_divergence_means_no_safe_distance() {
        assert_eq!(nohd_m(1000.0, 5.0, 0.0, mpe_mw_cm2(0.25)), None);
        let r = estimate(&ExposureInput { power_mw: 1000.0, aperture_mm: 0.0, divergence_mrad: 0.0, distance_m: 100.0, attenuation: 0.0, exposure_s: 0.25 }).unwrap();
        assert!(r.nohd_m.is_none() && r.ratio > 1.0);
    }

    #[test]
    fn untrusted_inputs_are_refused() {
        let ok = ExposureInput { power_mw: 100.0, aperture_mm: 5.0, divergence_mrad: 1.0, distance_m: 10.0, attenuation: 0.0, exposure_s: 0.25 };
        assert!(estimate(&ok).is_ok());
        for bad in [
            ExposureInput { power_mw: f32::NAN, ..ok },
            ExposureInput { power_mw: -1.0, ..ok },
            ExposureInput { aperture_mm: -1.0, ..ok },
            ExposureInput { divergence_mrad: f32::INFINITY, ..ok },
            ExposureInput { distance_m: 0.0, ..ok },
            ExposureInput { attenuation: 1.0, ..ok },
            ExposureInput { exposure_s: 20.0, ..ok },
        ] {
            assert!(estimate(&bad).is_err(), "{bad:?}");
        }
        assert!(ExposureSettings { distance_m: -3.0, ..Default::default() }.validate().is_err());
        assert!(ExposureSettings { distance_m: f32::NAN, ..Default::default() }.validate().is_err());
        assert!(ExposureSettings { attenuation_pct: 100.0, ..Default::default() }.validate().is_err());
        assert!(ExposureSettings::default().validate().is_ok());
        let fixed = ExposureSettings { distance_m: f32::NAN, attenuation_pct: 250.0 }.sanitized();
        assert_eq!(fixed, ExposureSettings { distance_m: 3.0, attenuation_pct: 99.9 });
    }

    fn sheet() -> ProjectorInfo {
        ProjectorInfo { power_mw: [300.0, 400.0, 300.0], wavelength_nm: [638, 520, 450], aperture_mm: 5.0, divergence_mrad: 1.0, ..ProjectorInfo::default() }
    }

    #[test]
    fn the_report_adds_colours_and_applies_the_caps() {
        // 1 W in all, caps 50 % × green clipped at 0.5: 300×0.5 + 400×0.25
        // + 300×0.5 = 400 mW at the output.
        let limits = OutputLimits { max_power: 0.5, max_color: [1.0, 0.5, 1.0] };
        let r = report(&sheet(), &limits, &ExposureSettings { distance_m: 10.0, attenuation_pct: 0.0 });
        assert_eq!((r.power_full_mw, r.power_capped_mw), (1000.0, 400.0));
        let res = r.result.unwrap();
        // 400 mW on 15 mm: 0.4 × 565.884 = 226.35 mW/cm².
        assert!(close(res.irradiance_mw_cm2 as f64, 226.354, 1e-4), "{res:?}");
        assert!(close(r.ratio_full.unwrap() as f64, 222.30, 1e-3));
        assert!(close(r.nohd_full_m.unwrap() as f64, 218.64, 1e-3));
        assert!(close(r.ratio_ilda_static.unwrap() as f64, 226.354 / 2.5, 1e-4));
        // The suggestion is the cap that brings the ratio to 1, and only a
        // suggestion: below the current cap.
        let s = r.suggested_max_power.unwrap();
        assert!(s < limits.max_power && close(s as f64, 0.5 / res.ratio as f64, 1e-6));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(r.disclaimer.contains("ne remplace pas"));
    }

    #[test]
    fn unknowns_are_assumed_on_the_dangerous_side_and_said() {
        let r = report(&ProjectorInfo::default(), &OutputLimits::default(), &ExposureSettings::default());
        assert!(r.result.is_none() && r.ratio_full.is_none());
        assert!(r.warnings.iter().any(|w| w.contains("rien à estimer")));
        let p = ProjectorInfo { power_mw: [0.0, 500.0, 0.0], ..ProjectorInfo::default() };
        let r = report(&p, &OutputLimits::default(), &ExposureSettings::default());
        assert!(r.result.unwrap().nohd_m.is_none());
        for needle in ["Diamètre de sortie non renseigné", "Divergence non renseignée", "Puissance rouge non renseignée", "Longueur d'onde vert non renseignée"] {
            assert!(r.assumptions.iter().any(|a| a.contains(needle)), "{needle}: {:?}", r.assumptions);
        }
        let ir = ProjectorInfo { wavelength_nm: [808, 520, 450], ..sheet() };
        assert!(report(&ir, &OutputLimits::default(), &ExposureSettings::default()).warnings.iter().any(|w| w.contains("808 nm")));
    }

    #[test]
    fn under_the_mpe_suggests_nothing() {
        let p = ProjectorInfo { power_mw: [0.0, 0.5, 0.0], ..sheet() };
        let r = report(&p, &OutputLimits::default(), &ExposureSettings::default());
        assert!(r.result.unwrap().ratio < 1.0);
        assert!(r.suggested_max_power.is_none());
    }
}
