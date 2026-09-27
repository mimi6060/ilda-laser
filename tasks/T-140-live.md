---
id: T-140
title: Étage de modificateurs en direct maître (géométrie, luminosité, vitesse)
status: todo
area: live
priority: P1
depends_on: [T-145]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §1 et §6 (A)
---

## Contexte
Pendant qu'un cue joue, le laseriste change en permanence taille, position, rotation, vitesse et luminosité de tout ce qui sort. C'est la « Live Control » maître de QuickShow/BEYOND.

## À faire
- Nouveau `studio/src/live.rs` : `LiveModifiers` appliqué à la liste de points **après** le rendu du look et **avant** calibration + sécurité.
- Ordre des opérations : vitesse d'animation (multiplie le temps des générateurs) → taille (XY puis X, Y ; négatif = retournement) → rotations X/Y (3D + perspective) → rotation Z → position → luminosité.
- Rotation : angle fixe + vitesse ; la vitesse peut être en °/s ou, avec *Sync tempo*, en tours par mesure, **phase calculée depuis `TempoClock`** (T-150 si fait, sinon temps libre).
- Préréglages de vitesse de rotation : Lent 30 °/s, Moyen 90 °/s, Rapide 270 °/s ; en *Sync tempo* : Lent = 1 tour / 4 mesures, Moyen = 1 tour / mesure, Rapide = 1 tour / 2 temps. *Inverser* (momentané) inverse le sens tant qu'il est tenu.
- `LiveModifiers::default()` est l'identité : le frame de sortie est identique bit à bit.
- Enregistre dans le registre T-145 : `master.brightness`, `master.size`, `master.size_x`, `master.size_y`, `master.pos_x`, `master.pos_y`, `master.rot_x.angle`, `master.rot_y.angle`, `master.rot_z.angle`, `master.rot_x.speed`, `master.rot_y.speed`, `master.rot_z.speed`, `master.rot.preset` (choix Stop/Lent/Moyen/Rapide), `master.rot.sync`, `master.rot.reverse` (momentané), `master.perspective`, `master.speed`.

## Modèle de données
```rust
#[serde(default)]
pub struct LiveModifiers {
    pub brightness: f32,          // 0..1, 1.0
    pub size: f32,                // 0..2, 1.0
    pub size_x: f32, pub size_y: f32, // -2..2, 1.0
    pub pos_x: f32, pub pos_y: f32,   // -1..1, 0.0
    pub rot_angle: [f32; 3],      // degrés -180..180, [0,0,0]
    pub rot_speed: [f32; 3],      // °/s -720..720 ou tours/mesure si rot_sync
    pub rot_sync: bool,           // false
    pub rot_reverse: bool,        // false (momentané)
    pub perspective: f32,         // 0..1, 0.3
    pub speed: f32,               // 0..4, 1.0
    // champs ajoutés par T-141 (couleur) et T-142 (strobe, tracé, miroir…)
}
```
Stocké dans l'état partagé (`Shared.master: LiveModifiers`), persistant dans `studio-data/live.json`.

## Interface
Rien d'autre que l'API dans cette tâche (le panneau est T-143). `GET/POST /api/live` pour l'objet complet ; chaque champ est aussi accessible par `/api/control`.

## Critères d'acceptation
- [ ] `LiveModifiers::default()` ne change aucun point (test d'égalité sur 5 cues du catalogue)
- [ ] Taille 2,0 double les coordonnées avant calibration ; taille X −1 retourne l'image horizontalement
- [ ] Rotation Z à 90 °/s : +90° après 1 s simulée (±0,5°) ; *Inverser* tenu 1 s ramène à 0°
- [ ] Vitesse 0 fige les générateurs ; 2,0 les fait aller deux fois plus vite
- [ ] Aucun point hors −1..1 après calibration (le bornage existant reste en place)
- [ ] Coût < 1 ms pour 2000 points en release

## Tests
Unitaires sur chaque transformation + identité + perf (bench simple). e2e : `POST /api/control master.size` → étendue du frame plus grande.

## Notes
Le masque de sécurité (T-003) s'applique après : un déplacement en direct ne doit jamais faire entrer du faisceau dans une zone interdite. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
