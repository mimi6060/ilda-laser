---
id: T-200
title: Entrée/sortie MIDI native (midir, CoreMIDI)
status: done
area: midi
priority: P1
depends_on: []
owner: "dev-agent midi-core"
branch: feat/midi-core
source: docs/research/midi-apc40.md#4-architecture-native-rust-midi-vs-web-midi
---

## Contexte
L'utilisateur veut piloter le studio avec son Akai APC40. Le MIDI doit vivre
dans le processus Rust (pas dans l'onglet du navigateur) : le laser doit
continuer à répondre onglet fermé, les LED doivent suivre l'état du moteur, et
la latence doit rester sous la milliseconde. C'est la brique de base de toutes
les tâches MIDI (T-201 à T-210).

## À faire
- Ajouter la dépendance `midir = "0.11"` (MIT) au crate `laser-studio`.
- Nouveau module `midi.rs` :
  - énumérer les ports d'entrée et de sortie CoreMIDI ;
  - ouvrir **tous** les ports d'entrée (sauf ceux désactivés par l'utilisateur)
    avec `ignore(Ignore::None)` pour recevoir le SysEx ;
  - le callback midir ne fait que **décoder** et pousser un `MidiEvent` dans
    un `std::sync::mpsc::Sender` (aucun verrou dans le callback) ;
  - un thread « midi » consomme le canal, garde le dernier message reçu et
    l'historique court (20 derniers) dans `Shared.midi`, et expose un point
    d'accroche `fn handle(&mut Shared, &MidiEvent)` que T-202 remplira ;
  - **branchement à chaud** : midir n'a pas de notification, donc re-scanner
    la liste des ports toutes les 2 s ; ouvrir les nouveaux ports, fermer
    proprement ceux qui ont disparu, sans bloquer le moteur 60 fps ;
  - ouvrir le port de **sortie** du même nom que chaque entrée (pour les LED,
    T-205) et exposer `fn send(&self, port: &str, bytes: &[u8])`.
- Décodeur pur (testable sans matériel) : octets → `MidiEvent`
  (NoteOn vel>0, NoteOff ou NoteOn vel 0, CC, PitchBend, SysEx, Clock `F8`,
  Start/Stop/Continue), messages incomplets ignorés.
- Option CLI `--no-midi` : n'ouvre aucun port (obligatoire pour les tests et
  l'e2e, pour ne jamais prendre ni allumer l'APC40 de l'utilisateur).
- API : `GET /api/midi` → `{ enabled, devices: [{name, input, output, connected, profile}], last: MidiEvent|null }`.
- Journal (`log::info!`) à chaque connexion/déconnexion.

## Modèle de données
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MidiMsg {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
    Cc { channel: u8, number: u8, value: u8 },
    PitchBend { channel: u8, value: u16 },
    SysEx { bytes: Vec<u8> },
    Clock, Start, Stop, Continue,
}
pub struct MidiEvent { pub port: String, pub msg: MidiMsg, pub at: Instant }
pub struct MidiState { pub enabled: bool, pub devices: Vec<MidiDevice>, pub last: Option<MidiEvent>, pub recent: VecDeque<MidiEvent> }
```
`Shared` gagne `midi: MidiState` (non sérialisé dans les scènes).

## Interface
Aucune interface pour l'instant, sauf une ligne d'état dans la barre du haut :
« MIDI : APC40 mkII connecté » / « MIDI : aucun contrôleur » (lecture de
`/api/midi` dans la boucle d'état existante).

## Critères d'acceptation
- [ ] `cargo run -p laser-studio -- --port 8090 --data-dir /tmp/studio-test` liste l'APC40 branché dans `/api/midi`.
- [ ] Débrancher/rebrancher l'APC40 : le studio le voit repartir en ≤ 3 s, sans redémarrage ni saccade du rendu.
- [x] `--no-midi` : aucun port ouvert, `/api/midi` renvoie `enabled: false`.
- [x] Le callback midir ne prend aucun verrou (revue de code).
- [x] Build, tests, clippy `-D warnings` verts.

## Tests
- Unitaires sur le décodeur : Note On/Off, Note On vel 0 = Off, CC, pitch bend 14 bits, SysEx complet, octets tronqués ignorés, messages temps réel.
- Test de connexion avec un port virtuel (`midir::os::unix::VirtualOutput`) dans T-209.

## Notes
- midir ne notifie pas les branchements : le polling 2 s suffit ; le crate `coremidi` (notifications) est une option plus tard.
- Plusieurs applis peuvent ouvrir le même port d'entrée CoreMIDI ; deux instances du studio piloteraient les mêmes LED : d'où `--no-midi` pour les tests.
- Sécurité laser : ce module ne touche ni à l'armement ni à la sortie laser.
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
- 2026-09-27 — dev-agent midi-core (feat/midi-core) : module `studio/src/midi/` (décodeur pur, thread « midi », re-scan 2 s, `--no-midi`, `GET /api/midi`, ligne d'état « MIDI : … » dans la barre du haut). midir 0.11 (MIT). Frontière `Backend` : tests avec un faux CoreMIDI (branchement/débranchement, envoi, pannes) et deux tests `#[ignore]` sur ports virtuels CoreMIDI (`-- --ignored midi_virtual`, verts). 116 tests verts + 2 ignorés, clippy propre. Non cochés : les deux critères qui demandent le vrai APC40 (à valider par l'utilisateur). Voir docs/prs/midi-core.md.
- 2026-09-27 — architecte (review) : APPROUVÉ et fusionné dans develop. Critères matériels à vérifier avec l'APC40 mkII de l'utilisateur.
