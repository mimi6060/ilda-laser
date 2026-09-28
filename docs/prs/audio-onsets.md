# feat/audio-onsets — T-232 Onsets (spectral flux) and kick / snare / hi-hat

## What / why
The legacy `beat` fired on any bass swell, bass lines included. The
native analysis thread now finds real onsets and says whether each is a
kick, a snare or a hi-hat, every hop (256 samples, 5.3 ms at 48 kHz),
from the power spectrum `spectrum.rs` already computes (no second FFT).

New `studio/src/audio/onsets.rs` (`OnsetDetector`, owned by
`analysis::Analyzer`, so it runs on the **analysis** thread, never in the
audio callback):

- **ODF**: log-compressed (`log10(1 + 10⁴·|X|)`, |X| scaled so a
  full-scale sine reads 1), half-wave rectified spectral flux (Bello
  2005, Dixon 2006), **SuperFlux** form (Böck & Widmer 2013): the
  reference is the frame 2 hops back, max-filtered over ±1 bin. Computed
  over the whole spectrum and per band (40–150 Hz, 150 Hz–5 kHz,
  5–15 kHz), each as the mean over its bins, so one `delta` means the same
  in every band. The whole-spectrum ODF of the last 8 s is kept in a ring
  (`odf_history()`, `odf_rate()`) for T-233.
- **Peak picking**, causal: local max over the previous 30 ms and the
  `lookahead_hops` after (0–2, default 1), ≥ mean of the previous 100 ms
  + `delta`, ≥ 30 ms since the last onset of that function (kick:
  `kick_refractory_ms`, 100). A candidate the rules reject does **not**
  start the refractory (otherwise low-band noise just before a kick
  hid it).
- **Rules** (on the energy an onset brings: band power over the peak and
  look-ahead hops against the ~26 ms before):
  - *kick*: low-band candidate, low-band power ≥ 9 dB over its mean of the
    previous ~85 ms (fast rise; a bass line changing note or a kick tail
    beating with a bass note doesn't), new low energy ≥ ½ the new energy
    above 150 Hz and ≥ 0.2 × the power above 150 Hz (a kick is a big
    share of the mix; the 3 low bins flicker in noise);
  - *snare*: mid-band candidate, ≥ 40 % of the 500 Hz–5 kHz bins rose
    ≥ 3 dB within 30 dB of the band's loudest bin (a noise burst:
    "flatness"; computed on the new energy, because a snare's own 200 Hz
    body ruins the plain spectral flatness), its 1–5 kHz new energy
    ≥ 0.003 × the low band's (not a kick's leakage), and not hat-like;
  - *hi-hat*: high-band candidate whose new energy per bin in 5–15 kHz is
    ≥ 6 dB above that in 1–2 kHz ("little energy below 2 kHz"; the
    range under 1 kHz is left out so a kick under a hat doesn't hide it).
- Output `Onsets { onset, kick, snare, hat (u64 counters),
  onset/kick/snare/hat_strength (0..1: the peak over the band's loudest
  onset of the last ~5 s), last_onset/kick/snare/hat_t (audio time of
  the ODF peak, studio clock) }`. No event while `silent`.
- `OnsetConfig { delta: 0.1, lookahead_hops: 1, kick_refractory_ms: 100 }`
  (`#[serde(default)]`, sanitised: delta 0.01..2, look-ahead ≤ 2,
  refractory 30..1000 ms) lives in `AnalysisConfig.onsets`, i.e.
  `audio.json` → `analysis.onsets`. `POST /api/audio/config` patches it
  field by field (`{"analysis": {"onsets": {"delta": 0.2}}}`); the merge
  now recurses into nested objects, still 400 on an unknown field or a
  wrong type. Changing it never reopens the input.

Wiring:
- `analysis.rs`: the old bass-swell beat heuristic is gone; **the legacy
  `beat` is now the kick counter** (task acceptance: `beat` = `kick` for
  the native source). `level` and `bass` are unchanged. `Meter.onsets`.
- `worker.rs`: the analysis thread carries all onset counters across a
  stream reopen (as it did `beat`); `NativeSnapshot.onsets` is published
  with the rest (same leaf mutex held for a copy: nothing new blocks the
  capture or the engine).
- `/api/state.audio.onsets` = the counters when the native capture is
  fresh, else `null` (like `spectral`). `AudioFeatures` is untouched:
  T-237 brings the new fields to the engine.
- Not done (deliberately): the UI (*Kick / Caisse / Charleston* lights,
  *Sensibilité*): `index.html` is T-243's. The API above is all it needs.

## Testing
- `cargo test -p laser-studio`: 557 passed, 6 ignored (+ 2 in
  `tests/shutdown.rs`), after rebasing on develop 654c35e. New tests, all
  on signals generated in the tests (stateless hash noise; kick = sine
  gliding 150 → 50 Hz, snare = noise + 200 Hz, hat = second difference
  of noise, clicks, bass line), no device:
  - 128 BPM pattern (kick every beat, snare on 2 and 4, eighth hats, over
    a −60 dBFS noise bed), 8 bars, at three offsets off the hop grid,
    ±50 ms: **kick F 0.97–1.0 (≥ 0.95), snare 0.97–1.0 (≥ 0.85), hat
    0.86 (≥ 0.8)**; whole-spectrum onsets one per eighth. Same result
    30 dB quieter.
  - **Latency** (sample of the transient → end of the hop that reports
    it): every kick and every click ≤ 25 ms, never early (mean ≈ 14 ms
    with look-ahead 1, ≈ 4 ms with 0); `last_kick_t` within ±15 ms of the
    transient; 44.1/96 kHz ≤ 25 ms, 16 kHz ≤ 65 ms (16 ms hops).
  - A legato bass line (41–110 Hz, a note per eighth) gives no kick
    (the first note out of silence may), also with hats and snares on
    top (hats still found); a kick over a held 55 Hz bass is still found.
  - Each drum alone is only itself (8 kicks → (8, 0, 0), etc.).
  - False positives over 10 s: white noise, a six-note chord, digital
    silence → no kick/snare/hat, ≤ 1 onset; loud brown rumble → ≤ 3 kicks
    (see Risks).
  - Configurable kick spacing and `delta`; counters carried, strengths in
    0..1; 8 s ODF history; settings sanitised; **no allocation per hop**
    (whole `Analyzer` under the counting allocator).
  - `onset_cost_is_small` (`#[ignore]`, `--release -- --ignored
    onset_cost`): **0.13 % of a core** on this M3 (spectral: 0.24 %).
  - hub: nested patch / validation of `analysis.onsets`;
    `/api/state.audio.onsets` null when not fresh, counters when fresh;
    `beat` = kick; worker: all counters survive a reopen.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: `studio/e2e/tests/audio.spec.ts` updated (`analysis.onsets` in the
  config, `onsets` null with `--no-audio`) + one new test (onset settings
  patched, validated, kept across a restart). Full suite green:
  **145 passed**.

## Licences
No new crate. Everything is our own code written from the papers'
descriptions (Bello et al. 2005; Dixon 2006; Böck, Krebs & Schedl 2012;
Böck & Widmer 2013); no GPL source (aubio, BTrack, madmom, …) was read.

## Risks
- **`beat` changes meaning for the native source**: it now counts kicks
  instead of bass swells. That was the point, but a track with a soft or
  sidechain-less kick under a loud sustained sub may give fewer beats
  than before (a kick must jump ≥ 9 dB in 40–150 Hz). The browser
  source's `beat` is unchanged.
- Thresholds were tuned on **synthetic** drums only; real music (T-244
  corpus) must confirm them. Known weak spots: a loud low rumble (brown
  noise) gives ~0.3 false kicks/s; a hat that lands together with a snare
  is not counted (the snare's noise hides it: hat F 0.86 on the pattern);
  a lone click reads as a snare; kicks closer than ~250 ms on a long
  decaying kick may be missed (the second doesn't rise 9 dB).
- At 96 kHz the 1024-point FFT gives a single 40–150 Hz bin; at 16 kHz
  latency is ~50 ms. 44.1/48 kHz are the targets.
- `delta` is in mean log10 flux per bin: not a percentage; the UI
  (T-243) should map *Sensibilité* onto it (e.g. 0.3 … 0.03).
- `/api/state.audio` grows by ~250 bytes (`onsets`); `audio.json` gains
  `analysis.onsets` (old files load with the defaults).
- Safety: unchanged. Audio only feeds `AudioFeatures` (still the legacy
  three fields); the strobe limiter, calibration and output gate come
  after as before.

## Review

Reviewed by the architect (integrator). Clean merge on 654c35e; 557 unit
+ 2 signal tests, e2e 145/145, clippy clean. No new crate; algorithms
from the papers. Accepted: native `beat` = kick counter (browser source
unchanged). Thresholds tuned on synthetic drums only — must be checked on
real music (T-244) before relying on kick-triggered effects in a show.
Verdict: APPROVED
