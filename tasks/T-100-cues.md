---
id: T-100
title: Générateurs cadencés au beat (beat_pos, bpm, groupes)
status: todo
area: cues
priority: P1
depends_on: [T-150]
owner: ""
branch: ""
source: docs/research/festival-looks.md#47-engine-features-these-parameters-imply
---

## Contexte
Les looks de festival se mesurent en temps (1 tour par mesure, chase à 1/2 temps). Aujourd'hui les générateurs ne reçoivent que `t` en secondes et des événements de beat sans phase : impossible de caler un mouvement sur le premier temps. Cette tâche donne aux générateurs une position en beats et les briques communes (enveloppes, groupes de faisceaux, gate par faisceau) dont toutes les tâches T-102 à T-123 ont besoin.

## À faire
1. Passer aux générateurs un contexte `GenCtx { t, beat_pos, bpm, level, bass, scale }` au lieu des arguments séparés. `beat_pos` = nombre de temps (flottant) depuis le lancement du cue, recalé sur le premier temps de l'horloge de tempo.
2. Source du tempo : uniquement l'horloge de tempo de T-150 (tap, BPM auto, recalage). Pas d'horloge interne propre à cette tâche.
3. Helpers partagés dans `generators.rs` (ou `beat.rs`) :
   - `phase(beat_pos, period_beats) -> 0..1`
   - `env_stab(phase_in_beat, gate_beats, decay_beats) -> 0..1` (attaque instantanée, puis coupure ou décroissance exponentielle)
   - `ease_sine`, `ease_in`, `smoothstep`
   - `step_index(beat_pos, steps_per_beat) -> u64`
   - `seeded_rand(seed, i) -> f32` déterministe (pour étoiles, positions aléatoires)
4. Couleur et luminosité **par faisceau** : un générateur `dots` peut renvoyer une intensité 0..1 et une couleur optionnelle par point (pour chase, traîne, pair/impair). `colorize` les respecte.
5. Groupes virtuels (« têtes virtuelles ») : `groups: u32` (1 à 4) découpe les N faisceaux en groupes contigus ; `group_mode` = `unison | mirror | offset` s'applique aux mouvements (même phase, phase opposée, décalage 1/groups de cycle).
6. Les générateurs existants gardent exactement leur rendu actuel (ils continuent d'utiliser `t`).

## Modèle de données
```rust
pub struct GenCtx { pub t: f32, pub beat_pos: f64, pub bpm: f32, pub level: f32, pub bass: f32, pub scale: f32 }

// ajouts à GenParams, tous #[serde(default)]
pub period_beats: f32,      // défaut 4.0 — période du mouvement principal
pub steps_per_beat: f32,    // défaut 1.0 — pas de chase / positions
pub gate_beats: f32,        // défaut 0.0 = pas de gate
pub direction: i8,          // défaut 1 ; -1 = sens inverse
pub groups: u32,            // défaut 1
pub group_mode: GroupMode,  // défaut Unison
pub beat_sync: bool,        // défaut false : true = le look utilise beat_pos au lieu de t
```
Les scènes et presets existants doivent se recharger à l'identique (`#[serde(default)]`).

## Interface
Onglet Effet : section « Tempo du look » visible si `beat_sync` : Période (temps) 1/2/4/8/16/32, Pas par temps 1/2, 1, 2, 4, 8, Gate (temps), Sens (→ / ←), Groupes 1–4, Mode de groupe (Ensemble / Miroir / Décalé). Affichage du BPM courant et d'un indicateur de temps (1-2-3-4).

## Critères d'acceptation
- [ ] `generate` reçoit `GenCtx` ; les 20 générateurs existants produisent les mêmes points qu'avant (test de non-régression)
- [ ] Avec `beat_sync` et `period_beats = 4`, un mouvement revient à la même position tous les 4 temps, quel que soit le BPM
- [ ] Intensité et couleur par faisceau prises en compte par `colorize` et dans l'aperçu
- [ ] Groupes : en mode Miroir, deux groupes ont des décalages x opposés
- [ ] Anciennes scènes JSON rechargées sans erreur

## Tests
Unitaires : `phase`, `env_stab`, `step_index` à 128 et 150 BPM ; non-régression des 20 générateurs (même sortie pour même `t`) ; groupes miroir/décalé ; désérialisation d'un `GenParams` ancien.
e2e : activer « Tempo du look », régler 120 BPM, vérifier via `/api/state` que les champs sont sauvegardés.

## Notes
Coordonner avec les tâches tempo (T-140–T-199) : une seule horloge de tempo dans l'appli. Les générateurs doivent rester des fonctions pures de (params, ctx) pour être testables.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
- 2026-09-27 — architecte : dépend désormais de T-150 (une seule horloge de tempo dans l'appli) ; l'horloge temporaire est retirée du périmètre.
