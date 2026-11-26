# feat/audio-sections — T-236 Build-up / drop / break / silence detection

## What / why
These are exactly the moments an operator changes the look by hand (the
festival grammar: darkness in the breakdown, a build that climbs with the
snare roll, blackout on the beat before the drop, everything on beat 1).
The native analysis thread now says which section the music is in, with a
confidence, the build-up progress, a drop counter and the times, so T-240
can automate cues and the UI can show it. **Detection only**: nothing in
this branch changes the laser; `drop`, `buildup` and `section` reach the
engine's `AudioFeatures` (T-237's fields, T-237's `engine::Section`
enum reused), which nothing reads yet except the API view (T-153
routing and T-240 cues will, with their own rules).

New `studio/src/audio/sections.rs` (`SectionDetector`, owned by
`analysis::Analyzer`, fed every hop on the analysis thread, after the
spectrum, onsets and tempo; our own heuristics after
docs/research/audio-analysis.md § 2.6, no source read):

- **Trend frames** (~20 ms, 4 hops at 48 kHz): mean power of the low
  bands (sub + bass, 20–150 Hz), of the rest (150 Hz–12 kHz) and of the
  high band (2–12 kHz), mean log-centroid, onsets and kicks counted. 8 s
  kept in a ring allocated in `new`. Windows count in **beats**: the
  detected tempo (125 BPM until there is one), or the groove's measured
  kick spacing when that is longer (a 90 BPM groove with loud eighths
  reads 180 on T-233: the kick spacing keeps the windows right). *Short* =
  the last beat, *long* = the 4 before. Silence (and the 300 ms hold
  before `silent`) is left out of the ring.
- **References**: the groove's low and rest levels (one-beat means),
  averaged with τ = 8 beats (≈ the 16-beat mean of the task), updated
  only in *normal* / settled *drop*. Nothing but *normal* before 8 beats
  of groove.
- **Break**: low ≥ 10 dB under its reference **beyond what the whole mix
  lost** (a fader move is no break), the rest ≤ 6 dB under its own, and
  no kick for 1.75 kick periods. Its start is placed one kick period after
  the last kick. (The task says "≥ 2 beats" without a kick; that would make
  the detection always a full beat late, and 1.75 already means a kick
  expected and missed.)
- **Build-up**: rising evidence held ≥ 4 frames (~80 ms, so the hats of a
  returning groove don't read as a build-up just before its drop): the
  centroid up 0.2 octave with the high band up 2 dB (a centroid rise
  alone happens when the kick leaves), or the high band up 4 dB, or
  2 more onsets per beat (snare roll), over a low end still ≥ 6 dB down.
  `buildup` 0..1 = mean of how far the high band (/18 dB), the centroid
  (/2 octaves) and the onset rate (/6 per beat) have climbed since it
  started, **held at its maximum** (monotonic by construction; 0 outside a
  build-up). Back to *break* if the high band falls 6 dB from its peak
  while the mix holds (not when everything fades: a blackout is coming).
- **Drop**: on each kick during a break / build-up that has lasted ≥ 2
  beats, for the next 100 ms (the band filters lag the onset): the low band
  (30 ms smoothing) ≥ 8 dB over **its peak** of the kick period before
  (peaks against peaks: a kick fading back in, louder each beat, is no
  drop), and ≤ 10 dB under the groove reference. → `drop += 1`,
  `last_drop_t` = the kick's time, snapped onto T-233's beat grid when the
  tempo is *locked* / *coasting* and within ¼ beat; section *drop* for
  16 beats, then *normal*. Kicks returning without the jump → *normal*,
  no drop ("a false drop is worse than a missed one").
- **Silence**: the spectrum's `silent`. A silence under 4 beats inside a
  break / build-up (the blackout before the drop) goes back to that
  section, so the drop still counts; > 3 s forgets the references (a new
  track). *Nouveau morceau* forgets them too.
- `SectionState { section, confidence, buildup, drop, last_drop_t,
  start_t, since_s, history }` (the task's model plus `confidence`,
  `start_t` and the last 5 changes, newest first: `{t, section}`, for the
  UI's history). Confidence is a heuristic 0..1: break 0.5–1 with the
  low-end fall (10 → 20 dB), build-up 0.5–1 with the evidence and progress,
  drop 0.5–1 with the jump, normal = warm-up progress, silence 1.

Wiring:
- `Meter.sections`, `NativeSnapshot.sections`,
  `/api/state.audio.sections` (null when nothing fresh, like `tempo`).
- `web.rs`: T-234's simulated estimate snapshot gets a default
  `sections` (nothing else changed there).
- `analysis::native_features` takes the detector's state: `section`
  (silence still wins when `silent`), `buildup`, `drop`.
- `worker.rs` carries the drop counter and history across a stream reopen
  (`Analyzer::carry_sections`), like the onset counters.
- Not done (deliberately): the *Silence / Normal / Break / Montée / Drop*
  banner, gauge and history list (`index.html`, T-243); reacting to it
  (T-240).

## Testing
All on arrangements generated in the tests (T-232's kick / snare / hat
voices, a 55 Hz bass, a held A-major pad, a riser = noise high-passed at
1 kHz and low-passed with a cutoff rising 1.5 → 12 kHz, louder and louder,
over a snare roll in quarters → eighths → sixteenths); **every scenario
runs at 90, 128 and 174 BPM**. No device.
- `cargo test -p laser-studio` (rebased on develop 65ff47c: T-237,
  T-234 and T-162 merged): **642 passed**, 7 ignored (+2 +2 integration). New: 12
  in `sections.rs`, assertions in `analysis.rs` and `mod.rs`.
  - Acceptance: 16 beats groove → 8 break → 8 build-up → drop. Detection
    after the true start (90 / 128 / 174 BPM): break +519 / +379 / +313 ms
    (0.78 / 0.81 / 0.91 beat), build-up +71 / +85 / +72 ms, drop reported
    +17 / +20 / +18 ms and placed −0.5 / +2.6 / +2.7 ms from the true beat.
    Exactly one drop; the history reads normal → break → build-up → drop.
  - `buildup` never falls (tolerance 0.02) through the riser, climbs ≥ 0.4
    to > 0.6, is 0 before and after.
  - Break straight into the groove (no build-up): break → drop.
  - 2 beats of digital silence before the drop: build-up → silence →
    build-up → drop, one drop.
  - Silence between two grooves: *silence* within 350 ms, then *normal*,
    no drop.
  - False positives: 64 beats of steady groove, kicks over pink-ish noise,
    a −12 dB then −20 dB global volume change: *normal* throughout, no
    drop, `buildup` 0. A break whose groove fades back in over 8 beats
    (from −30 dB): back to *normal*, **no drop**.
  - **No allocation per hop** (whole `Analyzer`, counting allocator, a
    full arrangement including a drop).
  - NaN / ∞ / absurd tempo input; serialisation of the history; drop
    counter carried and kept by *Nouveau morceau*.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: `audio.spec.ts` checks `audio.sections` is null with `--no-audio`.
  Full suite on the final tree: **166 passed**, twice in a row. Two
  earlier full runs each had one unrelated UI test time out
  (`figures.spec.ts` draw/save, `tempo-follow.spec.ts` "an unsure
  estimate…" reading `coasting` instead of `waiting`); both pass alone
  (tempo-follow 3/3) and develop was green twice meanwhile. With
  `--no-audio` the detector never runs in e2e, so I read them as load
  flakes; worth watching `tempo-follow.spec.ts:81`.

## Licences
No new crate. Our own heuristics from the research note; no GPL source
(aubio, essentia, madmom…) read.

## Risks
- All thresholds are tuned on **synthetic** arrangements; real tracks
  (T-244's corpus) must re-check them: breaks that keep a bass line
  (low end not 10 dB down) won't read as breaks; build-ups with a kick
  roll and a full low end won't read as build-ups; a drop whose first kick
  is not detected is caught on the next one (one beat late).
- Half-time grooves (kick every 2 beats) learn a 2-beat kick period:
  breaks are then detected up to 2 beats late (safer, not faster).
- `buildup` is held at its maximum inside a build-up: a riser that dips
  does not lower it.
- *Drop* is reported for 16 beats whatever follows (unless a new break /
  build-up starts after its first 2 beats).
- `/api/state.audio` grows by ~250 bytes (`sections`). No `Settings` or
  `audio.json` change.
- Safety: unchanged. Nothing reacts to the section yet: no look reads
  `section` / `buildup` / `drop`; T-153 routing and T-240 automation will
  come with their own rules.

## Review
