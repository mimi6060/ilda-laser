# feat/tunnel-gens — T-105 tunnels, cones, sun and rotating rays

## What / why
The tunnel that tightens through the build and opens on the drop, the cone
of beams turning overhead and the sunrise fan of rays are among the most
recognisable festival moments (`docs/research/festival-looks.md` section B,
looks 13–18). This branch adds four beat-synced generators on the T-100
base, plus snap rotation for the existing `polygon_tunnel`. All of it is our
own maths, written from the research's descriptions in words. Nothing is
copied from any vendor's content.

New module `studio/src/tunnels.rs`, which follows the `fans.rs` pattern:
- `generators::generate` falls through to fans and then to tunnels.
- The names are **appended** to `GENERATOR_NAMES`.
- Each generator is a pure function of (params, ctx) and moves from
  `ctx.beat_pos`, whatever `beat_sync` says.
- Rotating looks read `a` as **turns per beat**, clamped to 0..1 and
  computed in f64 so they stay exact on the beat.
- The tunnels are centred at (0, `TUNNEL_CY` = 0.5) with a radius of at
  most 0.5, so the whole cone stays above the horizon (y ≥ 0).

| Generator | UI label | Parameters |
|---|---|---|
| `finger_tunnel` (look 14) | Tunnel de faisceaux | `count` (≤ 16) beams on a **true** circle, radius = size, `a` turns per beat, `direction` -1 = the other way. |
| `tunnel_pump` (look 16) | Tunnel qui pompe | One continuous circle. `b` < 0.5 (pump): r = r_base·(1 + a·env), where env jumps to 1 on the beat and falls exponentially (τ = 0.1 beat) to exactly 0 at 1/2 beat. r_base = size, capped so the peak is ≤ 0.5. `b` ≥ 0.5 (ramp): r goes linearly from size to 0.04 over `period_beats`, then opens again; `direction` -1 grows instead. |
| `twin_tunnel` (look 17) | Double tunnel | Two concentric rings, outer = size and inner = size·0.2/0.35, turning in opposite directions at `a` turns per beat (the UI starts at 1/8 = one turn per 2 bars). Each ring has a 10 % gap as its marker. |
| `sunburst` (look 15) | Soleil levant | `count` (≤ 24) rays of length = size from (0, 0), `a` turns per beat. There are 2N ray slots around the whole circle, starting half a slot off the horizon. Only those with y > 0 are drawn, so there are N (N−1 while one crosses), none below the horizon. `b` ≥ 0.5: odd and even rays swap between 100 % and 40 % on each **whole** beat (never faster, whatever `steps_per_beat` says). |
| `polygon_tunnel` + `snap` | Rotation par à-coups | `GenParams.snap: bool` (`serde(default)` false). When on, the rotation is `step·360°/sides` (`ctx.step`, one step per beat by default) and holds still between steps. Off: exactly the old drawing. |

UI (Effet tab):
- Four French labels.
- Help lines under Nombre, Forme A, Forme B and in the tempo panel.
- A « Vitesse (tours par temps) » select (Glaciale 1/64, Lente 1/32,
  Moyenne 1/16, Rapide 1/4, Très rapide 1/2, plus a disabled « Autre » shown
  for other values) that fills `a`. It is shown only for the three rotating
  generators. For those, Forme A's slider step becomes `any` and its value
  reads « x.xxx tour/temps ».
- A « Rotation par à-coups » checkbox with a hint, shown only for
  `polygon_tunnel`.
- Picking one of the four starts it in tempo with its starting values (the
  `GEN_DEFAULTS` mechanism from fan-gens).

### Deviations from the task text
- **« Soleil » is already the label of the old `starburst`**, so `sunburst`
  is « Soleil levant » to avoid two identical entries. I didn't rename the
  old label.
- There is already a « Vitesse » slider (`speed`), so the new select is
  called « Vitesse (tours par temps) ».
- The tunnels are centred at y = 0.5 and not at 0, so that the cone passes
  over the audience (research: "above y_h"). The size is capped at 0.5.
- The task doesn't say which parameter selects the mode, so I chose them:
  `tunnel_pump`'s ramp mode is `b` ≥ 0.5 (task: "b = 1"), and sunburst's
  odd/even is also `b` ≥ 0.5. The ramp starts at the size, so the UI would
  set size 0.35 to match the task's "0.35 → 0.04". I didn't change the look
  size on pick, because it is a look-level setting.

No cues were added (the « Festival » page is T-110).

## Testing
- `cargo test -p laser-studio`: **404 passed** (+ 2 in the second test binary), 2 ignored, after rebasing
  on develop 4528f9a. This branch adds 12 tests: 11 in `tunnels.rs` and 1
  in `generators.rs`. `cargo clippy -p laser-studio --all-targets -- -D
  warnings`: clean.
- Non-regression:
  - The pinned generator digest (`GENERATOR_NAMES[..20]`) is unchanged.
    That covers `polygon_tunnel` without snap.
  - The cue-frame and cue-id digests in `presets.rs` are unchanged.
  - The fans test now checks `GENERATOR_NAMES[20..25]`, because the list
    grew.
- Unit tests, per acceptance criterion:
  - `finger_tunnel`: every beam at r ±1 % for four sizes and four beats,
    evenly spread.
  - Rotation at 1/4 turn per beat: +90° after 1 beat, home after 4 and
    after 400 beats, reversed with `direction` -1. At 1/16 turn per beat:
    a half turn at beat 8, home at beat 16.
  - `tunnel_pump`: the largest radius is on the beat (0.39 for r_base 0.3,
    depth 0.3), it shrinks steadily, and it is exactly r_base from 1/2 beat
    on. The jump at the release is under 0.003. A large size is capped at
    0.5. The ramp goes 0.35 → 0.04, is linear, and reopens on the period;
    reversed, it grows.
  - `twin_tunnel`: radii 0.2 and 0.35, each with a 10 % gap. The outer
    ring turns +1/4 and the inner −1/4 after 2 beats, and both are home
    after 8 beats.
  - `sunburst`: for 5 counts × 2 directions × 200 beats, after `colorize` +
    `densify`, no lit point has y ≤ 0. There are N or N−1 rays, each at
    radius 0.85. Odd/even: steady within a beat; 100/40 swap on the next
    beat, neighbours alternate, half are bright, and `steps_per_beat` 8
    doesn't make it faster.
  - `polygon_tunnel` snap: the angle is constant from beat 1.0 to 1.99
    (even when `t` changes), +1/3 turn per beat for a triangle and +1/4 for
    a square, reversible. Without snap it still turns from `t` and ignores
    the beat. Old JSON loads with `snap` = false.
  - All four stay finite and inside -1..1 and at or above the horizon
    (tunnels within r ≤ 0.5), for 5 parameter sets (including the UI's
    a=3/b=2 and count 64), 7 sizes and 80 beats.
  - Same frame at 90 and 174 BPM at the same beat.
- **Flashing:** every new generator runs through the real
  `safety::StrobeLimiter` for 10 s at 60 fps and 250 BPM, with the fastest
  settings (a = 1 turn per beat, odd/even on, ramp over 1 beat). None is
  ever seen as `fast` or `active`, and the frame's light level never drops
  below half its maximum. The generators don't rely on the limiter.
- **Point counts** (after `colorize` + `densify`, worst of 64 frames over 4
  beats; budget `layers::DEFAULT_POINT_BUDGET` = 750, asserted in
  `tunnels_hold_30_fps_at_30_kpps`):

  | Generator | N=8, size 0.35 | N=12, size 0.5 | N=16, size 0.85 | N=24, size 1.0 |
  |---|---|---|---|---|
  | finger_tunnel | 172 | 260 | 318 | 318 (capped at 16) |
  | tunnel_pump | 151 | 151 | 151 | 151 |
  | twin_tunnel | 223 | 231 | 231 | 231 |
  | sunburst | 144 | 216 | 303 | 432 |

  A 151-point circle redraws at about 200 Hz at 30 kpps, well above the
  40 Hz a solid cone needs. For `polygon_tunnel` with snap: 4 square rings
  at size 1.0 give 556 points, and 4 triangles at 0.7 give 367.
- e2e: full suite **98 passed**. Two new cases in `content.spec.ts`:
  - Select « Tunnel de faisceaux ». Check that `beat_sync` is on,
    a = 1/16, N = 12, the Vitesse row is visible with 1/16 selected, and
    the help shows. Choose « Rapide (1/4) », which sets a = 0.25 and shows
    « 0.250 tour/temps ». `/api/frame` must have exactly 12 beams, all at
    0.5 ± 0.01 from (0, 0.5), and the first beam must move between reads.
  - Select « Soleil levant »: every lit point has y > 0. Then pick
    `polygon_tunnel`: the snap row shows, the Vitesse row hides, and
    ticking it sets `snap`.

  Preview only: the harness's own studio, with `--no-midi` and no
  `--device`.

## Risks
- **The snap rotation of a regular polygon by 360°/sides maps it onto
  itself.** In a solid colour, the snap is invisible. It shows with colour
  modes that vary along the path (Dégradé, Arc-en-ciel) or with the per-ring
  twist `b`. This is what the task asked for, and the UI hint says so. A
  reviewer may prefer half a side per beat.
- "Above the horizon" is in the look's own frame. The user's rotation,
  offsets and calibration are applied afterwards, as for the fans. T-101's
  `beam_floor_y` still blanks beams in output space.
- In the sunburst, a ray going under the horizon disappears and one appears
  on the other side at the same moment, so the light stays constant (the
  limiter test confirms this). At 1 turn per beat with 24 rays, the rays
  near the horizon flick in and out fast. That is motion, not a flash.
- The tunnels have a fixed centre height (0.5). You can move them with the
  look position or offsets, but there is no parameter for it (`b` is used
  for the modes).
- `polygon_tunnel` at many rings and full size was already over 750 points
  before this branch (8 rings at size 1.0 give 992). Snap doesn't change the
  point count.

## Review
