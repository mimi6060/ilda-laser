---
id: T-233
title: Estimation du BPM et suivi des temps (autocorrélation, peigne, programmation dynamique) avec confiance
status: in-progress
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
- [ ] Clics synthétiques à 128 BPM : verrouillage en < 8 s, erreur < 0,5 BPM
- [ ] Motif 4 temps à 174 BPM : 174 (ou 87 documenté) ; à 70 BPM avec contretemps : 140 ou 70, choix documenté et testé
- [ ] Tempo qui passe de 124 à 128 BPM : nouveau tempo adopté en < 4 s
- [ ] Break de 16 temps sans percussions : l'état passe à *Maintien*, le BPM ne bouge pas
- [ ] Silence : *Pas d'entrée*, BPM inchangé
- [ ] Erreur de phase des temps prédits < 20 ms en régime établi sur le motif synthétique

## Tests
Unitaires sur signaux générés (tempos fixes, changement de tempo, break, bruit sans pulsation → confiance < 0,3).

## Notes
Remplace l'implémentation navigateur prévue dans T-152 : l'architecte doit re-cadrer T-152 (interface/API) ou la fermer au profit de T-233/T-234. BTrack (GPL-3) et aubio (GPL-3) ne doivent pas être lus comme code source de référence : on part des articles. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
