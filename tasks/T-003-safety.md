---
id: T-003
title: Zones de sécurité, horizon, calibration couleur
status: todo
area: safety
priority: P0
depends_on: []
owner: ""
branch: ""
source: docs/research/pangolin.md
---

## Contexte
Sécurité public : zones où le laser ne doit jamais s'allumer, et atténuation sous une hauteur donnée. Indispensable avant tout usage devant du public.

## À faire
Nouveau `studio/src/safety.rs` appliqué après la calibration, avant l'envoi (aperçu et laser identiques). Jusqu'à 5 zones polygonales (Blank ou Dim) avec découpe des segments au bord ; horizon avec rampe ; gain par couleur ; niveau minimum des diodes. Sauvegarde `safety.json`, `GET/POST /api/safety`.

## Modèle de données
`SafetySettings { zones: Vec<Zone>, horizon: Option<Horizon>, color_gain: [f32;3], min_diode_level: f32 }`.

## Interface
Section « Sécurité » ; zones affichées en rouge translucide dans l'aperçu.

## Critères d'acceptation
- [ ] Aucun point allumé à l'intérieur d'une zone Blank, même sur un segment qui la traverse
- [ ] Horizon : luminosité réduite sous y, avec rampe
- [ ] Réglages persistants après redémarrage
- [ ] 60 im/s tenus avec 2000 points

## Tests
Unitaires géométriques + e2e (zone ajoutée → aucun point allumé dedans).

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
