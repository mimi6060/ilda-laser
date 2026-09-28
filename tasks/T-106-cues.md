---
id: T-106
title: Nappes : liquid sky, lame, rideaux, cascade, scanner, lamelles, aurore, grille
status: in-progress
area: cues
priority: P1
depends_on: [T-100]
owner: "dev-agent (sheet-gens)"
branch: feat/sheet-gens
source: docs/research/festival-looks.md#c-sheets-ceilings-and-walls
---

## Contexte
Les nappes (plafond de lumière au-dessus du public, rideaux, cascades) sont les looks calmes des breaks et les ouvertures de DJ. Le générateur `liquid_sky` actuel est trop limité.

## À faire
- `liquid_sky` v2 (look 19) : hauteur `y0 = y_h + b` (0.05–0.6), ondulation amp `a` (0.01–0.03) à 0.25 cycle/temps, respiration de luminosité optionnelle `0.7 + 0.3·sin(2π·beat_pos/8)`. Garder l'ancien comportement si `beat_sync` est faux.
- `blade` (look 20) : une ligne passant par le centre, inclinée d'un angle θ oscillant entre ±30° sur `period_beats` (défaut 16), limitée au demi-plan supérieur.
- `curtain` (look 21) : N ≤ 5 traits verticaux de `y_h` à `y_h+0.6`, espacement 0.35, centrés.
- `waterfall` (look 22) : 4–8 traits horizontaux courts qui descendent de y 0.9 à `y_h` en 2 temps, un nouveau tous les 1/2 temps, x décalés en quinconce.
- `scanner` (look 23) : une ligne horizontale qui monte/descend (au-dessus de l'horizon) ou verticale qui balaye x ; linéaire, aller-retour ou bouclé, 1 passage par mesure par défaut.
- `slats` (look 24) : une ligne coupée en k segments (6–12), 50 % allumé, motif décalé d'un segment par 1/2 temps.
- `aurora` (look 25) : ligne dont y = somme de 3 sinus lents (0.03, 0.05, 0.08 cycle/temps), amplitude 0.1–0.25, mode couleur Dégradé conseillé.
- `grid_scan` existant : option `lines_x`/`lines_y` (≤ 6 + 6) et défilement d'une ligne par temps.

## Modèle de données
Réutilise `count`, `a`, `b`, `period_beats`. Ajout `loop_mode: LoopMode { Wrap, PingPong }` (défaut PingPong).

## Interface
Libellés : « Plafond liquide », « Lame », « Rideaux », « Cascade », « Scanner », « Lamelles », « Aurore », « Grille ».

## Critères d'acceptation
- [ ] Tous les nouveaux générateurs restent au-dessus de l'horizon
- [ ] `waterfall` : un trait met exactement 2 temps pour descendre
- [ ] `slats` : motif décalé d'un segment tous les 1/2 temps
- [ ] `liquid_sky` sans `beat_sync` produit les mêmes points qu'avant
- [ ] Budget de points respecté

## Tests
Unitaires géométriques et de non-régression ; e2e : sélectionner « Plafond liquide », vérifier la sauvegarde des paramètres.

## Notes
Le plafond liquide est volontairement proche du public : il doit toujours respecter l'horizon de T-101/T-003.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
