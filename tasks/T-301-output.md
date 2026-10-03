---
id: T-301
title: Sortie Art-Net « ShowNET carte SD » (profil DJ 19 canaux)
status: todo
area: output
priority: P3
depends_on: [T-302]
owner: ""
branch: ""
source: docs/research/shownet-options.md#2-protocoles-ouverts-du-shownet-lui-même
---

## Contexte
Sans SDK, le seul protocole **documenté publiquement** pour piloter le ShowNET est le déclenchement DMX/Art-Net des figures de sa carte SD (manuel ShowNET 2019). Le studio peut ainsi servir de pupitre : chaque cue de la banque SD (T-302) correspond à un numéro de figure. Pré-requis côté utilisateur, hors code : firmware à jour, DHCP, DIP 10 ON + adresse, Admin Tool « Data source for internal DMX effects = ArtNet input ».

## À faire
- Encodeur **ArtDmx** maison (en-tête `Art-Net\0`, OpCode 0x5000, version 14, séquence, univers, 512 canaux) dans `studio/src/artnet.rs`, sans nouvelle dépendance.
- `ShowNetDmxOutput` qui implémente `Output` mais ignore les points : il envoie l'état DMX à ~30 Hz en unicast vers l'IP configurée (univers et adresse de départ réglables).
- Mapping profil DJ (manuel 2019) : can. 1 intensité = master × luminosité (0 si désarmé), can. 2 figure = numéro de banque du cue actif (0 si aucun), can. 3 vitesse = 0 (50 fps), can. 14 mode = 0 (DMX), canaux de taille/position/rotation au neutre ; can. 16–17 zone de sécurité depuis un réglage.
- **Sécurité** : désarmé / Échap / e-stop / `blank_now` → can. 1 = 0 **et** can. 2 = 0, envoyés immédiatement puis à chaque tick. On ne compte jamais sur « arrêter d'envoyer » (comportement du firmware sur perte de signal inconnu).
- CLI : `--shownet-artnet <ip>` (+ `--artnet-universe`, `--dmx-address`), exclusif avec `--device`, `--ponk`, `--test-output`.

## Modèle de données
```rust
#[serde(default)]
pub struct ShowNetDmxConfig { pub universe: u16, pub address: u16 /* 1 */, pub safety_zone: u8, pub zone_intensity: u8 }
```

## Interface
Bandeau : « Sortie : ShowNET carte SD (Art-Net <ip>) ». Dans la grille de cues, un badge *SD nnn* sur les cues de la banque ; un cue hors banque est affiché grisé (rien ne sort).

## Critères d'acceptation
- [ ] Un paquet ArtDmx encodé a le bon en-tête, OpCode, longueur et valeurs (test octet par octet)
- [ ] Désarmé : can. 1 et 2 à 0 dans chaque paquet émis
- [ ] Armé sur un cue de la banque n° 7 : can. 2 = 7, can. 1 > 0
- [ ] Cue hors banque : can. 2 = 0
- [ ] `--shownet-artnet` avec `--device` : refus au parsing ; démarrage désarmé

## Tests
Unitaires sur l'encodage et le mapping. Test réseau uniquement vers `127.0.0.1` sur un port libre. **Jamais** vers 192.168.129.51, jamais sur le port 6454 d'une machine réelle.

## Notes
- Le firmware 2016050502 de l'utilisateur précède le manuel 2019 : l'Art-Net n'y est pas garanti. Ne pas démarrer cette tâche avant que l'utilisateur ait confirmé (Admin Tool) que le mode Art-Net marche.
- Mode exclusif avec le streaming (MadMapper) : changer les DIP demande un redémarrage du boîtier.
- Le tableau de canaux est réécrit depuis le manuel public (fait technique) ; aucun fichier Laserworld copié.

## Journal
- 2026-10-03 — agent de recherche : tâche créée depuis `docs/research/shownet-options.md`.
