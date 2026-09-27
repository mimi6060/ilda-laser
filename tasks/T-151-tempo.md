---
id: T-151
title: Modulateurs LFO synchronisés au tempo sur n'importe quel contrôle
status: todo
area: tempo
priority: P1
depends_on: [T-150, T-145]
owner: ""
branch: ""
source: docs/research/pro-live-operation.md §1.3, §6 (B)
---

## Contexte
Les effets pro sont des paramètres qui oscillent (taille qui respire, rotation qui balance, couleur qui tourne) calés sur la musique. Remplace l'item W2-animator de la feuille de route.

## À faire
- Un `Modulator` pilote un contrôle continu (par id T-145 ou paramètre de cue) : forme d'onde, période en Hz **ou** en temps, profondeur, phase, décalage.
- **Phase calculée depuis l'horloge de tempo** (`beat_at(t) / period_beats`), jamais intégrée frame par frame : deux modulateurs de même période restent en phase à vie.
- Formes : sinus, triangle, carré, dent de scie montante/descendante, aléatoire échantillonné (nouvelle valeur à chaque période, graine stable).
- Valeur finale = base du contrôle + profondeur × onde × (max - min) / 2, bornée.
- Jusqu'à 16 modulateurs actifs au niveau maître, enregistrés dans l'état (et plus tard dans les cues, T-157).

## Modèle de données
```rust
pub enum Rate { Hz(f32), Beats(f32) }          // Beats(n) = un cycle toutes les n temps
pub enum Wave { Sine, Triangle, Square, SawUp, SawDown, Random }
#[serde(default)]
pub struct Modulator { pub target: String, pub wave: Wave, pub rate: Rate /*Beats(4.0)*/,
    pub depth: f32 /*0..1, 0.5*/, pub phase: f32 /*0..1, 0*/, pub offset: f32 /*-1..1, 0*/, pub enabled: bool }
```

## Interface
Section « Modulateurs » : liste ; pour chacun *Cible* (menu des contrôles continus), *Forme*, *Période* (*1/4, 1/2, 1, 2, 4, 8, 16 temps* ou *Hz*), *Profondeur*, *Phase*, *Actif*. Mini-graphe animé de l'onde.

## Critères d'acceptation
- [ ] Un modulateur sinus de 4 temps sur `master.size` revient à la même valeur tous les 4 temps (±1e-4)
- [ ] Deux modulateurs identiques lancés à 10 s d'écart sont en phase
- [ ] Changer le BPM change la vitesse sans saut de valeur
- [ ] Profondeur 0 = contrôle inchangé
- [ ] Les scènes existantes chargent toujours

## Tests
Unitaires : chaque forme d'onde à des phases connues, bornage, synchro tempo. e2e : ajout d'un modulateur → `/api/frame` varie dans le temps.

## Notes
Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, sécurité appliquée en dernier (après calibration), tests en aperçu uniquement (jamais `--device`). Propriété intellectuelle : concepts inspirés de la doc publique Pangolin/Laserworld, rien de copié (ni noms d'effets, ni contenus, ni icônes).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/pro-live-operation.md`.
