# Laser Studio — end-to-end tests

Playwright tests that click through the real UI in headless Chromium and
check what the user sees plus the studio's API (`/api/state`,
`/api/frame`, `/api/live`, `/api/control-values`).

## Safety

The harness (`studio.ts`) only ever starts a **preview-only** studio:

- never with `--device` (no laser output). Arming needs a connected output
  (T-401), so by default it starts with the fake one, `--test-output` (a
  call log next to the data dir, never a laser), and checks that this is
  the output; `useStudio({ output: 'none' })` starts with none,
- always with `--test-devices` (a scan returns a fixed list, never the
  network or USB, and « Connecter » can only open fake outputs),
- always with `--no-midi` (never opens the user's MIDI controller); MIDI
  specs add `--midi-test`, which plugs in a *simulated* APC40 mkII
  (« Test APC40 mkII ») fed by `POST /api/midi/inject` — still no real port,
- always with `--no-audio` (never opens the Mac's microphone or an audio
  interface; audio specs use the browser source, `POST /api/audio`),
- on a free port, never 8080 (the user's instance),
- with a fresh temporary `--data-dir` per spec file, deleted afterwards —
  never the user's `studio-data/`,
- stopped with Ctrl+C (SIGINT) after the file, disarmed first,
- never left behind (T-358): started with the hidden `--exit-with-parent
  <worker PID>`, it shuts down cleanly by itself within a second if its
  Playwright worker dies or its data dir is deleted, and
  `global-teardown.ts` stops any studio of the run still alive.

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
- `global-setup.ts` — `cargo build -p laser-studio` before the run, and
  the file where the run's studios record their PIDs.
- `global-teardown.ts` — stops this run's leftover studios (T-358).
- `studio.ts` — the harness: `useStudio({ midiTest?, files? })`, `openUi()`,
  `focusPage()`, API helpers and frame maths (`extent`, `centroid`).
- `midi.ts` — the simulated APC40 mkII (`--midi-test`): `new Apc(studio)`
  with `pressPad(r, c)`, `moveFader(n, v)`, `turnKnob(cc, delta)`,
  `shift(down)`, `ledAt(r, c)` (from `GET /api/midi/sent`), and
  `testProfileFiles()`, a test layout seeded into the data dir.
- `tests/` — one file per area: `content`, `laser`, `cues`, `live`,
  `tempo`, `scenes`, `persistence`, `midi`, …
- `soak/` — the endurance soak (`operator.ts`, the long `soak.spec.ts`),
  a Playwright project of its own that `npm test` never runs.

## Endurance soak (T-482)

A scripted operator clicks through the real UI for a long time, driven by
a seeded generator: every workspace, LIVE panel and CRÉATION tab, cue
pages and cues, slider drags (latest wins, T-484), tap tempo and BPM, the
timeline examples, dialogs and popovers (closed by their button or by
Échap), project « Enregistrer sous » / « Ouvrir », a second tab coming
and going, and explicit arm → Échap → reset cycles on the fake output.
The warm-up starts with a tour of everything once (panels build their
content on first use). It checks:

- no page error and no crash;
- `armed` only inside an arm window the test opened, and the fake
  output's log: never more arms than asked, never lit while disarmed;
- Échap always disarms (the worst delay is reported) and closes what it
  closes; the reset clears the banner;
- the studio stays responsive (`/api/state` polled every 250 ms) and so
  does the page (long tasks, an animation-frame probe);
- the engine's frame times (`/api/perf`, T-448): in a release build no
  frame over 50 ms after the warm-up, each late frame logged with the
  operator's step;
- memory: the studio's `footprint`, the page's JS heap after a forced
  garbage collection, its DOM nodes and listeners, the studio's open
  files. Compared by floors (the lowest value of the first third of the
  samples after the warm-up against the last third).

```sh
# the long run: release build, 120 min by default, report in docs/qa/
npm --prefix studio/e2e run soak
SOAK_MINUTES=20 SOAK_SEED=7 npm --prefix studio/e2e run soak
```

`npm run soak` keeps the Mac awake (`caffeinate -dims`): a Mac left
alone goes to sleep, which freezes the studio and the browser alike. A
run during which everything stood still for more than 30 s (a laptop
can still doze off in « Deep Idle ») fails and says so in its report.

Options: `SOAK_MINUTES` (120), `SOAK_SEED` (482; a failure replays with
the seed printed in the report), `SOAK_WARMUP_MINUTES` (5),
`SOAK_SAMPLE_SECONDS` (60), `SOAK_REPORT_DIR` (default `docs/qa/` when
the checkout has a `docs/` folder, else the test's output folder). The
report, `soak-<date>.md`, is in French: verdict, criteria, late frames,
the memory curve.

`tests/soak.spec.ts` runs the same operator for 2 minutes in the normal
suite (debug build: frames only bounded against a stall, 250 ms; memory
bounds looser), so the scenario itself keeps working.

At night: `studio/e2e/soak.sh` (same thing, for a scheduler). With
launchd, a `~/Library/LaunchAgents/<label>.plist` whose
`ProgramArguments` is the script's absolute path and whose
`StartCalendarInterval` is the hour; or a crontab line such as
`0 3 * * * /path/to/ilda-laser/studio/e2e/soak.sh`. It never uses
`--device`, port 8080 or `studio-data/`, like every spec here.

## Safety invariants (T-264)

The output safety rules are also checked as property tests in Rust, run
by every `cargo test`: `studio/src/safety_invariants.rs` drives the real
engine frame (`Engine::tick`) on a simulated clock with random cues,
looks, LFOs, audio, MIDI, timeline, patterns, zones, caps and
arm/e-stop actions, and checks every frame sent (never lit while disarmed
or e-stopped, never above the caps, nothing in a Blank zone or below a
full horizon, coordinates in -1..1, nothing arms but an explicit
request); the API side is `no_api_request_arms_except_an_explicit_arm_request`
in `web.rs`. A failure prints its seed: replay it with
`SAFETY_SEED=<n> cargo test -p laser-studio <test name>`, or run more
cases with `SAFETY_CASES=<n>`.

When a new protection lands, add its invariant there (see the module
docs: randomise its settings in `random_case`, exercise its controls in
`random_action`, assert it in `check_points` or a dedicated test), check
that removing the protection makes the test fail, and add an e2e spec
here only for what the user sees (messages, buttons), like
`arm-refused.spec.ts`.

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
