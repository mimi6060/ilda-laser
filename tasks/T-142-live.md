---
id: T-142
title: Strobe, points visibles, pointillés, miroir/prisme, figer, noir momentané
status: todo
area: live
priority: P2
depends_on: [T-140, T-150]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §1.2–§1.5
---

## Contexte
Les accents du laseriste sur les drops : strobe, noir avant le drop (Flash2Black), figer sur un coup, pointillés sur des faisceaux, miroir et prisme pour multiplier un motif.

## À faire
- *Stroboscope* : cadence en Hz (1..25) ou en temps (1/1..1/16), rapport cyclique 10..90 %, phase calée sur `TempoClock` ; mode verrouillé et mode *tenu* (momentané).
- *Points visibles* (tracé) : ne dessine que les N premiers % du chemin allumé (0..100 %, défaut 100).
- *Pointillés* : éteint k points allumés sur n (0 = plein, 1 = très espacé).
- *Miroir* : aucun, X, Y, XY (ajoute des copies miroirs ; attention au budget de points).
- *Prisme* : N copies tournées autour du centre (1 = désactivé, max 8).
- *Figer* (momentané) : fige le temps d'animation et de modulation tant que c'est tenu.
- *Noir* (momentané) : sortie noire tant que c'est tenu, **sans désarmer** (différent d'Échap).
- Contrôles : `master.strobe.on`, `master.strobe.hold`, `master.strobe.rate`, `master.strobe.duty`, `master.trace`, `master.dots`, `master.mirror`, `master.prism`, `transport.freeze`, `transport.black_hold`.

## Modèle de données
```rust
#[serde(default)]
pub struct Strobe { pub on: bool, pub rate: Rate /*Hz(8.0)*/, pub duty: f32 /*0.5*/ }
pub enum Mirror { None, X, Y, XY }
// ajoutés à LiveModifiers : strobe: Strobe, trace: f32 /*1.0*/, dots: f32 /*0.0*/, mirror: Mirror, prism: u8 /*1*/, freeze: bool, black: bool
```

## Interface
Boutons *Stroboscope* (bascule) et *Flash strobe* (tenu) ; curseurs *Cadence*, *Rapport* ; *Points visibles*, *Pointillés* ; *Miroir* (Aucun/X/Y/XY) ; *Prisme* 1–8 ; *Figer* et *Noir* (tenus). Raccourcis : `Maj` tenu = strobe tenu (ne pas utiliser de lettre).

## Critères d'acceptation
- [ ] Strobe 10 Hz / 50 % : sur 1 s à 60 im/s, 30 frames allumés ± 1
- [ ] Strobe en temps 1/4 : les fronts tombent sur les doubles-croches de `TempoClock` (±1 frame)
- [ ] *Noir* tenu : `/api/frame` sans point allumé ; relâché : retour immédiat ; `armed` inchangé
- [ ] *Points visibles* 50 % : environ la moitié des points allumés, uniquement au début du chemin
- [ ] *Prisme* 4 : 4 copies, point de départ de chaque copie précédé d'un déplacement éteint

## Tests
Unitaires sur chaque effet et sur le chronométrage du strobe. e2e : *Noir* tenu via `/api/control`.

## Notes
Le limiteur de stroboscope de T-101 (> 4 Hz pendant plus de 5 s → sortie continue 2 s) s'applique aussi au strobe en direct, après cet étage : ne pas le contourner. Afficher un avertissement épilepsie dans l'aide. Le strobe ne doit jamais augmenter la puissance crête. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
