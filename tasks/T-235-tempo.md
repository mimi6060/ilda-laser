---
id: T-235
title: Détection du temps fort (début de mesure) et des phrases de 8/16 mesures
status: todo
area: tempo
priority: P2
depends_on: [T-234, T-236]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §2.5
---

## Contexte
Les cues évolutifs et la timeline raisonnent en mesures et en phrases. Deviner le « 1 » évite au jockey de faire Resync à chaque morceau.

## À faire
- Pour chaque position de temps modulo 4 : moyenne glissante (8 mesures) de la force d'onset basse bande et du changement harmonique (flux de chroma, 12 classes calculées depuis la FFT) ; le « 1 » = la position la plus forte.
- Confiance = écart entre la meilleure position et la deuxième.
- Les frontières de sections de T-236 (drop, fin de break) tombent sur un « 1 » de phrase : si la confiance est haute, y aligner la mesure.
- Ne recaler la mesure que lors d'une transition confiance basse → haute ou sur un drop ; sinon afficher la suggestion.

## Modèle de données
```rust
pub struct DownbeatEstimate { pub beat_in_bar_offset: u8, pub confidence: f32, pub phrase_start: Option<f64> }
```

## Interface
Voyant du temps 1 suggéré (contour pointillé) dans la barre de tempo, texte *Temps 1 suggéré*, bouton *Accepter* (= Resync sur la suggestion).

## Critères d'acceptation
- [ ] Motif synthétique avec kick plus fort et accord qui change au temps 1 : suggestion correcte en ≤ 8 mesures
- [ ] Motif sans accent : confiance < 0,3 et aucun recalage automatique
- [ ] Un recalage automatique n'a lieu qu'au moment d'un drop ou d'une montée de confiance

## Tests
Unitaires sur motifs générés (accent sur 1, accent sur 3, sans accent, drop après 16 mesures).

## Notes
Fiabilité limitée par nature : le Resync manuel (T-150) reste la référence. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
