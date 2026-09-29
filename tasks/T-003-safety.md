---
id: T-003
title: Zones de sécurité, horizon, calibration couleur
status: review
area: safety
priority: P0
depends_on: []
owner: "dev-agent (safety-zones)"
branch: feat/safety-zones
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
- [x] Aucun point allumé à l'intérieur d'une zone Blank, même sur un segment qui la traverse
- [x] Horizon : luminosité réduite sous y, avec rampe
- [x] Réglages persistants après redémarrage
- [x] 60 im/s tenus avec 2000 points

## Tests
Unitaires géométriques + e2e (zone ajoutée → aucun point allumé dedans).

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-29 — dev-agent (safety-zones), branche `feat/safety-zones` : nouveau `zones.rs` (zones Blank/Dim polygonales avec découpe des segments aux bords, horizon étendu aux lignes avec rampe, gain par couleur, niveau minimum des diodes), branché dans `safety::apply` après la calibration et avant le limiteur de strobe et la porte de sortie. L'horizon faisceaux de T-101 est conservé (`horizon.y`, compatibilité `beam_floor_y`). Assouplir un réglage exige une confirmation explicite (409 sinon). UI RÉGLAGES › Sécurité + surcouche rouge sur l'aperçu 2D (3D laissé à T-279). `cargo test` 680 + 4 OK, clippy OK, e2e 177/177 (rebasé sur develop 070613d). 2000 points × 8 zones : 0,10 ms/image (release). Choix à valider : `horizon.lines` désactivé par défaut, 8 zones au lieu de 5. Note : `docs/prs/safety-zones.md`.
