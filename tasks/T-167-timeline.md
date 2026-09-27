---
id: T-167
title: Génération automatique d'un show depuis l'analyse du morceau
status: todo
area: timeline
priority: P3
depends_on: [T-165, T-161, T-152]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.3 (CloudLase, Lazura)
---

## Contexte
Déposer un morceau et obtenir un premier show calé sur les temps et les sections, à retoucher ensuite : ce que proposent les logiciels récents.

## À faire
- Analyse hors ligne (serveur) : fonction d'onsets → tempo et grille de temps (programmation dynamique, d'après Ellis 2007) → premiers temps de mesure → courbe d'énergie et de nouveauté (Foote 2000) → sections classées *intro / couplet / montée / drop / pause / fin*.
- Résultat : carte de tempo, marqueurs de sections, puis application des modèles T-166 par type de section avec cues choisis par catégorie et énergie (graine aléatoire affichée, *Relancer la section*).
- Tout est éditable ensuite.

## Modèle de données
```rust
pub struct Analysis { pub bpm: f32, pub beats_s: Vec<f64>, pub downbeats_s: Vec<f64>, pub sections: Vec<Section> }
pub struct Section { pub start_s: f64, pub end_s: f64, pub kind: SectionKind, pub energy: f32 }
```

## Interface
Bouton *Créer un show automatiquement* après import ; barre de progression ; sections colorées sur la forme d'onde ; *Relancer la section* au clic droit.

## Critères d'acceptation
- [ ] Sur un morceau synthétique (clics à 128 BPM, 16 mesures calmes puis 16 mesures fortes), BPM 128 ± 0,5 et une frontière de section à ± 1 mesure
- [ ] L'analyse d'un morceau de 4 min prend < 10 s en release
- [ ] Même graine → même show

## Tests
Unitaires sur signaux générés (pas de musique tierce dans le dépôt).

## Notes
Réimplémentation à partir des publications ; pas de code copié d'une bibliothèque sous licence incompatible. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
