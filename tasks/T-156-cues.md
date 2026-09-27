---
id: T-156
title: Quatre calques avec gradateur, muet/solo et budget de points
status: review
area: cues
priority: P1
depends_on: [T-155, T-140]
owner: "dev-agent (layers)"
branch: feat/layers
source: docs/research/pro-live-operation.md §2.5, §5
---

## Contexte
Superposer un faisceau sur un tunnel, ou un texte sur un abstrait, et doser chaque couche. Au laser, les calques s'additionnent (pas de transparence) et chaque couche coûte des points.

## À faire
- 4 calques ; chaque case a un calque (défaut 1) ; rendu calque 1 → 4, points concaténés, déplacement éteint entre calques.
- Par calque : *Gradateur* 0..1, *Muet*, *Solo*, modificateurs (T-144).
- **Budget de points** : si le total dépasse `point_budget` (défaut 750 = 30 kpps / 40 im/s), augmenter d'abord l'espacement des points de tous les calques (via `lit_step` de l'optimiseur T-002 si disponible, sinon décimation régulière), puis couper le calque de plus haut numéro ; avertissement dans l'interface.
- Contrôles : `layer.<n>.dimmer`, `layer.<n>.mute`, `layer.<n>.solo`, `layer.<n>.clear`.

## Modèle de données
```rust
#[serde(default)]
pub struct Layer { pub dimmer: f32 /*1.0*/, pub mute: bool, pub solo: bool, pub modifiers: LiveModifiers }
pub struct Mixer { pub layers: [Layer; 4], pub point_budget: usize /*750*/ }
```

## Interface
Quatre bandes *Calque 1–4* sous la grille : gradateur vertical, *Muet*, *Solo*, *Vider* ; compteur *Points : 620 / 750* qui devient orange au-delà du budget.

## Critères d'acceptation
- [x] Cue A calque 1 + cue B calque 2 : les deux dans `/api/frame`
- [x] Gradateur calque 2 à 0 : seuls les points du calque 1 sont allumés
- [x] *Solo* calque 2 : seul le calque 2 sort
- [x] Avec 4 calques lourds, le frame reste ≤ `point_budget` et l'avertissement s'affiche

## Tests
Unitaires : concaténation, solo/muet, budget. e2e : deux cues sur deux calques.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
- 2026-09-27 — dev-agent (layers), branche `feat/layers` : `layers.rs` (Mixer, dimmer/muet/solo, budget : points espacés jusqu'à 1 sur 2 puis calques du haut coupés, le calque le plus bas jamais coupé), `CueSlot.layer` + `ActiveCue.layer`, « Un cue » remplace seulement dans le même calque, contrôles `layer.<n>.dimmer/mute/solo/clear`, `GET/POST /api/layers`, bandes *Calque 1–4* + compteur *Points* + avertissement, *Calque* dans « Propriétés du cue ». `Layer.modifiers` laissé à T-144 ; `lit_step` (T-002) absent → décimation régulière. `cargo test` 174 ok (+12), clippy propre, e2e 55 ok / 2 ignorés (dont 4 nouveaux dans `layers.spec.ts`). Note : `docs/prs/layers.md`. Statut → review.
