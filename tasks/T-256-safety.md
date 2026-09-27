---
id: T-256
title: Garde anti-point fixe (taille minimum, vitesse, temps de pose)
status: todo
area: safety
priority: P1
depends_on: [T-250, T-003]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#22-mpe-for-audience-exposure
---

## Contexte
L'exposition d'un œil dépend de la taille, de la vitesse et des pauses de la figure, pas seulement de la luminosité : une figure réduite à 2 % de taille, un effet figé ou un zoom à 0 devient un faisceau fixe, et nos pauses de coin (`densify`) concentrent l'énergie aux angles. Il faut détecter quand l'énergie se concentre et réduire puis couper.

## À faire
1. **Emprise de la trame** : pour les points allumés, calculer l'étendue (boîte englobante, diagonale en unités normalisées). Si la diagonale < `min_extent` (défaut 0,05) et que la trame ne contient pas que des faisceaux voulus (voir 3), atténuer linéairement jusqu'à 0 à `min_extent / 2`.
2. **Grille d'exposition glissante** : grille 32×32 sur le champ ; chaque point allumé ajoute son énergie relative (luminosité × durée du point = 1/pps) à sa case ; fenêtre glissante de 250 ms. Si une case dépasse `max_cell_dose` (défini en fraction de la dose totale possible, défaut 0,25 = un quart de l'énergie de la fenêtre dans une seule case), multiplier les points de cette case par un facteur de réduction ; au-delà de 2× le seuil, les éteindre.
3. **Profils** : `Strict` (appliqué dans les zones marquées Public de T-003 et partout en mode balayage public T-255), `Beams` (défaut hors public : seuls les plafonds de la grille s'appliquent, les faisceaux statiques au-dessus de l'horizon restent permis mais une case ne peut pas dépasser `max_cell_dose_beams`, défaut 0,6), `Off` (uniquement en aperçu / sortie désarmée : sans effet).
4. Placé après T-003 et T-254, avant la porte T-250 ; l'aperçu affiche les cases qui interviennent (surimpression orange).
5. Coût : < 0,5 ms pour 2000 points.

## Modèle de données
```rust
#[serde(default)]
pub struct DwellGuard { pub min_extent: f32 /*0.05*/, pub max_cell_dose: f32 /*0.25*/, pub max_cell_dose_beams: f32 /*0.6*/, pub window_ms: u32 /*250*/, pub grid: u16 /*32*/ }
pub enum DwellProfile { Strict, Beams, Off }
```

## Interface
Section Sécurité : « Garde anti-point fixe » (profil), « Taille minimum (%) », indicateur « Garde active » quand elle atténue. Surimpression des cases dans l'aperçu (option).

## Critères d'acceptation
- [ ] Cercle qui rétrécit jusqu'à 0 en profil Strict → luminosité de sortie 0 sous 2,5 % de diagonale
- [ ] Un point fixe allumé dans une zone Public est éteint en ≤ 250 ms
- [ ] Un éventail de faisceaux au-dessus de l'horizon en profil Beams reste allumé
- [ ] Un carré plein champ à 30 000 pps n'est jamais atténué
- [ ] < 0,5 ms pour 2000 points (bench)

## Tests
Unitaires : figures synthétiques (cercle de taille variable, point fixe, carré, éventail), fenêtre glissante avec horloge simulée. e2e : régler la taille à 0 sur un look, vérifier `/api/frame` de sortie éteint.

## Notes
- Ce garde **ne remplace pas** un anti-défaut de balayage matériel : une panne de galvo n'est pas visible dans nos points (pas de retour de position depuis ShowNET/IDN/Ether Dream).
- La dose en « énergie relative » ne donne pas des mW/cm² : pour une estimation absolue voir T-257.
- CLAUDE.md : sécurité après calibration ; tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
