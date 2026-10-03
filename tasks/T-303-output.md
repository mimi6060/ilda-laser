---
id: T-303
title: DAC Helios USB et choix de la sortie (Helios, HeliosPRO/IDN, Ether Dream)
status: todo
area: output
priority: P2
depends_on: []
owner: ""
branch: ""
source: docs/research/shownet-options.md#5-repli-matériel--dac-ouverts-compatibles-macos--laser-dac
---

## Contexte
Repli sans Laserworld : un DAC ouvert branché sur le câble ILDA DB25 du laser, à la place du ShowNET. HeliosPRO (IDN) et Ether Dream 4 marchent déjà via `laser-dac` ; le Helios USB (≈ 99 €) demande la feature `helios`. Il manque aussi un moyen simple de voir quels DAC sont présents.

## À faire
- Activer la feature `helios` de `laser-dac` dans `studio/Cargo.toml`. Vérifier que ça compile sur macOS sans installation manuelle (`rusb`/libusb : vendored ou Homebrew) ; sinon documenter la commande dans le README.
- `GET /api/devices` : liste des DAC trouvés (`list_devices`), en lecture seule : type, id, nom. **Le scan n'ouvre ni n'arme aucun DAC.**
- Option `--list-devices` qui imprime la même liste et quitte.
- Documenter dans le README : brancher un Helios/HeliosPRO/Ether Dream à la place du ShowNET, `--device <id>`, réglage du pps (Helios USB : 4095 points/frame max).

## Modèle de données
Pas de changement de `Settings`. `DeviceInfo { kind: String, id: String, name: String }` pour l'API.

## Interface
*Réglages › Sortie* : liste « DAC détectés » (lecture seule) avec un bouton *Rafraîchir* et le rappel « Relancez le studio avec --device <id> pour sortir sur ce DAC ». Pas de bascule à chaud en V1.

## Critères d'acceptation
- [ ] `cargo build -p laser-studio` passe avec la feature `helios` sur macOS
- [ ] `/api/devices` répond une liste (vide en CI) sans ouvrir de DAC
- [ ] `--list-devices` imprime et quitte avec le code 0
- [ ] Clippy propre

## Tests
Unitaires : sérialisation de `DeviceInfo`, parsing de `--list-devices`. e2e : l'onglet affiche la liste (vide) sans erreur. Aucun test n'ouvre un DAC ni n'utilise `--device`.

## Notes
- Règles de CLAUDE.md : laser désarmé au démarrage, Échap = blackout, tests en aperçu uniquement.
- Le HeliosPRO a son propre lecteur `.ild` hors ligne : piste future, pas dans cette tâche.

## Journal
- 2026-10-03 — agent de recherche : tâche créée depuis `docs/research/shownet-options.md`.
