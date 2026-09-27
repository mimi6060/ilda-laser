---
id: T-123
title: Cue évolutif E12 « Grille qui se resserre » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-106, T-101]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

Grille 5 + 5 lignes dans la moitié haute.
- b 0–16 : révélation d'une ligne par temps, alternant horizontale et verticale ; couleur principale.
- b 16–28 : espacement `s = lerp(1, 0.4, (b−16)/12)`, défilement d'une ligne par temps.
- b 28–32 : strobe 1/4 temps (≤ 1 mesure, sous le limiteur), puis à b = 32 bascule sur une seule nappe horizontale. Boucle possible (option).

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] 1 ligne à b=0, 10 lignes à b=9
- [ ] Espacement 0.4 à b=28
- [ ] Strobe entre 28 et 32
- [ ] Une seule nappe à la fin
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
