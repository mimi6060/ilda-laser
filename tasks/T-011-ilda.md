---
id: T-011
title: Médiathèque ILDA : import et export depuis l'interface
status: todo
area: ilda
priority: P2
depends_on: [T-001]
owner: ""
branch: ""
source: docs/ROADMAP.md#wave-2
---

## Contexte
Jouer ses propres fichiers .ild comme des cues, et exporter nos looks vers la carte SD du ShowNET.

## À faire
Import de fichiers .ild (glisser-déposer) dans `studio-data/media/`, joués comme contenu (sans repasser par l'optimiseur) ; export d'un look en format 5 avec nom SD valide.

## Modèle de données
`Content::Ilda { file, fps }`.

## Interface
Section « Médiathèque ».

## Critères d'acceptation
- [ ] Import → le fichier apparaît et se joue
- [ ] Export → fichier valide relu par T-001
- [ ] Rappel à l'écran : n'importer que des fichiers dont on a les droits

## Tests
e2e avec un fichier généré.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
