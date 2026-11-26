---
id: T-103
title: Chasers, coups sur le kick et strobes de faisceaux
status: todo
area: cues
priority: P1
depends_on: [T-100, T-101]
owner: ""
branch: ""
source: docs/research/festival-looks.md#a-beam-fans-the-backbone
---

## Contexte
Le chase de faisceaux et les « stabs » sur le kick sont le vocabulaire principal du techno et du hardstyle (Awakenings, Defqon.1). Il faut aussi un strobe de faisceaux calé sur le tempo.

## À faire
- `chase_fan` (look 7) : fan de N positions, le galvo visite toutes les positions (géométrie stable), seul le gate par faisceau change. Formes (`a` entier) : 0 avant, 1 arrière, 2 aller-retour, 3 centre → extérieur, 4 extérieur → centre, 5 pair/impair, 6 aléatoire (seedé par pas), 7 remplissage puis vidage. Traîne : `b` = longueur (0–3), intensités 1, 0.4, 0.15. Pas = `steps_per_beat`. Couleur de traîne = `color2` si mode Alternate.
- Modificateur de gate universel `gate_beats` (T-100) appliqué à n'importe quel générateur : allumé pendant `gate_beats` après chaque temps (stab, look 8). Option décroissance exponentielle τ = 0.12 temps. Option « temps et contretemps ».
- Strobe de faisceaux (look 10) : `strobe_div` ∈ {0, 1/2, 1/4, 1/8 temps}, rapport cyclique 30–50 %. Toujours soumis au limiteur T-101.

## Modèle de données
`GenParams` : `chase_shape: u8` (via `a`), `tail` (via `b`). Dans `Settings` : `gate: GateMode { Off, Beat, BeatAndOffbeat }`, `gate_beats: f32` (0.2), `gate_decay: bool`, `strobe_div: f32` (0 = off), `strobe_duty: f32` (0.4).

## Interface
Onglet Effet : « Chaser » (forme, traîne, pas). Bloc « Rythme » commun à tous les looks : Gate (Aucun / Temps / Temps + contretemps), Durée gate, Strobe (Aucun, 1/2, 1/4, 1/8). Raccourci : maintenir `S` = strobe 1/4 tant que la touche est enfoncée.

## Critères d'acceptation
- [ ] `chase_fan` forme 0, 8 faisceaux, 1 pas/temps : faisceau k allumé au temps k mod 8
- [ ] La traîne a les intensités 1 / 0.4 / 0.15
- [ ] Gate 0.2 temps à 128 BPM : allumé ≈ 94 ms après chaque temps, éteint ensuite
- [ ] Strobe 1/8 à 128 BPM coupé par le limiteur après 5 s
- [ ] Positions des faisceaux identiques d'une image à l'autre (seul le gate change)

## Tests
Unitaires : index de chase par forme, intensités de traîne, enveloppe de gate. e2e : cue chaser, deux lectures de `/api/frame` à 1/2 temps d'écart → faisceau allumé différent.

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
