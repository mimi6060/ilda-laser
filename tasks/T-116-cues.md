---
id: T-116
title: Cue évolutif E5 « Plafond qui descend » (32 temps)
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

`liquid_sky` v2. `y0 = y_h + lerp(0.6, 0.08, smooth(b/32))`, ondulation `0.01 + 0.02·smooth(b/32)` à 0.25 cycle/temps, mode Dégradé principale → `color2`, dérive de teinte ±15° sur le cue, luminosité `0.7 + 0.3·sin(2π·b/8)`. Sur [31.5, 32) la nappe remonte à `y_h + 0.6`. Boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Hauteur y_h+0.6 à b=0 et ≈ y_h+0.08 à b=31
- [ ] Jamais sous l'horizon
- [ ] Remontée sur le dernier demi-temps
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
