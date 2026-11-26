---
id: T-274
title: Aperçu avant diffusion (préparer un cue sans l'envoyer)
status: todo
area: cues
priority: P2
depends_on: [T-270]
owner: ""
branch: ""
source: docs/research/visualiser-ux.md#21-what-pros-put-on-screen
---

## Contexte
Pour régler un cue ou choisir le suivant pendant qu'un autre joue, il faut le voir sans qu'il parte au laser. Les pros ont un mode aperçu séparé de la sortie.

## À faire
- Deuxième « Animator d'aperçu » dans le moteur, qui n'est jamais envoyé à une `Output`.
- Bouton *Préparer* dans la Scène : quand il est actif, un clic sur une case (ou `Alt+clic` sans l'activer) charge le cue dans l'aperçu préparé au lieu de la sortie ; l'onglet *2D+3D* montre alors *Sortie* et *Préparé* côte à côte, avec étiquettes.
- *Envoyer* (ou `Entrée` numérique `NumpadEnter`) remplace la sortie par le cue préparé, en respectant les transitions (T-158) si elles existent.
- `GET /api/preview-frame` renvoie l'image préparée.

## Modèle de données
```rust
pub struct PreviewSlot { pub settings: Option<Settings>, pub animator: Animator }
```
Dans `Shared` ; non sauvegardé.

## Interface
Libellés : *Préparer*, *Préparé*, *Sortie*, *Envoyer*. Bordure bleue autour de la vue *Préparé* pour la distinguer de la sortie.

## Critères d'acceptation
- [ ] En mode *Préparer*, cliquer une case change `/api/preview-frame` mais pas `/api/frame`
- [ ] *Envoyer* copie le cue préparé vers la sortie
- [ ] Le laser armé ne reçoit jamais l'image préparée (revue de code : l'aperçu n'a pas d'`Output`)

## Tests
Unitaires : l'aperçu préparé n'est pas routé vers la sortie. e2e : préparer, comparer `/api/frame` et `/api/preview-frame`, envoyer.

## Notes
`Entrée` principale est le *Tap* du tempo (T-150) : utiliser `NumpadEnter` ou le bouton. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld et des visualiseurs du marché, rien de copié (ni captures d'écran, ni icônes, ni noms d'effets, ni contenus).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/visualiser-ux.md`.
