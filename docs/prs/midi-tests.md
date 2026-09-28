# feat/midi-tests — T-209 MIDI testing without hardware

## What / why
Agents and CI have no APC40, and must never touch the user's. This adds a
simulated APC40 (mkII first, the user owns one) so detection, mappings,
LEDs and the T-208 safety rules can be tested end to end: in unit tests,
over real CoreMIDI virtual ports (opt-in), and from Playwright through an
HTTP injection route that only exists in a test-only mode.

- `studio/src/midi/testing.rs` (new):
  - `FakeApc { model, received, faders }` for `Apc40Mk2` and `Apc40`:
    builds the bytes of a pad (`pad(row, col, down)`, row 0 at the top;
    mkII notes 0–39 on channel 0, APC40 notes 0x35–0x39 with channel =
    column), a button, Shift (0x62), Stop All (0x51), a fader (CC 7 on
    channel 0–7, master CC 14), a relative encoder (two's complement).
    `receive(bytes)` records what the studio sent and answers the Device
    Inquiry (product `0x29` / `0x73`) and, for the mkII, the Introduction
    with the `0x61` fader-position message. `led_at(row, col)` /
    `pads()` / `mode()` read the LEDs and Introduction mode back. History
    capped at 4096 messages.
  - `SimMidi`: a `Backend` (same trait as CoreMIDI's) hosting fake
    devices; `inject(port, bytes)` delivers bytes to the studio's open
    input exactly as the CoreMIDI callback would (decode → channel →
    worker). No CoreMIDI call anywhere.
- `--midi-test` (hidden CLI flag, `main.rs`): clap-enforced to need
  `--no-midi` and to conflict with `--device`. Plugs in a simulated
  « Test APC40 mkII » and runs the real MIDI worker on `SimMidi`.
  `MidiState.sim` holds it; `None` in every normal run.
- `midi/api.rs`: `POST /api/midi/inject {port?, bytes: [..]}` (1–1024
  bytes, default port = the first simulated device; unknown port 404,
  disabled port 409) and `GET /api/midi/sent` (`devices: [{port, model,
  mode, sent, pads}]`). Both match only when `MidiState.sim` is set, so
  without `--midi-test` they fall through to the plain 404. `GET
  /api/midi` gains `test: bool`.
- `midi/backend.rs`: real studios no longer list (so never open) CoreMIDI
  ports named `Laser Studio Test…` — the virtual ports our own CoreMIDI
  tests create, which are visible to every app on the Mac. The ignored
  tests use the new `MidirBackend::all_ports()`.
- `midi/worker.rs`: `spawn_on(backend, …)` (generic); `spawn` = CoreMIDI.
  The ignored `midi_virtual_apc40_mk2_is_detected_over_coremidi` now
  plays the device with `FakeApc` and also checks the `0x61` reply and a
  LED sent through `MidiSender` reaching the pad.
- e2e: `studio.ts` gets `useStudio({ midiTest?, files? })` (files are
  seeded into the fresh data dir before the first start; the harness still
  always passes `--no-midi` and refuses `--device` / port 8080).
  `midi.ts`: `Apc` with `pressPad`, `releasePad`, `button`, `shift`,
  `moveFader`, `turnKnob`, `ledAt`, `device`, and `testProfileFiles()` (a
  T-204-like test layout: 40 grid pads, Stop All → blackout, scene 1 →
  page 1 / Shift → page 2, track fader 1 → master size with pickup, master
  fader → brightness, Cue Level encoder → position X, Shift + 0x5B → arm).
  `tests/midi.spec.ts` (6 tests):
  1. detected by inquiry as APC40 mkII, profile applied, Introduction 0x41,
     UI top bar « MIDI : APC40 mkII connecté »;
  2. grid pad (0,1) plays page 1's 2nd cue (API + highlighted in the UI),
     a second press stops it;
  3. fader pickup: not caught below the value, caught within 3 %, then
     drives it; re-armed after an API change, caught again by crossing;
     encoder steps position X up and down;
  4. Stop All → disarmed, e-stop latched with source `midi`, banner shown,
     `/api/arm` refused (409) until « Réinitialiser »;
  5. Shift layer: Shift + scene 1 = page 2 (tab highlighted), grid pad
     then plays page 2's cue, scene 1 alone = page 1;
  6. nothing arms by default: past the 5 s plug guard, Shift + arm button
     held 1.5 s, then every note and CC on 16 channels with Shift held
     (Stop All left out, a blackout would hide an arming), then the arm
     button held again → still disarmed, no e-stop. Positive control
     (preview only): with `allow_arm` on, the same gesture arms — so the
     refusal came from the option — then disarmed and the option reset.
- `studio/e2e/README.md`: `--midi-test` and `midi.ts` documented.

## Safety
- Injection can't do more than a real controller: it enters before the
  decoder, so every T-208 rule (blackout first, opt-in Shift + 1 s arming
  with the 5 s plug guard and the interlock gate, brightness cap/pickup)
  applies. Unit tests `nothing_injected_arms_by_default` and
  `opt_in_gesture_still_arms_through_the_simulator` and e2e test 6 cover
  it.
- The routes don't exist without `--midi-test`; `--midi-test` can't be
  combined with `--device` (preview only) and needs `--no-midi` (no real
  port is ever opened alongside it). Unit test on the CLI parser.
- The lock order stays Shared → sim (the inject route runs under the
  `Shared` lock and the sim callback only pushes to a channel); the worker
  never holds the sim lock while taking `Shared`.

## Testing
- `cargo test -p laser-studio` (rebased on develop 82353c0): **317 passed**, 2 ignored
  (CoreMIDI virtual ports). New: 14 in `testing.rs` (byte layouts mkII /
  APC40, inquiry replies, 0x61 after the Introduction, LEDs per pad,
  bounded history, mkII and APC40 detected through the worker, pad → grid
  cue + LED via `MidiSender`, Stop All → e-stop, Shift layer + pickup,
  injection errors, nothing arms by default + opt-in positive control),
  2 in `api.rs` (routes absent without the sim; inject/sent, bad input
  400, non-simulated port 404, disabled port 409), 1 in `backend.rs`
  (test ports hidden), 1 in `main.rs` (CLI rules).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- `npm --prefix studio/e2e test`: **77 passed** (71 existing + 6 new), ~27 s;
  `midi.spec.ts` also passed `--repeat-each 3` (18/18).
- **Not run**: `cargo test -p laser-studio -- --ignored midi_virtual`. A
  studio was running on port 8080 at the time; an older build doesn't
  have the « Laser Studio Test » filter, so it would have opened the
  virtual APC and received its pad press. They compile and should be run
  once no other studio is running.

## Risks / for the reviewer
1. The e2e LED check is a placeholder: the studio sends no pad LEDs yet
   (T-205), so test 2 asserts `ledAt(0, 1) === null`. T-205 must switch it
   to the expected colour. The LED path itself (sender → device →
   `led_at`) is unit-tested.
2. `MidiState.enabled` is `true` under `--midi-test` (the UI shows the
   simulated device as connected); no CoreMIDI client is created.
3. `testing.rs` is compiled into the release binary (the runtime needs
   it for `--midi-test`); it's small and inert without the flag.
4. The worker unit tests keep their own `FakeBackend`; `SimMidi` is the
   device-level simulator. Merging the two was left out to keep the diff
   small.
5. Filtering `Laser Studio Test…` ports means a user can't name a real
   device like that — harmless.
6. e2e test 6 waits up to ~5.5 s after the studio started (T-208 plug
   guard) and holds buttons for 1.5 s twice: the timing is what's tested.

## Review

Reviewed by the architect (integrator). Clean merge on 82353c0; 317 unit
tests, clippy clean, e2e 77/77 three runs in a row. Checked the injection
gates: `--midi-test` requires `--no-midi` and conflicts with `--device`,
routes 404 without it, bytes go through the normal decoder → mapping →
T-208 rules → arming gate. Real studios ignore our own virtual test ports.
LED criterion stays open until T-205. Also fixed here T-294: the flaky
« Synchro tempo » test was a real UI race (a frame fetched before a live
change was answered re-drew old values over the controls); live controls
now ignore such frames, 180/180 on --repeat-each 20.
Verdict: APPROVED
