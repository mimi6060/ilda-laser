# feat/layers — T-156 four cue layers, dimmer / mute / solo, point budget

## What / why
Put a beam over a tunnel, or text over an abstract, and set the level of
each. A laser has no transparency: layers add up, and every layer costs
points, so stacking cues lowered the frame rate (flagged in the T-155
review). This adds layers on top of the existing cue deck (no rewrite).
- `studio/src/layers.rs` (new): `Layer { dimmer, mute, solo }`,
  `Mixer { layers: [Layer; 4], point_budget }` (default 750 = 30 kpps at
  40 frames/s), saved to `studio-data/layers.json` (same once-a-second
  throttle as `live.json`, so MIDI faders don't hammer the disk).
  `mix()` draws the rendered looks layer 1 → 4 (a layer's cues keep their
  order) with blanked, densified travel (`engine::join_looks`), applies the
  dimmer (0 = layer off), mute and solo (solo on any layer = only solo
  layers; mute wins). Returns a `MixReport` (demand, points, decimated,
  dropped layers, points per layer).
- **Point budget**: if the frame exceeds `point_budget`, first the lit
  points of every layer are thinned evenly, as little as needed and never
  below 1 point in 2 (T-002's `lit_step` doesn't exist yet, so it's a
  regular decimation). Blanked points and both ends of every lit stroke are
  always kept, so the beam never draws across a blanked jump. If that isn't
  enough, the highest-numbered layer is cut, and so on. The lowest audible
  layer is never cut (thinned as far as allowed instead): a single heavy
  cue still plays rather than going dark.
- `cues.rs`: `CueSlot.layer` (1..4, `None` = 1, saved in `grid.json`),
  `ActiveCue.layer` (taken when the cue starts), `clear_layer()`,
  `layered_looks()`. **« Un cue » now replaces latched cues of the same
  layer only**, otherwise layers would be useless without « Multi ». With
  every cue on layer 1 (the default) it behaves exactly as before.
  Flash / solo / groups / limiter are untouched: a flash in « Un cue » mode
  still takes over the whole output, cue-solo still hides everything,
  releases return to exactly what played (new unit test with a flash on
  layer 4). Groups and `max_active` stay global.
- Engine (`main.rs`): every visible look is still rendered (muted layers
  keep animating, so they come back in motion), then `layers::mix`
  replaces `join_looks`. Live modifiers, colour, calibration clamp,
  arm/disarm and blackout apply to the mixed frame exactly as before.
- Controls (external, for MIDI faders/buttons): `layer.<1-4>.dimmer`
  (continuous 0..1), `layer.<n>.mute`, `layer.<n>.solo` (toggles),
  `layer.<n>.clear` (trigger: stops every cue of the layer, held ones
  too). `current()` reports dimmer/mute/solo. `docs/controls.md`
  regenerated.
- API: `GET/POST /api/layers` (whole mixer, sanitised: budget 100..10000,
  dimmer 0..1). `/api/cues/slot` accepts `layer`. `/api/frame` gains
  `layers: { mixer, mix }` and a `layer` per active cue.
- UI: four strips *Calque 1–4* under the grid (vertical *Gradateur*,
  *Muet*, *Solo*, *Vider*, points per layer or « coupé »), a counter
  *Points : demand / budget* that turns orange over budget, and a warning
  « budget dépassé : points espacés » / « calque 4 coupé ». Strips follow
  server values (MIDI/API changes show up; a slider being dragged is left
  alone). « Propriétés du cue » gains *Calque*; cells show `C2`..`C4`.

## Adapted / left out
- `Layer.modifiers: LiveModifiers` is not added: per-layer modifiers are
  T-144 (which depends on this task). The mixer is where they'll plug in.
- The point budget isn't editable in the UI (API / `layers.json` only).
- Layer dimmers are not LFO targets (the LFO `slot()` table only knows
  look/master fields); easy to add later.

## Testing
- `cargo test -p laser-studio`: 174 passed, 2 ignored (+12).
  `layers.rs`: order + blanked travel between layers, dimmer scaling and
  dimmer 0, mute/solo combinations, over budget → thinned with stroke ends
  kept, 4 × 600 points → layers 4 and 3 cut and frame ≤ 750, a single
  layer is never cut, `decimate` keeps blanked points and never joins two
  strokes, JSON defaults/sanitize. `cues.rs`: « Un cue » replaces within a
  layer only, `clear_layer` (held included), flash on another layer
  returns to what played in both modes, layer in `grid.json` (1 and out of
  range dropped). `controls.rs`: layer controls drive the mixer,
  `current()`, `layer.5.*` unknown, `layer.2.clear` leaves the layer-1 cue
  as primary.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `studio/e2e/tests/layers.spec.ts` (4 tests, also run
  `--repeat-each 3`): strips + counter; cue set to *Calque 2* via the
  right-click menu, two cues on two layers both in `/api/frame`, dimmer 0 →
  only layer 1 lit, *Solo* 2 → only layer 2, *Muet* 1, *Vider* 2; dimmer
  and mute set through `/api/control` show on the strips; four heavy cues
  (400–700 points each) on layers 1–4 → frame ≤ 750 over several frames,
  layer 1 never cut, counter orange, warning shown, layer 4 « coupé ».
  Full suite: 55 passed, 2 skipped (existing `fixme`).
- Checked the strips by screenshot on a throwaway preview-only instance
  (`--port 8097 --no-midi`, scratch data dir, no `--device`).

## Risks
- Behaviour change: in « Un cue » mode a cue on another layer no longer
  replaces the others. Only visible once a cue is given a layer ≥ 2.
- The budget also applies to a single cue: 12 catalogue cues render more
  than 750 points alone (max ≈ 940) and are now thinned just enough to
  fit (about 4 lit points in 5) instead of dropping to ~32 frames/s.
  Visible as slightly coarser curves on those cues. Setting `point_budget` higher in
  `layers.json` restores the old output.
- `layers.json` persists mute/solo: a solo left on is still on after a
  restart (the strip shows it).
- The mix report in `/api/frame` is from the last rendered frame, so it
  can lag a cue change by one frame (the e2e polls for it).

## Review
