---
id: T-239
title: Préréglages de réactivité audio (correspondances bandes → paramètres laser)
status: todo
area: live
priority: P2
depends_on: [T-153, T-238]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §6.1
---

## Contexte
L'utilisateur veut un résultat « pro » sans câbler dix routes. Des préréglages maison, fondés sur la règle graves = grands mouvements lents, aigus = petits détails rapides, événements = changements discrets.

## À faire
- Préréglages (nos propres réglages, rien de copié) appliqués en un clic comme ensemble de `AudioRoute` : *Punch* (kick → luminosité + taille), *Groove* (basses → taille, bas-médiums → rotation, caisse → changement de couleur), *Scintillement* (aigus → pointillés/points), *Festival* (combinaison + montée → vitesse, drop → pleine ouverture), *Calme* (basses → taille lente uniquement).
- Chaque préréglage indique les contrôles cibles par id (T-145) ; s'il manque un contrôle, la route est ignorée avec un avertissement.
- Jamais d'aigus continus sur la luminosité (scintillement).

## Modèle de données
```rust
pub struct AudioPreset { pub id: &'static str, pub name_fr: &'static str, pub routes: Vec<AudioRoute> }
pub fn builtin_presets() -> Vec<AudioPreset>;
```

## Interface
Menu *Préréglage audio* dans « Musique » : *Punch*, *Groove*, *Scintillement*, *Festival*, *Calme*, *Personnalisé*.

## Critères d'acceptation
- [ ] Appliquer *Punch* crée les routes attendues et une impulsion de kick augmente luminosité et taille dans `/api/frame`
- [ ] Aucun préréglage ne route `high` ou `hat` en continu vers `master.brightness`
- [ ] Changer de préréglage remplace les routes sans toucher aux autres réglages de la scène

## Tests
Unitaires : contenu des préréglages, validité des ids de contrôle. e2e : choix du préréglage + `POST /api/audio` simulé → trame modifiée.

## Notes
Contenu 100 % maison (voir CLAUDE.md, propriété intellectuelle). Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
