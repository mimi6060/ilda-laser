---
id: T-160
title: Timeline : modèle de show et lecteur (pistes, événements, carte de tempo)
status: done
area: timeline
priority: P2
depends_on: [T-150, T-156]
owner: "dev-agent (timeline)"
branch: feat/timeline
source: docs/research/pro-live-operation.md §3.1, §6 (C)
---

## Contexte
Deux usages : (1) un show calé sur un morceau précis, rejouable à l'identique ; (2) des phrases en mesures qui suivent le tempo en direct (timelines festival T-124–T-129). BEYOND gère les deux avec l'option « Follow System BPM ».

## À faire
- Nouveau `studio/src/timeline.rs` : `Show`, sauvegardé dans `studio-data/shows/<nom>.json`.
- **Base de temps** du show :
  - *Secondes* (show sur un morceau) : positions en secondes = référence ; temps et mesures calculés par la carte de tempo (changer le BPM d'une section ne déplace aucun événement, raison documentée par BEYOND) ;
  - *Temps* (phrase qui suit le tempo) : positions en temps, lecture pilotée par `TempoClock` ; lancement quantifié à la mesure suivante ; si le BPM change, la phrase accélère.
- Pistes *Cues* (liées à un calque 1–4) et *Bus* (enveloppes appliquées à toutes les pistes de leur calque).
- Événement : début/durée (dans la base de temps du show), source (cue du catalogue/grille ou look intégré), mode de temps du contenu (*Temps musicaux*, *Secondes*, *Ajuster* = étire le programme du cue à la durée), action de fin (*Arrêt*, *Garder*, *Continuer*), fondu d'entrée/sortie, transition vers le suivant, modificateurs.
- Lecteur : lecture/pause/arrêt/position, boucle de région ; à chaque frame il fournit les `ActiveCue` des événements actifs au même pipeline que le direct ; **les modificateurs maîtres en direct s'appliquent par-dessus** (show hybride).
- Un show peut être posé dans une case de la grille (cue de type show).
- Contrôles : `timeline.play`, `timeline.stop`, `timeline.loop`.

## Modèle de données
```rust
pub enum TimeBase { Seconds, Beats }
pub struct Show { pub name: String, pub time_base: TimeBase, pub tempo_map: Vec<TempoPoint>, pub audio: Option<AudioRef>,
    pub tracks: Vec<Track>, pub markers: Vec<Marker>, pub loop_region: Option<(f64, f64)> }
pub struct TempoPoint { pub at_s: f64, pub bpm: f32, pub beats_per_bar: u8 }
pub struct Track { pub name: String, pub kind: TrackKind /*Cues|Bus*/, pub layer: u8, pub mute: bool, pub solo: bool, pub events: Vec<Event> }
pub struct Event { pub id: u64, pub start: f64, pub len: f64 /*unités de time_base*/, pub source: EventSource, pub time: TimeMode,
    pub end: EndAction, pub fade_in: Rate, pub fade_out: Rate, pub to_next: Option<Transition>,
    pub modifiers: LiveModifiers, pub envelopes: Vec<Envelope> }
pub struct Marker { pub at: f64, pub name: String, pub color: [u8; 3] }
```

## Interface
API : `GET/POST /api/shows`, `POST /api/timeline/{load,play,pause,stop,seek,loop}`, état dans `/api/state.timeline { name, position, beat, bar, playing }`. L'éditeur est T-162.

## Critères d'acceptation
- [x] Show *Secondes* : un événement de 2 s à 4 s est actif exactement dans [4, 6[ s
- [x] Carte de tempo 120 puis 140 BPM à 30 s : conversion secondes↔temps continue et exacte
- [x] Show *Temps* : passer de 128 à 150 BPM en cours de lecture garde l'événement courant sur la même mesure (pas de saut)
- [x] Show *Temps* lancé à la mesure 3,5 : démarre à la mesure 4,0
- [x] Boucle de région : la position revient au début sans saut visible
- [x] Les modificateurs maîtres s'appliquent aussi en lecture de timeline
- [x] Le lecteur n'arme jamais le laser ; Échap arrête la sortie et la timeline

## Tests
Unitaires : activité des événements dans les deux bases de temps, carte de tempo, fondus, boucle ; lecture simulée 60 s.

## Notes
Les timelines festival T-124–T-129 utilisent la base *Temps*. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
- 2026-09-28 — agent de développement (branche `feat/timeline`) : `timeline.rs` (modèle `Show`, carte de tempo, lecteur, `ShowStore` → `studio-data/shows/`). Les événements actifs sont rendus par le moteur dans le même mix de calques, étage direct, calibration, limiteur de sécurité et porte de sortie que les cues. Contrôles `timeline.play/pause/stop/loop` ; API `GET/POST /api/shows`, `GET /api/timeline`, `POST /api/timeline/{load,play,pause,stop,seek,loop}`, `timeline` dans `/api/state` et `/api/frame` ; case de grille « Show » (`CueSlot.show`) ; section UI *Timeline* minimale (choix du show, Lecture/Pause/Arrêt/Boucle, tête de lecture, clic = se placer). Adapté : durées de fondu/transition en `Dur` (secondes ou temps) au lieu de `Rate` ; enveloppes et pistes *Bus* stockées mais évaluées par T-163 ; *Ajuster* étire un programme nominal de 16 temps (en attendant T-157) ; *Morph* joué comme un fondu enchaîné. Échap : la timeline s'arrête sur place (position gardée) et *Lecture* est refusée tant que l'arrêt d'urgence n'est pas réinitialisé. Tests (après rebase sur develop 04227e5) : 355 unitaires OK (2 ignorés), clippy `-D warnings` propre, e2e 86/86 (dont `timeline.spec.ts`, 6 tests). Statut → review.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
