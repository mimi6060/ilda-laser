---
id: T-150
title: Moteur de tempo : BPM, tap, resync, phase temps/mesure
status: todo
area: tempo
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §4 et §6 (D)
---

## Contexte
Tout ce qui est « au beat » (rotation synchro, chenillard couleur, strobe, cues évolutifs, timeline) doit lire une seule horloge de tempo, sinon les effets dérivent (leçon Globe/Resync de BEYOND).

## À faire
- Nouveau `studio/src/tempo.rs` avec `TempoClock` côté serveur, avancée par la boucle moteur à 60 im/s.
- `beat_at(t) = (t - origin) * bpm / 60` ; mesure = `floor(beat / beats_per_bar)` ; phase dans le temps et dans la mesure.
- `set_bpm` ré-ancre l'origine pour que la phase soit **continue** (aucun saut au changement de BPM).
- Tap : horodatages des 8 derniers taps ; remise à zéro si écart > 2 s ; BPM = 60 / médiane des intervalles dès 3 taps ; le dernier tap tombe sur un temps entier.
- Resync : l'instant présent devient le premier temps d'une mesure.
- Nudge ± : décale la phase de 1/32 de temps par appui ; ×2 et ÷2.
- BPM borné à 40..=250, défaut 120, 4 temps par mesure.
- Le `beat` détecté par le navigateur continue d'exister (flash audio) mais ne pilote pas l'horloge (ce sera T-152).
- Enregistre les contrôles `tempo.*` dans le registre (T-145 si déjà fait ; sinon routes dédiées, à brancher ensuite).

## Modèle de données
```rust
pub enum TempoSource { Manual, Tap, Audio, MidiClock, Link }
pub struct TempoClock {
    pub bpm: f64,            // 120.0
    pub beats_per_bar: u8,   // 4
    origin_s: f64,
    pub source: TempoSource, // Manual
    taps: VecDeque<f64>,     // max 8
}
impl TempoClock { fn beat_at(&self, t: f64) -> f64; fn set_bpm(&mut self, bpm: f64, t: f64);
  fn tap(&mut self, t: f64); fn resync(&mut self, t: f64); fn nudge(&mut self, beats: f64); }
```
`/api/state` expose `tempo: { bpm, beat, bar, beat_in_bar, source }`.

## Interface
Barre « Tempo » en haut : BPM en grand (éditable), 4 voyants de temps (le 1 plus visible), boutons *Tap* (touche Entrée), *Resync* (touche ⌫), *÷2*, *×2*, *◀* / *▶* (nudge). Touches lues via `e.code` (indépendant AZERTY/QWERTY). Espace et Échap restent réservés à la sécurité ; les lettres restent des cues.

## Critères d'acceptation
- [ ] 4 taps réguliers à 500 ms donnent 120,0 BPM (±0,1)
- [ ] Un tap isolé après 3 s de silence ne change pas le BPM
- [ ] Changer le BPM de 120 à 128 ne fait pas sauter `beat_at(now)` (écart < 1e-6)
- [ ] Après Resync, `beat_in_bar` vaut 0 à l'instant du resync
- [ ] Pas de dérive : après 1 h simulée à 128 BPM, `beat_at` = 7680,000 exactement (±1e-6)
- [ ] Entrée et ⌫ n'agissent pas quand le focus est dans un champ texte

## Tests
Unitaires sur `TempoClock` avec temps simulé (tap, médiane, bornes, continuité, resync, nudge). e2e : 4 appuis sur Entrée → `/api/state.tempo.bpm` proche du rythme tapé.

## Notes
Une seule horloge dans l'appli : T-100 prévoit une horloge provisoire pour `GenCtx.beat_pos` ; cette tâche la remplace (ou la reprend si T-100 est livrée avant) et fournit `beat_pos` à T-100/T-111. L'horloge MIDI (entrée/sortie) est T-207. Pangolin recommande le tap (« tap the space bar 10 times ») : le tap doit rester la méthode la plus fiable. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
