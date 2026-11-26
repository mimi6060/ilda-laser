# Research: visualisation, operator ergonomics and show files

Scope: how professional laser software and dedicated previsualisers show
lasers (2D graphics view vs 3D beam view in haze), how operators lay out their
screens for live busking, and how whole shows are saved, versioned and moved
between machines. The last sections turn this into a concrete screen layout
and task files `tasks/T-270` … `tasks/T-289` for Laser Studio.

Public information only: the Pangolin wiki and forum, Syncronorm (Depence),
Capture, L8, Laserworld/Showcontroller product pages and manuals, MA Lighting
online help, three.js community examples. Nothing was decompiled, no UI assets,
icons, names of effects or content are copied. We take concepts and rebuild
them. Accessed 2026-09-27.

It builds on `pro-live-operation.md` (live modifiers, cue model, tempo,
timeline: T-140 … T-171), `festival-looks.md` (the beam looks the visualiser
must make readable: T-100 … T-130) and `midi-apc40.md` (T-200 … T-211).

---

## 0. TL;DR

1. **Two views, always.** Every pro tool shows the laser as (a) a flat
   "graphics" view, which is what we have today (the 2D canvas: x,y in -1..1),
   and (b) a **beam view in perspective**: beams leaving a projector through
   haze, seen from the audience. QuickShow hides (b) in the projection zone's
   preview tab; users on the Pangolin forum are surprised it exists
   ([forum](https://forums.pangolin.com/threads/laser-output-preview-in-perspective.29229/)).
   Since 5.5 (Jan 2024) both BEYOND and QuickShow have a full **3D Preview**
   window: a room (4 walls, floor), gauze screens, projectors placed by
   X/Y/Z + rotation, per-projector brightness and scan angle, a **fog
   intensity** that interacts with **room lighting**, three camera orbit
   modes, and saved 3D layouts
   ([BEYOND 3D Preview](http://wiki.pangolin.com/doku.php?id=beyond:3d-preview),
   [5.5 release](https://pangolin.com/blogs/news/beyond-and-quickshow-5-5-release)).
2. **Dedicated visualisers go further but cost money and a second
   machine.** Depence R4 (Syncronorm) renders "hot beams" whose thickness
   grows with distance (diode divergence), animated haze clouds, and
   **scanner inertia**, so a too-complex frame visibly flickers or rounds its
   corners like real galvos
   ([Depence laser](https://www.syncronorm.com/products/depence2/visualization/laser),
   [Pangolin Depence module](https://pangolin.com/products/depence-laser-module)).
   Capture (Polar) takes BEYOND's output and places projectors in a venue
   render ([Pangolin × Capture](https://pangolin.com/blogs/news/pangolin-partners-with-capture-sweden-for-visualization-of-laser-shows)).
   L8 (ex-LightConverse) does photoreal lasers for Pangolin and Lasergraph
   ([L8](https://l8.ltd/m/), [PLSN](https://plsn.com/articles/software-solutions/from-lightconverse-to-l8/)).
   Showcontroller relies on external visualisers (Realizzer 3D, Depence,
   Capture) for 3D
   ([Realizzer support](https://www.showcontroller.com/en/showcontroller-software/support-of-realizzer-3d.html),
   [Capture demo stage](https://www.laserworld.com/en/download-file-1772-Showcontroller_Capture_Demo.html)).
3. **A browser beam view is realistic and cheap.** A projector is a point
   with a scan cone; each output point (x,y) becomes a direction, each
   direction a ray to the first wall/floor/screen. Drawing every sample as a
   line (or every pair of lit samples as a thin triangle) with **additive
   blending** and **alpha divided by the number of samples** reproduces what
   the eye sees: a static beam stacks many samples and looks bright, a fan
   or sheet spreads the same energy and looks dimmer. 4 projectors × 3 000
   points = 24 000 line vertices per frame: trivial for WebGL2 on an M-series
   Mac. The real cost is fill rate (overlapping translucent sheets), handled
   with a half-resolution beam buffer and a cheap bloom. Budget: ≤ 4 ms GPU
   and ≤ 2 ms JS per frame at 1080p.
4. **Busking screens share one shape**: a big cue grid in the middle with page
   tabs, a live preview, master controls on the side, and a tempo readout.
   QuickShow = cue grid (largest) + QuickTools below + a right panel with
   *Live Control* / *Effect Editor* tabs
   ([main window](https://wiki.pangolin.com/doku.php?id=quickshow%3Amain_control_window)).
   BEYOND is "designed for a dual monitor system" and can use 3–4, with a
   second undockable grid, together showing up to 200 cues, and undockable
   QuickFX
   ([grids and pages](https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages)).
   Cues are triggered by mouse, **touch screen** or letter keys (unshifted and
   shifted); pages can be bound to F-keys
   ([QuickShow cue grid](https://wiki.pangolin.com/quickshow:cue_grid)).
5. **Safety and "don't break the show" ergonomics** matter more than looks:
   a blackout that is always visible and always one key away (we already
   have Escape), a clear armed/disarmed state, no destructive action on a
   single click during a show, and an undo history for edits (the grandMA3
   **Oops** key and Oops menu are the lighting-industry reference:
   tap = undo last, hold = history list
   ([MA help](https://help.malighting.com/grandMA3/2.3/HTML/ws_oops_overlay.html))).
6. **One file per show.** QuickShow's workspace (`.qsw`) is a "master file"
   with all cues, frames, text, effects and some configuration
   ([cue grid wiki](https://wiki.pangolin.com/quickshow:cue_grid)); BEYOND
   saves/loads whole workspaces, single pages or single cues. Weak points
   reported by users: audio files are **not** inside the workspace, and a full
   backup means copying the whole `C:\BEYOND` directory
   ([forum](https://forums.pangolin.com/threads/data-and-settings-backup-for-beyond.1590/));
   the scripted autosave "pauses output and isn't recommended for live"
   ([PangoScript examples](https://wiki.pangolin.com/doku.php?id=examples%3Apangoscript)).
   We can do better: one self-describing project file, an optional bundle
   with media, autosave that never touches the engine thread, and migrations.
7. **For Laser Studio**: (A) a layout refresh with a fixed top bar (safety +
   tempo + masters), a stage area with *2D / 3D* tabs, a right panel with
   tabs, and a configurable cue grid; (B) a WebGL2 visualiser with venue,
   projectors, haze, camera presets and safety overlays; (C) show-mode lock,
   night theme, touch mode, shortcut help, undo; (D) a project file with
   autosave, versions and import/export. See § 6 and § 7.

---

## 1. Visualisers

### 1.1 What the reference products show

| Product | 2D graphics view | Beam view | Venue / projectors | Haze | Other |
|---|---|---|---|---|---|
| QuickShow ≤ 5.4 | per-zone preview | "perspective" option in the projection zone's *Preview* tab | – | – | beams as dots in flat view ([forum](https://forums.pangolin.com/threads/laser-output-preview-in-perspective.29229/)) |
| QuickShow / BEYOND 5.5+ | yes | **3D Preview** window | room L×W×H, origin, 4 walls + floor, gauze/mesh screens, projectors X/Y/Z + rotation, scan angle, brightness; multi-select; command line; save/open/reset layouts; floor/wall textures | fog intensity + room light | 3 camera rotation modes: around the room, around the selection, around the camera ([wiki](http://wiki.pangolin.com/doku.php?id=beyond:3d-preview)) |
| Showcontroller | OpenGL preview, PicEdit 2D/3D | via Realizzer 3D, Depence, Capture | in the external tool | external | ([Showcontroller](https://www.showcontroller.com/en/), [about](https://www.showcontroller.com/en/manual/1-showcontroller/1-2-about-showcontroller.html)) |
| MadMapper / MadLaser | mapping view over a video/scene; "laser beam preview" | limited; fog is a physical recommendation | surfaces/quads, mesh warp | – | ([MadLaser](https://madmapper.com/extensions/madlaser), [guide PDF](https://madmapper.com/files/MadLaser%20Guide.pdf)) |
| Depence R4 | – | photoreal real-time | full venue, fixtures, MVR import | intensity, dynamic clouds, fluid | hot beams with divergence, **galvo inertia** vs scan rate ([Syncronorm](https://www.syncronorm.com/products/depence2/visualization/laser), [help](https://help.depence.com/depence-construction/depence-laser)) |
| Capture | – | yes | import renders, place projectors | yes | fed live by BEYOND ([Pangolin](https://pangolin.com/blogs/news/pangolin-partners-with-capture-sweden-for-visualization-of-laser-shows)) |
| L8 | – | photoreal, VR | Vectorworks/MVR/FBX/glTF import | yes | Pangolin and Lasergraph input ([L8](https://l8.ltd/m/)) |
| WYSIWYG (Cast) | – | lighting-first; lasers are secondary | CAD venue | yes | industry reference for venue CAD, not laser-specific |

Take-aways:
- The **2D graphics view** is what the laser draws on a flat screen. It is the
  right tool for graphics, text and calibration. It is misleading for
  beam shows: a fan of 8 beams looks like 8 dots.
- The **beam view** is what a festival audience sees. It is the only way to
  design fans, tunnels, liquid sky and crossings without a laser and haze.
- Pro visualisers share a small set of parameters: room size, projector pose,
  **scan angle** (how far ±1.0 goes in degrees), per-projector brightness,
  haze density, room light, gauze screens, and camera modes.
- Realism extras (divergence, galvo inertia, animated haze) are nice but
  second-order. The first-order value is **correct geometry and relative
  brightness**.

### 1.2 How to build it in the browser

**Geometry.** A projector `P` has a position, a yaw/pitch/roll and a scan
angle `θ` (full optical angle, typically 30–60°). An output point
`(x, y) ∈ [-1, 1]²` is a direction in the projector's frame:
`yaw = x · θ/2`, `pitch = y · θ/2` (the same convention as galvo angles; the
engine's final output, i.e. after calibration and safety, is used so what
you see is what the laser would do). The ray from `P` is intersected with
the venue: an axis-aligned box (floor, ceiling, walls; outdoor = no ceiling
and far walls) plus a list of gauze/screen quads. The hit gives the beam
end point and a "spot" on the surface.

**Beams.** Two primitives, both with `blending = additive`,
`depthWrite = false`:
- *Beam lines*: one line (or a screen-aligned quad for thickness) from `P`
  to the hit point per lit sample. Colour = sample RGB × brightness.
- *Sheets*: for two consecutive lit samples, a thin triangle `P, hit_i,
  hit_{i+1}`. This is what persistence of vision does to a fast scan: a
  sweeping line becomes a plane of light in haze.

**Energy conservation (the key trick).** A galvo spends equal time on every
sample. So each sample gets alpha `k / N` where `N` is the number of samples
in the frame and `k` a global exposure. A static beam repeated by corner
dwell (`densify`) stacks its samples and becomes bright; a 200-point sheet
spreads the same energy and becomes a faint plane. This matches reality with
no special cases, and it also makes the preview show the cost of too many
beams (each gets dimmer), which is exactly the lesson laserists learn on
site (see `festival-looks.md` § 4.3).

**Haze.** Uniform density `ρ` (0–1): beam alpha × `ρ`, attenuated along the
beam by `exp(-σ·d)`. Optional animated noise: a 3D value-noise texture
scrolled slowly, sampled in the beam fragment shader by world position,
to fake drifting clouds (Depence's "dynamic clouds"). A **room light**
slider (0 = pitch dark) draws the venue geometry at that brightness, so the
contrast between beams and a lit room is visible (BEYOND's fog vs room light
idea).

**Spots.** Where a beam hits a surface: a small additive sprite; on a
gauze screen the spots together form the 2D graphic, which is how graphics
"float" in 3D previews.

**Divergence (optional).** Thickness grows with distance
(`w = w0 + d · tan(div)`), alpha divided by the width so energy is kept
("hot beam").

**Scanner inertia (optional).** Run the frame through a first-order low-pass
filter at the output's scan rate (e.g. 30 kpps) before drawing: corners get
rounded and over-complex frames flicker, as Depence simulates. This depends
on T-171 (point budget, pps per output).

**Post.** Render beams into a half-resolution float target, blur it
(two-pass Kawase or a 5-tap Gaussian, 2–3 levels) and add it back: "bloom"
sells the glow of beams in haze.

**Camera.** Orbit (around the room centre or a target), pan and zoom with the
mouse / trackpad / touch, plus **presets**: *Public (régie)* at head height
at the back, *Premier rang*, *Scène* (from behind the projectors, looking at
the audience: it shows audience exposure), *Dessus* (plan), *Côté*
(elevation), *Libre*. Smooth 400 ms transitions.

**Venue.** A small model: box room L×W×H, a stage block, a truss height, an
audience area rectangle, and N gauze screens. Presets: *Club* (15×10×5 m),
*Salle* (30×20×10 m), *Festival plein air* (60×40 m, no ceiling, distant
back plane). Projectors: position, rotation, scan angle, brightness,
colour tag, which output/zone it shows. BEYOND also exposes a command line
for positioning. A numeric form is enough for us, and gizmo dragging can
come later.

**Safety overlays.** Draw T-003 zones and the horizon as translucent volumes
projected into the venue, and the audience area as a floor rectangle
raised to 3 m (the usual minimum separation height; the actual value is
configurable and must follow local regulation). Any beam segment inside that
volume is drawn **red** and counted. This turns the visualiser into a
planning tool for audience safety, not just eye candy. It must never be
presented as a certification; it is a design aid.

**Library choice.**
- three.js is MIT-licensed; `OrbitControls`, `EffectComposer` and
  `UnrealBloomPass` exist as examples. But the studio must work **offline**
  in a venue: loading from a CDN at runtime is a single point of failure.
  Recommendation: vendor one pinned `three.module.min.js` (+ the orbit
  controls file) under `studio/src/vendor/`, served by `web.rs` via
  `include_bytes!`, and list them in `docs/CONTENT_SOURCES.md` (URL, MIT,
  date). No build step is needed: `<script type="importmap">` + ES modules.
- Alternative: raw WebGL2 (~500 lines: lines, triangles, a box, a blur). No
  dependency, but more code to maintain. The first task should decide with a
  quick spike; the report recommends three.js vendored.
- If WebGL2 is unavailable, the 3D tab says so and the 2D view stays.

**Data path.** Today the UI polls `GET /api/frame` (JSON) for the one output.
For the visualiser: `GET /api/frames` returns one entry per output (or the same
frame for every projector until multi-output lands, with per-projector
*miroir X* for symmetric rigs). JSON at 3 000 points × 5 floats ≈ 100 kB per
frame. At 30 Hz that is 3 MB/s on localhost, acceptable. A later optimisation
is a binary `application/octet-stream` (f32 array) or a WebSocket.

### 1.3 Performance budget (target: MacBook M-series, 1080p–1440p window)

| Item | Budget |
|---|---|
| Frame fetch + parse | ≤ 2 ms JS (binary), ≤ 5 ms (JSON) at 30 Hz |
| Geometry build (rays → hits, 4 × 3 000 samples) | ≤ 1 ms JS (typed arrays, no per-point objects) |
| Draw calls | ≤ 20 (venue, beams, sheets, spots per projector, post) |
| GPU beams + sheets | ≤ 2 ms (half-res target) |
| Bloom + composite | ≤ 1.5 ms |
| Total | 60 fps render, frame data at 30 Hz interpolated; drop to 30 fps render when the tab is not visible; `requestAnimationFrame` only |
| Engine impact | zero: the engine thread never waits on the UI |

Quality levels *Basse / Moyenne / Haute* (sheets on/off, bloom levels,
noise haze on/off, render scale 0.5/0.75/1).

---

## 2. Operator UX for live busking

### 2.1 What pros put on screen

- **Cue grid** is the centre of gravity. QuickShow: 60 cues visible per page,
  up to 32 pages, ~2 000 cues per workspace; BEYOND: 100 cues per page (grid
  size set in configuration), up to 250 pages, two grids for 200 visible
  cues ([BEYOND grids](https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages),
  [QuickShow cue grid](https://wiki.pangolin.com/quickshow:cue_grid)).
  Showcontroller LIVE: 40 scenes per bank × 10 banks, i.e. 8×5, the APC40
  layout ([Showcontroller LIVE](https://www.showcontroller.com/en/showcontroller-software/showcontroller-live)).
  The **8×5 = 40** page matching the APC40 is our natural default (T-204).
- Each cue cell shows: name, the **key** that triggers it, a **thumbnail**,
  a colour, and states (active, flash held, queued for the next beat).
- **Page tabs** above the grid; pages on F-keys.
- **Preview**: one live output preview is always visible; BEYOND adds more on
  extra monitors (ERP, Universe, video).
- **Masters**: brightness/dimmer, size, speed, and a blackout, always on
  screen (QuickShow's *Live Control* tab applies to the whole output).
- **Tempo**: BPM in large digits, beat lights (1 of 4 highlighted), tap,
  resync (see T-150).
- **Status**: output state (disarmed/armed), pps / points, fps, which DAC.

### 2.2 Conventions for dark venues

- Dark UI (near-black backgrounds, low-luminance panels) so the operator's
  eyes stay dark-adapted and the booth screen doesn't light the room.
  We already use `#0b0d10`; add a **night mode** that lowers overall
  luminance (e.g. `filter: brightness(.6)`), removes large bright fills and
  optionally tints UI to red/amber like astronomy apps.
- Colour is reserved for **state**: red = laser armed/danger, green = active
  cue, amber = queued/warning, beat light = white/amber. Don't use
  saturated colour for decoration.
- Large tabular numbers for BPM and time.

### 2.3 Touch screens

- BEYOND/QuickShow cues respond to touch; many LJs use a touch monitor or an
  iPad-style surface. Rules: targets ≥ 44 px (Apple HIG), no hover-only
  information, no right-click-only actions (provide a long press), no
  accidental drag scroll on the grid (`touch-action: manipulation`), flash
  cues must support pointer down/up (hold) via Pointer Events.

### 2.4 Keyboard

Current: letters (AZERTY rows `azertyuiop` / `qsdfghjklm` / `wxcvbn`) = cues of
the page, Space = arm toggle, Escape = blackout. Planned elsewhere: Enter =
tap, ⌫ = resync (T-150), held Shift = strobe (T-142), held `<` = invert
rotation (T-143). Additions here: `F1`–`F12` and `Alt+1…9` = pages (Mac
F-keys often need fn, hence the Alt alternative), `?` = shortcut overlay,
`Cmd+S` / `Cmd+O` / `Cmd+Z` / `Cmd+Maj+Z` for the project and undo,
`Alt+Shift+1…6` = camera presets in 3D. Keys are read with `e.code` so
AZERTY/QWERTY both work (T-150 already requires this). Note: Pangolin uses
shifted letters for a second row of cues. We can't, because Shift is taken by
the strobe (T-142), so the first 26 cues of a page have keys and the rest are
click/MIDI only.

### 2.5 Multi-monitor

A browser can't dock windows, but it can open **extra windows** of the same
UI with a view parameter: `/?vue=sortie` (full-screen output/3D view for a
second screen or a client), `/?vue=grille` (grid only, for a touch screen),
`/?vue=direct` (masters only). All share state via the server
(`/api/state`), and `BroadcastChannel` for instant UI sync (current page,
selection). The Fullscreen API makes a clean output monitor.

### 2.6 Avoiding accidents

- **Show mode lock** (*Mode spectacle*): editing controls (delete cue, rename,
  calibration, venue, save-over) disabled or hidden; only triggers, masters,
  tempo and blackout stay live. A visible padlock in the top bar; unlocking
  asks for a long press (1 s).
- Destructive actions: confirm or **hold to confirm**; never on a single
  click next to a trigger.
- Arm: explicit click on *LASER OFF* or Space, as today (CLAUDE.md). The arm
  button is separated from the grid and far from other buttons. Escape
  always works, even in fields and in every window.
- **Undo** for edits (not for live triggers or arm): the grandMA3 Oops model
  is a good one. `Cmd+Z` undoes the last edit, and a *Historique* panel lists
  the last 50 actions; clicking one undoes back to it
  ([Oops menu](https://help.malighting.com/grandMA3/2.3/HTML/ws_oops_overlay.html)).

---

## 3. Show-file management

### 3.1 What exists

- QuickShow: one workspace file `.qsw` = cues, frames, text, effects and part
  of the configuration ([cue grid](https://wiki.pangolin.com/quickshow:cue_grid)).
- BEYOND: save/load the whole workspace, a page, or one/several cues. A
  timeline show inside a cue is saved in the workspace, **but not its audio**.
  Complete backup = copy the BEYOND directory
  ([forum: backup](https://forums.pangolin.com/threads/data-and-settings-backup-for-beyond.1590/),
  [export a show](https://forums.pangolin.com/threads/creating-a-backup-or-export-a-show.25261/)).
- BEYOND autosave exists only as a PangoScript example (quick-save every
  60 s), and saving pauses output, so it is "not recommended for live"
  ([PangoScript](https://wiki.pangolin.com/doku.php?id=examples%3Apangoscript)).
- Laser Studio today: `scenes.json` and `calibration.json` in `--data-dir`;
  planned: `studio-data/shows/<nom>.json` for timelines (T-160), MIDI
  profiles (T-203/T-211), safety (`safety.json`, T-003).

### 3.2 What we should do

- **Project file** (*projet*, extension `.lsproj`, JSON, UTF-8, pretty-printed
  so it diffs well in git): `format_version`, `app_version`, `saved_at`,
  then sections: `pages` (grid layout: which cue in which cell, user cues
  with full `Settings`/program, built-in cues **by id**), `scenes`,
  `playlist`, `timelines`, `tempo` (default BPM), `live` (master defaults),
  `midi` (mappings, profile choice), `venue` (visualiser: room,
  projectors), `outputs` (zones and routing, T-012), `ui` (grid size, open
  tabs). We use the word *projet* and not *show*, because T-160 already
  calls a timeline a `Show`.
- **Site profile** (*profil de site*) kept separate: calibration, safety
  zones, horizon, DAC addresses, i.e. things that belong to a venue and rig and
  must not be overwritten by opening a project made elsewhere. A project can
  embed a copy; on open, the user chooses *Garder le profil actuel* (default)
  or *Utiliser celui du projet*. Safety settings never silently weaken:
  if the project's zones are less restrictive, a warning lists the
  differences.
- **Media**: ILDA files and audio are referenced by path + SHA-256. *Exporter
  un paquet* (`.lspack` = zip) copies the project + referenced media for
  moving to another Mac. The user's own media only (never commit it;
  IP rules).
- **Atomic writes** (write `*.tmp`, fsync, rename) on a worker thread, never
  the engine thread: saving must never cause a glitch in output (the BEYOND
  pitfall).
- **Autosave**: 30 s after the last change (debounced) and on quit, into
  `autosave/<projet>-<horodatage>.lsproj`, ring of 20. On start, if an
  autosave is newer than the saved project: *Récupérer* dialog.
- **Versions**: *Enregistrer une version* (named snapshot) into
  `versions/`; list with date and name, restore = open as a copy.
- **Format migrations**: `format_version` integer, a chain of
  `migrate_vN_to_vN+1(serde_json::Value)` functions, fixture files per
  version in tests; unknown fields preserved where possible;
  `#[serde(default)]` everywhere (as CLAUDE.md already requires for
  `Settings`).
- **Partial import**: from another project, import pages, cues, MIDI
  mappings or venue, with conflict handling (*Renommer*, *Remplacer*,
  *Ignorer*).
- **Undo** operates on the in-memory project (edit commands), independent
  of files.

---

## 4. What "good" looks like for Laser Studio (principles)

1. The operator can run a whole night looking at **one screen**: grid,
   masters, tempo, output preview, blackout.
2. The designer can build a festival look **without a laser**: 3D beam view,
   audience viewpoint, haze, several projectors, and safety overlays.
3. Nothing the operator does during a show can lose work or glitch the
   output: lock mode, undo, autosave off the engine thread.
4. One file holds the show; one bundle moves it; the site profile protects
   calibration and safety.

---

## 5. Current UI (studio/src/index.html) vs target

Today: header (title, output, *LASER OFF* button); left: square 2D canvas
(`#preview`, 900×900) + points/fps; below it, the *Cues* section (category
tabs, auto-fill grid of 120 px cells, letter keys). Right column (380 px)
stacks *Contenu*, *Apparence*, *Musique* (with beat dot), *Scènes*
(playlist), calibration. Dark theme tokens already exist (`--bg`, `--panel`,
`--accent`, `--danger`).

Gaps: the grid is below the fold on a laptop (the square preview pushes it
down); no persistent master/tempo bar; the right column is a long scroll
of editors mixing *show* and *design* tasks; no beam view; no page-to-key
mapping beyond letters; no lock, undo or project file.

---

## 6. Recommended layout

Target: 1440×900 (MacBook) without scrolling in *Direct*; degrades to
1280×800 (T-143 criterion) and to a single column below 900 px.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ BARRE DU HAUT (fixe, 56 px)                                                  │
│ [Projet ▾ nom •]  [BPM 128.0 ●○○○ Tap Resync]  [Maître ███▁ 80 %]            │
│ [Noir (Échap)]  [Verrou: Mode spectacle]  [Sortie: aperçu | ShowNET | IDN]  [LASER OFF] │
├──────────────────────────────────────────┬───────────────────────────────────┤
│ SCÈNE (≈ 55 % largeur)                   │ PANNEAU (onglets, 360–420 px)     │
│ onglets: [2D] [3D] [2D+3D]               │ [Direct] [Contenu] [Musique]      │
│ ┌──────────────────────────────────────┐ │ [Lieu] [Réglages]                 │
│ │  aperçu sortie (16:9 en 3D, carré 2D)│ │  Direct = T-143 (taille, pos,     │
│ │  badge « Laser éteint : aperçu »     │ │  rotation, vitesse, couleur,      │
│ │  alertes public (rouge)              │ │  luminosité, Tout réinitialiser)  │
│ └──────────────────────────────────────┘ │  Contenu = éditeur actuel         │
│ points · img/s · kpps · qualité 3D       │  Lieu = salle, projecteurs, brume │
├──────────────────────────────────────────┴───────────────────────────────────┤
│ GRILLE DE CUES (≈ 40 % hauteur)                                              │
│ onglets de pages [F1 Festival][F2 Éventails]…[+]      taille: 8×5 ▾  [Calques]│
│ ┌────┬────┬────┬────┬────┬────┬────┬────┐                                    │
│ │ A  │ Z  │ E  │ R  │ T  │ Y  │ U  │ I  │  cases 8×5 : nom, touche, vignette, │
│ ├────┼────┼────┼────┼────┼────┼────┼────┤  couleur, état actif / en file /    │
│ │ …  │    │    │    │    │    │    │    │  tenu                               │
│ └────┴────┴────┴────┴────┴────┴────┴────┘                                    │
│ barre d'état: Historique (Cmd+Z) · sauvegarde auto 12:04 · ? raccourcis      │
└──────────────────────────────────────────────────────────────────────────────┘
```

Region rules:
- **Top bar** is always visible, in every tab and every view (`?vue=`),
  with the blackout control and arm state on the right edge, far from the
  grid. The blackout (*Noir*) button mirrors Escape.
- **Stage** tabs *2D* (today's canvas, keep it: best for graphics and
  calibration), *3D* (visualiser), *2D+3D* (side by side).
- **Right panel** tabs group by job: *Direct* (masters, T-143), *Contenu*
  (today's content/appearance editors), *Musique*, *Lieu* (venue and
  projectors), *Réglages* (calibration, safety, outputs, MIDI).
- **Cue grid** at the bottom, full width, fixed grid size (default 8×5)
  instead of auto-fill, so cell positions match the APC40 and muscle
  memory. The page tabs show the F-key.
- **Status line**: undo history, autosave time, shortcut help.

In *Mode spectacle*, the right panel collapses to *Direct* only, and editing
controls disappear.

---

## 7. Feature list → tasks

| Id | Titre | Prio | Dépend de |
|---|---|---|---|
| T-270 | Nouvelle disposition de l'écran (régions, onglets de panneau) | P1 | – |
| T-271 | Barre du haut fixe : noir, armement, maître, tempo, état de sortie | P0 | T-270 |
| T-272 | Grille de cues à taille fixe (8×5 par défaut), pages sur touches F | P1 | T-270 |
| T-273 | Vignettes animées des cues | P2 | T-272 |
| T-274 | Aperçu avant diffusion (préparer un cue sans l'envoyer) | P2 | T-270 |
| T-275 | Visualiseur 3D : socle WebGL2, salle, caméra orbitale, un projecteur | P1 | – |
| T-276 | Rendu des faisceaux : énergie conservée, nappes, brume, halo | P1 | T-275 |
| T-277 | Lieu et projecteurs multiples (modèle `Venue`, `/api/frames`) | P1 | T-275 |
| T-278 | Points de vue caméra (public, premier rang, scène, dessus, côté) | P2 | T-277 |
| T-279 | Surcouches de sécurité dans le visualiseur (zone public, horizon) | P1 | T-277, T-003 |
| T-280 | Simulation de l'inertie des galvos et divergence | P3 | T-276, T-171 |
| T-281 | Fenêtres supplémentaires / multi-écran (`?vue=`) | P2 | T-270, T-275 |
| T-282 | Mode nuit et mode tactile | P2 | T-270 |
| T-283 | Mode spectacle (verrouillage) et protection contre les clics accidentels | P1 | T-270 |
| T-284 | Table unique des raccourcis clavier et aide « ? » | P2 | T-270 |
| T-285 | Annuler / rétablir et historique des modifications | P2 | T-286 |
| T-286 | Fichier projet `.lsproj` : ouvrir, enregistrer, récents | P1 | – |
| T-287 | Sauvegarde automatique et récupération après plantage | P1 | T-286 |
| T-288 | Versions du format (migrations) et versions nommées | P2 | T-286 |
| T-289 | Import partiel, profil de site et paquet d'export `.lspack` | P2 | T-286, T-288 |

Suggested order: T-286 → T-287 in parallel with T-270 → T-271/T-272/T-283,
and T-275 → T-276/T-277 → T-279. Everything else after.

Links to existing tasks: T-271 shows T-150's tempo and T-140's master
brightness when they exist (placeholders until then); T-272 aligns with the
APC40 profile (T-204) and click modes (T-155); T-277's per-output frames
are what T-012/T-170 produce; T-279 draws T-003's zones; T-286 stores
T-160 timelines, T-202 MIDI mappings and T-156 layers as optional sections.

---

## 8. Safety and IP notes

- The visualiser reads the **same** final frame (after calibration and
  safety) as the DAC; it never has its own output path and never arms.
- Tests and agents run preview-only (no `--device`), as for every task.
- Visual design is ours: no Pangolin/Laserworld screenshots, icons,
  names of effects or layouts are copied; we only reuse generic ideas
  (grid of cues, tabs, masters, beam view) that are common to the whole
  industry.
- three.js (MIT) vendored → listed in `docs/CONTENT_SOURCES.md`.
- The audience-exposure overlay is a design aid, not a compliance tool.

---

## Sources (accessed 2026-09-27)

Pangolin (public wiki, blog, forum):
- BEYOND 3D Preview: http://wiki.pangolin.com/doku.php?id=beyond:3d-preview
- BEYOND and QuickShow 5.5 release: https://pangolin.com/blogs/news/beyond-and-quickshow-5-5-release
- PLSN on 5.5: https://plsn.com/featured/pangolin-beyond-quickshow-5-5-are-here/
- Forum, perspective preview: https://forums.pangolin.com/threads/laser-output-preview-in-perspective.29229/
- BEYOND grids and pages: https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages
- BEYOND cue grid: https://wiki.pangolin.com/doku.php?id=beyond:cue_grid
- QuickShow cue grid: https://wiki.pangolin.com/quickshow:cue_grid
- QuickShow main control window: https://wiki.pangolin.com/doku.php?id=quickshow%3Amain_control_window
- PangoScript examples (autosave): https://wiki.pangolin.com/doku.php?id=examples%3Apangoscript
- Forum, data and settings backup: https://forums.pangolin.com/threads/data-and-settings-backup-for-beyond.1590/
- Forum, export a show: https://forums.pangolin.com/threads/creating-a-backup-or-export-a-show.25261/
- Forum, dual-monitor setup: https://forums.pangolin.com/threads/dual-monitor-setup-in-beyond.1980/
- Pangolin × Capture: https://pangolin.com/blogs/news/pangolin-partners-with-capture-sweden-for-visualization-of-laser-shows
- Visualization software collection: https://pangolin.com/collections/visualization-software
- Depence laser module: https://pangolin.com/products/depence-laser-module
- Realizzer 3D video: https://wiki.pangolin.com/doku.php?id=beyond:video:laser_show_visualization_with_realizzer_3d

Visualisers:
- Depence laser simulation: https://www.syncronorm.com/products/depence2/visualization/laser
- Depence R4 help, laser: https://help.depence.com/depence-construction/depence-laser
- L8: https://l8.ltd/m/ · PLSN "From LightConverse to L8": https://plsn.com/articles/software-solutions/from-lightconverse-to-l8/

Laserworld / Showcontroller:
- Showcontroller: https://www.showcontroller.com/en/
- About Showcontroller: https://www.showcontroller.com/en/manual/1-showcontroller/1-2-about-showcontroller.html
- Realizzer 3D support: https://www.showcontroller.com/en/showcontroller-software/support-of-realizzer-3d.html
- Capture demo stage: https://www.laserworld.com/en/download-file-1772-Showcontroller_Capture_Demo.html
- Showcontroller LIVE: https://www.showcontroller.com/en/showcontroller-software/showcontroller-live

MadMapper:
- MadLaser: https://madmapper.com/extensions/madlaser
- MadLaser guide (PDF): https://madmapper.com/files/MadLaser%20Guide.pdf

Operator UX:
- grandMA3 Oops menu: https://help.malighting.com/grandMA3/2.3/HTML/ws_oops_overlay.html
- grandMA3 Oops key: https://help.malighting.com/grandMA3/2.1/HTML/key_oops.html

WebGL techniques:
- Additive-cone god rays and volumetric spots: https://threejsdemos.com/demos/lighting/godrays
- Volumetric light scattering in three.js: https://medium.com/@andrew_b_berg/volumetric-light-scattering-in-three-js-6e1850680a41
- Volumetric lighting with post-processing and raymarching: https://blog.maximeheckel.com/posts/shaping-light-volumetric-lighting-with-post-processing-and-raymarching/
- three.js (MIT licence): https://github.com/mrdoob/three.js
