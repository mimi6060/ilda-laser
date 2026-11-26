---
id: T-292
title: La cue active reste « en cours » côté serveur après un changement de look à la main
status: done
area: cues
priority: P2
depends_on: []
owner: dev-agent (fix/ui-state)
branch: fix/ui-state
source: docs/prs/e2e.md#bugs-found
---

## Contexte
Trouvé par les tests e2e (T-004). Après avoir joué une cue puis changé le
look à la main (forme, curseur…), une scène ou la playlist, le serveur garde
`active_cue`. La page enlève la surbrillance localement, mais après un
rechargement la cue réapparaît comme « en cours », et le retour LED MIDI
(T-145/T-200) montrera une cue qui ne joue plus.

## À faire
Remettre `Shared::active_cue` à `None` quand le look est remplacé par autre
chose qu'une cue : `POST /api/settings`, `/api/scenes/play`,
`/api/playlist/start` et l'avance de playlist (`advance_playlist`), et les
contrôles `look.*` qui changent le contenu si c'est le cas. Garder la cue
active quand seuls des modificateurs **direct** (`master.*`) ou le tempo
changent : la cue joue toujours.

## Modèle de données
—

## Interface
Aucun changement : la surbrillance de la grille suit l'état du serveur.

## Critères d'acceptation
- [x] Cue → changement de forme à la main : `/api/control-values` renvoie `active_cue: null`, et après rechargement aucune cue n'est en surbrillance.
- [x] Cue → jouer une scène / lancer la playlist : `active_cue: null`.
- [x] Cue → « Taille maître » : la cue reste active.
- [x] Le test e2e `T-292: changing the look by hand clears the active cue` passe de `test.fixme` à `test` et est vert.

## Tests
Unitaire (web/controls) + e2e `studio/e2e/tests/cues.spec.ts` (déjà écrit,
en `test.fixme`).

## Notes
Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — QA e2e (T-004) : reproduit à chaque exécution.
- 2026-09-27 — dev (fix/ui-state) : `controls::set_look` pour `POST /api/settings` : un autre dessin (type de contenu, forme, générateur ; `Content::same_drawing`) arrête les cues et montre le look seul comme une scène (`active_cue: null`, LED éteintes) ; les autres retouches (taille, couleur, texte, paramètres du générateur) modifient toujours la cue du dessus (T-155). Scènes/playlist passaient déjà par `show_look`. `master.*` et tempo ne touchent pas la cue. Tests : 4 unitaires (controls.rs), e2e T-292 activé + 2 nouveaux (taille/Taille maître gardent la cue, une scène l'arrête). cargo test 168 ok, clippy ok, e2e 56/56 (×5 : 280/280). PR : docs/prs/ui-state.md.
- 2026-09-27 — architecte (review) : APPROUVÉ et fusionné dans develop.
