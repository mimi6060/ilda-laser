# feat/audio-features-v2 — T-237 AudioFeatures v2: one complete snapshot for the engine and /api/state

## What / why
T-230 to T-233 compute bands, onsets and a tempo estimate on the analysis
thread, but the engine still received only `level` / `bass` / `beat`.
Every analysis now reaches the engine through one `Copy` struct, read once
at the top of each frame, and the UI can show it from `/api/state` without
analysing anything in the browser.

- `engine::AudioFeatures` v2 (`#[serde(default)]`, still `Copy`): the
  legacy `level`, `bass`, `beat` (always filled) plus `level_db`, `bands`
  (`sub`, `bass`, `low_mid`, `mid`, `high`, the T-231 `Bands`), the
  counters `onset`, `kick`, `snare`, `hat`, `drop`, `kick_strength`,
  `snare_strength`, `hat_strength`, `centroid_hz`, `silent`, `bpm` and
  `bpm_confidence` (the T-233 proposal; the clock is still `TempoClock`'s),
  `section`, `buildup`, and `t` (audio time on the studio clock).
  - `engine::Section { Silence, Normal, Break, Buildup, Drop }` (snake_case),
    the enum T-236 specifies. T-236 fills `section` / `buildup` / `drop`
    in `analysis::native_features`; until then the native source only says
    `silence` or `normal`, and `buildup` / `drop` stay 0.
  - `neutral()` (silence that keeps the counters, `bpm` and `t`),
    `sanitized()` (0..1, finite, `level_db` −120..+12, `bpm` 0..400),
    `value(id)` / `counter(id)` with the id lists `AUDIO_VALUES` /
    `AUDIO_EVENTS`, for T-153 routing and T-238 shaping.
- **Native source** (`analysis::native_features`, on the analysis thread,
  once per hop, no allocation): legacy `bass` = `max(bands.sub,
  bands.bass)` (the old meter was "everything under 150 Hz"; the T-230
  150 Hz low-pass is removed), legacy `beat` = `kick` (unchanged since
  T-232).
- **Browser source** (`audio::browser_features`, `POST /api/audio`):
  takes the old body `{level, bass, beat}` or any part of the v2 snapshot.
  What the old body doesn't send is filled where it means something:
  `bands.sub` = `bands.bass` = `bass`, `kick` = `onset` = `beat`,
  `level_db` from `level` (the page's level is RMS × 6), `silent` under
  −60 dBFS, `section` silence/normal, `t` = arrival. Everything is
  sanitised; a non-object or a wrong type → 400 (as before).
- **Staleness decays instead of jumping.** `AudioHub::frame()` (called by
  the engine once per frame, instead of `effective()`) passes fresh
  features through untouched; once the source is stale (> 500 ms, as
  before) or switched off, every continuous value falls to neutral with
  `y += (x − y)(1 − e^(−dt/τ))`, τ = 100 ms, on the real frame `dt`
  (neutral in ≤ 1 s, < 16 % of the value per 60 fps frame). Counters are
  held (going stale is never a beat). The state is a leaf mutex in the hub
  held for a copy; no allocation (tested).
- `/api/state.audio` gains, whatever the source: `bands` (always present,
  0..1), `silent`, `section`, `buildup`, `counters {beat, onset, kick,
  snare, hat, drop}`, `signals {<AUDIO_VALUES id>: 0..1}`, `detected_bpm`,
  `detected_confidence`, and `features` (the whole snapshot). Fresh
  features are shown as the next frame will use them; while falling back,
  the engine's last (decaying) frame. Existing keys are unchanged.
- `GET /api/audio/spectrum`: `{bands: 64, lo_hz: 20, hi_hz: 20000, t, db,
  values}`: 64 log bands (the loudest FFT bin of each, or the nearest bin
  where a low band is narrower than one), `db` in dBFS (a full-scale sine
  = 0), `values` over −90..0 dBFS → 0..1; nulls when no fresh native
  capture. Computed by the analysis thread once per poll (not per hop)
  into the snapshot (`NativeSnapshot.spectrum`, a `[f32; 64]`): the HTTP
  thread never touches the analyser.
- Not done (deliberately): `index.html` (the « Musique » panel v2 reading
  these fields is T-243); the page still posts the old format, which now
  fills the bands as above.

## Testing
- `cargo test -p laser-studio`: **610 passed**, 7 ignored (+ 2 + 2
  integration tests). New:
  - engine: a look saved before T-237 (audio reaction on: size, rotate,
    colour on beat, flash) rendered 90 frames through a swell and beats
    gives **identical frames** with legacy-only features, with the v2
    snapshot full of other values, and through the old `POST` body; old
    JSON loads, v2 round-trips, `neutral`, `sanitized`, signal ids.
  - hub: old / new / nonsense browser bodies; stale native audio stays
    until 500 ms, then falls monotonically (no step > 20 %) to < 0.01 by
    1 s with the counters kept, `/api/state` shows the falling value, new
    audio is taken at once; same for a stopped browser; `frame()` makes no
    allocation; spectrum served only when fresh; `bands` present and
    bounded in the view.
  - analysis: the native snapshot carries bands, counters, tempo,
    centroid, silence; legacy `bass` of a 40 Hz sine (in `sub`) > 0.9.
  - spectrum: a 1 kHz half-scale sine lands in band 36 at −6 ± 1.5 dBFS,
    far bands 40 dB lower, silence and 16 kHz at the floor, no allocation.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e (`audio.spec.ts`): *Aucune* test now polls the level down (it
  decays); three new tests: the new body → `/api/state.audio` (bands,
  counters, section, detected BPM, signals, bounds, 400s); the old body
  filled in, then the page stops and within ~1 s everything is neutral,
  `silent`, `section: silence`, counters kept; `/api/audio/spectrum` nulls
  with `--no-audio`. Full suite: **153 passed**.

## Risks
- **Native `bass` changes value**: it is now the auto-gained low end
  (T-231) instead of the fixed −60..−10 dBFS low-pass meter. Same range
  and meaning, but auto-gain makes a quiet input pump as much as a loud
  one, and the 10 dB-under-the-loudest-band limit makes a mix without
  low end read low. The browser source is unchanged.
- **Stale → neutral is now ~0.5 s slower to reach zero** (release after
  the 500 ms staleness instead of a jump). Switching the source to
  *Aucune* also fades instead of cutting.
- `section` defaults to `normal` when a browser body sends nothing and is
  not silent; T-236 should keep the `Section` enum in `engine.rs` (or
  re-export it) to avoid two definitions.
- `/api/state.audio` grows by ~700 bytes (`features`, `signals`,
  `counters`, `bands`). The spectrum endpoint is on demand.
- Safety: unchanged. Audio still only feeds the look renderer; live
  modifiers, calibration, strobe limiter and the output gate come after.

## Review

Reviewed by the architect (integrator). Clean merge on 4677ad1; 610 unit
+ integration tests, clippy clean; e2e 153/153 on two full runs (a first
run under a load average of ~11 from parallel agents had 4 timeouts that
did not reproduce). Accepted: native `bass` from auto-gained bands (check
on real music, T-244), stale audio fades instead of cutting, `Section`
lives in engine.rs (T-236 told to reuse it). Safety order unchanged.
Verdict: APPROVED
