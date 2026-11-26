---
id: T-159
title: Lancement quantifié et mode beat (changement automatique au temps)
status: todo
area: cues
priority: P2
depends_on: [T-150, T-155]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §2.6
---

## Contexte
Pour que les cues partent pile sur le temps même si on appuie un peu en avance ou en retard, et pour faire défiler des frames au rythme.

## À faire
- `Quantize` : *Off*, *Temps*, *Mesure* ; un cue déclenché attend la prochaine frontière (tolérance : si on est à moins de 1/8 de temps après la frontière, départ immédiat « en rattrapage »).
- Le temps local du cue part de sa frontière de départ (`started_beat`) : les cues évolutifs (T-157) démarrent en phase.
- *Mode beat* (inspiré de Showcontroller) : sur la page courante, change de cue tous les N temps parmi les cues actifs ou sélectionnés.
- Défaut : *Temps* quand le tempo tourne, *Off* sinon.

## Modèle de données
```rust
pub enum Quantize { Off, Beat, Bar }
pub struct PendingLaunch { pub slot: (u8, u8, u8), pub at_beat: f64 }
```

## Interface
Barre de la grille : *Quantif. : Off / Temps / Mesure* ; case en attente qui clignote au rythme.

## Critères d'acceptation
- [ ] Déclenché à beat 10,4 en *Temps* → démarre à 11,0
- [ ] Déclenché à beat 11,05 en *Temps* → démarre immédiatement (rattrapage)
- [ ] *Mesure* : départ au prochain multiple de 4
- [ ] *Off* : départ immédiat

## Tests
Unitaires sur la quantification avec horloge simulée.

## Notes
T-111 quantifie déjà le lancement des cues évolutifs : factoriser dans une seule fonction de quantification utilisée par la grille, T-111 et la timeline. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
