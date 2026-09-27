---
id: T-141
title: Couleur en direct : fixe, teinte, palette, arc-en-ciel, chenillard
status: review
area: live
priority: P1
depends_on: [T-140, T-150]
owner: "agent-dev"
branch: "feat/live-color"
source: docs/research/pro-live-operation.md §1.3, §1.4, §6 (A)
---

## Contexte
Le laseriste recolore tout le show pour suivre l'éclairagiste ou l'énergie du morceau, sans changer de cue. C'est le curseur « Color » de QuickShow, le « Colorspectrum » et les palettes de Showcontroller, et les « color channels » de BEYOND.

## À faire
- `ColorOverride` dans `LiveModifiers` (maître, puis calques/cues via T-144) :
  - *Normal* : couleurs du cue inchangées ;
  - *Fixe* : une couleur RGB pour tous les points allumés (luminosité du point conservée) ;
  - *Teinte* : garde saturation et valeur, remplace la teinte (0..360°) ;
  - *Palette* : mode *Plus proche* (chaque couleur → couleur la plus proche de la palette) ou *Pas à pas* (nouvelle couleur de la palette à chaque changement de couleur le long du tracé), avec un *Décalage* qui fait tourner la palette ;
  - *Arc-en-ciel* : teinte le long du tracé (*Étalement* 0..4 cycles) qui défile (vitesse en Hz ou en temps) ;
  - *Chenillard* : couleurs de la palette qui avancent d'un pas tous les N temps (1/8..4), réparties sur *Tout*, *Par trait* ou *Par point*.
- 8 palettes intégrées créées par nous : Froid, Chaud, Feu, Océan, Néon, Forêt, Tricolore, Blanc pur ; palettes utilisateur (jusqu'à 16 couleurs) dans `studio-data/palettes.json`.
- Les points éteints (blanking) restent éteints ; la luminosité maître s'applique après.
- Contrôles : `master.color.mode`, `master.color.hue`, `master.color.palette`, `master.color.offset`, `master.color.rate`, `master.color.spread`.

## Modèle de données
```rust
pub enum PaletteMode { Nearest, Step }
pub enum ChaseSpread { Whole, Stroke, Point }
pub enum ColorOverride {
    Normal,
    Fixed { rgb: [u8; 3] },
    Hue { hue: f32 },
    Palette { palette: usize, mode: PaletteMode, offset: usize },
    Rainbow { spread: f32 /*1.0*/, rate: Rate /*Beats(4.0)*/ },
    Chase { palette: usize, step: Rate /*Beats(1.0)*/, spread: ChaseSpread /*Stroke*/ },
}
pub struct Palette { pub name: String, pub colors: Vec<[u8; 3]> } // 1..=16
```

## Interface
Dans « Direct » (T-143), bloc *Couleur* : boutons *Normal, Fixe, Teinte, Palette, Arc-en-ciel, Chenillard* ; sélecteur de couleur ; bande de teinte cliquable ; vignettes de palettes ; *Pas* (1/8, 1/4, 1/2, 1, 2, 4 temps) ; *Étalement*. Clic droit = *Normal*.

## Critères d'acceptation
- [x] *Normal* ne change aucun point
- [x] *Fixe* rouge : tous les points allumés sont rouges avec leur intensité d'origine ; les points éteints restent à 0
- [x] *Chenillard* au pas de 1 temps : la couleur change exactement aux frontières de temps de `TempoClock` (±1 frame)
- [x] *Palette* plus proche : un point vert pur devient la couleur de palette la plus proche (distance RGB)
- [x] Les palettes utilisateur survivent au redémarrage

## Tests
Unitaires : chaque mode sur un frame de test, frontières de temps du chenillard. e2e : bouton *Fixe* → couleurs de `/api/frame`.

## Notes
Réutiliser les palettes nommées de T-130 (agent festival) si elles existent : un seul type `Palette` dans l'appli ; T-130 colore les looks, cette tâche recolore en direct par-dessus. Les palettes et leurs noms sont les nôtres, pas celles de Showcontroller ou de Pangolin. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
- 2026-09-27 — agent de dev (feat/live-color) : `ColorOverride` + `ColorParams` (mémoire des réglages par mode) dans `LiveModifiers`, `Rate` Hz/temps lu sur `TempoClock` à chaque image, 8 palettes intégrées + `PaletteStore` (`palettes.json`, 8 palettes utilisateur max, 1..16 couleurs), `/api/palettes`, contrôles `master.color.*` (mode, hue, palette, palette_mode, offset, rate, rate_hz, spread, chase_spread, red/green/blue), bloc *Couleur* du panneau « Direct » (clic droit = Normal). 87 tests unitaires (+16), clippy propre, vérifié en aperçu (API + Chrome). Pas de suite e2e dans l'arbre : le test e2e *Fixe* a été vérifié à la main via l'API. Note : `docs/prs/live-color.md`.
