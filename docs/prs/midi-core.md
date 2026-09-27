# feat/midi-core — T-200 native MIDI I/O + T-201 controller detection and profiles

## What / why
The base for every MIDI task (T-202 to T-211): controllers are handled in the
Rust process (not Web MIDI), so the laser keeps answering the APC40 with the
browser tab closed.

- **New dependency**: `midir = "0.11"` in `studio/Cargo.toml` only.
  Licence **MIT** (midir 0.11.0, and its macOS backend crates `coremidi`
  0.9.2 / `coremidi-sys` 3.2.1 are MIT too; checked in their Cargo.toml
  2026-09-27). It's a code dependency, not a content file, so
  `docs/CONTENT_SOURCES.md` is unchanged.
- `studio/src/midi/` (new):
  - `decode.rs`: pure, stateful decoder bytes → `MidiMsg` (Note On/Off,
    Note On vel 0 = Off, CC, 14-bit pitch bend, Program Change (for T-211),
    SysEx incl. split over packets, Clock/Start/Stop/Continue, running
    status; truncated/garbage input dropped, never panics).
  - `detect.rs`: Device Inquiry `F0 7E 7F 06 01 F7`, reply parsing (Akai
    `0x73` APC40, `0x29` APC40 mkII, `0x28` APC mini; three-byte
    manufacturer ids handled so e.g. Novation's product byte can't read as
    an APC), port-name hints, Introduction bytes
    `F0 47 7F <pid> 60 00 04 <mode> 01 00 00 F7`, mkII `0x61` fader-position
    reply, and "all LEDs off" messages per model.
  - `profile.rs`: `Profile` (`version`, `name`, `driver`
    generic/apc40/apc40mk2, `match {port_contains, product_id}`, `host_mode`
    default `0x41`, `mappings` kept as raw JSON until T-202 types them,
    unknown fields preserved via `extra`), `ProfileStore` (built-ins via
    `include_str!` from `studio/profiles/*.json`, never written; user
    profiles in `<data-dir>/midi/profiles/<slug>.json`; `devices.json` =
    port → profile/enabled; editing a built-in saves `<slug>-perso` and
    assigns it to the port). Choice order: saved preference → product id of
    the detected model → longest `port_contains` match → `generic`. Broken
    files → French error in `/api/midi`, fallback `generic`, no panic.
  - `backend.rs`: `Backend` trait (list ports, open input with a callback,
    open output). `MidirBackend` = CoreMIDI; a fresh client per listing /
    connection so hot-plugged devices show up. Duplicate port names get
    " (2)".
  - `worker.rs`: the "midi" thread. The CoreMIDI callback only decodes and
    pushes a `MidiEvent` into an `mpsc` channel (**no lock**). The worker
    drains it, takes the `Shared` lock once per batch, records `last` /
    `recent` (20; MIDI clock not recorded so it doesn't flush the monitor)
    and calls `midi::handle(&mut Shared, &MidiEvent)` — the empty hook for
    T-202. Re-scans every 2 s (open new ports, drop vanished ones, keep them
    listed as disconnected), sends the Device Inquiry on connect, waits
    ≤ 500 ms then falls back to the port name, applies the profile and sends
    the Introduction (`host_mode`, default `0x41`) to APC drivers. UI changes
    (enabled / profile) are picked up within 250 ms. On quit (Ctrl-C), on
    disable, or when switching an APC to a non-APC profile: LEDs off then
    Introduction `0x40` (device back in Generic mode). CoreMIDI calls never
    happen under the `Shared` lock; lock poisoning is tolerated.
  - `MidiSender` in `Shared.midi.sender`: `send(port, bytes)` from any
    thread, non-blocking (queued to the worker, which owns the connections).
  - `api.rs`: routes as a pure function of `Shared` (tested without HTTP):
    `GET /api/midi` → `{enabled, devices:[{name,input,output,connected,
    enabled,model,model_label,profile,profile_error,faders}], last, recent,
    errors}`, `GET /api/midi/profiles`, `POST /api/midi/profile {port,
    profile}` (`null`/`""` = automatic, unknown → 404),
    `POST /api/midi/device {port, enabled}`.
- `main.rs`: `--no-midi` flag (no thread, no port opened, `enabled:false`),
  `Shared.midi`, joins the MIDI thread at exit so the APC is released.
  `web.rs`: one match arm forwarding `/api/midi*`.
- `index.html`: "MIDI : APC40 mkII connecté" / "MIDI : aucun contrôleur" /
  "MIDI : désactivé" in the top bar (polled every 2 s); a collapsible
  "Contrôleur" section listing devices with "Modèle détecté", "Activé",
  "Profil" and "Réinitialiser le profil".
- Built-in profiles `studio/profiles/{apc40,apc40-mk2,generic}.json`: our
  own, with empty mappings (T-204 writes the layout).

Not done here (by design): mapping engine (T-202), learn (T-203), APC layout
(T-204), LED feedback (T-205), MIDI test mode / e2e injection (T-209).

## Safety
- Nothing in the MIDI path touches arming or the laser output; the hook is
  empty and T-202 must go through `controls::apply(…, from_external: true)`
  (which refuses `transport.arm`). Everything sent to controllers is
  Device Inquiry, Introduction, and LED-off Note messages.
- No device present, CoreMIDI unavailable, open failures or unplugging
  mid-run: logged / shown in `/api/midi.errors`, retried on the next scan,
  never a panic. The engine thread never waits on MIDI I/O.

## Testing
- `cargo test -p laser-studio`: **116 passed, 2 ignored** (45 new MIDI
  tests): decoder (all message kinds, vel 0, 14-bit bend, split/aborted
  SysEx, truncation, running status, real-time inside SysEx); inquiry
  replies 0x73/0x29/0x28/unknown/truncated/3-byte manufacturer; port-name
  hints; exact Introduction bytes per model/mode; 0x61 parsing; profile
  JSON round trip, minimal profile defaults, invalid profiles, choice rules,
  `-perso` copy surviving a reload, corrupt files → readable errors +
  generic; worker with a fake CoreMIDI backend: mkII identified by reply
  (neutral port name), silent APC40 → name fallback after 500 ms, unknown
  device → generic with no APC SysEx, unplug/replug within one 2 s scan,
  disabled ports not opened, disabling/profile change releases the APC
  (LEDs off + 0x40), shutdown goodbye, sender, backend failures; API routes.
- `cargo test -p laser-studio -- --ignored midi_virtual`: **2 passed** on
  real CoreMIDI with virtual ports only (backend filtered so no real device
  is opened): a fake "Laser Studio Test APC40 mkII" answering the inquiry is
  identified, introduced with 0x41, a pad Note On is received, and it gets
  0x40 at shutdown; a virtual port appearing/disappearing is seen by the
  next listing.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- Ran the studio with `--port 8096 --data-dir <scratch> --no-midi`:
  `/api/midi` → `enabled:false`, profiles listed, profile POST saved to
  `midi/devices.json`, unknown profile → 404. UI script passes
  `node --check`. The studio was **not** run without `--no-midi` (to avoid
  grabbing the user's APC40).

## Risks / to check on the real APC40 mkII
1. Port name as CoreMIDI shows it (expected "APC40 mkII"), and that input and
   output share it (the output is opened by the same name).
2. The Device Inquiry reply arrives within 500 ms with product byte `0x29`
   (`/api/midi` → `model: "Apc40Mk2"`, profile `apc40-mk2`).
3. After the Introduction 0x41, pads stop lighting by themselves when
   pressed, Track Select buttons send notes.
4. The `0x61` reply format (length 9, fader values) — check
   `devices[].faders` moves with the faders at connect time.
5. Ctrl-C: all pad/button LEDs go dark and the device returns to Generic
   mode (knob rings aren't explicitly cleared).
6. Unplug / replug: back as connected within ≤ 3 s, no preview stutter.
7. A second studio instance without `--no-midi` would also take the device
   (CoreMIDI allows several readers) — tests/e2e must pass `--no-midi`
   (T-209 adds it to the e2e launcher; there is no `studio/e2e/` yet).
- `midi::handle` is called under the `Shared` lock: T-202 must keep it cheap
  and panic-free.

## Review
