# feat/timeline — T-160 timeline show model and player

## What / why
Two uses: a show locked to a given song, replayed identically, and bar
phrases that follow the live tempo (the festival timelines T-124–T-129).
This adds the show model and the player; the editor is T-162.
- `studio/src/timeline.rs` (new):
  - `Show { name, time_base, tempo_map, audio, tracks, markers, loop_region }`
    saved as `studio-data/shows/<name>.json` (`ShowStore`; names are
    letters/digits/space/`-`/`_` only, so a name can't leave the folder).
    Everything `#[serde(default)]`; `sanitize()` clamps BPM/meter/layers,
    sorts, and gives events unique ids.
  - **Secondes** base: positions are seconds; `beat_at_s` / `s_at_beat` /
    `bar_at_s` walk the tempo map (continuous and exact at a change; moving
    a tempo point moves no event).
  - **Temps** base: positions are beats of the one `TempoClock`. Play waits
    for the next bar (now, if exactly on the one); a BPM change speeds the
    phrase up with no jump because the clock itself never jumps.
  - `Track { kind: Cues|Bus, layer 1–4, mute, solo, events }`,
    `Event { start, len, source: Cue{id}|Look{settings}, time: Beats|Seconds|Fit,
    offset_beats, end: Stop|Hold|Continue, fade_in, fade_out, to_next,
    modifiers, envelopes }`, `Marker`, `TempoPoint`, `AudioRef`, `Envelope`/`Key`.
  - `Player`: play / pause / stop / seek, loop (show region or whole show;
    the remainder is kept, so the playhead lands where it should and events
    spanning the loop point keep their animator), stops by itself at the
    end. `frame(clock)` returns the `TimelineCue`s of the active events:
    layer, fade gain, content beat (its own clock per time mode), frozen
    flag (paused, or *Garder* after its end), event modifiers.
- Engine (`main.rs`, `timeline_cues`): timeline events are rendered with
  their own animators (ids ≥ 2^62, never clashing with deck instances) and
  **joined to the cue deck's looks before `layers::mix`**, so they go
  through layer dimmer/mute/solo, the point budget, the master live stage
  (show hybride: master modifiers apply on top), calibration, the strobe
  limiter/horizon and the output gate exactly like cues played by hand.
  Timeline events come first in a layer, cues played live draw on top.
  Per-event `modifiers` are applied to that event's points before the mix.
- Safety: the player never reads or writes the arm state. While the e-stop
  is latched the engine halts the timeline every frame (no frozen frame
  either); `timeline.play` is refused until the reset, and resuming after
  the reset is an explicit *Lecture* (the position is kept).
- Controls: `timeline.play`, `timeline.pause`, `timeline.stop` (triggers),
  `timeline.loop` (toggle), with LED feedback in `current()`.
  `docs/controls.md` regenerated. Play takes over like a latched cue
  (`look_on = false`, playlist stopped); the operator's master values are
  untouched.
- Grid « cue de type show »: `CueSlot.show` — a press on that cell loads
  and plays the show, a second press stops it (release ignored).
- API: `GET/POST /api/shows`, `GET /api/timeline` (state + show),
  `POST /api/timeline/{load,play,pause,stop,seek,loop}` (`load {name}` or
  `{show}`; `loop {on?, region?}` with `region: null` clearing it), and
  `timeline { name, position, length, beat, bar, beats_per_bar, playing,
  paused, waiting, loop, loop_region, active }` in `/api/state` and
  `/api/frame`.
- UI (minimal, French): section *Timeline* under the layers — show picker,
  *Lecture / Pause / Arrêt / Boucle*, position « 0:12.3 / 1:00.0 · mesure 7.2 »
  (« attend la mesure suivante… » while waiting), a bar with the playhead
  and loop region (click = seek), refusal message. « Propriétés du cue »
  gains *Show*.

## Adapted / left out
- Fade and transition lengths are a `Dur { Seconds | Beats }` instead of
  `Rate` (a `Rate::Hz` is a frequency, not a duration).
- Envelopes and *Bus* tracks are stored but not evaluated (T-163); bus
  tracks play no content.
- *Ajuster* stretches a nominal 16-beat program (`FIT_BEATS`) until cue
  programs have a length (T-157). *Morph* plays as a crossfade.
  *Secondes* content runs at a nominal 120 BPM.
- `offset_beats` on events covers T-124's "second half of an evolving cue".
- Audio, timecode, the editor, recording: T-161/T-168/T-162/T-169.

## Testing
- `cargo test -p laser-studio`: 355 passed, 2 ignored, on develop 04227e5 (see the task
  Journal). `timeline.rs`: event active exactly in [4, 6[ s; tempo map
  120→140 at 30 s continuous and round-trips; Temps show launched at bar
  3.5 starts at 4.0 (and at once when on the one); 128→150 BPM keeps the
  position and the event; loop wrap keeps the remainder and the instance;
  fades in/out (seconds and beats), Garder/Continuer, crossfade; content
  clocks per time mode; seconds content in a Temps show doesn't jump on a
  tempo change; mute/solo/bus; pause/seek/stop; 60 s simulated at 60 fps
  (30 events in order, stops at the end); sanitize; store round trip and
  rejected names; `look_of`. `main.rs`: the timeline feeds the frame, an
  e-stop halts it for good, play refused while latched, never arms.
  `controls.rs`: transport controls, grid show cell. `web.rs`: shows and
  timeline routes, e-stop → play 409, never armed.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `studio/e2e/tests/timeline.spec.ts` (6 tests, also `--repeat-each 3`):
  load + play + playhead + stop (output dark afterwards), pause freezes,
  seek by clicking the bar, loop toggle; control ids; master size and
  brightness apply over the show; Escape stops output and timeline, play
  refused until reset, never armed; grid cell set to a show. Full suite: 86 passed.

## Risks
- Playing a show sets `look_on = false` (like a latched cue), so after
  *Arrêt* the output is dark until something is picked.
- Seeking restarts the content animation of the events under the playhead.
- The timeline position in `/api/state` is the one computed by the last
  engine frame (≤ 1 frame late).
- A Temps show resumed from pause also waits for the next bar and shows its
  frozen frame meanwhile.
- Unit tests save shows under a per-process temp folder that is not deleted.

## Review
