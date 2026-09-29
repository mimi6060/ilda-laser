---
id: T-236
title: Détection montée / drop / break et silence (sections musicales)
status: review
area: tempo
priority: P2
depends_on: [T-231, T-232]
owner: "dev-agent (T-236)"
branch: feat/audio-sections
source: docs/research/audio-analysis.md §2.6
---

## Contexte
Ce sont exactement les moments où l'opérateur change de look à la main : les détecter permet l'automatisation (T-240) et l'aide au jockey.

## À faire
- Tendances lissées sur 1–8 s : énergie basse par rapport à sa moyenne sur 16 temps, pente du centroïde spectral, énergie aiguë, densité d'onsets (roulements de caisse).
- *Break* : basses < −10 dB sous la moyenne et plus de kick depuis ≥ 2 temps, niveau global maintenu.
- *Montée* : pente du centroïde > 0 et aigus / densité d'onsets en hausse sur 4–16 temps, basses encore réduites → valeur continue `buildup` 0..1.
- *Drop* : après un break ou une montée, basses de retour (+8 dB en ≤ 1 temps) et kicks qui reprennent → événement `drop` (compteur) ; aligné sur le temps le plus proche si le tempo est verrouillé.
- *Silence* (T-231) → état `Silence`.

## Modèle de données
```rust
pub enum Section { Silence, Normal, Break, Buildup, Drop }
pub struct SectionState { pub section: Section, pub buildup: f32, pub drop: u64, pub last_drop_t: f64, pub since_s: f32 }
```

## Interface
Bandeau d'état *Silence / Normal / Break / Montée / Drop* avec jauge *Montée* ; historique des 5 derniers changements (heure, section).

## Critères d'acceptation
- [x] Scénario synthétique 16 temps normal → 8 temps break → 8 temps montée (bruit filtré montant) → drop : chaque section détectée avec ≤ 1 temps de retard, drop à ±1 temps
- [x] Un simple changement de volume global ne déclenche ni break ni drop
- [x] `buildup` croît de façon monotone (à la tolérance près) pendant la montée synthétique

## Tests
Unitaires sur scénarios générés ; test de non-déclenchement sur morceau stationnaire (bruit rose + kicks).

## Notes
Valeurs volontairement prudentes : un faux drop est pire qu'un drop manqué. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-29 — dev-agent (T-236), branche `feat/audio-sections` : `audio/sections.rs` (`SectionDetector` sur le fil d'analyse ; tendances en temps, références du groove, break / montée / drop / silence, confiance, historique des 5 derniers changements), publié dans `/api/state.audio.sections` et dans `AudioFeatures.section / buildup / drop` (enum `engine::Section` de T-237 réutilisé). Détection seulement : aucun look ne réagit. Scénarios synthétiques à 90, 128 et 174 BPM : break +0,78 à +0,91 temps, montée +71 à +85 ms, drop signalé +17 à +20 ms et placé à ±3 ms du temps ; pas de faux break / drop sur groove stable, bruit rose + kicks, changements de volume (−12 et −20 dB), groove qui revient en fondu ; aucune allocation par hop. Écarts : kick manquant à 1,75 période de kick (et non 2 temps) pour rester sous 1 temps de retard ; les règles de kick comptent en période de kick mesurée (tempo lu à l'octave). `cargo test` 642 OK, clippy OK, e2e 166/166 deux fois (rebasé sur develop 65ff47c, T-237, T-234 et T-162 inclus). UI (bandeau, jauge, historique) : T-243.
