# PR: four workspaces LIVE / TIMELINE / CRÉATION / RÉGLAGES (T-295 + T-270)

Branch: `feat/workspaces` · Tasks: `tasks/T-295-ui.md`, `tasks/T-270-ui.md`

## What / why

The page was one long column that mixed playing and designing; on a
laptop the preview pushed the cue grid below the fold. The user asked for
an organisation by big tabs, like Showcontroller's Live / RealTime /
content modules (the idea, not their UI). This is a **UI reorganisation
only**: no server change, no control changes meaning, every element `id`
is kept (checked by diffing the ids before/after: none lost, none
duplicated), so MIDI learn `data-control`, e2e selectors and scripts keep
working.

- **Top bar, always on screen** (`#top`, sticky, outside every workspace):
  Projet menu, tempo (BPM, beats, Tap, Sync 1, ÷2, ×2, nudges), MIDI status,
  output, LASER ON/OFF + arm info + presence/engine LEDs, **ARRÊT**, then
  the banners (e-stop, hold-to-run, MIDI lost, MIDI learn), then the new
  workspace tab bar `#wsTabs`: **LIVE F1 · TIMELINE F2 · CRÉATION F3 ·
  RÉGLAGES F4**, with a reminder « Espace laser · Échap arrêt, dans tous les
  onglets · lettres = cues (LIVE) ».
- **One CSS grid below it** (`main#workspace[data-ws-active]`), filling the
  window under the top bar (`--top-h`, kept up to date by a
  ResizeObserver since banners change its height). Panels scroll inside
  themselves, so the preview and the grid stay put.
- **Stage** (`#stage`, T-270): viewbar with stage tabs **2D / 3D / 2D+3D**
  (2D+3D is new: canvas and WebGL view side by side, both fed by the same
  `/api/frame` fetch), the view (square 2D, 3D fills the stage; sized with
  container query units), 3D render settings, stats line, evolving
  progress bar. Shown in every workspace.
- Workspace containers carry `data-ws`; switching only sets `hidden` on
  them and posts nothing.

### Where each section went

| Workspace | Sections |
|---|---|
| **LIVE** | stage · **Calques** (next to the preview) · right panel with tabs **Direct** (rotation, master size/position/tilt/speed/brightness, colour block) / **Modulateurs** (LFO) / **Musique** / **Scènes** (scenes + playlist) · **Cues** full width at the bottom (modes, Multi/Max, Tout arrêter, page tabs, grid; the grid scrolls inside its region) |
| **TIMELINE** | stage · Timeline player (show, play/pause/stop/loop, bar, playhead) · « À venir » note (editor, waveform, templates, timecode) |
| **CRÉATION** | stage · Contenu (Forme/Texte/Onde/Effet, generators + help, evolving-cue panel) · Apparence (look colour, size, rotation, brightness) · « À venir » note |
| **RÉGLAGES** | stage (useful for calibration) · Sécurité (strobe, burst, horizon, UI timeout, hold-to-run) · Calibration · Contrôleur (devices incl. T-205 « Retour LED », mappings, MIDI learn mode, MIDI safety) · Sorties et projecteurs (placeholder) |

The Projet menu stays in the top bar (T-295 lists "projet" under
RÉGLAGES; the top bar is visible there too, and Cmd+S/Cmd+O work
everywhere). The `#cueMenu` popup moved out of the stage to `<body>`
(`position: fixed`, unchanged). Clicking the MIDI status opens
RÉGLAGES › Contrôleur.

### Keyboard

- Escape (stop), Shift+Escape (disarm), Space (arm toggle), the hold-to-run
  key, Enter (tap) / Backspace (resync) — tempo is in the top bar — and
  Cmd+S / Cmd+O: every workspace, unchanged.
- **F1–F4** switch workspaces, whatever has focus (they type nothing),
  not with modifiers.
- **Cue letters (and Shift+letter flash) only in LIVE**. The `<` key
  (hold to reverse the master rotation, a Direct control) is LIVE-only as
  well.
- MIDI learn: right click works on every `[data-control]` of the visible
  workspace and on the top bar; learn mode (started in RÉGLAGES) keeps
  running while you switch to LIVE and click a cue/control (tab buttons
  are not `data-control`, so they are not intercepted).

### Remembered

`localStorage` keys `laserStudio.workspace`, `laserStudio.livePanel`,
`laserStudio.stageTab`; every read/write in `try/catch`; without storage
the page starts on LIVE / Direct / 2D.

### Layout

1440×900: preview, layers, Direct panel and ≥ 3 rows of cues without page
scroll. 1280×800: no horizontal scroll in any workspace. Below 900 px:
one column (stage, layers, grid, panel).

## Screenshots (Playwright, 1440×900, `studio/e2e/test-results/workspaces-*.png`)

- **LIVE**: top bar across; tab bar with LIVE underlined in green; square
  2D preview top left with the « Laser éteint » badge; the 4 layer strips
  (vertical faders, Muet/Solo/Vider) in the middle column; Direct panel
  on the right with its 4 tabs (rotation presets, master sliders, scrolls
  inside); the Cues region across the bottom (modes, page tabs, 10 cells
  per row, 3 full rows visible, the rest scrolls inside).
- **TIMELINE**: large preview on the left, the Timeline player card at
  the top right (show selector, Lecture/Pause/Arrêt/Boucle, position bar),
  « À venir » note underneath.
- **CRÉATION**: large preview on the left, Contenu (Forme/Texte/Onde/Effet
  + shapes grid) and Apparence cards on a 440 px right column.
- **RÉGLAGES**: preview on the left (2/5), a grid of cards on the right:
  Sécurité (with the limiter badge), Calibration, Contrôleur (collapsed
  `details` as before) and the Sorties placeholder.
- **LIVE 2D+3D**: square canvas and WebGL beam view side by side.
- **narrow (800 px)**: single column.

## Testing

- `cargo test -p laser-studio`, `cargo clippy -p laser-studio --all-targets -- -D warnings`: green (no Rust change).
- New `studio/e2e/tests/workspaces.spec.ts` (13 tests): 4 tabs and each
  section in its workspace (and hidden elsewhere); top bar / LASER /
  ARRÊT / MIDI-lost banner visible in every tab, ARRÊT button latches and
  its banner shows in every tab; Space / Shift+Escape / Escape in every
  tab; Escape while typing in CRÉATION; cue letters (plain, Shift, `<`)
  ignored outside LIVE then working in LIVE; F1–F4 (also from a text
  field) leave `/api/state` and `/api/frame` unchanged (tempo phase and
  presence excluded, they tick on their own); clicking all tabs, panel
  tabs and stage tabs while armed with a cue playing leaves them
  unchanged; workspace / panel / stage tab back after reload; page works
  with a throwing `localStorage`; right-click MIDI menu in LIVE,
  CRÉATION (top bar) and RÉGLAGES; MIDI status → RÉGLAGES; T-270 layout
  at 1440×900 (preview and first two cue rows inside the viewport, no
  page scroll), no horizontal scroll at 1280×800, one column at 800 px;
  screenshots.
- Existing specs: new helpers in `studio.ts`: `openWorkspace(page, ws)`
  and `reveal(page, selector)` (clicks the workspace and LIVE panel tab
  holding an element, then asserts it is visible). `openUi` now clicks
  LIVE first (a remembered tab survives `page.reload()` inside a test);
  its assertions are unchanged. Specs only gained `reveal`/`openWorkspace`
  calls before touching a section that is now behind a tab; no assertion
  removed or loosened. `cues.spec` « cue keys are ignored while typing »
  additionally types in a LIVE text field (#sceneName), since CRÉATION
  now blocks cue letters anyway. `evolving.spec` also checks `#evoBar`
  stays visible in CRÉATION.
- Full e2e suite after rebasing on develop (bcedd88, with T-230 and T-205): **136 passed**, 0 failed. The T-205 « Retour LED » checkbox sits in RÉGLAGES › Contrôleur (`midi.spec` reveals it).

## Risks

- Layout is new CSS grid + container query units (`cqw/cqh`, Chrome 105+,
  Safari 16+). Older browsers would get a mis-sized preview.
- `<` (reverse hold) is now LIVE-only; Enter/Backspace (tempo) stay
  global. If the operator wanted `<` in other workspaces, it is a one-word
  change.
- Controls in a hidden workspace are not clickable; anything a user
  script drives through the DOM must switch tabs first (the API and MIDI
  are unaffected).
- The 2D+3D mode runs both renderers: more GPU/CPU than 3D alone.
- RÉGLAGES keeps the preview (T-295 asks it for the three other tabs; it
  helps calibration). The Sécurité / Calibration / Contrôleur sections
  stay collapsible `details` as before.

## Review

Reviewed by the architect (integrator). Clean merge on bcedd88; no
server change, all element ids kept; 492 unit + 2 signal tests, clippy
clean; e2e 136/136 on 3 consecutive full runs after I fixed a race in
the « Soleil levant » content test (it read one frame that could still
be the previous look; it now polls the whole condition). Safety:
LASER/ARRÊT/banners outside every workspace, Escape handled first.
Accepted departures: `<` LIVE-only, preview also in RÉGLAGES, project
menu in the top bar, empty « Sorties et projecteurs » card for T-277.
Verdict: APPROVED
