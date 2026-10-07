//! Input health (T-242): is the native input really delivering music?
//!
//! On macOS a refused microphone permission (TCC) often opens fine and
//! then delivers *exact* zeros, without any error: the operator believes
//! the laser "doesn't react". The analysis thread runs every hop through a
//! `HealthMeter`; the capture state (no device, device lost, refusal
//! reported by the backend) completes the picture in `combine`.
//!
//! Detection and explanation only: the studio never touches the privacy
//! settings, and the health changes nothing in what the engine does (no
//! arming, the audio flash limiter stays as it is).

use super::analysis::to_db;
use super::CaptureState;
use serde::{Deserialize, Serialize};

/// Exact zeros for longer than this (s): *Autorisation refusée ?*.
pub const ZERO_SUSPECT_S: f64 = 2.0;
/// A peak at or above this (dBFS) is *Entrée trop forte*.
pub const CLIP_DB: f32 = -0.5;
/// A mean level under this (dBFS) is *Entrée trop faible*.
pub const QUIET_DB: f32 = -45.0;
/// Time constant (s) of the mean level.
pub const MEAN_TAU_S: f64 = 1.0;
/// A clip stays shown this long (s), so a single peak is readable.
pub const CLIP_HOLD_S: f64 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputHealth {
    #[default]
    Ok,
    /// No input device on the Mac.
    NoDevice,
    /// The chosen input vanished (unplugged).
    DeviceLost,
    /// Refused by macOS, or exact zeros for more than 2 s.
    PermissionSuspected,
    /// Mean level under −45 dBFS (but above the silence threshold).
    TooQuiet,
    /// Peaks at −0.5 dBFS or more.
    Clipping,
    /// Background noise only: the input works, no music arrives.
    Silent,
}

impl InputHealth {
    /// The badge text (French UI).
    pub fn label(self) -> &'static str {
        match self {
            InputHealth::Ok => "Signal correct",
            InputHealth::NoDevice => "Aucune entrée audio",
            InputHealth::DeviceLost => "Entrée perdue",
            InputHealth::PermissionSuspected => "Autorisation refusée ?",
            InputHealth::TooQuiet => "Entrée trop faible",
            InputHealth::Clipping => "Entrée trop forte",
            InputHealth::Silent => "Silence",
        }
    }

    /// Badge colour: `ok` (green), `warn` (orange), `bad` (red).
    pub fn tone(self) -> &'static str {
        match self {
            InputHealth::Ok => "ok",
            InputHealth::TooQuiet | InputHealth::Clipping | InputHealth::Silent => "warn",
            InputHealth::NoDevice | InputHealth::DeviceLost | InputHealth::PermissionSuspected => "bad",
        }
    }

    /// What the operator can do about it (French UI). The studio never
    /// changes the privacy settings itself.
    pub fn help(self) -> &'static str {
        match self {
            InputHealth::Ok => "L'entrée reçoit du son dans la zone conseillée (−25 à 0 dBFS).",
            InputHealth::NoDevice => "Aucune entrée audio sur ce Mac : branchez un micro ou une carte son, puis vérifiez Réglages Système › Son › Entrée.",
            InputHealth::DeviceLost => "L'entrée choisie a disparu (débranchée ?). Le studio réessaie toutes les 2 s : rebranchez-la ou choisissez-en une autre.",
            InputHealth::PermissionSuspected => {
                "L'entrée ne renvoie que des zéros exacts : macOS a probablement refusé l'accès au micro. \
                 Ouvrez Réglages Système › Confidentialité et sécurité › Microphone, activez Laser Studio \
                 (ou l'application qui a lancé le studio : Terminal, iTerm…), puis relancez le studio. \
                 Une entrée numérique sans aucun son (BlackHole, carte son coupée) donne le même résultat."
            }
            InputHealth::TooQuiet => "Niveau moyen sous −45 dBFS : montez le gain de la carte son ou du micro (zone conseillée −25 à 0 dBFS).",
            InputHealth::Clipping => "Des crêtes atteignent −0,5 dBFS : baissez le gain de la carte son ou le volume envoyé. La saturation fausse l'analyse ; le limiteur de flashs audio reste actif.",
            InputHealth::Silent => "Bruit de fond seulement : l'entrée fonctionne mais aucune musique n'arrive.",
        }
    }

    /// Music is coming in (perhaps too quiet or too loud).
    pub fn signal_present(self) -> bool {
        matches!(self, InputHealth::Ok | InputHealth::TooQuiet | InputHealth::Clipping)
    }
}

/// The health shown for a capture `state` and the analysis' own verdict
/// (`signal`, from a fresh snapshot). `None`: nothing to say (capture off,
/// or running with no fresh analysis yet).
pub fn combine(state: CaptureState, signal: Option<InputHealth>) -> Option<InputHealth> {
    match state {
        CaptureState::NoDevice => Some(InputHealth::NoDevice),
        CaptureState::DeviceLost => Some(InputHealth::DeviceLost),
        CaptureState::PermissionDenied => Some(InputHealth::PermissionSuspected),
        CaptureState::Running => signal,
        CaptureState::Off | CaptureState::Disabled | CaptureState::Error => None,
    }
}

/// Classifies the signal hop by hop (the analysis thread, one per stream).
#[derive(Clone, Debug, Default)]
pub struct HealthMeter {
    /// Seconds of exact zeros in a row.
    zero_s: f64,
    /// Mean square, smoothed over `MEAN_TAU_S` (`None` before the first hop).
    mean_sq: Option<f64>,
    /// Seconds since the last clipped peak (`None`: never).
    since_clip: Option<f64>,
}

impl HealthMeter {
    /// One hop of mono samples lasting `dt` seconds; `silence_db` is the
    /// analysis' silence threshold. Returns the current verdict.
    pub fn hop(&mut self, samples: &[f32], dt: f64, silence_db: f32) -> InputHealth {
        if samples.is_empty() {
            return self.verdict(silence_db);
        }
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        for &s in samples {
            peak = peak.max(s.abs());
            sum_sq += f64::from(s) * f64::from(s);
        }
        let hop_sq = sum_sq / samples.len() as f64;
        self.zero_s = if peak == 0.0 { self.zero_s + dt } else { 0.0 };
        let k = 1.0 - (-dt.max(0.0) / MEAN_TAU_S).exp();
        self.mean_sq = Some(match self.mean_sq {
            Some(m) => m + (hop_sq - m) * k,
            None => hop_sq,
        });
        self.since_clip = if to_db(peak * peak) >= CLIP_DB { Some(0.0) } else { self.since_clip.map(|s| s + dt) };
        self.verdict(silence_db)
    }

    fn verdict(&self, silence_db: f32) -> InputHealth {
        let mean_db = to_db(self.mean_sq.unwrap_or(0.0) as f32);
        if self.zero_s > ZERO_SUSPECT_S {
            InputHealth::PermissionSuspected
        } else if self.since_clip.is_some_and(|s| s < CLIP_HOLD_S) {
            InputHealth::Clipping
        } else if mean_db < silence_db {
            InputHealth::Silent
        } else if mean_db < QUIET_DB {
            InputHealth::TooQuiet
        } else {
            InputHealth::Ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::testsig::sine;
    use super::super::analysis::HOP;
    use super::*;

    const RATE: u32 = 48_000;
    const DT: f64 = HOP as f64 / RATE as f64;

    /// White noise at `db` dBFS RMS (a fixed LCG: reproducible).
    fn noise(db: f32, n: usize, seed: &mut u32) -> Vec<f32> {
        // Uniform in −a..a has RMS a / √3.
        let a = 10f32.powf(db / 20.0) * 3f32.sqrt();
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                a * ((*seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    /// Runs `seconds` of `signal` through a meter; returns each verdict
    /// with its time.
    fn run(m: &mut HealthMeter, seconds: f64, signal: &[f32]) -> Vec<(f64, InputHealth)> {
        let hops = (seconds / DT) as usize;
        (0..hops).map(|h| ((h + 1) as f64 * DT, m.hop(&signal[(h * HOP) % (signal.len() - HOP + 1)..][..HOP], DT, -60.0))).collect()
    }

    #[test]
    fn exact_zeros_read_as_a_refused_permission_within_3_s() {
        let mut m = HealthMeter::default();
        let zeros = vec![0.0f32; RATE as usize];
        let verdicts = run(&mut m, 3.0, &zeros);
        let first = verdicts.iter().find(|(_, h)| *h == InputHealth::PermissionSuspected).expect("suspected").0;
        assert!((ZERO_SUSPECT_S - DT / 2.0..=3.0).contains(&first), "after 2 s, before 3 s: {first}");
        assert!(verdicts.iter().take_while(|(t, _)| *t < ZERO_SUSPECT_S - DT / 2.0).all(|(_, h)| *h == InputHealth::Silent), "silence before that");
        // One sample of sound ends it at once.
        let mut blip = vec![0.0f32; HOP];
        blip[3] = 1e-4;
        assert_eq!(m.hop(&blip, DT, -60.0), InputHealth::Silent);
    }

    #[test]
    fn noise_at_minus_70_is_silence_not_a_refusal() {
        let mut m = HealthMeter::default();
        let bed = noise(-70.0, RATE as usize * 6, &mut 7);
        let verdicts = run(&mut m, 5.0, &bed);
        assert!(verdicts.iter().all(|(_, h)| *h == InputHealth::Silent), "{:?}", verdicts.iter().find(|(_, h)| *h != InputHealth::Silent));
    }

    #[test]
    fn a_full_scale_sine_is_too_loud_and_a_normal_one_is_ok() {
        let mut m = HealthMeter::default();
        let loud = sine(440.0, 1.0, RATE, RATE as usize * 2, 0);
        assert_eq!(run(&mut m, 1.0, &loud).last().unwrap().1, InputHealth::Clipping);
        // −12 dBFS peaks: back to Ok once the clip hold has passed.
        let fine = sine(440.0, 0.25, RATE, RATE as usize * 2, 0);
        let verdicts = run(&mut m, 1.5, &fine);
        assert_eq!(verdicts[0].1, InputHealth::Clipping, "held");
        assert_eq!(verdicts.last().unwrap().1, InputHealth::Ok);
        // A peak just under −0.5 dBFS is not a clip.
        let mut m = HealthMeter::default();
        let under = sine(440.0, 0.93, RATE, RATE as usize, 0);
        assert_eq!(run(&mut m, 0.5, &under).last().unwrap().1, InputHealth::Ok);
    }

    #[test]
    fn a_weak_signal_is_too_quiet() {
        let mut m = HealthMeter::default();
        // −50 dBFS RMS: over the −60 silence, under −45.
        let weak = noise(-50.0, RATE as usize * 3, &mut 11);
        assert_eq!(run(&mut m, 2.0, &weak).last().unwrap().1, InputHealth::TooQuiet);
        // The silence threshold is the analysis' own.
        assert_eq!(m.hop(&weak[..HOP], DT, -40.0), InputHealth::Silent);
    }

    #[test]
    fn the_capture_state_comes_first() {
        use CaptureState as S;
        assert_eq!(combine(S::NoDevice, Some(InputHealth::Ok)), Some(InputHealth::NoDevice));
        assert_eq!(combine(S::DeviceLost, None), Some(InputHealth::DeviceLost));
        assert_eq!(combine(S::PermissionDenied, None), Some(InputHealth::PermissionSuspected));
        assert_eq!(combine(S::Running, Some(InputHealth::Clipping)), Some(InputHealth::Clipping));
        assert_eq!(combine(S::Running, None), None);
        for s in [S::Off, S::Disabled, S::Error] {
            assert_eq!(combine(s, Some(InputHealth::Ok)), None);
        }
    }

    #[test]
    fn every_state_is_explained_in_french_with_a_colour() {
        let all = [InputHealth::Ok, InputHealth::NoDevice, InputHealth::DeviceLost, InputHealth::PermissionSuspected, InputHealth::TooQuiet, InputHealth::Clipping, InputHealth::Silent];
        for h in all {
            assert!(!h.label().is_empty() && h.help().len() > 30, "{h:?}");
            assert!(["ok", "warn", "bad"].contains(&h.tone()));
        }
        let help = InputHealth::PermissionSuspected.help();
        assert!(help.contains("Confidentialité et sécurité › Microphone"));
        // Explains only: never a command that changes the privacy settings.
        assert!(!help.contains("tccutil") && !help.contains("sudo"));
        assert_eq!(serde_json::to_value(InputHealth::PermissionSuspected).unwrap(), "permission_suspected");
        assert!(InputHealth::Clipping.signal_present() && !InputHealth::Silent.signal_present() && !InputHealth::PermissionSuspected.signal_present());
    }
}
