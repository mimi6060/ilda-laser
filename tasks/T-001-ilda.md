---
id: T-001
title: Lecteur/écrivain ILDA maison
status: todo
area: ilda
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/ROADMAP.md#w1-ilda
---

## Contexte
Pouvoir charger les fichiers .ild que l'utilisateur possède, et exporter nos looks pour la carte SD du ShowNET. La crate `ilda` 0.2.0 a des bugs (couleurs format 4, palettes format 2, panics).

## À faire
Nouveau `studio/src/ilda.rs`, implémentation maison (pas de nouvelle dépendance). Lecture formats 0, 1, 2 (palette), 4, 5 ; big-endian ; bit 6 = blanking ; palette ILDA par défaut 64 couleurs ; fichiers tronqués → erreur, jamais de panic. Écriture format 5. Aide `shownet_sd_name(n)` : 001–229 valides, 000 et 230–255 réservés ; limite 8 Mo.

## Modèle de données
`IldaFrame { name, points: Vec<Point> }` ; `read(&[u8]) -> Result<Vec<IldaFrame>>` ; `write_format5(&[IldaFrame]) -> Vec<u8>` ; `SHOWNET_MAX_FILE_BYTES`.

## Interface
Aucune (branché à l'UI par T-011).

## Critères d'acceptation
- [ ] Round-trip écriture → lecture fidèle à la quantification près
- [ ] Couleurs par point en format 4/5 (régression du bug de la crate)
- [ ] Palette format 2 appliquée aux frames indexées suivantes
- [ ] Fichier tronqué → Err sans panic
- [ ] Noms SD : 0 et 230–255 refusés

## Tests
Tests unitaires qui génèrent leurs propres octets ILDA (aucun fichier tiers).

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
