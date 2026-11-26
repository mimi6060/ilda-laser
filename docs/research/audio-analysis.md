# Pro-grade audio analysis for music-reactive lasers

Research report for Laser Studio. Scope: what to compute from live audio,
where to compute it, which libraries/algorithms we may use (licences), how
the tempo engine (T-150) consumes detections, and how features map to laser
parameters. Public information only. Tasks derived from this report:
**T-230 – T-246**.

## 0. Summary

1. **Move analysis into Rust.** Capture with `cpal` (CoreAudio) on a
   dedicated thread, FFT with `realfft`, feed the engine directly. The
   browser becomes a display and a fallback source. Reason #1: the laser
   must keep reacting when the tab is hidden or closed; today the browser
   loop uses `setTimeout(analyse, 25)`, which Chrome/Safari throttle to
   ≥ 1 s in background tabs, and the server already drops audio older than
   500 ms (`AUDIO_STALE`), so a hidden tab = no music reaction.
2. **Compute a small, well-defined feature set** every hop (~5.3 ms):
   5 band energies with auto-gain, RMS/peak in dBFS, spectral flux onset
   function (total and per band), kick/snare/hat onsets, spectral centroid,
   silence gate; at a slower rate (every ~0.5 s): tempo estimate +
   confidence, beat phase, downbeat guess, energy-trend section state
   (build-up / drop / break).
3. **Tempo detection never owns the clock.** It proposes `(bpm, confidence,
   beat_time)` to `TempoClock` (T-150), which accepts it only when source =
   *Audio* and confidence is high, "coasts" otherwise, lets taps override,
   and corrects phase gently (PLL-style, bounded per beat).
4. **Licences:** use only permissive crates in the default build (`cpal`,
   `realfft`/`rustfft`, `rtrb`, `screencapturekit` — MIT/Apache). **No
   aubio / aubio-rs / essentia / BTrack / Queen Mary plugins** (GPL/AGPL)
   because the build will later link the proprietary ShowNET SDK (T-015).
   Reimplement from papers (Bello 2005, Dixon 2006, Böck 2013, Scheirer
   1998, Ellis 2007, Davies & Plumbley 2007, Stark 2009). Algorithms are
   not copyrightable; code is — read papers, not GPL source.
5. **Latency budget:** onset-driven effects must hit the laser within
   ~40 ms of the sound; beat-grid effects driven by the tempo clock can be
   *predictive* (0 ms or even negative with a user offset).

## 1. What exists today

- `studio/src/index.html` (§ music): `getUserMedia` with AGC/echo/noise
  processing disabled (good), `AnalyserNode` fftSize 2048, smoothing 0.3.
  `level` = RMS × gain; `bass` = mean byte magnitude of bins < 150 Hz;
  beat = bass > 1.35 × its EMA (α = 0.05) and > 0.12, 200 ms refractory.
  POSTs `{level, bass, beat}` to `/api/audio` at most once per in-flight
  request, loop every 25 ms.
- `studio/src/engine.rs`: `AudioFeatures { level, bass, beat: u64 }`;
  `AudioReact { enabled, size, rotate, color_on_beat, flash }`.
- `main.rs`: features older than 500 ms are replaced by silence.

Weaknesses: byte magnitudes (dB-scaled, clipped 0..255, depends on
`minDecibels`), one band, no normalisation across songs/venues, the beat
detector fires on any bass swell (basslines, not just kicks), no tempo,
browser-dependent timing, stops with the tab.

## 2. Real-time features to compute

Audio at 48 kHz mono (sum L+R), frame N = 1024 (21.3 ms), hop H = 256
(5.33 ms, ~188 hops/s), Hann window, real FFT → 513 bins of 46.9 Hz.
For the sub band the resolution is poor; use either a second, longer FFT
(N = 4096, hop 512) for < 150 Hz, or — simpler and lower latency —
**IIR band filters** (biquad Linkwitz-Riley / Butterworth) on the time
signal for the band energies, and the FFT only for flux/centroid. Both are
cheap: < 1 % of one core on Apple Silicon.

### 2.1 Band energies

| Band | Range | Typical content | Laser use |
|---|---|---|---|
| `sub` | 20–60 Hz | kick fundamental, 808 | size "punch", brightness pulse |
| `bass` | 60–150 Hz | kick body, bassline | size, zoom, beam height |
| `low_mid` | 150–500 Hz | snare body, toms, vocals | rotation speed, position |
| `mid` | 500–2 000 Hz | snare crack, synths, vocals | colour/hue shift, wave amplitude |
| `high` | 2–12 kHz | hi-hats, cymbals, air | sparkle: dots/dash, jitter, scan rate |

Per band: RMS over the hop → dB → **adaptive normalisation** (see § 5.2)
to 0..1. Also `level` (full-band RMS), `peak`, and `level_db` for the
input meter (target −25..0 dBFS, as PangoBeats advises).

### 2.2 Onset detection function (ODF)

Spectral flux with log compression and half-wave rectification
(Bello et al. 2005; Dixon 2006):
`SF(n) = Σ_k H( log(1+γ|X_n(k)|) − log(1+γ|X_{n−1}(k)|) )`, γ ≈ 1…100.
The **SuperFlux** variant (Böck & Widmer 2013) compares against a
maximum-filtered previous frame (3 bins, lag 2 frames) and suppresses
vibrato false positives — worth it, it is 20 lines.

Peak picking (causal, Dixon 2006 / Böck 2012): an onset at hop n if
`SF(n) = max(SF[n−w1..n])`, `SF(n) ≥ mean(SF[n−w3..n]) + δ`, and
`n − last_onset > w5` (≥ 30 ms). Causal picking costs **no look-ahead**
beyond the frame length; allowing 1–2 hops of look-ahead (5–10 ms)
improves precision and is still within budget.

Compute SF over the whole spectrum (for tempo) **and per band** (for
instrument onsets).

### 2.3 Kick / snare / hi-hat

Band-wise onset detection plus simple rules, not ML:

- **Kick**: onset in the sub+bass flux (40–150 Hz) whose energy rise is
  fast (< 20 ms) and whose high band does not dominate. Refractory 100 ms.
- **Snare/clap**: onset in 150 Hz–5 kHz flux with broadband spread
  (spectral flatness above a threshold) and no simultaneous strong kick,
  or both (kick+snare).
- **Hi-hat**: onset in 5–15 kHz flux with low energy below 2 kHz.

Expose `kick`, `snare`, `hat` as **counters** (like today's `beat: u64`)
plus a 0..1 `strength`, so a missed frame never loses an event. Accuracy
target on electronic music: kick F ≥ 0.9, snare/hat ≥ 0.7. Optional later:
a tiny learned classifier trained on our own recordings (no pretrained
models with non-commercial or unclear licences).

### 2.4 Tempo (BPM) estimation and beat tracking

Standard pipeline (Scheirer 1998; Ellis 2007; Davies & Plumbley 2007;
Stark et al. 2009; Percival & Tzanetakis 2014):

1. ODF resampled to ~100 Hz (every 2 hops), buffered over **6–8 s**.
2. **Autocorrelation** (or generalised ACF via FFT) of the mean-removed
   ODF over lags 60/250 … 60/40 s.
3. **Comb-filter / harmonic enhancement**: score(τ) = ACF(τ) + ½ ACF(2τ)
   + ⅓ ACF(3τ) + ¼ ACF(4τ); weight with a log-Gaussian **tempo prior**
   centred on 125 BPM (σ ≈ 1 octave) → favours 90–180, solves most
   half/double ambiguity (dance music: 120–150 dominates).
4. Pick the best lag with **parabolic interpolation** (sub-bin BPM,
   needed for < 0.5 BPM error) and track it over time with a Viterbi-like
   or simple hysteresis (a new tempo must win for ≥ 2 s).
5. **Confidence**: ratio of the winning peak to the mean score
   (peak-to-average) combined with stability over the last N estimates and
   ODF "pulse clarity". Map to 0..1; empirically calibrate thresholds on
   the test corpus (T-244).
6. **Beat phase**: cross-correlate the last 2–4 beat periods of ODF with a
   pulse train at the chosen period (comb over phases), or run the causal
   **dynamic-programming** tracker (Ellis 2007, made online by Stark 2009:
   cumulative score `C(n) = ODF(n) + α·max_{τ∈[P/2,2P]} W(n−τ)·C(n−τ)`
   with a log-Gaussian transition window around the period P, then predict
   the next beat one period ahead). Output `beat_time` = time of the most
   recent beat (and predicted next).

EDM is easy (four-on-the-floor, steady tempo); breakdowns without drums
are the hard part → "coasting" (§ 4).

### 2.5 Downbeat / bar detection

Harder and less reliable (Klapuri et al. 2006; Durand et al. 2017 uses
deep nets). Pragmatic heuristics that work on dance music:

- Accumulate, per beat position modulo 4, the average **low-band onset
  strength and harmonic-change (chroma flux)**; the bar "1" tends to have
  the strongest bass and the most chord changes. Needs ~8 bars.
- Phrase boundaries: big energy changes (§ 2.6) almost always land on a
  bar 1 of a 8/16-bar phrase → snap downbeat there with high confidence.
- Always allow the user **Resync** (T-150) to override; show the guess
  with its confidence, never silently re-phase the bar in the middle of a
  show (only at low confidence → high confidence transitions).

### 2.6 Build-up / drop / break / silence

Computed on smoothed trends (1–8 s windows):

- **Silence**: level < −60 dBFS (configurable) for > 300 ms → `silent`.
  After 2 s, state *Pas d'entrée*.
- **Break/breakdown**: low-band energy falls > 10 dB below its 16-beat
  average and kick onsets stop for ≥ 2 beats while overall level stays up.
- **Build-up (riser)**: over the last 4–16 beats, **spectral centroid
  slope > 0**, high-band energy rising, onset rate rising (snare rolls:
  onset density doubling), low band still reduced. Output `buildup` 0..1.
- **Drop**: after a break/build-up, low band returns > +8 dB within one
  beat, kick onsets resume → `drop` event (counter) + `section` =
  *Drop*. Snap to the nearest downbeat if tempo is locked.

These are exactly the moments an operator rides by hand (see
`festival-looks.md`), so expose them both as continuous values (for
routing) and as events (for cue triggers, T-240).

### 2.7 Latency budgets

| Stage | Typical | Notes |
|---|---|---|
| CoreAudio input buffer | 2.7–10.7 ms (128–512 frames @ 48 kHz) | request 256 via `cpal::BufferSize::Fixed` |
| Analysis frame (half window as group delay) | ~10 ms (N = 1024) | IIR band followers: ~1–3 ms |
| Peak picking look-ahead | 0–10 ms | configurable |
| Engine tick (60 fps) | 0–16.7 ms (avg 8) | read latest features at frame start |
| DAC buffer (IDN / Ether Dream) | 10–40 ms | depends on point rate & buffer fill |
| Galvo/laser response | < 1–2 ms | negligible |
| **Total, onset-driven** | **~35–80 ms** | target ≤ 50 ms |

Perception: ITU-R BT.1359 puts detectability of AV offset at about +45 ms
(sound before picture) / −125 ms (sound after picture); vision lagging
sound is more tolerated, but a laser flash is a sharp transient so aim for
≤ 50 ms. **Beat-grid effects** (chasers, strobes on the beat, cue
launches) should use the tempo clock's *predicted* beat, so their latency
is zero; add a global **output offset** (ms, −100..+100) that shifts the
beat clock to compensate the measured DAC latency (T-246). Browser path
today adds requestAnimationFrame/setTimeout jitter (25 ms loop) plus an
HTTP round-trip: another reason to go native.

## 3. Where to run it: browser vs native

| Criterion | Browser (Web Audio) | Native (Rust, `cpal`) |
|---|---|---|
| Keeps working with tab hidden/closed | **No** (timer throttling ≥ 1 s in background tabs, stops when closed) — AudioWorklet keeps running while the tab is open, but the results still need to reach the server | **Yes** |
| Latency / jitter | 25 ms loop + HTTP POST, jittery | hop-accurate (5 ms), in-process |
| Deterministic tests | JS tests in Node with synthetic signals | `cargo test` with synthetic buffers, same as the rest of the engine |
| Single clock with `TempoClock` | needs timestamps mapped across processes | same process, same `Instant` |
| Device choice | `enumerateDevices` (labels need permission) | `cpal` host/device enumeration, persisted in config |
| macOS permission | browser asks once per site | terminal/app asks (TCC), see below |
| Mac's own output | not possible (no loopback in Web Audio) | via BlackHole-type virtual device, or ScreenCaptureKit / CoreAudio process tap |
| Dependencies | none | `cpal` (Apache-2.0), `realfft` (MIT/Apache) |

**Recommendation: native capture and analysis in Rust**, in a new
`studio/src/audio/` module on its own thread, writing an `AudioFeatures`
snapshot (lock-free: `rtrb` ring or an `ArcSwap`-like atomic swap, or a
`Mutex` held for microseconds) that the 60 fps engine reads. Keep
`POST /api/audio` as a secondary source *Navigateur* for compatibility and
for running the UI on another machine; the UI shows meters from
`/api/state` instead of computing them. The audio callback must not
allocate or lock: copy samples into an SPSC ring (`rtrb`), analyse on a
separate thread.

### 3.1 macOS microphone permission for a CLI

- The Transparency, Consent & Control (TCC) prompt is attributed to the
  **responsible process**: run from Terminal/iTerm, the *terminal app*
  gets the "would like to access the microphone" prompt and the grant.
  Launched another way (launchd, a double-clicked unsigned binary), the
  request may fail silently and deliver **zeros** — detect this (all-zero
  input for > 2 s while "listening" → UI warning "Autorisation micro
  refusée ? Réglages Système › Confidentialité › Micro").
- A bundled `.app` must have `NSMicrophoneUsageDescription` in
  `Info.plist` (and the `com.apple.security.device.audio-input`
  entitlement if hardened runtime is enabled). Relevant later when we
  package the studio.
- Reset during testing: `tccutil reset Microphone`.

### 3.2 Capturing the Mac's own output

Three routes, all optional, in order of simplicity:

1. **Virtual loopback device** (BlackHole — GPL-3 driver, installed
   separately by the user, *not linked* by us, so no licence impact; or
   Rogue Amoeba Loopback, commercial). User creates a Multi-Output Device
   (speakers + BlackHole) in Audio MIDI Setup, we capture BlackHole as a
   normal `cpal` input. Zero code: document it in the UI help.
2. **ScreenCaptureKit audio** (macOS 13+): `SCStream` with
   `capturesAudio = true` gives system (or per-app) audio without a
   driver. Needs the **Screen Recording** permission (macOS 15 re-asks
   periodically), which is awkward for a lighting tool. Rust bindings:
   `screencapturekit` crate (MIT/Apache-2.0). A `cpal` PR (#894) explores
   exposing this as a loopback host.
3. **CoreAudio process taps** (macOS 14.2+, `AudioHardwareCreateProcessTap`
   + aggregate device): the modern Apple API for capturing output of all or
   some processes, needs `NSAudioCaptureUsageDescription`. Best quality and
   lowest latency but requires Objective-C/FFI work; no mature Rust crate.

Recommendation: ship (1) as documentation now; implement (3) or (2) as an
optional source later (T-241). For shows, the normal input is a line feed
from the DJ mixer booth out into a USB audio interface — design for that
first.

## 4. Libraries and licences

Constraint: non-commercial project, but the default build will link the
proprietary ShowNET SDK (T-015) → **no GPL/AGPL code in the default
build**. GPL tools may be used *outside* the binary (e.g. to produce
reference annotations for our test corpus, run as separate programs), never
linked or copied.

| Library | Licence | Verdict |
|---|---|---|
| [aubio](https://aubio.org/) (C) / [aubio-rs](https://github.com/katyo/aubio-rs) | GPL-3.0 | **No** (linked). Fine as an offline reference tool only |
| [Essentia](https://essentia.upf.edu/) | AGPL-3.0 (commercial licence available) | **No** |
| [BTrack](https://github.com/adamstark/BTrack) | GPL-3.0 | **No** — reimplement from Stark et al. 2009 paper |
| Queen Mary Vamp plugins / qm-dsp | GPL-2+ | **No** |
| [madmom](https://github.com/CPJKU/madmom) | code BSD-2, **models CC BY-NC-SA** | Python, not embeddable; models non-commercial/share-alike → no |
| [librosa](https://librosa.org/) | ISC | Offline reference/evaluation only (Python) |
| [mir_eval](https://github.com/craffel/mir_eval) | MIT | Evaluation metrics (offline, T-244) |
| [cpal](https://github.com/RustAudio/cpal) | Apache-2.0 | **Yes** — capture |
| [rustfft](https://github.com/ejmahler/RustFFT) / [realfft](https://github.com/HEnquist/realfft) | MIT OR Apache-2.0 / MIT | **Yes** — FFT |
| [rtrb](https://github.com/mgeier/rtrb) | MIT OR Apache-2.0 | **Yes** — lock-free SPSC ring |
| [biquad](https://crates.io/crates/biquad) | MIT OR Apache-2.0 | Yes (or write the 10-line biquad ourselves) |
| [beat-detector](https://github.com/phip1611/beat-detector) | MIT | Allowed, but naive (low-pass + envelope); reimplement instead |
| [spectrum-analyzer](https://crates.io/crates/spectrum-analyzer) | MIT | Allowed; not needed with `realfft` |
| [pitch-detection](https://crates.io/crates/pitch-detection) | MIT/Apache (check at add time) | Not needed (pitch is irrelevant for lasers) |
| [screencapturekit](https://crates.io/crates/screencapturekit) | MIT OR Apache-2.0 | Yes, optional feature for system audio |
| BlackHole | GPL-3.0 | Separate driver installed by the user; not linked → OK |

Rule for developers: every new crate is checked with `cargo tree` +
licence field (add `cargo deny` later); anything not MIT/Apache/BSD/ISC/
Zlib/MPL-2.0 needs the architect's approval. Reimplement from papers:
spectral flux / SuperFlux, adaptive peak picking, ACF + comb tempo,
causal DP beat tracker, band splitting, envelope followers — all small.

## 5. How the tempo engine consumes detection (T-150)

Detection produces, ~2×/s, `TempoEstimate { bpm, confidence, beat_time,
next_beat, downbeat_time?, downbeat_conf }` plus per-hop onsets. The
`TempoClock` stays the single source of beat phase.

States (mirrors the PangoBeats behaviour described in
`pro-live-operation.md` § 4):

| State | Condition | Clock behaviour |
|---|---|---|
| *Pas d'entrée* | silent | clock free-runs at last BPM (coast) |
| *Vérification* | confidence < 0.4 or < 4 s of audio | no change |
| *Verrouillé* | confidence ≥ 0.6 for ≥ 2 s, BPM stable ±1 | accept BPM + phase correction |
| *Maintien* (coast) | was locked, confidence dropped (breakdown) | keep BPM and phase, keep predicting beats; unlock after 30 s |
| *Guidé* | user tapped | taps become a strong prior (±3 %) for the estimator |

Rules:

- **Source priority**: Tap/Manual > MIDI clock/Link > Audio. Any tap switches
  the source to *Tap* (T-150); a *Auto* button returns it to *Audio*.
  A "guide" tap (T-152) adds evidence without taking over.
- **BPM changes**: only when the new estimate is stable for 2 s and differs
  by > 0.3 BPM; apply via `set_bpm` (phase-continuous). Octave jumps
  (×2/÷2) need a longer confirmation (4 s) — they are usually errors.
- **Phase correction (PLL)**: error `e` = detected beat time − clock's
  nearest beat, wrapped to ±½ beat. Only when locked; ignore |e| > ¼ beat
  unless it persists 4 beats (then treat as a resync). Correct by
  `k·e` per beat with k ≈ 0.1–0.25 and a hard cap of **1/16 beat per
  beat** (as T-152 says), so effects never visibly jump. A small
  integral term corrects BPM drift (±0.05 BPM per bar).
- **Downbeat**: only re-phase the bar when downbeat confidence is high and
  the change happens at a phrase boundary (drop); otherwise display the
  suggestion ("Temps 1 suggéré") and let the operator press Resync.
- **New track**: resets history; also triggered automatically after a
  silence > 3 s.
- **Latency compensation**: detection timestamps are in the audio clock
  (sample count → `Instant` via the callback timestamp); subtract analysis
  delay; add the user output offset (T-246).

This supersedes the *browser* implementation proposed in T-152 (same
behaviour, native). The architect should re-scope T-152 to "UI + API of
the auto-BPM" or close it in favour of T-233/T-234.

## 6. Mapping features to laser parameters

### 6.1 Suggested defaults (our own presets, T-239)

| Feature | Good targets | Why |
|---|---|---|
| `kick` event | brightness pulse (flash), size punch, beam fan "hit", cue step | the most reliable, most visible transient |
| `sub`/`bass` continuous | size / zoom, beam height, tunnel depth | slow, big movements match low frequencies |
| `snare` event | colour change, mirror/flip, chaser step | backbeat accents (beats 2 and 4) |
| `low_mid`/`mid` continuous | rotation speed, wave amplitude, hue drift | melodic energy, medium motion |
| `high` / `hat` | dots/dash amount, sparkle, small jitter, scan rate | fine texture ↔ fine detail |
| `buildup` 0..1 | speed ramp, strobe rate division (1/4 → 1/16), size shrink/tighten | follows the riser |
| `drop` event | switch to "drop" cue, full brightness, widen | the moment the crowd expects |
| `silent` | fade to a calm look or blackout (configurable) | avoid frozen strobes in silence |

Principle from lighting practice: **low frequencies → large, slow
parameters; high frequencies → small, fast parameters; events → discrete
changes**. Never map continuous `high` to brightness directly (flicker).

### 6.2 Signal conditioning (per route)

Each route: `source → gate → gain/auto-gain → curve → attack/release →
range → target`.

- **Auto-gain** per band: track a slow max (peak with 5–10 s release)
  and floor (5th percentile over 10 s); normalise to 0..1. Makes a
  whisper-quiet club and a loud festival feed look the same. Manual gain
  override stays available.
- **Gate** (threshold) with hysteresis to kill noise.
- **Curve**: linear, square (punchier), sqrt (more sensitive), S-curve.
- **Envelope follower**: one-pole with separate attack/release,
  `y += (x − y)·(1 − exp(−dt/τ))`, τ = attack when rising, release when
  falling. Defaults: size 10/150 ms, rotation 50/400 ms, brightness pulse
  0/120 ms. Evaluate at the engine rate with the real `dt`.
- **Event → envelope**: an event (kick) triggers an AD (or ADSR) envelope
  whose length can be **in beats** (e.g. decay ¼ beat) via the tempo clock.
- **Range**: min..max in the target's units (from the control registry,
  T-145), bipolar allowed (amount −1..1).
- **Mixing** with LFOs (T-151) and the manual value: `value = base +
  Σ modulations`, clamped to the control range; a master *Temps ↔ Audio*
  crossfader (T-153).

### 6.3 Per-control audio source routing

T-153 already defines `AudioRoute { source, target, amount, attack_ms,
release_ms, gate }`. Extensions from this research: `AudioSource` gains
`Sub, Bass, LowMid, Mid, High, Level, Kick, Snare, Hat, Onset, Buildup,
Drop, Beat, Bar`; add `curve`, `min/max`; event sources produce an
envelope with `decay_beats`. Route evaluation lives in the engine (Rust)
so it runs without the browser.

### 6.4 Safety

Audio can drive brightness and strobes at up to 10 Hz+: all audio-driven
flashes must pass through the strobe limiter (T-101) and the safety stage
(zones, brightness scaling) applied last. Stale or absent audio must decay
to neutral values (not freeze at the last loud value). Audio never arms
the laser.

## 7. Test strategy (T-244)

- **Synthetic signals** generated in Rust tests: click trains at known
  BPM, kick/snare/hat patterns (sine sweep kick, noise-burst snare,
  high-passed noise hat), tempo changes, breakdowns (drums removed),
  risers (filtered noise with rising cutoff), silence.
- **Metrics** (mir_eval-style, reimplemented or offline): onset F-measure
  with ±50 ms window, BPM accuracy (Acc1 ±4 %, Acc2 with octave errors),
  beat F-measure ±70 ms, lock time, latency (sample of the transient →
  feature timestamp).
- **Real music**: a private corpus of the user's own recordings or tracks
  under CC licences allowing it; never committed unless licence allows
  (`docs/CONTENT_SOURCES.md`). Annotations can be produced offline with
  any tool, including GPL ones run as separate programs.

## Sources

- J. P. Bello et al., "A Tutorial on Onset Detection in Music Signals",
  IEEE TSAP 2005 — <https://ieeexplore.ieee.org/document/1495485>
- S. Dixon, "Onset Detection Revisited", DAFx 2006 —
  <https://www.dafx.de/paper-archive/2006/papers/p_133.pdf>
- S. Böck, G. Widmer, "Maximum Filter Vibrato Suppression for Onset
  Detection" (SuperFlux), DAFx 2013 —
  <https://www.dafx.de/paper-archive/2013/papers/09.dafx2013_submission_12.pdf>
- S. Böck, F. Krebs, M. Schedl, "Evaluating the Online Capabilities of
  Onset Detection Methods", ISMIR 2012 —
  <https://archives.ismir.net/ismir2012/paper/000049.pdf>
- E. Scheirer, "Tempo and beat analysis of acoustic musical signals", JASA
  1998 — <https://doi.org/10.1121/1.421129>
- D. P. W. Ellis, "Beat Tracking by Dynamic Programming", JNMR 2007 —
  <https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf>
- M. Davies, M. Plumbley, "Context-dependent beat tracking of musical
  audio", IEEE TASLP 2007 — <https://doi.org/10.1109/TASL.2006.885257>
- A. Stark, M. Davies, M. Plumbley, "Real-time beat-synchronous analysis
  of musical audio", DAFx 2009 —
  <https://www.dafx.de/paper-archive/2009/papers/paper_37.pdf>
- G. Percival, G. Tzanetakis, "Streamlined Tempo Estimation Based on
  Autocorrelation and Cross-correlation With Pulses", IEEE TASLP 2014 —
  <https://doi.org/10.1109/TASLP.2014.2348916>
- A. Klapuri, A. Eronen, J. Astola, "Analysis of the meter of acoustic
  musical signals", IEEE TASLP 2006 — <https://doi.org/10.1109/TSA.2005.854090>
- S. Durand et al., "Robust Downbeat Tracking Using an Ensemble of
  Convolutional Networks", IEEE TASLP 2017 (background only)
- ITU-R BT.1359, relative timing of sound and vision —
  <https://www.itu.int/rec/R-REC-BT.1359>
- BTrack (GPL-3.0) — <https://github.com/adamstark/BTrack>,
  <https://adamstark.co.uk/project/btrack-a-real-time-beat-tracker/>
- aubio (GPL-3.0) — <https://aubio.org/>; aubio-rs —
  <https://github.com/katyo/aubio-rs>
- Essentia (AGPL-3.0) — <https://essentia.upf.edu/licensing_information.html>
- madmom — <https://github.com/CPJKU/madmom> (LICENSE: code BSD, models
  CC BY-NC-SA)
- beat-detector (MIT) — <https://github.com/phip1611/beat-detector>,
  <https://crates.io/crates/beat-detector>
- cpal — <https://github.com/RustAudio/cpal>; ScreenCaptureKit loopback
  PR — <https://github.com/RustAudio/cpal/pull/894>
- realfft — <https://github.com/HEnquist/realfft>; rustfft —
  <https://github.com/ejmahler/RustFFT>; rtrb — <https://github.com/mgeier/rtrb>
- screencapturekit crate (MIT/Apache-2.0) —
  <https://crates.io/crates/screencapturekit>,
  <https://github.com/svtlabs/screencapturekit-rs>
- Apple: ScreenCaptureKit —
  <https://developer.apple.com/documentation/screencapturekit>;
  capturing system audio with Core Audio taps —
  <https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps>;
  NSMicrophoneUsageDescription —
  <https://developer.apple.com/documentation/bundleresources/information-property-list/nsmicrophoneusagedescription>
- Chrome background-tab timer throttling —
  <https://developer.chrome.com/blog/timer-throttling-in-chrome-88>
- BlackHole (GPL-3.0) — <https://github.com/ExistentialAudio/BlackHole>
- Pangolin PangoBeats / realtime audio / channels (behaviour reference,
  see `pro-live-operation.md` § 4) —
  <https://wiki.pangolin.com/doku.php?id=beyond:pangobeats>,
  <https://wiki.pangolin.com/doku.php?id=beyond%3Arealtime_audio>
