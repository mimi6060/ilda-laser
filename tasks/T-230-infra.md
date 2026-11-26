---
id: T-230
title: Capture audio native (cpal, CoreAudio) sur un fil dédié
status: done
area: infra
priority: P1
depends_on: []
owner: "dev-agent (audio-capture)"
branch: feat/audio-capture
source: docs/research/audio-analysis.md §3
---

## Contexte
Aujourd'hui l'analyse tourne dans l'onglet du navigateur : si l'onglet est masqué ou fermé, le laser ne réagit plus à la musique (minuteries bridées, `AUDIO_STALE` à 500 ms). La capture doit vivre dans le serveur.

## À faire
- Nouveau module `studio/src/audio/mod.rs` + `capture.rs` : ouverture d'une entrée avec `cpal` (Apache-2.0), 48 kHz si possible, tampon demandé 256 trames (`BufferSize::Fixed`, repli sur le défaut).
- Le callback audio **n'alloue pas et ne verrouille pas** : il mixe en mono (moyenne des canaux) et pousse dans un anneau SPSC `rtrb` (MIT/Apache) ; horodatage par compteur d'échantillons + `Instant` du premier callback.
- Un fil d'analyse lit l'anneau par blocs de 256 échantillons (hop) et publie un instantané (T-237).
- Liste des entrées (`GET /api/audio/devices`), choix persistant dans la config (`--audio-device <nom>` en CLI, `none` pour désactiver), reprise automatique si l'appareil disparaît puis revient (réessai toutes les 2 s).
- Source audio globale : `Native` (défaut si une entrée est choisie), `Navigateur` (l'actuel `POST /api/audio`, conservé), `Aucune`.
- Vumètre : niveau RMS et crête en dBFS exposés même sans autre analyse.

## Modèle de données
```rust
pub enum AudioInputSource { Native, Browser, None }
#[serde(default)]
pub struct AudioConfig { pub source: AudioInputSource /*Native*/, pub device: Option<String> /*None = défaut système*/,
    pub buffer_frames: u32 /*256*/ }
pub struct CaptureStats { pub sample_rate: u32, pub channels: u16, pub overruns: u64, pub rms_db: f32, pub peak_db: f32 }
```

## Interface
Section « Musique » : liste *Entrée audio* (appareils du Mac + *Navigateur* + *Aucune*), bouton *Écouter* / *Arrêter*, vumètre dBFS, texte d'état (*Pas d'entrée*, *Autorisation refusée ?*, *Débordements : N*).

## Critères d'acceptation
- [x] Avec un appareil d'entrée, `/api/state.audio.level_db` bouge alors qu'aucun onglet n'est ouvert (test unitaire avec source simulée ; à vérifier sur le Mac, voir la note de PR)
- [x] Le callback ne fait aucune allocation (vérifié par revue et par un test avec un allocateur compteur si possible)
- [x] Débrancher puis rebrancher l'interface USB : la capture reprend seule en < 5 s (simulé ; vraie interface USB à vérifier à la main)
- [x] `--audio-device none` : aucune capture, aucun message d'erreur répété
- [x] La source *Navigateur* fonctionne toujours comme avant (non-régression)

## Tests
Unitaires : mixage mono, conversion i16/f32, anneau (débordement compté, jamais bloquant) avec un faux producteur. Pas de test sur vrai matériel en CI : le module expose un trait `SampleSource` que les tests alimentent avec des signaux synthétiques. e2e : sélection *Aucune* puis *Navigateur* via l'API.

## Notes
Sur macOS, la permission micro est demandée au terminal qui lance le studio (voir T-242). Ne pas utiliser de tampon plus petit que 128 trames (charge CPU). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-28 — dev-agent (audio-capture), branche `feat/audio-capture` : module `studio/src/audio/` (`capture.rs` callback sans allocation → anneau `rtrb`, trait `SampleSource` + `CpalSource` ; `worker.rs` fils *audio-capture* (ouverture, liste toutes les 2 s, reprise toutes les 2 s) et *audio-analysis* (sauts de 256) ; `analysis.rs` vumètre dBFS + `level`/`bass`/`beat` provisoires ; `mod.rs` `AudioConfig` dans `audio.json`, sources *Native*/*Navigateur*/*Aucune*, repli navigateur). API `GET /api/audio/devices`, `GET|POST /api/audio/config`, `/api/state.audio`. CLI `--audio-device`, `--no-audio` (ajouté au lanceur e2e et au test d'arrêt). **Interface « Musique » non faite** (index.html en cours de réorganisation par un autre agent) : l'API suffit, à reprendre dans T-242/T-243 ou une petite tâche. Licences : cpal Apache-2.0, rtrb MIT/Apache, dépendances macOS MIT/Apache/Zlib/BSD-2. `cargo test` 476 ok (3 ignorés), clippy propre, e2e 122/122. Vraie capture non ouverte par l'agent (invite micro macOS). Note : `docs/prs/audio-capture.md`.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné ; source par défaut = Navigateur (pas d'ouverture du micro sans choix explicite).
