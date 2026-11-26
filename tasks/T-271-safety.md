---
id: T-271
title: Barre du haut fixe : noir, armement, maître, tempo, état de sortie
status: todo
area: safety
priority: P0
depends_on: [T-270]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#2-operator-ux-for-live-busking
---

## Contexte
L'opérateur doit voir en permanence, quel que soit l'onglet ou la fenêtre, si le laser est armé, et pouvoir faire le noir d'un geste. Les pros ont aussi le BPM, le battement et le maître luminosité toujours sous les yeux.

## À faire
- Barre du haut fixe (56 px) présente dans toutes les vues, de gauche à droite : nom du projet (+ point si non enregistré, rempli par T-286), zone *Tempo* (BPM en grand, 4 voyants de temps, *Tap*, *Resync* : branchés sur T-150 s'il est fait, sinon zone masquée), *Maître* (curseur luminosité maître : `master.brightness` de T-140 s'il existe, sinon le réglage `brightness` actuel), indicateur de sortie (*Aperçu seulement*, nom du DAC), bouton **Noir** et bouton **LASER OFF/ON** à l'extrême droite.
- **Noir** = même action qu'Échap (`/api/arm` à `false`), jamais d'autre effet ; toujours actif, même en *Mode spectacle* (T-283) et quand un champ a le focus.
- Le bouton d'armement reste séparé des autres commandes par au moins 24 px et n'est jamais sous la grille.
- Quand le laser est armé : bordure rouge de 3 px autour de toute la fenêtre et libellé *LASER ON* en blanc sur rouge.
- Si `/api/frame` ne répond plus pendant > 1 s : bandeau *Connexion au moteur perdue* (orange) dans la barre.

## Modèle de données
Pas de nouveau modèle Rust. Lit `armed`, `pps`, `output` depuis `/api/frame` et `/api/state` (et `tempo` quand T-150 existe).

## Interface
Libellés : *Noir (Échap)*, *LASER OFF*, *LASER ON*, *Maître*, *Aperçu seulement*, *Connexion au moteur perdue*. Raccourcis inchangés : Échap = noir, Espace = armer/désarmer (hors champ texte).

## Critères d'acceptation
- [ ] La barre reste visible en faisant défiler la page et dans chaque onglet
- [ ] Clic sur *Noir* : `/api/state.armed == false` et `/api/frame` sans point allumé en < 100 ms
- [ ] Échap fait le noir même quand le focus est dans un champ texte
- [ ] Laser armé : bordure rouge visible ; désarmé : pas de bordure
- [ ] Moteur arrêté : le bandeau d'erreur apparaît en < 2 s

## Tests
e2e : armer (aperçu, sans `--device`), cliquer *Noir*, vérifier `/api/state` ; Échap depuis un champ texte ; arrêt du serveur de test → bandeau.

## Notes
Ne jamais ajouter d'armement automatique (au chargement d'un projet, d'une page, d'une fenêtre). Le curseur *Maître* ne peut pas dépasser 100 % ni contourner la mise à l'échelle de sécurité. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
