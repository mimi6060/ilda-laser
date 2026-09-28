# feat/fan-gens — T-102 festival fan generators

## What / why
Beam fans are the backbone of festival laser shows
(`docs/research/festival-looks.md` section A). This branch adds five
beat-synced fan generators built on the T-100 toolkit (`beat.rs`, `GenCtx`).
Each one is our own maths, written from the research's descriptions in
words. Nothing is copied from any vendor's content.

New module `studio/src/fans.rs`. `generators::generate` falls through to it
for unknown names. The names are **appended** to `GENERATOR_NAMES`, and
nothing that existed before changes: the 20 old generators, the 202 cue ids
and their frames. All five are `dots`, move from `ctx.beat_pos` (their motion
is defined in beats, whatever `beat_sync` says), are pure functions of
(params, ctx), and are clamped to x in -1..1 and y in `HORIZON`(0)..1.
Widths follow the look size (`scale`), heights are frame units above the
horizon, and there are at most 32 beams.

| Generator | UI label | Parameters |
|---|---|---|
| `fan` (looks 1, 4) | Éventail | N beams on `y = b`, half-width = size. `a` = pump depth (0 = static, 1 = full): the width opens on each beat over 1/16 beat, then closes towards `w_min = 0.02` (τ = 1/6 beat, 95 % closed at 1/2 beat). |
| `fan_sweep` (look 2) | Éventail balayé | A half-size fan whose centre is `a·swing(cycle)` over `period_beats`, per group (`group_mode`: mirror = the groups cross like scissors, offset = a travelling wave). |
| `fan_tilt` (look 3) | Éventail qui se lève | Rises from 0.05 to 0.7 over the period (ease-in), then snaps back. `direction` -1 comes down, 0 goes up and down (ping-pong). Mirror groups move in opposite directions. |
| `fan_wave` (look 6) | Éventail vague | `y_i = b + a·sin(2π(x_i/λ − 0.5·beat_pos))`, λ = the fan's width. `a` ≤ 0.45, and `b` is lifted so the trough stays above the horizon. `beam_wave` is kept. |
| `positions` (look 9) | Positions au temps | One position per step (`steps_per_beat`): P0 wide fan high, P1 narrow fan at +15°, P2 narrow fan at −15°, P3 V with legs of N/2 beams at ±30° from vertical. Travel between positions is blanked (by `colorize` inside a frame, and by the output on frame wrap). |

`GenParams` gains `easing: Easing { Sine (default), Triangle, Trapezoid }`
(`serde(default)`, lowercase). `Easing::swing(phase)` lives in `beat.rs`.
All three shapes pass through 0, +1, 0, −1 at the quarter points, so
switching easing keeps the timing. **Trapezoid** holds each end for a
quarter of the cycle (`clamp(2·triangle)`).

UI (Effet tab):
- Five French labels in the generator list.
- A short help line under Nombre, Forme A and Forme B, and one in the tempo
  panel, shown only for the fans.
- A « Courbe du balayage » select.
- « ↔ aller-retour » added to Sens, with direction 0 now shown correctly.
- Forme A's step goes from 0.1 to 0.05, so 0.35 is exact.
- Picking a fan in the list starts it in tempo (`beat_sync` on) with its
  starting values (N, a, b, period, steps). This is written as a single
  `LOOK_EDITS` edit, so it fits develop's replayable edit queue.

No cues were added (the « Festival » page is T-110). No brightness flashing
was added. The fan pump modulates width, not intensity.

## Testing
- `cargo test -p laser-studio`: **278 passed**, 2 ignored, after rebasing on
  develop 6e83091 (+11 from this branch: 10 in `fans.rs`, 1 easing test in
  `beat.rs`). `cargo clippy -p laser-studio --all-targets -- -D warnings`:
  clean.
- Non-regression: the digest test `existing_generators_draw_exactly_what_they_did`
  now hashes `GENERATOR_NAMES[..20]` (the pre-T-102 list) explicitly, and its
  pinned value is unchanged. The cue-frame and cue-id fingerprints are also
  unchanged.
- Unit tests:
  - Every fan, over 5 parameter sets (including the UI's a=3 / b=2 and
    count 64), 5 scales and 80 beat positions (some negative), gives exactly
    N beams, all finite, in -1..1 and at or above the horizon.
  - `fan`: static at a=0; closed on the beat, fully open 1/16 beat later,
    mostly closed at 1/2 beat.
  - `fan_sweep`: at period 4, the same frame at beats 0, 4 and 8; ±a at
    beats 1 and 3; reverse direction; trapezoid holds.
  - Mirror groups sweep in opposition (equal and opposite shifts).
  - `fan_tilt`: ease-in, top at the end of the period, instant return,
    ping-pong, descending.
  - `fan_wave`: a wavelength every 2 beats, inverted at 1 beat, amplitude
    and lift.
  - `positions`: 0.999 → P0 and 1.000 → P1 (and 2.999 / 3.0), cycling back
    to P0, the four positions all different, ±15° slopes, V legs at 30°,
    and 2 steps per beat.
  - Frames are the same at 90 and 174 BPM at the same beat.
- Point counts (after `colorize` + `densify`, worst of 64 frames over 4
  beats):

  | Generator | N=8, size 0.7 | N=16, size 0.7 | N=64→32, size 1.0 |
  |---|---|---|---|
  | fan | 158 | 273 | 514 |
  | fan_sweep | 137 | 243 | 506 |
  | fan_tilt | 158 | 273 | 533 |
  | fan_wave | 182 | 289 | 524 |
  | positions | 158 | 273 | 514 |

  All counts are under develop's layer budget of 750 points (40 fps at
  30 kpps). The test `fans_hold_30_fps_at_30_kpps` asserts the budget
  against `layers::DEFAULT_POINT_BUDGET`. The existing ≤ 3000 test also
  covers the new names.
- e2e: full suite **63 passed**. New case in `content.spec.ts`: select
  « Éventail balayé » and check that `beat_sync`, a=0.35 and N=8 are set,
  the tempo panel and the help are visible, the easing select writes
  `easing`, `/api/frame` has at least 80 lit points all at y ≥ 0, and the
  fan's centre moves between two reads. Preview only: the harness's own
  studio, with `--no-midi` and no `--device`.

## Risks
- "Above the horizon" means y ≥ 0 in the look's own frame. The user's
  rotation (`rotation_speed`, live rotation), position offsets and
  calibration are applied afterwards and can still bring beams down. A real
  audience-safe zone is a separate safety task.
- Some numbers are my own reading of the task:
  - Trapezoid = a quarter of the cycle held at each end.
  - Pump decay τ = 1/6 beat.
  - P0 sits 0.2 above `b`; the narrow fans are 0.3 × the size wide.
  - The +15° tilt is counter-clockwise, so the right end is up.
  - The V apex is 0.05 above the horizon, with legs as long as the size.

  All are constants at the top of `fans.rs` and easy to tune by eye.
- The fans follow the tempo clock even with « Tempo du look » off. With it
  off, `gate_beats` doesn't apply, which matches T-100's gate semantics.
- `positions` at 8 steps per beat and 250 BPM snaps position about 33 times
  a second. That is motion (the beams stay lit, with blanked jumps), not
  flashing, but T-101 may want to cap it.
- `fan_tilt` ignores `a` and `b`; the help says « Non utilisé ».

## Review
