---
id: T-245
title: Sécurité de la réactivité audio (limiteur, péremption, silence, pas d'armement)
status: todo
area: safety
priority: P0
depends_on: [T-237, T-101]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §6.4
---

## Contexte
L'audio peut piloter luminosité et strobes plusieurs fois par seconde : ce chemin doit respecter les mêmes garde-fous que le reste.

## À faire
- Toute modulation audio de luminosité/visibilité passe par le limiteur de stroboscope (T-101) : fréquence maximale de flashs réglable (défaut 10 Hz, jamais au-delà de la limite du limiteur).
- Audio périmé, appareil perdu ou source changée → toutes les modulations audio retombent vers leur valeur neutre avec la relâche (jamais de gel sur une valeur haute).
- Silence : action configurable (*garder le look*, *look calme*, *noir*), défaut *garder le look* sans modulation.
- L'audio et les déclencheurs audio (T-240) ne peuvent jamais armer, désarmer ni lever un blackout ; Échap reste immédiat.
- La sécurité (zones, luminosité max, calibration bornée) reste appliquée en dernier, après les modulations audio.

## Modèle de données
```rust
pub enum SilenceAction { Keep, CalmLook(String), Blackout }
#[serde(default)] pub struct AudioSafety { pub max_flash_hz: f32 /*10*/, pub silence_action: SilenceAction /*Keep*/ }
```

## Interface
Dans « Musique » : *Flashs max / s*, *En cas de silence* (*Garder*, *Look calme*, *Noir*).

## Critères d'acceptation
- [ ] Kicks simulés à 20 Hz routés vers la luminosité : ≤ `max_flash_hz` flashs par seconde dans `/api/frame`
- [ ] Coupure de l'audio pendant un pic : valeurs neutres atteintes en ≤ release + 1 trame
- [ ] Aucun événement audio ne change `armed`
- [ ] Le rendu respecte toujours la luminosité maximale et les zones (test sur le pire cas)

## Tests
Unitaires sur le chemin de modulation + sécurité ; e2e : flux simulé à haute fréquence → comptage des flashs dans les trames successives.

## Notes
Aucun test ne doit utiliser `--device`. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
