# feat/e2e — T-004 Playwright click tests

## What / why
Automatic proof that the UI really works, click by click, so UI changes
(and the laser-safety keys) can't silently break. `studio/e2e/` is a
Playwright project (headless Chromium) that drives the real `index.html`
against a real studio and asserts on what the user sees plus
`/api/state`, `/api/frame`, `/api/live` and `/api/control-values`.

- `studio.ts` — the harness. `global-setup.ts` runs
  `cargo build -p laser-studio`; each spec file then starts
  `target/debug/laser-studio` on a free port (never 8080) with a fresh
  temporary `--data-dir` (deleted afterwards), **never `--device`**, and
  checks the instance reports no output and starts disarmed. It is stopped
  with SIGINT (the studio's Ctrl+C path), SIGKILL after 5 s. `restart()`
  keeps the data dir to test persistence.
- 7 spec files, 53 tests:
  - `content` — start look, shape buttons, text typed, Effet tab and
    generator select, look size / brightness / colour / rotation sliders
    → state and frame (extent, colour, brightness, motion).
  - `laser` — LASER button and Space toggle `armed` (button text, badge);
    Space ignored while typing; Escape ends with `armed=false` after the
    button, after Space, while typing, with a slider focused, when already
    off, after a Space burst; Space after clicking the button toggles once;
    a restarted studio comes back disarmed.
  - `cues` — one tab per category, cue click plays it (and the Effet panel
    follows), cue keeps the look brightness, page tab + cue click plays
    that page's cue (and `cue_page` follows), AZERTY keys (`z`, `q`) play
    the matching cue of the current page, keys ignored while typing, a page
    or grid cell changed through the API shows up in the UI.
  - `live` (« Direct ») — rotation presets Stop/Moyen/Rapide
    (`rot_speed[2]`, highlight, the drawing spins), Synchro tempo keeps the
    step (90 °/s ↔ 1 turn/bar), master size 150 % (`/api/live`,
    `master.size` in control-values, frame extent ×1.5), position X,
    master brightness, Réinitialiser, Inverser held by mouse and by `<`,
    a live change from the API moves the slider.
  - `tempo` — BPM field and beat dots, **Enter** taps (5 real key presses
    400 ms apart → 140–160 BPM, source `tap`), **Backspace** puts now on
    the one (at 40 BPM, pressed on beat 3), both ignored in the BPM field,
    Tap / Sync 1 buttons, ×2 / ÷2. The test checks the page is visible
    (a hidden tab throttles timers).
  - `scenes` — empty hint and name required, save → play → delete,
    playlist advances on its own (0 → 1 → 0, look follows, row
    highlighted) and Stop ends it, a manual change takes over.
  - `persistence` — calibration sliders move the output and survive a
    restart (API, `calibration.json`, sliders after reload); output stays
    in -1..1 with calibration ×2 and master size 200 %; scenes and
    `live.json` survive a restart.
- No fixed sleeps to wait for results: everything polls
  (`expect.poll`, web assertions). Fixed waits only where timing is the
  subject (tap spacing) or to prove something did *not* happen.
- `package.json` (`npm test`), `README.md`, `.gitignore` (node_modules,
  test-results, playwright-report, local browser dirs). No application
  code changed.

## How to run
```sh
npm --prefix studio/e2e install
npm --prefix studio/e2e run install-browser   # Chromium, once
npm --prefix studio/e2e test
```
See `studio/e2e/README.md` for single files, `--headed`, traces.

## Results
- `npm --prefix studio/e2e test`: **49 passed, 4 skipped** (`test.fixme`,
  the bugs below), ~14 s with 4 workers (plus the cargo build).
- Flakiness check: `--repeat-each 5` → 245 passed, 0 failed.
- Each `test.fixme` was run as `test.fail` to confirm it reproduces the
  bug (T-290 5/5 with a slowed `/api/arm`).
- `cargo test -p laser-studio`: 71 passed.
- No temp data dir or studio process left behind after a run.

## Bugs found
Filed as tasks, each with a ready `test.fixme('T-29x: …')` to flip on:

1. **T-290 (safety, P2)** — the laser toggle (button / Space) computes
   `!armed` from the page's local copy, updated only when the POST answers
   and overwritten by older `/api/frame` replies. Two quick Space presses
   send `on:true` twice, so "on, off" leaves the laser **on**. Escape is
   unaffected (always `on:false`).
2. **T-291 (ui, P1)** — every `<input>` counts as "typing": after moving a
   slider or ticking « Synchro tempo », **Space no longer turns the laser
   off**, and cue keys / Enter / Backspace are dead until the user clicks
   elsewhere. Escape still works.
3. **T-292 (cues, P2)** — the server never clears `active_cue` when the
   look is changed by hand, by a scene or by the playlist; after a reload
   (and for future MIDI LED feedback) the old cue still shows as playing.
4. **T-293 (ui, P1)** — the page doesn't follow look changes it didn't
   make (playlist, API/MIDI cues): the Contenu/Apparence controls show the
   old look, and the next slider move posts that stale look back — during
   a playlist the content on stage jumps to the pre-playlist look.

## Risks / notes
- Playwright 1.63 needs its own Chromium build (~95 MB, shared cache
  `~/Library/Caches/ms-playwright`); `npm run install-browser` fetches it.
- Tests within a file share one studio; `studio.reset()` restores the
  start-up look, live modifiers, playlist and arm state before each test.
- The tempo tests take ~5 s (real tap spacing, a 40 BPM bar).

## Review
