# feat/power-caps — T-254 per-output power caps and projector sheet

## What / why
Brightness was only a look setting (0..1). Cues, LFOs, audio, the timeline
and MIDI could all push it to 1.0, and no per-output safety maximum existed
(T-208 used a provisional 1.0). The operator also had nowhere to record the
projector (class, power, divergence...), which T-257 (MPE/NOHD estimate) and
T-263 (safety sheet) need.

- `studio/src/power.rs` (new):
  - `OutputLimits { max_power (0.5), max_color: [1.0; 3] }`, and
    `power::cap(frame, limits)`: `r = min(r, max_color[0]) * max_power`
    (the same for g and b). It is a clip per colour, then a global
    multiplier. It can only reduce: a channel never goes above
    `max_color[c] · max_power`, dark stays dark, positions are untouched,
    and NaN or negative values go dark. If the limit itself is broken (NaN),
    it sanitises to 0, the tightest value.
  - `ProjectorInfo { name, class ("4"), power_mw, wavelength_nm,
    aperture_mm, divergence_mrad, scan_angle_deg (40), hw_scan_fail:
    Option<bool>, divergence_lens, notes }`. The sheet is informative only.
    It never arms anything and never blocks arming. The task does not ask
    for an interlock on a missing sheet, so there is none. The accepted
    classes are "", 1, 1M, 2, 2M, 3R, 3B and 4.
  - `OutputStore` saves `<data-dir>/outputs.json` as
    `{outputs: [{id, limits, projector}]}`. The `main` output always
    exists, since there is one output today (T-277 will add more).
    - With no file (older installs), the defaults apply and nothing is
      written until something changes.
    - A hand-edited file is clamped into range.
    - A file that can't be parsed is copied to `outputs.json.bad`, the
      cautious defaults apply, and `load_error` is shown in the panel. This
      is the same pattern as `safety.json`.
  - `OutputStore::set(id, limits?, projector?, armed, confirm)`:
    - Lowering a cap is applied at once, even while armed.
    - Raising one is refused while armed (`SetError::Armed`, even when
      confirmed). When disarmed, it needs the explicit confirmation
      (`SetError::Raises` with a French list, like the safety-zones
      loosen pattern).
    - The projector sheet alone never asks.
    - Every change is logged with old and new values: `info` when lowered,
      `warn` when raised. The T-259 journal will take this over.
- `main.rs`: `Shared.outputs`. The engine reads `active_limits()` under the
  lock at the top of each frame and calls `power::cap` right after
  `safety::apply`, before the gate: layers → live → calibration → safety
  stage (horizon, zones, colours, strobe) → **power cap** → gate. A lowered
  cap therefore applies on the next tick, and the preview (`/api/frame`)
  shows the capped frame.
- `web.rs`:
  - `GET /api/outputs/limits` → `{active, outputs, defaults, classes,
    max_brightness_effective, load_error}`.
  - `POST /api/outputs/limits {id?, limits?, projector?, confirm_loosen?}`
    (a missing part is kept; `confirm_raise` is an alias). It returns 200
    `{output, max_brightness_effective}`, 400 for invalid values, 409
    `{error, loosen}` for a raise without confirmation, and 409 `{error,
    loosen, armed: true}` while armed.
  - `/api/state` adds `output_limits` and `max_brightness_effective`
    (= `max_power` of the active output, for T-208).
- Nothing else can reach the caps. They are not a control in the registry
  (so MIDI, OSC and LFOs can't touch them), not in `Settings` (so cues,
  scenes, the timeline and the look API can't either), and not in projects.
  `project.rs` doc and test cover this: a project carrying an `outputs`
  section changes nothing.
- UI, RÉGLAGES › « Sorties et projecteurs » (the T-277 placeholder card):
  - « Limites de sécurité »: « Puissance max (%) », « Rouge / Vert / Bleu
    max (%) ». Raising shows the red bar « Relever le plafond de puissance ?
    Puissance max : 30 % → 80 % » with « Confirmer la hausse » / « Annuler ».
    While armed, the panel shows the server's refusal instead.
  - « Fiche projecteur »: name, class, mW and nm for R/V/B, aperture,
    divergence, scan angle, hardware scan-fail (Inconnu/Oui/Non), divergence
    lens, notes.
  - Under « Luminosité maître »: « Plafond de la sortie : 50 % (appliqué
    après cette luminosité) ».

### Deviations / choices (please check)
- **Default 50 % changes every install's output.** This is what the task
  asks for: a look at 100 % now reaches the laser (and the preview) at
  50 %. The preview dims to match, because it shows the real output. The
  operator raises the cap once, with confirmation.
- **T-208 MIDI brightness cap left at 1.0** (`midi::safety::BRIGHTNESS_MAX`,
  doc comment updated). The task says `max_brightness_effective` should
  replace the provisional 1.0. But the task's cap is *multiplicative*, so
  also capping the MIDI fader to `max_power` would apply it twice (fader
  0.5 × cap 0.5 = 25 %). The multiplicative cap already bounds whatever MIDI
  sets. `max_brightness_effective` is exposed in `/api/state` and
  `/api/outputs/limits` for the UI and T-208. If the architect prefers the
  literal reading, it is a one-line change in `midi/engine.rs`.
- **No tick mark on the brightness slider.** For the same reason, a mark at
  `max_power` on a 0..100 % slider would be misleading (with a
  multiplier, the slider position is not what comes out). There is a text
  hint under the master brightness slider instead.
- The cap is applied *after* the strobe limiter, as the last thing in the
  safety stage (per the brief "after everything else"). The research
  pipeline lists it before the dwell guard (T-256) and the sky mask (T-262).
  Both of those only reduce, so the order does not change any bound.
- A raise refused while armed is refused even with `confirm_loosen`. The
  operator must disarm first.
- Min diode level (T-003) runs before the cap, so a very dim channel scaled
  by `max_power` can end up below the diode threshold. That is darker,
  never brighter.

## Testing
- `cargo test -p laser-studio`: **703 passed** (+2 +2 in the other
  targets), 7 ignored (pre-existing), on the branch rebased onto develop
  4164ab2 (T-153 audio routing, T-299 theme).
  - New in `power.rs` (7):
    - Full white at `max_power` 0.3 → 0.3.
    - Colour clip before scale (green 0.2 → ≤ 0.2; with 50 % → 0.1).
    - Only reduces: dark points and positions untouched, NaN/negative go
      dark, identity at 100 %/100 %, NaN limit → dark.
    - Validation and clamping of limits and sheet.
    - Store: lowering applies even armed; raising armed is refused even
      confirmed; raising disarmed without confirmation gives the French
      list; confirmed → applied; sheet alone never asks; invalid values and
      unknown output are refused.
    - File: missing → defaults, nothing written; round-trip persists;
      partial/hand-edited file clamped; broken file → `.bad` copy,
      defaults, `load_error`.
    - **Every catalogue cue** through the engine's pipeline: look at
      brightness 1.0; ~35 % of the `master.*`/`look.*` external controls
      set to random values through `controls::apply` (as MIDI would); 4
      random LFOs (any wave, Hz or beat rates, random depth, phase and
      offset) on random modulatable controls; random audio features
      (level, bass, beat, onset, kick, buildup); `live::apply`;
      `safety::apply`; then `cap` with random limits per cue. Every channel
      stays ≤ its ceiling in every frame (10 per cue). A non-vacuity check
      requires that most frames come above half of the cap. Afterwards,
      every external control is driven to 1.0 and the caps are unchanged.
  - `web.rs` (1): defaults over HTTP; armed → lowering 200, raising
    confirmed 409 `armed:true`; disarmed → 409 with « 30 % → 60 % », then
    confirmed 200 and `max_brightness_effective` 0.6; sheet 200; bad class
    / power 400; a look carrying `max_power` changes nothing.
  - `project.rs`: the « piège » project with an `outputs` section leaves
    the caps at their defaults.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `studio/e2e/tests/power-caps.spec.ts` (4, new):
  - Default cap: frame peak 0.5, panel shows 50 %, hint under master
    brightness.
  - Cap set to 30 % on the slider, master brightness 100 %, and an LFO
    pushing the look brightness: no colour > 0.3 in `/api/frame` across 20
    frames. Green 20 % → green ≤ 0.06 while red stays 0.3.
    `studio.restart()` → same limits, frame still capped, panel shows
    30 % / 20 %.
  - Raising on the slider shows the bar, « Annuler » keeps 30 %, HTTP
    without confirmation → 409. Armed (preview only, no `--device`):
    confirmed raise → 409, lowering to 20 % → 200 and the frame follows.
    Disarmed: « Confirmer la hausse » → 80 % and the frame follows.
  - Projector sheet saved, caps unchanged, and it is still there after a
    reload. Screenshot: `studio/e2e/test-results/power-caps.png`.
- Full e2e suite: **187/187** (2 workers) after the rebase. No existing test needed a
  change. The ones that assert brightness use relative values or upper
  bounds.
- All preview only: no `--device`. The one arm in e2e is on a studio
  without an output.

## Risks
- Behaviour change: default output at 50 %. An operator who doesn't read
  the release note will find the laser dimmer and has to raise the cap
  (with confirmation) in RÉGLAGES › Sorties et projecteurs.
- The cap is software. It does not replace the projector's own power
  setting or hardware protection (docs/research/safety-regulation.md).
- The sheet's values are whatever the operator types. Nothing checks them
  against the hardware.
- Single output: `max_brightness_effective` is `main`'s cap. When T-277
  adds more outputs, the engine must pick each output's own limits.
- The panel doesn't poll `/api/outputs/limits`. A change made over HTTP
  elsewhere shows after a reload, and the server stays the authority.

## Review
