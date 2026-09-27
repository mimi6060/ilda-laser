---
id: T-169
title: Enregistrer le jeu en direct dans la timeline
status: todo
area: timeline
priority: P3
depends_on: [T-160, T-145]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.3
---

## Contexte
Transformer une bonne prise improvisée en show éditable : on joue sur le morceau, tout est enregistré.

## À faire
- Bouton *Enregistrer* : pendant la lecture de la timeline, chaque déclenchement de cue devient un événement (calque, début, fin) et chaque changement de contrôle continu devient des nœuds d'enveloppe (réduits : un nœud au plus toutes les 50 ms, simplification Douglas-Peucker).
- Quantification optionnelle des débuts au temps.
- Écrit dans de nouvelles pistes *Prise n*.

## Modèle de données
```rust
pub struct Recorder { pub armed: bool, pub quantize: Quantize, pub take: u32, events: Vec<Event>, env: HashMap<String, Vec<Key>> }
```

## Interface
*● Enregistrer* dans la barre de transport ; pistes *Prise 1, Prise 2…*

## Critères d'acceptation
- [ ] Jouer 3 cues pendant 30 s produit 3 événements aux bons instants (±1 frame, ou alignés au temps si quantifié)
- [ ] Un curseur bougé en continu produit ≤ 20 nœuds par seconde
- [ ] Réécouter la prise redonne le même rendu (±1 frame)

## Tests
Unitaires sur l'enregistreur et la simplification.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
