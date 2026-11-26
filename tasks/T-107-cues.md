---
id: T-107
title: Textures : ciel étoilé, éclairs, faisceaux épais
status: todo
area: cues
priority: P2
depends_on: [T-100]
owner: ""
branch: ""
source: docs/research/festival-looks.md#d-mirrors-geometry-and-stage-bound-looks
---

## Contexte
Des textures pour la tension avant un drop (étoiles qui s'accélèrent), les impacts (éclairs) et un faux « BeamBrush » (faisceau épais) sans matériel spécial.

## À faire
- `starfield` (look 30) : K faisceaux à positions aléatoires (seedées par numéro de pas), au-dessus de l'horizon ; nouveau tirage à chaque pas (`steps_per_beat`) ; durée de vie 1/8–1/4 temps.
- `lightning` (look 31) : ligne brisée aléatoire (6–10 segments) du haut vers un point bas, visible 2 à 4 images puis éteinte ; déclenchée sur chaque temps ou chaque mesure (`a`).
- `fat_beams` (look 32) : 4–6 positions de fan, chacune dessinée comme un petit cercle r = 0.01–0.03, pour épaissir le faisceau dans la fumée.

## Modèle de données
Réutilise `count`, `a`, `b`, `steps_per_beat`. Graine = hash(numéro de pas, index) pour un rendu déterministe.

## Interface
Libellés : « Ciel étoilé », « Éclairs », « Faisceaux épais ».

## Critères d'acceptation
- [ ] `starfield` : même graine + même pas → mêmes positions (déterministe)
- [ ] `lightning` éteint en dehors de sa fenêtre de 2 à 4 images
- [ ] `fat_beams` : chaque faisceau est un cercle fermé de rayon demandé

## Tests
Unitaires : déterminisme, fenêtre d'éclair, rayon des cercles.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
