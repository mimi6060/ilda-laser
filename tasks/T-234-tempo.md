---
id: T-234
title: Brancher la détection sur l'horloge de tempo : verrouillage, maintien, tap prioritaire, recalage de phase
status: done
area: tempo
priority: P1
depends_on: [T-233, T-150]
owner: "agent-dev (Claude)"
branch: feat/tempo-follow
source: docs/research/audio-analysis.md §5
---

## Contexte
La détection ne doit jamais faire sauter les effets : elle propose, `TempoClock` dispose. Le tap reste roi.

## À faire
- `TempoClock` accepte `TempoEstimate` seulement si `source == Audio` et `state == Locked` (confiance ≥ seuil, défaut 0,6).
- BPM : appliqué par `set_bpm` (phase continue) si stable 2 s et écart > 0,3 BPM.
- Phase (boucle à verrouillage de phase) : erreur `e` = temps détecté − temps d'horloge le plus proche, ramenée à ±½ temps ; correction `k·e` par temps, k = 0,2 par défaut, **plafonnée à 1/16 de temps par temps** ; |e| > ¼ temps ignorée sauf si elle persiste 4 temps (alors recalage franc, journalisé) ; terme intégral léger pour la dérive de BPM (≤ 0,05 BPM par mesure).
- *Maintien* : l'horloge continue au dernier BPM et à la dernière phase ; déverrouillage après 30 s.
- Priorités : Tap/Manuel > horloge MIDI (T-207) / Link (T-154) > Audio. Un tap bascule la source sur *Tap* ; bouton *Auto* pour revenir à *Audio*.
- *Guider* : un tap en mode *Audio* n'impose pas le BPM mais ajoute un a priori fort (±3 %) à l'estimateur (état *Guidé*).

## Modèle de données
```rust
#[serde(default)] pub struct AudioTempoConfig { pub min_confidence: f32 /*0.6*/, pub phase_gain: f32 /*0.2*/,
    pub max_phase_step_beats: f64 /*1.0/16.0*/, pub coast_timeout_s: f32 /*30*/ }
impl TempoClock { fn apply_detection(&mut self, est: &TempoEstimate, now: f64); fn guide_tap(&mut self, now: f64); }
```

## Interface
Sources de tempo : *Manuel*, *Tap*, *Audio*, (*MIDI*, *Link*) ; boutons *Auto* et *Guider*. Contrôles `tempo.auto`, `tempo.guide`, `tempo.new_track` dans le registre (T-145).

## Critères d'acceptation
- [x] Avec une erreur de phase simulée de ¼ temps, la correction par temps ne dépasse jamais 1/16 de temps
- [x] Estimations bruitées ±1 BPM autour de 128 : BPM affiché stable (±0,3)
- [x] Un tap en source *Audio* passe la source à *Tap* et ignore ensuite toute détection jusqu'à *Auto*
- [x] Passage *Verrouillé* → *Maintien* : `beat_at(t)` continue sans discontinuité
- [x] Aucune discontinuité de `beat_at` > 1/16 temps entre deux trames moteur, sur tout le scénario de test

## Tests
Unitaires sur `TempoClock` avec suites d'estimations simulées (bruit, dérive, saut d'octave, silence, taps). e2e : source *Audio* + `POST` d'estimations simulées → `/api/state.tempo`.

## Notes
Les horodatages de détection sont dans l'horloge audio : les convertir en temps moteur et retrancher le retard d'analyse avant comparaison (voir T-246). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
- 2026-09-29 — agent-dev (Claude), branche `feat/tempo-follow` : *Tempo auto* branché sur l'unique `TempoClock` (`TempoSource::Audio`, `AudioTempoConfig`, `FollowState`, `apply_detection` appelé par le moteur à chaque trame, `set_auto`, `guide_tap` → `AudioHub::set_guide` → `Analyzer::set_guide`, `new_track`). Contrôles `tempo.auto`, `tempo.guide`, `tempo.new_track` ; bouton *Auto*, *Guider* et état dans la barre de tempo ; hook e2e `POST /api/test/tempo_estimate` (`--test-hooks` seulement). Écart assumé : le recalage après 4 temps se fait au plafond de 1/16 de temps par temps (pas de saut), pour tenir le critère « aucune discontinuité > 1/16 ». MIDI/Link (T-207/T-154) et compensation de latence (T-246) restent à faire. cargo test vert (616 + 4 intégration), clippy propre, e2e 154/154. PR : `docs/prs/tempo-follow.md`.
- 2026-09-29 — architecte (review) : APPROUVÉ et fusionné dans develop.
