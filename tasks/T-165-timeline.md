---
id: T-165
title: Modèles de timeline (phrases prêtes à poser)
status: todo
area: timeline
priority: P2
depends_on: [T-160, T-163]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.3, §6 (C)
---

## Contexte
Remplir un morceau en quelques minutes : poser un modèle « Montée 16 mesures » ou « Drop » sur une région, choisir les cues, et c'est programmé.

## À faire
- Un modèle = une phrase de N mesures avec des *emplacements* (A, B, C…) au lieu de cues précis, des événements positionnés en temps, des enveloppes en temps, des modificateurs.
- *Appliquer* : sur une région (entre deux marqueurs ou sélection), l'utilisateur associe chaque emplacement à un cue (ou *Auto* : choix dans une catégorie du catalogue) ; le modèle est étiré ou répété au nombre de mesures ; le résultat devient des événements normaux (copie, modifiable ensuite).
- *Enregistrer comme modèle* à partir d'une sélection d'événements (les cues deviennent des emplacements).
- Stockage `studio-data/templates/*.json` ; les modèles intégrés sont en code (T-166).

## Modèle de données
```rust
pub struct Template { pub name: String, pub length_bars: u16, pub slots: Vec<String>, pub events: Vec<TemplateEvent>, pub bus_envelopes: Vec<Envelope> }
pub struct TemplateEvent { pub track: u8, pub start_beat: f32, pub len_beats: f32, pub slot: String,
    pub modifiers: LiveModifiers, pub envelopes: Vec<Envelope /*en temps*/> }
pub enum Fit { Stretch, Repeat, Crop }
```

## Interface
Panneau *Modèles* dans la timeline : liste, aperçu schématique, *Appliquer à la région*, choix des emplacements (*A : cue…*, *Auto : catégorie…*), *Ajuster : Étirer / Répéter / Couper*, *Enregistrer la sélection comme modèle*.

## Critères d'acceptation
- [ ] Appliquer un modèle de 8 mesures sur 16 mesures en *Répéter* : deux copies exactes, alignées aux mesures
- [ ] *Étirer* : positions multipliées par 2
- [ ] Enregistrer une sélection puis la réappliquer ailleurs donne les mêmes événements décalés
- [ ] Changer le BPM d'une section ne décale pas les événements déjà posés (secondes = référence)

## Tests
Unitaires : application, étirement/répétition, conversion en temps via la carte de tempo.

## Notes
Les timelines festival T-124–T-129 (base *Temps*) peuvent être enregistrées comme modèles une fois cette tâche faite. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
