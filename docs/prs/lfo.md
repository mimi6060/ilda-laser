# feat/lfo — T-151 tempo-synced LFO modulators

## What / why
Pro effects are parameters that oscillate in time with the music (size
that breathes, rotation that swings, hue that turns). This adds master
LFO modulators that can drive any safe continuous control.
- `studio/src/lfo.rs`: `Modulator { target, wave, rate, depth, phase,
  offset, enabled }` (`#[serde(default)]`, default: `master.size`, sine,
  `Beats(4)`, depth 0.5). Waves: sine, triangle, square, saw up/down,
  random (sample-and-hold: a new value each cycle, from a hash of the cycle
  index and the target, so it is stable from run to run). `rate` reuses
  `live::Rate` (`Beats(n)` = one cycle every n beats, or `Hz`).
- **Phase comes from the one clock**: `Rate::cycles(t, tempo.beat_at(t)) +
  phase`, with nothing accumulated per LFO. Two identical modulators stay in
  phase forever, and a BPM change (`set_bpm` re-anchors the origin) changes
  the speed without a jump.
- Value = base + depth × (wave + offset) × (max − min) / 2, clamped to
  the control's range. Several modulators on one control add up.
- **The stored values are never written.** The engine clones `settings`
  and `live` every frame, `lfo::modulate` moves the copies, and the frame
  is rendered from them. Faders, MIDI feedback, `/api/live`, saved scenes
  and `live.json` all keep the operator's base value.
- **Allowed targets** (26): continuous **and** external registry controls
  that have a slot in `lfo::slot`, which is the allow-list (`look.size /
  brightness / rotation_speed`, `audio.size/rotate/flash`, `master.*`
  geometry, speed, perspective, `master.color.hue/spread/offset/red/green/
  blue`). Excluded: `transport.*` (arm, blackout), `tempo.*` (BPM would
  modulate the clock itself), `cue.*`, grid/page, choices and toggles,
  `master.color.rate_hz`. Calibration isn't in the registry and can't be
  reached. **Brightness can only dim**: on `look.brightness` and
  `master.brightness` the fader is a ceiling (value = min(modulated, base)).
  Order is unchanged: live modifiers → calibration clamp.
- Colour targets rebuild the active colour override from the modulated
  `color_params`, so the running mode (Teinte, Fixe…) follows.
- Storage: `LfoStore` in `Shared.lfos`, `lfos.json` in the data dir, at
  most 16 modulators, numbers sanitised (depth 0..1, phase wraps into 0..1,
  offset −1..1, beats 1/16..64, Hz 0..20, NaN → default), and unknown
  targets dropped on load. `master.reset` doesn't touch LFOs.
- API: `GET /api/lfos` returns `{lfos, targets:[{id,label,group,min,max}],
  max}`. `POST /api/lfos` takes the whole list and answers 400 with a French
  message when it's invalid (for example target `transport.arm`).
  `/api/frame` gains `lfos: [{phase, value}]` for the graphs.
- UI: a « Modulateurs » section under « Direct ». *+ Modulateur* adds one,
  and each row has an animated mini graph (one cycle of the wave, with a dot
  at the current position), *Cible* (menu of the allowed controls),
  *Forme*, *Période* (1/4, 1/2, 1, 2, 4, 8, 16 temps or *Hz (libre)* with a
  Hz field), *Profondeur*, *Phase*, *Actif* and ×.

## Testing
- 117 unit tests (+12 in `lfo.rs`):
  - every wave at phases 0, ¼, ½, ¾ (plus triangle at ⅛);
  - random holds for a whole cycle, is reproducible and spreads around 0;
  - a 4-beat sine on `master.size` repeats every 4 beats (±1e-4, including
    200 s later), with a peak of 1.5 and a trough of 0.5;
  - two modulators created apart are in phase;
  - a BPM change 120 → 174 has no jump and the new period is right;
  - depth 0 or disabled leaves the control unchanged, even with an offset;
  - values clamp, modulators add up, and offset −1 stays below the base;
  - brightness never goes above the fader but does dim;
  - `transport.arm`, blackout, `tempo.bpm`, `cue.max_active`, toggles and
    grid can't be targets, and each of the 26 allowed targets really moves;
  - a hue LFO updates the active `Hue` override;
  - the store validates, sanitises and reloads;
  - serde defaults and format.
- `cargo clippy -p laser-studio --all-targets -- -D warnings` is clean,
  and the UI script parses (node + `new Function`).
- Preview instance (`--port 8099`, temp `--data-dir`, no `--device`):
  `transport.arm` as a target → 400. With a 1-beat sine at depth 1 on
  `master.size`, the frame's max |x| goes 0.95 → 0.47 → 0.15 → 0.99 across
  successive `/api/frame` calls, while `live.size` stays 1.0, `armed`
  stays false and `lfos.json` is written.
- No e2e: `studio/e2e/` isn't on develop yet (feat/e2e). The e2e the task
  asks for (add a modulator → `/api/frame` varies) should be added there.

## Risks / follow-ups
- `master.rot_*.speed` is modulated in the registry's units (±720 deg/s).
  When *Synchro tempo* is on, those speeds are turns per bar, so a deep LFO
  gives a very fast spin (it's still clamped). A later pass could scale by
  the sync mode.
- Rotation-speed and animation-speed targets modulate a speed, so the angle
  is integrated by the existing rotation state (as before), not by the LFO.
- Cue-parameter targets and LFOs stored in cues are for T-157, and audio
  routing is T-153.
- When a colour target is modulated in *Normal* colour mode, the base
  colour params change but nothing is visible, which is expected.
- The UI posts the whole list on every slider move (at most 16 small
  objects, each one written to disk). This is the same pattern as palettes.

## Review
