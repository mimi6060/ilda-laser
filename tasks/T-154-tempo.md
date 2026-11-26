---
id: T-154
title: Ableton Link : étude de licence et intégration optionnelle
status: todo
area: tempo
priority: P3
depends_on: [T-150]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §4
---

## Contexte
Se caler sur le logiciel du DJ (Ableton Live, Rekordbox, Traktor…) par le réseau, avec le tempo et la phase. BEYOND 5.5+ le propose (bouton LINK). L'horloge MIDI est traitée à part (T-207).

## À faire
- **Étape 1 (obligatoire) : étude.** Le SDK Link est sous double licence GPLv2+ ou propriétaire (sur demande à Ableton) ; les crates Rust (`rusty_link` GPL-2+, `ableton-link-rs` GPL-3) héritent de cette contrainte. Rédiger `docs/research/ableton-link-licence.md` : options (projet sous GPL, fonction Cargo `link` désactivée par défaut et jamais distribuée, licence Ableton), conséquences, recommandation. **L'utilisateur décide** ; aucune dépendance GPL n'est ajoutée avant.
- **Étape 2 (si accord) :** source de tempo *Link* : BPM et phase de la session Link appliqués à `TempoClock` (quantum = 4 temps), changement local de BPM renvoyé à la session ; affichage du nombre de pairs.

## Modèle de données
```rust
// seulement derrière #[cfg(feature = "link")]
pub struct LinkSource { pub enabled: bool, pub quantum: f64 /*4.0*/, pub peers: usize }
```

## Interface
Source de tempo *Link* (grisée tant que la fonction n'est pas compilée) ; *Pairs : 2*.

## Critères d'acceptation
- [ ] Le document de licence est écrit et relu par l'utilisateur
- [ ] Sans la fonction `link`, le binaire ne contient aucune dépendance GPL (vérifié par `cargo tree`)
- [ ] Avec la fonction (si acceptée) : BPM identique à une session Link de test à ±0,01 et phase à ±5 ms

## Tests
Étape 2 : test d'intégration avec deux instances locales.

## Notes
Licences : voir docs/research/pro-live-operation.md §4. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
- 2026-09-27 — architecte : l'utilisateur confirme un usage **non commercial**. Décision : Ableton Link (GPLv2+) est autorisé, mais **uniquement derrière une feature Cargo optionnelle `ableton-link`, désactivée par défaut**. Raison : le SDK ShowNET de Laserworld (T-015) sera propriétaire et sous NDA, et un binaire qui contient à la fois du code GPL et ce SDK ne peut pas être distribué. Les deux ne doivent jamais être activés dans le même build distribué ; le documenter dans le README.
