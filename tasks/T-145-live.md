---
id: T-145
title: Identifiants de contrôle stables (registre des contrôles)
status: todo
area: live
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §6 (F)
---

## Contexte
Tous les réglages (curseurs en direct, cases de la grille, tempo, timeline) doivent avoir un identifiant stable pour que l'interface, le MIDI/APC40 (T-200+), l'OSC (T-013), les enveloppes de timeline et le routage audio pilotent exactement la même chose.

## À faire
- Nouveau module `studio/src/controls.rs` : un registre de descripteurs de contrôles et une fonction `apply(id, value)` qui modifie l'état partagé.
- Grammaire des identifiants (minuscules ASCII, séparés par des points, **jamais renommés** une fois livrés ; ajouter des alias si besoin) :
  - `master.<param>` : modificateurs en direct maîtres (liste dans T-140/T-141/T-142) ;
  - `layer.<1-4>.<param>` : calques (T-156) ;
  - `cue.<id-du-cue>.<param>` : modificateurs d'un cue (T-144) ;
  - `grid.<page 1-10>.<ligne 1-5>.<colonne 1-8>` : case de la grille (matrice 5×8 = APC40) ;
  - `fx.<ligne 1-4>.<case 1-8>` et `fx.<ligne>.action` : grille FX (T-146) ;
  - `tempo.tap`, `tempo.resync`, `tempo.bpm`, `tempo.nudge_up`, `tempo.nudge_down`, `tempo.double`, `tempo.half` ;
  - `page.next`, `page.prev`, `page.<n>` ; `transport.blackout`, `transport.freeze`, `transport.black_hold` ;
  - `timeline.play`, `timeline.stop`, `timeline.loop` ; `vlj.enabled`.
- Types : continu (min, max, défaut, unité), bascule, momentané (vrai tant que tenu), déclencheur (une fois), choix (liste).
- Valeurs : en unités natives (`value`) **ou** normalisées 0..1 (`norm`, pour le MIDI) ; le registre convertit et borne.
- Le registre est construit au démarrage ; les autres tâches y ajoutent leurs contrôles (fonction `register_*` par module).
- **Armer le laser** : contrôle `transport.arm` avec `external: false` par défaut (seuls le bouton de l'interface et Espace arment). L'armement depuis un contrôleur n'est possible que via l'option explicite de T-208 (Shift + maintien 1 s) ; `transport.blackout` est toujours externe et prioritaire.
- Alias : accepter `live.<param>` comme alias de `master.<param>` (exemple utilisé dans T-202).

## Modèle de données
```rust
pub struct ControlId(pub String);
pub enum Unit { None, Percent, Deg, DegPerSec, Hz, Beats, Bpm }
pub enum ControlKind {
    Continuous { min: f32, max: f32, default: f32, unit: Unit },
    Toggle { default: bool },
    Momentary,
    Trigger,
    Choice { options: Vec<&'static str>, default: usize },
}
pub struct ControlDesc { pub id: ControlId, pub label_fr: &'static str, pub group: &'static str, pub kind: ControlKind, pub external: bool }
pub struct ControlRegistry { descs: Vec<ControlDesc>, by_id: HashMap<String, usize> }
```

## Interface
- `GET /api/controls` → liste des descripteurs (id, libellé français, groupe, type, bornes).
- `POST /api/control` `{ "id": "master.size", "value": 1.2 }` ou `{ "id": "master.size", "norm": 0.6 }` ; momentané : `{ "id": "transport.black_hold", "value": true|false }`.
- `GET /api/control-values` → valeurs courantes (pour le retour LED de l'APC40).
- Dans l'interface : infobulle de chaque contrôle = son identifiant (aide au mapping MIDI).

## Critères d'acceptation
- [ ] Chaque id est unique ; un test échoue si deux descripteurs ont le même id
- [ ] `norm` 0 et 1 donnent exactement min et max ; les valeurs hors bornes sont bornées
- [ ] Un id inconnu renvoie HTTP 404 avec un message clair, sans panique
- [ ] `transport.arm` est refusé via `/api/control` tant que l'option de T-208 n'est pas activée
- [ ] Un fichier `docs/controls.md` généré par un test (ou `--list-controls`) liste tous les ids

## Tests
Unitaires : unicité, conversion norm↔valeur, bornage, refus d'armer. e2e : `POST /api/control master.size` change `/api/frame`.

## Notes
Les ids servent de contrat avec les agents MIDI (T-200+) : tout renommage casse leurs mappings. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
