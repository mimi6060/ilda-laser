# feat/timeline-audio — T-161 audio file and waveform in the timeline

Branch: `feat/timeline-audio` · Task: `tasks/T-161-timeline.md`

## What / why

Programming to the music: import the song, see its waveform, hear it, and
have the laser follow it without drift. The editor (drag and drop) is T-162.

### Where playback happens: native, and the song is the clock

**Decision: the studio plays the song itself** on the Mac's default output
(cpal / CoreAudio), not the browser.
- The show must keep going with the tab hidden or closed (same reason as
  the native capture, T-230). A background tab also throttles the timers a
  browser-side sync would need.
- Only the native side knows which sample the sound card is playing now
  (CoreAudio reports each buffer's playback time).

**Sync: the song is the transport's clock** (`studio/src/audio/playback.rs`):
- The timeline player still owns the transport (play, pause, seek, loop,
  stop, e-stop). It now counts every playhead discontinuity (`jumps()`).
  Each one becomes a `Command { gen, playing, file_s, at_s, gain }`: "at
  studio time `at_s` the song is at `file_s`".
- The output callback (`Renderer::render`) takes a new command and starts
  the song where the playhead will be **when that buffer is heard** (callback
  time + CoreAudio's reported output latency), so nothing jumps when the
  audio takes over. Every buffer it publishes a `ClockSnap` (song position of
  the buffer's first sample + the studio time it reaches the speaker).
- Every engine frame, `SongSync::sync` (in `timeline_cues`, after the
  e-stop check) sends the commands and, once the snapshot answers the
  latest command, pulls the timeline towards the song position
  (`Player::follow`: 10 % of the error per frame; > 100 ms = jump). The
  sound card's crystal, not the system clock, sets the pace, so there is no
  drift however long the song.
- Until the song answers (device opening, file decoding, `--no-audio`, no
  output device, device lost, a snapshot older than 0.5 s), the timeline
  runs on the system clock exactly as before.
- **Latency**: CoreAudio's own output latency is taken into account
  automatically; the laser path (DAC buffers, projector) is compensated
  with the show's *Décalage* (±500 ms; positive = the sound is heard later
  than the laser). The show position is what the laser shows; the song
  position heard = show position − offset.
- The callback is real-time safe: no allocation (tested with the counting
  allocator), `try_lock` only (a busy lock = the command is taken next
  buffer), 5 ms fades on start / pause / jumps against clicks.
- Threads: callback, **song-playback** thread (decodes the wanted file,
  opens the output, reopens it every 2 s after a failure; owns the stream),
  engine. None waits on another; the HTTP worker never touches CoreAudio.

### Files

- `audio/decode.rs` (new): format recognised from the bytes (RIFF/WAVE,
  FORM/AIFF|AIFC, fLaC, ID3 / MPEG sync). WAV → `hound`, FLAC → `claxon`,
  MP3 → `nanomp3`, AIFF / AIFF-C (`NONE`, `twos`, `sowt`, `fl32`) → a small
  reader here. Result: interleaved `i16`, 1–2 channels (more: the first two),
  30 min max. `WaveformPeaks { block: 256, min, max, sample_rate, frames }`
  (min/max over all channels per block) and `view(offset, from, to, px)`.
  French errors (« fichier WAV illisible : … », « format audio non reconnu »,
  « le fichier ne contient aucun son »…).
- `audio/media.rs` (new): `MediaStore`, `studio-data/media/audio/`.
  - Names: `<stem>.<ext>`, stem = `valid_show_name` (letters, digits,
    space, `-`, `_`, ≤ 64), ext ∈ wav/mp3/aif/aiff/flac (lower case). An
    uploaded name is reduced to its last path component and sanitised
    (`../../Mon Titre (live).MP3` → `Mon Titre _live_.mp3`). Symlinks are
    not followed; the cache folder isn't listed.
  - Import: **decoded before anything is written** (a damaged file leaves
    nothing), written to `.name.part` then renamed. Same bytes again → the
    existing file; other bytes, same name → « nom 2 ». 512 MB max.
  - Peaks cached in memory and on disk (`.peaks/<file>.peaks`, binary,
    keyed by the file's size + mtime: a replaced file is decoded again).
- `audio/playback.rs` (new): `Renderer`, `SongHub`, `SongSync`, the
  `AudioOut` trait (`CpalOut`; a fake in tests), the `Worker` and `spawn`.
- `timeline.rs`: `AudioRef` gains `duration_s` (noted when attached, so
  `Show::end()` covers the song) and doc; `MAX_AUDIO_OFFSET_S`; `sanitize`
  clamps offset ±0.5 s and gain 0..1; `Show::song()` (Secondes shows with a
  file only); `Player::jumps()` / `follow()`; `TimelineState.audio`.
- `main.rs`: `Shared.media`, `Shared.song`, `Shared.song_sync`; the
  song-playback thread starts with the audio threads (not with
  `--no-audio`, whose message now says so).
- `web.rs`:
  - `GET /api/media/audio` → `{files:[{file,size}], extensions}`.
  - `POST /api/media/audio?name=<file name>`, raw bytes as the body →
    `{file, duration_s, sample_rate, channels, existing}`; 400 with the
    French reason, 413 if too big. The lock is not held while reading,
    decoding or writing.
  - `GET /api/timeline/waveform?from=&to=&px=[&file=]` → `{file,
    duration_s, offset_s, sample_rate, block, from, to, px, min[], max[]}`
    in show seconds (default: the loaded show's song over the whole show;
    `px` 1..8192; values to 3 decimals).
  - `POST /api/timeline/audio {file?: string|null, offset_s?, gain?}` on
    the loaded show (409 without one, 400 for a Temps show or an unknown
    file); the show is **saved at once** (it is the file in `shows/`).
  - `timeline_audio {state: disabled|idle|loading|ready|error|no_device,
    file, message, device, sample_rate, clock: system|audio}` in
    `/api/state`, `/api/frame` and `GET /api/timeline` (`audio`).
- `index.html`, TIMELINE workspace: an **Audio** track above the position
  bar: song picker (« Aucun morceau »), *Importer un morceau…*, *Décalage*
  (−500…+500 ms), *Volume* (0–100 %), status (« lecture sur … · horloge du
  morceau / système », « lecture audio désactivée (--no-audio) », errors),
  and the waveform canvas with the playhead (click = seek). The « À venir »
  note no longer lists the waveform.
- `.gitignore`: `studio-data/` at any depth and `*.wav *.mp3 *.aif *.aiff
  *.flac`, so the user's songs can't be committed by accident.

### Adapted / left out
- **No `symphonia`** (the task suggested it): it is MPL-2.0 and the rule is
  MIT/Apache by default. The three crates above plus our AIFF reader cover
  the four formats. AAC/M4A/OGG are not accepted.
- `AudioRef.duration_s` added to the task's model (the show length must
  cover the song without decoding it on every `end()`).
- The song belongs to Secondes shows only (a Temps phrase follows the live
  tempo, not a file): the picker is disabled and the API says why.
- No per-show latency calibration tool; the offset is set by ear/eye.

## Testing

- `cargo test -p laser-studio`: 542 passed, 5 ignored (rebased on develop
  2426fa6). New tests:
  - decode: **generated 10 s WAV, sine 0.5 → peaks max ≈ 0.5, min ≈ −0.5**,
    every block reaches the crest; stereo peaks, silence flat; view with
    offset, zoom, past the end, empty range; WAV 8/24-bit int and 32-bit
    float; > 2 channels; AIFF, AIFC `NONE` and `sowt` (written by the test);
    FLAC (a VERBATIM FLAC written by the test, with its CRCs); MP3 (silent
    MPEG-1 Layer III frames behind an ID3 tag, built by the test);
    **damaged files**: empty, text, truncated headers, bad FLAC/MP3/AIFF, then
    about 1 400 truncated, mutated or random files behind every magic: an
    error, never a panic.
  - media: names and sanitising, import refuses a damaged file and writes
    nothing, dedup / « nom 2 », disk cache read back by a fresh store,
    replaced file re-decoded, symlink not followed.
  - playback: the renderer starts where the playhead is when heard (latency
    included), pause, gain, resampling (interpolation), mono / stereo / 4-ch
    devices, silence past the end with the clock still counting; **the
    callback never allocates**; **3 minutes of playback with a sound card
    100 ppm fast and ±1 ms callback jitter: worst gap timeline ↔ song heard
    ≈ 0.5 ms (< 10 ms asserted, < 3 ms in practice)**, where the system
    clock alone would be 18 ms off; transport → commands (play, seek, halt =
    song paused, volume without re-seek, show without a song → silence);
    stale / foreign / `--no-audio` clocks not followed; worker: device
    missing → retried every 2 s, lost → reopened, damaged / missing file →
    clear error and no output opened.
  - timeline: song length / sanitising / old show files, `follow` and
    `jumps` (incl. the loop wrap), Temps shows never follow.
  - web: import with a damaged file (400, nothing written), a path in the
    name (kept inside `media/audio/`), attach (clamped, show saved, length),
    waveform in show time with the offset, `file=../…` refused, detach;
    percent-decoding of the query.
  - Opt-in, real hardware (`#[ignore]`):
    `cargo test -p laser-studio real_output -- --ignored --nocapture` opens
    the default output and plays **at volume 0** for 2 s. Run once here:
    « Haut-parleurs MacBook Pro » at 44.1 kHz, song clock 1.7067 s over
    1.7067 s of callbacks, song where the playhead is (< 30 ms).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e (`npm --prefix studio/e2e test`, after the rebase): **144 passed**.
  New `tests/timeline-audio.spec.ts` (3 tests; WAVs generated in the test):
  import from the TIMELINE workspace → song attached, length 3 s, file only
  in `media/audio/`, waveform API peaks ≈ ±0.5 and canvas inked, offset
  +200 ms / volume 40 % applied, clamped and saved in `shows/*.json`, the
  waveform shifted by the offset, play / pause, click on the waveform
  seeks, reload the show → song back, « Aucun morceau » → removed, picked
  again from the library; damaged WAV / fake MP3 / `.txt` → « Import
  refusé : … » with the reason, nothing written, still disarmed; names
  confined, `waveform?file=../…` 404, Temps show → picker disabled and 400,
  import without a show kept in the library. Screenshot:
  `studio/e2e/test-results/timeline-audio.png`.
- No test plays sound: unit tests use a fake output, e2e runs `--no-audio`.

## Licences (new crates, checked 2026-09-28 from each crate's Cargo.toml and LICENSE files; no transitive dependencies)

| Crate | Version | Licence | Use |
|---|---|---|---|
| hound | 3.5.1 | Apache-2.0 | WAV |
| claxon | 0.4.3 | Apache-2.0 | FLAC |
| nanomp3 | 0.1.1 | MIT OR Apache-2.0 | MP3 (pure-Rust port of minimp3, itself CC0) |

`symphonia` (MPL-2.0) deliberately not used. No third-party content file
added (`docs/CONTENT_SOURCES.md` unchanged): every test signal is generated.

## Risks

- **It makes sound.** Loading a show with a song opens the Mac's default
  output and *Lecture* plays it (at the show's volume). `--no-audio`
  disables it. The e-stop pauses the song with the timeline.
- `nanomp3` is a machine translation of C (unsafe inside); it is fed the
  user's own files and fuzzed lightly in the tests, but a crafted MP3
  could in theory crash it. Decoding runs on the HTTP worker (import) and
  on the song-playback thread; a panic there would trip the panic hook
  (e-stop) and take that thread down.
- Memory: the song is held decoded (16-bit): ~10 MB per stereo minute at
  44.1 kHz; the import holds the upload in memory (512 MB max). 30 min max.
- Output format: only devices whose default format is f32 (all CoreAudio
  devices in practice); others → `no_device` with the reason.
- Following the song: callback timing jitter is smoothed by the 10 %
  correction; if CoreAudio mis-reports its latency the difference is
  constant and belongs in *Décalage*. A seek or loop wrap costs one frame
  (~16 ms) before the song is re-positioned.
- The show is saved when its song, offset or volume change (it is the file
  loaded from `shows/`); a show loaded inline through the API with a
  valid name is written too.
- A project (`.lsproj`) names the song file but doesn't contain it: on
  another Mac the song is missing (status « morceau introuvable », the
  timeline runs on the system clock).
- Output hot-plug / default-device change is detected through cpal's error
  callback only (then reopened every 2 s); a device that goes quiet without
  an error falls back to the system clock after 0.5 s but isn't reopened.
- Not tested: a real 3-minute listen with the laser (the drift test is a
  simulation; the real output was only checked at volume 0 for 2 s).

## Review

Reviewed by the architect (integrator). Clean merge on 2426fa6; 542 unit
+ 2 signal tests, e2e 144/144, clippy clean. Licences OK (symphonia
avoided: MPL-2.0; hound/claxon Apache-2.0, nanomp3 MIT/Apache). Callback
real-time safe (no allocation, try_lock only); audio never touches the
gate; song paths confined. Accepted: native playback, song as the clock
with system-clock fallback, `--no-audio` disables it. Filed T-298: a
nanomp3 panic would hit the global panic hook and stop the whole studio —
decoding must be isolated before relying on MP3 in a show.
Verdict: APPROVED
