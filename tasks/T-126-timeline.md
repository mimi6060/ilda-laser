---
id: T-126
title: Timeline « Break trance → drop euphorique » (48 mesures)
status: todo
area: timeline
priority: P2
depends_on: [T-160, T-111, T-116, T-113, T-106, T-105]
owner: ""
branch: ""
source: docs/research/festival-looks.md#6-six-pre-made-full-timelines
---

## Contexte
Une des 6 timelines prêtes à l'emploi du rapport (section 6) : un enchaînement de looks sur une grille de mesures en 4/4, qui suit l'horloge de tempo. Les mesures sont numérotées à partir de 1 ; « mesure 17 » = changement instantané au temps 1 de la mesure 17.

## À faire
Écrire cette timeline avec le format de timeline de T-160, en utilisant les cues évolutifs et générateurs cités. Lancement quantifié à la mesure suivante. Spécification :

1. Mesures 1–16 : E5 (T-116) deux fois, cyan → bleu.
2. Mesures 17–24 : aurore (T-106) violet-bleu, très lente.
3. Mesures 25–32 : tunnel qui se ferme de 0.35 à 0.05, rotation de 1/32 à 1/4 tour/temps, vers le blanc ; strobe 1/2 temps mesure 31 ; noir sur le dernier temps de la mesure 32.
4. Mesure 33 : moitié drop de E2 (T-113), cyan + blanc.
5. Mesures 41–48 : tunnel de faisceaux arc-en-ciel à dérive lente (seul usage volontaire de l'arc-en-ciel), 1/4 tour/temps.

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
