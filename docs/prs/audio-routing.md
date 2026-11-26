# feat/audio-routing — T-153 audio routes to any control

## What / why
Until now only a look's own `AudioReact` (size, rotation, flash from the
bass) followed the music. A pro wants to choose the band or event and
the parameter it drives. This wires T-238's `audio::shape` into the
engine: any `AudioFeatures` value or event → `Shaper` → any control an
LFO may move.

- `studio/src/audio/routes.rs`:
  - `AudioRoute { source, target, shape: Shaper, enabled }`
    (`#[serde(default)]`, default `bass → master.size`). `source` is an
    `AUDIO_VALUES` / `AUDIO_EVENTS` id (19 sources with French labels in
    `SOURCES`: basses, sub, bas-médiums, médiums, aigus, niveau, montée,
    forces kick/caisse/charleston, brillance, confiance du tempo; events
    kick, caisse claire, charleston, temps, attaque, drop). `shape.max` is
    the *Quantité* (−1..1 of the target's range, negative pulls down).
  - `AudioRouting { mix, routes }`: the saved form. `mix` is the *Temps ↔
    Audio* crossfader (0..1, default 0.5): routes are scaled by
    `min(1, 2·mix)`, LFOs by `min(1, 2·(1 − mix))`, so the default leaves
    both in full and LFO behaviour is unchanged.
  - `validate`: at most 16 routes, known source, target bound through
    `shape::Target::bind` (= the LFO allow-list). French 400 messages
    (« « transport.arm » ne peut pas être piloté par l'audio »).
  - `RouteStore`: `audio_routes.json` in the data dir, sanitised
    (`Shaper::sanitized`, mix clamped, NaN → defaults), unknown
    sources/targets dropped on load. **Binding happens only when routes
    change** (`set`, `replace_in_memory`, load): sources parsed, targets
    bound, routes grouped per target. A route whose source and target are
    unchanged at the same index keeps its running envelope (new
    `Shaper::retune`), so dragging a slider doesn't restart it.
  - `RouteStore::apply` (each frame): every route `feed`s on the frame's
    features with the real `dt` and the clock's beat length; enabled
    routes on the same control are **summed then applied once**
    (`base + Σ`, clamped once, as the research §6.2 asks) through
    `Target::apply` → `lfo::offset`. Allocation-free (tested with the
    counting allocator). Disabled routes keep running for their meter
    (and so a counter primed long ago doesn't fire when re-enabled).
- `main.rs` engine loop: on the same per-frame copies as before, LFOs
  (`lfo::modulate_scaled` with the time share) then routes, then
  `recolor_live` if a colour target moved. Everything after (layers, live
  stage, calibration, strobe limiter, horizon, gate) is unchanged.
- `lfo.rs`: `modulate_scaled(…, share)`; `modulate` (share 1) is now
  test-only.
- `shape.rs`: `Shaper::retune`; the `dead_code` allowance is removed now
  that everything is used.
- Projects (`project.rs`): new section `audio_routes` (defaults when
  missing, so projects from before open with no routes and the same
  fingerprint), validated on open with the registry (refused target → 400,
  nothing changes), written to the working copy, `audio_routes.json` in
  the first-start import list.
- API: `GET /api/audio/routes` → `{routes, mix, sources:[{id,label,event}],
  targets:[{id,label,group,min,max}], max}`; `POST /api/audio/routes`
  takes `{mix, routes}` (400 + French message when invalid).
  `/api/frame` gains `audio_routes: [{value, input, open}]` (shaped output
  −1..1, continuous input 0..1 or `null` for events, gate open).
- UI: a separate « Liens audio » block under LIVE › Modulateurs (its own
  `<section data-panel="lfo" id="arPanel">`, nothing touched in
  « Musique »): *+ Lien audio*, *Temps ↔ Audio* slider, and per route two
  live meters (entrée / sortie, input dimmed while the gate is shut),
  *Source* (grouped *Continu* / *Évènements*), *Cible* (allowed controls
  only), *Quantité* −100..100 %, *Seuil*, *Courbe* (Linéaire, Carré,
  Racine, En S), *Attaque*, *Relâche* (continuous) or *Déclin* (events),
  each in ms or *temps*, *Actif*, ×.

### Safety
Same rules as LFOs, by construction (`Target::bind` + `lfo::offset`):
engine copies only (`live.size` in `/api/live` stays the operator's),
never transport/arm/blackout, tempo, cues, grid, toggles, calibration or
safety settings; brightness only dims below the fader; audio-driven
flashes go through the T-101 strobe limiter like everything else (the
render path after the routes is unchanged). Nothing can arm.

### Deviations from the task file
- The data model follows T-238 (the task file predates it): a `Shaper`
  instead of `amount / attack_ms / release_ms / gate`, sources are the
  T-237 ids (a superset of `Low/Mid/High/Level/Beat`), no new
  `AudioFeatures` field was needed (the browser's `bass`/`level`/`beat`
  already fill bands and counters since T-237).
- **`AudioReact` is not converted into routes.** It belongs to each look
  (scene, cue, timeline event) while routes are master-level; converting
  would make one scene's reaction apply to every look after it. It keeps
  rendering exactly as before (T-237's identical-frames test still
  passes), which satisfies the non-regression criterion; routes come on
  top. A converter can be added in T-239 presets if wanted.
- The three band meters (*Basses / Médiums / Aigus*) are T-243's
  « Musique » panel (merged separately); this block only shows per-route
  meters.

## Testing
- `cargo test -p laser-studio`: **694 passed** (rebased on 8d2cd1d), 7 ignored (+2 +2
  integration). New:
  - `audio::routes` (12): every feature id has a source label; `bass →
    master.size` follows the bass and nothing else (level, mid, high,
    kick, snare, build-up at full leave settings/live untouched);
    one-frame bass pulse rises by the attack and falls to 1/e in 150 ms
    ± 1 frame, monotonic; kick route pulses on its own frame and decays;
    routes on one target sum before the clamp; disabled route moves
    nothing but meters; crossfader at 0 / 0.25 / 0.5 / 0.75 / 1; refused
    targets (arm, blackout, tempo, cue, grid, toggle, calibration,
    safety, unknown), unknown source, > 16 routes; audio never arms and
    only moves copies, brightness only dims; store validates, sanitises,
    writes nothing on refusal, reloads, drops routes no longer allowed;
    editing a route keeps its envelope, changing its source restarts it;
    serde defaults; 600 frames of 4 routes (colour, brightness, two on
    size) allocate nothing.
  - `lfo`: the time share scales modulators (1, 0.5, 0, >1).
  - `project`: routes saved in a project come back on open (memory and
    `audio_routes.json`), a project without `audio_routes` opens with
    none and unmodified; refused route target / unknown source → 400,
    nothing changes.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `npm --prefix studio/e2e test`: **181 passed** (new
  `audio-routes.spec.ts`, 4 tests): API lists sources/targets and
  refuses `transport.arm`, blackout, `tempo.bpm`, `cue.max_active`,
  calibration, an unknown source and 17 routes; a route added in the UI
  (*+ Lien audio*, *Quantité* 25 %) makes `/api/frame` grow from 0.5 to
  0.75 with browser `POST /api/audio` bass 1, the meter moves,
  `/api/live` size stays 1, never armed; *Actif* off and *Temps ↔ Audio*
  at 0 bring it back to 0.5; routes survive a restart (API + UI) and a
  project save-as → change → open; *Nouveau* project has none.

## Risks
- The UI posts the whole list on every slider move (like LFOs); each
  post rebinds (small allocations on the HTTP thread, under the lock),
  running envelopes are kept.
- `/api/frame` meters are indexed like `routes`; a route dropped at load
  (target no longer allowed) disappears from both.
- LFO and route offsets on the same control are applied in two steps
  (LFO sum clamped, then route sum clamped). Within the routes it's one
  sum as specified.
- The *Temps ↔ Audio* crossfader doesn't scale a look's own
  `AudioReact` (kept as before on purpose).
- Rotation-speed targets inherit the LFO caveat with *Synchro tempo* (the
  speed is in turns per bar there).

## Review

Merged into develop by the integrator. Suite on the merged result: cargo test 694 passed, integration 2+2, clippy clean, e2e 181 passed (--workers=2).

Verdict: APPROVED
