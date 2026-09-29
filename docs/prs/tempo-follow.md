# feat/tempo-follow — T-234 Tempo auto: the tempo clock follows the detection

## What / why
T-233 made the native analysis propose a `TempoEstimate`; nothing used it.
This branch lets the **single** `TempoClock` follow it when the operator
turns on *Tempo auto*, without ever making effects jump, and with the tap
still in charge. No second clock: the estimate is read by the engine thread
and handed to `s.tempo.apply_detection` once per frame, under the lock it
already holds. Arming, the output gate and the safety chain are untouched.

`studio/src/tempo.rs`:
- `TempoSource::Audio` (serialised `audio`) next to `manual` / `tap`.
- `AudioTempoConfig { min_confidence 0.6, phase_gain 0.2,
  max_phase_step_beats 1/16, coast_timeout_s 30 }` (`#[serde(default)]`,
  held on the clock as `follow_config`; **not persisted yet**, defaults
  only).
- `FollowState` (`/api/state.tempo.follow`): `off` (source not audio),
  `waiting` (auto on, no lock yet), `locked`, `coasting`, `unlocked`.
  `TempoState` also gains `confidence`, `detected_bpm`, `guide_bpm`.
- `apply_detection(&TempoEstimate, now)`, per engine frame:
  - Followed only if source = Audio **and** `state == Locked` **and**
    confidence ≥ 0.6 (and a sane BPM; NaN/∞ ignored). Anything else =
    *Maintien*: nothing changes, the clock free-runs at its last BPM and
    phase; after 30 s of it the follower unlocks (history forgotten, the
    next lock starts afresh). A stale / missing native snapshot counts as
    *no input*.
  - **BPM**: the detected BPM is averaged (the tempo's whole life, then
    an EMA over 10 s; a value > 2 % away starts a new average). It is
    applied through `set_bpm` (phase-continuous) once it has held 2 s
    (4 s when the ratio is beyond ×1.3/÷1.3, i.e. octave jumps) and is
    > 0.3 BPM from the clock.
  - **Phase (PLL)**: on each new `beat_time` (only when clock and detection
    agree within 4 % and the beat is less than 2 beats old):
    `e` = nearest whole beat − `beat_at(beat_time)`, wrapped to ±½.
    |e| ≤ ¼: correct `k·e` (capped to 1/16 beat), **spread over the next
    beat** (a slew, never a step). |e| > ¼: ignored unless 4 consecutive
    beats agree (within 1/8): then a resync of the whole error, logged, at
    the same capped rate (1/16 beat per beat). Integral term: `0.02·e·bpm`
    BPM per beat, capped at 0.05 BPM per bar, only when |e| < 1/16 (so
    pulling in a big offset is not mistaken for drift).
- **Tap always wins**: the *first* tap while in Audio switches the source
  to `tap` (detection ignored from then on); a manual BPM, ×2, ÷2 do the
  same (`manual`). Resync and nudge keep Audio.
- `set_auto(bool)`: on → Audio (`waiting`); off → Manual at the current
  BPM and phase.
- `guide_tap(now)` (*Guider*): its own tap buffer; from the third tap its
  tempo becomes the guide, the clock itself is never touched.
  `new_track()` forgets the guide and the follower history.

Wiring:
- `AudioHub::set_guide(Option<f64>)` / `guide()`: a leaf-locked value +
  generation; the analysis thread applies it (`Analyzer::set_guide`, now
  used — the `#[allow(dead_code)]` is gone) at its next poll and on every
  new analyser (stream reopen). `AudioHub::fresh_tempo(now)`: the latest
  estimate if fresh (< 500 ms) and the native input is the selected source.
- Controls (registry, `docs/controls.md` regenerated): `tempo.auto`
  (toggle), `tempo.guide` (trigger), `tempo.new_track` (trigger).
  `Shared::tempo_new_track()` = hub new track + clear the guide on both
  sides; `POST /api/audio/tempo/new_track` now goes through it too.
- `POST /api/test/tempo_estimate` (**`--test-hooks` only**, 404 otherwise):
  publishes a simulated estimate as a native snapshot, beats on the grid
  `offset_s + n·60/bpm` in studio time. For the e2e tests.
- `DetectState` derives `Deserialize` (for that hook).
- UI (`index.html`, minimal, in the existing tempo bar): an **Auto** button
  (lit when the source is Audio; click toggles `tempo.auto`), a **Guider**
  button and a short state text (« verrouillé 82 % · guide 128 ») shown
  only while Auto is on. Tap / BPM field unchanged.

## Testing
- `cargo test -p laser-studio`: **616 passed**, 7 ignored (+ 2 + 2
  integration). New:
  - tempo.rs (12, a 60 fps simulated engine against simulated detector
    estimates, no device): lock brings 120 → 128 and the phase onto the
    grid (< 0.02 beat) in 30 s; a ¼-beat error (gain 0.2 and 1.0) never
    shifts more than **1/16 beat in any one-beat window**; a 0.4-beat error
    is ignored for 3 beats then resynced once, at the capped rate; ±1 BPM
    noise around 128 keeps the shown BPM within **127.7–128.3** for 60 s;
    a 128 → 129 drift over a minute is followed; an octave jump (128 → 64)
    waits 4 s; Checking / Guided / confidence 0.5 / NaN never move the
    clock; Locked → Coasting → Unlocked after 30 s with the BPM held and
    `beat_at` continuous, then a fresh lock at 132; a tap takes over (first
    tap), 20 s of other estimates are ignored, Auto brings it back, a
    manual BPM leaves Auto; guide taps set the guide (127.7) without
    touching the BPM, beat or source, *Nouveau morceau* clears it;
    detection ignored unless the source is Audio; a full scenario (lock,
    break, new track off-phase, slower tempo) has **no per-frame jump
    > 1/16 beat** and ≤ 1/16 beat per beat.
  - controls.rs: `tempo.auto` / tap / `tempo.guide` / `tempo.new_track`
    through the registry; arming untouched.
  - audio/mod.rs: `fresh_tempo` (fresh, stale, other source), guide
    generations. worker.rs: the guide reaches the analyser (state
    `guided`), survives a reopen, and `None` clears it.
  - web.rs: the estimate hook is 404 without `--test-hooks`, publishes a
    grid-aligned estimate with it.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `tempo-follow.spec.ts` (4 tests, `--test-hooks`, `--no-audio`):
  Auto → waiting → locked, BPM 120 → 128 in the field, then *maintien*
  when estimates stop, never armed; a tap takes over and 140 BPM estimates
  are ignored until Auto, then followed, Auto off → manual at 140; unsure
  estimates (checking, 0.4) never move the clock; *Nouveau morceau* keeps
  Auto. Full suite: **154 passed**.

## Risks
- Gains and thresholds (2 % same-tempo band, 10 s average, 4 % phase
  gate, integral gain) are tuned on simulated estimates only; T-244's real
  music corpus must confirm them.
- The resync after 4 beats of a big error is done at 1/16 beat per beat,
  not as a step (the task said « recalage franc »): a half-beat offset takes
  ~8 beats to pull in. Chosen so that `beat_at` never jumps more than 1/16
  beat, the other acceptance criterion. Easy to make faster if wanted.
- No latency compensation yet: detection times are compared as they are
  (already on the studio clock); T-246 adds the analysis delay and output
  offset.
- `AudioTempoConfig` is in memory only (defaults); no API/UI to change it
  and not saved (no `Settings`/project change).
- The estimator forgets its guide after > 3 s of silence (T-233) while the
  clock still shows it until *Nouveau morceau* or a new guide.
- Tempo auto only follows the **native** input (the browser path has no
  tempo estimate). With the audio source on *Navigateur* the follower
  stays in *en attente*.
- MIDI clock / Link priority (T-207 / T-154) doesn't exist yet; when it
  does it should sit between Tap/Manual and Audio as the task says.
- Safety: no output path, no arming; the clock only feeds beats as before.

## Review
