---
id: T-288
title: Versions du format (migrations) et versions nommées
status: todo
area: infra
priority: P2
depends_on: [T-286]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#32-what-we-should-do
---

## Contexte
Le format du projet va évoluer à chaque nouvelle fonction. Un projet d'il y a un an doit toujours s'ouvrir. L'utilisateur veut aussi garder des états nommés de son show (« avant festival », « version courte »).

## À faire
- `format_version` entier ; chaîne de migrations `fn migrate_v1_to_v2(v: serde_json::Value) -> Result<Value>` appliquées sur le JSON brut avant la désérialisation.
- Un projet d'une version **plus récente** que l'appli s'ouvre en lecture seule avec un avertissement (jamais d'écrasement silencieux).
- Avant une migration, copie de l'original en `<nom>.v<N>.bak.lsproj`.
- Fichiers d'exemple par version dans `studio/tests/fixtures/projects/` (créés par nous, sans contenu tiers), tous testés à l'ouverture.
- **Versions nommées** : *Enregistrer une version…* (nom libre) → `<data-dir>/versions/<projet>/<AAAAMMJJ-HHMM>-<nom>.lsproj` ; liste avec date et nom ; *Ouvrir comme copie* ; *Supprimer* (avec confirmation).

## Modèle de données
```rust
pub const PROJECT_FORMAT: u32 = 1;
pub fn migrate(v: serde_json::Value) -> anyhow::Result<(serde_json::Value, bool /* migré */)>;
```

## Interface
Libellés : *Enregistrer une version…*, *Versions*, *Ouvrir comme copie*, *Projet créé par une version plus récente : ouverture en lecture seule*, *Projet mis à jour vers le nouveau format (copie de sauvegarde créée)*.

## Critères d'acceptation
- [ ] Chaque fichier d'exemple de `fixtures/projects/` s'ouvre sans erreur
- [ ] Un projet `format_version: 99` s'ouvre en lecture seule et *Enregistrer* est désactivé
- [ ] Une migration crée la copie `.bak` avant d'écrire
- [ ] Une version nommée s'ouvre comme copie sans modifier le projet courant

## Tests
Unitaires Rust : chaîne de migrations, version future, copie `.bak`. e2e : enregistrer une version, l'ouvrir comme copie.

## Notes
Garder `#[serde(default)]` sur toutes les structures (règle CLAUDE.md pour `Settings`). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
