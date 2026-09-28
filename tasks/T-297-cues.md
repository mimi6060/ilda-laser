---
id: T-297
title: Espace CRÉATION : import SVG et vectorisation d'image vers figure laser
status: todo
area: cues
priority: P2
depends_on: [T-296]
owner: ""
branch: ""
source: demande utilisateur 2026-09-28 (idée « Tracer / SVG Tool »)
---

## Contexte
Créer du contenu vite à partir d'un logo ou d'un dessin existant de
l'utilisateur : importer un SVG, ou vectoriser une image (PNG/JPG), et
obtenir une figure laser éditable.

## À faire
- Import SVG (chemins, lignes, polygones, cercles ; transformations ;
  couleurs de trait) → figure (T-296), avec simplification des courbes.
- Vectorisation d'image : seuil / contours, lissage, réduction de points,
  choix du nombre de couleurs ; aperçu avant validation.
- Optimisation de l'ordre de tracé (plus proche voisin) et respect du budget
  de points ; avertissement si la figure est trop lourde.
- Le fichier importé reste chez l'utilisateur (studio-data), jamais dans le dépôt.

## Modèle de données
Produit une `Figure` de T-296.

## Interface
Onglet CRÉATION → « Importer » : glisser-déposer un fichier, réglages,
aperçu, « Créer la figure ».

## Critères d'acceptation
- [ ] Un SVG simple (logo) devient une figure fidèle et éditable
- [ ] Une image contrastée devient une figure sous le budget de points
- [ ] Fichiers invalides → message clair, aucun plantage

## Tests
Unitaires avec SVG/images générés dans les tests (aucun fichier tiers).

## Notes
Bibliothèques éventuelles : licence MIT/Apache uniquement, listées dans
`docs/CONTENT_SOURCES.md` si des fichiers sont intégrés. L'utilisateur doit
avoir les droits sur ce qu'il importe (rappel dans l'interface).

## Journal
- 2026-09-28 — architecte : créée à la demande de l'utilisateur.
