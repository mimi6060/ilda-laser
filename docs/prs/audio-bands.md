# feat/audio-bands — T-231 Spectral analysis: 5 bands, dBFS, auto-gain

## What / why
One `bass` value in 0..1 with a fixed −60..−10 dBFS mapping isn't enough:
looks need bands that are stable from one track and one room to the
next. The native analysis thread (T-230) now also computes, every hop
(256 samples, 5.3 ms at 48 kHz):

New `studio/src/audio/spectrum.rs` (`SpectralAnalyzer`, owned by
`analysis::Analyzer`, so it runs on the **analysis** thread, never in the
audio callback):

- **Five bands** `sub` 20–60, `bass` 60–150, `low_mid` 150–500, `mid`
  500–2000, `high` 2000–12000 Hz, by IIR filters on the time signal (a
  1024-point FFT has 47 Hz bins, too coarse for sub vs bass). Each band =
  Butterworth high-pass + low-pass written from the RBJ cookbook in f64,
  **8th order on the inner edges** (so a 40 Hz sine sits 28 dB down in
  `bass`), 2nd order at 20 Hz, 4th at 12 kHz; an edge past 0.45 × the rate
  is skipped (16 kHz inputs). Power smoothed per band (τ 50/30/15/10/10
  ms, about a period of the band's lowest note) → `bands_db` in dBFS.
- **Auto-gain per band** → `bands` 0..1: ceiling = peak, instant attack,
  release τ 7 s (towards the current level, never aiming more than 48 dB
  below); floor = 5th percentile of 10 s (one mean-dB value per 100 ms
  block, 100 entries, `select_nth_unstable` 10×/s). The range spans at
  least 12 dB (a steady tone at the ceiling reads 1, not 0) and at most
  48 dB. Two limits of my own, needed for the acceptance criteria and
  for the laser: a band's ceiling never sits more than **10 dB under the
  loudest band's** (a sine's leakage or a track without highs is not
  boosted to full scale), nor under `silence_db + 6` (room noise stays
  low). **Frozen while `silent`**, so the gain comes back slowly after a
  silence instead of blowing up the background.
- **Manual gain** (`auto_gain: false`): `bands = (band_db + manual_gain_db
  + 60) / 50`, i.e. the browser bass meter's −60..−10 dBFS. The
  auto-gain state keeps tracking underneath, so switching back is
  immediate.
- **FFT** `realfft` N = 1024, Hann, every hop; plan, window and buffers
  allocated once (a test with the counting allocator from T-230 proves a
  hop allocates nothing). Gives `centroid_hz` and `flatness` (for T-232 /
  T-236) and keeps the power spectrum (`power()`, for T-232's flux).
- **Silence**: every hop's own RMS under `silence_db` (−60 by default)
  for 300 ms → `silent = true` (judged per hop, not on the smoothed
  meter, so it comes on in 300 ms ± 1 hop whatever came before).
- Samples are sanitised (NaN/inf → 0, clamped to ±4 = +12 dBFS).

`SpectralFrame { t, bands: Bands, bands_db: [f32; 5], level_db,
centroid_hz, flatness, silent }` as in the task, `Bands { sub, bass,
low_mid, mid, high }`, `AnalysisConfig { auto_gain: true, manual_gain_db:
0, silence_db: −60 }` (`#[serde(default)]`, sanitised: gain −40..40,
threshold −100..−20).

Wiring:
- `NativeSnapshot.spectral` is published with the rest after each batch
  of hops (same leaf mutex held for a copy: nothing new blocks capture or
  the engine).
- `AudioConfig.analysis` (saved in `audio.json`; old files load with the
  defaults). `POST /api/audio/config {"analysis": {...}}` patches field by
  field, 400 on an unknown field or a wrong type. **An analysis change no
  longer bumps the capture generation**: only source/device/buffer
  reopen the input. The analysis thread reads the settings (a `Copy`)
  once per poll that has hops.
- `/api/state.audio.spectral` = the frame when the native capture is
  fresh, else `null`.
- `level`, `bass`, `beat` (T-230's browser-compatible heuristics) are
  **unchanged**, so current looks react exactly as before. T-237 moves the
  legacy `bass` onto the normalised band and `beat` onto `kick` (T-232).

Not done (deliberately): the UI (five meters, *Gain automatique*,
*Gain*, *Silence*): `index.html` is being edited by other agents; it's
T-243. The API above is all it needs.

## Testing
- `cargo test -p laser-studio`: 508 passed, 4 ignored (+ 2 in
  `tests/shutdown.rs`). New tests, all on synthetic signals, no device:
  - spectrum: 40 Hz → `sub` > 0.9 and every other band < 0.1 (and its
    dBFS is the sine's); 5 kHz → `high` > 0.9, others < 0.1; ten sines
    (30 Hz … 9 kHz) land in their band, by dBFS and normalised, at the
    right level; the same synthetic track (kick, noise, hats, chord) at
    −30 and −10 dBFS gives bands within ±0.1 after 12 s; digital silence
    → `silent` at 300 ms ± 1 hop, bands 0, centroid/flatness 0; ceiling
    frozen during 20 s of silence, a note 26 dB quieter afterwards reads
    low, then catches up; hard clipping, NaN, ±inf, ±1e30 stay bounded
    and finite; manual gain replaces auto and back; configurable silence
    threshold; centroid of 1 kHz ≈ 1 kHz, tone flatness < 0.05, white
    noise > 0.4; 16 / 44.1 / 96 kHz rates; no allocation per hop.
  - `spectral_cost_is_under_2_percent_of_a_core` (`#[ignore]`, run with
    `cargo test -p laser-studio --release -- --ignored spectral_cost`):
    **0.21 % of a core** on this M3 (30 s of audio in 64 ms).
  - analysis: every hop carries its spectral frame; manual gain reaches it.
  - worker (fake source): the published snapshot has `mid` > 0.9 for a
    1 kHz tone; an analysis setting reaches the running analyser without
    reopening the stream.
  - hub: analysis patch field by field / validation / old files; an
    analysis change keeps the capture generation; `/api/state.audio.
    spectral` present only when the native capture is fresh.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: `studio/e2e/tests/audio.spec.ts` updated (config now has
  `analysis`; `spectral` is `null` with `--no-audio`) + one new test
  (analysis patch, validation, persistence across a restart). Full suite
  green: 137 passed.

## Licences (new crates)
Checked from each crate's `Cargo.toml` `license` field (2026-09-28,
`cargo metadata`); no GPL/AGPL.

| Crate | Version | Licence |
|---|---|---|
| realfft | 3.5.0 | MIT |
| rustfft | 6.4.1 | MIT OR Apache-2.0 |
| num-complex | 0.4.6 | MIT OR Apache-2.0 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| primal-check | 0.3.4 | MIT OR Apache-2.0 |
| strength_reduce | 0.2.4 | MIT OR Apache-2.0 |
| transpose | 0.2.3 | MIT OR Apache-2.0 |
| (already present: num-traits, autocfg) | | MIT OR Apache-2.0 |

The filters, auto-gain, centroid and flatness are our own code from the
textbook definitions (RBJ Audio EQ Cookbook, Butterworth pole angles).

## Risks
- **Latency of the low bands**: 8th-order edges at 60 and 150 Hz have a
  group delay of roughly 15–25 ms near the edges (plus the 30–50 ms
  smoothing). Fine for continuous modulation (size, brightness); kick
  timing is T-232's job (FFT flux), and T-246 compensates latency.
- The **two extra auto-gain limits** (≤ 10 dB under the loudest band,
  ≥ silence + 6 dB) are my additions: on a very bass-heavy mix the
  `high` band may top out below 1 (e.g. ~0.7). Tunable constants in
  `spectrum.rs`; worth listening to on real music (T-244 corpus).
- Room noise above `silence_db` (−60) is not silence: once a track has
  stopped and the ceiling has released (~10–20 s), noise louder than
  `silence_db + 6` gets normalised towards full scale. Raise *Silence* in
  a noisy venue (T-242 handles the input-level warnings).
- `/api/state.audio` grew by ~200 bytes (`spectral`); `audio.json` gains
  an `analysis` object (old files load).
- Safety: unchanged. Audio only feeds `AudioFeatures`, still the legacy
  three fields; the strobe limiter, calibration and output gate come
  after as before.

## Review
