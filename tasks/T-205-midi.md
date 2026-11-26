---
id: T-205
title: Retour LED sur l'APC40 (cue active, page, calques, battement)
status: done
area: midi
priority: P1
depends_on: [T-201, T-204, T-150]
owner: "dev-agent (apc-leds)"
branch: feat/apc-leds
source: docs/research/midi-apc40.md#13-outbound-host--device-leds-and-rings
---

## Contexte
Sans LED, on joue à l'aveugle. L'APC40 doit montrer quelles cases ont une
cue, laquelle joue, quelle page est ouverte, quels calques/bascules sont
actifs, et battre le tempo — même onglet fermé.

## À faire
- Module `midi_led.rs` : à partir de l'état `Shared` (pas de l'UI), calculer
  l'**état LED désiré** de chaque appareil (tableau `note/canal → valeur`),
  le comparer au dernier état envoyé et n'envoyer que les **différences**,
  au plus 30 fois par seconde. Au (re)branchement : tout renvoyer.
- Pilote `Apc40` (APC40 d'origine, LED vert/rouge/jaune) :
  - pad vide = éteint (0), cue présente = **jaune** (5), cue en cours = **vert** (1), cue en cours en mode flash/beat = **vert clignotant** (2), cue sélectionnée dans l'UI mais pas jouée (si T-155) = rouge (3) ;
  - Scene Launch : page courante = allumée (1), page 6–10 = clignotante (2) sur le bouton 1–5 correspondant ;
  - Clip Stop 1–8 : allumé si le calque joue ; Activator/Solo/Record Arm/Track Select : état booléen correspondant ; Pan/Send A/B : banque de potards ;
  - Metronome (Device 8, `0x41`) : **allumé sur chaque temps** pendant 1/8 de temps (horloge T-150) ;
  - anneaux des potards : envoyer la valeur actuelle (CC `0x30`–`0x37`, `0x10`–`0x17`) au changement de page, de banque, de calque ou quand la valeur change par l'UI ; type d'anneau (CC `0x38`–`0x3F`, `0x18`–`0x1F`) : « pan » pour les valeurs centrées (position, rotation), « volume » sinon.
- Pilote `Apc40Mk2` : mêmes règles pour les LED non-RGB ; les couleurs des pads et Scene Launch viennent de T-206 (ici : blanc 3 = présente, vert 21 = en cours, en attendant T-206). Metronome `0x5A` pour le battement.
- À l'arrêt / déconnexion volontaire : tout éteindre (Note Off).
- Pas de LED pour Tap, Shift, flèches, Nudge, Stop All (le matériel n'en a pas).

## Modèle de données
```rust
pub struct LedFrame(pub BTreeMap<(u8 /*status*/, u8 /*note|cc*/), u8 /*value*/>);
pub trait LedDriver { fn render(&self, s: &Shared, dev: &DeviceState) -> LedFrame; }
```
`DeviceState` garde `last_sent: LedFrame`.

## Interface
Case « Retour LED » par appareil (onglet Contrôleur), cochée par défaut pour
les APC.

## Critères d'acceptation
- [x] Jouer une cue depuis l'UI ou le clavier allume son pad en vert sur l'APC ; l'ancienne repasse en jaune.
- [x] Changer de page (UI ou Scene Launch) redessine la grille en < 50 ms.
- [x] Onglet fermé : Tap puis pads → les LED suivent toujours.
- [x] La LED de battement clignote au tempo courant.
- [x] Au repos, aucun message LED n'est envoyé (diff vide).

## Tests
- Unitaires : rendu `Apc40` d'un état donné (pad vide/présent/actif/flash, page 7 → Scene Launch 2 clignotant), diff (seules les LED changées sont envoyées), rendu complet au rebranchement, débit ≤ 30 envois/s.
- Avec port virtuel (T-209) : lire les octets reçus par un faux APC et vérifier la LED de la cue active.

## Notes
- Valeurs de vélocité et de canal : protocoles Akai (rapport §1.3, §2.3). Sur l'APC40 d'origine le clignotement a sa propre cadence (non synchronisée).
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal

- 2026-09-28 — dev-agent (apc-leds), branche `feat/apc-leds`, note `docs/prs/apc-leds.md`.
  `studio/src/midi/led.rs` : `LedFrame` / `LedState` (dernier envoi, diff, ≤ 30 mises à jour/s, tout renvoyer après Introduction ou rebranchement) et `render(driver, &Shared, port, t)` lu **depuis le profil** (grille, `page.N`, calques, bascules, anneaux) + Metronome au temps (horloge T-150) + rangée Clip Stop clignotante tant que l'arrêt d'urgence est verrouillé.
  Le thread MIDI rend sous un seul verrou, envoie hors verrou ; `goodbye` éteint aussi les anneaux. Case « Retour LED » par appareil (`devices.json`, `POST /api/midi/device {leds}`, 2 lignes dans index.html).
  Écarts : fonction `render` par pilote plutôt qu'un trait ; mkII : clignotement « flash » et arrêt d'urgence faits à la main (pas d'horloge MIDI envoyée), Shift + Scene Launch (pages 6–8) en orange ; pas de rouge « sélectionnée dans l'UI » (la sélection n'existe que dans l'UI), pas de banques Pan/Send ni Track Select (pas d'ids de contrôle). Au repos, seul le Metronome envoie (2 messages par temps).
  Tests : cargo test 465 OK (2 ignorés CoreMIDI), clippy -D warnings propre, e2e 118/118 (`--workers=2` ; à 4 workers, machine chargée, le premier test de 1–2 fichiers expire au chargement de l'UI, aussi avec l'index.html de develop), specs MIDI `--repeat-each 3` 54/54. L'assertion e2e `ledAt(0,1) === null` de T-209 est devenue un vrai contrôle de couleur (vert 21 / blanc 3).
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop. À vérifier sur l'APC40 mkII de l'utilisateur.
