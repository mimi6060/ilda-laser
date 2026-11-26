---
id: T-104
title: Croisements, faisceau chaud et convergences
status: todo
area: cues
priority: P2
depends_on: [T-102]
owner: ""
branch: ""
source: docs/research/festival-looks.md#a-beam-fans-the-backbone
---

## Contexte
Les X de faisceaux qui se croisent (« crossfire »), le faisceau unique très intense et les faisceaux qui convergent sont les moments forts des mainstages. Avec un seul projecteur on les approxime par groupes virtuels.

## À faire
- `scissor` (look 5) : 2 groupes de N/2 faisceaux. Centre G = `−c + A·sin φ`, centre D = `+c − A·sin φ`, φ = 2π·beat_pos/period_beats (défaut 2). Chaque groupe est incliné de ±15° (y proportionnel à x dans le groupe) pour lire un X. Couleur G = couleur principale, D = `color2`. Option : flash blanc 1/8 temps quand les groupes se croisent.
- `hot_beam` (look 11) : 1 faisceau, dwell long, panoramique lent A = 0.3 sur 16 temps (jamais immobile : vitesse minimale imposée, voir Notes).
- `bundle_burst` (look 12, approximation 1 projecteur) : faisceaux regroupés en un point (largeur 0.02), puis explosion vers un fan large en 1/4 temps au temps 0 de chaque période ; `a` = durée d'ouverture en temps.
- `beam_v` (look 29) : « V » ou « Λ » de 2×5 faisceaux, jambes à ±30°, rotation par pas optionnelle.

## Modèle de données
Réutilise `count`, `a`, `b`, `period_beats`, `color2`, `groups=2`. Ajout `cross_flash: bool` (défaut false).

## Interface
Libellés : « Ciseaux (X) », « Faisceau chaud », « Faisceaux qui s'ouvrent », « V de faisceaux ».

## Critères d'acceptation
- [ ] `scissor` : les deux groupes se croisent au centre deux fois par période
- [ ] `hot_beam` ne reste jamais immobile plus de 100 ms
- [ ] `bundle_burst` : largeur ≤ 0.02 juste avant le temps, largeur max 1/4 temps après
- [ ] Tous les points au-dessus de l'horizon

## Tests
Unitaires géométriques aux phases clés ; test de mouvement minimal de `hot_beam`. e2e léger : chaque générateur sélectionnable et non vide.

## Notes
Le faisceau chaud concentre toute la puissance en un point : Pangolin recommande qu'il bouge en permanence. Imposer une vitesse minimale et une luminosité max réglable (défaut 0.6).

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
