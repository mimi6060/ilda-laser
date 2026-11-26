# feat/presets — procedural effects and a 202-cue library

## What / why
The user wants ready-made cues like Pangolin's and Showcontroller's
libraries. Their content can't be reused (Pangolin EULA §10; no licence on
Laserworld packs — see docs/research/), so this adds **our own** library:

- `generators.rs`: 20 procedural effect families written from scratch
  (lissajous, spirograph, rose, flower, vortex, tunnel, polygon tunnel,
  pulse rings, beam fan / cone / wave, sweep, liquid sky, grid scan,
  oscillator stack, helix, spiral arms, starburst, spectrum, clock).
  Geometry only; `colorize` applies solid / rainbow / gradient /
  alternate colour modes.
- `engine.rs`: new `Content::Generator { generator, params }` (serde
  default, old scenes still load); generator time scales with speed and
  bass.
- `presets.rs`: 202 cues in 8 pages (Abstraits, Tunnels, Faisceaux,
  Balayages, Vagues, Géométrie, Audio, Texte & horloge), stable ids.
- API: `GET /api/presets`, `POST /api/presets/play` (keeps the operator's
  brightness and music settings unless the cue is an Audio cue).
- UI: cue grid with page tabs under the preview, AZERTY key shortcuts per
  page (Space/Escape stay reserved for laser on/off and blackout), an
  "Effet" content tab with generator parameters.

## Testing
- `cargo test -p laser-studio`: 43 passed (every generator in bounds at
  several times; every cue renders lit points within budget; unique ids).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- Manual/API: played all 202 cues through the HTTP API and measured the
  frame: 0 cues under 30 fps at 30 kpps, heaviest 931 points. Clicked tabs
  and cues in Chrome; preview and generator panel update.

## Risks
- Point budget: a few cues are near 900 points (≈33 fps at 30 kpps).
- Beam cues assume haze; on a wall they read as dots.
- Clock uses UTC (no timezone crate).

## Review
Built and self-reviewed by the architect session (no separate reviewer
agent ran for this branch).
Verdict: APPROVED
