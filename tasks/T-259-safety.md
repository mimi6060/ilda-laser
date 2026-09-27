---
id: T-259
title: Journal des événements de sécurité
status: todo
area: safety
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/research/safety-regulation.md#7-proposed-features
---

## Contexte
En cas d'incident, de contrôle ou pour une demande d'autorisation, il faut pouvoir montrer qui a armé, quand, quel cue tournait, quand l'arrêt d'urgence a été utilisé et quelles protections sont intervenues. Un journal en ajout seul, simple à lire, suffit.

## À faire
- Module `studio/src/safety_log.rs` : fichier JSON Lines par jour, `<data-dir>/logs/safety-AAAA-MM-JJ.jsonl`, une ligne par événement, écrit en ajout seul (`OpenOptions::append`), `flush` après chaque ligne.
- Écriture hors du fil moteur (canal + fil d'écriture) : le moteur ne bloque jamais sur le disque.
- Événements : démarrage/arrêt de l'application ; armement (source) ; désarmement (raison, source) ; arrêt d'urgence et reset ; verrou qui tombe ; intervention d'un limiteur (strobe T-101, garde T-256, « Ciel coupé » T-262) — agrégée : au plus une ligne par limiteur et par seconde ; cue/scène lancé pendant l'armement ; changement d'un réglage de sécurité (ancienne et nouvelle valeur) ; liste de contrôle ; changement de profil ; déverrouillage du mode public.
- Rétention : 365 jours par défaut, suppression des fichiers plus anciens au démarrage.
- API : `GET /api/safety/log?date=AAAA-MM-JJ` (JSON), `GET /api/safety/log.csv?date=…` (export).

## Modèle de données
```rust
#[derive(Serialize)]
pub struct SafetyEvent { pub ts: String /*RFC 3339, heure locale avec décalage*/, pub kind: String, pub source: Option<String>, pub detail: serde_json::Value, pub session: String }
```

## Interface
Onglet Sécurité → « Journal » : tableau (heure, événement, détail), filtre par jour et par type, bouton « Exporter CSV ».

## Critères d'acceptation
- [ ] Armer, lancer un cue, Échap → 3 lignes dans l'ordre, avec sources
- [ ] Un strobe limité pendant 10 s → ≤ 10 lignes de limiteur
- [ ] Moteur à 60 im/s tenu pendant un disque lent (écriture simulée de 200 ms)
- [ ] Fichiers de plus de 365 jours supprimés au démarrage

## Tests
Unitaires : format, agrégation, rotation par date, rétention (répertoire temporaire). e2e : armer/désarmer en aperçu, lire `/api/safety/log`.

## Notes
- Pas de données personnelles au-delà du nom d'opérateur saisi volontairement.
- Les autres tâches de sécurité (T-250…T-262) appellent ce journal ; T-259 n'a pas de dépendance pour pouvoir être fait en premier.
- CLAUDE.md : tests jamais avec `--device`, `--data-dir` temporaire.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
