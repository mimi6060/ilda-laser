---
id: T-113
title: Cue évolutif E2 « Tunnel qui zoome sur le drop » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-105, T-103]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

Montée (b < 16) puis drop (b ≥ 16).
- b < 16 : `tunnel_pump` en mode rampe, rayon `lerp(0.35, 0.04, smooth(b/16))` ; rotation de 1/32 à 1/4 tour/temps (interpolée) ; strobe 1/2 temps de b=12, 1/4 temps de b=14 ; noir sur [15.5, 16).
- b ≥ 16 : rayon 0.45 au temps 16, puis pompe sur chaque temps `r = 0.35 + 0.1·exp(−ph/0.15)` ; rotation 1/4 tour/temps, sens inversé tous les 4 temps ; couleur = `color2` à partir de b = 16.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Rayon ≤ 0.05 à b=15
- [ ] Rayon 0.45 à b=16
- [ ] Sens de rotation inversé à b=20, 24, 28
- [ ] Couleur 2 dès b=16
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
