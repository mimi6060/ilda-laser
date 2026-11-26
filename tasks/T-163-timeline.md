---
id: T-163
title: Enveloppes de paramètres sur n'importe quel contrôle
status: todo
area: timeline
priority: P2
depends_on: [T-160, T-145]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §3.1 (envelopes, key effects)
---

## Contexte
Dessiner une montée de taille, une accélération de rotation, un fondu au noir sur la timeline, comme l'automation d'un logiciel audio.

## À faire
- Enveloppe sur un événement (paramètres du cue) ou sur une piste *Bus* (id de contrôle maître/calque).
- Nœuds (temps, valeur, courbe) ; double-clic ajoute, clic droit supprime ; courbes de T-157.
- Mode *Absolu* (remplace la valeur) ou *Relatif* (multiplie/ajoute).
- Évaluation dans le lecteur T-160 avant les modificateurs en direct.

## Modèle de données
```rust
pub struct Envelope { pub target: String, pub mode: EnvMode /*Absolute|Relative*/, pub keys: Vec<Key /*positions relatives à l'événement, dans la base de temps du show*/> }
```

## Interface
Sous chaque piste : *+ Enveloppe* → choix du paramètre ; ligne éditable ; valeur au survol.

## Critères d'acceptation
- [ ] Enveloppe luminosité 1 → 0 sur 4 temps : 0,5 à 2 temps
- [ ] Une enveloppe de bus agit sur toutes les pistes du calque
- [ ] Enveloppe sur un id inconnu : ignorée avec avertissement, pas de panique

## Tests
Unitaires : évaluation, modes absolu/relatif, bus.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
