---
id: T-232
title: Fonction d'onsets (flux spectral) et détection kick / caisse claire / charleston
status: in-progress
area: tempo
priority: P1
depends_on: [T-231]
owner: "dev-agent (audio-onsets)"
branch: feat/audio-onsets
source: docs/research/audio-analysis.md §2.2 et §2.3
---

## Contexte
Le « beat » actuel se déclenche sur toute montée de basses (y compris la ligne de basse). Il faut de vrais onsets et savoir si c'est un kick, une caisse claire ou un charleston.

## À faire
- Flux spectral log-compressé redressé (Bello 2005, Dixon 2006), variante SuperFlux (filtre max sur 3 bins, décalage 2 trames, Böck & Widmer 2013) ; calculé sur tout le spectre **et** par bande (40–150 Hz, 150 Hz–5 kHz, 5–15 kHz).
- Sélection de pics causale : maximum local, au-dessus de moyenne mobile + δ, période réfractaire ≥ 30 ms ; anticipation réglable 0–2 hops.
- Règles : *kick* = onset basse bande à montée rapide sans dominance des aigus (réfractaire 100 ms) ; *caisse claire* = onset médium large bande (platitude élevée) ; *charleston* = onset aigu avec peu d'énergie < 2 kHz.
- Sorties : compteurs `onset`, `kick`, `snare`, `hat` (u64, comme `beat`) + force 0..1 + horodatage audio ; la fonction d'onsets (ODF) est conservée dans un tampon de 8 s pour T-233.

## Modèle de données
```rust
pub struct Onsets { pub onset: u64, pub kick: u64, pub snare: u64, pub hat: u64,
    pub kick_strength: f32, pub snare_strength: f32, pub hat_strength: f32, pub last_kick_t: f64 }
#[serde(default)] pub struct OnsetConfig { pub delta: f32 /*réglé sur le corpus T-244*/, pub lookahead_hops: u8 /*1*/,
    pub kick_refractory_ms: f32 /*100*/ }
```

## Interface
Trois voyants *Kick*, *Caisse*, *Charleston* à côté des vumètres ; réglage *Sensibilité* (δ).

## Critères d'acceptation
- [ ] Motif synthétique 4 temps kick + caisse sur 2 et 4 + charleston en croches à 128 BPM : F-mesure kick ≥ 0,95, caisse ≥ 0,85, charleston ≥ 0,8 (fenêtre ±50 ms)
- [ ] Une ligne de basse tenue sans kick ne produit pas de `kick`
- [ ] Latence entre la transitoire et l'événement ≤ 25 ms (mesurée en échantillons)
- [ ] `beat` hérité reste alimenté (= `kick` quand la source est native) pour la compatibilité

## Tests
Unitaires avec générateurs de kick (sinus glissant 150→50 Hz), caisse (bruit + 200 Hz), charleston (bruit passe-haut), ligne de basse. Métriques du banc T-244 quand il existe.

## Notes
Pas de modèle appris pré-entraîné de licence douteuse ; un petit classifieur entraîné sur nos propres enregistrements est possible plus tard. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
