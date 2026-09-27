---
id: T-117
title: Cue évolutif E6 « Soleil levant » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-105, T-130]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`sunburst` N = 16, r = 0.85, demi-plan supérieur.
- b < 16 : les rayons apparaissent par paires symétriques depuis l'horizon vers le haut, une paire par temps ; pas de rotation.
- b ≥ 16 : rotation 1/64 tour/temps ; alternance pair/impair 100 %/40 % à chaque temps.
- Couleur : ambre (255,140,0) → blanc selon l'index du rayon (dégradé), tout blanc à b = 31. Pas de boucle (reste sur la fin).

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Nombre de rayons allumés = 2·(⌊b⌋+1) pour b < 8 (16 max)
- [ ] Rotation seulement après b=16
- [ ] Blanc à b=31
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
