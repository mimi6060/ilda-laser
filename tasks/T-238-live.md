---
id: T-238
title: Conditionnement des signaux audio : seuil, courbe, attaque/relâche, enveloppes en temps musicaux
status: review
area: live
priority: P1
depends_on: [T-237, T-150]
owner: "dev-agent (T-238)"
branch: feat/audio-shaping
source: docs/research/audio-analysis.md §6.2
---

## Contexte
Brancher une bande brute sur un paramètre donne un laser qui tremble. Il faut une chaîne de mise en forme commune à toutes les routes audio.

## À faire
- Chaîne par route : source → seuil avec hystérésis → gain → courbe (linéaire, carré, racine, S) → suiveur d'enveloppe (attaque/relâche séparées, `y += (x−y)(1−exp(−dt/τ))` avec le vrai `dt`) → plage min..max.
- Sources événementielles (kick, caisse, charleston, onset, drop, temps, mesure) : déclenchent une enveloppe AD dont la décroissance peut être en ms **ou en fractions de temps** (lue sur `TempoClock`).
- Code dans `studio/src/audio/shape.rs`, pur et testable, utilisé par T-153 (routage).

## Modèle de données
```rust
pub enum Curve { Linear, Square, Sqrt, SCurve }
pub enum Decay { Ms(f32), Beats(f32) }
#[serde(default)] pub struct Shaper { pub gate: f32 /*0.05*/, pub hysteresis: f32 /*0.02*/, pub gain: f32 /*1*/,
    pub curve: Curve /*Linear*/, pub attack_ms: f32 /*10*/, pub release_ms: f32 /*150*/, pub decay: Decay /*Ms(120)*/ }
impl Shaper { fn process(&mut self, x: f32, dt: f32, beat_len_s: f32) -> f32; fn trigger(&mut self, strength: f32); }
```

## Interface
Éditeur de route (T-153) : champs *Seuil*, *Courbe*, *Attaque*, *Relâche*, *Déclin (ms / temps)* avec petite courbe de prévisualisation.

## Critères d'acceptation
- [x] Échelon 0→1 avec attaque 10 ms : 63 % atteints à 10 ms ±1 trame
- [x] Déclin `Beats(0.25)` à 120 BPM : retour sous 5 % en 125 ms ±1 trame
- [x] Signal oscillant autour du seuil ±0,01 : pas de battement grâce à l'hystérésis
- [x] Même résultat à 30 et 60 im/s (indépendance au pas de temps, ±2 %)

## Tests
Unitaires purs sur `Shaper` avec temps simulé.

## Notes
Réutilisable par les LFO (T-151) pour le lissage. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-29 — dev-agent (T-238), branche `feat/audio-shaping` : `audio/shape.rs` (`Shaper` : seuil à hystérésis, gain, courbe, suiveur attaque/relâche exact en `dt`, enveloppe AD sur événement, plage −1..1 ; `Span` ms ou temps pour attaque, relâche et déclin, lus sur la longueur de temps de `TempoClock` ; `Source` + `feed` depuis `AudioFeatures` ; `Target` lié via la liste autorisée des LFO). `lfo::offset` devient le passage unique LFO + audio (copies seulement, luminosité ne fait que baisser). Déclin mesuré depuis le pic (125 ms avec attaque 0, 135 ms avec 10 ms). Pas encore branché dans le moteur ni l'UI (T-153). 660 tests unitaires, clippy propre, e2e 166/166. Note : docs/prs/audio-shaping.md. Statut `review`.
