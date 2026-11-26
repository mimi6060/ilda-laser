---
id: T-108
title: Graphismes : compte à rebours, anneau de progression, fil de fer 3D
status: todo
area: cues
priority: P2
depends_on: [T-100]
owner: ""
branch: ""
source: docs/research/festival-looks.md#e-graphics-text-and-abstracts-projected-on-screens-scrims-smoke
---

## Contexte
Pour les intros de DJ et les montées : un compte à rebours 4-3-2-1, un anneau qui se ferme à l'approche du drop, et des objets 3D en fil de fer.

## À faire
- `countdown` (look 34) : affiche N, N−1, … 1 avec `font.rs`, un chiffre par pas (`steps_per_beat`, défaut 1 par mesure = 0.25) ; éteint pendant le dernier 1/2 temps.
- `clock` : mode `beat_sync` où l'anneau se remplit sur `period_beats` (0 → 360°) au lieu de l'heure.
- `wire3d` (look 36) : cube, octaèdre, icosaèdre en fil de fer, projection perspective simple, rotation 1 tour / 2 mesures sur deux axes.

## Modèle de données
`countdown` : `count` = nombre de départ (défaut 4). `wire3d` : `a` = solide (0 cube, 1 octaèdre, 2 icosaèdre). Arêtes dédupliquées, trajets éteints entre arêtes non contiguës.

## Interface
Libellés : « Compte à rebours », « Anneau de progression », « Objet 3D ».

## Critères d'acceptation
- [ ] `countdown` affiche 4,3,2,1 aux bons pas puis noir
- [ ] `clock` en mode beat : 50 % rempli à `period_beats/2`
- [ ] `wire3d` : tous les points dans -1..1 à toute rotation

## Tests
Unitaires : chiffre affiché par pas, remplissage de l'anneau, bornes du 3D.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
