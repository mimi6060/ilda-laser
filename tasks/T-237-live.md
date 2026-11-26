---
id: T-237
title: AudioFeatures v2 : instantané complet côté moteur et dans /api/state
status: done
area: live
priority: P1
depends_on: [T-231]
owner: "dev-agent (T-237)"
branch: feat/audio-features-v2
source: docs/research/audio-analysis.md §2 et §3
---

## Contexte
Toutes les analyses doivent arriver au moteur par une seule structure, lue une fois par trame, et être visibles par l'interface sans refaire l'analyse dans le navigateur.

## À faire
- Étendre `AudioFeatures` (compatible : `#[serde(default)]`, `level`, `bass`, `beat` conservés et toujours remplis) avec les bandes, onsets, section, silence, horodatage.
- Publication sans blocage du fil d'analyse vers le moteur (mutex tenu quelques µs ou échange atomique d'instantané) ; le moteur lit l'instantané au début de chaque trame.
- Instantané périmé (> 500 ms) → valeurs neutres qui **décroissent** (release) au lieu de geler.
- `/api/state.audio` : bandes, dBFS, compteurs, section, tempo détecté ; `GET /api/audio/spectrum` (64 bandes log) pour l'affichage, à la demande.
- `POST /api/audio` (source *Navigateur*) accepte l'ancien format et le nouveau.

## Modèle de données
```rust
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFeatures { pub level: f32, pub bass: f32, pub beat: u64,           // hérités
    pub level_db: f32, pub bands: Bands, pub onset: u64, pub kick: u64, pub snare: u64, pub hat: u64,
    pub kick_strength: f32, pub centroid_hz: f32, pub silent: bool,
    pub section: Section, pub buildup: f32, pub drop: u64, pub t: f64 }
```

## Interface
L'interface lit `/api/state.audio` pour tous les vumètres (plus d'analyse locale quand la source est *Native*).

## Critères d'acceptation
- [x] Une scène sauvegardée avant la tâche charge et rend à l'identique (test de non-régression)
- [x] `POST /api/audio {level, bass, beat}` (ancien format) fonctionne toujours
- [x] Fil d'analyse arrêté : en ≤ 1 s les valeurs retombent à neutre, sans saut brutal
- [x] `/api/state.audio.bands` présent et borné 0..1

## Tests
Unitaires : sérialisation ancien/nouveau format, décroissance à la péremption. e2e : `POST /api/audio` nouveau format → `/api/state.audio`.

## Notes
`bass` hérité = `bass` normalisé (T-231) ; `beat` hérité = `kick` en source native. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-29 — agent de développement (branche `feat/audio-features-v2`) : `AudioFeatures` v2 (bandes, onsets, BPM détecté + confiance, `Section`, `buildup`, `drop`, `silent`, `t`), remplie par l'analyse native et par `POST /api/audio` (ancien et nouveau format). `AudioHub::frame` : instantané périmé → retour au neutre en τ 100 ms (neutre en ≤ 1 s). `/api/state.audio` : `bands`, `counters`, `signals`, `section`, `detected_bpm`… ; `GET /api/audio/spectrum` (64 bandes log). `bass` hérité natif = max(sub, bass) normalisés. 610 tests unitaires, clippy propre, e2e 153/153. Note : `docs/prs/audio-features-v2.md`. Statut `review`.
- 2026-09-29 — architecte (review) : APPROUVÉ et fusionné dans develop.
