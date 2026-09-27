---
id: T-105
title: Tunnels, cônes, soleil et rayons tournants
status: todo
area: cues
priority: P1
depends_on: [T-100]
owner: ""
branch: ""
source: docs/research/festival-looks.md#b-tunnels-and-cones
---

## Contexte
Le tunnel qui rétrécit pendant la montée puis s'ouvre sur le drop est un des moments les plus reconnaissables (trance, big-room). Il manque le vrai cône de faisceaux, le soleil et le tunnel qui pompe.

## À faire
- `finger_tunnel` (look 14) : N faisceaux sur un **vrai cercle** (pas écrasé comme `beam_circle`), rayon r = `scale`, rotation en tours/temps (`a`, défaut 1/16), `direction`.
- `tunnel_pump` (look 16) : cercle continu ; rayon `r = r_base·(1 + depth·env_stab(phase_temps))` (depth `a` défaut 0.3, décroissance 1/2 temps) ; ou mode rampe : r de 0.35 à 0.04 sur `period_beats` (`b` = 1).
- `twin_tunnel` (look 17) : 2 cercles r1 = 0.2, r2 = 0.35 tournant en sens opposés (1 tour / 2 mesures), avec un repère visible (trou de 10 % ou dégradé) pour voir la rotation.
- `sunburst` (look 15) : N rayons (12–24) sur le demi-plan supérieur, rayon 0.85, rotation lente ; les rayons qui sortent du demi-plan sont éteints. Option pair/impair : alternance 100 %/40 % à chaque temps.
- `polygon_tunnel` existant : ajouter la rotation par à-coups de 360°/côtés par temps (`snap: true`).
Vitesses de rotation (rapport 4.1) : glaciale 1/64, lente 1/32, moyenne 1/16, rapide 1/4, très rapide 1/2–1 tour/temps.

## Modèle de données
Ajout `GenParams.snap: bool` (défaut false). `a` = tours par temps pour les générateurs tournants.

## Interface
Libellés : « Tunnel de faisceaux », « Tunnel qui pompe », « Double tunnel », « Soleil ». Liste déroulante « Vitesse » (Glaciale, Lente, Moyenne, Rapide, Très rapide) qui remplit `a`.

## Critères d'acceptation
- [ ] `finger_tunnel` : tous les faisceaux à distance r du centre (± 1 %)
- [ ] Rotation 1/4 tour/temps : après 4 temps, retour à l'angle initial
- [ ] `tunnel_pump` : rayon max juste après le temps, retour à r_base en 1/2 temps
- [ ] `sunburst` : aucun rayon allumé sous l'horizon
- [ ] `polygon_tunnel` avec snap : angle constant entre deux temps

## Tests
Unitaires géométriques (rayon, angle aux temps entiers) ; non-régression de `polygon_tunnel` sans snap.

## Notes
Garder le cercle au-dessus de ~40 Hz de rafraîchissement pour que le cône paraisse plein.

Règles de CLAUDE.md (sécurité laser, propriété intellectuelle) : looks écrits par nous en maths, rien de copié depuis Pangolin/Laserworld. Tests uniquement en aperçu, jamais `--device`.

## Journal
