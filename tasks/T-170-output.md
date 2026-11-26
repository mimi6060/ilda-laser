---
id: T-170
title: Groupes de projecteurs et chenillard entre zones
status: todo
area: output
priority: P3
depends_on: [T-012, T-150]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §5
---

## Contexte
Avec plusieurs lasers, faire passer l'effet d'un projecteur à l'autre au rythme (gauche, centre, droite…), comme le Chaser de Showcontroller ou le Zone Chase de BEYOND.

## À faire
- Groupes nommés de zones (jusqu'à 32), ex. *Centre*, *Côtés*, *Tous*.
- Chenillard : liste ordonnée de groupes, pas en temps (1/4..4), mode *Avant*, *Aller-retour*, *Aléatoire* ; *libre* (réglage global) ou *lié au cue* (enregistré dans la case ou dans l'événement de timeline).
- Le routage cue → zones de T-012 reste la base ; le chenillard sélectionne le sous-ensemble actif à chaque pas.

## Modèle de données
```rust
pub struct ZoneGroup { pub name: String, pub zones: Vec<usize> }
pub struct Chaser { pub name: String, pub steps: Vec<usize /*groupe*/>, pub step: Rate /*Beats(1.0)*/, pub mode: ChaseMode }
```

## Interface
Section *Sorties* : *Groupes*, *Chenillards*, vitesse ; contrôles `chaser.<n>.enabled`, `chaser.rate`.

## Critères d'acceptation
- [ ] Chenillard 3 groupes au pas d'un temps : le groupe actif change exactement à chaque temps
- [ ] Chenillard désactivé : routage normal
- [ ] Aucune zone ne reçoit de points si elle est hors du groupe actif

## Tests
Unitaires avec horloge simulée.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
