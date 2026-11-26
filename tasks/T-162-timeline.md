---
id: T-162
title: Éditeur de timeline (pistes, glisser, magnétisme, zoom, marqueurs, copier-coller)
status: todo
area: ui
priority: P2
depends_on: [T-160, T-161]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.1, §3.3
---

## Contexte
Monter un show vite : poser des cues au temps, les déplacer, les dupliquer phrase par phrase.

## À faire
- Vue timeline (canvas) : règle en mesures/temps et en secondes, forme d'onde, pistes, événements colorés par calque.
- Glisser un cue depuis la grille vers une piste ; déplacer/redimensionner ; `Alt`+glisser = dupliquer.
- Magnétisme : *Fort* (temps/mesures), *Moyen* (quelques pixels), *Off* ; aimant sur marqueurs ; déplacer un marqueur déplace les événements aimantés.
- Marqueurs (couleur, nom) ; les lettres restent des cues, donc on pose un marqueur pendant la lecture avec `Entrée` **uniquement quand la timeline a le focus** (sinon Entrée = tap tempo, T-150).
- Région de boucle (poignées dans la règle) ; zoom molette centré sur le curseur ; *Tout afficher*.
- Copier/coller d'une sélection (phrase) collée au prochain début de mesure après la tête de lecture.
- Annuler/rétablir (Cmd+Z / Cmd+Maj+Z) sur 50 niveaux.

## Modèle de données
Utilise `Show` (T-160).

## Interface
Libellés : *Timeline*, *Nouveau show*, *Ouvrir*, *Enregistrer*, *Lecture*, *Pause*, *Arrêt*, *Boucle*, *Magnétisme : Fort / Moyen / Off*, *Ajouter un marqueur*, *Tout afficher*, *Copier la phrase*, *Coller*.

## Critères d'acceptation
- [ ] Glisser un cue sur la piste 1 à la mesure 5 crée un événement qui démarre exactement à la mesure 5
- [ ] Copier 8 mesures et coller : les événements sont décalés d'un nombre entier de mesures
- [ ] Annuler rétablit l'état précédent
- [ ] Fluide (≥ 30 im/s) avec 500 événements

## Tests
e2e : création d'un show, glisser, copier-coller, sauvegarde, rechargement.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
