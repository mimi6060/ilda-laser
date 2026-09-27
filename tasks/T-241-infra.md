---
id: T-241
title: Capturer le son du Mac lui-même (BlackHole documenté, puis capture système optionnelle)
status: todo
area: infra
priority: P3
depends_on: [T-230]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §3.2
---

## Contexte
Quand la musique est jouée sur le Mac (Spotify, fichier, logiciel DJ), l'utilisateur veut que le laser l'écoute sans câble.

## À faire
- Étape 1 : aide dans l'interface expliquant la voie BlackHole (pilote GPL-3 installé par l'utilisateur, non lié à notre binaire) : périphérique multi-sortie dans Configuration audio et MIDI, puis choisir *BlackHole 2ch* comme entrée.
- Étape 2 (optionnelle, fonction Cargo `system-audio`, désactivée par défaut) : source *Son du Mac* via les « process taps » Core Audio (macOS 14.2+) ou ScreenCaptureKit (crate `screencapturekit`, MIT/Apache), avec repli propre sur les versions plus anciennes.
- Documenter les permissions : *Enregistrement de l'écran* (ScreenCaptureKit) ou `NSAudioCaptureUsageDescription` (process taps).

## Modèle de données
```rust
pub enum AudioInputSource { Native, Browser, System /*feature system-audio*/, None }
```

## Interface
Dans *Entrée audio* : entrée *Son du Mac* (grisée si non compilée ou macOS trop ancien) et lien *Comment écouter le son du Mac ?*.

## Critères d'acceptation
- [ ] L'aide BlackHole est présente et exacte (relue sur un Mac)
- [ ] Sans la fonction `system-audio`, `cargo tree` ne contient pas `screencapturekit`
- [ ] Avec la fonction, un son joué par le Mac fait bouger les vumètres

## Tests
Étape 2 : test manuel documenté dans la note de PR (pas de capture système en CI).

## Notes
Ne jamais embarquer BlackHole (GPL-3) dans le dépôt ou le binaire. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
