---
id: T-202
title: Moteur de correspondances MIDI → contrôles (boutons, faders, encodeurs, Shift)
status: todo
area: midi
priority: P1
depends_on: [T-200, T-201, T-145]
owner: ""
branch: ""
source: docs/research/midi-apc40.md#6-midi-learn-and-mapping-storage
---

## Contexte
Un message MIDI doit pouvoir piloter n'importe quel contrôle du studio : cue,
modificateur en direct, tempo, calque, transport de la timeline, blackout.
Tous ces contrôles ont un identifiant stable défini par T-145 ; ce moteur
relie « message MIDI » → « identifiant de contrôle + valeur ».

## À faire
- Appliquer les `mappings` du profil actif de chaque port, dans le thread MIDI
  (T-200), en passant par la **même fonction d'action** que l'API HTTP
  (celle de T-145) : aucun chemin de contrôle réservé au MIDI.
- Types de correspondance (`mode`) :
  - `trigger` : Note On (ou CC > 63) déclenche l'action une fois ;
  - `toggle` : chaque appui inverse un booléen ;
  - `momentary` : actif tant que la touche est tenue (Note Off / CC 0 = relâché) — sert au flash de cue et au strobe ;
  - `absolute` : CC 0–127 → `min..max` du contrôle, courbe `linear` ou `log` ;
  - `relative` : encodeur (1–63 = +n, 127…64 = −1…−64, format « two's complement 7 bits » des APC), multiplié par `step` ;
  - `grid` : slot *n* de la page de cues courante (utilisé par T-204).
- **Reprise en douceur (pickup)** pour `absolute` quand `pickup: true` :
  si la position physique est à plus de 3 % de la valeur actuelle, ignorer le
  fader jusqu'à ce qu'il croise la valeur (ou arrive à ±3 %). Positions
  initiales connues via la réponse `0x61` du mkII (T-201), sinon inconnues =
  pickup actif.
- **Calque Shift** : un contrôle déclaré `shift_key` dans le profil (Shift de
  l'APC, note `0x62`) ; chaque mapping a `shift: bool`. Tant que Shift est
  tenu, seuls les mappings `shift: true` sont cherchés, puis repli sur les
  `shift: false`.
- Un canal `null` dans l'entrée = n'importe quel canal. Les APC utilisent le
  canal pour la piste (0–7) : il fait alors partie de la clé.
- Les changements venant du MIDI se voient dans l'UI (via `/api/state` déjà
  relu par le navigateur).
- Limiter le débit : un fader qui envoie 200 CC/s ne doit pas provoquer plus
  d'une écriture d'état par trame moteur (coalescer le dernier CC par contrôle).

## Modèle de données
```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mapping {
    pub input: MidiInput,              // { kind: note|cc|pitch_bend, channel: Option<u8>, number: u8 }
    #[serde(default)] pub shift: bool,
    pub target: String,                // identifiant T-145, ex. "live.size"
    #[serde(default)] pub args: serde_json::Value, // ex. { "page": 3 } ou { "slot": 12 }
    pub mode: MapMode,                 // Trigger | Toggle | Momentary | Absolute | Relative | Grid
    #[serde(default)] pub min: Option<f32>,
    #[serde(default)] pub max: Option<f32>,
    #[serde(default)] pub step: Option<f32>,
    #[serde(default)] pub curve: Curve,  // Linear (défaut) | Log
    #[serde(default)] pub pickup: bool,
}
```
État d'exécution (non sérialisé) : `shift_held`, positions physiques connues,
pickup « accroché » par contrôle.

## Interface
Pas d'UI propre (voir T-203). Les valeurs pilotées au MIDI bougent les
curseurs de l'interface.

## Critères d'acceptation
- [ ] Un CC mappé en `absolute` sur un modificateur change sa valeur dans `/api/state` dans la trame suivante.
- [ ] Un encodeur relatif (CC `0x2F` « Cue Level ») augmente/diminue la valeur, bornée à `min..max`.
- [ ] Avec pickup, un fader physique à 100 % alors que la valeur est à 20 % ne change rien tant qu'il ne repasse pas par 20 %.
- [ ] Shift + bouton déclenche l'action Shift, le bouton seul l'action normale.
- [ ] `momentary` : la cue flash s'arrête au relâchement.
- [ ] Toutes les actions MIDI passent par la fonction commune de T-145 (revue).

## Tests
- Unitaires : conversion absolute (bornes, courbe log), relative (1, 63, 64, 127), pickup (croisement vers le haut et vers le bas, seuil 3 %), Shift + repli, canal `null` vs canal fixe, coalescence de 100 CC en une écriture.
- Aller-retour JSON d'un `Mapping` et chargement d'un profil sans champs optionnels.

## Notes
- **Sécurité** : l'armement et le blackout suivent les règles de T-208 ; ce moteur ne doit jamais armer le laser tout seul.
- Dépend du schéma d'identifiants de T-145 ; si un identifiant de profil n'existe pas, le mapping est ignoré avec un avertissement (pas de panique).
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
