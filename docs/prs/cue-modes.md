# feat/cue-modes — T-155 cue trigger modes, exclusive groups, limiter

## What / why
A click used to replace the look. A laserist wants to toggle cues, flash
them while a key is held, solo one, restart one, and stack several.
- `studio/src/cues.rs` (new): `CueDeck` = list of `ActiveCue`s plus the
  grid's trigger settings (`click_mode`, `multi`, `max_active`, per-cue
  `slots`). Cues are *latched* (stay until stopped) or *held* (flash/solo,
  removed on release).
  - **Basculer** (default): press 1 starts, press 2 stops; releases ignored.
  - **Flash**: held while down. In « Un cue » mode it takes over the output
    (the others are hidden, not stopped); in « Multi » it adds on top.
  - **Solo**: held while down, only held solo cues are shown; release
    brings the others back.
  - **Relancer**: every press starts the cue over (new instance → fresh
    animator).
  - **Un cue / Multi**: a latched start replaces all latched cues, or adds.
  - **Groups 1..8** (per cue): a latched start stops the latched cues of
    its group; a held flash in a group hides its group-mates while held.
  - **Limiter** `max_active` (default 4, 1..16): beyond it the oldest
    *latched* cue stops. Flashes neither count nor get stopped, so a flash
    release always returns to exactly what played before.
  - `studio-data/grid.json`: `click_mode`, `multi`, `max_active` and the
    per-cue `slots` (`mode`, `group`), keyed by preset id. The playing list
    is not saved.
- Rendering (`main.rs`, `engine::join_looks`): one `Animator` per playing
  cue instance; the visible looks are rendered and chained with **blanked,
  densified travel** between them. Live modifiers, calibration clamp,
  arm/disarm and blackout apply to the joined frame exactly as before.
- The newest cue is the *primary*: its look lives in `Shared::settings`, so
  the look panel, `/api/settings` and `look.*` controls edit the cue on top
  (edits survive a flash over it). `controls::with_deck` keeps the two in
  sync. The operator's look brightness applies to every stacked cue.
- Scenes, playlist, manual look: `Shared::look_on`. A scene/playlist start
  stops all cues and shows the look (`controls::show_look`). A latched cue
  takes over (stops the playlist, as before); a *flash* over a scene or the
  playlist does not — the scene comes back on release (the playlist's next
  scene is parked meanwhile). Stopping the last latched cue leaves the
  output dark (« clic 2 arrête »); editing the look shows it again.
- Controls: `grid.<page>.<row>.<col>` are now **Momentary** (value ≥ 0.5 =
  press, < 0.5 = release: a MIDI note-on/off maps straight onto flash), and
  report whether their cue plays (LED feedback via `/api/control-values`).
  New: `cue.mode` (Choice Basculer/Flash/Solo/Relancer), `cue.multi`,
  `cue.max_active`, `cue.stop_all`. `docs/controls.md` regenerated.
- API: `POST /api/cue {id, down, mode?}` (press/release, optional mode
  override), `GET /api/cues` (deck settings + slots), `POST /api/cues/slot
  {id, mode, group}`. `/api/frame` gains `cues: {active, shown, click_mode,
  multi, max_active}`. `/api/presets/play` = press with « Relancer ».
- UI: grid bar *Basculer / Flash / Solo / Relancer*, *Un cue / Multi*, *Max*,
  *Tout arrêter*. Cue buttons press on pointerdown and release on
  pointerup/pointercancel anywhere in the window. Letters press on keydown
  and release on keyup (auto-repeat ignored, so a held key no longer
  toggles on and off); `Maj` + letter / `Maj` + click = flash. Window blur
  releases everything held. Right click → « Propriétés du cue » (mode,
  groupe); cells show a small tag (F/S/R/B, G1..G8). Latched cues are
  green, held ones orange, playing-but-hidden ones dimmed.

## Adapted / left out
- The grid layout is still derived from the catalogue (T-145: 8 category
  pages, 5×8 ids); `grid.json` stores properties per preset id rather than
  a free-form grid of `CueSlot { cue, … }`. Moving cues between cells, and
  10 pages, belong to a grid-editing task.
- `CueSlot.layer`, `transition`, `quantize`, `modifiers`, `vlj_skip` are
  not added: layers are T-156, the others belong to their own tasks.
  `ActiveCue` already records `started_s` / `started_beat` for quantize.
- No e2e tests: `studio/e2e/` does not exist on develop yet.

## Testing
- 89 unit tests (+18): `cues.rs` covers toggle, single vs multi, flash in
  both modes returning to the same instances, deck/slot flash modes, flash
  of a running cue, solo (new and running cue), restart, groups, flash
  masking its group, limiter (flashes excluded, `max_active` lowered),
  `looks()` primary/brightness/dark-after-stop, `grid.json` round trip and
  old/empty file defaults. `controls.rs`: grid momentary toggle + LED value,
  flash pad returning to the edited cue below, flash over a scene giving
  the scene back, `cue.*` controls and the limiter via grid ids.
  `engine.rs`: `join_looks` travels blanked, within `MAX_STEP`.
- Clippy `-D warnings` clean; UI script parses (`new Function`).
- Throwaway instance (`--port 8095 --data-dir <scratch>`, no `--device`):
  curl run of multi, solo hold/release, group replace, grid control,
  stop-all, grid.json written. In Chrome: pointerdown/up toggle,
  Shift+pointer flash (orange, only it shown, previous cue back on
  release), Shift+key hold with a repeat event then keyup, key toggle,
  « Propriétés du cue » menu saving mode (tag updates).

## Risks
- If the browser dies while a flash is held, the flash stays until
  « Tout arrêter » (blur releases cover the usual cases).
- Several stacked cues multiply points: the frame rate drops with 4 heavy
  cues (the limiter bounds it; a point budget is T-156).
- The UI no longer clears the cue highlight when the look is edited: the
  cue keeps playing with the edit (it used to look "detached").
- Music-reactive settings of non-primary cues are frozen at their start;
  only brightness follows the operator live.

## Review

Reviewed by the architect (integrator). Merged after live-color; resolved
the engine-loop conflict so the joined multi-cue look goes through the
colour stage with user palettes. Checked in Chrome on the merged build:
cue press/release plays one cue, Fixe colour mode recolours it red, no
console errors. Accepted behaviour changes: stopping the last latched cue
blacks out the output (look_on); grid controls are momentary; edits
apply to the top cue. Follow-ups: a flash can stay held if the browser
dies mid-hold (T-252 heartbeat should release held cues); e2e coverage
once T-004 lands; point budget across stacked cues (T-156).
Verdict: APPROVED
