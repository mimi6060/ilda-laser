---
id: T-120
title: Cue évolutif E9 « Vague » (16 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-102, T-130]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

`fan_wave` N = 14. Amplitude `lerp(0.05, 0.35, b/8)` pour b < 8, tenue jusqu'à 12, redescend à 0.05 entre 12 et 16. Vitesse 0.5 cycle/temps ; sens de la vague inversé à b = 8, et mode Chase couleur (front d'un faisceau par 1/4 temps) à partir de b = 8. Boucle.

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Amplitude 0.35 à b=8
- [ ] Sens inversé à b=8
- [ ] Chase couleur actif après b=8
- [ ] Amplitude 0.05 à b=16 (bouclage sans saut)
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
