# feat/midi-map — T-202 MIDI mapping engine + T-208 MIDI safety

## What / why
The APC40 (and any controller) can now drive the studio: a profile's
mappings turn MIDI messages into control ids (T-145), through the same
`controls::apply(…, from_external = true)` as `/api/control`. T-208's
rules make sure a bumped controller can't hurt anyone.

- `studio/src/midi/mapping.rs` (new): typed `Mapping` / `MidiInput`
  (`note | cc | pitch_bend | program_change`, `channel: null` = any) /
  `MapMode` (`trigger | toggle | momentary | absolute | relative | grid`) /
  `Curve` (`linear | log`) / `RelEncoding` (`twos_complement` (APC,
  default) | `sign_bit` | `offset64`, the three common encoder formats, for
  T-211). Pure helpers: curve + inverse, encoder decoding, pickup rule
  (within 3 % or crossed since the last position).
- `studio/src/midi/profile.rs`: `Profile.mappings` is now `Vec<Mapping>`
  (was raw JSON), `Profile.shift_key: Option<MidiInput>`,
  `DevicesFile.safety: MidiSafety` (in `devices.json`), `set_safety`.
- `studio/src/midi/engine.rs` (new): the runtime (`MidiState.map`).
  - `handle_batch(shared, events)` replaces the empty `midi::handle` hook;
    the worker calls it once per batch under the lock.
  - Shift: the profile's `shift_key` is consumed; while held, `shift: true`
    mappings are searched first, then the normal ones.
  - Buttons (trigger / toggle / momentary / grid) act on the press edge (a
    CC button repeating 127 fires once). What a release does is decided at
    press time, so a flash pad releases the right cue even if Shift or the
    cue page changed while it was held. Program Change = press without
    release.
  - `grid`: `args.slot` 0–39 → `grid.<current page>.<row>.<col>`; an empty
    slot does nothing (no warning).
  - `absolute`: 0–127 (14-bit pitch bend) → mapping `min..max` (default:
    the control's range; choices = option index), linear or log.
  - `relative`: signed steps × `step` (default 1/127 of the range, 1 for
    choices), clamped to `min..max`.
  - Pickup per (port, control): ignored until the fader is within 3 % of
    the value or crosses it; re-armed when the value was changed elsewhere
    (UI, reset, other control: > 1 % drift from what we wrote). APC40 mkII
    positions from the `0x61` reply seed the "previous position".
  - Coalescing: absolute/relative writes are queued (last value per
    control) and applied by `engine::frame`, called by the engine thread
    once per frame → at most one write per control per frame (100 CCs in =
    1 `apply`).
  - Unknown target / bad grid args: ignored, logged once, listed in
    `/api/midi` `errors`. Never a panic.
- `studio/src/midi/safety.rs` (new) + engine, T-208:
  - **Blackout first**: a first pass over each batch (following Shift
    presses within the batch) applies any press that maps to
    `transport.blackout` before anything else; it also cancels an arming
    in progress.
  - **Arming from MIDI**: `controls::apply` still refuses `transport.arm`
    externally. The only way is the opt-in gesture: `allow_arm` (false by
    default, `devices.json`), a mapping to `transport.arm`, **Shift held**,
    button held **1 s** (checked by `frame`, cancelled if the button or
    Shift is released, the port closes, or a blackout comes), and the press
    must come **≥ 5 s after the controller was connected**. At completion
    it re-checks Shift, the held button, the device still connected and
    the option still on, then sets `armed = true` (logged as a warning).
  - **Brightness** (`master.brightness` / `live.brightness` /
    `look.brightness`) from MIDI is capped at `safety::BRIGHTNESS_MAX`
    (1.0 until T-003 adds the global maximum) and always uses pickup, even
    if the profile says `pickup: false`.
  - **Disconnect**: the device gets `lost: true` (red banner « Contrôleur
    MIDI déconnecté » in the UI) until it comes back; its Shift / held /
    pickup state is dropped. Option `blackout_on_disconnect` (false by
    default). Disabling a port by hand is not a loss (no banner, no
    blackout).
- `studio/src/midi/api.rs`: `GET /api/midi` adds `safety` and
  `devices[].lost`; `POST /api/midi/safety {allow_arm?, blackout_on_disconnect?}`
  (partial update).
- `studio/src/midi/worker.rs`: calls `handle_batch`; sets `lost` and calls
  `engine::port_closed` on unplug / disable.
- `studio/src/main.rs`: `midi::engine::frame(&mut s, now)` in the engine
  loop's locked section (a no-op when nothing is pending).
- `studio/src/index.html`: « Sécurité » box in the Contrôleur section with
  the two checkboxes; red banner under the top bar when a controller is
  lost.

Built-in profiles still have no mappings (T-204 writes the APC layout).

## Testing
- `cargo test -p laser-studio`: **187 passed, 2 ignored** (CoreMIDI
  virtual-port tests, unchanged). New tests (37):
  - mapping.rs: encoders (1, 63, 64, 127 and the two other encodings),
    curves (bounds, log, inverse), pickup rule (window, crossing up/down),
    channel `null` vs fixed, message reduction, `Mapping` JSON round trip +
    defaults + bad mode rejected.
  - engine.rs: absolute on next frame + bounds + log, range override,
    100 CCs → 1 write, relative bounded, other encodings + choice steps,
    pickup (crossing both ways, 3 % window, re-pickup after a UI change),
    Shift + fallback, Shift vs plain press, channel as key, trigger edge,
    toggle, momentary release, grid follows the page, flash cue stops on
    release after a page change, unknown ids warned once, unmapped/realtime
    harmless, pitch bend / program change. Safety: [pad, fader, blackout]
    ends disarmed; blackout is the first `apply` of its batch; Shift +
    Stop All in one batch is not a blackout; **exhaustive default refusal**
    (every note/CC/program on 16 channels, with and without Shift, held
    5 s); opt-in: 0.9 s nothing, released early nothing, 1.1 s arms; Shift
    released cancels; 5 s plug guard; unplug cancels arming; blackout on
    disconnect opt-in; master fader at 127 leaves brightness alone; mkII
    fader positions seed pickup; brightness cap (range override 0..5,
    relative, toggle); every absolute mapping of the built-in profiles has
    pickup (guards T-204).
  - worker.rs (fake CoreMIDI): unplug sets `lost`, replug clears it,
    opt-in blackout on disconnect, disabling isn't a loss; bytes from a
    fake port → blackout + fader written at the frame.
  - api.rs: safety defaults off, partial updates, bad JSON 400, never arms.
  - profile.rs: safety options survive a reload; typed `shift_key`.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- UI script parses (`new Function` over the `<script>`).
- Ran the studio with `--no-midi --port 8100 --data-dir <scratch>`:
  `/api/midi` shows `safety` (both false), `POST /api/midi/safety` saves to
  `midi/devices.json`, `armed` stays false. Not run without `--no-midi`.

## Risks / for the reviewer
1. **MIDI arming path** (`engine::frame`) sets `s.armed = true` directly,
   not through `controls::apply` (which refuses `transport.arm` from
   outside, by design). It's the only place; check its gates: option on,
   Shift + 1 s hold, ≥ 5 s after connect, still connected. Tests cover
   each.
2. Coalesced writes land at the next engine frame, so button actions in a
   batch are applied before fader values of the same batch.
3. Pickup drift threshold (1 % of the range) means a UI change smaller
   than that keeps the fader engaged.
4. Blackout is applied twice in a batch (first pass, then in order);
   harmless (idempotent).
5. Not done here: T-203's refusal to *learn* the arm target when the option
   is off (T-203 must check `store.devices.safety.allow_arm`); the e2e test
   with `--midi-test` needs T-209's injection route; « Shift + flash » on
   the grid (T-204) needs a flash override in `controls` (not in the id
   scheme yet).
6. On hardware (APC40 mkII): Shift is note `0x62` once T-204 writes it in
   the profile; check the `0x61` fader reply seeds pickup correctly.

## Review

Reviewed by the architect (integrator). Merged after arming (T-250/251):
the branch wrote `s.armed = true` directly, which no longer exists. MIDI
opt-in arming now goes through `Shared::request_arm_midi_opt_in` →
`ArmGate::request_arm_midi_opt_in`: the option is re-checked, and every
interlock plus the latched e-stop still apply (new unit test); plain
`request_arm(Midi)` stays refused. MIDI blackout goes through
`transport.blackout`, which latches the e-stop since T-251. Tests moved to
the gate API. 249 unit + 53 e2e green, clippy clean. UI: kept both the
e-stop banner and the MIDI-lost banner inside the sticky top bar.
Verdict: APPROVED
