---
id: T-002
title: Optimiseur de points (tracé laser pro)
status: todo
area: output
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/research/madmapper-and-open-content.md
---

## Contexte
Moins de scintillement, coins nets, déplacements éteints plus doux : c'est ce qui distingue un rendu pro.

## À faire
Nouveau `studio/src/optimize.rs` qui remplace `engine::densify`. Segments allumés découpés à `lit_step` ; déplacements éteints avec easing (smoothstep) et pas plus grands ; arrêt aux angles = ceil(angle / 0.6 rad) plafonné ; ordre des traits stable (plus proche voisin, inversion possible). Ne pas dupliquer le blanking inter-frames de laser-dac.

## Modèle de données
`OptimizeParams { lit_step: 0.025, blank_step: 0.15, corner_rad_per_point: 0.6, max_corner_dwell: 8, endpoint_dwell: 3 }` (serde default).

## Interface
Plus tard : réglages avancés par sortie.

## Critères d'acceptation
- [ ] Pas max respecté sur les segments allumés
- [ ] Déplacements éteints plus lents aux extrémités qu'au milieu, et éteints
- [ ] Carré : 3 points d'arrêt par coin ; cercle : aucun
- [ ] Ordre déterministe
- [ ] Tous les cues restent ≥ 30 im/s à 30 kpps

## Tests
Unitaires + mesure des 202 cues via l'API.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
