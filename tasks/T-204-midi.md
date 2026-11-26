---
id: T-204
title: Profil APC40 par défaut (disposition Laser Studio) pour APC40 et APC40 mkII
status: done
area: midi
priority: P1
depends_on: [T-201, T-202, T-140, T-145, T-150, T-155, T-160, T-208, T-209]
owner: "dev-agent (apc40-profile)"
branch: "feat/apc40-profile"
source: docs/research/midi-apc40.md#5-our-default-apc40-layout-proposal-same-for-both-models
---

## Contexte
L'utilisateur branche son APC40 et tout marche sans réglage : la grille joue
les cues, les faders et potards pilotent les modificateurs, Tap règle le
tempo, Stop All fait le blackout. Deux fichiers (APC40 d'origine et mkII) car
la grille et quelques notes diffèrent. C'est notre propre disposition.

## À faire
- Écrire `studio/profiles/apc40.json` et `studio/profiles/apc40-mk2.json`
  (format T-201/T-202) avec la disposition ci-dessous.
- **Page de cues côté serveur** : la grille MIDI a besoin de la page courante.
  Si T-155 ne l'a pas déjà fait, ajouter `Shared.cue_page` (index de page +
  tranche de 40) et `POST /api/cues/page`, et faire suivre les onglets de page
  de l'UI (dans les deux sens).
- Numérotation des 40 pads : slot 0 = **en haut à gauche**, ligne par ligne.
  - APC40 : pad (ligne *r* 0–4 depuis le haut, colonne *c* 0–7) = note
    `0x35 + r`, **canal c** ;
  - APC40 mkII : note `0x20 + c − 8·r` (rangée du bas = `0x00`–`0x07`), canal
    quelconque.
- Disposition (identifiants T-145 indicatifs, à aligner) :

| Matériel | Action | Shift + |
|---|---|---|
| Grille 5×8 | jouer la cue du slot *n* de la page (mode flash respecté, T-155) | cue en flash (tant que tenu) |
| Scene Launch 1–5 (`0x52`–`0x56`) | page 1–5 | page 6–10 |
| Up / Down (`0x5E`/`0x5F`) | page précédente / suivante | — |
| Left / Right (`0x61`/`0x60`) | tranche de 40 précédente / suivante si la page a > 40 cues | — |
| Stop All Clips (`0x51`) | **blackout** (comme Échap) | armer (maintien 1 s, seulement si autorisé, T-208) |
| Clip Stop 1–8 (`0x34` c0–7) | arrêter le calque 1–8 (T-155) ; sans calques : 1 = arrêter la cue | — |
| Track Select 1–8 / Master (`0x33` c0–7 / `0x50`) | calque édité par les potards Device (Master = global) | — |
| Activator 1–8 (`0x32`) | couper/rétablir le calque | — |
| Solo 1–8 (`0x31`) | solo du calque | — |
| Record Arm 1–8 (`0x30`) | bascules T-140 : strobe, cycle couleur, miroir X, miroir Y, réaction audio, mode beat, inversion, figer | — |
| Faders 1–8 (CC `0x07` c0–7) | taille, taille X, taille Y, vitesse d'animation, vitesse de rotation, stroboscope (0 = off), décalage de teinte (0 = couleur de la cue), sensibilité audio | — |
| Master (CC `0x0E`) | luminosité maître, `pickup`, plafonnée (T-208) | — |
| Crossfader (CC `0x0F`) | position X (centre = 0) | — |
| Potards du haut 1–8 (CC `0x30`–`0x37`) | position Y, angle, teinte, vitesse de cycle couleur, ondulation, pulsation de zoom, densité de points, fondu de transition | — |
| Pan / Sends / User (mk1 : Pan / Send A / B) (`0x57`–`0x59`) | banque des potards du haut : position / couleur / effets | — |
| Potards Device 1–8 (CC `0x10`–`0x17`) | paramètres du générateur de la cue du calque choisi (`count`, `a`, `b`, `speed`…) | — |
| Tap Tempo (`0x63`) | tap (T-150) | remise à zéro de la phase |
| Nudge − / + (mk1 : `0x65`/`0x64`, **mkII : `0x64`/`0x65`**) | BPM −/+ 0,1 | phase −/+ |
| Tempo (mkII, CC `0x0D` relatif) / Shift + Cue Level (mk1) | BPM ± 0,5 par cran | BPM ± 0,1 |
| Cue Level (CC `0x2F` relatif) | position Y fine | — |
| Metronome (mkII `0x5A`) / Device 8 (mk1 `0x41`) | « cues sur le temps » on/off (T-150) | — |
| Play (`0x5B`) | timeline lecture/pause (T-160) | mkII : stop |
| Stop (mk1 `0x5C`) | timeline stop | — |
| Record (`0x5D`) | enregistrer les actions live dans la timeline si T-160 le permet | — |
| Pédale (CC `0x40`) | tap tempo | — |

- Les contrôles non listés restent libres pour l'apprentissage (T-203).

## Modèle de données
Uniquement les deux JSON (+ `Shared.cue_page` si absent). Champ
`"driver": "apc40"` / `"apc40mk2"`, `"host_mode": 65` (0x41), `"shift_key"`
= note `0x62`, encodeurs déclarés (`0x2F`, `0x0D`).

## Interface
Dans l'onglet Contrôleur : « Profil : APC40 mkII — Laser Studio (intégré) ».
Une aide « Disposition de l'APC40 » affiche ce tableau (voir aussi T-210).

## Critères d'acceptation
- [x] APC40 mkII branché : le pad en haut à gauche joue la 1re cue de la page, celui en bas à droite la 40e.
- [x] Même chose avec un APC40 d'origine (même disposition physique).
- [x] Scene Launch 3 affiche la page 3 dans l'UI ; l'onglet de page cliqué dans l'UI change la page de la grille MIDI.
- [x] Stop All = blackout immédiat (laser désarmé).
- [x] Tap tapé 4 fois à 120 BPM → tempo 120 ± 1.
- [x] Master fader : pas de saut de luminosité au branchement (pickup).
- [x] Les deux profils se chargent sans avertissement (tous les identifiants existent).

## Tests
- Unitaires : table note → slot pour les deux modèles (4 coins + centre), tous les identifiants des profils intégrés existent dans le registre T-145, pas deux mappings sur le même message+Shift.
- e2e via `/api/midi/inject` (T-209) avec un faux APC40 mkII : pad → cue, Scene Launch → page, Stop All → `armed: false`.

## Notes
- Inspirations (pas de copie) : grille 5×8 + banques de Showcontroller LIVE, calques/Shift de BEYOND. Voir le rapport §3.
- Le Nudge est inversé entre les deux modèles (protocoles Akai) : piège classique.
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — architecte : l'utilisateur possède un **APC40 mkII** → c'est la cible principale à tester en premier ; le profil APC40 d'origine reste utile. Le support générique de tout contrôleur est dans T-211.
- 2026-09-28 — dev-agent (feat/apc40-profile) : profils `apc40-mk2.json` et `apc40.json` écrits (grille, pages, Stop All = blackout, calques 1–4, faders/potards en reprise, master, Tap/Nudge/Tempo, Play/Stop, pédale), uniquement avec des identifiants existants + un nouveau `timeline.toggle` (Play = lecture/pause). Sécurité : tant que « Armer depuis le MIDI » est décoché, Shift + Stop All reste un blackout (avant : rien). Profils intégrés affichés « (intégré) ». Non faits faute d'identifiant (libres pour T-203) : Shift + pad = flash, tranches Left/Right, Track Select/potards Device, banques Pan/Sends/User, fader strobe, Metronome, Record, Nudge en BPM (il décale la phase). Critères vérifiés en simulation (mkII e2e, les deux modèles en unitaires) ; reste à confirmer sur le vrai APC40 mkII. `cargo test` 410 ok, clippy propre, e2e 104/104 (sur develop d680a15). Note : docs/prs/apc40-profile.md.
- 2026-09-28 — architecte (review) : APPROUVÉ et fusionné dans develop. À essayer sur l'APC40 mkII de l'utilisateur.
