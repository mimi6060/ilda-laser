---
id: T-171
title: Budget de points, vitesse de balayage et minimum de points par sortie
status: todo
area: output
priority: P2
depends_on: [T-002]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §5
---

## Contexte
Garder une image stable quand on empile des calques et des effets : trop de points = scintillement, trop peu = surcharge et perte de luminosité.

## À faire
- Par sortie : vitesse de balayage (pps) par défaut 30 000 avec **bornes min/max** (protection des galvos), minimum de points par frame (défaut 200, compléter par des points **allumés** répétés sur le dernier point plutôt que des points éteints quand c'est sûr, sinon éteints), cadence minimale visée (40 im/s).
- Contrôle en direct *Vitesse de balayage* 50..150 % (`master.scan`) qui agit sur l'espacement des points (`lit_step`), borné par les limites de la sortie.
- Indicateur dans l'interface : points par frame, images/s estimées, alerte si < 35 im/s.

## Modèle de données
```rust
#[serde(default)]
pub struct ScanLimits { pub pps: u32 /*30000*/, pub pps_min: u32 /*10000*/, pub pps_max: u32 /*40000*/, pub min_points: usize /*200*/, pub min_fps: f32 /*40.0*/ }
```

## Interface
Indicateur *Points / Images/s* près de l'aperçu ; réglages *Vitesse de balayage*, *Min / Max*, *Points minimum* dans *Sorties*.

## Critères d'acceptation
- [ ] Un frame de 50 points est complété à 200
- [ ] `master.scan` à 150 % ne dépasse jamais `pps_max`
- [ ] L'indicateur passe en alerte sous 35 im/s estimées

## Tests
Unitaires sur le complément et le calcul d'images/s.

## Notes
Ne pas dupliquer ce que fait déjà `laser-dac` (blanking entre frames, délai couleur 150 µs). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
