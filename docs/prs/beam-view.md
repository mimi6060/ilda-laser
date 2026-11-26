# feat/beam-view — T-275 3D visualiser base, T-276 beam rendering in haze

## What / why
Designing festival beam looks needs to see beams in perspective from the
audience, not points on a flat canvas: an 8-beam fan is 8 dots in 2D. This
adds a **3D** view next to the existing 2D preview (`2D | 3D` above the
canvas) that draws every lit point as a beam from a projector into a hazy
room.

- `studio/src/beam3d.js` (new, ES module, ~560 lines, **raw WebGL2, no
  library**), served by `web.rs` at `/beam3d.js` (`text/javascript`, via
  `include_str!`) and loaded by the page with `import()` on the first click
  on *3D*. 2D-only users never load it.
- **Geometry**: room box 10 × 15 × 5 m, projector at the stage edge
  (0, 3, 11) facing the audience (-z), scan angle 40°. Point (x, y) →
  `yaw = x·θ/2`, `pitch = y·θ/2` in the projector frame (`pointToDir`),
  ray/box exit (`rayBox`) gives the beam end and the spot. Hard-coded model,
  as the task asks (persistent venue = T-277).
- **Energy conservation**: each sample has weight `1/N` (N = all samples,
  blanked included), times *Exposition*. Dwell points stack, so a static
  beam is bright and a 200-point sweep is a faint plane. Rendered into a
  half-float (RGBA16F) target so thousands of 1/N contributions add up,
  then `1 − exp(−x)` + gamma.
- **Beams** = one instanced screen-aligned quad per lit point (clipped at
  the near plane in the vertex shader); **sheets** = triangle projector →
  hit i → hit i+1 per consecutive lit pair; **spots** = additive point
  sprites at the hits (not affected by haze); **haze** = density × `exp(−σ·d)`
  with optional drifting 3D value noise (*Brume animée*, procedural, no
  texture); **room light** scales the floor/grid/box; **halo** = separable
  Gaussian at 1/2 and 1/4 resolution added back.
- Controls (French): *Exposition* (tooltip: design aid, not a photometric
  measure), *Brume : densité*, *Lumière de salle*, *Nappes*, *Impacts*,
  *Halo*, *Brume animée*, *Qualité* (*Auto / Basse / Moyenne / Haute*),
  *Recentrer*, hint *Glisser pour tourner, molette pour zoomer,
  Maj+glisser pour déplacer*. Orbit / pan / zoom with mouse, trackpad and
  touch (pinch); double-click recentres. Render settings are remembered in
  `localStorage` (per browser, try/catch) until T-277 stores them in the
  venue.
- Quality levels: Basse = no sheets/halo/noise, scale 0.5; Moyenne = sheets
  + 1 halo level, 0.75; Haute = all, 1.0. *Auto* starts at Haute and drops
  one level when frames exceed 20 ms for 2 s (never climbs back, no
  oscillation). The user's switches can only turn features off.
- Without WebGL2: *Visualiseur 3D indisponible sur ce navigateur*, 2D stays.

**Library choice (T-275 spike).** The research recommended vendoring
three.js. I went with raw WebGL2 instead: every pass needs a custom shader
anyway (1/N weights, haze attenuation, noise, sheets), and three.js bloom
means vendoring 5–6 example files (EffectComposer, passes, shaders) on top
of a ~700 kB core. Raw WebGL2 is one file of our own, no third-party code,
so **nothing is added to `docs/CONTENT_SOURCES.md`** and the offline
requirement is met by construction. If a later task wants three.js (gizmos,
GLTF venues), it can be vendored then.

**Preview only.** The page's existing `/api/frame` loop (one fetch, 30 Hz)
hands `f.points` to either the 2D `draw()` or `view3d.setFrame()`; the
module contains no `fetch`, `XMLHttpRequest`, `WebSocket`, `/api/` or
`import(` (enforced by a Rust unit test on the served file). It cannot arm
or send. The render loop (`requestAnimationFrame`) runs only while 3D is
shown, and stops when switching back. The engine and web server are
untouched apart from one static route.

2D view: unchanged code path; while 3D is shown the hidden 2D canvas is not
redrawn, and switching back redraws the last frame immediately. The
`#offBadge` moved into a `.view` wrapper with both canvases (still top-left
of the image).

## Testing
- `cargo test -p laser-studio`: green (new `web::tests`: page + module are
  served with the right types; the module has no way to reach the server).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `studio/e2e/tests/beam3d.spec.ts` (4 tests):
  1. click *3D* → WebGL2 canvas visible, lit pixels drawn, render bar and
     hint shown, still disarmed; click *2D* → 2D canvas visible and drawing
     the circle again; **no console errors, no request outside the
     studio's origin**.
  2. an *Éventail* cue becomes beams whose hits spread > 4 m across the
     back wall (screenshot saved to `test-results/beam3d-fan.png`).
  3. pure JS: `pointToDir(0,0)` = (0,0,-1) horizontal, `rayBox` hits the
     back wall at 11 m; x = 1 → 20° right, y = 1 → 20° up; `beamAlpha`;
     sheets = consecutive lit pairs (a blank breaks one); `AutoQuality`
     stays Haute at 16 ms, goes Moyenne after 2 s at 25 ms.
  4. pixels (`readPixels`): static beam (200 repeated points) > 150 and
     > 3× a 200-point sweep at the same spot; haze 0 → beam pixel 0, the
     wall spot still lit.
- After rebasing on develop 6e83091: `cargo test` 269 passed (2 ignored), clippy clean, full e2e suite **66/66 passed**.
- Performance (`draw()` + 1-pixel readback to force GPU completion, median
  of 40, headless Chromium with `--use-angle=metal`, Apple M3 Max, haze
  noise on): 3 000 pts Haute 900 px **2.6 ms**; 4 000 pts Haute 900 px
  **2.7 ms**, 1 800 px **5.2 ms** (≤ 16 ms budget); Moyenne 1.1 ms, Basse
  1.0 ms. Geometry build 0.2–0.3 ms for 3–4 k points (budget ≤ 1 ms). In
  the default e2e headless mode (SwiftShader software GL) the same frames
  take 43–85 ms, which is exactly what *Auto* is for.

## Risks
- Default e2e Chromium renders with SwiftShader; the pixel test thresholds
  are chosen with margin but depend on half-float targets
  (`EXT_color_buffer_float`, present in both SwiftShader and Metal). Without
  it the view falls back to RGBA8 and very faint sheets can vanish.
- Beam brightness gains (`BEAM_GAIN`, `SHEET_GAIN`, `SPOT_GAIN`) are tuned
  by eye; it is a design aid, not photometry.
- *Auto* never raises quality again; to go back up, pick a level or
  re-select *Auto*.
- Camera orientation: seen from the audience, image x+ appears on the
  viewer's left (the projector's right), which is physically correct but
  may surprise at first.
- Only the stats line (`Auto : Haute · 2.6 ms`) is shown for performance;
  no FPS counter yet.

## Review

Reviewed by the architect (integrator). Clean merge on 6e83091; 269 unit
+ 66 e2e green, clippy clean. Checked in real Chrome (GPU): 3D switch
shows the room, projector and a « Faisceaux » cue's 8 beams in haze, no
console errors; 2D unchanged. Accepted: raw WebGL2 module of our own
instead of vendoring three.js (no third-party file, works offline). The
module has no network access (enforced by a unit test) and never feeds
the laser.
Verdict: APPROVED
