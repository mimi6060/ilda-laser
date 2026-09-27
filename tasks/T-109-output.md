---
id: T-109
title: Cibles miroir : faisceaux dirigés vers des points calibrés
status: todo
area: output
priority: P3
depends_on: [T-003, T-100]
owner: ""
branch: ""
source: docs/research/festival-looks.md#d-mirrors-geometry-and-stage-bound-looks
---

## Contexte
Les effets de rebond sur miroirs (toiles, cages de lumière) demandent de viser des points précis de la salle. Utile en club et en salle techno, après les zones de sécurité.

## À faire
Liste de cibles nommées (jusqu'à 16), chacune un point (x, y) calibré par l'utilisateur en visant le miroir en aperçu puis au laser. Générateur `mirror_targets` : allume les cibles de l'ensemble choisi (masque de bits), avec chase possible (1 cible par temps). Les cibles sous l'horizon doivent être dans une zone « scène » autorisée de T-003, sinon refusées.

## Modèle de données
`Targets { items: Vec<Target { name: String, x: f32, y: f32 }> }` sauvegardé dans `targets.json`. `GenParams` : `a` = masque, `steps_per_beat` pour le chase.

## Interface
Section « Cibles miroir » : liste, bouton « Viser » (déplacer un point à la souris dans l'aperçu), nom. Générateur « Cibles miroir ».

## Critères d'acceptation
- [ ] Cibles persistantes après redémarrage
- [ ] Une cible sous l'horizon hors zone scène est refusée avec un message
- [ ] Chase : une cible par temps

## Tests
Unitaires : validation des cibles, masque. e2e : ajouter une cible, la retrouver après rechargement.

## Notes
Réglage des cibles uniquement en aperçu ou laser désarmé par défaut ; jamais de test automatique vers un vrai laser.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
