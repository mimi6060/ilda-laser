# feat/sheet-gens — T-106 festival sheet generators

## What / why
Sheets are the calm looks of festival sets: a ceiling of light over the
crowd in the breakdown, curtains for a DJ entrance, falling lines after a
drop (`docs/research/festival-looks.md` section C, looks 19–26). This branch
adds eight beat-synced **line** generators on the T-100 toolkit (`beat.rs`,
`GenCtx`), following the fan-gens structure. They are our own maths,
written from the research's descriptions in words. Nothing is copied from
any vendor's content.

New module `studio/src/sheets.rs`. `generators::generate` falls through to
`fans`, then to `sheets`. The names are **appended** to `GENERATOR_NAMES`
after the fans. Nothing that existed before changes: the 25 old generators,
the cue ids and their frames. All eight are pure functions of (params,
ctx), move from `ctx.beat_pos`, and are clamped to x in -1..1 and y in
`fans::HORIZON` (0)..1. Widths follow the look size (`scale`, capped at 1).

**Above the audience, never at it.** Every sheet is placed in y ≥ 0 of the
look's frame by construction (heights are "`b` above the horizon"), and the
final clamp is only a guard. No look is meant to scan the audience
(research §F). The blade cuts the part of its line that would dip below the
horizon instead of drawing it.

| Generator | UI label | Behaviour (parameters) |
|---|---|---|
| `ceiling` (look 19) | Plafond liquide | A flat line at `b` (0.05–0.6) above the horizon. Ripple of amplitude `a` (0–0.03), 2 wavelengths across, travelling 0.25 cycle/beat. With `beat_sync`, the brightness breathes `0.7 + 0.3·(½ + ½·sin(2π·beat/8))`. |
| `blade` (look 20) | Lame | A line of half-length = size through the centre, raised to `b`, tilting ±30° over `period_beats` (UI default 16) with `easing`. The part below the horizon is cut off and the angle is kept. |
| `curtain` (look 21) | Rideaux | `count` ≤ 5 vertical strokes, 0.6 high from `b` above the horizon, 0.35 apart at size 0.7 (spacing scales with size), centred. `a` (≤ 0.3) slides them sideways over the period. Drawn up/down alternately. |
| `waterfall` (look 22) | Cascade | Short horizontal strokes fall linearly from y 0.9 to `b` in exactly 2 beats. A new one starts every step (`steps_per_beat`; UI default 2 = every ½ beat, 4 alive). They are placed in `count` lanes (≤ 8) in a staggered order (even lanes, then odd). |
| `scanner` (look 23) | Scanner | One pass per `period_beats` (UI default 4 = a bar), linear. `a` < 0.5: a horizontal bar rising and falling in a box `[b, b+size]` above the horizon. `a` ≥ 0.5: a vertical bar crossing the width. `loop_mode` sets ping-pong or wrap, and `direction` reverses it. |
| `slats` (look 24) | Lamelles | A line at `b` cut into `count` (2–16) segments, 50 % lit in runs of `a` segments (UI default 2). The pattern moves one segment per step (UI default 2 steps/beat = every ½ beat). Dark segments are still visited, blanked, so the geometry is stable. |
| `aurora` (look 25) | Aurore | A line whose height is a weighted sum (0.5/0.3/0.2) of three sines at 0.03, 0.05 and 0.08 cycle/beat, amplitude `a` ≤ 0.25 around `b`. The trough is lifted clear of the horizon. The UI starts it in « Dégradé ». |
| `grid` (look 26) | Grille | `count` horizontal and `a` vertical lines (≤ 6 + 6) in the box `[b, b+size]`, scrolling one line spacing per beat and wrapping. Drawn as a serpentine. |

Data model: `GenParams.loop_mode: LoopMode { Wrap, PingPong }`, default
PingPong, `serde(default)`, serialised as `"wrap"` / `"ping_pong"`.
`LoopMode::travel(passes)` lives in `beat.rs` next to `Easing`.

### Deviation from the task text (please check)
The task asks for "`liquid_sky` v2" and a `grid_scan` option. I added them
as **new names** instead: `ceiling` (« Plafond liquide ») and `grid`
(« Grille »). `liquid_sky` (« Nappe ») and `grid_scan` (« Grille de
balayage ») are left byte-identical. Reasons:
- The brief says append-only, with existing generators and digests
  unchanged.
- Since T-100, `beat_sync: true` already has a meaning for `liquid_sky`
  (t from beats), and saved looks or evolving keys may use it. Changing it
  would change them.
- The old `liquid_sky` puts its line at `b·scale`, which can be below the
  horizon. A new name lets the v2 be above the horizon by construction
  without moving old looks.

So "`liquid_sky` sans `beat_sync` produit les mêmes points qu'avant" holds
in every mode, pinned by the existing digest test. The UI labels asked for
are the new generators' labels.

### UI (Effet tab)
- Eight French labels in the list.
- The Nombre / Forme A / Forme B / tempo help lines, as for the fans.
- Starting values when picked (in tempo, like the fans), plus an optional
  `mode` in `GEN_DEFAULTS` (aurora → Dégradé).
- A « Fin de passage » select (Aller-retour / Bouclé) in the tempo panel.
- Forme A's step goes from 0.05 to 0.01 and its readout shows 2 decimals,
  so the ceiling's 0.01–0.03 ripple can be set. This is finer for every
  generator, and no value changes.

No cues were added (the Festival page is T-110). No brightness flashing
was added.

## Testing
- Rebased onto develop `ad9c9fd`, which includes T-105 tunnels. Both
  groups are kept in `GENERATOR_NAMES`: fans 20..25, tunnels 25..29, sheets
  29..37. `generate` chains fans → tunnels → sheets. The tunnels' list test
  now slices `[25..29]`. The tunnels' `showGenHelp` step rule gives 0.01
  instead of 0.05 for non-rotating generators.
- `cargo test -p laser-studio`: **424 passed**, 2 ignored (+14 in
  `sheets.rs`). `cargo clippy -p laser-studio --all-targets -- -D warnings`:
  clean. `node --check` on the page script: OK.
- Non-regression: the digest `existing_generators_draw_exactly_what_they_did`
  is unchanged. It hashes `GENERATOR_NAMES[..20]`, which includes
  `liquid_sky` and `grid_scan`. The cue-frame and cue-id fingerprints are
  unchanged too. The fans' list test now slices `[20..25]`.
- Unit tests (`sheets.rs`):
  - Every sheet, over 5 parameter sets (UI defaults a=3/b=2, negative b,
    count 64, a=10, period 0, steps 0/8, reversed), 5 sizes (0 to 1.5) and
    80 beat positions: lines, non-empty, finite, |x| ≤ 1, **0 ≤ y ≤ 1**.
  - `ceiling`: at b ± a; back in place after 4 beats and inverted after 2
    (0.25 cycle/beat); b and a clamped. The breath is off without the tempo,
    and with it 0.85 / 1.0 / 0.7 at beats 0 / 2 / 6, repeating at 8.
  - `blade`: 0°, +30°, 0°, −30° at beats 0/4/8/12 (period 16), turning
    about the raised centre. When low, the end is cut at the horizon and
    the angle is kept.
  - `curtain`: count 9 → 5 strokes, vertical, x = −0.7…0.7 in steps of
    0.35, from y_h to y_h+0.6; static at a=0; slide of a at a quarter
    period.
  - `waterfall`: a line started at beat 3 is at 0.9, then 0.45 at beat 4,
    near the horizon just before beat 5, and **gone at exactly beat 5**
    (2 beats). Always 4 alive at 2 steps/beat. Lane order is 0,2,4,1,3,5.
  - `scanner`: bottom / middle / top at beats 0/2/4 (one pass per bar),
    back down by beat 8 (ping-pong). Wrap jumps back at beat 4. Reversed
    direction. Vertical mode crosses −w → w.
  - `slats`: `11001100` at beat 0, unchanged at 0.499, shifted by one
    segment at 0.5, by two at 1.0, back at 2.0. 50 % lit. The segments
    tile the line.
  - `aurora`: within ±a of b; always bends (spread > 0.12 at a=0.2); moves
    by less than 0.1 per beat; repeats after 100 beats; clamped and lifted.
  - `grid`: 4+4 lines. The set of rows at beat 1.3 equals the set at 0.3
    (one line per beat), with ¼ spacing per ¼ beat. Capped at 6+6, and no
    columns at a=0.
  - Strobe: each sheet, with two parameter sets (including 4 steps/beat,
    period 1, wrap), fed to the real `safety::StrobeLimiter` at 60 fps and
    180 BPM for 12 s. It is never "fast", and the output is never altered.
  - Same frame at 90 and 174 BPM at the same beat. `loop_mode` defaults to
    ping-pong when missing from old JSON.
- Point counts (after `colorize` + `densify`, worst of 64 frames over 4
  beats × 3 parameter sets, **size 1.0**, i.e. the largest):

  | Generator | Worst points |
  |---|---|
  | ceiling | 107 |
  | blade | 74 |
  | curtain | 188 |
  | waterfall | 283 (16 steps/beat → clamped to 4, 8 lines) |
  | scanner | 74 |
  | slats | 132 |
  | aurora | 107 |
  | grid | 730 (6 + 6 lines, box 0.98 high) |

  `sheets_hold_30_fps_at_30_kpps` asserts all of them against
  `layers::DEFAULT_POINT_BUDGET` (750).
- e2e: full suite **105 passed**, twice in a row after the rebase. A new case in
  `content.spec.ts`:
  1. Select « Plafond liquide ».
  2. Check `ceiling`, `beat_sync`, a=0.02 (readout « 0.02 »), b=0.15, and
     the help line.
  3. `/api/frame` has at least 50 lit points, all at 0.15 ± 0.02.
  4. Set B to 0.3 and « Fin de passage » to Bouclé, then save the scene
     « Plafond ».
  5. The scene's saved settings hold `ceiling`, b=0.3, `loop_mode: wrap`
     and period 16.
  6. Switch to the square, then play the scene: the ceiling comes back
     with its parameters and controls, drawn above 0.25.

  An early version of the test was flaky (1 in 4). It waited for "a
  shape" rather than "the square", so the square edit could still be
  unsent when the scene played and got replayed on top of it. It now waits
  for the square. Everything ran preview only: the harness's own studio,
  no `--device`.

## Risks
- "Above the horizon" means y ≥ 0 in the look's own frame. The user's
  rotation, live rotation, position offsets and calibration are applied
  afterwards and can still bring a sheet down. T-101's `blank_low_beams`
  only blanks *beams* (coincident dots), not lines, so a sheet rotated
  below the horizon is **not** blanked by it. A real audience-safe zone for
  lines is T-003/T-255. The ceiling is deliberately close above heads
  (0.05 minimum), so the zone matters most there.
- The grid at size 1.0 with 6+6 lines is 730 points, close to the 750
  budget. With other layers on top, the mixer's budget logic decides (as
  for any heavy look).
- Choices of mine, all constants at the top of `sheets.rs`:
  - Ripple of 2 wavelengths across the width.
  - Blade pivot raised to `b` and a fixed ±30° (`a` unused).
  - Curtain spacing scales with size (exactly 0.35 at 0.7).
  - Waterfall spawn rate via `steps_per_beat` (clamped 0.5–4) and lane
    count via `count`.
  - Scanner box = `[b, b+size]` and mode switch at `a` = 0.5.
  - Slats run length = `a`.
  - Aurora spatial wavelengths 1.0/1.5/2.3 and weights 0.5/0.3/0.2.
  - Grid box = `[b, b+size]`.
- Wrap mode (scanner, grid, waterfall landing) makes a line jump. That is
  a move, not a flash: the amount of lit line is the same, and the
  limiter test confirms it.
- Waterfall with a non-integer `2 × steps_per_beat` (not offered by the
  UI) has 1 line more or less at times.
- `gA` step 0.01 is finer for all generators. Any existing value still
  shows (now with 2 decimals).

## Review

Reviewed by the architect (integrator). Clean merge on ad9c9fd; 424 unit
+ 2 signal tests, e2e 105/105, clippy clean. Accepted: new names
`ceiling`/`grid` instead of changing `liquid_sky`/`grid_scan` (append-only
keeps saved looks stable). Sheets are built at y ≥ 0; lines are not yet
covered by the output horizon (only beams are) — T-003/T-255 must add
line zones before any audience use. Grid at 730/750 points is accepted.
Verdict: APPROVED
