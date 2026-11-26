---
id: T-110
title: Page de cues « Festival »
status: todo
area: cues
priority: P1
depends_on: [T-102, T-103, T-104, T-105, T-106, T-107, T-130]
owner: ""
branch: ""
source: docs/research/festival-looks.md#1-catalogue-of-festival-looks
---

## Contexte
L'utilisateur veut lancer des looks de festival en un clic. Une page de la grille de cues rassemble les meilleurs réglages des nouveaux générateurs.

## À faire
Ajouter dans `presets.rs` une page « Festival » (et si besoin « Festival 2 ») d'au moins 40 cues construits à partir de T-102 à T-107, chacun avec une palette de T-130. Répartition : 10 éventails/balayages, 6 chasers/stabs, 4 croisements/faisceau chaud, 8 tunnels/soleils, 8 nappes, 4 textures. Nommer les cues en français par leur usage : « Drop – éventail blanc », « Break – plafond bleu », « Montée – tunnel qui se ferme »… Chaque cue a `beat_sync` activé et des valeurs de départ tirées du rapport (section 4).

## Modèle de données
Même structure `Preset` que T-005. Ajout d'un champ optionnel `tag: Intro | Groove | Break | Build | Drop` (défaut Groove) pour colorer les cases.

## Interface
Nouvel onglet de page « Festival » dans la grille ; pastille de couleur par tag (Intro gris, Break bleu, Montée orange, Drop rouge). Raccourcis AZERTY comme les autres pages.

## Critères d'acceptation
- [ ] ≥ 40 cues sur la page Festival, tous chargent sans erreur
- [ ] Chaque cue produit des points au-dessus de l'horizon
- [ ] Les noms sont en français et uniques
- [ ] Les tags sont visibles dans la grille

## Tests
Unitaires : chaque preset de la page génère une image non vide et dans le budget de points. e2e : ouvrir la page Festival, cliquer 3 cues, vérifier `/api/state`.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
