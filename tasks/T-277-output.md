---
id: T-277
title: Lieu et projecteurs multiples (modèle `Venue`, `/api/frames`)
status: todo
area: output
priority: P1
depends_on: [T-275]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#12-how-to-build-it-in-the-browser
---

## Contexte
Un look festival se conçoit avec plusieurs projecteurs placés dans un lieu réel (scène, régie, public). BEYOND 5.5 permet de régler la salle, la position/rotation, l'angle de scan et la luminosité de chaque projecteur, et d'enregistrer la disposition.

## À faire
- Nouveau `studio/src/venue.rs` : `Venue` (salle, scène, zone public, écrans en gaze, projecteurs, réglages de rendu), sauvegardé dans le projet (T-286) ou `studio-data/venue.json` en attendant.
- Préréglages : *Club* (15×10×5 m), *Salle* (30×20×10 m), *Festival plein air* (60×40 m, sans plafond).
- Projecteur : nom, position (m), lacet/tangage/roulis (°), angle de scan (°), luminosité (%), sortie associée, *Miroir X* (pour les rigs symétriques tant qu'il n'y a qu'une sortie).
- `GET /api/frames` : une image par projecteur (tant qu'il n'y a qu'une sortie, la même image, avec miroir appliqué côté UI). Quand T-012/T-170 existent, chaque projecteur montre l'image de sa sortie/zone.
- `GET/POST /api/venue`.
- Onglet *Lieu* du Panneau : formulaire numérique (liste de projecteurs, ajouter/dupliquer/supprimer), sélection d'un projecteur = surbrillance dans la vue 3D.
- Écrans en gaze : quad (position, taille, rotation) ; les impacts dessus reconstituent le graphisme.

## Modèle de données
```rust
#[serde(default)]
pub struct Venue { pub preset: String, pub room: Room, pub stage: Rect3, pub audience: AudienceArea, pub screens: Vec<Screen>, pub projectors: Vec<Projector>, pub render: RenderSettings }
#[serde(default)]
pub struct Room { pub width_m: f32, pub depth_m: f32, pub height_m: f32, pub ceiling: bool }
#[serde(default)]
pub struct Projector { pub name: String, pub pos_m: [f32; 3], pub yaw_deg: f32, pub pitch_deg: f32, pub roll_deg: f32, pub scan_deg: f32 /* 40 */, pub brightness: f32 /* 1.0 */, pub output: Option<String>, pub mirror_x: bool }
#[serde(default)]
pub struct AudienceArea { pub rect_m: [f32; 4], pub min_height_m: f32 /* 3.0 */ }
```

## Interface
Libellés : *Lieu*, *Préréglage*, *Club*, *Salle*, *Festival plein air*, *Largeur*, *Profondeur*, *Hauteur*, *Plafond*, *Projecteurs*, *Ajouter un projecteur*, *Dupliquer*, *Supprimer*, *Position X/Y/Z (m)*, *Lacet*, *Tangage*, *Roulis*, *Angle de scan*, *Luminosité*, *Sortie*, *Miroir X*, *Écrans*, *Zone public*.

## Critères d'acceptation
- [ ] Ajouter un 2e projecteur avec *Miroir X* : la vue 3D montre deux éventails symétriques
- [ ] `/api/venue` enregistre et relit le lieu à l'identique (aller-retour JSON)
- [ ] Un fichier `venue.json` sans champ `screens` se charge (valeurs par défaut)
- [ ] Changer le préréglage redimensionne la salle sans supprimer les projecteurs
- [ ] `/api/frames` renvoie autant d'entrées que de projecteurs

## Tests
Unitaires Rust : sérialisation, valeurs par défaut, préréglages. e2e : ajout/duplication d'un projecteur, `/api/frames`.

## Notes
Le lieu n'a aucun effet sur la sortie laser (données de visualisation seulement). La zone public sert aux surcouches de T-279. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
