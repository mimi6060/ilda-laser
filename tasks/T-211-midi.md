---
id: T-211
title: Contrôleurs MIDI génériques (n'importe quel appareil)
status: todo
area: midi
priority: P1
depends_on: [T-200, T-201, T-202, T-203]
owner: ""
branch: ""
source: demande utilisateur 2026-09-27 ; docs/research/midi-apc40.md
---

## Contexte
L'utilisateur a un APC40 **mkII** (cible principale, voir T-204/T-206),
mais veut que le studio marche avec **n'importe quel contrôleur MIDI** :
un autre APC, un Launchpad, un nanoKONTROL, un X-Touch Mini, un clavier,
un pad… Rien dans le moteur MIDI ne doit être spécifique à l'APC40 en
dehors de ses profils.

## À faire
- Tout appareil MIDI branché apparaît dans la liste des contrôleurs, avec
  son nom de port, même s'il n'est pas reconnu, et utilise le profil
  `generic` (aucune correspondance au départ).
- Le MIDI learn (T-203) fonctionne sur tous les types de messages :
  Note On/Off, CC absolu 0–127, CC relatif (les 3 encodages courants :
  complément à 2, signe binaire, offset 64), Pitch Bend (14 bits), Program
  Change ; sur n'importe quel canal.
- Plusieurs contrôleurs en même temps, chacun avec son profil.
- Retour LED générique optionnel par correspondance : pour une sortie
  « bouton », valeurs de vélocité/CC configurables pour éteint / allumé /
  clignotant (beaucoup de contrôleurs allument leurs LED avec la
  vélocité d'une Note On sur la même note).
- Export / import de profils (fichier JSON) depuis l'interface, pour les
  partager ou les sauvegarder.
- Modèles de départ facultatifs, écrits par nous à partir des
  documentations publiques des fabricants : Akai APC mini, Novation
  Launchpad (mode programmeur), Korg nanoKONTROL2, Behringer X-Touch Mini.
  Chaque modèle = un fichier JSON de profil ; aucun n'est obligatoire pour
  que l'appareil fonctionne (le learn suffit).

## Modèle de données
Réutilise `Profile` / `Mapping` de T-201/T-202 :
- `Driver::Generic` pour tout appareil non reconnu.
- `MidiInput` couvre `Note { ch, note }`, `Cc { ch, cc, mode: Absolute | Relative(RelEncoding) }`,
  `PitchBend { ch }`, `ProgramChange { ch, program }` ; `ch: Option<u8>` (None = tous canaux).
- `LedFeedback { off: u8, on: u8, blink: Option<u8> }` optionnel par correspondance.
- Profils utilisateur dans `studio-data/midi/profiles/<nom>.json`.

## Interface
Section « Contrôleurs MIDI » :
- liste des appareils branchés (nom, profil utilisé, activité en direct : un
  voyant qui s'allume à chaque message reçu) ;
- menu « Profil » par appareil (générique, modèles, profils perso) ;
- boutons « Exporter le profil » / « Importer un profil » ;
- moniteur MIDI (derniers messages reçus, pour aider au learn).

## Critères d'acceptation
- [ ] Un appareil inconnu est listé et contrôlable après un MIDI learn, sans code spécifique.
- [ ] Learn validé pour Note, CC absolu, CC relatif (3 encodages), Pitch Bend, Program Change.
- [ ] Deux contrôleurs branchés en même temps pilotent chacun leurs contrôles.
- [ ] Export puis import d'un profil redonne exactement les mêmes correspondances.
- [ ] Le retour LED générique allume/éteint la LED d'une Note mappée (vérifié avec un port virtuel).
- [ ] Les règles de sécurité de T-208 (blackout prioritaire, armement désactivé par défaut) s'appliquent à tous les appareils, pas seulement à l'APC40.

## Tests
Unitaires sur le décodage des messages et des encodages relatifs ; tests
avec ports virtuels / appareil simulé de T-209.

## Notes
Les modèles de profils sont écrits par nous d'après les tableaux MIDI
publics des fabricants (faits techniques), à lister dans
`docs/CONTENT_SOURCES.md` si un fichier tiers est repris tel quel.

## Journal
- 2026-09-27 — architecte : créée à la demande de l'utilisateur (APC40 mkII principal, mais MIDI générique).
