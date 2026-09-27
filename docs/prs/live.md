# feat/live — T-140 master live modifiers (+ first part of T-143 panel)

## What / why
The laserist's constant moves while a cue plays, on top of any cue.
- `studio/src/live.rs`: `LiveModifiers` (brightness, size, size X/Y with
  flips, position, rotation X/Y/Z angle + speed, tempo sync, momentary
  reverse, perspective, animation speed) and `LiveState` (accumulated
  spin). Applied after the look, before calibration (so the clamp and the
  future safety stage still come last). Identity by default (bit-exact).
- Rotation presets Stop/Lent/Moyen/Rapide: 0/30/90/270 °/s, or with
  « Synchro tempo » 0/¼/1/2 turns per bar from the T-150 clock. Switching
  sync keeps the same step.
- Animation speed multiplies generator time (0 freezes).
- Persisted to `studio-data/live.json` (at most once per second, so MIDI
  faders don't hammer the disk). `GET/POST /api/live`; every field is a
  control: `master.brightness/size/size_x/size_y/pos_x/pos_y`,
  `master.rot_{x,y,z}.{angle,speed}`, `master.rot.preset` (new Choice
  kind), `master.rot.sync`, `master.rot.reverse` (momentary),
  `master.perspective`, `master.speed`, `master.reset`.
- **Renamed** the look-level controls from `master.*` to `look.size`,
  `look.brightness`, `look.rotation_speed` (no mappings existed yet; they
  shipped a few hours earlier in T-145).
- UI « Direct » panel: rotation presets, synchro tempo, hold-to-reverse
  (button or `<` key), master size, position X/Y, 3D tilt X/Y, animation
  speed, master brightness, reset. Sliders send their control id directly.
  Fixed a race where a page-tab click could be undone by an older frame.

## Testing
- 71 unit tests (+9): identity on 5 catalogue cues, size ×2 and X flip,
  90 °/s → 90° after 1 s and reverse back to 0, 1 turn/bar at 120 BPM →
  180° after 1 s, rotation maths, Y-rotation 3D collapse, position and
  dimmer, 2000 points well under budget; controls for presets/sync/reset.
  Clippy `-D warnings` clean.
- In Chrome on a throwaway instance: cue click, master size 150 % →
  extent grows (clamped at 1 by calibration), Rapide + Synchro tempo →
  2 turns/bar, live.json written; tab click plays the cue of the clicked
  page; a server-side page change (MIDI/API) shows up in the UI.

## Risks
- Speed-0 freeze is exercised through the engine loop, not a unit test.
- Colour (T-141), strobe (T-142) and the full T-143 panel still to come.

## Review
Architect self-review (no separate reviewer agent).
Verdict: APPROVED
