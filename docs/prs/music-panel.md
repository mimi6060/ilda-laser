# feat/music-panel — T-243 « Musique » panel v2 (LIVE › Musique)

## What / why
T-230 to T-237 made the studio hear the music natively (bands, onsets,
tempo, sections) but the UI still showed two browser meters. The
operator now sees at a glance what the studio hears and how sure it is,
and can choose the source without touching `audio.json`.

`studio/src/index.html`, LIVE › Musique (same tab, same ids kept for
`#micBtn`, `#micSelect`, `#gain`, `#beatDot`, `#aOn`…; `#mLevel` /
`#mBass` removed, replaced by the new meters):

- **Source**: segmented *Navigateur / Native / Aucune*.
  - *Navigateur* (default, unchanged): the page's micro, analysed by the
    page and POSTed to `/api/audio` exactly as before; « Écouter » /
    « Arrêter » is the old « Activer / Couper le micro ». Its gain slider
    is now « Gain du navigateur » in *Réglages de l'analyse*.
  - *Native*: picking it **opens nothing**; it shows a note saying that
    the first « Écouter » makes **macOS ask for microphone permission**
    for the app that launched the studio (Terminal, iTerm…), and that the
    default stays *Navigateur*. The Mac's inputs come from
    `GET /api/audio/devices` (« Entrée par défaut du Mac » + the list,
    « (par défaut) » / « (absente) » marks). « Écouter » posts
    `{source: native, device}`; « Arrêter » posts `{source: browser}`
    (nothing listens any more; *Native* stays picked in the panel).
    Choosing Native or Aucune stops the page's own analysis (no browser
    analysis while the source is Native).
  - *Aucune*: posts `{source: none}` at once.
  - A source changed elsewhere (API, another page) is followed.
- **Status line**: capture device + rate, the server's French message
  (no device, device lost, refused…), `--no-audio`.
- **Permission hint**: native source, capture `running` and the meter
  exactly at the floor (−120 dBFS, i.e. digital zeros) for > 2 s → a
  warning with the path Réglages Système › Confidentialité et sécurité ›
  Microphone and `tccutil reset Microphone`; also shown at once on
  `permission_denied`. (Client-side, until T-242 classifies input health
  on the server.)
- **Analyse** (`<details>`, open by default; closing it stops the
  spectrum): level in dBFS (−60..0 scale, −25..0 target zone marked,
  peak tick, red at ≥ −0.5 dBFS peak), the five bands *Sub, Basses,
  Bas-médiums, Médiums, Aigus*, the **64-band spectrum** on a canvas
  (native: `GET /api/audio/spectrum`; Navigateur with the page's micro on:
  the page's own analyser on the same log scale; else a short note),
  *Kick / Caisse / Charleston* lights (flash when their counter moves),
  *BPM détecté*, *Confiance* gauge, detector state (*Pas d'entrée,
  Vérification, Verrouillé, Maintien, Guidé*; « estimation native
  seulement » for the browser), *Section* (*Silence, Normal, Break,
  Montée, Drop* + seconds since it started), *Montée* gauge, drop count.
- **Réglages de l'analyse** (`<details>`): *Gain automatique*, *Gain
  manuel* (−40..+40 dB, enabled when auto is off), *Seuil de silence*
  (−100..−20 dBFS), *Sensibilité des coups* 0..100 % mapped
  logarithmically onto `analysis.onsets.delta` 0.3..0.03 (default 0.1 =
  48 %). Posted to `/api/audio/config` on `change`; never reopens the
  input (T-231).
- **« Nouveau morceau »**: the existing `tempo.new_track` control
  (`data-ctl`, MIDI-learnable).
- **Polling**: one loop, only while the panel is on screen (LIVE +
  Musique tab + page visible): `GET /api/audio/state` 10/s (6.7/s while
  the spectrum is polled), `GET /api/audio/spectrum` 20/s (native only),
  devices every 5 s with *Native* picked → ≤ 30 requests/s, **none** while
  hidden. The preview's `/api/frame` loop is untouched.

Server (`web.rs`, small, for the panel only):
- `GET /api/audio/state`: `/api/state.audio` alone (same `AudioHub::view`),
  so the panel doesn't pull the whole state 10×/s.
- `POST /api/test/native_audio` (**`--test-hooks` only**, 404 otherwise):
  a simulated native input for e2e: capture `state` / `device` /
  `message` (`set_status`), `devices` (`set_devices`) and/or one analysis
  `snapshot` (`features` read like `POST /api/audio`'s body, `level_db`,
  `peak_db`, `spectrum_db`, `tempo_state`, `section_since_s`), fresh
  for 500 ms like a real one.
- `CaptureState` and `InputDevice` derive `Deserialize` (for that hook).
No behaviour change otherwise; no `Settings` / `audio.json` change.

## Testing
- `cargo test -p laser-studio`: 643 passed, 7 ignored (+2 +2
  integration). New `web::tests::the_native_audio_hook_exists_only_with_test_hooks`
  (404 without hooks; state/devices/snapshot reach `/api/audio/state`,
  `/api/audio/devices` and `/api/audio/spectrum`; state-only post keeps
  the device; 400 on bad values; never arms).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `studio/e2e/tests/music-panel.spec.ts` (7 tests, `--no-audio`,
  `--test-hooks`, no page micro ever opened, no console error in any):
  source choice (Navigateur default, Native note + no config change until
  « Écouter », device list and chosen device posted, Arrêter → browser,
  Aucune, external change followed); meters/lights/section/BPM following
  a simulated `POST /api/audio`; a native snapshot → dBFS text, band
  heights, 64 spectrum bars with the peak in the right band, three
  lights, BPM / 80 % / Verrouillé, « Montée · 6 s », 40 %, 1 drop;
  exact-zero input → hint after 2 s (not before), gone when sound
  returns, immediate on `permission_denied`; analysis settings and
  « Nouveau morceau »; panel hidden (other LIVE tab + page
  `visibilityState` hidden) → zero audio polls while the laser keeps
  following the native snapshot (`/api/frame` extent > 0.7, then back
  < 0.3), display resumes when shown; ≤ 31 polls/s measured while
  visible; layout at 1280 px (nothing overflows the panel, no page
  scroll).
- Full suite: see the final run in the task Journal.
- Looked at a Playwright screenshot of the panel at 1280×900 (native,
  simulated snapshot).

## Risks
- The permission hint is client-side and only looks at the RMS floor
  (−120 dBFS = exact zeros); T-242 should move the classification to the
  server (`InputHealth`) and the panel then just shows it.
- « Arrêter » on Native switches the server source back to *Navigateur*
  (nothing listening), not to *Aucune*: the laser would follow the page's
  micro if it were re-enabled. Deliberate (the default source), easy to
  change.
- Browser spectrum uses the page's analyser dB scale, not identical to
  the native one (display only).
- Kick/snare/hat lights sample counters at 6.7–10 Hz: two kicks within
  one poll flash once.
- The new test hook is behind `--test-hooks` (hidden flag, never used by
  the user's instance).
- Safety: unchanged. The panel only changes the audio source/analysis
  config and posts `tempo.new_track`; no arming, no output path.

## Review

Reviewed by the architect (integrator). Merged after audio-shaping (index
only); 661 unit + integration tests, clippy clean, e2e 173/173 with 2
workers. Checked: Navigateur stays the default, the Mac mic is only opened
after an explicit « Écouter », the test hook is 404 without --test-hooks,
polling only while visible (≤ 30 req/s), never arms. Follow-up: move the
silent-permission heuristic server-side (T-242).
Verdict: APPROVED
