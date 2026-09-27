# feat/arming — T-250 arming interlocks + T-251 latched emergency stop

## What / why
Arming was a bare `Shared.armed` boolean that any handler could set. Later
safety tasks (operator presence, watchdog, checklist, audience mode, sky
mask) each need to veto arming, and the operator needs to see why the laser
went dark. Escape only disarmed, and one keypress could turn the laser
straight back on.

- `studio/src/interlock.rs` (new):
  - `ArmGate` is now the **only** place where the armed state changes
    (`Shared.armed` is gone; `grep "armed ="` only matches the gate).
    `request_arm(src)` succeeds only if every interlock is `ok`. Otherwise
    it returns the French labels of the blocking interlocks. It **always
    refuses** `ArmSource::Midi` and `ArmSource::System`, so a controller
    can never arm, even if it gets past the controls layer.
    `disarm(reason, src)` is always accepted and records
    `(reason, source, time)`. `set_interlock(id, false)` while armed
    disarms at once, with reason `Interlock(label)` and source `System`.
    Interlocks that were never registered still block. The gate starts
    disarmed with reason `Démarrage`.
  - `EStop`: the latched emergency stop, made of atomics only (latched,
    source, time) plus an optional **kill switch**. `trip()` never takes
    the `Shared` lock. It calls the kill switch, which `DacOutput`
    implements with laser-dac's thread-safe `StreamControl::disarm()`, so
    the DAC is disarmed straight from the HTTP thread without waiting for
    the engine. `ArmGate::sync_estop` mirrors the latch into the gate
    (disarm with reason `EStop`, `estop` interlock open).
    `ArmGate::reset_estop` records any trip the gate has not seen yet,
    then releases the latch. It never re-arms.
- `output.rs`: `OutputStage::emit` is the last stage of the pipeline. It
  re-reads the e-stop latch lock-free at send time, so a stop that arrives
  while a frame is rendering still blanks that frame. It sends a
  fully blanked frame (positions kept) whenever the output is not armed,
  and it reports how many lit points were sent. `Output::kill_switch()`
  is a default method that returns `None`.
- `main.rs`: `Shared.gate` and `Shared.estop` (an `Arc` shared with the
  web fast path) replace `armed`, and `Shared.output_lit` is new. The
  engine syncs the latch into the gate at the top of every frame and goes
  through `OutputStage`. The preview frame (`Shared.frame`) stays
  unblanked. A hidden `--test-interlock` flag adds an interlock that is
  never satisfied, for refusal tests.
- `web.rs`: the receiving thread handles `POST /api/estop` **itself**,
  ahead of every queued request. It never reads or validates the body
  (`?source=keyboard|ui` is optional), trips the latch, and syncs the gate
  only if the lock is free (`try_lock`). All other requests go, in order,
  to one worker thread, so the existing handlers keep their serial
  semantics. New endpoints: `GET /api/arm` →
  `{armed, since, source, source_fr, last_disarm: {reason, reason_fr, source, source_fr, at}, blocking, estop}`
  (times in ms since the epoch). `POST /api/arm {on, source?}`: `on:false`
  is always accepted, whatever other fields the body has; `on:true`
  returns 409 `{blocking: [...]}` when refused. `POST /api/estop/reset`
  never arms. `/api/state` and `/api/frame` keep `armed` and add `estop`
  and `arm`, and `/api/frame` adds `output_lit`.
- Controls: `transport.blackout` and the new `safety.estop` (external,
  trigger) both **latch** the e-stop, with source `midi` when they come
  from a controller. As T-208 asks, the controller's blackout pad becomes
  the stop button. `transport.arm` (still not external) goes through
  `request_arm`, and a refusal returns 403. `docs/controls.md` was
  regenerated.
- UI: a round red **ARRÊT** button in the sticky top bar
  (`z-index` above the menus), and a full red banner
  « ARRÊT D'URGENCE — réinitialiser pour réarmer » with a
  « Réinitialiser l'arrêt d'urgence » button. Under LASER ON/OFF, a status
  line reads « Désarmé — raison : Arrêt d'urgence (clavier), 21:42:10 » or
  « Armé depuis 3 min 12 s (clavier) ». A refused arm shows a red bubble:
  « Armement impossible : » followed by the list of blocking interlocks.
  **Escape** is still handled first, whatever has focus. It now sends
  `/api/estop` (no JSON, no await before the request leaves) and updates
  the display immediately. **Shift+Escape** is a plain disarm without the
  latch. **Space** is unchanged, apart from sending `source: keyboard`.

## Adapted / left out
- `since` is a `SystemTime` (ms since the epoch) rather than an `Instant`,
  because the UI shows it as a time of day or a duration.
  `Shared.estop_at` lives in the lock-free `EStop` (`state()`), not in
  `Shared`, so the fast path never needs the lock.
- No e2e test: `studio/e2e/` is not on this branch (a129422). The same
  flows are covered at HTTP level by the `web.rs` tests, and by hand below.
- No `--no-midi` flag here, because MIDI isn't on this branch. When
  merging with `feat/midi-core`, map its blackout action onto
  `transport.blackout` / `safety.estop`, and make sure no MIDI path calls
  `ArmGate::request_arm` with a source other than `Midi` (the gate refuses
  `Midi`).
- The heartbeat, watchdog, checklist and so on only use the mechanism
  added here (their own tasks).

## Testing
- 133 unit tests, all passing.
  - `interlock.rs`: starts disarmed with reason `Démarrage`; source and
    reason recorded; transition table (interlock ok/not × armed
    before/not); an interlock that drops disarms at once with its label
    and does not re-arm when it comes back; an unregistered failing
    interlock blocks; the test interlock; MIDI and system can never arm;
    disarm always accepted; `gate()` blanks every colour but keeps
    positions; e-stop disarms, latches and blocks arming; reset (even
    before a sync) keeps the reason, releases the latch and never arms; a
    second trip keeps the first source and time; the kill switch fires on
    every trip; lenient source parsing.
  - `output.rs`: with a probe output, a disarmed stage sends only blank
    points; an armed stage arms once; **an e-stop tripped after the gate
    was read as armed blanks the very next emitted frame** and disarms
    the output; preview-only mode counts `lit`.
  - `web.rs`: a real tiny_http server on port 0 (localhost), no engine.
    Covers arm and read-back; 409 with the label from the test interlock;
    `on:false` with extra fields and an unknown source; `source:"midi"`
    over HTTP cannot arm; e-stop with no body and with invalid JSON → 200,
    latched, arm refused with 409, reset does not arm, re-arm works; **an
    e-stop is served and latched while the test thread holds the `Shared`
    lock** (a queued arm stays blocked behind it and is then refused);
    `/api/frame` keeps the preview but reports `output_lit: 0`;
    fast-path classification.
  - `controls.rs`: a controller cannot arm; a controller blackout latches;
    `safety.estop` from MIDI latches with source `midi`; **every external
    control, driven to 0 and 1, leaves the laser disarmed**; the UI arm
    toggle off is a plain disarm.
- `cargo clippy -p laser-studio --all-targets -- -D warnings` is clean.
  The UI script parses (`node` + `new Function`).
- Throwaway instance (`--port 8097 --data-dir <scratchpad>`, no
  `--device`), driven with curl: armed → `output_lit` 127; `/api/estop`
  with no body → 200, `armed=false`, `estop=true`, `output_lit` 0, the
  preview still had 127 points; arm → 409 « Arrêt d'urgence enclenché »;
  reset → still disarmed, reason `estop`; arm → 200; `/api/control
  transport.arm` → 403; `transport.blackout` → latched, source MIDI.
  `--test-interlock` → 409 « Verrou de test (--test-interlock) ».
- **Not clicked through in a browser.** The banner, bubble, status line,
  Escape and Shift+Escape were checked only by reading the code and
  parsing the script.

## Risks
- Behaviour change: Escape now **latches**. After Escape, Space no longer
  re-arms until « Réinitialiser l'arrêt d'urgence » is pressed. Operators
  used to Escape/Space for rehearsals should use Shift+Escape.
- Behaviour change: a controller's `transport.blackout` now latches too.
- HTTP now uses two threads (receiver and worker). Handlers still run one
  at a time, in order. tiny_http serialises requests on the **same**
  keep-alive connection, so an e-stop pipelined behind a slow request on
  that one connection waits for it. Browsers usually open several
  connections, but this is not a hard guarantee.
- If the engine thread is stuck (T-253), the DAC-level kill switch still
  disarms laser-dac's stream. Per laser-dac's docs, that is software
  blanking in its stream loop for Ether Dream/IDN (no hardware shutter).
  A software e-stop is not a hardware e-stop (see T-258).
- The banner may flicker for one poll right after Escape, until the server
  reports the latch (at most one engine frame).

## Review
