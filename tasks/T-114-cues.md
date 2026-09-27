---
id: T-114
title: Cue évolutif E3 « Ciseaux croisés » (32 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-104]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`scissor`, 2 groupes de 5 faisceaux inclinés ±15°. Centre G = `−0.3 + 0.45·sin(2π·b/P)`, centre D opposé. P = 2 temps, sauf pour b mod 8 ∈ [4, 8) où P = 1 (vitesse doublée 4 temps sur 8). G couleur principale, D `color2` ; flash blanc 1/8 temps à chaque temps entier (croisement). Boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Période 2 temps sur les temps 0–4, 1 temps sur 4–8 de chaque bloc de 8
- [ ] Flash blanc au début de chaque temps pendant 1/8 temps
- [ ] Les groupes ont des couleurs différentes
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
