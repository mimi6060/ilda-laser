---
id: T-166
title: Pack de modèles intégrés (nos propres phrases)
status: todo
area: timeline
priority: P3
depends_on: [T-165]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §6 (C)
---

## Contexte
Avoir tout de suite des phrases utilisables, écrites par nous.

## À faire
8 modèles en code, avec enveloppes de bus : *Montée 8 mesures* (taille et vitesse qui montent, strobe sur la dernière mesure), *Montée 16 mesures*, *Drop 16* (faisceaux, chenillard couleur au temps, calque 2 en flash sur les temps 1), *Pause 8* (graphique lent, couleurs froides, luminosité 60 %), *Couplet 16* (alternance de deux cues toutes les 4 mesures), *Sortie 8* (fondu au noir), *Chenillard couleurs 4*, *Stroboscope final 1*.

## Modèle de données
Utilise `Template` (T-165).

## Interface
Les modèles apparaissent dans le panneau *Modèles* avec un badge *Intégré*.

## Critères d'acceptation
- [ ] Les 8 modèles s'appliquent sans erreur sur un show vide de 64 mesures
- [ ] Chaque modèle a un nom et une description en français

## Tests
Unitaires : chaque modèle se valide et s'applique.

## Notes
Contenu 100 % original (procédural), aucun import d'un autre logiciel. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
