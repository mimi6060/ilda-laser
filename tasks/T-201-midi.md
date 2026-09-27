---
id: T-201
title: Détection des contrôleurs et profils par appareil (APC40 / APC40 mkII)
status: review
area: midi
priority: P1
depends_on: [T-200]
owner: "dev-agent midi-core"
branch: feat/midi-core
source: docs/research/midi-apc40.md#25-model-detection
---

## Contexte
L'APC40 d'origine et l'APC40 mkII n'adressent pas la grille de la même façon
et n'ont pas les mêmes LED. Le studio doit reconnaître le modèle tout seul,
le passer en mode « hôte » (LED pilotées par nous) et charger le bon profil.

## À faire
- À la connexion d'un port, envoyer la **Device Inquiry** `F0 7E 7F 06 01 F7`
  et attendre la réponse ≤ 500 ms : octet 7 = `0x73` → APC40, `0x29` →
  APC40 mkII, `0x28` → APC mini (reconnu, profil générique pour l'instant).
  Sans réponse : repli sur le nom du port (`APC40 mkII` / `APC40`), sinon
  appareil générique.
- Pour un APC40/mkII : envoyer l'**Introduction**
  `F0 47 7F <pid> 60 00 04 41 <maj> <min> <bugfix> F7` (mode `0x41`,
  « Ableton Live mode » : tous les boutons momentanés, Track Select envoie des
  notes, LED pilotées par l'hôte). Le mode (`0x40`/`0x41`/`0x42`) est un champ
  du profil. Sur mkII, lire la réponse `0x61` (positions des 9 faders) et la
  garder pour la reprise en douceur (T-202).
- Quand on quitte (Ctrl-C) ou qu'on ferme le port : éteindre les LED (T-205)
  puis renvoyer le mode `0x40` pour rendre l'appareil dans son état d'origine.
- Profils :
  - profils **intégrés** (`studio/profiles/apc40.json`, `apc40-mk2.json`,
    `generic.json`) chargés via `include_str!`, jamais écrits sur disque ;
  - profils **utilisateur** dans `<data-dir>/midi/profiles/<slug>.json` ;
  - `<data-dir>/midi/devices.json` : nom de port → slug de profil choisi,
    port activé ou non ;
  - si l'utilisateur modifie un profil intégré, on enregistre une copie
    `<slug>-perso` qui devient le profil de ce port.
- API : `GET /api/midi/profiles`, `POST /api/midi/profile {port, profile}`,
  `POST /api/midi/device {port, enabled}`.
- Charger un profil invalide ne plante jamais : erreur dans `/api/midi` et
  repli sur `generic`.

## Modèle de données
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,                 // 1
    pub name: String,
    #[serde(default)] pub driver: Driver,   // Generic | Apc40 | Apc40Mk2
    #[serde(default)] pub r#match: DeviceMatch, // port_contains: Vec<String>, product_id: Option<u8>
    #[serde(default = "mode_41")] pub host_mode: u8,
    #[serde(default)] pub mappings: Vec<Mapping>, // défini par T-202
}
pub enum Model { Apc40, Apc40Mk2, ApcMini, Unknown }
```
Chaque `MidiDevice` gagne `model: Model` et `profile: String`.

## Interface
Onglet « Contrôleur » (créé ici, complété par T-203/T-210) : liste des
appareils MIDI avec modèle détecté, case « Activé », menu déroulant « Profil »
(profils intégrés + profils perso). Libellés : « Modèle détecté »,
« Profil », « Réinitialiser le profil ».

## Critères d'acceptation
- [ ] Un APC40 mkII branché est reconnu `Apc40Mk2`, reçoit l'Introduction 0x41 et ses pads cessent de s'allumer tout seuls quand on appuie.
- [ ] Un APC40 d'origine est reconnu `Apc40` (même comportement).
- [x] Le choix de profil pour un port est retrouvé après redémarrage.
- [x] Un JSON de profil corrompu donne un message d'erreur lisible et le profil `generic`, sans panique.
- [ ] À l'arrêt du studio, les LED de l'APC s'éteignent.

## Tests
- Unitaires : décodage de la réponse Device Inquiry (0x73, 0x29, 0x28, réponse tronquée) ; octets exacts de l'Introduction pour chaque modèle/mode ; choix du profil (nom de port, product id, préférence enregistrée) ; aller-retour JSON des profils ; `#[serde(default)]` sur un profil minimal.
- Avec port virtuel (T-209) : un faux APC40 mkII qui répond à l'Inquiry est détecté.

## Notes
- Octets et modes : protocoles officiels Akai (voir `docs/research/midi-apc40.md` §1.1, §2.1).
- Les profils intégrés sont notre propre disposition (T-204), pas une copie des modèles BEYOND/Showcontroller.
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — dev-agent midi-core (feat/midi-core) : Device Inquiry (réponse ≤ 500 ms, sinon nom du port), Introduction 0x41 (mode = `host_mode` du profil), réponse 0x61 du mkII gardée dans `MidiDevice.faders`, à l'arrêt ou à la désactivation : LED éteintes puis mode 0x40. Profils intégrés `studio/profiles/*.json` (mappings vides, remplis par T-204), profils perso `<data-dir>/midi/profiles/`, `devices.json`, copie `-perso`, `GET /api/midi/profiles`, `POST /api/midi/profile`, `POST /api/midi/device`, section « Contrôleur » (modèle détecté, Activé, Profil, Réinitialiser le profil). Faux APC40 mkII sur port virtuel CoreMIDI détecté et introduit (test ignoré, vert). Non cochés : les critères qui demandent le vrai matériel (pads, LED).
