# fix/decode-isolation — T-298 audio decoding can't stop the studio

Branch: `fix/decode-isolation` · Task: `tasks/T-298-infra.md`

## What / why

T-161 decodes the user's songs with `hound`, `claxon` and `nanomp3`
(machine-translated C, `unsafe` inside), on the HTTP worker (import,
waveform, attach) and on the song-playback thread. Since T-253 **any panic
stops the whole studio** (the global hook fires the kill switch and
shuts down). A crafted or damaged MP3 imported during a show would
therefore end the show. After this branch, a decoder that panics, aborts,
segfaults or hangs gives a French error (« Import refusé : … le studio
continue ») and nothing else happens: the laser stays armed, frames keep
flowing, and the global hook still stops everything for any other panic.

### Choice: a child process, with a contained thread as fallback

| | Child process (chosen) | Contained thread, panic joined as an error (fallback) |
|---|---|---|
| Panic | contained | contained |
| `abort`, segfault, stack overflow, UB / memory corruption in `unsafe` | contained (only the child dies) | **kills or corrupts the studio** |
| Hang / endless loop | killed at the time limit | can't be stopped (a thread can't be killed) |
| Memory blow-up | the child's own address space | the studio's |
| Cost | one process spawn per decode (~10 ms) + the samples copied over a pipe | none |

The whole point is `unsafe` machine-translated C, where the worst cases
are not panics, so the child process is the real isolation. The contained
thread only serves unit tests (the test harness is not the studio binary)
and the case where `current_exe()` fails (logged as an error).

### How

- `audio/isolate.rs` (new):
  - `laser-studio --decode <file> --data-dir <dir> [--decode-peaks]` (hidden
    flags, `main.rs`) runs `child_main` **before** anything else: no panic
    hook, no output, no port. It reads one file of `<data-dir>/media/audio/`,
    decodes it and writes the reply on stdout.
  - Confinement (`media::source_path`): the child gets a **name**, never a
    path. Accepted: a valid song name (`valid_song_name`: no `/`, no `..`)
    or an import in progress `.<song name>.part`. It must be a regular file
    (symlinks refused), ≤ 512 MB.
  - Reply format: `LSDECOD1`, then an error (UTF-8, ≤ 4 KB), the samples
    (rate, channels, frames, i16 LE), or only the waveform (rate, channels,
    frames, block, count, min[], max[]) for import and waveform requests,
    which don't need the samples.
  - `read_reply` checks every header field **before allocating**
    (1 000–384 000 Hz, 1–2 channels, ≤ 30 min, block = 256, count
    matching the frames, peaks in −1..1, nothing after the reply).
  - `run_child`: stdin null, stdout piped, stderr inherited (the child's
    panic message ends up in the studio's log). A reader thread parses the
    reply; the caller waits at most `DECODE_TIMEOUT` (**120 s**), then kills
    the child. Errors (French, shown after « Import refusé : »):
    - « le décodeur a planté sur ce fichier, abîmé ou piégé : fichier
      refusé, le studio continue » (exit 101 = panic);
    - « le décodeur s'est arrêté brutalement (signal N) … » (abort,
      segfault);
    - « décodage trop long (plus de N s) … » (killed);
    - « le décodeur n'a pas répondu (code N) … » (garbage / no reply);
    - the decoder's own error, as before (« fichier MP3 illisible : … »).
  - `Isolation::Thread`: a scoped thread named `decode`, flagged by a
    thread-local that the global panic hook reads
    (`panic_is_contained()`). A panic there is joined as an error. The
    hook skips `watchdog::on_panic` **only** for that thread; every other
    thread (engine, output, MIDI, capture, HTTP) is unchanged.
  - Test hooks (`--test-hooks` only; also passed on to the child): a file
    starting with `LSPANIC!` / `LSABORT!` / `LSHANG!!` makes `decode()`
    panic / abort the process / hang. Without the flag such a file is
    « format audio non reconnu ». `--test-decode-timeout-ms` (requires
    `--test-hooks`) shortens the limit for tests.
- `audio/media.rs`: `MediaStore` holds an `Isolation`
  (`MediaStore::new` = thread, `.isolated(Isolation::child(..))` in the
  studio). `decode` (playback) asks for samples, `peaks` for the waveform.
  **Import** changed order: dedup check → bytes written to `.<file>.part`
  → decoded by the child → renamed; on any failure the `.part` is
  removed, so a refused file still leaves nothing. An identical re-import
  is checked by decoding the existing file.
- `web.rs`: the three requests that may decode (`POST /api/media/audio`,
  `GET /api/timeline/waveform`, `POST /api/timeline/audio`) now run on
  their own thread (`decode_aside`, at most 4 at once, else 503). **Why:**
  the HTTP worker is single-threaded, and a decode that took more than 2 s
  (a hang up to the limit, or even a long MP3 in a debug build) held up the
  heartbeats queued behind it, so presence (T-252) disarmed the laser as
  « Interface perdue ». Everything else keeps its order on the worker.
- `main.rs`: the hidden flags, the early `--decode` exit, the hook
  exemption, `Isolation::child(data_dir, test_hooks, timeout)`.
- `index.html`: unchanged. The existing « Import refusé : <raison> » and
  the song status line (« error » + message) already show the French reason.

## Testing

- `cargo test -p laser-studio`: **566 passed, 6 ignored** + 2 shutdown +
  **2 new subprocess tests** (rebased on develop ad2e974).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- New unit tests:
  - `isolate.rs`:
    - replies round-trip (samples, peaks, error, a 5 000-char message cut
      on a char boundary);
    - malformed replies refused **before allocating**: empty, garbage,
      truncated, trailing bytes, wrong kind, `u64::MAX` frames, 0 Hz,
      3 channels, 31 min, 0 frames, a 4 GB error message, wrong
      block/count, a peak of 2.0, and 2 000 random replies;
    - `run_child` against `/bin/sh` stand-ins: SIGSEGV, SIGABRT,
      exit 101, garbage + exit 0, exit 3, `sleep 30` (killed at 300 ms,
      returns in < 3 s), magic then hang, a missing executable, an error
      reply passed on verbatim;
    - the contained thread: a `LSPANIC!` file → the French error, the
      decode thread is flagged and the caller's is not, other files still
      decode, `../` refused.
  - `media.rs`: a decoder panic refuses the import and leaves no song and
    no `.part`, then a good import still works; `source_path` accepts only
    songs and `.<song>.part` (not `.x.part`, `../`, `..a.wav.part`,
    `notes.txt`…), and `path()` never accepts a `.part`.
  - `decode.rs`: **stronger fuzz** `fuzzed_files_never_panic_the_decoders`:
    about 2 200 files: WAV / FLAC / AIFF / AIFF-C sowt / MP3 bases with
    random bytes **anywhere** (the old fuzz only touched the first 512
    bytes), bit flips, deleted runs, inserted runs, tails of other files
    spliced in; MPEG headers of every version × layer × bitrate × rate
    with random bodies (3 frames each); noise of 0–4 kB with and without
    an ID3 tag. Every success is checked against the 2-channel / 30-min
    limits. No panic found in the decoders.
  - `web.rs`: only the three decoding requests leave the worker.
- `studio/tests/decode_isolation.rs` (real binary, `--test-output` fake
  output, `--no-midi --no-audio --test-hooks`, a 2 s limit, a page beating
  every 300 ms):
  - **armed and lit**, import `LSPANIC!` → 400 « le décodeur a planté … le
    studio continue »; `LSABORT!` → 400 « arrêté brutalement (signal 6) ».
    After each: process alive, `/api/state` answers in < 1.5 s, still
    armed, the output log has no `disarm`/`blank`/`close` and ends with
    `lit`, `output_lit` > 0, `media/audio/` empty;
  - `LSHANG!!` → while it hangs, `/api/state` answers every 100 ms for
    1.5 s; then 400 « décodage trop long (plus de 2 s) », same checks;
  - a real WAV then imports (200) and its waveform works;
  - the `--decode` child alone: `../secret.wav`, `/etc/passwd`, `.peaks`,
    a symlink, a missing file → an error reply (exit 0), a song → samples
    (exact size), `--decode-peaks` → peaks, a `LSPANIC!` file without
    `--test-hooks` → « format audio non reconnu »; `--decode-peaks` alone
    and `--test-decode-timeout-ms` without `--test-hooks` are refused.
- e2e (`npm --prefix studio/e2e test`): **146 passed**. New
  `tests/decode-isolation.spec.ts` (studio with `testHooks`,
  `decodeTimeoutMs: 2000`, new harness option): armed from the page, then
  `piège.mp3` (panic), `abort.mp3` (abort), `lent.mp3` (hang) imported from
  the TIMELINE workspace → « Import refusé : … le studio continue » with
  the reason, in < 8 s, still armed, frames still drawn, nothing written;
  then a real WAV imports; disarm at the end is not `ui_lost`. The existing
  `timeline-audio.spec.ts` now goes through the child for every import and
  waveform.

## Risks

- **A process per decode.** Import, waveform (not cached) and song load
  each spawn `laser-studio --decode`. Cost ~10 ms (8 ms measured, debug
  build) plus the samples piped back (for playback: ~10 MB per stereo
  minute; a 30 min song ≈ 320 MB copied once). The waveform path sends
  only peaks.
- **The binary must still be there.** If the executable is replaced
  while the studio runs (a rebuild), the child is the *new* binary. Same
  reply format, so harmless within this branch, but a format change needs
  the studio restarted. If it was deleted, decoding fails with « le décodeur
  n'a pas répondu … » / « impossible de lancer le décodeur ».
- **120 s limit.** A 30 min MP3 decodes in a few seconds in release; a
  debug build is much slower and could, in theory, get close. A killed
  decode is an error, never a crash. The limit is not user-configurable.
- **No memory / CPU rlimit on the child** (no `libc` dependency added; macOS
  doesn't enforce `RLIMIT_AS` well anyway). The child's memory is bounded
  by the decoder's own 30 min × 2 channel limit, as before, but it's the
  child's, and it dies with its reply.
- **Shutdown during a decode:** the studio doesn't wait for decoder
  children. If it exits mid-decode, the child finishes its file (≤ the time
  limit), fails to write to the closed pipe, and exits.
- Import now writes `.<file>.part` **before** decoding (the child can only
  read files of the folder). It is removed on any failure; a studio killed
  mid-import can leave one, which is never listed or served (not a valid
  song name).
- Media requests now run beside the HTTP worker (up to 4 at once), so
  they may complete out of order with later requests. `attach_song` still
  mutates the show under the lock; only its decoding moved.
- Thread fallback: catches panics only. It is logged as an error when
  used; in normal use it never is.
- `--test-hooks` now also enables the three crash magics in the decoder.
  Test-only; without the flag they are plain unknown files.

## Review

Reviewed by the architect (integrator). Clean merge on ad2e974; 566 unit
+ shutdown + 2 isolation subprocess tests, e2e 146/146, clippy clean.
Checked: the global panic hook skips shutdown only for the contained
decode thread (thread-local flag set only in `isolate::contained`); the
child gets a name, never a path; reply sizes validated before allocation;
every child failure returns within the limit. Good catch on decodes
blocking heartbeats behind the single HTTP worker: those requests now run
on their own threads (≤ 4). Accepted risks listed in the note.
Verdict: APPROVED
