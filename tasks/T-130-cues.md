---
id: T-130
title: Palettes festival et équilibre perçu des couleurs
status: todo
area: cues
priority: P2
depends_on: [T-100]
owner: ""
branch: ""
source: docs/research/festival-looks.md#3-colour-practice
---

## Contexte
Les shows pro utilisent peu de couleurs, bien choisies, et changent de couleur sur les phrases musicales. Le rouge et le bleu paraissent beaucoup plus sombres que le vert dans la fumée.

## À faire
1. Palettes nommées (couleur 1, couleur 2) : Vert pur, Blanc, Cyan glacier (cyan + blanc), Rouge hardstyle (rouge + blanc), Afterlife (bleu + magenta), Vert/bleu, Coucher de soleil (ambre + blanc), Pop (magenta + cyan).
2. Mode couleur « Vers le blanc » : désaturation progressive sur `period_beats` (pour les montées).
3. Mode couleur « Chase couleur » : un front de `color2` traverse les faisceaux (faisceau i change au pas i).
4. Changement de palette quantifié : appliqué au prochain début de mesure (ou de phrase de 8 mesures, au choix).
5. Préréglage « Équilibre perçu » : gain vert 0.6, rouge 1.0, bleu 1.0 (réglable), appliqué dans la calibration couleur de T-003 si elle existe.

## Modèle de données
`ColorMode` : ajouter `ToWhite`, `ColorChase`. `Palette { name, c1, c2 }` (liste statique). `palette_quantize: Quantize { Now, Bar, Phrase }`.

## Interface
Rangée de pastilles de palettes au-dessus de la grille de cues ; clic = appliquer (quantifié). Libellés en français.

## Critères d'acceptation
- [ ] Les 8 palettes s'appliquent au look courant
- [ ] `ToWhite` : saturation 0 à la fin de la période
- [ ] Changement de palette pendant une mesure appliqué au temps 1 suivant
- [ ] Anciens modes couleur inchangés

## Tests
Unitaires : désaturation, chase couleur, quantification. e2e : clic sur une palette, `/api/state` mis à jour au bon moment (tolérance d'une image).

## Notes
Arc-en-ciel : à garder comme moment voulu, pas comme mode par défaut (rapport section 3.3).

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
