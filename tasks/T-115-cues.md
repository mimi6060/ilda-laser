---
id: T-115
title: Cue évolutif E4 « Stabs sur 4 positions » (16 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-102, T-103]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`positions` (P0 fan large haut, P1 fan étroit incliné gauche, P2 fan étroit incliné droite, P3 V de 2×4). Temps k → `P[k mod 4]`, gate 0.2 temps. À partir de b = 8 : changement aussi sur le contretemps (pas 1/2 temps, gate 0.15). Couleur alternée principale/`color2` à chaque mesure. Boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Position P[k mod 4] au temps k
- [ ] Éteint après 0.2 temps (b < 8)
- [ ] Pas de 1/2 temps à partir de b=8
- [ ] Couleur change à chaque mesure
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
