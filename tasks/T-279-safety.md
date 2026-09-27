---
id: T-279
title: Surcouches de sécurité dans le visualiseur (zone public, horizon)
status: todo
area: safety
priority: P1
depends_on: [T-277, T-003]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#12-how-to-build-it-in-the-browser
---

## Contexte
Le visualiseur est l'endroit idéal pour voir avant le show si un look envoie des faisceaux dans le public. Les zones et l'horizon de T-003 existent en 2D ; il faut les voir dans la salle.

## À faire
- Afficher la zone public comme un volume translucide (rectangle au sol extrudé jusqu'à `min_height_m`, défaut 3 m, réglable).
- Projeter les zones de sécurité et l'horizon de T-003 dans la salle depuis chaque projecteur (cônes/plans translucides).
- Tout segment de faisceau qui entre dans le volume public est dessiné **en rouge** ; compteur *Faisceaux dans le public : N* dans la vue et dans la barre d'état ; le compteur tient compte de l'image finale (après zones de sécurité).
- Bouton *Afficher les surcouches de sécurité* (activé par défaut).
- Le calcul est fait côté UI (visualisation) ; il ne modifie jamais la sortie.

## Modèle de données
Réutilise `Venue.audience` (T-277) et `SafetySettings` (T-003, lu via `/api/safety`).

## Interface
Libellés : *Surcouches de sécurité*, *Zone public*, *Hauteur minimale (m)*, *Faisceaux dans le public : N*, infobulle : *Aide à la conception, ne remplace pas une étude de sécurité ni la réglementation locale*.

## Critères d'acceptation
- [ ] Un faisceau dirigé vers le public (pitch négatif) est rouge et compté
- [ ] Avec une zone de blanking T-003 qui couvre le public, le compteur tombe à 0
- [ ] Désactiver les surcouches ne change ni `/api/frame` ni la sortie

## Tests
Unitaires JS : intersection segment/volume public. e2e : cue test dirigé vers le bas → compteur > 0 ; ajout d'une zone T-003 → 0.

## Notes
Ne jamais présenter ce compteur comme une conformité réglementaire. Aucune sortie laser ne dépend de ce calcul (la sécurité réelle reste dans `safety.rs`). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
