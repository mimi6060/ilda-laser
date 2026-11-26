---
id: T-164
title: Éditeur de cue évolutif
status: todo
area: ui
priority: P2
depends_on: [T-157, T-162]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §6 (B)
---

## Contexte
Créer et retoucher un cue évolutif sans écrire de JSON.

## À faire
- Ouvert par *Modifier le programme* dans les propriétés d'une case. Réutilise le composant timeline de T-162 en échelle de temps (temps/mesures).
- Rangée *Étapes* (blocs de contenu, glisser pour déplacer, bords pour la transition), une rangée par *Courbe* (clic = ajouter une image-clé, glisser = déplacer, clic droit = supprimer, menu de courbe), liste *Modulateurs*.
- *Longueur* (4, 8, 16, 32, 64 temps), *Lecture* (Boucle / Une fois / Aller-retour).
- Aperçu en direct en boucle pendant l'édition ; *Enregistrer* / *Annuler*.

## Modèle de données
Utilise `CueProgram` (T-157) et les clés `Content::Evolving` de T-111 (les deux sont éditables ici).

## Interface
Libellés : *Éditeur de cue*, *Étapes*, *Courbes*, *Ajouter une courbe*, *Modulateurs*, *Longueur*, *Lecture*, *Boucle*, *Une fois*, *Aller-retour*, *Enregistrer*, *Annuler*.

## Critères d'acceptation
- [ ] Ajouter une courbe *Taille* et deux clés → l'aperçu change
- [ ] Magnétisme sur les temps par défaut (désactivable avec Alt)
- [ ] *Annuler* ne modifie pas le cue

## Tests
e2e : créer un programme, l'enregistrer, relancer le cue, vérifier `/api/frame` à deux instants.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
