---
id: T-111
title: Moteur de cues évolutifs (images clés en temps)
status: done
area: cues
priority: P1
depends_on: [T-100]
owner: "dev-agent (evolving)"
branch: feat/evolving
source: docs/research/festival-looks.md#5-twelve-pre-made-evolving-cues
---

## Contexte
Un cue évolutif change de lui-même sur 8 à 32 temps (par exemple un éventail qui se lève et accélère jusqu'au drop). C'est ce qui donne l'impression d'un LJ pro sans toucher à rien.

## À faire
1. Nouveau contenu `Content::Evolving { length_beats, loop, keys }`. Chaque image clé : `at_beats`, `generator`, overrides de `GenParams`, couleur, luminosité, gate/strobe, `ease` vers la clé suivante.
2. Interpolation : champs numériques (a, b, scale, luminosité, period_beats, couleurs RGB) interpolés selon l'easing ; champs discrets (générateur, count, mode couleur, forme de chase) changés exactement au temps de la clé.
3. `beat_pos` du cue évolutif = temps depuis le lancement, quantifié : lancement au prochain temps (défaut) ou à la prochaine mesure.
4. Fin : boucle si `loop`, sinon reste sur la dernière clé (ou « enchaîner vers le cue suivant », utilisé par T-160).
5. Les cues évolutifs sont des `Preset` comme les autres et sont sauvegardables dans les scènes.

## Modèle de données
```rust
pub struct EvolvingKey { pub at_beats: f32, pub generator: String, pub params: GenParams,
  pub color: [u8;3], pub brightness: f32, pub gate_beats: f32, pub strobe_div: f32, pub ease: Easing }
Content::Evolving { length_beats: f32, #[serde(default)] loop_: bool, keys: Vec<EvolvingKey> }
```
Easing : `Step, Linear, EaseIn, EaseOut, Smooth`.

## Interface
Dans l'aperçu : barre de progression du cue évolutif (temps courant / longueur, graduée par mesure). Pas d'éditeur de clés dans cette tâche (lecture seule, JSON possible via l'API).

## Critères d'acceptation
- [x] Un cue de 16 temps avec 2 clés interpole linéairement `scale` à mi-parcours
- [x] Changement de générateur exactement au temps de la clé (± 1 image)
- [x] Boucle : à `length_beats` on revient à la clé 0
- [x] Lancement quantifié au temps suivant
- [x] Un cue évolutif sauvé dans une scène se recharge

## Tests
Unitaires : interpolation, bascule discrète, boucle, sérialisation. e2e : lancer un cue évolutif de test, lire `/api/state` à deux instants.

## Notes
La structure doit rester simple : T-160 (timeline) enchaînera des cues évolutifs sur une grille de mesures.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
- 2026-09-28 · dev-agent (evolving) · branche `feat/evolving`, rebasée sur develop 82353c0 (après T-101). Nouveau module `studio/src/evolving.rs` : `Content::Evolving(EvolvingCue { length_beats, loop, launch: beat|bar, keys })`, `EvolvingKey` (générateur, `GenParams`, couleur, taille, luminosité, gate, `strobe_div`, `ease`), easing `KeyEase` (`step`, `linear`, `ease_in`, `ease_out`, `smooth` ; nommé `KeyEase` car `generators::Easing` existe déjà — c'est lui que T-157 doit réutiliser). Nombres interpolés (couleur, taille, luminosité, gate ; a, b, speed, color2 et période si la clé suivante a le même générateur), le reste bascule au temps de la clé. La période interpolée garde une phase continue (∫ 1/période). Lancement quantifié au temps suivant (défaut) ou à la mesure ; boucle ou maintien de la dernière clé, avec `ended` dans la progression pour l'enchaînement de T-160 (pas d'enchaînement ici). Progression dans `/api/frame` et `/api/state` (`evolving`), barre sous l'aperçu graduée par mesure, note lecture seule dans « Contenu ». Aucun cue ajouté au catalogue (ids et empreintes inchangés). Tests : `cargo test` 318 OK (+19), clippy propre, e2e complet 74 OK (dont `evolving.spec.ts`, 3 tests, 5 répétitions). Note PR : `docs/prs/evolving.md`.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
