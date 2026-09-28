---
id: T-233
title: Estimation du BPM et suivi des temps (autocorrélation, peigne, programmation dynamique) avec confiance
status: done
area: tempo
priority: P1
depends_on: [T-232, T-150]
owner: "dev-agent (audio-bpm)"
branch: feat/audio-bpm
source: docs/research/audio-analysis.md §2.4
---

## Contexte
Suivre le DJ sans taper en permanence. Version native (Rust) de l'idée de T-152, qui tourne même onglet fermé.

## À faire
- ODF ré-échantillonnée à ~100 Hz, fenêtre glissante 8 s, estimation toutes les 0,5 s.
- Autocorrélation (via FFT) + renforcement harmonique (peigne : τ, 2τ, 3τ, 4τ) + a priori log-gaussien centré sur 125 BPM (σ ≈ 1 octave) ; plage 40–250 BPM ; interpolation parabolique (erreur < 0,5 BPM).
- Phase : suivi causal par programmation dynamique (Ellis 2007 rendu en ligne comme Stark 2009) → `beat_time` (dernier temps) et `next_beat` prédit.
- Confiance 0..1 : rapport crête/moyenne du score × stabilité des 4 dernières estimations × clarté de pulsation ; seuils calibrés sur T-244.
- Hystérésis : un nouveau tempo doit gagner ≥ 2 s (≥ 4 s pour un saut ×2/÷2).
- *Nouveau morceau* : oublie l'historique ; automatique après > 3 s de silence.

## Modèle de données
```rust
pub struct TempoEstimate { pub bpm: f32, pub confidence: f32, pub beat_time: f64, pub next_beat: f64,
    pub state: DetectState }
pub enum DetectState { NoInput, Checking, Locked, Coasting, Guided }
```

## Interface
Dans la barre « Tempo » (T-150) : BPM détecté en petit à côté du BPM actif, jauge *Confiance*, état en toutes lettres (*Pas d'entrée*, *Vérification*, *Verrouillé*, *Maintien*, *Guidé*), bouton *Nouveau morceau*.

## Critères d'acceptation
- [x] Clics synthétiques à 128 BPM : verrouillage en < 8 s, erreur < 0,5 BPM
- [x] Motif 4 temps à 174 BPM : 174 (ou 87 documenté) ; à 70 BPM avec contretemps : 140 ou 70, choix documenté et testé
- [x] Tempo qui passe de 124 à 128 BPM : nouveau tempo adopté en < 4 s
- [x] Break de 16 temps sans percussions : l'état passe à *Maintien*, le BPM ne bouge pas
- [x] Silence : *Pas d'entrée*, BPM inchangé
- [x] Erreur de phase des temps prédits < 20 ms en régime établi sur le motif synthétique

## Tests
Unitaires sur signaux générés (tempos fixes, changement de tempo, break, bruit sans pulsation → confiance < 0,3).

## Notes
Remplace l'implémentation navigateur prévue dans T-152 : l'architecte doit re-cadrer T-152 (interface/API) ou la fermer au profit de T-233/T-234. BTrack (GPL-3) et aubio (GPL-3) ne doivent pas être lus comme code source de référence : on part des articles. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-28 — dev-agent (audio-bpm), branche `feat/audio-bpm` : `studio/src/audio/bpm.rs` (`BpmTracker` sur le fil d'analyse) : ODF tempo = somme des flux des 3 bandes de T-232 (sinon les charlestons font doubler le tempo), ~100 Hz, fenêtre 8 s pondérée vers le récent ; autocorrélation par FFT + peigne (τ…4τ) × a priori log-gaussien 125 BPM, interpolation cubique + parabolique ; hystérésis ±4 % suivi direct, 2 s sinon, 4 s pour ×2/÷2 ; confiance = clarté × crête/moyenne × présence (2 dernières s) × stabilité ; temps par programmation dynamique causale (Ellis/Stark) avec recalage sous-trame. `TempoEstimate` publié dans `NativeSnapshot.tempo` et `/api/state.audio.tempo` ; `POST /api/audio/tempo/new_track` (*Nouveau morceau*) ; `set_guide` côté estimateur prêt pour T-234 (non branché). L'horloge de tempo n'est pas touchée. Mesures : verrouillage 6,0 s, erreur ≤ 0,17 BPM de 70 à 180 ; 174 → 174 ; 70 avec contretemps → 140 (choix documenté, 70 sans charlestons ou guidé) ; 124 → 128 adopté en 2,85 s ; break de 16 temps : *Maintien* après 2,0 s, BPM figé, temps toujours prédits ; silence → *Pas d'entrée*, oubli après 3 s ; bruit : confiance 0 ; phase des temps prédits ≤ 4 ms ; aucune allocation par saut ; 0,023 % d'un cœur. `cargo test` 601 OK (rebasé sur develop d9cdf5b), clippy propre, e2e 150/150. Interface laissée à T-243. Note : `docs/prs/audio-bpm.md`. Statut → review.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
