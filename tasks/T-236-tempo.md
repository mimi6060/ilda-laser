---
id: T-236
title: Détection montée / drop / break et silence (sections musicales)
status: todo
area: tempo
priority: P2
depends_on: [T-231, T-232]
owner: ""
branch: ""
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
- [ ] Scénario synthétique 16 temps normal → 8 temps break → 8 temps montée (bruit filtré montant) → drop : chaque section détectée avec ≤ 1 temps de retard, drop à ±1 temps
- [ ] Un simple changement de volume global ne déclenche ni break ni drop
- [ ] `buildup` croît de façon monotone (à la tolérance près) pendant la montée synthétique

## Tests
Unitaires sur scénarios générés ; test de non-déclenchement sur morceau stationnaire (bruit rose + kicks).

## Notes
Valeurs volontairement prudentes : un faux drop est pire qu'un drop manqué. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
