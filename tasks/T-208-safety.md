---
id: T-208
title: Sécurité du pilotage MIDI (blackout prioritaire, armement opt-in, reprise en douceur)
status: done
area: safety
priority: P0
depends_on: [T-202]
owner: "dev-agent midi-map"
branch: feat/midi-map
source: docs/research/midi-apc40.md#7-safety-notes-claudemd
---

## Contexte
Un contrôleur physique peut envoyer des commandes sans regarder l'écran, par
accident (coude sur un fader, pad appuyé en le rangeant). Les règles de
CLAUDE.md s'appliquent : laser toujours désarmé au départ, seul un geste
explicite l'arme, Échap = blackout immédiat.

## À faire
- **Blackout prioritaire** : l'action blackout (Stop All Clips par défaut)
  est traitée avant tout autre message du même lot et désarme le laser
  exactement comme Échap/`/api/arm {on:false}`.
- **Armement par MIDI désactivé par défaut** : réglage global « Autoriser
  l'armement depuis le contrôleur » (faux par défaut, enregistré dans
  `<data-dir>/midi/devices.json`). Même activé :
  - il faut **Shift + maintien 1 s** du bouton d'armement ;
  - impossible pendant les 5 s qui suivent le branchement du contrôleur ;
  - l'apprentissage MIDI (T-203) refuse la cible « armer » tant que
    l'option est désactivée.
- **Luminosité** : toute valeur venant du MIDI est bornée par le maximum de
  sécurité (T-003 ; tant qu'il n'existe pas : 1,0) ; le fader maître est toujours en `pickup`, même si un
  profil perso dit le contraire.
- **Pickup** activé par défaut pour tous les contrôles absolus des profils
  intégrés (pas de saut au branchement ni au changement de page/calque).
- **Déconnexion** du contrôleur : l'état reste, bandeau « Contrôleur MIDI
  déconnecté » ; option « Blackout si le contrôleur se déconnecte » (faux
  par défaut).
- Aucune action MIDI ne contourne l'état désarmé : sans armement, rien ne
  sort, quel que soit le message reçu.

## Modèle de données
```rust
pub struct MidiSafety {
    pub allow_arm: bool,              // false
    pub blackout_on_disconnect: bool, // false
}
```

## Interface
Onglet Contrôleur, encadré « Sécurité » : « Autoriser l'armement depuis le
contrôleur (Shift + maintien 1 s) », « Blackout si le contrôleur se
déconnecte ». Bandeau rouge quand un contrôleur se déconnecte.

## Critères d'acceptation
- [x] Par défaut, aucune combinaison MIDI ne peut armer le laser (test exhaustif sur le profil APC40).
- [x] Option activée : Shift + Stop All tenu 1 s arme ; relâché avant 1 s ne fait rien.
- [x] Un lot [pad, fader, blackout] finit désarmé.
- [x] Fader maître à 127 au branchement : luminosité inchangée.
- [x] La luminosité issue du MIDI ne dépasse jamais le maximum de sécurité.

## Tests
- Unitaires : ordre de traitement du blackout, armement refusé par défaut, délai 1 s, garde de 5 s après branchement, bornage de luminosité, pickup forcé sur le maître.
- e2e (`--no-midi --midi-test`) : injection de Shift + Stop All → `armed` reste `false`.

## Notes
- Tests et agents : jamais `--device`, jamais d'armement réel (CLAUDE.md).
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — dev-agent midi-map (branche `feat/midi-map`, avec T-202) : `studio/src/midi/safety.rs` + moteur. Blackout traité en premier dans chaque lot ; armement MIDI seulement avec `allow_arm` (faux par défaut, `devices.json`), Shift + maintien 1 s, ≥ 5 s après branchement, re-vérifié à l'échéance (seul chemin qui met `armed = true` hors UI) ; luminosité MIDI plafonnée (`BRIGHTNESS_MAX` = 1,0 en attendant T-003) et toujours en pickup ; déconnexion → `lost` + bandeau rouge, option blackout. `POST /api/midi/safety`, encadré « Sécurité » dans l'onglet Contrôleur. Test exhaustif du refus par défaut (toutes notes/CC/programmes, 16 canaux, avec/sans Shift). Reste : refus d'apprendre « armer » → T-203 ; test e2e `--midi-test` → T-209. Statut → review.
- 2026-09-27 — architecte (review) : APPROUVÉ et fusionné ; l'armement MIDI passe désormais par la porte d'interlocks (T-250) et reste bloqué par l'arrêt d'urgence.
