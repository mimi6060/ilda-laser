# feat/safety-zones — T-003 projection zones, full horizon, colour calibration

## What / why
Audience safety needs places where the laser must never light up, and a
horizon that covers *everything*, not just beams: T-101's provisional
horizon only blanked held points, so a line, text or sheet brought below
the horizon (rotation, live position, calibration) went straight through
(docs/prs/sheet-gens.md, Risks). T-255/T-256/T-279 build on this.

- `studio/src/zones.rs` (new): `Zone { id, name, kind: Blank|Dim, level,
  points }`, `Horizon { y, lines, ramp, level }` and `Mask` (the compiled
  zones + horizon + colour calibration for one frame).
  - Polygons, concave allowed (even-odd), up to `MAX_ZONES` = 8 with 3 to
    `MAX_VERTICES` = 24 vertices, in **output** coordinates (-1..1, y up,
    after calibration — what the preview draws).
  - **Segment splitting.** Blanking samples is not enough: the DAC draws a
    straight line between two samples, which can cross a zone. Every
    segment with a lit end is cut where it crosses a zone edge (or the
    horizon). Around each crossing, samples are inserted 1e-3 either side
    of the edge; the hop across the edge is emitted at the *lower* level
    of the two sides, with duplicate samples, so the result is right
    whether the DAC colours a segment from its start or its end sample.
    Levels come from the middle of each interval between crossings (so a
    segment that clips a corner shorter than 2e-3 is still cut), and every
    emitted sample is also scaled by the attenuation at its own position:
    no lit sample can be inside a Blank zone by construction.
  - Dim zones scale the colour by their level; overlapping attenuations
    multiply (a Blank zone wins).
  - Horizon: beams below `y` are always blanked (T-101's
    `blank_low_beams`, kept as is — "keeping its protection"). With
    `lines` on, everything below `y` is scaled to `level` (default 0 =
    blank), with a linear ramp from `level` at `y` to full at `y + ramp`.
    The ramp is **above** `y`, so below the horizon light never exceeds
    `level`. Beams below `y` stay blanked even when `level` > 0.
  - Colour calibration: `color_gain` per channel (0..1, can only dim) and
    `min_diode_level` (0..0.5): a lit channel `v` goes out as
    `min + v·(1 − min)`; a dark channel stays 0 (a Blank zone stays blank).
  - Nothing configured (the default) → identity, same frame as before.
- `studio/src/safety.rs`:
  - `SafetySettings` gains `zones`, `horizon`, `color_gain`,
    `min_diode_level`; `beam_floor_y` becomes `horizon.y`. Old
    `safety.json` files and old API bodies with `beam_floor_y` still load
    (`SafetyWire`, serde `from`). No longer `Copy`.
  - **Loosening needs an explicit operator action.** `SafetyStore::set(new,
    confirm_loosen)` compares with the current settings: zone removed,
    moved/redrawn, Blank→Dim, Dim level raised, horizon lowered, lines
    unticked, level under the horizon raised, ramp shortened, gain raised,
    diode minimum raised, strobe limits raised (even within the defaults)
    → `SetError::Loosens(french list)` unless confirmed. Tightening applies
    at once. Strobe limits still can never go beyond the defaults (400).
    Zones get ids from the store (`id` 0 = new). Confirmed loosenings are
    logged (`warn`).
  - A `safety.json` that can't be parsed is copied to `safety.json.bad`,
    the safe defaults are used and the error is reported (`load_error`
    in `GET /api/safety`, shown in the panel) — so an operator notices
    their zones were not loaded instead of silently losing them.
  - `apply()`: beam horizon → zones/horizon/colours → strobe limiter. The
    limiter now **measures the masked frame** (the light that really goes
    out), and a frame it puts back during a hold is the *unmasked* one
    re-masked with the **current** settings (a zone added during a hold
    applies at once, no double dimming). `StrobeStatus.points_masked`.
- Pipeline position unchanged: layers → live → calibration → **safety**
  → output gate. Cues, timeline events, live moves, LFOs, text, figures,
  sheets all pass through it. Settings live in the data dir's
  `safety.json` only; projects never touch them (existing project test).
- `web.rs`: `GET /api/safety` → `{settings, defaults, status, load_error,
  limits}`. `POST /api/safety` → 200 `{settings}` (with ids), 400
  invalid, **409** `{error, loosen: [...]}` when looser without
  `"confirm_loosen": true`. Missing fields are the defaults, so a partial
  body that would drop zones is a 409, not a silent removal.
- UI (RÉGLAGES › Sécurité): « Horizon », « L'horizon coupe aussi lignes,
  textes et nappes », « Rampe de l'horizon », « Luminosité sous
  l'horizon »; « Zones de projection » (one row per zone: name,
  Masquer/Atténuer, % kept, vertices as `x y ; x y ; …`, « Dessiner » —
  click vertices on the 2D preview, « Terminer » —, « Supprimer »),
  « Ajouter une zone » (a Blank band over the bottom of the field),
  « Afficher sur l'aperçu »; « Couleurs » (gain R/V/B, niveau minimum des
  diodes). A looser edit shows a red bar « Assouplir la sécurité ? … »
  with « Confirmer l'assouplissement » / « Annuler » (no browser dialog).
  The 2D preview draws the zones as translucent red (Dim: orange), the
  horizon as a dashed red line, and the covered area below it when lines
  are on — from the settings the server applies, not a pending edit.

### Deviations / choices (please check)
- `horizon.lines` defaults to **off**: turning it on by default would cut
  the lower half of every centred figure on every install. Default
  behaviour is exactly T-101's (beams below 0 blanked). T-255 is where
  arming should require a zone or a lines horizon.
- 8 zones instead of the task's 5 (T-255 adds audience zones on top of
  the operator's own). Easy to lower (`MAX_ZONES`).
- Any change of an existing zone's vertices counts as loosening (proving
  that a new polygon contains the old one is not worth the complexity);
  adding zones and renaming never ask.
- The 3D view has no overlay: T-279 covers zones/horizon in the venue.
- Zones and the horizon are in output coordinates (after calibration), like T-101's floor: a calibration change moves the content, not the zones.

## Testing
- `cargo test -p laser-studio`: **680 passed** (+ 4 in the other targets),
  7 ignored (pre-existing), on the branch rebased on develop 070613d.
  New in `zones.rs` (12): point-in-polygon on a concave L; a segment
  crossing a zone split at both edges (lit up to 1e-3 from each edge); a
  segment clipping a corner by less than 2 × the gap; samples inside
  blanked, Dim scaling, overlaps multiply; a Dim edge crossed at the lower
  level with the duplicate; blanked travel left untouched (no extra
  samples); a lit → dark segment through a zone stopped at the edge (the
  "start colour" DAC convention); horizon ramp values and a one-segment
  vertical line fully dark below; gain / minimum diode level / clamping;
  identity when unconfigured; **200 random seeds × 3 random (often
  concave) zones × random long polylines** → no lit sample inside and no
  lit segment through a Blank zone under either colouring convention;
  performance (below). In `safety.rs` (+8 / updated): validation ranges,
  clamping of hand-edited files, T-101 file migration, every loosening
  kind refused without confirmation and the settings unchanged, renaming
  allowed, store round-trip and a broken file kept aside and reported;
  **every catalogue cue** (12 frames each, > 1000 frames) through the real
  `Animator` and `apply` with random zones per cue, the lines horizon on
  every other cue and a Dim band → no violation, nothing lit below the
  horizon, Dim band ≤ 30 %; a line below the horizon cut only with
  `lines`; beams below the horizon blanked even at level 50 %; a frame
  held by the limiter is blanked by a zone added during the hold; default
  settings = identity and a half-field zone leaves the other half
  byte-identical. `web.rs`: `beam_floor_y` compat, zone id assignment,
  409 with the zone's name, confirm → defaults.
- Performance: 2000-point dense spiral, 8 zones × 24 vertices, lines
  horizon, gain and minimum level: **0.10 ms/frame in release**, 1.2 ms
  in the debug test build (the test asserts < 8 ms).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `studio/e2e/tests/safety.spec.ts` (8 tests, 4 new, 2 updated):
  « Ajouter une zone » + redraw as a stripe through the centre
  (confirmation bar), then circle, line, square and text → `/api/frame`
  has lit points outside and **no lit point / lit segment inside** (zone
  shrunk by 3e-3 for the 1e-3 rounding); zone persists after
  `studio.restart()` (same settings, frame still masked, row shown in the
  panel) and the overlay pixel inside the zone is red, and not red with
  « Afficher sur l'aperçu » unticked; deleting a zone asks, « Annuler »
  keeps it, a body without it is 409 over HTTP, « Confirmer » removes it;
  ticking the lines horizon → no lit point below 0 and `points_masked` >
  0, unticking asks first. Updated: lowering the beam horizon is 409
  without confirmation; raising the strobe back from 2 to 3 Hz shows the
  bar and « Annuler » keeps 2 Hz. Screenshot:
  `studio/e2e/test-results/safety-zones.png`.
  Full suite: **177/177** on the rebased branch.
- All preview only: no `--device`, studio disarmed throughout (asserted).

## Risks
- Inserted samples add points (≤ 3 per crossing): a figure crossing many
  zone edges gets slightly more points, so a slightly lower frame rate at
  fixed pps. Negligible for normal zones.
- The DAC/galvo lag is not modelled: the colour switches at the sample,
  the mirrors arrive a little later, so real light can spill a little
  past a zone edge at speed. Operators should draw zones with a margin;
  a configurable margin/colour-shift compensation could be a follow-up.
- Every zone vertex edit asks for confirmation, which is safe but chatty
  while drawing a zone for the first time (add, then redraw once).
- `/api/state.safety` / `GET /api/safety` changed shape (`horizon`
  object instead of `beam_floor_y`); the only in-repo client is the UI,
  updated. Old bodies with `beam_floor_y` are still accepted.
- f32 values go through `serde_json::Value`, so the API shows e.g.
  `0.30000001192092896`; the UI rounds.
- Software masking does not replace hardware safety (scan-fail detection,
  physical masks); see docs/research/safety-regulation.md.

## Review

Reviewed by the architect (integrator). Clean merge on 070613d; 680 unit
+ integration tests, clippy clean, e2e 177/177 (2 workers). Order checked:
layers → live → calibration → zones/horizon/colour → strobe limiter →
output gate, so every source is masked and the preview shows the real
output. Edge splitting covered by 200 random seeds, every catalogue cue
with random zones, both DAC colour conventions. Loosening needs explicit
confirmation (409 otherwise); settings never in projects; bad file →
safe defaults. Accepted: full horizon off by default (T-101 behaviour
kept), 8 zones. Follow-ups: automatic margin for galvo lag, 3D overlay
(T-279), T-255 arming requirements.
Verdict: APPROVED
