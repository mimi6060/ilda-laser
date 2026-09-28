---
id: T-298
title: Isoler le décodage audio (MP3) pour qu'un fichier piégé ne puisse pas arrêter le studio
status: review
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
- [x] Un décodeur qui panique n'arrête pas le studio ni le laser en cours
- [x] Le hook de panique global reste actif pour tout le reste (moteur, sortie)
- [x] Message d'erreur clair dans l'interface

## Tests
Test qui force une panique dans le décodeur et vérifie que le studio répond encore.

## Notes
Règles de sécurité de CLAUDE.md inchangées : une panique ailleurs doit
toujours désarmer et éteindre.

## Journal
- 2026-09-28 — architecte : créée depuis la review de feat/timeline-audio.
- 2026-09-28 — dev-agent (decode-isolation) : pris, branche `fix/decode-isolation`.
- 2026-09-28 — dev-agent (decode-isolation) : fait. Le décodage tourne dans
  un sous-processus (`laser-studio --decode <nom> --data-dir <dossier>`,
  lecture confinée à `media/audio/`, réponse binaire validée, limite 120 s),
  avec un fil contenu (exempté du hook global) en secours seulement. Une
  panique, un abort, un segfault ou un blocage du décodeur donnent
  « Import refusé : … le studio continue » ; laser armé et sortie intacts.
  Les requêtes qui décodent quittent le worker HTTP (sinon un décodage long
  bloquait les battements → « Interface perdue »). Fuzz renforcé (≈ 2 200
  fichiers). Tests : 566 unitaires + 2 shutdown + 2 nouveaux tests
  sous-processus, clippy propre, e2e 146/146 (nouveau
  `decode-isolation.spec.ts`). Note : `docs/prs/decode-isolation.md`.
  Statut → review.
