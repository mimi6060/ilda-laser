//! Latency compensation (T-246): the *Décalage de sortie*, saved in
//! `latency.json`, and the figures shown next to it.
//!
//! The offset itself lives on the tempo clock (`TempoClock::effect_beat_at`):
//! the effects read the beat a little ahead, so the light, late by the
//! output's latency, lands on the music's beat. It only changes *what* a
//! frame shows, never *when* it goes out: no frame is held back, and arming,
//! blackout, the e-stop, the strobe limiter and every safety stage keep the
//! real clock and act on the very next frame.
//!
//! The estimate shown is indicative: the engine's frame period plus the
//! frames an output of that kind usually holds in its buffer, at the
//! current point rate (buffer ÷ point rate). The operator calibrates by ear
//! (*Caler à l'oreille*), not by this number.

use crate::tempo::LatencyConfig;
use crate::Shared;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The engine's frame period (ms): a frame is computed, then sent.
const ENGINE_FRAME_MS: f32 = 1_000.0 / 60.0;

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("latency.json")
}

/// `latency.json`, or the defaults (offset 0); a damaged offset is 0.
pub fn load(path: &Path) -> LatencyConfig {
    crate::load_json::<LatencyConfig>(path).sanitized()
}

/// Sets the *Décalage de sortie* from the UI or a controller: refused when
/// not a number, clamped to ±100 ms, saved. Never touches arming.
pub fn set_offset(s: &mut Shared, ms: f64) -> Result<f32, &'static str> {
    let applied = s.tempo.set_output_offset_ms(ms)?;
    if let Some(path) = &s.latency_path {
        crate::save_json(path, &LatencyConfig { output_offset_ms: applied, analysis_delay_ms: 0.0 });
    }
    Ok(applied)
}

/// The output's estimated latency, for the UI.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputEstimate {
    /// `Output::kind`, or `None` for the preview alone.
    pub kind: Option<&'static str>,
    /// Indicative latency (ms); `None` where the offset does not apply
    /// (the ShowNET SD card plays its own files).
    pub ms: Option<f32>,
    /// What the figure is made of, in French.
    pub detail: String,
}

/// Frames an output of this kind usually holds before the light: the DAC's
/// own buffer (Ether Dream, IDN, Helios: about one frame queued behind the
/// one playing), PONK: ours plus MadMapper's. `None`: not a live stream.
fn buffered_frames(kind: Option<&str>) -> Option<f32> {
    match kind {
        None | Some("test") => Some(0.0),
        Some("artnet") => None,
        Some("ponk") => Some(2.0),
        Some(_) => Some(1.0),
    }
}

/// Engine frame + buffered frames × (points per frame ÷ point rate).
pub fn estimate(kind: Option<&'static str>, frame_points: usize, pps: u32) -> OutputEstimate {
    let Some(frames) = buffered_frames(kind) else {
        return OutputEstimate { kind, ms: None, detail: "carte SD : le boîtier joue ses propres fichiers".into() };
    };
    let pps = pps.max(1) as f32;
    // An empty frame (nothing on show) still takes a frame period.
    let frame_ms = if frame_points == 0 { ENGINE_FRAME_MS } else { frame_points as f32 / pps * 1_000.0 };
    let buffer_ms = frames * frame_ms;
    let ms = ENGINE_FRAME_MS + buffer_ms;
    let detail = match kind {
        None | Some("test") => format!("aperçu : une image du moteur ({ENGINE_FRAME_MS:.0} ms)"),
        _ => format!("image du moteur {ENGINE_FRAME_MS:.0} ms + tampon ≈ {buffer_ms:.0} ms ({:.0} points à {:.0} pts/s), indicatif", frames * frame_points as f32, pps),
    };
    OutputEstimate { kind, ms: Some(ms), detail }
}

/// `/api/latency`: the offset, its range, the analysis delay already taken
/// off the detection times and the output's estimate.
pub fn view(s: &Shared) -> serde_json::Value {
    let rate = match s.audio_in.status().sample_rate {
        0 => 48_000,
        r => r,
    };
    let onsets = s.audio_in.analysis_config().onsets;
    let analysis_ms = (crate::audio::onsets::analysis_delay_s(rate, &onsets) * 1_000.0) as f32;
    serde_json::json!({
        "config": LatencyConfig { output_offset_ms: s.tempo.output_offset_ms(), analysis_delay_ms: analysis_ms },
        "min_ms": -crate::tempo::MAX_OUTPUT_OFFSET_MS,
        "max_ms": crate::tempo::MAX_OUTPUT_OFFSET_MS,
        "output": estimate(s.output_kind, s.frame.len(), s.pps),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_is_the_buffer_over_the_point_rate() {
        // 500 points at 30 kpps: 16.7 ms a frame; a DAC holds one more.
        let dac = estimate(Some("dac"), 500, 30_000);
        assert!((dac.ms.unwrap() - (ENGINE_FRAME_MS + 500.0 / 30.0)).abs() < 0.01, "{dac:?}");
        let ponk = estimate(Some("ponk"), 500, 30_000);
        assert!((ponk.ms.unwrap() - (ENGINE_FRAME_MS + 2.0 * 500.0 / 30.0)).abs() < 0.01);
        // Twice the points at the same rate: twice the buffer.
        let slow = estimate(Some("dac"), 1_000, 30_000);
        assert!(slow.ms.unwrap() > dac.ms.unwrap());
        assert!((estimate(None, 0, 30_000).ms.unwrap() - ENGINE_FRAME_MS).abs() < 0.01, "preview: the engine frame");
        assert_eq!(estimate(Some("artnet"), 500, 30_000).ms, None, "the SD card plays its own files");
        assert!(estimate(Some("dac"), 10, 0).ms.unwrap().is_finite(), "no rate: no division by zero");
    }

    #[test]
    fn the_offset_is_saved_and_loaded_clamped() {
        let dir = std::env::temp_dir().join(format!("laser-studio-latency-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = crate::test_support::shared();
        s.latency_path = Some(path(&dir));
        assert_eq!(load(&path(&dir)), LatencyConfig::default(), "no file: 0 ms");
        assert_eq!(set_offset(&mut s, 42.0), Ok(42.0));
        assert_eq!(load(&path(&dir)).output_offset_ms, 42.0);
        assert!(set_offset(&mut s, f64::NAN).is_err());
        assert_eq!(load(&path(&dir)).output_offset_ms, 42.0, "a refused value is not saved");
        std::fs::write(path(&dir), r#"{ "output_offset_ms": -1e9 }"#).unwrap();
        assert_eq!(load(&path(&dir)).output_offset_ms, -100.0);
        std::fs::write(path(&dir), r#"{ "output_offset_ms": "x" }"#).unwrap();
        assert_eq!(load(&path(&dir)).output_offset_ms, 0.0, "a damaged file: the default");
        let v = view(&s);
        assert_eq!(v["config"]["output_offset_ms"], 42.0);
        assert!(v["config"]["analysis_delay_ms"].as_f64().unwrap() > 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
