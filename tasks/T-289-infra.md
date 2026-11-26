---
id: T-289
title: Import partiel, profil de site et paquet d'export `.lspack`
status: todo
area: infra
priority: P2
depends_on: [T-286, T-288]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#32-what-we-should-do
---

## Contexte
Un show se prépare chez soi et se joue ailleurs : il faut l'emporter avec ses médias, sans écraser la calibration et les zones de sécurité du lieu. On veut aussi reprendre une page ou des correspondances MIDI d'un autre projet. Chez BEYOND l'audio n'est pas dans l'espace de travail et une sauvegarde complète oblige à copier tout le dossier.

## À faire
- **Profil de site** (`<data-dir>/sites/<nom>.lssite`) : calibration, zones de sécurité et horizon (T-003), zones de projection (T-012), adresses des sorties. Un profil actif à la fois ; *Profils de site* dans *Réglages*. Migration : l'actuel `calibration.json` devient le profil *Par défaut*.
- À l'ouverture d'un projet qui embarque un profil : choix *Garder le profil actuel* (défaut) ou *Utiliser celui du projet*. Si le profil du projet est **moins restrictif** (zones plus petites, horizon plus haut, luminosité max plus élevée), liste des différences et confirmation explicite.
- **Paquet d'export** `.lspack` (zip) : le projet + profil de site optionnel + fichiers ILDA/audio référencés (avec SHA-256 dans un manifeste). *Importer un paquet* : décompresse dans `projects/<nom>/`, vérifie les empreintes, réécrit les chemins.
- **Import partiel** depuis un autre projet : cocher *Pages*, *Cues utilisateur*, *Scènes*, *Correspondances MIDI*, *Lieu*, *Timelines* ; conflit de nom → *Renommer*, *Remplacer*, *Ignorer*.

## Modèle de données
```rust
#[serde(default)]
pub struct SiteProfile { pub format_version: u32, pub name: String, pub calibration: Calibration, pub safety: Option<serde_json::Value>, pub zones: Option<serde_json::Value>, pub outputs: Option<serde_json::Value> }
pub struct PackManifest { pub project: String, pub site: Option<String>, pub media: Vec<MediaEntry> }
pub struct MediaEntry { pub path: String, pub sha256: String, pub bytes: u64 }
```
Dépendance : crate `zip` (licence MIT/Apache, à vérifier).

## Interface
Libellés : *Profils de site*, *Profil actif*, *Garder le profil actuel*, *Utiliser celui du projet*, *Le profil du projet est moins restrictif : …*, *Exporter un paquet…*, *Importer un paquet…*, *Importer depuis un projet…*, *Renommer*, *Remplacer*, *Ignorer*.

## Critères d'acceptation
- [ ] Ouvrir un projet avec profil embarqué ne change pas la calibration active tant que l'utilisateur ne choisit pas *Utiliser celui du projet*
- [ ] Un profil moins restrictif déclenche la liste des différences et demande une confirmation
- [ ] Exporter puis importer un paquet sur un `--data-dir` vide redonne le même projet et les mêmes médias (empreintes égales)
- [ ] Un paquet avec un chemin `../` est refusé (pas d'écriture hors du dossier cible)
- [ ] Importer seulement *Correspondances MIDI* ne touche ni aux pages ni aux scènes
- [ ] La calibration reste bornée à -1..1 après import

## Tests
Unitaires Rust : comparaison de restriction des profils, manifeste et empreintes, refus « zip slip », import partiel avec conflits. e2e : export/import avec `--data-dir` temporaires, sans `--device`.

## Notes
Les paquets contiennent seulement les médias de l'utilisateur ; ne jamais committer de paquet ni de fichier ILDA/audio dans le dépôt, et ne jamais importer de contenus Pangolin/Laserworld. Aucune importation n'arme le laser. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
