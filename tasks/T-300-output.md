---
id: T-300
title: Sortie PONK vers MadMapper/MadLaser (pont légal vers le ShowNET)
status: todo
area: output
priority: P1
depends_on: []
owner: ""
branch: ""
source: docs/research/shownet-options.md#13-ponk-le-pont-légal
---

## Contexte
Le protocole de streaming du ShowNET est chiffré et le SDK Laserworld n'arrive pas. MadMapper/MadLaser (que l'utilisateur fait déjà marcher sur son ShowNET depuis le Mac) reçoit depuis la 5.2 des tracés laser d'autres logiciels via **PONK**, un protocole UDP ouvert (Apache-2.0) publié par MadMapper. Laser Studio fait le rendu, MadMapper sert de pont officiel vers le ShowNET.

## À faire
- Nouveau `PonkOutput` dans `studio/src/output.rs` (ou `studio/src/ponk.rs`) qui implémente le trait `Output`.
- Encodage avec la crate `ponk-protocol` (MIT, zéro dépendance, MSRV 1.88) **ou** un encodeur maison d'environ 150 lignes si la crate pose problème. Format `XY_F32_RGB_U8`, découpage ≤ 8192 octets, CRC, numéro de frame, id d'émetteur stable (sauvegardé dans `studio-data/`) et nom « Laser Studio ».
- Découper la frame en chemins aux points blankés (un chemin PONK par segment allumé) ; les déplacements blankés ne sont pas envoyés (MadMapper les refait).
- Métadonnées par chemin pour que MadMapper respecte notre rendu : `PRESRVOR=1` (garder l'ordre), `PATHNUMB`, et des réglages par défaut documentés pour `ANGLEOPT`/`MINIPNTS`.
- **Sécurité** : désarmé, e-stop, Échap ou `blank_now` → envoyer **à chaque tick** une frame vide (en-tête seul, zéro chemin). Ne jamais arrêter d'émettre : MadMapper garde sinon la dernière frame reçue (bug connu, ofxPonk #2). La luminosité, les zones et la calibration sont appliquées **avant** l'envoi, comme pour `DacOutput`.
- CLI : `--ponk` (multicast 239.255.10.24:5583 par défaut) et `--ponk-target <ip:port>` (unicast). Exclusif avec `--device` et `--test-output`. Laser désarmé au démarrage, comme toujours.
- Vérifier (manuellement, par l'utilisateur, avec la sortie laser **désactivée** dans MadMapper) l'échelle des coordonnées attendue par MadMapper et documenter les réglages côté MadMapper (média PONK, surface laser) dans la note de PR.

## Modèle de données
```rust
pub struct PonkOutput { socket: UdpSocket, target: SocketAddr, sender_id: u32, frame_no: u8, armed: bool }
```
Pas de changement de `Settings`.

## Interface
Le bandeau de sortie affiche « Sortie : PONK → MadMapper (<cible>) ». Aucun nouveau contrôle en V1.

## Critères d'acceptation
- [ ] Une frame encodée puis décodée (crate ou décodeur de test) redonne les mêmes chemins, à la précision f32/u8 près
- [ ] Désarmé : chaque tick émet une frame vide (0 chemin), jamais rien
- [ ] `blank_now` émet immédiatement une frame vide
- [ ] Frame > 8192 octets : découpée en morceaux, réassemblée identique
- [ ] `--ponk` avec `--device` ou `--test-output` : refus au parsing
- [ ] Le studio démarre désarmé avec `--ponk`

## Tests
Unitaires : encodage/décodage, découpage aux points blankés, frames vides quand désarmé, CLI. Les tests réseau envoient en **unicast vers `127.0.0.1` sur un port libre** et lisent avec un socket local. **Jamais** le port 5583 ni le multicast, jamais vers un MadMapper réel. Pas d'e2e (aucun changement d'UI hors libellé).

## Notes
- PONK sert seulement de pont vers un logiciel sous licence ; on ne touche ni au ShowNET ni à MadMapper (aucune rétro-ingénierie). Si la crate `ponk-protocol` est utilisée, noter sa licence MIT dans la note de PR.
- MadMapper reste la dernière barrière (sa sortie laser doit aussi être activée) : ça ne remplace pas notre armement.
- Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout instantané, tests en aperçu uniquement.

## Journal
- 2026-10-03 — agent de recherche : tâche créée depuis `docs/research/shownet-options.md`.
