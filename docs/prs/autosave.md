# PR: autosave and crash recovery (T-287)

Branch: `feat/autosave` · Task: `tasks/T-287-infra.md`

## What / why

If the studio crashes or the power goes in the middle of a prep session,
the work must not be lost. In BEYOND, scripted autosave pauses the
output. Ours never touches the engine.

- `studio/src/autosave.rs` (new): a thread of its own (`autosave::spawn`)
  looks at the open project every 0.5 s. When it has unsaved changes
  (fingerprint ≠ saved fingerprint) and nothing changed for `delay_s`
  (default 30 s, debounced), it writes
  `<data-dir>/autosave/<nom>-<AAAAMMJJ-HHMMSS>.lsproj` and deletes the
  oldest beyond `keep` (20) **for that project only**. The stamp is UTC,
  so the order never jumps with daylight saving time; the UI shows local
  time. Clean shutdown (Ctrl+C / SIGTERM) writes one last autosave if
  something is unsaved (`main` waits at most 3 s for it).
- Engine safety: the `Shared` lock is held only for `project::snapshot`
  (the same copy *Enregistrer* already does). Timelines are read from
  disk, the JSON is built and the file written (`write_atomic`: `.tmp`,
  fsync, rename, folder fsync) with the lock released.
- Failure: any write error (folder not writable, disk full) is logged,
  stored as `autosave.error` and retried after the delay. The studio keeps
  running. UI: orange *⚠ Échec de la sauvegarde auto* in the status line
  (the error in its tooltip).
- Recovery: at startup (`project::startup` → `autosave::find_offer`), the
  newest autosave of the open project (*Sans titre* when unsaved) is
  offered if it is newer than the project file, was not ignored before,
  and differs from **both** the saved project and the working copy. So a
  clean restart whose work is already in the working copy (scenes, cues…)
  asks nothing; a crash that loses something (e.g. the tempo, which only
  lives in the project, or a corrupt working file) asks
  *Récupérer le travail non enregistré ?*
  - *Récupérer* (`POST /api/project/recover`): same checks and swap as
    *Ouvrir* (`read`, `check_lfos`, `apply`, `persist`), but the project
    name and saved fingerprint stay, so it shows as modified (« • »).
    Never arms, never touches calibration, safety, MIDI safety or the gate.
  - *Ignorer* (`POST /api/project/recover-ignore`): keeps the working copy
    and writes the file name to `autosave/.ignored`, so that autosave is
    not offered again. Autosave files are never deleted by this.
  - While the offer is pending, autosave keeps watching but does not
    write, so a newer copy can't bury the work to recover. Opening, saving
    or creating a project drops the offer.
- Settings: `AutosaveSettings { enabled: true, delay_s: 30, keep: 20 }` in
  `<data-dir>/prefs.json` (app preferences, not the project; unknown keys
  kept). `delay_s` clamped to 10–300 s (1–300 with `--test-hooks`, for the
  e2e tests), `keep` to 1–200. `POST /api/project/autosave`
  `{enabled?, delay_s?, keep?}`. `GET /api/project` gains `autosave`
  (settings, `last_ok_ms`, `error`, `offer`, folder).
- UI (French): *Sauvegarde auto 12:04* next to the project name; RÉGLAGES ›
  *Sauvegarde auto* (on/off, delay in s); the recovery dialog
  (*Récupérer* / *Ignorer*). Styles reuse the theme tokens (`--muted`,
  `--warn`, `.modal`, `.primary`).
- `main.rs`: the engine now also writes `live.json` / `layers.json` at
  clean shutdown when they are dirty (they were flushed at most once a
  second, so the last second of moves was lost on quit). Without this a
  clean restart right after a MIDI move would offer a recovery.
- `project.rs`: `civil()` factored out of `utc_now()`, `write_project_stamped`,
  `ProjectState::{autosave, saved_fingerprint, extra}`, `path_of` public.

## Tests

- Unit (`autosave::tests`, 11): debounce on a simulated clock (quiet
  period, restart on change, retry after failure, saved state, project
  name change); stamp format and parsing (incl. names with `-` and
  accents); ring rotation (20 kept, other projects untouched, hidden
  `.tmp` ignored); newest by stamp; a change autosaved only after the
  delay and never more than `keep` files; disabled / saved → nothing
  written, changes made while off are written once back on; unwritable
  folder (a file in its place) → error, no panic, API still answers;
  prefs clamping and unknown keys kept; crash → offer of the newest,
  autosave paused while pending, *Récupérer* restores and stays disarmed,
  autosave resumes after; no offer when the working copy already has it,
  after *Ignorer*, or when the project was saved later; a stand-in engine
  thread taking the lock in a loop never waits ≥ 16.7 ms while autosaves
  of 500 scenes run.
- e2e (`studio/e2e/tests/autosave.spec.ts`, 5; delay 1 s via seeded
  `prefs.json` + `--test-hooks`): change → file in `autosave/` + status
  line; SIGKILL then restart → dialog, *Récupérer* restores the tempo,
  « • », disarmed; *Ignorer* keeps the working copy and isn't asked again;
  RÉGLAGES toggles and delay saved to `prefs.json`; unwritable folder →
  orange warning, engine still drawing. `studio.ts` gains `kill()`
  (SIGKILL, preview-only studio).
- `cargo build`, `cargo test -p laser-studio` (706 passed, 7 ignored, + 2 + 2
  integration), `cargo clippy --all-targets -D warnings` clean,
  `npx playwright test --workers=2`: 188 passed.
  Note: under heavy machine load (other agents building, load avg ~15) a
  few runs had 1–3 timing flakes in unrelated specs (openUi's 5 s wait,
  presence's 2.5 s poll); they pass on rerun, and a develop-binary run
  under the same load also flaked (tempo Tap). The final run on a calmer
  machine was fully green.

## Risks

- A spec that changes the tempo (only kept in the project) and then
  restarts the studio will now get the recovery dialog over the page
  (clean shutdown autosaves it). None does today; such a test should
  answer the dialog or save first.
- *Sans titre* autosaves share the name with a real project called
  « Sans titre » (the one T-286 imports on first start); the fingerprint
  checks keep a wrong offer unlikely, and recovery is always a choice.
- The thread copies the project twice a second (same cost as the UI's
  2 s `GET /api/project`). Very large projects make this copy longer; the
  engine-wait test bounds it for 500 scenes in a debug build.
- Autosave file stamps have 1 s resolution: two autosaves in the same
  second overwrite each other (same project, newer content wins).

## Review
