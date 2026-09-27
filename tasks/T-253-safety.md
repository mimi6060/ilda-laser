---
id: T-253
title: Chien de garde du moteur et extinction propre
status: todo
area: safety
priority: P1
depends_on: [T-250]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#6-gaps-in-laser-studio-today
---

## Contexte
Si le fil moteur se bloque (verrou tenu trop longtemps, calcul trop lourd) ou panique, ou si on quitte l'application avec Ctrl+C, le DAC peut garder ou répéter la dernière trame selon le modèle. Il faut garantir qu'une panne logicielle finisse en noir.

## À faire
- Chien de garde dans un fil séparé : le moteur publie un compteur de ticks ; si aucun tick depuis `stall_ms` (défaut 100) alors que la porte est armée → désarmement (`EngineStall`) et, si possible, trame noire envoyée directement par la sortie.
- `std::panic::set_hook` : sur panique, tenter `output.send(noir)` puis `output.disarm()` avant de laisser le processus finir.
- SIGINT/SIGTERM (crate `ctrlc` ou équivalent) : désarmer, envoyer 3 trames noires, fermer la sortie, puis quitter.
- Trait `Output` : ajouter `fn blank_now(&mut self) -> Result<()>` (implémentation par défaut = envoyer une trame d'un point éteint) ; toute nouvelle sortie (ShowNET plus tard) doit l'implémenter.
- Test explicite de la règle « démarrage désarmé » : aucune option de ligne de commande ni fichier de réglages ne permet de démarrer armé.

## Modèle de données
`EngineHealth { last_tick: AtomicU64, stall_ms: u32 /*100*/ }` ; indicateur `engine_ok` exposé dans `/api/state`.

## Interface
Voyant « Moteur » (vert/rouge) et message « Moteur bloqué — laser coupé » si le chien de garde a déclenché.

## Critères d'acceptation
- [ ] Blocage simulé de 200 ms du moteur (option de test) → désarmé, raison « Moteur bloqué »
- [ ] Panique simulée → `blank_now` appelé (sortie factice de test)
- [ ] SIGTERM → trames noires puis fermeture (sortie factice)
- [ ] Aucun chemin de démarrage ne produit `armed=true`

## Tests
Unitaires avec une sortie factice (`MockOutput`) qui enregistre les appels ; test d'intégration du signal sur un sous-processus en aperçu.

## Notes
- Le chien de garde ne doit jamais prendre le verrou `Shared` (il pourrait être celui qui bloque) : utiliser des atomiques et un canal vers la sortie.
- CLAUDE.md : toute nouvelle sortie respecte armement/blackout ; tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
