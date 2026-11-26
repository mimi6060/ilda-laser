---
id: T-144
title: Modificateurs au niveau cue et calque, sauvegarde dans le cue, lissage
status: todo
area: live
priority: P2
depends_on: [T-140, T-156]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §1.3 (niveaux Cue/Maître/Zone, Physics, Append Cue FX)
---

## Contexte
Pouvoir régler un seul cue (ou un calque) sans toucher au reste, puis garder ce réglage dans le cue. Et des curseurs qui glissent au lieu de sauter.

## À faire
- Chaîne : cue → calque → maître (→ zone plus tard, T-012). Chaque niveau a son `LiveModifiers`.
- Onglets *Cue* et *Calque* du panneau « Direct » actifs : ils ciblent le cue sélectionné ou le calque sélectionné.
- *Enregistrer dans le cue* : copie les modificateurs courants du cue (et, en option, ceux du maître) dans la case de la grille (`CueSlot.modifiers`), comme « Append Cue FX to Cue Effect » de BEYOND.
- Lissage « physique » optionnel par contrôle continu : filtre masse-ressort (masse, rappel, frottement), désactivé par défaut.
- Composition : tailles multipliées, positions additionnées, angles additionnés, vitesses additionnées, luminosités multipliées, couleur : le niveau le plus bas non-*Normal* gagne, puis le maître peut forcer.

## Modèle de données
```rust
#[serde(default)]
pub struct Smoothing { pub enabled: bool, pub mass: f32 /*1.0*/, pub spring: f32 /*40.0*/, pub friction: f32 /*12.0*/ }
// CueSlot.modifiers: LiveModifiers (T-155) ; Layer.modifiers: LiveModifiers (T-156)
```
Contrôles : `cue.<id>.<param>` et `layer.<n>.<param>`.

## Interface
Onglets *Cue* / *Calque* ; bouton *Enregistrer dans le cue* ; case *Lissage* avec *Inertie*.

## Critères d'acceptation
- [ ] Taille cue 0,5 × taille maître 2,0 = taille effective 1,0
- [ ] *Enregistrer dans le cue* puis relance du cue : mêmes modificateurs
- [ ] Lissage activé : un saut de 0 à 1 atteint 0,9 en ~0,3 s sans dépasser 1,05
- [ ] Les cases existantes sans modificateurs chargent avec l'identité

## Tests
Unitaires de composition et du filtre. e2e : réglage cue + sauvegarde + relance.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
