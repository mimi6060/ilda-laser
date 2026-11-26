---
id: T-124
title: Timeline « Montée 16 mesures → drop » (32 mesures)
status: todo
area: timeline
priority: P2
depends_on: [T-160, T-111, T-112, T-113, T-114, T-118, T-119]
owner: ""
branch: ""
source: docs/research/festival-looks.md#6-six-pre-made-full-timelines
---

## Contexte
Une des 6 timelines prêtes à l'emploi du rapport (section 6) : un enchaînement de looks sur une grille de mesures en 4/4, qui suit l'horloge de tempo. Les mesures sont numérotées à partir de 1 ; « mesure 17 » = changement instantané au temps 1 de la mesure 17.

## À faire
Écrire cette timeline avec le format de timeline de T-160, en utilisant les cues évolutifs et générateurs cités. Lancement quantifié à la mesure suivante. Spécification :

1. Mesures 1–4 : E7 (T-118) pas 1 temps, fan bas.
2. Mesures 5–8 : même fan, pas 1/2 temps, fan qui se lève sur 4 mesures.
3. Mesures 9–12 : deuxième moitié de E1 (T-112) (élargissement, vers le blanc), pas 1/4.
4. Mesures 13–15 : montée de E2 (T-113) (tunnel qui se ferme), strobe 1/4 à partir de la mesure 14.
5. Mesure 16 : temps 1–3 faisceau chaud blanc, temps 4 noir.
6. Mesure 17 (drop) : fan large N=16 blanc, luminosité max, strobe 1/2 temps pendant 1 mesure.
7. Mesures 18–24 : E3 (T-114) couleur héros + blanc.
8. Mesures 25–32 : E8 (T-119), changement de couleur à la mesure 25.

## Modèle de données
Format de timeline défini par T-160 (pistes / clips positionnés en mesures). Pas de nouveau format ici ; si T-160 ne permet pas un point (par exemple « deuxième moitié d'un cue évolutif »), ajouter un décalage de départ `start_offset_beats` au clip, en coordination avec T-160.

## Interface
Apparaît dans la liste des timelines prêtes à l'emploi (« Modèles ») de l'interface de T-160, avec sa durée en mesures et son tempo conseillé.

## Critères d'acceptation
- [ ] La timeline se charge et se lit en aperçu du début à la fin
- [ ] Chaque changement tombe sur le temps 1 de la mesure indiquée (± 1 image)
- [ ] Les noirs avant les drops sont bien présents
- [ ] Suivi du tempo : à 140 BPM au lieu de 128, la durée totale change en proportion
- [ ] Aucun point sous l'horizon, limiteur de strobe jamais contourné

## Tests
Unitaire : parcourir la timeline à 128 BPM, échantillonner l'état au début de chaque section et vérifier le look actif. e2e : charger le modèle, lancer la lecture, vérifier `/api/state` sur 2 sections.

## Notes
Dépend du format de timeline de T-160 (écrit par un autre agent).

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
