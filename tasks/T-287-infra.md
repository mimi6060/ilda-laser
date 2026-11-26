---
id: T-287
title: Sauvegarde automatique et récupération après plantage
status: todo
area: infra
priority: P1
depends_on: [T-286]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#32-what-we-should-do
---

## Contexte
Un plantage ou une coupure de courant en pleine préparation ne doit pas faire perdre le travail. Chez BEYOND, la sauvegarde automatique par script met la sortie en pause : la nôtre ne doit jamais toucher au moteur.

## À faire
- Sauvegarde automatique 30 s après la dernière modification (anti-rebond) et à l'arrêt propre, dans `<data-dir>/autosave/<nom>-<AAAAMMJJ-HHMMSS>.lsproj`, anneau des 20 plus récents par projet.
- Même écriture atomique que T-286, sur le thread d'E/S.
- Au démarrage, si une sauvegarde automatique est plus récente que le projet enregistré : dialogue *Récupérer* (ouvre la sauvegarde comme projet modifié) / *Ignorer*.
- Ligne d'état : *Sauvegarde auto 12:04* ; en cas d'échec d'écriture : avertissement orange, pas de blocage.
- Réglage : intervalle 10–300 s, désactivable.

## Modèle de données
`AutosaveSettings { enabled: bool /* true */, delay_s: u32 /* 30 */, keep: u32 /* 20 */ }` dans les préférences de l'appli (`studio-data/prefs.json`), pas dans le projet.

## Interface
Libellés : *Sauvegarde auto*, *Récupérer le travail non enregistré ?*, *Récupérer*, *Ignorer*, *Échec de la sauvegarde auto*.

## Critères d'acceptation
- [ ] Après une modification et 30 s d'attente (temps simulé en test), un fichier apparaît dans `autosave/`
- [ ] Jamais plus de 20 fichiers par projet
- [ ] Tuer le processus puis relancer propose *Récupérer* et restaure la modification
- [ ] Aucune image moteur en retard pendant une sauvegarde automatique
- [ ] Un dossier `autosave/` non inscriptible n'empêche pas le studio de tourner

## Tests
Unitaires Rust : anti-rebond avec horloge simulée, rotation de l'anneau, détection de la sauvegarde la plus récente. e2e : modification, attente (délai réglé à 1 s en test), redémarrage du serveur de test, dialogue.

## Notes
La récupération n'arme jamais le laser. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
