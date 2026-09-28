# PR: project file `.lsproj` (T-286)

Branch: `feat/project-file` · Task: `tasks/T-286-infra.md`

## What / why

A laserist wants one file per show, easy to copy and back up. Until now
the state was spread over `studio-data/*.json`. This adds **projects**:

- `studio/src/project.rs`: `Project`, versioned JSON (`format_version: 1`,
  `app_version`, `saved_at`, `name`), pretty-printed, in
  `<data-dir>/projects/<nom>.lsproj`. Sections: `scenes`, `playlist`,
  `grid` (cue-grid properties from `grid.json`), `timelines` (the shows of
  `shows/`), `tempo` (BPM, beats per bar), `live` (master modifiers),
  `layers`, `lfos`, `palettes`, `midi.profiles` (user MIDI profiles).
  Every section has a default; unknown fields (future `pages`, `venue`,
  `outputs`, `ui`…) are kept in `extra` and written back on save.
- The data directory's files stay the **working copy**; a project is a
  snapshot of them. *Ouvrir* checks the whole file, swaps the stores in
  memory under the lock in one go (infallible), then rewrites the working
  files with the lock released (shows and user MIDI profiles not in the
  project are removed from `shows/` and `midi/profiles/`).
- **Never in a project, never changed by opening one**: calibration,
  `safety.json`, `presence.json` (heartbeat / hold-to-run), MIDI safety
  options and port choices (`midi/devices.json`), arming state, e-stop.
  Nothing in the open path touches the gate. A file carrying such fields
  just keeps them as unknown data.
- **Atomic open**: JSON, `format_version` (missing / 0 / newer than 1
  refused), duplicate or empty scene names, timeline names, LFO targets,
  palettes, MIDI slugs (`valid_slug`, not a built-in) and profiles
  (`Profile::parse`) are all checked before anything changes. French
  error, 400 (404 if the file doesn't exist); state untouched.
- **Paths**: `resolve()` accepts a bare name, `nom.lsproj`, or a full path
  whose folder canonicalises to `projects/`; names are letters, digits,
  spaces, `-`, `_` (same rule as show names). Anything else: 400 « chemin
  refusé ». Symlinks in `projects/` are not followed. Names *inside* a file
  (timelines, MIDI slugs) are validated before being used as file names.
- **Atomic writes**: `.name.tmp` + `fsync` + `rename` + fsync of the folder.
  Done on the HTTP worker thread with the `Shared` lock released (the lock
  is held only to copy the state out, or to swap it in).
- Routes: `GET /api/project` (name, file, `modified`, folder, project list,
  recent), `POST /api/project/new|open|save|save-as` (`{ "path": … }`).
  `save` without a file → 409.
- `recent.json`: current project + 10 most recent.
- First start (no `recent.json`) with existing data: imported into
  `projects/Sans titre.lsproj`; `scenes.json` etc. are kept.
- `modified` = fingerprint of the current sections ≠ fingerprint at last
  open/save (so undoing a change clears the « • »).
- UI: *Projet ▾* menu in the header (*Nouveau*, *Ouvrir…*, *Récents*,
  *Enregistrer*, *Enregistrer sous…*), « • » after the name when modified,
  Cmd/Ctrl+S, Cmd/Ctrl+O, a name/path field plus the list of `projects/`
  (no browsing of the disk), and the *Modifications non enregistrées :
  enregistrer avant ?* prompt (*Enregistrer*, *Ne pas enregistrer*,
  *Annuler*). Échap stays the e-stop, also while a dialog is open.
- Small helpers on the stores (`path()`, `replace_in_memory`,
  `CueDeck::set_config`, `lfo::validate`, `timeline::valid_show_name`,
  `ShowStore::load_all`, `ProfileStore::{dir,user_profiles,replace_user_in_memory}`).

Prepared for T-287 (autosave: reuse `write_atomic`, `snapshot`,
fingerprint), T-288 (the raw `Value` is checked before deserialising: the
migration chain slots into `parse`) and T-289 (site profile stays out of
the project).

## Testing

- `cargo test -p laser-studio`: green (12 new tests in `project::tests`:
  save → change → open round trip incl. playlist/grid/tempo/timelines and
  the working files, unknown fields kept, open never arms / keeps e-stop,
  calibration, safety, presence and MIDI safety even with a hostile file,
  8 invalid files change nothing, path refusal (API + `resolve`), symlink
  not followed, partial write never visible, first-start import keeps
  `scenes.json`, empty data dir = untitled without file, recent list
  capped at 10, 1 000 scenes: time under the lock < 16 ms).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `npm --prefix studio/e2e test` (rebased on develop eeb0932): 112/112, incl. new
  `tests/project.spec.ts` (first-start import; save as via the menu →
  change → reload page → open from *Récents* with *Ne pas enregistrer* →
  scenes, BPM back, still disarmed, calibration not reverted; *Annuler*
  keeps state; Cmd+S; refused paths via API and dialog; invalid file;
  *Nouveau*). Preview only, temporary `--data-dir`, `--no-midi`.

## Risks

- Opening (or *Nouveau*) **replaces** the working copy: shows and user
  MIDI profiles that are not in the project are deleted from `shows/` and
  `midi/profiles/`. That is the document model; the UI asks to save
  unsaved changes first and the first-start import keeps a copy of
  pre-project data. Reviewer: confirm this is acceptable vs. keeping them.
- The acceptance item « 1 000 scènes, compteur de retards à 0 »: there is
  no late-frame counter in the engine yet; covered by design (no I/O under
  the lock) and a timing test of the lock hold.
- `pages` (T-272) doesn't exist: the current grid properties are saved as
  `grid`; T-272 can add `pages` alongside.
- `GET /api/project` (polled every 2 s by the UI) copies the sections
  under the lock and reads `shows/` to compute `modified`.
- The live masters are part of the project, so moving a fader marks the
  project modified (like other show software).

## Review
