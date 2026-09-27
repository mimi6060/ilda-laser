---
id: T-152
title: Détection automatique du BPM depuis l'audio (avec confiance)
status: todo
area: tempo
priority: P2
depends_on: [T-150]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §4
---

## Contexte
Suivre un DJ sans taper en permanence. PangoBeats (Pangolin, 2026) montre la bonne approche : verrouiller quand on est sûr, « glisser » sinon, accepter des taps de guidage.

## À faire
- Dans le navigateur (Web Audio, où l'audio est déjà analysé) : flux spectral → fonction d'onsets ; estimation du tempo par autocorrélation sur 8 s ; préférence 90–180 BPM avec correction ×2/÷2 ; estimation de la phase des temps.
- États : *Vérification*, *Verrouillé*, *Maintien* (confiance basse : on garde le dernier BPM), *Guidé* (taps utilisateur ajoutés comme indices), *Pas d'entrée*.
- Envoi au serveur `POST /api/tempo/detect { bpm, confidence, beat_time }` seulement quand l'état est *Verrouillé* et que la source de tempo est *Audio* ; le serveur appelle `set_bpm` puis recale doucement la phase (au plus 1/16 de temps par temps).
- Boutons *Nouveau morceau* (oublie l'historique) et *Guider* (tap qui renforce la confiance sans imposer la valeur).

## Modèle de données
```rust
pub struct TempoDetect { pub bpm: f32, pub confidence: f32 /*0..1*/, pub beat_time: f64 }
// TempoClock: source == Audio ⇒ accepte TempoDetect si confidence >= 0.6 (configurable)
```

## Interface
Dans « Tempo » : source *Audio* ; jauge *Confiance* ; état en toutes lettres (*Verrouillé*, *Maintien*…) ; *Nouveau morceau*, *Guider* ; vumètre d'entrée avec zone cible −25..0 dBFS.

## Critères d'acceptation
- [ ] Sur un signal synthétique de clics à 128 BPM, verrouillage en < 8 s, erreur < 0,5 BPM
- [ ] Sur un signal à 70 BPM avec contretemps, résultat 140 (préférence 90–180) ou 70 : documenté et testé
- [ ] Silence : l'état passe à *Pas d'entrée* et le BPM ne change pas
- [ ] Le tap manuel reprend toujours la main (source *Tap*)

## Tests
Tests JS de l'algorithme (Node, signaux générés) ; unitaires Rust de l'acceptation et du recalage de phase.

## Notes
Ne pas embarquer de code sous licence incompatible ; s'inspirer des publications (Ellis 2007, Foote 2000) et réécrire. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
