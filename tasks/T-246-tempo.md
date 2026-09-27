---
id: T-246
title: Compensation de latence : décalage de sortie réglable et temps prédits
status: todo
area: tempo
priority: P2
depends_on: [T-234]
owner: ""
branch: ""
source: docs/research/audio-analysis.md §2.7
---

## Contexte
Le son, l'analyse, la boucle moteur et le tampon du DAC ajoutent 35 à 80 ms : un flash « sur le temps » arrive en retard. Les effets cadencés doivent utiliser le temps prédit, décalé de la latence mesurée.

## À faire
- Réglage global *Décalage de sortie* (−100..+100 ms, défaut 0) appliqué à la phase lue par les effets (`beat_at(t + offset)`), sans toucher à l'horloge elle-même.
- Retard d'analyse connu (demi-fenêtre + anticipation) retranché des horodatages de détection avant le recalage de phase (T-234).
- Estimation indicative de la latence de sortie par appareil (tampon du DAC ÷ débit de points) affichée à côté du réglage.
- Assistant *Caler à l'oreille* : clic métronome sur le Mac + flash laser (aperçu) au même temps ; l'utilisateur ajuste jusqu'à coïncidence.

## Modèle de données
```rust
#[serde(default)] pub struct LatencyConfig { pub output_offset_ms: f32 /*0*/, pub analysis_delay_ms: f32 /*calculé*/ }
```

## Interface
Dans la barre « Tempo » (menu avancé) : *Décalage de sortie (ms)*, *Latence estimée : 32 ms*, bouton *Caler à l'oreille*. Contrôle `tempo.output_offset` dans le registre (T-145).

## Critères d'acceptation
- [ ] Décalage +40 ms : les effets cadencés franchissent chaque temps 40 ms plus tôt (test sur temps simulé)
- [ ] Le décalage ne modifie ni `bpm` ni `beat_at` de l'horloge
- [ ] Horodatages de détection corrigés du retard d'analyse (test avec signal à transitoire connue)

## Tests
Unitaires sur la lecture décalée de phase et la correction d'horodatage.

## Notes
Le décalage s'applique aux effets cadencés au temps ; les réactions directes aux onsets (T-238) ne peuvent pas être avancées et gardent leur latence. Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Licences : aucune dépendance GPL/AGPL dans le build par défaut (aubio, essentia, BTrack exclus) ; algorithmes réécrits depuis les publications, voir docs/research/audio-analysis.md §4.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/audio-analysis.md`.
