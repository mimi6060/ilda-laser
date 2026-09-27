---
id: T-112
title: Cue évolutif E1 « Éventail qui monte » (16 temps)
status: todo
area: cues
priority: P2
depends_on: [T-111, T-102, T-103, T-130]
owner: ""
branch: ""
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un des 12 cues évolutifs prêts à l'emploi décrits dans le rapport (section 5). Notation : `b` = temps depuis le lancement (flottant), `ph = b mod 1`, `lerp`, `ease_in(x)=x²`, `smooth(x)=x²(3−2x)`, repère −1..1 avec +y vers le haut, `y_h` = horizon.

## À faire
Écrire ce cue comme un `Content::Evolving` (T-111) dans `presets.rs`, page « Festival évolutifs ». Spécification :

Fan N = 10, `fan` + chase remplissage (forme 7).
- Largeur `w = lerp(0.15, 0.7, smooth(b/16))`, hauteur `y0 = y_h + lerp(0.05, 0.6, ease_in(b/16))`.
- Pas du chase : 1 temps pour b < 8, 1/2 temps pour 8 ≤ b < 12, 1/4 temps pour b ≥ 12 ; quand tous les faisceaux sont allumés, le remplissage recommence.
- Couleur : principale, saturation `lerp(1, 0, smooth((b−8)/8))` (vers le blanc sur les 8 derniers temps).
- Luminosité 0 pour b ∈ [15.5, 16) (noir d'un demi-temps). Boucle : non (utilisé en montée).

Si une courbe ne peut pas s'exprimer par interpolation de clés, ajouter une clé par temps (générée par code), pas un nouveau cas spécial dans le moteur.

## Modèle de données
Aucune nouvelle structure : clés `EvolvingKey` générées par une fonction `fn evolving_eN() -> Preset` dans `presets.rs`.

## Interface
Case dans la page « Festival évolutifs » avec le nom du cue, sa longueur en temps et une pastille de tag (Montée / Drop / Break / Intro).

## Critères d'acceptation
- [ ] Largeur 0.15 à b=0 et 0.7 à b=16 (± 0.01)
- [ ] Pas du chase : 1, 1/2, 1/4 temps selon la zone
- [ ] Noir entre 15.5 et 16
- [ ] Blanc (saturation ≈ 0) à b=16
- [ ] Rendu identique à 120 et 150 BPM en fonction du temps musical (pas des secondes)
- [ ] Aucun point sous l'horizon

## Tests
Unitaire : échantillonner le cue aux temps clés (0, milieu, fin) et vérifier les valeurs ci-dessus. Test commun : le cue génère une image non vide dans le budget de points à chaque 1/4 de temps.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
