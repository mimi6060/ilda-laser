# feat/beat-gen — T-100 beat-synced generator base

## What / why
Festival looks are measured in beats (one turn per bar, a chase every
half beat). Until now generators only got `t` in seconds, so nothing could
land on the "one". This branch gives them a beat position from the **one
tempo clock** (T-150, `tempo.rs`), with no second clock, plus the shared
toolkit that T-102..T-123 need.

- `studio/src/beat.rs` (new), pure helpers: `phase(beat_pos, period)`,
  `env_stab(phase_in_beat, gate, decay)` (instant attack, then a hard cut
  or exponential decay), `ease_sine`, `ease_in`, `smoothstep`,
  `step_index(beat_pos, steps_per_beat)`, `seeded_rand(seed, i)`
  (SplitMix64), `bar_start`, and virtual heads: `group_of(i, n, groups)`
  (contiguous split, 1–4) and `GroupMode { Unison, Mirror, Offset }::motion`,
  which gives each group's phase and sign.
- `generators.rs`: `generate(name, &params, &GenCtx)` with
  `GenCtx { t, beat_pos, bpm, level, bass, scale }` and the helpers
  `ctx.cycle(p)` (0..1, reversed if `direction < 0`), `ctx.step(p)` and
  `ctx.beat_phase()`. `GenParams` gains `beat_sync`, `period_beats` (4),
  `steps_per_beat` (1), `gate_beats` (0), `direction` (1), `groups` (1)
  and `group_mode` (unison). All are `serde(default)`, so old scenes and
  presets load unchanged.
- Per-beam intensity and colour: `Geometry::styles: Vec<BeamStyle>`
  (`intensity` 0..1, `tint` Auto/Primary/Secondary/Rgb), one per stroke,
  where a missing entry means the default. `colorize` honours them. A beam
  at intensity 0 is still visited, with the beam off and the same dwell, so
  chase geometry and timing stay stable. `colorize` now takes `gain` for the
  colours it makes itself (rainbow, `Rgb`). This replaces the engine's
  rainbow post-multiply and gives bit-identical output.
- `engine.rs`: `Animator::render(…, &BeatClock)`, where `BeatClock` holds
  `{ beat, bpm, beats_per_bar }` read from `TempoClock` each frame.
  `beat_pos = clock.beat − bar_start(launch beat)`, so it counts from the
  first beat of the bar the cue was launched in. The launch beat is
  `ActiveCue::started_beat`, via `Animator::starting_at`. The manual look
  uses its first rendered frame. Tap, resync and nudge on the clock move
  every cue with it.
- `beat_sync = true` on an existing generator: `t = ±2π·beat_pos /
  period_beats`, and `speed` and the bass boost are ignored because the
  tempo sets the pace. Any motion that repeats every 2π of `t` then repeats
  exactly every period at any BPM. `gate_beats > 0` lights the look only
  for that long after each beat. With `beat_sync = false`, nothing changes.
- UI (Effet tab): « Tempo du look » checkbox, which reveals Période (1–32
  temps), Pas par temps (1/2–8), Gate (temps), Sens (→/←), Groupes (1–4),
  Mode de groupe (Ensemble/Miroir/Décalé), plus the current BPM and 1-2-3-4
  beat dots.

Not in scope: new generators (T-102..T-108). A test-only `test_fan` in
`generators.rs` shows how they use the toolkit: mirrored groups plus a
per-step chase through `styles`.

## Testing
- `cargo test -p laser-studio`: **126 passed** (+19).
  `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- Non-regression: before touching any code I recorded golden digests and
  pinned them in tests. The digests are FNV hashes of values rounded to
  1e-3, so libm last-bit differences don't break them. There are three:
  (1) the 30th frame of all 202 cues through `Animator`, (2) the geometry
  of the 20 generators × 3 parameter sets × 4 times, and (3) the list of
  cue ids. All three still match. `clock` is excluded from (1) and (2)
  because it shows system time.
- Beat tests: `phase`, `env_stab` and `step_index` at 128 and 150 BPM;
  a beat-synced look with a 4-beat period is back in place after 4 and 40
  beats and has moved after 2, at 128 and 150 BPM, and is identical at the
  same beat at 90 and 174 BPM; beat_pos anchors to the launch bar; reverse
  direction; gate on/off; free-running looks ignore the clock; Mirror
  groups have opposite x offsets; per-beam styles; old `GenParams` JSON
  gets the defaults.
- Manual, preview only (`--port 8098 --data-dir <scratch>`, no
  `--device`, laser disarmed): I POSTed a beat-synced `beam_circle` with
  every new field to `/api/settings`, and `/api/state` returned them all.
  In `/api/frame` the look is lit only while `tempo.phase < 0.25`, which
  is the gate. Playing preset `faisceaux-001` still renders. `node --check`
  passes on the page script.
- No e2e: `studio/e2e/` isn't on `develop` yet (it's on `feat/e2e`). The
  T-100 e2e case (turn on « Tempo du look » at 120 BPM, check `/api/state`)
  should be added there once it's merged. I didn't click through the new UI
  in a browser.

## Risks
- **Gate = strobe.** `gate_beats` flashes once per beat, which is 4.2 Hz
  at the 250 BPM maximum, just over the 4 Hz guideline. The T-101 limiter
  must cover it.
- `beat_sync` on the **existing** generators only loops cleanly for motion
  that repeats every 2π of `t` (lissajous, beam_circle, beam_fan,
  spiral_arms, …). Others, like the tunnel's 0.25·t, keep moving but don't
  land exactly on the period. Real festival looks come with T-102+.
- `groups`, `group_mode` and `steps_per_beat` don't change any existing
  generator yet. The UI shows them, and they take effect with T-102+.
  Until those land, most `beat.rs` helpers are only called from tests
  (`cfg_attr(not(test), allow(dead_code))`).
- `t` is computed in f64 and cast to f32. After an hour at 128 BPM the
  error is about 1e-3 rad, which you can't see.
- `Animator::render` gained a parameter. Branches in flight that call it
  (live, e2e, midi) need `&BeatClock::default()` or the real clock when
  they merge.

## Review
