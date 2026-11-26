---
id: T-296
title: Espace CRÉATION : éditeur de figures laser (dessin point par point, animation)
status: done
area: cues
priority: P1
depends_on: [T-295]
owner: "dev agent (figure-editor)"
branch: feat/figure-editor
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
- [x] Dessiner une figure, l'enregistrer, la rouvrir à l'identique
- [x] Une figure enregistrée apparaît comme cue jouable (grille ou bibliothèque)
- [x] Une animation de plusieurs images joue en boucle calée sur le tempo
- [x] Le rendu passe par calques, direct, calibration, sécurité et verrou de sortie
- [x] Annuler/rétablir sur au moins 50 actions

## Tests
Unitaires sur le modèle et la conversion en points ; e2e : dessin simple
→ enregistrer → jouer → frame non vide.

## Notes
Export ILDA des figures possible plus tard via T-001/T-011. Aucun contenu
tiers intégré.

## Journal
- 2026-09-28 — architecte : créée à la demande de l'utilisateur.
- 2026-09-28 — dev (feat/figure-editor) : fait. `figures.rs` (modèle
  `Figure`/`FigureFrame`/`Stroke`, `rate` + `per` temps|seconde, boucle /
  aller-retour / une fois, bibliothèque `studio-data/figures/`, noms
  confinés comme les shows), `Content::Figure` rendu par `engine.rs`
  (tracés éteints explicites, trajet éteint entre tracés, images calées
  sur le temps depuis le 1 de la mesure), page de cues « Figures » (9e page,
  cellules MIDI `grid.9.*`), API `/api/figures*`, section `figures` du
  fichier projet (anciens projets : s'ouvrent sans figures), UI CRÉATION ›
  Figures (outils, couleur par tracé, ordre, symétries, images, pelure
  d'oignon, compteur/budget, annuler/rétablir 200 niveaux). Tests :
  `cargo test` 519 + 2 OK, clippy OK, e2e 141/141 (dont
  `figures.spec.ts`, 4 tests) après rebase sur develop 9a86362. Note PR :
  `docs/prs/figure-editor.md`. À trancher en review : ouvrir un projet
  remplace la bibliothèque de figures (modèle document de T-286).
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
