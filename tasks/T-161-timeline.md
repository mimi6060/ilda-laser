---
id: T-161
title: Fichier audio et forme d'onde dans la timeline
status: todo
area: timeline
priority: P2
depends_on: [T-160]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.1, §3.3
---

## Contexte
Programmer sur la musique : voir la forme d'onde, entendre le morceau, et que le laser suive l'audio sans dérive.

## À faire
- Import d'un fichier audio (WAV, MP3, AIFF, FLAC) dans `studio-data/media/audio/` ; décodage côté serveur (crate `symphonia`, licence MPL-2.0 à vérifier et consigner) ; lecture côté serveur (`cpal`/`rodio`), l'horloge audio devient l'horloge du transport.
- Pics de forme d'onde précalculés (min/max par bloc de 256 échantillons) servis par `GET /api/timeline/waveform?from=&to=&px=`.
- Décalage audio réglable (±500 ms) pour compenser la latence.
- Les fichiers audio de l'utilisateur ne sont jamais commités.

## Modèle de données
```rust
pub struct AudioRef { pub file: String, pub offset_s: f64 /*0*/, pub gain: f32 /*1.0*/ }
pub struct WaveformPeaks { pub block: usize /*256*/, pub min: Vec<f32>, pub max: Vec<f32>, pub sample_rate: u32 }
```

## Interface
Piste *Audio* en haut de la timeline : forme d'onde, nom du fichier, *Importer un morceau*, *Décalage*, *Volume*.

## Critères d'acceptation
- [ ] Un WAV généré de 10 s donne des pics cohérents (sinus d'amplitude 0,5 → max ≈ 0,5)
- [ ] Après 3 min de lecture, écart position audio / position timeline < 10 ms
- [ ] Fichier corrompu : erreur claire, pas de panique

## Tests
Unitaires : calcul des pics sur signaux générés, décodage d'un WAV généré par le test. Pas de fichier audio tiers dans le dépôt.

## Notes
Ajouter les dépendances et leur licence dans `docs/CONTENT_SOURCES.md` si nécessaire. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
