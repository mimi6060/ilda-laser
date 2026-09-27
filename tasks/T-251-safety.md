---
id: T-251
title: Arrêt d'urgence verrouillé (clavier, bouton, API, MIDI)
status: todo
area: safety
priority: P0
depends_on: [T-250]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#34-outdoor-dgta-authorisation-confirmed-primary-sources
---

## Contexte
Les règles (DGTA en Belgique, IEC 60825-3, ILDA) exigent un opérateur capable d'arrêter la projection **à tout moment**. Aujourd'hui Échap désarme, mais seulement si l'onglet du navigateur a le focus, et on peut réarmer aussitôt par erreur. Un arrêt d'urgence doit être **verrouillé** : après lui, il faut un geste délibéré pour revenir.

## À faire
- `POST /api/estop` (corps vide accepté, aucune validation JSON) : traité avant toute autre requête en attente, désarme via `ArmGate::disarm(EStop, ..)` et active le verrou `estop` (non satisfait).
- La trame suivante envoyée à la sortie est éteinte : délai visé ≤ 1 tick moteur (≤ 17 ms à 60 im/s) entre la réception et l'envoi d'une trame noire. Envoyer en plus une trame noire immédiate depuis le gestionnaire si la sortie le permet.
- Réarmer exige d'abord `POST /api/estop/reset` (bouton « Réinitialiser l'arrêt d'urgence ») puis un armement normal. Le reset ne réarme jamais.
- Sources : Échap (appelle `/api/estop` au lieu de `/api/arm {on:false}`), bouton rouge permanent, action MIDI `safety.blackout` de T-208 (mappée sur estop), et Maj+Échap = simple désarmement sans verrou (pour les répétitions).
- L'état estop n'est pas sauvegardé : au redémarrage on est de toute façon désarmé.

## Modèle de données
Verrou `estop` dans `ArmGate` ; `Shared.estop_at: Option<SystemTime>`.

## Interface
- Bouton rond rouge « ARRÊT » toujours visible en haut à droite, sur toutes les vues, jamais masqué par un panneau.
- Pendant l'arrêt : bandeau rouge plein « ARRÊT D'URGENCE — réinitialiser pour réarmer » + bouton « Réinitialiser ».
- Raccourcis : Échap = arrêt d'urgence ; Maj+Échap = désarmer sans verrou.

## Critères d'acceptation
- [ ] Échap pendant l'armement → `armed=false`, `estop=true`, bandeau visible
- [ ] Espace ou `/api/arm {on:true}` pendant l'arrêt → refusé (409, verrou `estop`)
- [ ] Reset puis Espace → armé
- [ ] `/api/estop` sans corps ou avec un corps invalide → 200 et arrêt effectif
- [ ] Mesure en test : trame éteinte produite au plus tard au tick suivant

## Tests
- Unitaires : verrou, reset qui ne réarme pas, ordre de traitement.
- e2e (aperçu) : armer, Échap, vérifier `/api/state`, essayer Espace, reset, Espace.

## Notes
- Un arrêt d'urgence logiciel **ne remplace pas** l'interrupteur à clé et l'arrêt d'urgence matériel (interlock distant) du projecteur de classe 4 : ajouter cet item à la liste de contrôle (T-258).
- T-271 (barre du haut) : son bouton « Noir » appelle `/api/arm {on:false}` ; une fois T-251 fait, il doit appeler `/api/estop` comme Échap. Le bouton rond « ARRÊT » décrit ici peut être ce même bouton « Noir » de la barre si T-271 est déjà fait (ne pas en afficher deux).
- T-208 : le blackout MIDI devient l'estop ; garder la priorité de traitement décrite dans T-208.
- CLAUDE.md : Échap = blackout instantané ; tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
