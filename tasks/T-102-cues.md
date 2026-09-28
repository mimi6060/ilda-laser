---
id: T-102
title: Générateurs éventails : fan, balayage, levée, ouverture, vague, positions
status: done
area: cues
priority: P1
depends_on: [T-100]
owner: "dev-agent (fan-gens)"
branch: feat/fan-gens
source: docs/research/festival-looks.md#a-beam-fans-the-backbone
---

## Contexte
L'éventail de faisceaux est la base de tous les shows festival (Tomorrowland, EDC, Defqon.1). Il faut des éventails cadencés au tempo, avec les mouvements classiques des LJ.

## À faire
Nouveaux générateurs (tous `dots: true`, `beat_sync`), au-dessus de l'horizon :
- `fan` (looks 1 et 4) : N faisceaux sur une ligne `y = b`, demi-largeur `w`. `a` = ouverture animée : 0 = fixe ; sinon la largeur pompe entre `w_min=0.02` et `w` avec `env_stab` sur chaque temps (attaque ≤ 1/16 temps, décroissance 1/2 temps).
- `fan_sweep` (look 2) : fan dont le centre x = `A·sin(2π·beat_pos/period_beats)`, A = `a` (défaut 0.35). Option d'easing : sinus (défaut), triangle, trapèze (tenue aux extrémités 25 %). Groupes : miroir ou décalé (T-100).
- `fan_tilt` (look 3) : le fan monte de `y_h+0.05` à `y_h+0.7` sur `period_beats` (easing ease-in), puis revient instantanément (ou ping-pong si `direction` = 0).
- `fan_wave` (look 6) : hauteur de chaque faisceau `y_i = b + amp·sin(2π(x_i/λ − beat_pos·0.5))`, amp = `a`, λ = largeur du fan. Remplace à terme `beam_wave` (le garder pour compatibilité).
- `positions` (look 9) : 4 positions fixes enchaînées, une par pas (`steps_per_beat`) : P0 fan large haut, P1 fan étroit incliné gauche (+15°), P2 fan étroit incliné droite, P3 « V » (2 jambes de N/2 faisceaux à ±30° de la verticale). Saut éteint entre positions.
Paramètres de départ (rapport section 4) : N 6–16, w 0.5–0.8, période 2/4/8 temps.

## Modèle de données
`GenParams` : `count` = N, `a`/`b` comme décrit, `period_beats`, `steps_per_beat`, `groups`, `group_mode`. Ajouter `easing: Easing { Sine, Triangle, Trapezoid }` (défaut Sine).

## Interface
Les 5 générateurs apparaissent dans la liste des générateurs de l'onglet Effet, avec des libellés français : « Éventail », « Éventail balayé », « Éventail qui se lève », « Éventail vague », « Positions au temps ». Aide courte sous chaque paramètre.

## Critères d'acceptation
- [x] Les 5 générateurs produisent des points dans -1..1, tous au-dessus de l'horizon
- [x] `fan_sweep` à période 4 : même position aux temps 0, 4, 8
- [x] `positions` : la position change exactement au passage du temps entier
- [x] Groupes miroir : balayage gauche/droite en opposition
- [x] Budget de points respecté (test existant `every_generator_stays_within_a_sane_point_budget`)

## Tests
Unitaires par générateur (positions aux temps clés, bornes, nombre de faisceaux). e2e : sélectionner « Éventail balayé », vérifier `/api/frame` non vide et que des points bougent entre deux lectures.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
- 2026-09-28 — dev-agent (fan-gens), branche `feat/fan-gens` (rebasée sur develop 6e83091) : 5 générateurs dans `studio/src/fans.rs` (`fan`, `fan_sweep`, `fan_tilt`, `fan_wave`, `positions`), `Easing` dans `beat.rs`, `GenParams.easing`, libellés/aides/courbe dans l'onglet Effet. Ajout en fin de liste : les 20 générateurs et les 202 cues existants inchangés (empreintes identiques). `cargo test` 278 OK, clippy propre, e2e 63 OK (nouveau test « Éventail balayé »). Pire cas 533 points/trame (32 faisceaux) < budget 750. Note PR : docs/prs/fan-gens.md. Statut : review.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
