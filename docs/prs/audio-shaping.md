# feat/audio-shaping — T-238 audio signal conditioning

## What / why
A raw band wired to a parameter makes the laser tremble. This adds the
conditioning chain every audio route (T-153) and preset (T-239) will use,
in `studio/src/audio/shape.rs`, pure and testable, evaluated per frame
from the `AudioFeatures` snapshot with the real `dt` and no allocation.

- `Shaper` (`#[serde(default)]`): `gate` 0.05, `hysteresis` 0.02, `gain`
  1, `curve` (linear / square / sqrt / s_curve), `attack` Ms(10),
  `release` Ms(150), `decay` Ms(120), `min` 0, `max` 1. Running state is
  `#[serde(skip)]` and ignored by `PartialEq`.
  - continuous: gate with hysteresis (opens at `gate`, closes under
    `gate − hysteresis`) → gain (clamped to 1) → curve → one-pole follower
    `y += (x − y)(1 − e^(−dt/τ))`, τ = attack rising / release falling;
  - events: `trigger(strength)` starts an AD envelope (linear rise over
    the attack, exponential fall under 5 % = e^−3 `decay` after the peak;
    a hit during a decay rises again, never drops; hits under the gate are
    ignored). A trigger is shown on its own frame (no lost peak);
  - output = `max(follower, curve(envelope))` mapped to `min..max`, in
    −1..1 fractions of the target's range (`min > max` inverts, negative
    pulls the control down).
- **Deviation from the task's data model** (the brief asked for it):
  attack and release are `Span::Ms | Span::Beats` like the decay (`Decay`
  is kept as an alias of `Span`), so any of the three can follow the tempo.
  Beats are read from the beat length passed to every `process` (60/BPM of
  the one `TempoClock`): a BPM change retimes a running envelope, nothing
  keeps its own tempo. JSON: `{"ms":120}` / `{"beats":0.25}`, as `Rate`.
- `Shaper::feed(source, &features, dt, beat_len_s)`: one call per route
  per frame. `Source::parse(id)` accepts the `AUDIO_VALUES` /
  `AUDIO_EVENTS` ids once; events fire when the counter grows (the first
  frame only primes it, a held counter — stale audio — never fires);
  strength = `kick/snare/hat_strength`, 1 when the source doesn't measure
  one (browser beat).
- **Safety** — `Target::bind(reg, id)` goes through `lfo::modulatable`
  (the LFO allow-list: no transport/arm/blackout, tempo, cues, grid,
  toggles, calibration or safety settings) and caches the range, so
  `Target::apply(amount, &mut settings, &mut live)` is allocation-free. It
  calls the new `lfo::offset`, now the single place both LFOs and shaped
  audio move a control: engine copies only, clamped to the range,
  brightness can only dim below the fader. Frames are then rendered and go
  through calibration, the T-101 strobe limiter and horizon as before.
- `lfo.rs`: `modulate` now calls `lfo::offset` + `lfo::recolor_live`
  (same maths; a non-finite delta is ignored instead of writing NaN).
- Not wired into `main.rs` and no UI: nothing configures routes yet; T-153
  owns `AudioRoute`, the route editor (*Seuil*, *Courbe*, *Attaque*,
  *Relâche*, *Déclin*) and the engine loop. `index.html` untouched. The
  module has `#![cfg_attr(not(test), allow(dead_code))]` until then.

## Testing
- `cargo test -p laser-studio`: **660 passed**, 7 ignored (+2 +2
  integration). New in `audio::shape` (23 tests):
  - step 0→1, attack 10 ms: 63.2 % at 10 ms at 1 kHz, and by 10 ms + 1
    frame at 60 fps; release 150 ms: e^−1 at 150 ms, monotonic fall;
  - `Beats(0.25)` at 120 BPM (`TempoClock::default()`): under 5 % at
    125 ms ± 1 frame with attack 0 (135 ms with a 10 ms attack, the decay
    counting from the peak); 250 ms at 60 BPM; BPM change retimes a decay;
  - input oscillating 0.05 ± 0.01: the gate toggles once (chatters every
    frame without hysteresis) and closes when the signal really goes;
  - 30 vs 60 fps on a pulse train (two shaper settings) and on a kick
    train: within 0.02 at every shared time;
  - curves, gain + inverted range, feed (old counter isn't an event,
    strength, browser beat = 1, stale features decay to 0), source ids,
    NaN/∞ inputs stay bounded, sanitising, serde defaults, reset;
  - safety: targets = exactly the LFO allow-list (arm, blackout, tempo,
    cue, grid, calibration, safety ids refused); applying moves copies
    only and never arms; kick pulses on `master.brightness` /
    `look.brightness` never exceed the fader but can dim; a 12 Hz
    kick-driven brightness strobe rendered through `live::apply` and
    `safety::apply` is caught by the limiter and held steady after 5 s;
  - a 600-frame loop of two shapers + two targets allocates nothing.
  - existing LFO tests unchanged and green.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `npm --prefix studio/e2e test`: **166 passed** (no UI change).

## Risks
- The decay is measured from the **peak**, so with the default 10 ms
  attack a `Beats(0.25)` hit at 120 BPM ends at 135 ms, not 125 ms (the
  task's criterion holds with attack 0, the brightness-pulse default in
  the research). Easy to switch to "from the trigger" if preferred.
- Gate passes the raw input when open (a classic noise gate), so opening
  is a step of `gate` size before the follower; the attack smooths it.
- Audio offsets and LFO offsets on the same control are applied in turn
  (each clamps), not summed then clamped once; the brightness rule still
  holds since each step's ceiling is already ≤ the fader. T-153 can merge
  them into one sum if it matters.
- `Target::bind` allocates (registry lookup): do it when routes change,
  not per frame. Colour targets rebuild the colour override
  (`recolor_live`), as LFOs do.
- Nothing is live until T-153 wires routes into the engine loop.

## Review
