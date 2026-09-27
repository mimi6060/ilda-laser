# feat/tempo — T-150 single tempo clock

## What / why
One tempo clock for the whole app (`studio/src/tempo.rs`), so every
beat-synced feature (festival generators T-100, colour chases, strobes,
evolving cues, timeline) reads the same beat and nothing drifts.
- `beat_at(t) = (t - origin) * bpm / 60`; `set_bpm` re-anchors the origin
  so the beat position never jumps; BPM clamped 40–250, default 120, 4/4.
- Tap: last 8 taps, reset after a 2 s pause, BPM = 60 / median interval
  from the 3rd tap, last tap lands on a whole beat. Resync = now is the
  "one" of a bar. Nudge ±1/32 beat, ×2, ÷2.
- Controls (T-145 registry): `tempo.tap`, `tempo.resync`, `tempo.bpm`,
  `tempo.nudge_up/down`, `tempo.double/half`. `/api/state` and
  `/api/frame` expose `tempo { bpm, beat, bar, beat_in_bar, phase, … }`.
- UI: tempo bar in the header (editable BPM, 4 beat dots with the "one"
  highlighted, Tap, Sync 1, ÷2, ×2, nudge). Keys: Enter = tap, Backspace
  = resync (ignored while typing). Space/Escape unchanged.
- The browser's audio beat still drives flashes; auto-BPM from audio is a
  later task.

## Testing
- 62 unit tests (+10): 4 taps at 500 ms → 120.0; lone tap after silence
  ignored; median tolerates a sloppy tap; no jump on BPM change; resync
  puts now on beat 1; no drift after 1 h at 128 (7680.000); nudge; bounds;
  bar/beat maths; tempo controls through the registry. Clippy clean.
- API: taps via HTTP set source=tap; resync → beat_in_bar 0. Typed BPM in
  the UI → 128.
- Note: a background browser tab throttles timers to 1/s, so synthetic
  taps from a hidden tab measure ~60 BPM; real taps in the visible tab are
  fine (server timestamps; HTTP latency is roughly constant).

## Review
Architect self-review (no separate reviewer agent).
Verdict: APPROVED
