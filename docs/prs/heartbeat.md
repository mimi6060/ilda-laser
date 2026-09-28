# feat/heartbeat — T-252 operator presence + T-253 engine watchdog and clean shutdown

## What / why
The engine runs on the server and the operator sits at the browser. If every
tab closes or freezes, nobody can press Escape, but the laser stayed armed. A
flash held in a page that died stayed on (the known issue in
`docs/prs/cue-modes.md`). An engine stall, a panic or Ctrl+C could leave the
DAC repeating its last frame. None of the code added here can arm the laser.

### T-252 — operator presence (`studio/src/presence.rs`, new)
- **Heartbeat**: every page sends `POST /api/heartbeat {client_id, visible, hold}`
  every 500 ms. The beat comes from a Blob `Worker`, whose timers are not
  throttled like a background tab's. The worker only beats while the page's
  main thread answers its ping (within 1.5 s), so a hung page stops vouching
  for itself. On `pagehide` the page sends a `sendBeacon` with `gone: true`,
  so closing the last page counts at once. An arm request that carries
  `client_id` also counts as a beat.
- **`ui_alive` interlock** (`interlock::UI_ALIVE`, registered in `main`):
  - At least one page beat within `ui_timeout_ms` (default 2000, clamped to
    1000..10000). Otherwise:
    - if armed → `gate.disarm(UiLost, System)`, shown as « Interface perdue »;
    - arming is refused (409 « Aucune interface ouverte (battement perdu) »).
  - On the alive → lost transition, every **held flash/solo is released**
    (`controls::release_held` → `CueDeck::release_all_held`, through
    `with_deck` so the look stays consistent). Latched cues keep playing.
  - While disarmed nothing else happens.
- **Hold-to-run** (`PresenceSettings.hold_to_run`, default off; saved in
  `presence.json`):
  - The output sends dark frames unless the hold key is held. The default
    key is `ShiftRight` and can be changed; Space and Escape are refused. The
    `safety.hold` control (momentary, for a MIDI pad) also holds.
  - Releasing makes the frames dark at once, **without disarming**
    (`OutputStage::emit(…, hold_ok, …)` keeps the DAC armed). Pressing again
    resumes. After `hold_release_disarm_s` (10 s) released while armed →
    disarm, reason `hold_released` « Maintien relâché trop longtemps ».
  - A page's hold counts for at most 1 s after its last beat. The page sends
    a beat immediately on key down and up, on blur and on visibility change.
  - If every page is `hidden` for more than 5 s, the output is dark even
    while held.
  - A MIDI hold is released when its port closes.
- `Shared::sync_safety(now)` is the single place that applies e-stop +
  watchdog trip + presence to the gate. The engine calls it every frame, and
  `request_arm` calls it before arming. Presence is only *enforced* in the
  real studio (`Presence::enforced`); unit tests built from
  `test_support::shared()` are unaffected.
- API: `GET/POST /api/presence` (settings, clamped; the reply is what was
  kept). `/api/state` and `/api/frame` gain `presence` and `engine_ok`.
- UI:
  - Near the arm button: an « Opérateur présent / absent » LED and a
    « Moteur » LED, which turns red with « Moteur bloqué — laser coupé »
    after a stall.
  - In hold mode, a large banner: « MAINTENIR « Maj droite » POUR ÉMETTRE »
    in amber, or « ÉMISSION — maintien actif » in green.
  - Sécurité panel: « Couper si l'interface ne répond plus (ms) », « Mode
    maintien (homme mort) », and the hold key with a « Changer… » capture
    button.
  - When the hold key is a Shift key, it no longer counts as the Shift
    modifier: Shift+cue flash and Shift+Escape (plain disarm) only react to
    the *other* Shift. So Escape while holding still latches the e-stop.

### T-253 — watchdog and clean shutdown (`studio/src/watchdog.rs`, new)
- `EngineHealth` holds atomics only: last tick, armed-as-of-tick, tripped.
  The engine ticks at the top of every frame, under the lock, after
  `sync_safety`.
- A `watchdog` thread polls every 10 ms. If there has been no tick for more
  than `stall_ms` (100) while armed, it calls `EStop::kill_output()` (the
  DAC kill switch, **without** latching) and raises a trip. The watchdog
  never takes the `Shared` lock. The engine's output stage blanks the
  stalled frame (`armed && !health.is_tripped()`). The next `sync_safety`
  records `disarm(EngineStall)` « Moteur bloqué ». The operator can re-arm
  (it is not an e-stop).
- Panic hook: `watchdog::on_panic` fires the kill switch, marks the trip and
  sets `running = false`, then the default hook runs. If the engine thread
  itself panics, unwinding drops `OutputStage`, whose `Drop` sends
  `blank_now()` and then `set_armed(false)`.
- Ctrl+C **and SIGTERM** (`ctrlc` `termination` feature):
  1. The kill switch fires at once and `running` becomes false.
  2. The engine records `disarm(Shutdown)`, then `OutputStage::shutdown()`
     does `set_armed(false)`, sends 3× `blank_now()`, and closes (drops) the
     output.
  3. `main` waits at most 2 s for the engine, then exits anyway.
- `Output::blank_now()` is a new trait method. Its default sends one blanked
  point; `DacOutput` implements it.
- `startup_state()` was extracted from `main` so that "no startup path is
  armed" can be tested.
- Hidden test-only flags:
  - `--test-output <file>` (`FileLogOutput`, no laser; conflicts with
    `--device`): logs `arm/disarm/lit/dark/blank/close`.
  - `--test-hooks`: enables `POST /api/test/stall?ms=N` (≤ 2000), which makes
    the engine sleep while *holding the lock*, the worst case.

## Testing
- `cargo test -p laser-studio`: **392 unit tests + 2 subprocess tests**,
  green. `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- Unit tests, with simulated time (explicit `Instant` / ms values):
  - `presence.rs`: the timeout edge (1999 / 2000 ms); background-throttled
    beats; two pages with one closed; per-page ageing; `touch`; clamping
    (timeout ≤ 10 000, and a hand-edited file too); reserved or invalid hold
    keys; hold off; release → dark without disarm; press → resume; the 10 s
    release limit (and reset by a press, and never while disarmed); a page
    that dies with the key down; all pages hidden for 5 s; a bounded client
    table.
  - `watchdog.rs`: trips once, strictly after `stall_ms`; no trip while
    disarmed; regular ticks never trip; arming between ticks is watched; the
    real thread fires the kill switch without latching; `on_panic`.
  - `output.rs` (probe output):
    - hold released → dark frames, output still armed;
    - shutdown order `disarm, blank×3`, then nothing after the close;
    - **an engine thread that panics → `blank` then `disarm`**;
    - the default `blank_now`;
    - the `FileLogOutput` log.
  - `main.rs` (Shared):
    - no page → no arming;
    - lost heartbeat → `ui_lost` « Interface perdue », held flash released,
      no re-arm when the page returns;
    - a latched cue survives;
    - two pages, one closed → stays armed;
    - hold-to-run dark, then disarm after 10 s released;
    - watchdog trip → `engine_stall`, and re-arming works;
    - presence sync never arms (fuzz-ish loop);
    - **`no_startup_path_is_armed`**: no CLI option id contains "arm" or
      "emit"; data-dir files stuffed with `"armed": true`; four flag
      combinations → disarmed, reason `startup`, arm refused without a page;
      `--device` + `--test-output` is rejected.
  - `web.rs`: heartbeat → arm → `gone` → `ui_lost`; `client_id` on
    `/api/arm`; presence settings clamped and reported, never arming; the
    stall hook is 404 without `--test-hooks` and bounded.
  - `interlock.rs`: `kill_output` fires the switch without latching.
- `studio/tests/shutdown.rs` runs the real binary (`--no-midi`, free port,
  temp data dir, `--test-output`). It beats, arms, waits for `lit`, then
  sends **SIGTERM** (and, in a second test, **SIGINT**). It checks a clean
  exit and that the log ends with `disarm, blank, blank, blank, close`.
- e2e (`npm --prefix studio/e2e test`): **96 passed**. Added:
  - `presence.spec.ts` (8):
    - the LED shows the operator as present;
    - **closing the only page → disarmed in < 2.5 s**, reason « Interface
      perdue », and arming via the API is then refused;
    - **a hung page** (main thread stuck, so the worker loses its pong) →
      still armed at first, disarmed after the timeout;
    - two pages, one closed → still armed 3 s later, then disarmed when the
      second closes;
    - **a flash held in a page that hangs is released** by the server;
    - hold-to-run: `output_lit` is 0 when released while the preview still
      draws, > 0 while `ShiftRight` is held, released → dark and still
      armed, and released 3 s → `hold_released`;
    - Escape with the hold key down still latches the e-stop;
    - the Sécurité panel: 60000 → 10000, and the hold mode checkbox.
  - `watchdog.spec.ts` (2, studio with `--test-hooks`): a 200 ms stall →
    `engine_stall` « Moteur bloqué », the LED reads « Moteur bloqué — laser
    coupé », and re-arming works; a stall while disarmed changes nothing.
  - The harness gets a `testHooks` option.
  - `beam3d.spec.ts` counts `blob:<studio>/…` (the page's own heartbeat
    worker) as local, not as a request leaving the studio.
- The existing specs keep pages open while they arm, so their heartbeats
  flow. None arms through the API without a page.

## Risks / for the reviewer
- **Behaviour change**:
  - Arming needs at least one open UI page. `curl /api/arm` alone gets 409,
    and so does the MIDI opt-in arm without a page.
  - Closing or reloading the last tab disarms at once (the `gone` beacon).
- A hung page only stops beating once the worker gives up (1.5 s) plus the
  timeout (2 s), so it takes about 3.5 s in the worst case. Background
  timers are covered by the worker. If Chrome freezes a background tab
  (hidden for several minutes, when it is eligible), that page stops
  beating and the laser disarms. This is intended, but operators should
  keep the studio in a visible window. Only tested in Chromium, **not in
  Safari**.
- Machine sleep: `Instant` does not advance while macOS sleeps, so the
  timeout does not count the sleep itself. The page must still beat after
  wake.
- Watchdog `stall_ms` = 100 is fixed. A debug build under heavy load could
  in theory trip falsely. That fails safe (disarm with « Moteur bloqué »),
  and it never happened in the e2e runs.
- The kill switch after a stall disarms the DAC directly. `OutputStage`
  re-syncs its own `output_armed` on the next frames. A software watchdog is
  not a hardware interlock (see T-258).
- The panic hook stops the whole studio on any panic, in any thread. This
  is deliberate: a panic poisons `Shared` anyway.
- `safety.hold` from `/api/control` or MIDI is trusted like any control. It
  can only let an **already armed** laser emit, and is dropped when its MIDI
  port closes. Flashes held **by MIDI** are still not released on unplug
  (unchanged; out of scope).
- Rebased onto `develop` a1e7bdd (timeline, T-160). `main.rs` conflicted and
  was merged by hand: `startup_state()` now also builds `evolving`,
  `timeline` and `shows`, and the engine loop keeps the timeline rendering
  with `sync_safety`/`hold_ok`/`health` added. Please check that merge
  (`git diff develop -- studio/src/main.rs`).
- The Sécurité panel now holds both the strobe limiter (T-101, from develop)
  and the presence settings.

## Review

Reviewed by the architect (integrator). Clean merge on a1e7bdd; engine
loop checked by hand: timeline events + deck cues → layers mix → live →
calibration → safety (strobe limiter) → output gate, with presence and
watchdog syncing the gate each frame (disarm/block only). 392 unit + 2
signal tests, e2e 96/96 twice, clippy clean. Accepted behaviour changes
(told the user): arming needs at least one open UI page; closing the last
tab disarms; hold-to-run is opt-in. Follow-ups: Safari check; release
MIDI-held flashes when a controller is unplugged.
Verdict: APPROVED
