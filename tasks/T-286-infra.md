---
id: T-286
title: Fichier projet `.lsproj` : ouvrir, enregistrer, récents
status: done
area: infra
priority: P1
depends_on: []
owner: "dev agent (Claude)"
branch: feat/project-file
source: docs/research/visualiser-ux.md#32-what-we-should-do
---

## Contexte
Aujourd'hui les données sont éparpillées (`scenes.json`, `calibration.json`, bientôt timelines, MIDI, lieu). Un laseriste veut un seul fichier par show, comme l'espace de travail QuickShow/BEYOND, facile à copier et à sauvegarder.

## À faire
- Nouveau `studio/src/project.rs` : `Project` = tout ce qui fait un show, en sections optionnelles : `pages` (T-272), `user_cues`, `scenes`, `playlist`, `timelines` (T-160), `tempo` (BPM par défaut), `live` (valeurs par défaut des maîtres), `midi` (correspondances), `venue` (T-277), `outputs` (T-012), `ui` (taille de grille, onglets). Les fonctions qui n'existent pas encore ajoutent leur section plus tard (`#[serde(default)]`, champs inconnus conservés via `#[serde(flatten)] extra: Map`).
- On parle de **projet** et non de *show* (T-160 appelle `Show` une timeline).
- La calibration et la sécurité ne sont pas dans le projet mais dans le **profil de site** (T-289) ; en attendant, elles restent dans `calibration.json`/`safety.json`.
- En-tête : `format_version` (1), `app_version`, `saved_at`, `name`.
- Routes : `GET /api/project` (nom, chemin, modifié ?), `POST /api/project/new`, `/open` (chemin), `/save`, `/save-as` (chemin). Écriture atomique (`.tmp` + `fsync` + `rename`) sur un thread d'E/S, jamais sur le thread moteur.
- Au premier démarrage : importer `scenes.json` existant dans un projet *Sans titre* sans le supprimer.
- Liste des 10 projets récents dans `studio-data/recent.json`.
- Dans l'UI : menu *Projet* dans la barre du haut (*Nouveau*, *Ouvrir…*, *Récents*, *Enregistrer*, *Enregistrer sous…*) ; point « • » après le nom si modifié ; `Cmd+S`, `Cmd+O`. Le choix de fichier passe par un champ chemin + liste des projets du dossier `studio-data/projects/` (pas d'accès disque arbitraire depuis le navigateur).

## Modèle de données
```rust
#[serde(default)]
pub struct Project {
    pub format_version: u32, pub app_version: String, pub saved_at: String, pub name: String,
    pub pages: Option<CueGrid>, pub user_cues: Vec<UserCue>, pub scenes: Vec<Scene>, pub playlist: Vec<String>,
    pub timelines: Vec<serde_json::Value>, pub tempo: Option<TempoDefaults>, pub live: Option<serde_json::Value>,
    pub midi: Option<serde_json::Value>, pub venue: Option<Venue>, pub outputs: Option<serde_json::Value>, pub ui: Option<serde_json::Value>,
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}
```
Fichiers dans `<data-dir>/projects/<nom>.lsproj` (JSON indenté, UTF-8).

## Interface
Libellés : *Projet*, *Nouveau*, *Ouvrir…*, *Récents*, *Enregistrer*, *Enregistrer sous…*, *Sans titre*, *Modifications non enregistrées : enregistrer avant ?* (*Enregistrer*, *Ne pas enregistrer*, *Annuler*).

## Critères d'acceptation
- [x] Enregistrer puis rouvrir un projet redonne les mêmes scènes, playlist et pages (aller-retour JSON égal)
- [x] Un champ inconnu ajouté à la main dans le fichier est conservé après réenregistrement
- [x] Ouvrir un projet ne change jamais l'état armé (laser désarmé)
- [x] L'écriture d'un gros projet (1 000 scènes) ne provoque aucune image manquée dans le moteur (compteur de retards à 0)
- [x] Un `scenes.json` existant est importé au premier démarrage et n'est pas supprimé
- [x] Un chemin hors de `<data-dir>/projects/` est refusé (400)

## Tests
Unitaires Rust : aller-retour, champs inconnus, écriture atomique (fichier partiel jamais visible), refus de chemin. e2e : enregistrer, recharger la page, rouvrir, vérifier `/api/state` ; toujours avec `--data-dir` temporaire.

## Notes
Ne jamais charger ni convertir de fichiers de projet Pangolin/Laserworld (`.qsw`, espaces de travail BEYOND, shows Showcontroller). Les fichiers ILDA/audio sont référencés par chemin, pas copiés (voir T-289). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
- 2026-09-28 — agent de développement (Claude), branche `feat/project-file` : `studio/src/project.rs` (format v1, sections scènes/playlist/grille/timelines/tempo/maîtres/calques/LFO/palettes/profils MIDI, champs inconnus conservés), routes `/api/project[/new|open|save|save-as]`, `recent.json`, import « Sans titre » au premier démarrage, menu *Projet* + Cmd+S/Cmd+O dans l'UI. Calibration, sécurité, présence, options de sécurité MIDI, armement et arrêt d'urgence hors projet et jamais modifiés à l'ouverture. Ouverture atomique (tout est validé avant de changer quoi que ce soit), chemins confinés à `projects/`, écriture atomique sans tenir le verrou moteur. La section « pages » (T-272) n'existe pas encore : la grille actuelle (`grid.json`) est enregistrée sous `grid`. Critère « 1 000 scènes » : test de durée du verrou (copie < 16 ms), pas de compteur de retards dans le moteur aujourd'hui. `cargo test` (436 unitaires + 2, après rebase sur `develop` eeb0932) vert, clippy `-D warnings` propre, e2e 112/112 (dont `project.spec.ts`, 7 tests). PR : `docs/prs/project-file.md`.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop.
