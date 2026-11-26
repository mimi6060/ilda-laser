---
id: T-157
title: Cues évolutifs : un cue = une mini-timeline (calques internes, courbes sur modificateurs, LFO)
status: todo
area: cues
priority: P1
depends_on: [T-111, T-151, T-140]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §2.1, §6 (B)
---

## Contexte
T-111 donne le moteur d'images-clés d'un contenu (`Content::Evolving`). Pour qu'un cue soit une vraie mini-timeline comme une scène Showcontroller LIVE, il faut en plus : plusieurs couches dans un même cue, des courbes sur les modificateurs (taille, position, rotation, couleur, strobe) et des LFO calés au tempo.

## À faire
- Étend T-111, **ne le remplace pas** : les étapes de contenu d'un cue restent des `Content::Evolving` (ou tout autre `Content`).
- `Settings.program: Option<CueProgram>` (serde default → `None` = comportement actuel).
- `CueProgram` : longueur en temps (défaut = celle du contenu évolutif, sinon 16), lecture *Boucle / Une fois / Aller-retour*, jusqu'à 4 **couches internes** (chacune un `Content`, dessinées l'une après l'autre, comme les « Surface » de Showcontroller), des **courbes** sur les paramètres `LiveModifiers` du cue (ids relatifs : `size`, `pos_x`, `pos_y`, `rot_z.angle`, `rot_z.speed`, `brightness`, `color.hue`, `strobe.on`, `trace`, `prism`…) et des **modulateurs** (T-151) locaux.
- Courbes : réutiliser l'`Easing` de T-111 (*Palier, Linéaire, Douce, Accélère, Ralentit*) ; pas de second type.
- Temps local du cue = `beat_pos` de T-100/T-111 (même origine, lancement quantifié) : couches, courbes et LFO sont en phase avec les clés de contenu.
- Évaluation déterministe : même temps local → même frame.
- API : `POST /api/cues/program` pour enregistrer un programme sur un cue utilisateur.

## Modèle de données
```rust
#[serde(default)]
pub struct CueProgram { pub length_beats: Option<f32>, pub play: PlayMode /*Loop*/,
    pub layers: Vec<Content> /*0..=4 couches en plus du contenu principal*/,
    pub lanes: Vec<Lane>, pub lfos: Vec<Modulator> }
pub enum PlayMode { Loop, Once, PingPong }
pub struct Lane { pub target: String /*param LiveModifiers relatif*/, pub keys: Vec<Key> }
pub struct Key { pub beat: f32, pub value: f32, pub ease: Easing /*de T-111*/ }
```

## Interface
Pas d'éditeur ici (T-164). Badge « ∿ » sur les cases dont le cue a un programme ; l'aperçu au survol joue le programme au BPM courant.

## Critères d'acceptation
- [ ] Un cue sans programme rend exactement comme avant (et un `Content::Evolving` seul aussi)
- [ ] Courbe `size` 0,2 → 1,0 linéaire sur 16 temps : 0,6 à 8 temps (±1e-3)
- [ ] Une couche interne + le contenu principal : les deux sont dans `/api/frame`, séparés par un déplacement éteint
- [ ] *Une fois* : reste sur la dernière valeur ; *Aller-retour* : repart en arrière à la fin
- [ ] Deux lancements au même temps local donnent des frames identiques
- [ ] 3 cues d'exemple (nos propres réglages) ajoutés au catalogue

## Tests
Unitaires : courbes sur modificateurs, couches, modes de lecture, déterminisme, chargement d'anciennes scènes.

## Notes
Dépend de T-111 (moteur d'images-clés de contenu, agent « festival ») : même `Easing`, même `beat_pos`. Les cues E1–E12 (T-112–T-123) peuvent ensuite ajouter des courbes de modificateurs sans toucher au moteur. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
