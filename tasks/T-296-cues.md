---
id: T-296
title: Espace CRÉATION : éditeur de figures laser (dessin point par point, animation)
status: todo
area: cues
priority: P1
depends_on: [T-295]
owner: ""
branch: ""
source: demande utilisateur 2026-09-28 (« créer du contenu », idée PicEdit/Showeditor)
---

## Contexte
Pour créer son propre contenu (logos, formes, animations) sans dépendre
d'aucune bibliothèque tierce, il faut un éditeur de figures dans l'onglet
CRÉATION. Idée inspirée des éditeurs de figures des logiciels laser, notre
propre conception.

## À faire
- Canvas d'édition (repère −1..1) avec grille et magnétisme optionnel.
- Outils : point, polyligne, courbe (Bézier), rectangle, ellipse, polygone
  régulier, texte (police laser existante), gomme, sélection/déplacement,
  rotation/échelle d'une sélection, symétrie X/Y.
- Couleur par segment ; segments « éteints » (déplacement) explicites ;
  ordre de tracé visible (numéros/flèches) et modifiable.
- Animation image par image : liste d'images, dupliquer, pelure d'oignon,
  cadence (images par temps ou par seconde), boucle / aller-retour.
- Enregistrement dans une **bibliothèque de figures** (`studio-data/figures/`)
  et **jouable comme un cue** (nouveau type de contenu `Content::Figure`),
  avec tous les modificateurs live, calques, budget de points, sécurité.
- Compteur de points / images par seconde estimées, alerte au-delà du budget.
- Annuler / rétablir.

## Modèle de données
`Figure { name, frames: Vec<FigureFrame>, fps_or_beats, loop_mode }`,
`FigureFrame { strokes: Vec<Stroke { points: Vec<[f32;2]>, color: [u8;3], lit: bool }> }`.

## Interface
Onglet CRÉATION → « Figures » : barre d'outils à gauche, canvas au centre,
images en bas, propriétés à droite. Libellés en français.

## Critères d'acceptation
- [ ] Dessiner une figure, l'enregistrer, la rouvrir à l'identique
- [ ] Une figure enregistrée apparaît comme cue jouable (grille ou bibliothèque)
- [ ] Une animation de plusieurs images joue en boucle calée sur le tempo
- [ ] Le rendu passe par calques, direct, calibration, sécurité et verrou de sortie
- [ ] Annuler/rétablir sur au moins 50 actions

## Tests
Unitaires sur le modèle et la conversion en points ; e2e : dessin simple
→ enregistrer → jouer → frame non vide.

## Notes
Export ILDA des figures possible plus tard via T-001/T-011. Aucun contenu
tiers intégré.

## Journal
- 2026-09-28 — architecte : créée à la demande de l'utilisateur.
