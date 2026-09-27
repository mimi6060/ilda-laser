---
id: T-121
title: Cue évolutif E10 « Couloir de rideaux » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-106]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`curtain` 4 rideaux à x = −0.6, −0.2, 0.2, 0.6, de `y_h` à `y_h+0.6`.
- b < 16 : apparition de l'extérieur vers l'intérieur, un rideau tous les 4 temps (ordre −0.6, 0.6, −0.2, 0.2), fixes.
- 16 ≤ b < 32 : `x_i ← x_i·(1 − 0.5·smooth((b−16)/16))`, gate sur chaque temps (allumé 1/2 temps).
- b = 31 : fusion en un seul trait central, puis fin. Pas de boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] 1 rideau à b=0, 4 rideaux à b=12
- [ ] Les rideaux se rapprochent entre 16 et 32
- [ ] Un seul trait à b=31
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
