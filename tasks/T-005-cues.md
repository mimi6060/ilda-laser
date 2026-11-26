---
id: T-005
title: Bibliothèque de 202 cues procéduraux
status: done
area: cues
priority: P1
depends_on: []
owner: ""
branch: "feat/presets"
source: docs/prs/presets.md
---

## Contexte
Des cues prêts à l'emploi, créés par nous (pas de contenu Pangolin/Laserworld).

## À faire
20 générateurs, 4 modes couleur, 8 pages, grille avec raccourcis AZERTY.

## Modèle de données
`Content::Generator { generator, params: GenParams }`.

## Interface
Grille de cues + onglet Effet.

## Critères d'acceptation
- [x] Fait — voir docs/prs/presets.md

## Tests
43 tests.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — réalisé et fusionné dans develop (architecte).
