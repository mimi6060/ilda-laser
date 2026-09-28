# Laser Studio — end-to-end tests

Playwright tests that click through the real UI in headless Chromium and
check what the user sees plus the studio's API (`/api/state`,
`/api/frame`, `/api/live`, `/api/control-values`).

## Safety

The harness (`studio.ts`) only ever starts a **preview-only** studio:

- never with `--device` (no laser output; it also checks `output` is null),
- always with `--no-midi` (never opens the user's MIDI controller); MIDI
  specs add `--midi-test`, which plugs in a *simulated* APC40 mkII
  (« Test APC40 mkII ») fed by `POST /api/midi/inject` — still no real port,
- always with `--no-audio` (never opens the Mac's microphone or an audio
  interface; audio specs use the browser source, `POST /api/audio`),
- on a free port, never 8080 (the user's instance),
- with a fresh temporary `--data-dir` per spec file, deleted afterwards —
  never the user's `studio-data/`,
- stopped with Ctrl+C (SIGINT) after the file, disarmed first.

## Run

```sh
# once: install the runner and Chromium (browsers go to the shared
# Playwright cache, ~/Library/Caches/ms-playwright)
npm --prefix studio/e2e install
npm --prefix studio/e2e run install-browser

# every time (builds the studio with cargo first)
npm --prefix studio/e2e test
```

Useful options (from `studio/e2e/`):

```sh
npx playwright test tests/cues.spec.ts      # one file
npx playwright test -g "Escape"             # tests whose title matches
npx playwright test --headed                # watch it click
npx playwright test --repeat-each 5         # hunt for flakiness
npx playwright show-report                  # HTML report of the last run
LASER_STUDIO_SKIP_BUILD=1 npx playwright test   # reuse target/debug/laser-studio
LASER_STUDIO_BIN=/path/to/laser-studio npx playwright test
```

Failures keep a screenshot and a trace in `test-results/`
(`npx playwright show-trace <trace.zip>`).

## Layout

- `playwright.config.ts` — headless Chromium, 4 workers (one studio per spec file).
- `global-setup.ts` — `cargo build -p laser-studio` before the run.
- `studio.ts` — the harness: `useStudio({ midiTest?, files? })`, `openUi()`,
  `focusPage()`, API helpers and frame maths (`extent`, `centroid`).
- `midi.ts` — the simulated APC40 mkII (`--midi-test`): `new Apc(studio)`
  with `pressPad(r, c)`, `moveFader(n, v)`, `turnKnob(cc, delta)`,
  `shift(down)`, `ledAt(r, c)` (from `GET /api/midi/sent`), and
  `testProfileFiles()`, a test layout seeded into the data dir.
- `tests/` — one file per area: `content`, `laser`, `cues`, `live`,
  `tempo`, `scenes`, `persistence`, `midi`, …

## Writing tests

- Tests in one file share one studio and run in order; start each test
  from a known state (`studio.reset()` in `beforeEach`).
- Never sleep to wait for a result: poll with `expect.poll(...)` or a web
  assertion (`toHaveText`, `toHaveClass`…). A fixed wait is only OK when
  the timing *is* the thing tested (tap tempo) or to prove that something
  did *not* happen.
- Keyboard shortcuts: call `focusPage(page)` first so no input has focus,
  and use real `page.keyboard` presses in the visible page.
- A test that documents a known bug is `test.fixme('T-xxx: …')` with the
  task id; turn it back into `test` when the task is fixed.
