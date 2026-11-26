# feat/live-color — T-141 live colour on top of any cue

## What / why
The laserist recolours the whole show to follow the lighting desk or the
energy of the track, without changing cue.
- `studio/src/live.rs`: `ColorOverride` in `LiveModifiers` (`color`):
  *Normal*, *Fixe* (RGB, each point keeps its intensity = max(r,g,b)),
  *Teinte* (keeps S and V, replaces H), *Palette* (*Plus proche* by RGB
  distance, or *Pas à pas* = next colour at each colour change along the
  path, with a rotating *Décalage*), *Arc-en-ciel* (hue along the path,
  *Étalement* 0..4 cycles, scrolling at a `Rate`), *Chenillard* (palette
  colours advancing one step every N beats, spread *Tout / Par trait / Par
  point*, travelling forward along the path).
- `Rate::{Hz, Beats}` is a pure function of the clock: the engine sets
  `LiveState::set_clock(t, tempo.beat_at(t))` every frame, so chases and
  synced rainbows land exactly on `TempoClock` beats and never drift.
- Order in `live::apply`: geometry → colour → master dimmer (so the
  master brightness still applies last among the live modifiers; then
  calibration as before). Blanked points (intensity 0) are never touched.
- `LiveModifiers::default()` stays a bit-exact identity. `color_params`
  keeps every colour setting even for inactive modes, so switching mode
  back and forth brings the operator's values back; a setting control
  never switches mode by itself, only `master.color.mode` does.
- `Palette { name, colors }` (1..=16 colours) — meant as the one palette
  type of the app (T-130 can reuse it). 8 built-ins of our own: Froid,
  Chaud, Feu, Océan, Néon, Forêt, Tricolore, Blanc pur. Up to 8 user
  palettes in `studio-data/palettes.json` (`PaletteStore`, validated,
  saved on every change). Palette index = built-ins then user ones; a
  missing user palette leaves colours unchanged.
- API: `GET /api/palettes` → `{builtin, user}`, `POST /api/palettes`
  (whole user list; 400 with a French message when invalid). `/api/live`
  and `/api/frame` now carry `color` / `color_params`.
- Controls (all external): `master.color.mode` (choice), `.hue`,
  `.palette` (choice: built-ins + « Perso 1..8 »), `.palette_mode`,
  `.offset`, `.rate` (choice 1/8..4 beats), `.rate_hz`, `.spread`,
  `.chase_spread`, `.red/.green/.blue`. New `Unit::Hz`. `docs/controls.md`
  regenerated. `master.reset` also resets colour.
- UI « Direct » → bloc *Couleur*: mode buttons, colour picker (→ Fixe),
  clickable/draggable hue strip (→ Teinte), palette thumbnails (→ Palette
  unless a chase is running), *Plus proche / Pas à pas*, *+ Palette*
  (prompt name + hex colours) and × to delete a user palette, *Décalage*,
  *Pas* 1/8..4, *Étalement*, chase spread. Right-click in the block =
  *Normal*.

## Testing
- 87 unit tests (+16): Normal bit-exact on 5 catalogue cues even with
  colour settings changed; Fixe red keeps intensity and blanking; master
  brightness after colour; hue keeps S/V (grey untouched); HSV round trip;
  palette nearest (pure green → amber of « Chaud », dim green keeps its
  intensity, offset rotates); palette step; rainbow spread + beat and Hz
  scrolling; chase at 1 beat over 600 frames at 128 BPM with a nudged
  phase: colour changes on exactly the frames that cross a beat (21
  changes in 10 s); chase by stroke / by point; user palette indexing and
  missing palette; built-ins valid; user palettes survive a reload and
  invalid lists are refused; serde round trip + old `live.json` loads.
  Controls: modes remember settings, rate drives the active mode.
- `cargo clippy -p laser-studio --all-targets -- -D warnings` clean; UI
  script parses (`new Function`).
- Preview instance (`--port 8094`, temp `--data-dir`, no `--device`):
  `master.color.mode=1` → all 346 lit points of `/api/frame` pure red;
  user palette POSTed, studio restarted → still there; chase on the user
  palette alternates red/blue with the beat; invalid palette → 400. In
  Chrome: Arc-en-ciel, hue strip click (→ Teinte, blue circle), right-click
  → Normal all work.

## Risks
- No e2e suite exists in this tree (`studio/e2e/` absent), so the e2e
  « bouton Fixe → /api/frame » test from the task is covered by the manual
  API check above only.
- *Pas à pas* compares consecutive lit colours with a 1e-3 tolerance;
  cues with smooth gradients change colour at nearly every point (by
  design, but it can look busy).
- Rainbow spread is by point index, not arc length (densified frames make
  that close enough).
- Layers/cues (T-144) will need their own `ColorOverride`; only the master
  one exists.
- The *Pas* buttons show the chase step unless Arc-en-ciel is active;
  there is no UI for Hz rates (control `master.color.rate_hz` only).

## Review

Reviewed by the architect (integrator). Checked: colour stage runs after
geometry and before the master dimmer and calibration; unlit points stay
unlit; default is a bit-exact identity (dedicated test); beat-synced chase
reads the single T-150 clock; palette file validated; no path can arm the
laser. Accepted deviations: extra `color_params` field (restores per-mode
values) and serde tag `kind`. Follow-ups: e2e test once T-004 lands, a UI
for Hz rates, replace prompt()/confirm() in the palette editor.
Verdict: APPROVED
