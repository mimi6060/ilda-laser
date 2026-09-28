---
id: T-298
title: Isoler le décodage audio (MP3) pour qu'un fichier piégé ne puisse pas arrêter le studio
status: in-progress
area: infra
priority: P1
depends_on: []
owner: "dev-agent (decode-isolation)"
branch: fix/decode-isolation
source: docs/prs/timeline-audio.md (review T-161)
---

## Contexte
T-161 décode les MP3 avec `nanomp3` (traduction automatique de C, avec du
code `unsafe`). Depuis T-253, **toute panique arrête tout le studio**
(laser désarmé, sortie noire). Un MP3 abîmé importé pendant un show
couperait donc le show, même si c'est sans danger.

## À faire
- Décoder dans un contexte isolé : au minimum `catch_unwind` autour du
  décodage **avant** le hook de panique global (le hook ne doit pas
  s'appliquer à ce fil), idéalement un sous-processus dédié au décodage
  (le studio relance `laser-studio --decode <fichier>` et lit le résultat).
- Un échec de décodage = message clair en français, le studio continue.
- Fuzzing plus poussé du décodeur (fichiers tronqués / mutés / aléatoires).

## Critères d'acceptation
- [ ] Un décodeur qui panique n'arrête pas le studio ni le laser en cours
- [ ] Le hook de panique global reste actif pour tout le reste (moteur, sortie)
- [ ] Message d'erreur clair dans l'interface

## Tests
Test qui force une panique dans le décodeur et vérifie que le studio répond encore.

## Notes
Règles de sécurité de CLAUDE.md inchangées : une panique ailleurs doit
toujours désarmer et éteindre.

## Journal
- 2026-09-28 — architecte : créée depuis la review de feat/timeline-audio.
