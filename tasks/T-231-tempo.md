---
id: T-231
title: Analyse spectrale : 5 bandes, niveaux dBFS et gain automatique
status: done
area: tempo
priority: P1
depends_on: [T-230]
owner: "agent dev (T-231)"
branch: feat/audio-bands
source: docs/research/audio-analysis.md §2.1 et §6.2
---

## Contexte
Une seule bande « basses » en octets 0..255 ne suffit pas : il faut des bandes stables d'un morceau à l'autre et d'une salle à l'autre.

## À faire
- FFT réelle `realfft` (MIT) N = 1024, hop 256, fenêtre de Hann, plans préalloués.
- Bandes par filtres IIR (biquads, écrits nous-mêmes ou crate `biquad` MIT/Apache) : `sub` 20–60, `bass` 60–150, `low_mid` 150–500, `mid` 500–2000, `high` 2000–12000 Hz ; RMS par hop → dB.
- Gain automatique par bande : plancher = 5e centile sur 10 s, plafond = crête avec relâche 5–10 s ; sortie normalisée 0..1 ; gain manuel optionnel qui le remplace.
- Centroïde spectral (Hz) et platitude spectrale par hop (serviront à T-232 et T-236).
- Silence : `level_db < -60` (réglable) pendant 300 ms → `silent = true`.

## Modèle de données
```rust
pub struct Bands { pub sub: f32, pub bass: f32, pub low_mid: f32, pub mid: f32, pub high: f32 } // 0..1 normalisés
pub struct SpectralFrame { pub t: f64, pub bands: Bands, pub bands_db: [f32; 5], pub level_db: f32,
    pub centroid_hz: f32, pub flatness: f32, pub silent: bool }
#[serde(default)] pub struct AnalysisConfig { pub auto_gain: bool /*true*/, pub manual_gain_db: f32 /*0*/,
    pub silence_db: f32 /*-60*/ }
```

## Interface
Cinq vumètres *Sub / Basses / Bas-médiums / Médiums / Aigus*, case *Gain automatique*, curseur *Gain* (dB), seuil *Silence* (dB).

## Critères d'acceptation
- [x] Sinus 40 Hz → `sub` domine, les autres bandes < 0,1
- [x] Sinus 5 kHz → `high` domine
- [x] Un morceau à −30 dBFS et le même à −10 dBFS donnent des bandes normalisées à ±10 % après 10 s (gain auto)
- [x] Silence numérique → `silent` passe à vrai en 300 ms (±1 hop)
- [x] Coût < 2 % d'un cœur (mesuré en test de performance `--ignored`)

## Tests
Unitaires sur signaux synthétiques (sinus, bruit rose, silence, changements de niveau).

## Notes
Le gain automatique doit remonter lentement après un silence (pas d'explosion du bruit de fond) : geler le plancher/plafond pendant `silent`. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-28 — agent dev : réalisé sur `feat/audio-bands` (PR : `docs/prs/audio-bands.md`). Nouveau `studio/src/audio/spectrum.rs` sur le fil d'analyse : 5 bandes par Butterworth (ordre 8 aux frontières 60/150/500/2000 Hz), dBFS lissés, gain automatique par bande (plafond crête + relâche τ 7 s, plancher 5e centile sur 10 s, plafond d'une bande ≤ 10 dB sous la plus forte), gain manuel, silence (chaque hop < seuil pendant 300 ms, gain auto gelé), FFT `realfft` 1024 / Hann (centroïde, platitude, spectre gardé pour T-232). `AudioConfig.analysis` (`auto_gain`, `manual_gain_db`, `silence_db`), patch champ par champ, sans réouverture de l'entrée. `/api/state.audio.spectral`. `level`/`bass`/`beat` inchangés (T-237 les basculera). Tests : 40 Hz → sub, autres < 0,1 ; 5 kHz → high ; 10 sinus dans leur bande ; −30/−10 dBFS à ±0,1 ; silence en 300 ms ± 1 hop ; gel pendant le silence ; écrêtage/NaN bornés ; aucune allocation par hop ; coût 0,21 % d'un cœur (release, `--ignored`). `cargo test` 508 + 2 OK (4 ignorés), clippy propre, e2e 137/137. Interface (vumètres, cases) laissée à T-243.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
