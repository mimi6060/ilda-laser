---
id: T-125
title: Timeline « Bloc techno » style Awakenings (64 mesures)
status: todo
area: timeline
priority: P2
depends_on: [T-160, T-111, T-123, T-115, T-104, T-106, T-105]
owner: ""
branch: ""
source: docs/research/festival-looks.md#6-six-pre-made-full-timelines
---

## Contexte
Une des 6 timelines prêtes à l'emploi du rapport (section 6) : un enchaînement de looks sur une grille de mesures en 4/4, qui suit l'horloge de tempo. Les mesures sont numérotées à partir de 1 ; « mesure 17 » = changement instantané au temps 1 de la mesure 17.

## À faire
Écrire cette timeline avec le format de timeline de T-160, en utilisant les cues évolutifs et générateurs cités. Lancement quantifié à la mesure suivante. Spécification :

1. Mesures 1–16 : E12 (T-123) vert, sans strobe.
2. Mesures 17–32 : stabs sur le kick, fan étroit N=6, positions P1/P2 en miroir à chaque temps (variante de E4, T-115), vert.
3. Mesures 33–40 : break, faisceau chaud (T-104) panoramique 16 temps, rouge.
4. Mesures 41–48 : lamelles (T-106) décalage 1/2 temps, rouge + blanc.
5. Mesures 49–64 : tunnel de 12 faisceaux (T-105) 1/8 tour/temps, sens inversé toutes les 8 mesures, stab sur chaque temps, retour au vert à la mesure 49.
Boucle.

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
