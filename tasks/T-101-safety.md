---
id: T-101
title: Limiteur de stroboscope et horizon appliqués à tous les looks
status: review
area: safety
priority: P1
depends_on: [T-100]
owner: "dev-agent (strobe-limit)"
branch: feat/strobe-limit
source: docs/research/festival-looks.md#44-strobe-and-gating
---

## Contexte
Les looks festival utilisent beaucoup le strobe et les faisceaux juste au-dessus du public. Il faut qu'aucun preset, cue évolutif ou timeline ne puisse dépasser les limites de sécurité, quelle que soit sa programmation.

## À faire
1. Limiteur de stroboscope global, appliqué après le générateur et avant la calibration : mesure la fréquence des coupures franches de luminosité (passages allumé → éteint de toute l'image). Au-dessus de 4 Hz, compter la durée ; après 5 s continues, forcer la sortie allumée en continu (pas de clignotement) pendant au moins 2 s avant d'autoriser à nouveau un strobe rapide.
2. Horizon `y_h` : tant que T-003 n'est pas fait, un simple plancher (réglage « Horizon des faisceaux », défaut 0.0) sous lequel les points `dots` (faisceaux) sont éteints. Quand T-003 est fait, utiliser son horizon et supprimer ce plancher provisoire.
3. Ces deux protections s'appliquent aussi en aperçu, pour que l'aperçu montre ce qui sortira.

## Modèle de données
```rust
pub struct StrobeLimiter { max_hz: f32 /* 4.0 */, max_burst_s: f32 /* 5.0 */, cooldown_s: f32 /* 2.0 */, ... }
```
Réglages dans `Settings` globaux (pas par look) : `strobe_max_hz`, `strobe_burst_s`, `beam_floor_y`.

## Interface
Section « Sécurité » : « Strobe max (Hz) » (lecture seule au-dessus de 4 sauf mode expert, hors périmètre ici), « Rafale max (s) », « Horizon des faisceaux ». Un voyant « Limiteur actif » s'allume quand il intervient.

## Critères d'acceptation
- [x] Un strobe à 8 Hz est coupé (sortie continue) après 5 s ± 0,1 s
- [x] Un strobe à 4 Hz n'est jamais limité
- [x] Aucun faisceau (`dots`) sous l'horizon dans `/api/frame`
- [x] L'arrêt d'urgence (Échap) et l'armement restent inchangés

## Tests
Unitaires : limiteur avec une suite d'images synthétiques (2, 4, 8, 17 Hz). Plancher : générateur de faisceaux à y négatif → aucun point allumé. e2e : cue strobe rapide, attendre 6 s, vérifier que `/api/frame` est stable.

## Notes
Voir T-003 (zones et horizon définitifs). Seuil 4 Hz et rafales ≤ 5 s : pratique photosensibilité reprise dans le rapport (section 4.4).

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
- 2026-09-28 — dev-agent (feat/strobe-limit) : `studio/src/safety.rs` (limiteur + horizon provisoire), branché dans `run_engine` après mix des calques / direct / calibration et juste avant la porte de sortie (demande de l'architecte ; la tâche disait « avant la calibration » — l'horizon est donc en coordonnées de sortie). Réglages globaux dans `safety.json` (`/api/safety`, resserrement seulement), section UI « Sécurité » avec voyant « Limiteur actif », `/api/frame.strobe`. 299 tests unitaires (+19 limiteur/horizon, +1 HTTP), clippy propre, e2e 71/71 (nouveau `safety.spec.ts`, 4 tests) ; `live.spec.ts:43` (Synchro tempo) a échoué 2 fois sur ~13 passes complètes sous charge, sans lien apparent. Note PR : docs/prs/strobe-limit.md. Statut → review.

