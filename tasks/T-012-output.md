---
id: T-012
title: Zones de projection et correction géométrique
status: todo
area: output
priority: P2
depends_on: [T-003]
owner: ""
branch: ""
source: docs/research/pangolin.md
---

## Contexte
Chaque laser a sa zone, avec trapèze/coussin/4 coins ; les cues choisissent leurs zones.

## À faire
Correction géométrique par sortie (keystone, pincushion, bow, warp 4 coins) ; routage cue → zones ; plusieurs sorties.

## Modèle de données
`ProjectionZone { name, output, geometry, safety }`.

## Interface
Section « Zones ».

## Critères d'acceptation
- [ ] Warp 4 coins exact aux coins
- [ ] Un cue peut jouer sur plusieurs zones

## Tests
Unitaires géométrie.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
