# feat/midi-learn — T-203 MIDI learn (clic droit → « Apprendre MIDI »)

## What / why
Map any button, knob or fader of any controller to any control without
editing JSON: point at the control on screen, touch the hardware, done.
It is also what makes non-APC controllers usable (T-211).

- `studio/src/midi/learn.rs` (new), `MidiState.learn`:
  - `start(target, args, shift)`: target = any external control id (aliases
    resolved, e.g. `live.size` → `master.size`) or `grid` + `args.slot`.
    15 s timeout (`expire`, called by `engine::frame`).
  - `capture` runs first for each event of a worker batch (after the
    blackout-first pass): the next **Note On, CC, pitch bend or program
    change**, on any channel, of any port, becomes a mapping (channel
    stored). Not captured, and passed on to the engine as usual: Note Off,
    real-time / SysEx, the profile's Shift key, and a CC that repeats its
    last value (a still fader). The captured message is **consumed**: it
    doesn't act (learning « Tap » doesn't tap, learning a cue doesn't play it).
  - Mode inferred: button targets → `trigger` / `toggle` / `momentary`
    from the T-145 kind (cue cells `grid.p.r.c` → `momentary`); continuous
    / choice targets: CC declared as an encoder in the profile → `relative`
    with its encoding; an unknown CC whose first value looks like an encoder
    step (1–3 / 125–127 → two's complement, 61–63 / 65–67 → offset 64)
    waits for the next message: the same value again → `relative`, a
    different one → a fader; other CC / pitch bend →
    `absolute`, target's own range (`min`/`max` left empty), `pickup: true`,
    pickup seeded with the learned position so moving on across the value
    takes over. A note / program change on a continuous target is ignored
    with a message (learning keeps waiting).
  - Same hardware control already mapped (same Shift layer, overlapping
    channel) to another target → `conflict`; the UI asks « Remplacer
    l'ancienne affectation (Taille maître) ? » → `confirm {replace}`.
    Re-learning the same target updates it silently.
  - Saving: the profile the port uses. A built-in profile is never written:
    the edit goes to `<slug>-perso` (or `-perso-2`… if that name is taken, so
    an older copy is never overwritten), assigned to the port in
    `devices.json`; the device switches at once (the worker agrees within
    250 ms).
  - `transport.arm`: `start` returns 403 unless the T-208 option
    `allow_arm` is on; re-checked at capture / confirm. It is always a Shift
    mapping (the MIDI arming gesture needs Shift), so a profile without a
    Shift key refuses it. `Shift + 1 s hold` etc. stay in `engine.rs`.
    Learning never calls arm/disarm.
  - `delete(port, index)`, `forget(target)` (all ports; built-ins → copy).
- `profile.rs`: `Profile.encoders: [{kind, channel, number, encoding}]` is
  now a typed field (T-204 had written it into the built-in APC layouts,
  kept in `extra`; its unit test now reads the typed field), optional and
  kept out of the JSON when empty; `encoding` defaults to two's complement.
  `ProfileStore::edit_slug`.
- `api.rs`: `POST /api/midi/learn {target, args?, shift?}` (404 unknown,
  403 arm / not external, 409 `--no-midi`, 400 bad grid slot),
  `POST /api/midi/learn/cancel`, `POST /api/midi/learn/confirm {replace}`,
  `POST /api/midi/mapping/delete {port, index}`,
  `POST /api/midi/mapping/forget {target}` → `{removed}`. `GET /api/midi`
  adds `learn` (target, label, shift, remaining_ms), `learn_conflict`,
  `learned` (last mapping, numbered), `learn_notice` (last message,
  numbered) and `mappings` (per device: message, channel, target, French
  label, mode, Shift, profile, index).
- `engine.rs`: calls learn (`capture` / `observe`) in `handle_batch`,
  `expire` in `frame`; `seed_pickup`; `profile_of` / `is_shift_key` shared.
- `index.html`:
  - every mappable control has `data-control="<id>"` (sliders, buttons,
    seg groups, tempo bar, layers, arm/estop, look/audio controls; cue
    buttons get their `grid.p.r.c`).
  - Right click on any `[data-control]` → menu « Apprendre MIDI »,
    « Apprendre avec Shift », « Oublier MIDI » (with its current messages).
    Cues keep their « Propriétés du cue » menu, which gains the same three
    entries.
  - « Apprendre MIDI » / « Terminer » button in the Contrôleur section:
    controls get a dashed green outline, mapped ones a solid one plus a
    pill (« CC 7 · can. 1 ») on the control or its label; a click picks the
    target instead of using the control.
  - Green banner under the top bar: waiting text (« … touchez un bouton, un
    potard ou un fader de votre contrôleur… (Échap pour annuler) » +
    Annuler), the replace question (Oui / Non), then the result or the
    message (timeout, refusal). `/api/midi` is polled every 250 ms while
    learning, 2 s otherwise.
  - **Échap** still triggers the emergency stop first, then cancels learning.
  - « Affectations » list in the Contrôleur section: message, cible, mode,
    « Supprimer ».

## Testing
- `cargo test -p laser-studio` (rebased on develop eeb0932, with T-204's
  layouts): **437 passed** (+2 in the second test binary), 2 ignored (CoreMIDI
  virtual ports, unchanged). New: 13 in `learn.rs` (incl. learning on the real APC40 mkII layout: the
  perso copy keeps its 103 mappings, Shift key and encoders; a layout fader
  asks before replacing) (fader → absolute +
  pickup seeded + perso copy + built-in untouched; buttons per target kind,
  grid cell, alias, pitch bend, program change, the learned press doesn't
  act; known encoder and repeated-step encoder → relative, a step value
  waits for the next message, a fader moving on → absolute, a button target
  doesn't wait; not learned: clock / Start / Note Off / Shift key / still CC /
  SysEx, note on a fader target; conflict No / Yes / same target; 15 s
  expiry and cancel; arm refused without the option, Shift-only, never arms,
  learning while armed keeps it armed, option turned off while waiting;
  Shift learn without a Shift key; bad requests; delete / forget / alias /
  `-perso-2` / reload from disk; `/api/midi` fields), 1 in `api.rs`
  (routes, status codes).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `tests/midi-learn.spec.ts` (5 tests, `--no-midi --midi-test`,
  bytes injected through `/api/midi/inject`; the simulated device starts on
  the built-in T-204 layout, later tests empty its perso copy first): right
  click « Taille maître » → learn → device knob 1 (CC 16, free in the
  layout) drives the size (API + slider), listed in « Affectations »,
  `apc40-mk2-perso.json` on disk, still works after a restart; encoder →
  relative, conflict « Non » keeps / « Oui » replaces; « Oublier MIDI »,
  « Supprimer », Échap cancels and latches the e-stop; learn mode → click a
  cue → pad plays it, pill shown; `transport.arm` refused (banner + 403),
  with the option on learned as a Shift mapping without arming (button held
  1.2 s), learning while armed keeps it armed. `--repeat-each 5`: 25/25.
- Full `npm --prefix studio/e2e test` after the rebase on eeb0932: **110 passed** (105 existing + 5 new).
- Rebase on T-204: its layouts win in `studio/profiles/*.json` (my own
  `encoders` lines dropped, theirs are equivalent).
- Earlier rebase: `index.html` conflicts with develop (cue « Show » selector, hold-to-run
  keys in the Escape handler) resolved by keeping both; Escape keeps develop's
  `shiftMod` and cancels learning after the stop.

## Risks / for the reviewer
1. **Blackout first still wins while learning**: the batch's blackout pass
   runs before `capture`, so learning another target on the Stop All pad
   blacks out once (then asks to replace). Deliberate: safety over
   convenience.
2. The captured message is consumed, but its release (Note Off / CC 0) goes
   to the engine with the new mapping in place: harmless for every mode
   (releases act only on held buttons), covered by the "press is taken" test.
3. "CC that doesn't move" = same value as the last one seen from that CC
   (a map of last CC values per port, always kept, bounded by 16×128 per
   port). A CC button that only ever sends 127 learns fine on button
   targets; on a fader target its second press is read as an encoder
   (relative). Rare; the mapping can be deleted and there is no mode
   editor in the UI yet (T-211 territory).
4. Unknown encoders are guessed from two identical small values; an
   encoder with acceleration (1, 3, 2…) keeps learning waiting until a
   value repeats, and a sign-bit encoder (65 = −1) is taken as offset 64
   (65 = +1): direction reversed. Profiles can declare `encoders` to be
   exact (the APC ones do).
5. Absolute mappings leave `min`/`max` empty (= the control's range at run
   time) rather than copying the bounds, so they follow a registry change.
6. The e2e spec restarts its studio once (persistence check).
7. On hardware: check the APC40 mkII's Cue Level / Tempo knobs really send
   two's complement on CC 47 / 13 in mode 0x41 (Akai's public protocol says
   so).

## Review
