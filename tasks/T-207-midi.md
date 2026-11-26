---
id: T-207
title: Horloge MIDI (entrée pour caler le BPM, sortie vers l'APC40 mkII)
status: todo
area: tempo
priority: P3
depends_on: [T-200, T-150]
owner: ""
branch: ""
source: docs/research/midi-apc40.md#23-outbound-rgb-pads-and-other-leds
---

## Contexte
Un DJ qui joue avec un logiciel ou une platine qui émet l'horloge MIDI veut
que le laser suive son tempo sans taper Tap. Dans l'autre sens, l'APC40 mkII
cale la pulsation de ses pads sur l'horloge MIDI qu'il reçoit.

## À faire
- **Entrée** : option « Suivre l'horloge MIDI » avec choix du port. Compter
  les `F8` (24 par noire), lisser le BPM (moyenne glissante sur 2 temps,
  arrondi à 0,1), `Start` (`FA`) remet la phase à zéro, `Stop` (`FC`) fige.
  Quand l'horloge s'arrête > 2 s, revenir au tempo manuel sans saut.
  Pendant le suivi, Tap et Nudge sont ignorés (message dans l'UI).
- **Sortie** : option « Envoyer l'horloge au contrôleur » (par défaut active
  pour l'APC40 mkII) : émettre `F8` 24 fois par noire depuis le tempo T-150,
  dans un thread dédié à cadence stable (pas dans la boucle 60 fps).
- Le tempo reste celui de T-150 (une seule source de vérité), l'horloge MIDI
  n'est qu'une source ou une sortie.

## Modèle de données
`TempoSource::{Manual, Tap, MidiClock { port: String }}` (ou champ équivalent de T-150), `DeviceSettings.send_clock: bool`.

## Interface
Dans le panneau Tempo : menu « Source : Manuel / Horloge MIDI (<port>) » et
indicateur « Horloge MIDI : 124,0 BPM ». Onglet Contrôleur : case
« Envoyer l'horloge au contrôleur ».

## Critères d'acceptation
- [ ] Une horloge à 128 BPM est lue à 128,0 ± 0,2 après 2 temps.
- [ ] Arrêt de l'horloge : le tempo reste à la dernière valeur, pas de saut.
- [ ] Avec la sortie active, les pads pulsés de l'APC40 mkII battent au tempo du studio (à vérifier sur le matériel).

## Tests
- Unitaires : estimateur de BPM avec horloge simulée (jitter ±1 ms), Start/Stop, perte d'horloge.
- Port virtuel (T-209) : envoyer 96 `F8` à 120 BPM et lire le BPM.

## Notes
- MIDI Time Code (MTC) pour la timeline : relève de T-160, pas de cette tâche.
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
