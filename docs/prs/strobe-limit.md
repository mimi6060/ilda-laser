# feat/strobe-limit — T-101 strobe limiter and beam horizon

## What / why
Festival looks strobe a lot (T-100 beat gates, flash cues, audio flash,
brightness LFOs) and put beams just above the crowd. Photosensitivity
practice (docs/research/festival-looks.md §4.4) says: sustained flashing
at or below ~4 Hz, faster bursts ≤ 5 s. No preset, cue, layer or live move
should be able to exceed that, however it is programmed.

- `studio/src/safety.rs` (new):
  - `StrobeLimiter` looks only at the **light that comes out**: the mean
    drive level of the frame (average of each point's brightest channel,
    blanked points = 0), i.e. optical power at a fixed pps. It does not
    care what made the flash (gate, LFO, audio flash, cue, chase, layer
    dimmer). On/off with hysteresis against a decaying peak (on ≥ 60 %,
    off ≤ 30 %); every off → on edge is a flash.
  - "Too fast": the last k flash onsets (k = 3..8) span less than
    (k−1)/max_hz − 34 ms (two frames of slack, so a 4 Hz strobe sampled
    with frame jitter is never cut; 4.17 Hz — the 250 BPM beat gate —
    is caught after 6 onsets).
  - A burst starts at the first onset of the too-fast run. After
    `strobe_burst_s` (5 s) the output is **held steady**: dark/dim frames
    are replaced by the last lit frame. The hold lasts at least
    `strobe_cooldown_s` (2 s) **and** until the input has not been fast
    for 2 s — so a sustained 8 Hz strobe gives 5 s of strobe then steady
    light until it stops, not 5 s on / 2 s off forever. A pause shorter
    than the cooldown does not reset the burst clock (no 4.9 s + gap
    evasion). If the source stays dark longer than one allowed period
    (0.25 s) during a hold (look stopped), the output goes dark: at most
    one slow flash.
  - `blank_low_beams`: provisional horizon until T-003. A **beam** is a
    run of ≥ 6 coincident lit points (generator dots dwell 12, the "dots"
    shape 6, outline corners / line ends only 4), within 2e-3 — a figure
    shrunk to a point counts too. Beams below `beam_floor_y` (default
    0.0) are blanked, positions kept. Outlines, text, waves untouched.
  - `apply()` = horizon → limiter → horizon again (a held frame can
    predate a floor change). Idempotent.
  - `SafetySettings { strobe_max_hz: 4, strobe_burst_s: 5,
    strobe_cooldown_s: 2, beam_floor_y: 0 }` — global, `serde(default)`,
    in `safety.json` via `SafetyStore`. **Tighten-only**: max_hz
    0.5..4, burst 0..5, cooldown 2..30 (looser → 400 with a French
    message; a hand-edited file is clamped on load). The floor is
    -1..1; lowering it below 0 is the explicit operator setting.
- `main.rs` `run_engine`: layers mix → live → calibration →
  **`safety::apply`** → `OutputStage::emit` (gate). The preview frame
  (`Shared.frame`) is the limited one, so the preview/3D view shows what
  the laser would draw. `Shared.safety`, `Shared.strobe` (status).
- `web.rs`: `GET /api/safety` → `{settings, defaults, status}`,
  `POST /api/safety` (400 when looser/invalid), `/api/state.safety`,
  `/api/frame.strobe = {active, fast, rate_hz, burst_s, beams_blanked}`.
- UI: « Sécurité » panel (above Calibration): « Strobe max (Hz) »
  (0.5–4), « Rafale max (s) » (0–5), « Horizon des faisceaux » (-1..1),
  and a « Limiteur actif » light in the panel header (visible even when
  folded) plus a one-line status (rate, burst seconds, beams blanked).

### Deviation from the task text
- The task says "before calibration"; the architect asked for it **after
  calibration, right before the output gate** so nothing can bypass it.
  Consequence: `beam_floor_y` is in **output** coordinates (after
  offset/scale/rotation), i.e. where the beam really goes. With default
  calibration it's identical.
- The cooldown is exposed in the API but not in the UI (the task lists
  three controls); it can only be lengthened.

## Testing
- `cargo test -p laser-studio`: **299 passed** (2 ignored, pre-existing).
  New in `safety.rs` (19) with synthetic 60 fps on/off sequences:
  8 Hz cut at 5 s ± 0.1 s; 17 Hz at 20 % duty cut at 5 s ± 0.1 s; 2/3/4 Hz
  at 10 % and 50 % duty never limited over 30 s and output == input;
  4 Hz with 10–26 ms random frame jitter never limited; 250 BPM gate
  (4.17 Hz) is limited; hold lasts as long as the fast input and releases
  after the cooldown, then a new burst is allowed; a 1 s pause doesn't
  reset the burst; stopping the look during a hold goes dark within
  0.25 s; tighter settings cut sooner; a 100 %↔50 % shimmer is not a
  flash; validation/clamping/serde defaults; store round-trip. Horizon:
  beam_fan at y < 0 → no lit point; above → untouched; beam_circle keeps
  only its upper beams; shapes/text/wave have no beams; "dots" loses its
  lower 8 dots (of 16); a figure scaled to 0 is a beam; T-100 beat gate
  through the real `Animator` at 8 gates/s → steady after 5 s.
  `web.rs`: `/api/safety` tighten-only over HTTP, `/api/frame.strobe`.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e (`studio/e2e/tests/safety.spec.ts`, 4 tests): 8 Hz strobe (square
  LFO on master brightness) flashes, then held steady (every frame lit
  for 1.5 s, LED on), released after it stops, stays disarmed; 3 Hz
  never limited for 7 s; no lit beam below y = 0 in `/api/frame`, back
  after lowering the floor; panel shows defaults, sliders stop at 4 Hz
  / 5 s, tightening persists after reload, looser → 400.
  Full suite: **71/71** on the rebased branch (develop 37d4e54), repeated
  several times. `live.spec.ts:43` (« Synchro tempo keeps the preset
  step ») failed 2 times out of ~13 full runs under parallel load and
  passes 10/10 alone; it doesn't touch strobe or beams — reviewer may
  want to check it on develop.
- Manual, preview only (`--port 8097`, scratch `--data-dir`,
  `--no-midi`, no `--device`): with the 8 Hz LFO, `/api/frame.strobe`
  showed `fast` from ~0.35 s with rate ≈ 8 Hz, `burst_s` tracking wall
  time, `active` from 5.1 s.

## Risks
- **Visible change**: with the default floor 0.0, beams in the lower half
  disappear (half of « Cône » / beam_circle, the lower half of the « Points »
  shape, fans with negative y). That's the task's intended default; the
  operator lowers « Horizon des faisceaux » explicitly.
- Energy-based detection counts any ≥ 2× luminance swing as a flash,
  e.g. a very fast fill/empty chase could be held steady after 5 s.
  Conservative by design. Conversely a fast shimmer that never drops
  below 30 % of peak isn't counted.
- While held, dark frames are replaced by the last lit frame: motion
  "stutters" instead of flashing, and the held frame is a real look (a
  static beam look stays a static beam — T-256 dwell guard is separate).
- Beam detection is heuristic (≥ 6 coincident lit points). A future
  generator that draws beams with < 6 dwell points would escape the
  floor; T-003 replaces this with real zones.
- The limiter uses engine wall time; a very stalled engine (> 34 ms
  jitter on every frame) could make a 4 Hz strobe look slightly faster.
- `/api/frame` gains a `strobe` object; the old fields are unchanged.
  Arming, e-stop and the gate are untouched (the stage sits before
  `OutputStage::emit`).

## Review
