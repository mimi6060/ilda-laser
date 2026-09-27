---
id: T-290
title: Bouton laser / Espace basculent depuis une copie locale périmée de « armed »
status: todo
area: safety
priority: P2
depends_on: []
owner: ""
branch: ""
source: docs/prs/e2e.md#bugs-found
---

## Contexte
Trouvé par les tests e2e (T-004). Deux appuis rapides sur Espace (« allume,
puis éteins ») peuvent laisser le laser **allumé** : la bascule envoie
deux fois `on: true`.

## À faire
Dans `index.html`, `setArmed(!armed)` lit la variable locale `armed`, qui
n'est mise à jour qu'**après** la réponse du POST `/api/arm`, et que la
boucle d'aperçu écrase avec la valeur de chaque `/api/frame` (y compris une
réponse partie avant le clic). Pendant cet aller-retour, un deuxième appui
recalcule la même valeur.
Corriger en l'une de ces façons (au choix du développeur) :
- mémoriser l'état **demandé** côté page (mis à jour tout de suite, avant
  le `fetch`) et ignorer les `/api/frame` plus anciens que la dernière
  demande ; ou
- ajouter une bascule côté serveur (`POST /api/arm { toggle: true }`) qui
  répond avec le nouvel état, la page ne faisant qu'afficher.
Échap reste un `on: false` explicite (jamais une bascule).

## Modèle de données
—

## Interface
Aucun changement visible : bouton LASER ON/OFF et Espace.

## Critères d'acceptation
- [ ] Deux appuis rapides sur Espace, même avec un `/api/arm` lent (150 ms), finissent désarmés.
- [ ] Un clic sur le bouton suivi tout de suite d'Espace finit désarmé.
- [ ] Échap force toujours `armed=false`.
- [ ] Le test e2e `T-290: two quick Space presses…` (`studio/e2e/tests/laser.spec.ts`) passe de `test.fixme` à `test` et est vert.

## Tests
e2e : `studio/e2e/tests/laser.spec.ts` (déjà écrit, en `test.fixme`).

## Notes
Sécurité laser : ne jamais armer sur un événement qui n'est pas une action
explicite de l'utilisateur ; un doute doit toujours finir en désarmé.

## Journal
- 2026-09-27 — QA e2e (T-004) : reproduit à chaque fois avec `/api/arm` ralenti à 150 ms ; sans ralenti, un clic puis Espace à ~3 ms d'écart envoie `on:true` deux fois.
