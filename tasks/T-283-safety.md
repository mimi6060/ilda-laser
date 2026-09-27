---
id: T-283
title: Mode spectacle (verrouillage) et protection contre les clics accidentels
status: todo
area: safety
priority: P1
depends_on: [T-270]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#26-avoiding-accidents
---

## Contexte
Pendant un show, un clic de travers (supprimer une scène, écraser un réglage, changer la calibration) peut casser la soirée. Les consoles pros séparent le jeu de l'édition et demandent une confirmation pour les actions destructives.

## À faire
- Bouton *Mode spectacle* (cadenas) dans la barre du haut. Activé : le Panneau n'affiche que *Direct* ; édition de cues, renommage, suppression, glisser-déposer de la grille, calibration, lieu, *Enregistrer sous*/écrasement désactivés (masqués ou grisés). Restent actifs : cues, pages, maîtres, tempo, noir, armement.
- Déverrouiller demande un **appui long de 1 s** (anneau de progression) : pas de simple clic.
- Actions destructives hors mode spectacle (supprimer une scène/cue/page, réinitialiser la calibration, ouvrir un projet avec modifications non enregistrées) : confirmation (*Maintenir pour confirmer* 800 ms ou boîte de dialogue), jamais un simple clic.
- Le mode spectacle est exposé à l'API (`GET/POST /api/lock`) pour que le MIDI (T-202) ne puisse pas non plus déclencher d'édition quand il est actif.
- État enregistré dans le projet (T-286) ; au démarrage, reprendre le dernier état.

## Modèle de données
```rust
#[serde(default)]
pub struct UiLock { pub show_mode: bool }
```
Dans `Shared` ; les routes d'édition (`/api/scenes/delete`, `/api/scenes/save`, `/api/calibration`, `/api/grid`, `/api/venue`, …) renvoient 423 *Locked* quand `show_mode` est vrai.

## Interface
Libellés : *Mode spectacle*, *Maintenir pour déverrouiller*, *Maintenir pour confirmer*, *Verrouillé en mode spectacle*.

## Critères d'acceptation
- [ ] En mode spectacle, `POST /api/scenes/delete` renvoie 423 et la scène existe toujours
- [ ] En mode spectacle, cliquer une cue la joue ; Échap et *Noir* marchent ; Espace arme/désarme comme avant
- [ ] Un simple clic sur le cadenas ne déverrouille pas ; un appui de 1 s déverrouille
- [ ] Supprimer une scène hors mode spectacle demande une confirmation

## Tests
Unitaires Rust : routes d'édition refusées quand verrouillé, routes de jeu acceptées. e2e : verrouiller, tenter une suppression, déverrouiller par appui long.

## Notes
Le verrou ne doit jamais bloquer le noir, le désarmement ni les modificateurs de sécurité. Il n'arme jamais le laser. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
