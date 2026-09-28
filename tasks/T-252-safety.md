---
id: T-252
title: Présence opérateur — battement de l'interface et mode maintien
status: in-progress
area: safety
priority: P0
depends_on: [T-250]
owner: "dev-agent (heartbeat)"
branch: feat/heartbeat
source: docs/research/safety-regulation.md#6-gaps-in-laser-studio-today
---

## Contexte
Le moteur tourne côté serveur, l'opérateur est dans le navigateur. Si l'onglet est fermé, gelé, ou si le Mac se met en veille d'écran, le laser reste armé et plus personne ne peut appuyer sur Échap. Il faut détecter la perte de l'opérateur et, pour les modes à risque, exiger une action maintenue (« homme mort »).

## À faire
1. **Battement** : chaque page de l'interface envoie `POST /api/heartbeat {client_id}` toutes les 500 ms (aussi quand l'onglet est en arrière-plan : utiliser un `Worker` ou accepter l'étranglement des minuteries et régler le délai en conséquence).
2. Verrou `ui_alive` : satisfait si au moins un client a battu depuis moins de `ui_timeout_ms` (défaut 2000). Sinon → désarmement, raison `UiLost`.
3. Pas de battement nécessaire en aperçu (désarmé).
4. **Mode maintien** (option, défaut désactivé ; obligatoire en mode public T-255) : le laser n'émet que tant qu'une commande est maintenue : touche configurable (défaut `ShiftRight` ; une pédale USB qui envoie une touche fonctionne aussi), ou un pad MIDI désigné (T-208). Relâcher = trame noire immédiate **sans** désarmer (on reprend en réappuyant), mais au-delà de 10 s relâché → désarmement.
5. Le battement porte aussi `document.visibilityState` : si tous les clients sont `hidden` pendant plus de 5 s alors que le mode maintien est actif → sortie noire.

## Modèle de données
```rust
#[serde(default)]
pub struct PresenceSettings { pub ui_timeout_ms: u32 /*2000*/, pub hold_to_run: bool /*false*/, pub hold_release_disarm_s: f32 /*10.0*/ }
```
État : `HashMap<ClientId, Instant>`, `hold_active: bool`.

## Interface
- Section Sécurité : « Couper si l'interface ne répond plus (ms) », « Mode maintien (homme mort) », touche choisie.
- Indicateur « Opérateur présent » (vert/rouge) près du bouton Armer ; en mode maintien, gros indicateur « MAINTENIR POUR ÉMETTRE ».

## Critères d'acceptation
- [ ] Armé puis plus aucun battement pendant 2 s → désarmé, raison « Interface perdue »
- [ ] Deux onglets ouverts, un fermé → reste armé
- [ ] Mode maintien : relâché → aucun point allumé en sortie ; réappuyé → sortie reprend ; relâché 10 s → désarmé
- [ ] `ui_timeout_ms` ne peut pas dépasser 10 000

## Tests
- Unitaires : horloge simulée pour le délai, plusieurs clients, maintien.
- e2e : armer (aperçu), fermer la page, vérifier via une requête directe que `armed=false` après 2,5 s.

## Notes
- Les navigateurs ralentissent les minuteries des onglets en arrière-plan (≥ 1 s) : le délai de 2 s doit rester fiable ; tester sur Safari et Chromium.
- CLAUDE.md : tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
