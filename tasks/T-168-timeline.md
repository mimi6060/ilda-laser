---
id: T-168
title: Timecode entrant (MTC, puis LTC par l'entrée audio)
status: todo
area: timeline
priority: P3
depends_on: [T-160]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.2
---

## Contexte
Caler le laser sur la régie (vidéo, son, lumière) dans les productions : c'est le standard des tournées.

## À faire
- MTC (quarter-frames + full frame) via l'entrée MIDI ; LTC décodé depuis une entrée audio (code biphase), 24/25/29,97 DF/30 im/s.
- Options : *Continuer si le timecode s'arrête* (sinon arrêt après 1 s), *Lissage du temps* (suit la vitesse détectée, défaut activé), décalage de show.
- Bouton *TC-IN* à armer explicitement ; après un blackout, le timecode ne relance pas la sortie tant que *TC-IN* n'est pas réactivé ; **le timecode n'arme jamais le laser**.
- Timecode hors des bornes du show ignoré.

## Modèle de données
```rust
pub struct TimecodeIn { pub source: TcSource /*Off|Mtc|Ltc*/, pub fps: TcFps, pub keep_running: bool /*false*/, pub smooth: bool /*true*/, pub offset_s: f64, pub armed: bool /*false*/ }
```

## Interface
Timeline : bouton *TC-IN*, affichage vert *TC 01:02:03:04* quand du timecode arrive, réglages *Source*, *Images/s*, *Continuer si perte*, *Lissage*, *Décalage*.

## Critères d'acceptation
- [ ] Quarter-frames MTC simulés : position à ± 1 image
- [ ] LTC généré par le test (fichier audio synthétique) décodé à ± 1 image
- [ ] Perte du timecode : arrêt après 1 s (ou continue si l'option est cochée)
- [ ] Après Échap, le timecode ne relance rien tant que *TC-IN* n'est pas réarmé

## Tests
Unitaires : décodage MTC, décodage LTC sur signal généré, logique TC-IN.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
