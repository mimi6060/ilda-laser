# feat/evolving — T-111 evolving-cue engine (keyframes over beats)

## What / why
An evolving cue changes by itself over 8 to 32 beats, for example a fan
that rises and speeds up into the drop. That is what makes a show look run
by a pro LJ without anyone touching it (festival-looks §5, E1–E12). This
branch adds the engine only. The twelve festival cues are T-112..T-123.

- `studio/src/evolving.rs` (new):
  - `Content::Evolving(EvolvingCue)` with these fields:
    - `length_beats` (default 16)
    - `loop` (JSON name; `looped` in Rust)
    - `launch`: `beat` (default) or `bar`
    - `keys`, in any order
  - Each `EvolvingKey` holds `at_beats`, `generator`, `params: GenParams`,
    `color`, `scale`, `brightness`, `gate_beats`, `strobe_div` and `ease`.
    Every field is `serde(default)`, so a key only lists what it changes.
  - Easing is `KeyEase { step, linear, ease_in, ease_out, smooth }`. It is
    not called `Easing` because `generators::Easing` (the fan-sweep shape)
    already exists. T-157 should reuse `KeyEase`.
  - `EvolvingCue::sample(b)` gives the look at local beat `b`. It is a pure
    function:
    - **Numeric values** follow the current key's easing towards the next
      key: colour (RGB), size, brightness and gate. `a`, `b`, `speed`,
      `color2` and `period_beats` also ease, but only when the next key uses
      the same generator. Easing a fan's `a` towards a tunnel's `a` means
      nothing.
    - **Discrete values** switch exactly on the key's beat: the generator,
      `count`, colour mode, `steps_per_beat`, `direction`, groups and
      `strobe_div`. Any future `GenParams` field (a chase shape, for
      example) is discrete by default.
    - **Loop**: after the last key, the values ease back towards key 0,
      which is reached at `length_beats`. The local beat wraps, so every
      loop is identical. A key at exactly `length_beats` is only a target.
      **One-shot**: the last key holds, its generator keeps moving, and the
      progress reports `ended: true`. That is the hook T-160 will use to
      chain to the next cue. No chaining is done here.
    - **Phase-continuous period.** When `period_beats` changes (a rotation
      speeding up from 1/32 to 1/4 turn per beat, E2), a plain lerp makes
      the phase `b / period` jump, or even run backwards. The generator is
      instead given `period_beats = b / ∫₀ᵇ dx / period(x)`, so its phase
      is the number of cycles actually done. It is exact for constant or
      stepped periods, and Simpson-integrated (32 steps per key segment)
      for eased ones. Beat-locked things (steps, pumps, gates) still use
      the real `b`.
  - `Launch::quantize`: next beat by default (a press within 1/1000 beat
    after a beat counts as on it), or the next bar's « one ». Until then the
    cue holds its first frame and the progress shows `waiting`.
  - `strobe(b, div)`: `div` flashes per beat, lit for the first half of
    each slot, capped at 8 per beat. The T-101 limiter, which works on the
    finished frame, still bounds the flash rate.
- `engine.rs`: `Animator::render` sends `Content::Evolving` to
  `render_evolving`:
  - It samples the keys at the local beat and builds a plain generator
    `Settings` (beat-synced, with the key's gate).
  - The key's brightness is multiplied by the operator's look brightness
    (and the strobe). The look's rotation and audio settings still apply.
  - The look is drawn through the existing path (`render_look`, which is
    the old body of `render`), with `beat_pos` set to the local beat.
  - The launch beat is the cue's `started_beat` (from the deck), or the
    current beat when a look already on show is switched or edited into a
    different evolving cue. Edits around the cue (brightness, rotation,
    LFOs) don't restart it. Editing its keys does.
  - `Animator::progress()` reports pos, length, key, keys, loop, waiting,
    ended, pass and beats_per_bar.
- Deck, layers, point budget: nothing special. An evolving `Settings` is a
  look like any other, so a `Preset` or scene can hold one. It plays
  through `CueDeck`, layers 1–4, `layers::mix` (point budget), live
  modifiers, calibration, the T-101 limiter and the arm gate, in the same
  order as before.
- `main.rs` / `web.rs`: each frame records the progress of every evolving
  look on show. `/api/frame` and `/api/state` gain
  `evolving: [{pos, length, key, keys, loop, waiting, ended, pass,
  beats_per_bar, cue, layer}]`, where `cue` is `null` for the manual look.
- UI (`index.html`):
  - A progress bar under the preview: « Évolutif 5.2 / 16 temps », a fill,
    one tick per bar, and « clé 2/3 · boucle 1 », « attend le temps » or
    « fin (dernière clé) ». It follows the newest evolving cue on show.
  - A read-only note in « Contenu » (« Cue évolutif : N clés sur L temps… »).
  - No key editor, as the task says. Keys are JSON through
    `/api/settings`.
- `Content::same_drawing`: two evolving cues count as the same drawing, so
  editing one while cues play edits the top cue instead of stopping the
  deck (same as text).

Not added: no evolving cue in the catalogue. The « Festival évolutifs »
page and E1–E12 are T-112..T-123. `presets.rs` is untouched, so the cue
ids and the golden frame digests don't change.

## Testing
- `cargo test -p laser-studio`: **318 passed**, 2 ignored, rebased on
  develop 82353c0 (+19 from this branch).
  `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean. The
  golden digests are unchanged: cue frames, the 20 pre-T-102 generators,
  and the cue ids.
- `evolving.rs` (11 tests):
  - The easing curves.
  - The criterion: a 16-beat cue with 2 keys gives `scale` 0.6 at beat 8.
  - Ease-in and step.
  - The generator and discrete params switch at 7.999 → 8.0.
  - Colours and params ease, but params only within one generator.
  - The loop comes back to key 0 at `length_beats`, and one-shot holds
    its last key.
  - Unsorted keys, a first key after 0, keys past the end, no keys.
  - Launch quantization (beat, bar, 3/4, the on-the-beat tolerance).
  - The period: constant → unchanged; eased 8 → 1 → phase always moves
    forward, no jump, total ≈ (16/7)·ln 8; stepped → no jump at the key.
  - Strobe.
  - JSON round trip with defaults.
- `engine.rs` (6 tests):
  - A cue launched on beat 4, at beat 12, draws exactly the plain `fan` at
    size 0.6.
  - The same frame at 120 and 150 BPM.
  - Key brightness × look brightness, and the key's gate.
  - The generator switch within ±1 frame (1/60 beat) against the plain
    looks.
  - Pressed at 5.3, the cue holds until 6, then counts from 6.
  - The loop frame after 16 beats equals the first frame (`pass` 1).
  - One-shot `ended`.
  - A manual look switched to an evolving cue starts then. A brightness
    edit doesn't restart it; a key edit does.
  - Strobe on the beat grid.
  - `Settings` JSON.
- `controls.rs`: a 32-beam evolving preset on layer 2, over a catalogue cue
  on layer 1, played through `press_cue`. It is the primary look and sits
  on layers [1, 2]. At 6 beats across the key switch it is lit, and the
  mixed frame stays within the 750-point budget with layer 1 never cut.
  Stopping it gives the layer-1 cue back.
- `scenes.rs`: an evolving scene saved and reloaded is equal.
- e2e `studio/e2e/tests/evolving.spec.ts` (3 tests, also run
  `--repeat-each 5`, all green):
  - At 240 BPM, a 4-beat loop is posted to `/api/settings`, then
    `/api/frame` is read at several moments:
    - Launch on a whole beat (clock − pos ≈ integer).
    - Key 0 is a small green fan and key 1 a wide red one (width + colour).
    - Then a new pass back on key 0.
    - After reset, no evolving cue.
  - UI: the progress bar and note appear, and the bar fills. A brightness
    edit from the panel keeps the evolving content. It is hidden after
    reset.
  - Saved as a scene → studio restarted → the scene still holds the cue →
    playing it shows it again.
- Full e2e suite on the rebased branch (final run): **74 passed**. There
  was one unrelated flaky failure in an earlier full run:
  `live.spec.ts` « Synchro tempo keeps the preset step ». It passed 3/3 on
  its own.

## Risks
- **Look « Taille » and « Couleur » don't apply to an evolving cue.** They
  come from the keys (absolute sizes, so the E-cue specs can be written as
  they are). The master size (`Direct`) and the look brightness still
  apply. The UI note says so.
- **Launch quantized to the next beat**, as the task says. The generators
  get the cue's local beat, so a period-4 sweep launched on beat 3 of a bar
  is out of phase with the bar. Cues or timelines that need the bar should
  use `"launch": "bar"`. T-100 looks are unchanged: they still count from
  the start of their launch bar.
- **The period trick** hands generators an effective `period_beats` that
  differs from the key's value while the period is easing. That is correct
  for anything that uses the period as a phase (`ctx.cycle`, beat_sync `t`).
  A generator that used `period_beats` for something else would see the
  shift. None does today.
- **Direction flips** (`direction` is discrete) still mirror the phase
  instantly. E8's « ease through zero » will need keys on `a` or `speed`,
  or a small ramp of keys.
- Cost: `sample` scans the keys a few times per call, and the phase
  integral adds about 3 calls per key segment, or 33 when the period eases.
  With 32 per-beat keys that is a few thousand float operations per frame.
  It is negligible, but it grows as keys².
- Strobe up to 8 flashes per beat (33 Hz at 250 BPM) is allowed at the
  cue level. The T-101 limiter is what enforces the 4 Hz / 5 s rule. An
  evolving strobe longer than 5 s gets held steady by it.

## Review
