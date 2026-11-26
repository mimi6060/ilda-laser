---
id: T-146
title: Grille FX : effets par-dessus les cues
status: todo
area: live
priority: P2
depends_on: [T-140, T-151]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §1.2–§1.3 (QuickFX, FX grid, Drop effects, Action)
---

## Contexte
Ajouter un effet (vague, zoom qui pulse, rotation qui balance, chenillard) à tout ce qui joue d'un seul clic, sans modifier les cues.

## À faire
- 4 lignes × 8 cases (APC40 : une rangée de boutons par ligne) ; une case = un ensemble de modulateurs (T-151) et/ou de modificateurs, avec une durée en temps.
- Modes : *Un par ligne* (cliquer une case remplace l'effet de la ligne) ; *Arrêter FX*.
- *Effets ponctuels* (drop) : jouent une seule fois sur leur durée puis disparaissent (enveloppe montée/descente).
- Curseur *Action* par ligne (0 = pas d'effet, 1 = plein effet) : mélange linéaire des valeurs modulées.
- Destination : *Maître* (défaut) ou *Cue sélectionné*.
- 16 effets de départ créés par nous (ex. *Respiration*, *Balancier*, *Pulsation*, *Vague X*, *Zoom au temps*, *Tourbillon*, *Chenillard rapide*, *Strobe 1/8*, *Tracé*…).

## Modèle de données
```rust
#[serde(default)]
pub struct FxCell { pub name: String, pub modulators: Vec<Modulator>, pub modifiers: Option<LiveModifiers>,
    pub length: Rate /*Beats(4.0)*/, pub one_shot: bool }
pub struct FxLine { pub cells: [Option<FxCell>; 8], pub active: Option<usize>, pub action: f32 /*1.0*/ }
pub struct FxGrid { pub lines: [FxLine; 4] }   // studio-data/fx.json
```
Contrôles : `fx.<ligne>.<case>` (bascule ou déclencheur si ponctuel), `fx.<ligne>.action`, `fx.stop`.

## Interface
Section « Effets » : grille 4×8, nom de chaque case, *Action* par ligne, *Arrêter FX*, *Maître / Cue*.

## Critères d'acceptation
- [ ] Activer une case puis une autre sur la même ligne : un seul effet actif
- [ ] *Action* 0 : frame identique à sans effet
- [ ] Un effet ponctuel de 2 temps s'arrête seul après 2 temps (±1 frame)
- [ ] Le limiteur de strobe de T-101 agit aussi sur les effets de la grille
- [ ] Deux lignes actives se cumulent

## Tests
Unitaires sur mélange Action, fin des ponctuels. e2e : clic case → frame change.

## Notes
Les noms et réglages des effets de départ sont les nôtres. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
