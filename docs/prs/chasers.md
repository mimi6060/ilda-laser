# feat/chasers — T-103 chasers, kick stabs and beam strobes

## What / why
Beam chases, stabs on the kick and beam strobes are the main vocabulary of
techno and hardstyle drops (`docs/research/festival-looks.md`, looks 7, 8
and 10, and §4.4/§4.5). This branch adds them as our own maths. Nothing is
taken from any vendor's cues or content: the looks are built from the
research's descriptions in words.

### `chase_fan` generator (look 7), new module `studio/src/chases.rs`
- N beams (at most 32) on a fixed line `CHASE_HEIGHT` = 0.3 above the
  horizon. The half-width is the look size.
- The galvo visits **every** beam on every frame, so the geometry never
  moves. Only the per-beam gate changes, through `Geometry::styles`:
  intensity 0 means blanked but still visited, so the timing stays stable.
- `a` (rounded, clamped 0..7) is the shape (`ChaseShape`):
  - 0 forward, 1 backward
  - 2 bounce (ping-pong, end beams not repeated)
  - 3 centre → out, 4 out → centre (pairs for even N, one centre beam for
    odd N)
  - 5 odd/even
  - 6 random (`seeded_rand` per step, so it is reproducible)
  - 7 fill then empty (period 2N, with one dark step)
- `b` (rounded, 0..3) is the tail length. Tail intensities are 1 / 0.4 /
  0.15, and a third tail beam at 0.06 for b = 3. Group shapes (odd/even,
  fill) have no tail.
- The chase advances one step per `steps_per_beat`, read from
  `ctx.beat_pos` (like the T-102 fans).
- In colour mode « Alterné », the head is in the main colour and the tail
  in `color2`.
- `generate` falls through to it. `"chase_fan"` is **appended** to
  `GENERATOR_NAMES`. `fans::line` is now `pub(crate)`, so the chaser reuses
  it.

### « Rythme » on every look (looks 8 and 10), in `engine.rs`
New `Settings` fields, all `#[serde(default)]`, so old scenes and cues load
as "off":

| Field | Default | Meaning |
|---|---|---|
| `gate: GateMode` | `off` | `off` / `beat` / `beat_and_offbeat` (snake_case) |
| `gate_beats` | 0.2 | Lit for this long after each beat (or off-beat). Clamped to 0.02..slot; NaN → 0.2 |
| `gate_decay` | false | Instead of the hard cut: an instant flash fading with τ = `STAB_DECAY_BEATS` = 0.12 beat (the research's "or decay τ = 0.12") |
| `strobe_div` | 0 | Flashes per beat: 2 = every 1/2 beat, 4 = 1/4, 8 = 1/8. Clamped to ≤ 8 |
| `strobe_duty` | 0.4 | Lit fraction of each strobe slot, clamped to 30–50 % |

- `Settings::rhythm_gain(beat_pos)` gives gate × strobe. It multiplies the
  look's `gain` in `render_look`, so it works for shapes, text, wave,
  generators (with or without `beat_sync`), figures and evolving cues. The
  evolving look inherits the outer `Settings`' rhythm.
- New helper `beat::strobe_gate(beat_pos, div, duty)`.
- Units note: the task text says `strobe_div ∈ {0, 1/2, 1/4, 1/8 temps}`. I
  stored it as **flashes per beat** (0/2/4/8). This matches the existing
  `EvolvingKey::strobe_div` (T-111), so both "strobe_div" fields mean the
  same thing. The UI labels still read « 1/2 temps », « 1/4 temps », « 1/8
  temps ».
- The existing `GenParams::gate_beats` (T-100, beat-synced generators only)
  is unchanged. If both are set, they multiply.
- **Safety**: all of this happens inside content rendering, *before* the
  T-101 strobe limiter, the horizon and the zones. Nothing bypasses them.
  A 1/8-beat strobe at 128 BPM (17 Hz) is cut after 5 s (tested through the
  real Animator + `safety::apply`).

### Cues
Eleven cues are appended to the **Faisceaux** page, `faisceaux-032` to
`faisceaux-042`. All are `beat_sync`.
- 7 chasers, one per shape except backward:
  - 1/2-beat steps with a 2-beam tail for forward and bounce (the bounce is
    a white head with a blue tail)
  - 1-beat steps for centre/out, odd/even and fill
  - a random chaser on 12 beams
- « Coups sur le kick » (fan, 0.2-beat gate)
- « Coups qui s'éteignent » (decay)
- « Positions temps + contretemps » (positions every 1/2 beat, gated on
  beat and off-beat)
- « Strobe 1/2 temps » (4.3 Hz at 128 BPM: the sustained-safe rate; no
  faster strobe cue ships)

Thumbnails come automatically.

The golden digests `cue_frames_are_unchanged` / `cue_ids_are_unchanged` now
cover the first `LEGACY_CUES` = 202 cues (every cue before this branch). Their
pinned values are **unchanged**, so no existing id or frame moved. The
« Festival » page itself is still T-110.

### UI (`index.html`, French)
- **Effet tab:** « Chaser » is in the generator list. Picking it starts in
  tempo with N = 8, tail 2 and 2 steps per beat. A « Forme du chaser »
  select (8 shapes, shown only for the chaser) writes `a`. There are help
  lines for Nombre / A / B / tempo, and Forme B is the tail.
- **New « Rythme » section** in CRÉATION › Look, for every look:
  - Gate (Aucun / Temps / Temps + contretemps)
  - Durée gate (0.05–0.5 beat, disabled when off or in decay)
  - Décroissance
  - Strobe (Aucun, 1/2, 1/4, 1/8)
  - Rapport cyclique (30–50 %, disabled without a strobe)
  - a hint about the shortcut and the limiter
- **Hold `S`:** strobe 1/4 while held. Released, the look gets back its own
  strobe value (also on window blur). The press and release are two look
  edits.
  - **Deviation:** in LIVE, `s` is already a cue letter (12th cue of the
    page, AZERTY row). So the hold is **Alt+S** in LIVE and plain `S`
    everywhere else (Alt+S also works everywhere). Cmd/Ctrl+S is still
    « Enregistrer ».

## Testing
- `cargo build -p laser-studio`: OK.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- `cargo test -p laser-studio`: **710 passed**, 7 ignored, plus 2 + 2 in the
  other targets. That is +15 on develop:
  - `chases.rs` (7):
    - forward chase: beam k is lit on beat k mod 8, and 2 steps per beat
      work
    - each shape's beam order, including odd N, out-of-range `a`, NaN and
      one beam
    - tail 1 / 0.4 / 0.15 (+0.06), wrapping at the edge, no tail before the
      chase has made those steps, the bounce tail follows the head back,
      group shapes have no tail
    - positions are identical frame to frame and only the gates change
      (all 8 shapes)
    - « Alterné » head/tail colours
    - point budget (≤ 750 after densify, N up to 64→32) and bounds above
      the horizon
  - `engine.rs` (5):
    - a 0.2-beat gate at 128 BPM is lit at 90 ms after the beat and dark at
      98 ms (≈ 94 ms), on beats 0, 1, 7 and 30, on a plain shape
    - beat + off-beat; decay (1 → e⁻¹ after τ → < 0.001, also in the
      rendered frame); NaN length
    - strobe 1/4 at 40 %, duty and rate clamps, gate × strobe
    - rhythm applies to evolving cues
    - old looks load with the rhythm off, and the enum serialises in
      snake_case
  - `beat.rs` (1): `strobe_gate` duty and on-the-beat tolerance.
  - `safety.rs` (1): a chaser with a 1/8-beat look strobe at 128 BPM,
    rendered at 60 fps through the real Animator and `apply`, has more
    than 100 dark frames during the burst, none after 5.2 s, and the
    limiter is active.
  - `presets.rs` (1): the rhythm cues are appended on Faisceaux from
    `faisceaux-032`, all `beat_sync`, 7 chasers, gate/decay/off-beat cues,
    strobe ≤ 1/2 beat, and older cues have no rhythm.
  - Existing guard tests still pass with their pinned values:
    `every_cue_has_a_small_visible_thumbnail`,
    `every_cue_renders_something_within_budget`,
    `no_catalogue_cue_lights_a_blank_zone`,
    `existing_generators_draw_exactly_what_they_did`.
- e2e: new `studio/e2e/tests/chasers.spec.ts` (3 tests):
  - Play « Chaser → · vert » from the Faisceaux page. Two `/api/frame`
    reads half a beat (+20 ms) apart have their brightest beam at a
    different x, and every lit point is above y = 0.
  - Effet tab → « Chaser » starts with the starting values. « Forme du
    chaser » sets `a` = 3, and the row hides for other generators.
  - « Rythme » controls write `gate`, `gate_beats`, `gate_decay`,
    `strobe_div` and `strobe_duty`, and dark frames appear. Holding `S` in
    CRÉATION sets the strobe to 4 and releasing it restores 2.
- Full e2e run, `npx playwright test --workers=2`, done twice: 184–185 of
  186 passed. The failures were timing tests under heavy machine load (load
  average ≈ 16 from parallel agents):
  - `tempo.spec` « Tap button » measured 109.3 BPM, below the > 110 bound
  - `presence.spec` « two pages, one closed », once

  Neither touches this branch's code, and both specs pass when re-run
  (`tempo.spec.ts` + `presence.spec.ts`: 14/14). All chaser, content and
  cue specs passed in every run. Preview only: no `--device`, never port
  8080.

## Risks
- **The `S` key in LIVE is Alt+S**, not S, because S is a cue letter
  there. If the architect prefers S to win over the 12th cue in LIVE, it is
  a one-line change in the keydown handler.
- **The held strobe is a look edit.** If a scene or cue is saved while S is
  held, it saves `strobe_div` = 4. Released keys and window blur restore
  it.
- `strobe_div` means flashes per beat, not beats per flash (see the units
  note above).
- The look gate and strobe are new ways to flash. They stay subject to the
  limiter, and the shipped cues only strobe at 1/2 beat.
- **Merge conflicts:** `presets.rs` (the cue list and the `LEGACY_CUES`
  guard) and `GENERATOR_NAMES` are likely to conflict with other festival
  branches (T-104/T-107/T-108/T-110). Both are append-only. Resolve by
  keeping both lists in merge order. A branch that also appends cues must
  move `faisceaux-032` / page numbering expectations accordingly.

## Review
