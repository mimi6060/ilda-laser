# fix/ui-state — T-292 stale active cue, T-293 page follows the look

## What / why
Two bugs found by the e2e suite (T-004), both about the server and the
page disagreeing on "what plays".

**T-292 (server): a cue stayed « en cours » after the look was taken over.**
Scenes and the playlist already went through `controls::show_look` (all
cues stop, `active_cue = None`) since T-155; the hole was
`POST /api/settings`, which always edited the top cue. Keeping the T-155
design (look-panel edits apply to the newest cue), the new
`controls::set_look` draws the line on *what is drawn*:
- another drawing — other content kind, other shape, other generator
  (`Content::same_drawing`) — is the operator taking over by hand: the
  cues stop and the look shows by itself, like a scene (`show_look`);
  `active_cue` becomes `null`, grid LEDs go off;
- any other edit (size, colour, brightness, rotation, music, the text,
  generator parameters) still edits the playing cue, which stays active;
- `master.*` modifiers and tempo never touch the deck (unchanged).
The playlist stops on any `/api/settings`, as before.

**T-293 (page): it didn't follow look changes it didn't make.**
- `Shared::settings_rev: u64` (not saved anywhere) is bumped whenever
  `Shared::settings` changes: `/api/settings`, `show_look` (scene,
  playlist start), playlist advance, `with_deck` when the top cue's look
  changes (cue press/release from UI, API or MIDI grid), and `look.*` /
  `audio.*` controls. Exposed in `/api/frame` and `/api/state`;
  `POST /api/settings` answers `{ "rev": n }`.
- `POST /api/settings?rev=N` is refused with **409** when `N` is not the
  current revision: a page can no longer post a look that was replaced
  since it loaded it. Without `?rev` (scripts, the e2e harness) it works
  as before.
- `index.html`: the look panel now keeps edits as small per-control
  functions (one field each, with the value it had when moved) until the
  server accepts them. When `/api/frame` shows another revision, the page
  reloads `/api/state` and replays its unsent edits on top. On a 409 it
  does the same and resends. So a slider move sends the current look
  plus that one change, never the old look. Look requests are queued one
  at a time. A control that has focus (being dragged / typed in) is not
  overwritten on screen; it is redrawn when it loses focus.
- The « à plus long terme » item (per-control `/api/control look.*`
  instead of whole `Settings`) is not done: the rev check gives the same
  guarantee without new controls for colour/content/generator fields.

## Testing
- `cargo test -p laser-studio`: 168 passed, 2 ignored (+6): `set_look`
  with a new drawing stops the cues (LED off) / with an edit keeps the
  cue; scene clears, `master.*` and tempo keep the cue; the revision
  moves on every look change and not on master modifiers or an ignored
  release; the playlist advance bumps it (main.rs); `?rev=` parsing.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- `npm --prefix studio/e2e test`: **56 passed, 0 skipped** (was 49 + 4
  fixme). The two fixme tests are now real tests (T-292 cues, T-293
  scenes), plus new: size edit + master size keep the cue; a scene stops
  the cue; a cue played through `/api/control grid.1.1.3` updates the
  Effet panel and the next brightness move keeps that cue's content.
- `--repeat-each 5`: 280 passed, 0 failed.

## Risks
- Behaviour change: picking another shape / content kind / generator
  (including the « Effet » generator select) while cues play now stops
  them all (held flashes too) instead of replacing the top cue's
  drawing. Tweaks still edit the cue. Reviewer: confirm this is the
  intended reading of T-155 + T-292.
- A page edit that races a look change elsewhere is replayed on the new
  look (e.g. a colour picked just as the playlist advances applies to
  the next scene). A shape click racing a cue press from MIDI stops that
  cue — the most recent intent wins.
- Other `/api/settings` clients without `?rev` can still overwrite
  anything (unchanged, documented above).
- More `/api/state` fetches: one per look change made elsewhere (a MIDI
  fader on `look.size` → up to ~30 a second while moved).

## Review

Reviewed by the architect (integrator). Merged after beat-gen and
midi-map; the page's per-field look edits (LOOK_EDITS) now also cover the
T-100 « Tempo du look » fields, the beat-sync checkbox is synced with
`.checked`, and both sides' web.rs tests are kept. 255 unit + 58 e2e green
(0 fixme left), clippy clean. Accepted behaviour change: choosing another
shape/content/generator while cues play stops them (the look shows on its
own, like a scene); other edits keep editing the top cue.
Verdict: APPROVED
