---
id: T-111
title: Moteur de cues évolutifs (images clés en temps)
status: todo
area: cues
priority: P1
depends_on: [T-100]
owner: ""
branch: ""
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
- [ ] Un cue de 16 temps avec 2 clés interpole linéairement `scale` à mi-parcours
- [ ] Changement de générateur exactement au temps de la clé (± 1 image)
- [ ] Boucle : à `length_beats` on revient à la clé 0
- [ ] Lancement quantifié au temps suivant
- [ ] Un cue évolutif sauvé dans une scène se recharge

## Tests
Unitaires : interpolation, bascule discrète, boucle, sérialisation. e2e : lancer un cue évolutif de test, lire `/api/state` à deux instants.

## Notes
La structure doit rester simple : T-160 (timeline) enchaînera des cues évolutifs sur une grille de mesures.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
