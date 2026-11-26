# PR: figure editor in CRÉATION › Figures (T-296)

Branch: `feat/figure-editor` · Task: `tasks/T-296-cues.md`

## What / why

The user wants to make their own laser content (logos, shapes, small
animations) without any third-party library. This adds a figure editor to
the CRÉATION workspace, a figure library, and a new content type
`Content::Figure` that plays as a cue through the normal output path.
Our own design (the idea of a point-by-point figure editor, nothing copied).

### Model (`studio/src/figures.rs`, new)

- `Figure { name, frames: Vec<FigureFrame>, rate, per: beat|second, loop_mode: loop|ping_pong|once }`,
  `FigureFrame { strokes: Vec<Stroke> }`, `Stroke { points: Vec<[f32;2]>, color: [u8;3], lit }`.
  Every field `serde(default)`. The task's `fps_or_beats` is split into
  `rate` (frames per unit) + `per`.
- `Figure::validate`: name = file name (`timeline::valid_show_name`: letters,
  digits, spaces, `-`, `_`, ≤ 64 chars), limits (256 frames, 1 000 strokes
  per frame, 20 000 points per figure: refused with a French message),
  coordinates clamped to -1..1 (non-finite → 0), empty strokes dropped, at
  least one frame, rate clamped to 0.05..60.
- `frame_index(pos)`: loop / ping-pong (1 2 3 2 1…) / once (holds the last).
- `frame_points(i, scale, paint)`: strokes in order. A lit stroke starts
  with a blanked point (so the travel from the previous stroke is dark),
  a one-point stroke is held 8 samples (visible dot), a `lit: false`
  stroke is an **explicit blanked move** along its points.
- `FigureStore`: `studio-data/figures/<name>.json`, loaded at start-up and
  kept in memory (sorted); atomic writes (`project::write_atomic`); names
  never leave the folder; invalid files skipped with a warning.
- `refresh(&mut Shared)`: rebuilds the « Figures » cue page from the
  library (`Preset { id: "figure:<name>", category: "Figures", settings:
  { content: Figure, scale: 1.0 } }`) and rebuilds the control registry
  so the page's `grid.9.r.c` MIDI cells exist. Called at start-up, after
  save/delete, after a project open.

### Engine (`engine.rs`)

`Content::Figure(Figure)` (JSON `{"kind":"figure", ...}`) in `render_look`:
the frame index comes from the look's `beat_pos` (per beat: locked to the
tempo clock, counted from the bar the cue started in, like the other
beat-synced looks) or from a per-animator `fig_time` (per second, follows
the master « Vitesse »). Colours per stroke go through the look's
brightness, gate and audio hue shift; the result is rotated and
`densify`d like every look. `same_drawing` = same figure name. Nothing
else changes: a figure cue goes deck → layers + point budget → live
modifiers → calibration → strobe limiter / horizon → arm gate.

### Cue grid

`presets::CATEGORIES` gains a 9th page « Figures » (empty until the first
figure is saved; `page.9` added to `docs/controls.md`, regenerated).
Figure cues are ordinary presets: grid properties (mode, group, layer,
show), MIDI grid cells, timeline cue events (`EventSource::Cue`) work
unchanged.

### API (`web.rs`)

- `GET /api/figures` → `[{name, id, frames, points}]`
- `POST /api/figures` (a `Figure`) → validate, save, refresh → `{name, id}`; 400 with the French reason.
- `POST /api/figures/load {name}` → the figure (404 if unknown).
- `POST /api/figures/delete {name}` → remove file + cue.
- `POST /api/figures/text {text, size}` → `{strokes}`: the laser font
  as strokes for the text tool (`font::text_strokes`, same layout as
  `text_to_points`, new unit test). No state touched.
- Playing uses the existing `POST /api/presets/play {id: "figure:<name>"}`.

### Project file (`project.rs`)

T-286's design made it natural: a new section `figures: Vec<Figure>`
(`serde(default)`, so **older projects still open**, with no figures).
Handled like timelines: snapshot/fingerprint (saving a figure marks the
project modified), `check` (valid names, no duplicates, limits), `apply`
(memory + cue page), `persist` writes `figures/*.json` and prunes the
previous project's figure files (only `<valid name>.json` directly in the
folder), `figures` added to the first-start import list. Format version
unchanged (1): the section is optional.

### UI (`index.html`, French)

- CRÉATION gets two sub-tabs: **Look** (the existing Contenu / Apparence,
  unchanged) and **Figures** (remembered in `localStorage`
  `laserStudio.creationTab`, try/catch like the others; default Look).
- Figures layout: editor on the left (toolbar, square canvas with grid,
  image strip at the bottom), stage (live preview) and properties on the
  right.
- Tools: Sélection (click, Maj+click, drag to move), Point, Polyligne
  (clicks; double-click / right-click / « Terminer » ends), Courbe
  (Bézier, 4 clicks), Rectangle, Ellipse, Polygone (drag centre → vertex,
  « Côtés »), Texte (laser font, size slider), Déplacement éteint
  (blanked polyline), Gomme. Magnétisme (0.1 grid) optional.
- Per-stroke colour and Allumé/Éteint (applies to the selection and to new
  strokes); drawing order shown (number + arrow per stroke, dotted
  implicit travel, dashed blanked moves) and editable (Avant / Après /
  Inverser le sens); Supprimer (also the Suppr key). Transforms on the
  selection or the whole image: rotation ±15°, scale ±10 %, symmetry X / Y.
- Animation: + Image, Dupliquer, Supprimer l'image, move ◀ ▶, onion skin
  (previous image dimmed), cadence (images par temps / par seconde),
  Boucle / Aller-retour / Une fois, « Lire » plays it in the editor at
  the studio's BPM.
- Undo / redo: whole-figure snapshots, 200 levels (buttons, Cmd/Ctrl+Z,
  Cmd/Ctrl+Maj+Z, Ctrl+Y; never while typing).
- Counter: strokes and drawn points of the image, estimated laser points
  after densify, estimated images/s at the output's pps, heaviest image vs
  the point budget (warning colour when over budget or under 15 images/s).
- Library list (open, delete with confirmation), Nouvelle, Enregistrer,
  **Jouer** (= save, then play its cue). The Look sub-tab shows a note
  when the current look is a figure.
- Keyboard: Escape / Space / F1–F4 unchanged (the editor adds no global
  key; Enter and Backspace keep tap / resync, so polylines end with a
  double-click).

## Testing

- `cargo test -p laser-studio`: green (new: 9 tests in `figures::tests` —
  stroke → points with blanked travel and moves, dots, loop / ping-pong /
  once, validate (names, clamps, limits), store round trip and folder
  confinement, JSON defaults and `Settings` round trip, beat-locked
  animation from the bar's one and tempo independence, per-second
  animation, densified and dimmed output, figures as cues of the Figures
  page incl. MIDI grid cells and removal; `font::text_strokes`;
  `project`: figures saved / reopened identical / working copy pruned /
  old project without the section opens; invalid figure names and
  duplicates refuse the whole project).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `studio/e2e/tests/figures.spec.ts` (4 tests): draw (rectangle,
  polyline, blanked move, colours) → invalid name refused (UI and API) →
  save → restart → reopen identical → Jouer → non-empty frame with the
  figure's colours and size, nothing lit on the blanked move, still
  disarmed (`output_lit` 0), muting layer 1 blanks it, the cue is on the
  LIVE « Figures (1) » page and toggles off; 3-image animation (add,
  duplicate, drag) at 240 BPM in ping-pong: every image shows and the one
  on show matches the beat in the bar; undo / redo over 55 actions
  (buttons and keyboard, a new action clears redo), undo after reorder +
  mirror; delete from the library removes the cue; Look sub-tab back.
  Screenshot `test-results/figures-editor.png`. `reveal()` in `studio.ts`
  now also clicks the CRÉATION sub-tab of an element.
- Full e2e suite after rebasing on develop 9a86362 (with T-231):
  **141 passed**, 0 failed. Unit: 519 passed + 2 signal tests.

## Risks

- **Opening a project replaces the figure library** (like timelines and
  MIDI profiles, T-286's document model): opening an older project with no
  `figures` section deletes the working `figures/*.json`. The
  unsaved-changes prompt covers figures (they are in the fingerprint).
  Reviewer: confirm this is wanted rather than keeping figures across
  projects.
- A 9th cue page shifts nothing for existing pages (appended), but
  `page.next` now cycles through 9 pages, including an empty one until a
  figure exists.
- The control registry is rebuilt on each figure save/delete (cheap; LFOs
  only target continuous controls, mappings are by id). A MIDI mapping to
  a `grid.9.*` cell of a deleted figure just finds no cue.
- A figure is embedded in the cue's `Settings` (like evolving cues), so a
  scene or timeline keeps the version it was made with; re-saving a
  figure does not update a copy already playing (press it again). It is
  cloned with the look every frame; the 20 000-point cap bounds that.
- The editor's point / fps counter is a client-side estimate of `densify`;
  the budget itself is enforced by the mixer (thinning) as for any cue.
- The per-stroke colour is per stroke (polyline), not per segment inside a
  polyline: draw separate strokes for different colours.

## Review

Reviewed by the architect (integrator). Clean merge on 9a86362; 519 unit
+ 2 signal tests, e2e 141/141 twice, clippy clean. Figures render through
the normal path (deck → layers/budget → live → calibration → safety →
gate), strokes start blanked, names/paths confined, limits enforced, no
third-party content. Accepted: figures belong to the project (like shows
and MIDI profiles — told the user), a 9th « Figures » cue page, copies
not links, colour per stroke.
Verdict: APPROVED
