---
id: T-148
title: Pilote automatique (Virtual LJ) calé sur le tempo
status: todo
area: cues
priority: P3
depends_on: [T-159]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §2.6
---

## Contexte
Laisser tourner le show quand l'opérateur s'absente, ou lui donner une base qu'il enrichit à la main (« comme deux laseristes »).

## À faire
- Déclenche un cue de la page courante toutes les N mesures (1, 2, 4, 8), *Dans l'ordre* ou *Aléatoire* (sans répéter le précédent), en respectant quantification et transitions.
- Option : changement de page aléatoire parmi une sélection de pages.
- Les cases avec `vlj_skip` sont ignorées.
- Le déclenchement manuel reste possible pendant qu'il tourne.
- Contrôle : `vlj.enabled`, `vlj.every`, `vlj.order`.

## Modèle de données
```rust
#[serde(default)]
pub struct Autopilot { pub enabled: bool, pub every_bars: u8 /*4*/, pub random: bool, pub pages: Vec<u8> }
```

## Interface
Bouton *Pilote auto* à côté du BPM ; menu : *Toutes les 1/2/4/8 mesures*, *Ordre / Aléatoire*, *Pages*. Case de propriété *Ignorer par le pilote*.

## Critères d'acceptation
- [ ] Toutes les 4 mesures à 120 BPM : un changement toutes les 8 s (±1 frame)
- [ ] *Aléatoire* ne rejoue jamais deux fois de suite le même cue
- [ ] Une case *Ignorer* n'est jamais choisie
- [ ] N'arme jamais le laser

## Tests
Unitaires avec horloge simulée et graine fixe.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
