# feat/audio-bpm — T-233 BPM estimation and beat tracking, with confidence

## What / why
Follow the DJ without tapping all the time, natively (works with the tab
closed). The analysis thread now estimates the tempo and tracks the beats
from the onset function T-232 already computes, and publishes a
`TempoEstimate` next to the onsets. It is a **proposal only**: nothing in
this branch touches `TempoClock` (T-234 will, with its own rules; tap
stays king).

New `studio/src/audio/bpm.rs` (`BpmTracker`, owned by `analysis::Analyzer`,
fed one value per hop, never in the audio callback):

- **Tempo ODF at ~100 Hz**: the sum of the low / mid / high band fluxes
  of `onsets.rs` (new `OnsetDetector::band_flux()`), not the
  whole-spectrum flux: in that one a hi-hat's hundreds of bins drown the
  kick's three and every eighth looks like a beat (100 BPM grooves read
  200, 180 read 120). Decimated by the whole number of hops closest to
  10 ms (2 at 44.1/48 kHz, 4 at 96 kHz) by their **mean**, 8 s kept.
- **Estimate every 0.5 s** once 3 s of sound are in: window smoothed
  (binomial, σ ≈ 1 frame), mean removed, weighted towards its recent end
  (exp, 4 s), autocorrelated through a zero-padded `realfft` FFT. Periods
  from 60/250 to 60/40 s on a 0.1-frame grid scored by the comb
  `A(τ) + ½A(2τ) + ⅓A(3τ) + ¼A(4τ)` × a log-Gaussian prior (125 BPM,
  σ = 1 octave); the ACF is read between lags by **cubic** interpolation
  (linear pulled every peak onto a whole lag: 140 read 140.5), the best
  period refined by a parabola.
- **Octave handling**: the comb + prior; then hysteresis on the followed
  tempo: within ±4 % it is followed at once (124 → 128), otherwise a new
  tempo must win for 2 s, a ×2 / ÷2 jump for 4 s. An estimate whose
  confidence (before stability) is < 0.25 never moves the BPM.
- **Confidence 0..1** = clarity (comb's normalised ACF at the period,
  0.15 → 0.5) × peak-to-average of the score (2 → 6) × *presence* (ODF
  variance of the last 2 s over the window's, 0.1 → 0.4: a break empties
  it long before the old drums leave the 8 s window) × stability of the
  last 4 estimates (spread 1 % → 1, 4 % → 0, and n/4 while fewer).
  Thresholds are ours, calibrated on the synthetic signals; T-244's corpus
  must re-check them.
- **Beats**: online DP (Ellis 2007, made causal as in Stark et al. 2009):
  `C(n) = 0.1·O(n) + 0.9·max_{d∈[P/2,2P]} W(d)·C(n−d)`, log-Gaussian `W`
  (tightness 5). Half a period after each beat, the score is run one
  period ahead with no onsets, weighted by a Gaussian around P/2, and its
  argmax is the next beat. Two frames after that beat, it is moved onto
  the neighbouring ODF peak (parabolic, sub-frame), the ODF delay
  (0.42 FFT window ≈ 9 ms at 48 kHz, measured) is subtracted →
  `beat_time`; `next_beat = beat_time + 60/bpm`. Beats keep being
  predicted through a break. When the first tempo appears, the DP scores
  the window already there, so the phase is there at once.
- **States** (`DetectState`, serialised snake_case): `no_input` while the
  spectrum says `silent` (BPM unchanged); `checking`; `locked` once the
  confidence stayed ≥ 0.6 for 2 s (stays locked while ≥ 0.4); `coasting`
  when a locked estimate drops under 0.4 (BPM frozen, beats predicted)
  until ≥ 0.6 for 2 s again; `guided` = checking with a guide set.
  > 3 s of silence or *Nouveau morceau* forgets history, lock and guide;
  the last BPM stays shown until the next estimate.
- `TempoEstimate { bpm, confidence, beat_time, next_beat, state }` as in
  the task (times are audio times on the studio clock, like
  `onsets.last_kick_t`; T-246 subtracts the analysis delay).

Wiring:
- `NativeSnapshot.tempo`, `/api/state.audio.tempo` (null when nothing
  fresh, like `onsets`). The analysis thread carries the last BPM across
  a stream reopen (`Analyzer::carry_bpm`).
- `POST /api/audio/tempo/new_track` → `AudioHub::new_track()` bumps an
  atomic counter; the analysis thread applies it at its next poll (no
  lock, never blocks the HTTP thread, capture or engine).
- `Analyzer::set_guide(Option<f32>)` / `BpmTracker::set_guide`: the
  estimator side of T-234's *Guider* (±3 % prior; an estimate within 6 %
  of the guide is taken at once). **Not wired** to taps or HTTP: that is
  T-234 (`#[allow(dead_code)]` on the Analyzer method until then).
- `onsets.rs`: `band_flux()` added; `odf_history()` is no longer the
  tempo's input (kept, marked for an ODF display); its test helpers
  (`noise`, `Hit`, `render_at`) are `pub(crate)` for the tempo tests.
- Not done (deliberately): UI (*BPM détecté*, *Confiance* gauge, state
  text, *Nouveau morceau* button) is T-243's `index.html`; applying the
  estimate to the clock is T-234.

## Testing
All on signals generated in the tests (clicks, kick/snare/hat grooves
from T-232's generators over a −50 dBFS noise bed; no device).
- `cargo test -p laser-studio`, after rebasing on develop d9cdf5b:
  **601 passed**, 7 ignored (+2 + 2 in the integration tests). New: 15 in `bpm.rs`, 1 in `worker.rs`, hub view
  assertions in `mod.rs`.
  - Clicks at 128: locked at **6.0 s** (< 8), error ≤ 0.09 BPM after lock.
  - Clicks 70, 87, 100, 124, 140, 150, 174, 180 and full grooves 87–180:
    all locked at 6.0 s, worst error after lock **0.02–0.17 BPM**.
  - 174 BPM four-beat groove → **174** (the task also accepted 87).
  - 70 BPM groove with off-beat eighth hats → **140** (documented choice:
    every eighth is a pulse, the prior picks the dance-floor reading);
    the same kick + snare without hats → 70. A guide at 71 → 70.
  - Swing (off-beat at 0.6 and 0.667 of the beat) at 120 → 120.
  - 124 → 128 change: 128 (± 0.5) for good **2.85 s** after the change.
  - 16-beat break (held chord, no drums) at 128: BPM moves < 0.05 before
    coasting, frozen after; *Coasting* **2.0 s** into the break; ≥ 14
    beats predicted in the break, all within 30 ms of the grid; locked
    again 4.6 s after the drums return.
  - Silence: `no_input` at once, BPM unchanged; after 3 s forgotten
    (confidence 0), a new track at 100 locks from scratch in < 8 s with
    the old BPM shown until its first estimate. A 1.5 s silence keeps the
    history (back as *Coasting*, then *Locked*).
  - White noise (two levels): confidence ≤ 0.3 (measured 0), never locked.
  - Predicted beats in steady state (90 / 128 / 174, clicks and groove):
    worst `next_beat` error **0.8–3.9 ms**, bias ≤ 1.8 ms (< 20 ms).
  - 44.1 and 96 kHz: lock < 8 s, < 0.5 BPM, phase < 20 ms.
  - **No allocation per hop**, estimates included (whole `Analyzer` under
    the counting allocator for 8 s of groove).
  - NaN / ∞ / negative ODF, degenerate hop rates, 16 kHz (1 hop/frame).
  - `bpm_cost_is_small` (`#[ignore]`, `--release -- --ignored bpm_cost`):
    **0.023 % of a core** on this M3 (onsets 0.10 %).
  - worker: the estimate is published, the BPM survives a reopen,
    *Nouveau morceau* reaches the analyser without reopening the input.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: `audio.spec.ts`: `audio.tempo` is null with `--no-audio`; new test
  `POST /api/audio/tempo/new_track` → 200 and the tempo clock (bpm,
  source) unchanged. Full suite (rebased): **150 passed**.

## Licences
No new crate (`realfft` was already there). Our own code from the papers'
descriptions: Scheirer 1998; Ellis 2007; Davies & Plumbley 2007; Stark,
Davies & Plumbley 2009; Percival & Tzanetakis 2014. No GPL source (BTrack,
aubio, essentia, madmom, qm-dsp) was read.

## Risks
- Everything is tuned on **synthetic** drums: the confidence maps, the
  presence ramp, the ODF band weights (1, 1, 1) and the ODF delay (0.42
  window) must be re-checked on real music (T-244). Tracks whose pulse is
  mostly in a pad or a bass line (no transients) will stay *Checking*.
- Octave choice: equal-level off-beat hats read as double tempo at 70
  (→ 140) by design; at 90–110 with loud eighths it could also double on
  real music. T-234's 4 s octave hysteresis on the clock side and the
  *Guider* prior are the answers.
- Lock is never faster than ~6 s (3 s of audio + 4 estimates for the
  stability + 2 s hold). Task limit is 8 s.
- *Coasting* has no timeout here (the clock's 30 s unlock is T-234);
  `Guided` replaces only `checking` (a guided estimate that locks reads
  `locked`, so T-234 can apply it).
- `beat_time` is reported ~2 ODF frames (≈ 21 ms) after the beat; use
  `next_beat` for prediction.
- `/api/state.audio` grows by ~100 bytes (`tempo`). No `Settings` or
  `audio.json` change.
- Safety: unchanged; the estimate reaches neither the engine features nor
  the clock yet.

## Review

Reviewed by the architect (integrator). Clean merge on d9cdf5b; 601 unit
+ integration tests, e2e 150/150, clippy clean. No new crate, written from
the papers. The tempo clock is never touched (T-234 will apply the
estimate). Accepted: band-flux input instead of the whole-spectrum ODF;
70 BPM with loud off-beat hats reads 140 by design (a tap guide fixes it
once T-234 wires it). Thresholds must be checked on real music (T-244).
Verdict: APPROVED
