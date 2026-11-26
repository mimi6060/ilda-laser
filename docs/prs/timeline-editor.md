# PR: timeline editor in the TIMELINE workspace (T-162)

Branch: `feat/timeline-editor` · Task: `tasks/T-162-timeline.md`

## What / why

Until now a show could be played (T-160) and given a song (T-161), but it
could only be written by hand as JSON through `POST /api/shows`. This adds
the editor, so the operator can rough in a show quickly: drop cues on
tracks at the time they belong, move them, copy a phrase and paste it at
the next phrase boundary, and undo. The ideas (tracks, magnet modes,
markers that carry their events, pasting a phrase in whole bars) come from
public docs (`docs/research/pro-live-operation.md` §3.1, §3.3). The UI and
code are our own.

### Server (`timeline.rs`, `web.rs`)

The editor edits **the loaded show**. The page keeps a mirror of it and
sends every finished change whole. The server validates it and swaps it in.

- `POST /api/timeline/edit {show}` → `{state, show}`:
  - Returns 409 when no show is loaded, and 400 with a French reason when
    validation fails.
  - `Show::validate_edit` checks:
    - at most 64 tracks, 5 000 events and 500 markers;
    - track and marker names at most 64 characters;
    - every start and length finite, start ≥ 0, length > 0;
    - every cue id exists in the catalogue (figures included), or is
      already in the loaded show. A figure deleted since keeps its events
      until the operator removes them.
  - `Player::edit` replaces tracks, markers, tempo map and loop region.
    The name, time base and song stay the server's: the song is still
    managed by `/api/timeline/audio`, and renaming is `save`.
  - **Playback carries on.** Transport and position don't move and no
    playhead jump is counted, so the song doesn't re-seek. An event that
    stays under the playhead keeps its animator (instances are keyed by
    event id). Other events start or stop as if the playhead had just
    reached them.
- `POST /api/timeline/new {name, time_base}` creates a show and loads it.
  The name must pass `valid_show_name`, the same rule as `ShowStore`, so
  the file stays inside `shows/`. The call returns 409 if the name is
  already taken. The new show has two cue tracks on layers 1 and 2 and
  120 BPM in 4/4 for *Secondes*. It is written at once.
- `POST /api/timeline/save {name?}` writes the loaded show to `shows/`.
  With `name`, it saves a copy under the new name (the name is
  validated). It never touches the arm state or the transport.
- `TimelineState` gains `rev` and `modified`. `rev` counts changes to the
  loaded show: load, edit, song, loop region, save. The page re-fetches
  the show when `rev` changes elsewhere (a grid show cell, MIDI, the loop
  API, another tab). `modified` means edited since the last load or save.
  Saving the song (`/api/timeline/audio`) also saves the edits, and so
  clears `modified`.
- `Show::sanitize` now also:
  - clamps **event modifiers** to the master control ranges, with
    non-finite values reset to neutral: size 0..2, size_x/y ±2, position
    ±1, rotation ±3600, perspective 0..1, speed 0..4, brightness 0..1.
    This covers edits and show files alike;
  - sorts the markers.
- `GET /api/presets` items gain `beats`: the length of an evolving cue,
  `null` otherwise. The editor uses it as the default event length.

### UI (`index.html`, French)

TIMELINE gets an editor area across the bottom (`#tlEditor`). The stage
and the player/audio card share the top.

- **Toolbar**:
  - *Nouveau show* (name, *Secondes* / *Temps*), *Enregistrer*, and « ●
    modifié, non enregistré ». *Ouvrir* is the existing show picker, which
    asks before dropping unsaved edits.
  - *Magnétisme : Fort / Moyen / Off* (remembered in `localStorage`).
  - Show *BPM* and beats per bar. These edit the first tempo point and
    apply to *Secondes* shows; *Temps* shows follow the live tempo.
  - *Ajouter un marqueur*, *Tout afficher*, *Suivre* (the view pages
    along with the playhead).
  - *Copier la phrase*, *Coller*, *Dupliquer*, *Supprimer*, *Annuler*,
    *Rétablir*, *+ Piste*.
  - Name, colour and ✕ for the selected marker.
- **Library** on the left: every cue of the grid pages, *Figures* and
  evolving cues included (marked « évolutif · N t »). It can be filtered
  by page and text. Items are dragged with HTML5 drag and drop.
- **Track heads** (DOM, aligned with the lanes): name, layer C1–C4 (the
  event colour), M, S, ✕. All undoable.
- **Canvas**:
  - **Ruler**: bar numbers and seconds. A click seeks. A drag draws the
    loop region, whose edges can then be dragged. A double click removes
    the region.
  - **Marker lane**: a double click adds a marker. Dragging a marker moves
    the events that start exactly on it.
  - **Waveform lane**: the part of the song in view, re-fetched from
    `/api/timeline/waveform?from&to&px` once the view settles.
  - **Tracks**: events coloured by layer and brighter while active. A
    click selects, Maj+click adds to the selection and dragging an empty
    area draws a selection box. Dragging an event moves it, across tracks
    too. Dragging an edge resizes it. **Alt+drag duplicates.**
  - The playhead is extrapolated between frames while playing. The
    wheel zooms around the cursor; Maj+wheel or a horizontal swipe
    scrolls.
- **Magnet**:
  - *Fort* snaps to the grid in view: beats when they are at least 12 px
    apart, otherwise bars or groups of bars.
  - *Moyen* snaps to grid lines, markers and other events' edges within
    8 px.
  - A marker within 8 px wins in both modes.
  - *Off* doesn't snap.
- **Keys**:
  - With the timeline focused: **Entrée adds a marker at the playhead**
    and doesn't reach the tap tempo. Elsewhere Entrée is still the tap
    tempo (T-150).
  - With the timeline focused: Suppr/Retour arrière delete; Cmd+C / V / D
    / A copy, paste, duplicate and select all.
  - Anywhere in TIMELINE unless typing: Cmd+Z / Cmd+Maj+Z (or Cmd+Y) undo
    and redo, 50 levels.
  - Espace and Échap are never intercepted.
- **Copier la phrase / Coller**:
  - The phrase is the selection, or the events in the loop region when
    nothing is selected.
  - Its position is kept **in bars** from the bar it starts in. *Coller*
    puts it at the next bar start at or after the playhead, shifted by a
    whole number of bars. Each start and end is mapped through the tempo
    map, so the phrase keeps its musical length even across a tempo
    change.
  - *Dupliquer* pastes the selection right after itself (its span rounded
    up to whole bars).
  - The clipboard survives a change of show.
- **Undo** keeps snapshots of the show, taken before each finished change.
  A drag is one step. A change the server refuses is dropped and the page
  fetches the server's show again.
- **Performance**: redraws only when something changed or while
  playing. Only the events in view are drawn. `window.tlDebug` exposes the
  view and the draw time for the e2e tests.

### Laser safety

- The editor never arms, and neither do its routes (tested). Enregistrer
  only writes the show file.
- Editing while playing only changes which events are active and where.
  Events still go deck → layers + point budget → live stage → calibration
  (clamped to -1..1) → strobe limiter / horizon → gate, as before.
  Modifiers from a show can no longer be pushed out of range, even by a
  hand-made request. The e2e test edits while playing and checks that
  every frame point is within ±1.
- Échap / e-stop behaviour is unchanged (T-160): the player halts, and
  editing afterwards doesn't restart it.

## Testing

- `cargo test -p laser-studio`: 630 passed, 7 ignored (after rebasing on
  develop 4d64be2). New tests:
  - `timeline.rs`:
    - an edit during playback keeps it playing: same position, no jump,
      the active event keeps its instance, a moved event starts under the
      playhead, name/base/song unchanged, `rev`/`modified`;
    - validation: unknown cue, zero length, negative start, and each limit
      and name length;
    - modifiers clamped, NaN and infinities reset;
    - `new_empty`.
  - `web.rs`:
    - `new` (bad name 400, duplicate 409, written to disk);
    - `edit` (409 without a show, unknown cue 400, ids given, applied
      while playing, not saved until `save`);
    - `save` and save-as (`../` refused);
    - never armed.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `studio/e2e/tests/timeline-editor.spec.ts` (9 tests):
  1. *Nouveau show*, then drag the first library cue onto track 1, 5 px
     after bar 5. The event starts at exactly 8 s, the file stays
     untouched until *Enregistrer*, and a screenshot is taken
     (`test-results/timeline-editor.png`).
  2. In a *Temps* show, a drop at bar 3 → beat 8, 16 beats long.
  3. Move 2 bars + 7 px onto track 2 → 12 s. Resize by 33 px → the end
     lands on the beat (len 9). *Moyen* → onto the marker at 13.3 s.
     *Off* → 13.6 s, off the grid.
  4. Select all, *Copier la phrase* (« 8 événement(s), 8 mesure(s) »).
     With the playhead at 20.3 s, *Coller* → « Collé à la mesure 12 »:
     every pasted start is the original + 22 s (11 whole bars), same
     lengths and sources, unique ids. Then Cmd+Z / Cmd+Maj+Z, the
     *Annuler* / *Rétablir* buttons, Cmd+D (+19 bars), Suppr, and Cmd+Z
     again.
  5. Entrée on the page leaves the markers alone. Entrée with the
     timeline focused adds a marker at the playhead, and the BPM is
     unchanged. Dragging a marker 4 → 6 s moves the event that started on
     it, not the other. Rename it. *Ajouter un marqueur*.
  6. Loop region by dragging the ruler → [4, 8]. A click seeks and a
     double click clears the region. Wheel zoom keeps the time under the
     cursor. *Tout afficher*.
  7. *Enregistrer* → `modified` false and the file has the event and the
     marker. Load another show, reload the page, open the first one from
     the picker (the unsaved-changes prompt is accepted) → the events and
     marker are back and undo is empty.
  8. Resize an event while the show plays: it keeps playing with the
     event active, the frame is lit, and every point is within ±1. A
     hand-made edit with an unknown cue gets a 400 and playback carries
     on.
  9. 500 events on 4 tracks while playing: ≥ 30 draws per second, with
     an average draw time well under 33 ms.

  Every test checks it's still disarmed afterwards.
- Full e2e suite after the rebase: **166 passed**.

## Risks

- **Unsaved edits are live but not on disk.** A grid show cell (from the
  page or MIDI), `/api/timeline/load` or a project open reload the file
  and drop them without asking. Only the page's show picker and *Nouveau
  show* ask first. Saving the song settings (`/api/timeline/audio`) also
  saves the edits.
- The page sends the whole show on each change: about 100 KB for 500
  events, once per finished drag, never during it. It's fine locally.
  The 5 000-event cap bounds it.
- Two tabs editing the same show: the last change wins. The other tab
  picks it up through `rev`, but its undo stack is not merged.
- `Look` events (a look stored in the show) are accepted as sent. The
  editor never creates them, and their settings go through the normal
  render path, but a hand-made request could send extreme look values.
  Calibration still clamps positions. Non-finite values can't be written
  in JSON, but a huge `f32` could overflow to infinity.
- Moving a marker carries only the events whose start is *exactly* on it,
  to 1 µs. An event placed with *Off* near a marker doesn't follow.
- *Temps* shows count bars with the live tempo clock's meter, like the
  player. If the meter changes, bar positions in the editor change too.
- Only the first tempo point is editable in the toolbar. A tempo map
  with several points is used for the ruler and snapping but can only be
  edited through the API.
- Drag and drop from the library uses HTML5 drag events. It works with
  a mouse and a trackpad; there is no keyboard alternative yet.

## Review
