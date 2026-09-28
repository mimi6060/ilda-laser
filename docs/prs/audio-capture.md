# feat/audio-capture — T-230 Native audio capture (cpal, CoreAudio)

## What / why
Until now the music analysis ran in the browser tab and was POSTed to
`/api/audio`: hide or close the tab and the laser stops following the
music (throttled timers, `AUDIO_STALE` 500 ms). The studio now listens to
a Mac input itself, on its own threads, so reactivity no longer depends
on a page. The browser path is kept, as the *Navigateur* source and as a
fallback.

New module `studio/src/audio/`:

- `capture.rs`: the real-time side.
  - `Sink::push<T>` is the whole audio callback: it averages the channels
    of each frame to mono and pushes into an `rtrb` SPSC ring (1 s at the
    stream rate). **No allocation, no lock, no log, never blocks**: a full
    ring drops the newest samples and counts an overrun (`dropped`,
    `overruns` atomics). The first callback's `Instant` (relative to the
    studio epoch = the tempo clock's time base) is stored in an atomic, so
    analysis timestamps are `first + (samples read + dropped) / rate`.
  - `ToF32` converts f32 / i16 / i32 / u16.
  - `SampleSource` trait (`devices()`, `open(req, make_sink)`):
    `CpalSource` (CoreAudio through cpal: 48 kHz when the device has it,
    f32 preferred, `BufferSize::Fixed(256)` when in the device's range,
    else the default; retried with the default buffer if the fixed one is
    refused) and, in tests, `testing::FakeSource`, fed synthetic signals
    through the real `Sink` path. No test opens a real device.
  - The backend's error callback only sets flags: `Xrun` → overrun count,
    `RealtimeDenied` → ignored, anything else (device gone, route changed,
    invalidated) → `failed`.
- `worker.rs`: two threads, neither ever takes the `Shared` lock.
  - **audio-capture**: every 50 ms applies config changes (generation
    counter), scans devices every 2 s (the list served by
    `GET /api/audio/devices` is this cache: the HTTP thread never calls
    CoreAudio), and closes the stream when the backend reports it lost,
    when no callback came for 1 s (5 s grace for the first one), or when a
    named device vanished from the list. It retries every 2 s → unplug +
    replug resumes on its own (≈2–2.1 s after the device is back). It owns
    the cpal stream (not `Send` on every platform). Status changes are
    logged once, not on every retry.
  - **audio-analysis**: receives each new stream's ring via a channel,
    reads hops of 256 samples, runs `analysis::Analyzer` and publishes a
    `NativeSnapshot` in the hub. The beat counter carries over when the
    stream is reopened (no fake beat).
- `analysis.rs`: RMS (τ 40 ms) and peak (20 dB/s fall-back) in dBFS,
  plus provisional `level` / `bass` / `beat` computed like the browser
  (level = RMS × 6; bass = 150 Hz low-pass biquad, −60..−10 dBFS → 0..1;
  beat = bass > 1.35 × its average, > 0.12, 200 ms refractory, plus a
  re-arm hysteresis so one kick's decay isn't a second beat). This keeps
  existing looks reacting with the native source until T-231/T-232
  replace it with bands, auto-gain and real onsets.
- `mod.rs`: `AudioInputSource { Native (default), Browser, None }`,
  `AudioConfig { source, device: Option<String>, buffer_frames: 256 }`
  (`#[serde(default)]`, saved in `<data-dir>/audio.json`, buffer clamped
  to 128..4096, not part of `.lsproj` projects: a device name is
  machine-specific), `CaptureStats`, `CaptureStatus`, and `AudioHub`, the
  meeting point (leaf mutexes held only for a copy). `effective()`
  decides what the engine gets each frame:
  - *Native*: the native snapshot if < 500 ms old, else the browser's
    features if fresh (fallback), else silence;
  - *Navigateur*: the browser's features exactly as before;
  - *Aucune*: silence.
  Stale features become silence but keep their beat counter.

Wiring:
- `main.rs`: `--audio-device <name|default|none>` (overrides audio.json
  for this run; `none` = source *Aucune*: no capture, no error loop) and
  `--no-audio` (no audio thread at all; state `disabled`). The engine's
  audio block now calls `s.audio_in.effective(...)`. Shutdown waits at
  most 500 ms for the audio threads.
- `web.rs`: `GET /api/audio/devices` → `{capture, devices:[{name,
  is_default}]}`; `GET /api/audio/config`; `POST /api/audio/config`
  (patch: only the fields sent change; 400 on a bad value; reply = what
  was kept); `/api/state.audio` = `{source, device, buffer_frames,
  capture, active, state, message, capturing, level_db, peak_db, t,
  stats{sample_rate, channels, overruns, rms_db, peak_db}, level, bass,
  beat}`. `POST /api/audio` is unchanged.
- e2e harness (`studio/e2e/studio.ts`) and `studio/tests/shutdown.rs`:
  always `--no-audio` (the harness refuses to start without it).

**Not done here (deliberately)**: the « Musique » panel UI (device list
with *Navigateur* / *Aucune*, *Écouter* / *Arrêter*, dBFS meter, status
text). `index.html` is being reorganised by another agent right now, so
this branch doesn't touch it; the API above is everything that panel
needs. Suggest folding it into T-242/T-243 or a small follow-up once the
reorganisation lands. Until then native capture is driven by
`audio.json` / `--audio-device` / the API, and the existing « Activer le
micro » button still works (browser source / fallback).

## Testing
- `cargo test -p laser-studio`: 476 passed, 3 ignored (one new ignored:
  `real_input_default_device`, real hardware, opt-in). New unit tests:
  - capture: stereo → mono mix, partial trailing frame, i16/i32/u16 →
    f32, full ring counts overruns and never blocks (keeps the oldest
    samples), **the callback does not allocate** (a thread-local counting
    `#[global_allocator]` in the test binary, covering first callback,
    i16 and f32 paths and the overrun path), backend faults are flags.
  - analysis: −6 dBFS peak / −9 dBFS RMS on a half-scale sine, silence
    = −120 floor, 20 dB/s peak fall-back, 50 Hz vs 5 kHz bass, 120 BPM
    kicks → 7–9 beats while a held bass note doesn't stream beats,
    NaN/inf ignored.
  - worker (fake source, simulated time): meter published with no
    browser at all (`Active::Native`), unplug → `device_lost`, stays lost
    and retries only every 2 s, replug → running again in < 5 s and
    analysis resumes; a device vanishing from the list without an error is
    closed; a stream without callbacks is reopened; `--audio-device none`
    opens nothing and logs its state once, switching to Native starts it,
    Browser closes it; no device / permission refused reported and
    retried; beat counter survives a reopen; the real threads start, fill
    the hub and stop cleanly with a fake source.
  - hub: config defaults / old files / sanitising, CLI override, patch,
    generation + save + reload, source selection (native wins, browser
    fallback, browser / none ignore native, stale keeps the beat),
    `/api/state.audio` view.
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `studio/e2e/tests/audio.spec.ts` (5 tests, API only): with
  `--no-audio` devices are empty and state `disabled`; Native without a
  capture → the browser features stand in and move the frame; *Aucune* →
  browser ignored, the look stops reacting (extent 0.25 with size 1);
  *Navigateur* → as before (bass 1 → extent 0.75, bass 0 → 0.25);
  validation, field-by-field patch, persistence across a restart. Full
  suite: 122 passed.
- Manual, preview only (port 8093, temp data dir, `--audio-device none`,
  no `--device`): `GET /api/audio/devices` listed the Mac's real inputs
  (« Micro MacBook Pro » default, « Serato Virtual Audio »); Ctrl+C exits
  cleanly. I did **not** open a real input (it would pop the macOS
  microphone prompt on the user's desktop).

## Licences (new crates in the macOS build)
Checked from each crate's `Cargo.toml` `license` field (2026-09-28,
`cargo tree -p laser-studio -e normal`); no GPL/AGPL anywhere in the
tree.

| Crate | Version | Licence |
|---|---|---|
| cpal | 0.18.2 | Apache-2.0 |
| rtrb | 0.4.0 | MIT OR Apache-2.0 |
| coreaudio-rs | 0.14.2 | MIT/Apache-2.0 |
| objc2-audio-toolbox, objc2-core-audio, objc2-core-audio-types, objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-foundation | 0.3.2 | MIT |
| dasp_sample | 0.11.0 | MIT OR Apache-2.0 |
| mach2 | 0.6.0 | BSD-2-Clause OR MIT OR Apache-2.0 |
| (already present: objc2, objc2-encode, block2, dispatch2, bitflags, libc) | | MIT / Zlib-Apache-MIT / MIT-Apache |

`Cargo.lock` also gains Android-only crates of cpal (jni, ndk, num_enum,
toml_edit…); they are not compiled on macOS.

## Risks
- **macOS microphone permission (TCC).** The first native capture makes
  macOS ask the *terminal app* that launched the studio for microphone
  access (Terminal, iTerm…). If refused (or launched in a way macOS can't
  attribute), CoreAudio often **delivers silence instead of an error**:
  the state says `running` with a −120 dBFS meter. An explicit refusal
  shows `permission_denied` with the Réglages Système path. Detecting the
  silent case (exact zeros for > 2 s) is T-242. `tccutil reset Microphone`
  re-asks. A packaged `.app` will need `NSMicrophoneUsageDescription`.
- **Behaviour change at start-up**: the default source is *Native* with
  the system default input, so an existing install starts listening to
  the Mac's microphone on its next launch (and gets the prompt). Before,
  nothing listened until « Activer le micro » was clicked. Use
  `--audio-device none` or `{"source": "none"}` in `audio.json` to keep
  it off. Worth confirming with the user.
- The native `bass`/`beat` are a port of the browser's heuristics, not
  identical numbers (Web Audio's byte spectrum can't be reproduced
  exactly): the same look may pump a little more or less with the native
  source until T-231's auto-gain.
- Switching source while music plays can give one extra flash / colour
  step (the beat counters of the two sources differ).
- Real hot-plug was only tested with the fake source. cpal 0.18 reports
  `DeviceNotAvailable` on macOS when a device disappears; the device-list
  scan and the 1 s no-callback watchdog cover a backend that stays quiet.
- A stream is dropped on the capture thread; if CoreAudio ever blocks
  there, only that thread waits (the engine, UI and e-stop don't), and
  the process exit doesn't wait more than 500 ms for it.
- Safety: audio never touches arming, disarming, the e-stop or the output
  gate; it only feeds `AudioFeatures`, which go through the layers, live
  stage, calibration and strobe limiter as before.

## Review
