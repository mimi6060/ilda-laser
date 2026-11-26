---
id: T-155
title: Modes de déclenchement des cues, groupes exclusifs, limiteur
status: done
area: cues
priority: P1
depends_on: [T-145]
owner: "dev-agent (cue-modes)"
branch: feat/cue-modes
source: docs/research/pro-live-operation.md §2.2–§2.3
---

## Contexte
Aujourd'hui un clic remplace le look. Un laseriste veut basculer, flasher tant que la touche est tenue, jouer en solo, relancer, et empiler plusieurs cues.

## À faire
- Remplace le « look unique » par une liste de cues actifs (`ActiveCue`) rendus et concaténés (le rendu multi-calques est T-156 ; ici calque 1 seulement).
- Modes de clic de la grille : *Basculer* (défaut), *Flash* (actif tant que tenu), *Solo* (tenu : coupe temporairement les autres), *Relancer* (redémarre à chaque clic). Mode par case possible (`CueSlot.mode`).
- *Un cue / Multi* : exclusif ou additif.
- *Groupes* 1..8 : un seul cue actif par groupe (le nouveau remplace l'ancien).
- Limiteur : `max_active` (défaut 4) ; au-delà on arrête le plus ancien non-flash.
- `Maj` tenu + lettre = flash temporaire de ce cue.
- Les cases de la grille sont adressables `grid.<page>.<ligne>.<colonne>` (5×8 par page, 10 pages) ; les lettres AZERTY actuelles restent mappées sur les 26 premières cases.

## Modèle de données
```rust
pub enum ClickMode { Toggle, Flash, Solo, Restart }
#[serde(default)]
pub struct CueSlot { pub cue: String, pub mode: Option<ClickMode>, pub group: Option<u8>, pub layer: u8 /*1*/,
    pub transition: Option<Transition>, pub quantize: Quantize, pub modifiers: LiveModifiers, pub vlj_skip: bool }
pub struct ActiveCue { pub slot: (u8, u8, u8), pub started_beat: f64, pub started_s: f64, pub held: bool }
pub struct CueDeck { pub active: Vec<ActiveCue>, pub click_mode: ClickMode, pub multi: bool, pub max_active: u8 }
```
Grille persistante : `studio-data/grid.json` (préremplie avec le catalogue de 202 cues si absente).

## Interface
Barre de la grille : *Basculer / Flash / Solo / Relancer*, *Un cue / Multi*, champ *Groupe* dans le menu contextuel d'une case (clic droit : *Propriétés du cue*). Case active surlignée ; case en flash d'une autre couleur.

## Critères d'acceptation
- [x] *Basculer* : clic 1 démarre, clic 2 arrête
- [x] *Flash* : actif entre appui et relâchement seulement (souris et clavier)
- [x] *Solo* tenu : seul ce cue sort ; relâché : les autres reviennent
- [x] Deux cues du même groupe : le second remplace le premier
- [x] 5 cues en *Multi* avec `max_active` 4 : le plus ancien s'arrête
- [x] Les scènes et la playlist existantes fonctionnent toujours

## Tests
Unitaires sur `CueDeck` (tous les modes, groupes, limiteur). e2e : flash au clavier, solo, groupes.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
- 2026-09-27 — dev-agent (cue-modes), branche `feat/cue-modes` : `cues.rs` (CueDeck, modes Basculer/Flash/Solo/Relancer, Un cue/Multi, groupes 1..8, limiteur `max_active`), rendu multi-cues concaténé avec trajets éteints, contrôles `grid.*` momentanés + `cue.mode/multi/max_active/stop_all`, API `/api/cue`, `/api/cues`, `/api/cues/slot`, barre de grille, Maj+lettre = flash, menu « Propriétés du cue », `grid.json` (propriétés par cue). Adapté : la grille reste dérivée du catalogue (8 pages, T-145) ; `layer/transition/quantize/modifiers/vlj_skip` laissés à T-156 et suivantes ; pas d'e2e (`studio/e2e/` absent). 89 tests OK, clippy propre, vérifié dans Chrome sur une instance d'aperçu. Note PR : `docs/prs/cue-modes.md`.
- 2026-09-27 — architecte (review) : APPROUVÉ et fusionné dans develop (conflit moteur résolu avec T-141). Suivi : relâcher les flashs si le navigateur disparaît (T-252).
