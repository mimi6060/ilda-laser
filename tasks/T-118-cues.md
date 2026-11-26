---
id: T-118
title: Cue évolutif E7 « Chaser qui accélère » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-103]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`chase_fan` N = 8, forme aller-retour, traîne 2 (40 %, 15 %, traîne en `color2`). Pas : 1 temps (b 0–8), 1/2 (8–16), 1/4 (16–24), 1/8 (24–28) ; b 28–31.5 fan complet en strobe 1/4 temps ; noir 31.5–32. Pas de boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Pas du chase conforme aux 4 zones
- [ ] Strobe fan complet entre 28 et 31.5
- [ ] Noir à la fin
- [ ] Le strobe 1/4 reste sous la limite de rafale (≤ 5 s à 128 BPM)
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
