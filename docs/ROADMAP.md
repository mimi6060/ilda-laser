# Laser Studio roadmap

Synthesised from `docs/research/pangolin.md`, `docs/research/showcontroller.md`
and `docs/research/madmapper-and-open-content.md`. Each item is one branch
(`feat/<id>`), small enough for one developer agent, with acceptance
criteria the reviewer checks. Items in the same wave touch different files
so they can be built in parallel.

Key constraints from research:
- No Pangolin or Laserworld content can be bundled (EULA §10, no licence on
  Laserworld packs). Shipped content is procedural; users import their own
  ILDA files at runtime. Allowed bundled content: Hershey fonts (`.jhf`,
  with credit) and Relief SingleLine (OFL 1.1).
- `laser-dac`'s FrameSession already blanks between frames (dwell + eased
  move), applies a 150 µs colour delay and a 1 ms startup blank. The engine
  must not duplicate those.
- Targets: 30 kpps, ≥ 45 fps ideal / flicker below ~35 fps → ~500–850 points
  per frame.
- Keyboard: Space = laser on/off and Escape = blackout stay reserved for
  safety. Tap tempo goes elsewhere (e.g. `T`).

## Wave 1 — foundations (parallel)

### W1-ilda — ILDA reader/writer (`studio/src/ilda.rs`)
Own implementation (the `ilda` 0.2.0 crate is buggy; don't use it).
- Read formats 0, 1, 2 (palette), 4, 5; big-endian; blanking bit 6 overrides
  colour; default 64-colour palette; per-file palette from format 2;
  tolerate truncated files with an error (never panic); ignore trailing
  empty frame.
- Write format 5 (true colour, 2D).
- `shownet_sd_name(n)` helper: valid names `001.ild`–`229.ild` (000 empty,
  230–255 reserved), and a check that files stay ≤ 8 MB.
- Unit tests generate their own `.ild` bytes (no third-party files).
- Not wired into the UI yet (W2-ilda-ui).

### W1-optimize — point optimiser (`studio/src/optimize.rs`, engine hook)
Replace `engine::densify` with a configurable optimiser:
- `OptimizeParams { lit_step: 0.025, blank_step: 0.15, corner_rad_per_point: 0.6, max_corner_dwell: 8, endpoint_dwell: 3 }` (serde defaults).
- Lit segments split at `lit_step`; blanked moves eased (smoothstep) with
  larger steps; corner dwell = `ceil(angle / corner_rad_per_point)` capped.
- Stable nearest-neighbour ordering of disjoint strokes (keep the first
  stroke first; deterministic).
- Don't add inter-frame blanking (laser-dac does it).
- Keep all existing engine tests passing (update expectations where the
  new maths legitimately changes counts); add tests for eased blanking,
  angle-proportional dwell and ordering.

### W1-safety — safety zones, horizon dimmer, colour calibration
New `studio/src/safety.rs`, applied in `run_engine` **after** geometric
calibration, before sending:
- Up to 5 polygon zones, each `mode: Blank | Dim(f32)`; points inside are
  blanked or dimmed; segments crossing a blank-zone edge must not stay lit
  inside the zone (split at the boundary or blank the whole segment).
- Horizon dimmer: below a configurable Y, brightness scales down to a
  minimum (audience-scanning guard), with a soft ramp.
- Per-colour gain (R, G, B) and a minimum diode level (points with any
  colour above 0 get at least that level on that channel).
- Persisted in `studio-data/safety.json`; `GET/POST /api/safety`.
- UI: a "Sécurité" section (French) with horizon + colour gains; zones can
  be edited as a JSON textarea for now; preview draws zones as translucent
  red overlays.

### W1-e2e — end-to-end click tests (`studio/e2e/`)
- Playwright (headless Chromium) project in `studio/e2e/` with
  `npm test` → builds nothing, expects `target/debug/laser-studio` (the
  test script runs `cargo build -p laser-studio` first), starts it on a
  free port with a temp `--data-dir`, never with `--device`.
- Tests: page loads; clicking a shape changes `/api/state` content and
  `/api/frame` points; text input renders points; colour/size sliders
  change frame; laser button + Space toggle `armed`, Escape forces
  `armed=false`; save scene → appears in list → play → delete; playlist
  start advances after duration; calibration slider persists across a
  restart of the studio.
- `node_modules/`, Playwright reports ignored in git.

## Wave 2 — show control

- **W2-animator** — Showcontroller-style universal modulator: any numeric
  setting can be driven by `{ waveform: sine|triangle|square|saw|random, rate (Hz or beats), depth, phase }`; replaces ad-hoc rotation/audio code paths while keeping old scenes loading.
- **W2-effects** — effect stack applied per look: prism (N copies rotated),
  draw-on/erase, hue cycle, sparkle, mirror X/Y, wave warp. Each effect has
  parameters that the animator can modulate.
- **W2-cuegrid** — cue grid (8×5 per page, 10 pages) replacing the flat
  scene list: keyboard-mapped keys, click modes Toggle / Flash / Solo,
  groups (one active cue per group), hover preview thumbnails.
- **W2-tempo** — BPM: tap tempo (`T`), beat-synced animator rates, beat
  phase; server keeps the tempo; audio beat detection refines it.
- **W2-hershey** — Hershey `.jhf` fonts + Relief SingleLine for text,
  with `docs/CONTENT_SOURCES.md` entries; font picker in the UI.
- **W2-ilda-ui** — import `.ild` files into a media library (stored in
  `studio-data/media/`), play as content (skip our optimiser for imported
  frames), export any look as format-5 `.ild` with ShowNET-safe naming.

## Wave 3 — pro

- **W3-zones** — projection zones: per-output geometry (keystone, pincushion,
  bow, 4-corner warp), cue → zone routing, multiple outputs.
- **W3-presets** — our own procedural preset library (beams, tunnels,
  abstracts, spirograph/lissajous, clock, audio spectrum), organised in
  cue-grid pages.
- **W3-control** — MIDI in (APC40/APC mini mapping, right-click learn), OSC
  in, Art-Net/DMX remote chart.
- **W3-timeline** — timeline with audio file playback, cue events, MTC/LTC
  timecode later.
- **W3-shownet** — ShowNET `Output` from Laserworld's API (blocked on NDA).
