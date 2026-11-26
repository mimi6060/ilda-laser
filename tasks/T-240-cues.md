---
id: T-240
title: Déclencheurs sur événements audio (kick, drop, break → cues)
status: todo
area: cues
priority: P2
depends_on: [T-232, T-236, T-155]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §2.6 et §6.1
---

## Contexte
Pour jouer seul ou automatiser une partie du show : changer de cue au drop, faire un flash sur chaque kick, passer au look calme sur un break.

## À faire
- Règles `événement → action` : événements `kick`, `snare`, `drop`, `break`, `buildup ≥ x`, `silence`, `toutes les N mesures` ; actions : lancer une cue, cue suivante de la page, flash d'une cue pendant N temps, noir momentané.
- Quantification optionnelle au prochain temps/mesure (T-159 si disponible).
- Limiteur : au plus une action par règle par intervalle minimal (défaut 1 temps) ; désactivation globale en un clic.
- Désactivé par défaut ; ne touche jamais à l'armement.

## Modèle de données
```rust
pub enum AudioEvent { Kick, Snare, Drop, Break, BuildupAbove(f32), Silence, EveryBars(u32) }
pub enum TriggerAction { LaunchCue(String), NextCue, FlashCue { id: String, beats: f32 }, BlackHold { beats: f32 } }
#[serde(default)] pub struct AudioTrigger { pub event: AudioEvent, pub action: TriggerAction, pub min_interval_beats: f32 /*1*/, pub enabled: bool }
```

## Interface
Onglet *Déclencheurs audio* : liste de règles (*Quand*, *Alors*, *Intervalle min.*), interrupteur général *Automatisme audio* (contrôle `audio.triggers.enabled`, affectable au MIDI).

## Critères d'acceptation
- [ ] Règle drop → cue X : sur le scénario synthétique de T-236, la cue X est lancée une fois à ±1 temps du drop
- [ ] Kick à 128 BPM avec intervalle min. 1 temps : au plus 1 action par temps
- [ ] Interrupteur général coupé : aucune action
- [ ] Laser désarmé : les cues changent dans l'aperçu, rien n'arme le laser

## Tests
Unitaires sur le moteur de règles avec événements simulés. e2e : règle créée par l'API + événements simulés → cue active dans `/api/state`.

## Notes
Toute sortie passe par le limiteur de strobe (T-101) et la sécurité. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
