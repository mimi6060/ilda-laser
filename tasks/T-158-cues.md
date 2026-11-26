---
id: T-158
title: Transitions entre cues : coupe, fondu, fondu au noir, morph
status: todo
area: cues
priority: P2
depends_on: [T-155, T-150]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §2.4
---

## Contexte
Changer de cue sans à-coup visuel : fondu enchaîné ou morph de forme, calé en temps.

## À faire
- `Transition { kind, length }` : *Coupe*, *Fondu* (l'ancien baisse pendant que le nouveau monte ; les deux sont dessinés), *Fondu au noir* (moins de points : l'ancien descend, puis le nouveau monte), *Morph* (rééchantillonnage des deux frames au même nombre de points le long du chemin, puis interpolation positions + couleurs).
- Durée en secondes ou en temps (défaut 1 temps). Transition globale + surcharge par case.
- Cycle de vie du cue : *Démarrage* → *Lecture* → *Fin* (le cue qui s'arrête continue d'être dessiné pendant sa fin).
- En mode *Flash*, pas de transition par défaut (réaction immédiate).

## Modèle de données
```rust
pub enum TransitionKind { Cut, Fade, FadeThroughBlack, Morph }
#[serde(default)]
pub struct Transition { pub kind: TransitionKind /*Cut*/, pub length: Rate /*Beats(1.0)*/, pub in_flash: bool /*false*/ }
pub enum CuePhase { Starting(f32), Playing, Finishing(f32) }
```

## Interface
Barre de la grille : *Transition* (*Coupe, Fondu, Fondu au noir, Morph*) et *Durée* (*¼, ½, 1, 2, 4 temps* ou secondes). Propriétés de case : *Transition* (surcharge).

## Critères d'acceptation
- [ ] *Coupe* : aucun frame intermédiaire
- [ ] *Fondu* 1 s : à 0,5 s l'ancien et le nouveau sont à ~50 % de luminosité
- [ ] *Morph* entre deux formes à nombres de points différents : pas de panique, nombre de points de sortie stable
- [ ] Pas de point allumé hors sécurité pendant une transition

## Tests
Unitaires sur le rééchantillonnage, l'interpolation et le cycle de vie.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
