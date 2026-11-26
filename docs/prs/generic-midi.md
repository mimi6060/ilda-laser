# feat/generic-midi — T-211 Generic MIDI controllers (any device)

## What / why
The user's main controller is an APC40 mkII, but the studio must work with
**any** MIDI controller (another APC, Launchpad, nanoKONTROL, X-Touch Mini,
a keyboard…). The MIDI stack (T-200…T-209) already opened every port and
fell back to the `generic` profile; this branch fills the gaps so an unknown
device is fully usable with no device-specific code, and adds optional
starter templates.

- **Unknown devices** are listed (port name, « Modèle détecté : inconnu »),
  use `generic`, and are driven after a MIDI learn. Nothing APC-specific is
  sent to them (only the universal Device Inquiry, as before).
- **Learn on every message type, any channel** (already: Note, CC absolute,
  pitch bend 14 bits, program change). New in `learn.rs`: the **three
  relative encodings** are told apart from what the CC has sent so far
  (`cc_steps` bit set + `refine_encoding`: the only encoding whose small
  steps explain every value seen wins; 1–3 and 65–67 = sign bit; a fader
  swept through everything fits none → first guess). One direction alone
  is ambiguous (two encodings fit), so the mappings list gets a **mode
  select**: Absolu / Relatif (complément à 2 / bit de signe / offset 64).
- **Several controllers at once**, each through its own profile (worker
  test with a nanoKONTROL2 + X-Touch Mini: the same CC 1 drives a layer
  dimmer on one and the size on the other; blackout from either).
- **Generic LED feedback** per mapping: `Mapping.led: Option<LedFeedback
  { off, on, blink?, present? }>` (`#[serde(default)]`, skipped when
  `None`: existing profiles load and save unchanged). `led.rs`
  `render_generic` sends it back on the mapping's own note (Note On
  velocity) or CC (value) and channel (0 for "any channel"); toggles show
  their value, `page.N` the current page, grid pads present / playing /
  flash (`blink`, else blinked by hand with the beat), Shift mappings light
  a button their plain mapping leaves dark (`blink` value). Unlike the APC
  Activator, a generic `layer.N.mute` button is lit while **muted**.
  The worker now refreshes LEDs of any device with a profile (APCs as
  before); a generic device with nothing to light is only looked at every
  250 ms, and `goodbye` switches off what we lit on any device (APC
  Introduction 0x40 only for APCs).
- **Export / import** of profiles: `POST /api/midi/profile/export
  {slug | port}` → `{slug, profile}`; `POST /api/midi/profile/import
  {profile, slug?, port?}` checks it like a file loaded at start-up
  (`Profile::parse`), saves it under a **new** slug (`slugify` of the file
  name or profile name; `-2`, `-3`… if taken: built-ins and existing
  profiles are never overwritten) and optionally assigns it to the port.
- **Mapping edits**: `POST /api/midi/mapping/update {port, index, mode?,
  encoding?, led?}` (`led: null` removes it). Only CC on fader-like
  mappings can switch absolute ↔ relative; LED only on Note / CC. Like a
  learn, editing a built-in profile writes its `-perso` copy. Re-learning
  the same button keeps its LED settings.
- **Starter templates** (built-in, our own layouts, numbers from the makers'
  public MIDI charts — listed in `docs/CONTENT_SOURCES.md`, nothing copied):
  `apc-mini` (first model, matched by Akai product id 0x28 only),
  `launchpad` (X / Mini MK3 in Programmer mode), `nanokontrol2`,
  `x-touch-mini` (Standard mode, layer A, channel 11). Each maps grid /
  pages / layers / master controls, has a blackout button, and **never maps
  `transport.arm`** (unit test). They are picked automatically by port name
  (generic stays the fallback; « Réinitialiser le profil » / the menu
  switch back). `detect.rs`: an « APC mini mk2 » port is no longer taken for
  an APC mini (other layout).
- **UI** (« RÉGLAGES › Contrôleur », French): per device an **activity
  light** (flashes on each message: `devices[].messages` counter, plus
  `activity_ms`), the **profile menu grouped** (Générique / Modèles de
  départ / Akai APC40 / Profils perso), **Exporter le profil** (downloads
  `<slug>.json`) / **Importer un profil** (file picker); the mappings list
  gets the **Mode** select and a **LED** field (« 0 127 » or « 0 1 2 »,
  empty = none; hidden for APC drivers which light their own); a **Moniteur
  MIDI** (last 12 messages, with channel). The panel polls every 400 ms
  while open (2 s otherwise). Profiles are re-fetched when a device uses
  an unknown slug (a learn's `-perso` copy now shows in the menu at once).
- **Test mode**: `FakeApc::generic()` (answers nothing) and
  `POST /api/midi/plug {port}` (only with `--midi-test`, ≤ 8 devices) to plug
  a simulated generic controller next to the simulated APC.

## Safety (T-208 applies to every device)
- MIDI still never arms by itself: arming needs the opt-in « Autoriser
  l'armement depuis le contrôleur », Shift + 1 s hold, not in the first
  5 s — unchanged code path, shared by all devices. Templates have no arm
  mapping; an imported profile goes through the same engine and rules.
- Blackout first in each batch, for every device (tested from two generic
  devices at once). Brightness from MIDI stays capped and in pickup.
- No real MIDI port is opened by tests (SimMidi / FakeBackend only); e2e
  runs with `--no-midi --midi-test --no-audio`, never `--device`.

## Tests
- `cargo test -p laser-studio`: **708 passed**, 7 ignored (CoreMIDI
  virtual-port tests, as before). New/updated unit tests:
  - `mapping`: `led_feedback_defaults`, LED in the JSON round trip;
  - `learn`: `relative_encodings_are_told_apart` (sign bit / offset 64 /
    two's complement on channels 10, 16, 1; fader sweep; manual change; the
    engine follows), `mapping_updates_are_checked`;
  - `led`: `generic_led_feedback_uses_the_mapping_values`;
  - `profile`: 7 built-ins with 4 templates, choice by product id / name,
    `import_never_overwrites_and_round_trips`, `slugify_makes_valid_slugs`;
  - `worker`: `template_device_gets_generic_led_feedback`,
    `two_controllers_each_drive_their_own_controls`, unknown device
    renamed to « USB MIDI Keyboard » (a nanoKONTROL2 now gets its template);
  - `api`: export → import round trip, `mapping_update_route`,
    `templates_use_known_controls_and_never_arm`,
    `plugged_generic_device_learns_and_gets_its_led` (simulated port);
  - `mod`: per-device activity.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `npx playwright test --workers=2`: **185 passed**, including the new
  `tests/generic-midi.spec.ts` (unknown device listed, activity light,
  monitor, templates in the menu; learn CC + note on non-default channels;
  encoding select; LED field lights / clears the note on the simulated
  device; export download then import via the file picker gives the same
  mappings under `generic-perso-2`). One earlier full run had 2 flaky
  failures in unrelated specs (`cues.spec` tab count, `presence.spec`),
  green on rerun and on a second full run.

## Risks / notes
- Template numbers come from public docs as remembered, not checked on the
  hardware: the X-Touch Mini channel (11) and the nanoKONTROL2 LED mode
  (« External » needed for host LEDs, set with Korg's editor) are the most
  likely to need a tweak. They are only templates: learn / edit fixes any
  mismatch, and the user can switch a device back to « Générique ».
- Templates are chosen automatically by port name: a nanoKONTROL2 /
  X-Touch Mini / Launchpad X / Mini MK3 / APC mini now gets mappings at
  plug-in instead of an empty profile.
- LED `present` (cue present on a grid pad) has no UI field; the UI keeps
  it when the other values are edited.
- Relative-encoding detection can only be sure once both directions were
  turned; otherwise the guess can be wrong (sign bit turned left looks like
  offset 64) — fixed in one click in the Mode select.
- Launchpad Programmer mode must be entered on the device (no SysEx sent).

## Review
