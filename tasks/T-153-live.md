---
id: T-153
title: Routage des bandes audio vers n'importe quel contrôle
status: todo
area: live
priority: P2
depends_on: [T-145, T-151]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §4
---

## Contexte
Aujourd'hui seuls la taille, la rotation et le flash suivent les basses. Un pro veut choisir la bande (basses, médiums, aigus) et la cible.

## À faire
- Le navigateur envoie `low` (20–150 Hz), `mid` (150–2000 Hz), `high` (2–12 kHz), `level` et `beat` (champ ajouté à `AudioFeatures`, compatible).
- `AudioRoute` : source (bande, niveau, beat), cible (id de contrôle continu), quantité −1..1, attaque/relâche en ms, seuil.
- Mixage « temps / audio » maître (0 = tout suit le temps, 1 = tout suit l'audio), inspiré du mixeur de BEYOND.
- L'ancien `AudioReact` reste chargé et est converti en routes équivalentes (compatibilité).

## Modèle de données
```rust
pub enum AudioSource { Low, Mid, High, Level, Beat }
#[serde(default)]
pub struct AudioRoute { pub source: AudioSource, pub target: String, pub amount: f32 /*0.5*/,
    pub attack_ms: f32 /*10*/, pub release_ms: f32 /*150*/, pub gate: f32 /*0.05*/, pub enabled: bool }
```

## Interface
Section « Musique » : 3 vumètres *Basses / Médiums / Aigus* ; liste *Liens audio* : *Source*, *Cible*, *Quantité*, *Attaque*, *Relâche*, *Seuil*. Curseur *Temps ↔ Audio*.

## Critères d'acceptation
- [ ] Une route `Low → master.size` à 1,0 fait varier la taille avec les basses et rien d'autre
- [ ] Attaque/relâche : une impulsion d'un frame produit une montée puis une décroissance conforme (±1 frame)
- [ ] Une scène sauvegardée avec l'ancien `AudioReact` donne le même rendu qu'avant (test de non-régression)

## Tests
Unitaires : enveloppe attaque/relâche, conversion `AudioReact` → routes. e2e : POST `/api/audio` avec `low` élevé → taille plus grande dans `/api/frame`.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
