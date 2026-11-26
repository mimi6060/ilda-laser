# feat/apc-leds — T-205 LED feedback on the APC40 / APC40 mkII

## What / why
Without LEDs the operator plays blind. The APC now shows which pads hold a
cue, which one plays, the cue page, the layer buttons, the beat and a
latched emergency stop — computed from `Shared` in the MIDI thread, so it
keeps working with the browser tab closed.

- `studio/src/midi/led.rs` (new):
  - `LedFrame(BTreeMap<(status, note|cc), value>)` — the desired state of
    every LED the studio drives on one device; `diff(last)` gives only the
    messages that changed (an LED no longer driven is switched off).
  - `LedState { last_sent, next_at }` (the task's `DeviceState`), kept per
    device by the worker: `due(now)` / `update(frame, now)` (≤ 1 update
    per `LED_EVERY` = 33 ms, i.e. ≤ 30/s) / `forget()` (after an
    Introduction or a reconnect: the next update resends everything).
  - `render(driver, &Shared, port, t)`: instead of a trait with one impl
    per model, one function with the model differences in two small
    tables (`kind_of`, `velocity`). **What lights is read from the
    device's profile**, so the built-in layouts and anything learned later
    (T-203) light the same way:
    - grid pad (mode `grid`): empty = off, cue present = **yellow 5**
      (mkII **white 3**), playing = **green 1** (mkII **green 21**), playing
      while held (flash/solo) = **green blink 2** (mkII: green 21 / dim
      green 22 alternating every half beat of the tempo clock);
    - `page.N`: lit (1 / mkII green 21) on the current page; a Shift
      mapping lights its button as « alternate »: page 6–8 on Scene Launch
      1–3 = **blink 2** (mkII **orange 9**);
    - `layer.N.mute` (Activator): lit = layer heard; `layer.N.clear` (Clip
      Stop): lit while that layer plays a cue; any toggle / momentary
      control (Solo, Record Arm modifiers, `timeline.loop`…): its value;
      `timeline.toggle` / `.play` (mkII Play): lit while the timeline plays;
    - knob rings (absolute mappings on CC `0x30`–`0x37`, `0x10`–`0x17`):
      the control's value (range + curve inverted, 0–127) and ring type
      (CC + 8): « pan » (3) when the range straddles 0 (position, angles),
      « volume » (2) otherwise;
    - **Metronome** (mkII `0x5A`, APC40 device button 8 `0x41`): on for
      the first 1/8 of each beat of the single tempo clock (T-150), at
      least 70 ms so a 30 Hz refresh can't miss it;
    - **E-stop latched**: the 8 Clip Stop LEDs blink until « Réinitialiser »
      (APC40: device blink 2; mkII: 2 Hz by hand, its own blink follows a
      MIDI clock we don't send).
    - Tap, Shift, arrows, Nudge, Stop All, Bank, the APC40's transport:
      never addressed (no LED). A per-track button mapped on « any
      channel » is skipped (we can't know which track LED it is).
- `studio/src/midi/worker.rs`: `update_leds` runs at the end of every
  step: for each introduced APC whose update is due, **one** `Shared` lock
  renders every frame, the lock is dropped, then the diffs are sent.
  The wait for MIDI events is capped at the next LED update so a change
  (UI, keyboard, HTTP, MIDI) reaches the pads within ~33 ms. LEDs start
  only after the Introduction; a (re)plug or new Introduction resends the
  full frame. `goodbye` (quit, disable, profile switch) now also zeroes
  what we lit — including knob rings — before the existing all-off and
  Introduction `0x40`.
- « Retour LED » per device (on by default): `PortPrefs.leds` in
  `devices.json`, `ProfileStore::port_leds/set_port_leds`,
  `MidiDevice.leds` in `/api/midi`, `POST /api/midi/device` now takes
  `enabled` and/or `leds` (either optional, neither = 400). Unticking
  switches off what was lit and stops all LED traffic (beat included).
- `studio/src/index.html`: **two lines** — a « Retour LED » checkbox under
  « Activé » in each device of the Contrôleur section, and its handler.
- Small visibility changes: `controls::show_cue_playing` and
  `midi::engine::range` are `pub(crate)` / `pub(super)` (reused, not
  duplicated).

Not done (not in the state yet, or T-206): « cue selected in the UI but
not playing = red » (the selection is UI-only), Pan/Send knob banks and
Track Select (no control ids yet, T-204 left them free), per-cue colours
on the mkII (T-206), sending MIDI clock so the mkII's own blink/pulse
follow our tempo.

## Safety / performance
- Nothing here writes to `Shared`: rendering is read-only, and the LED
  path can't arm, disarm or change a control.
- CoreMIDI is never called with the `Shared` lock held: the worker's fake
  backend now asserts `try_lock()` succeeds on every send
  (`led_setup`-based tests). The engine only ever waits for one render
  (a scan of the profile's ~100 mappings + 40 pads, microseconds), at most
  30 times a second per device.
- The e-stop stays visible on the controller itself.

## Testing
- `cargo test -p laser-studio`: **465 passed** (+ 2 shutdown integration tests), 2 ignored (CoreMIDI
  virtual ports, not run: they create ports other apps can see). New:
  - `led.rs` (11): APC40 pads empty / present yellow / playing green /
    flash blink, old pad back to yellow when the next cue starts; mkII
    white → green, flash alternating on the beat; Scene Launch page 3 lit,
    page 7 → Scene 2 blinking (mkII orange); Activator/Solo/Clip Stop/
    Record Arm on both models, no LED for Shift/Stop All/Tap; metronome
    at 120 and 60 BPM and after « Recaler sur le 1 »; e-stop row blink on
    both models; rings value + type, value changed by the UI; diff (idle =
    nothing, a cue = its pad + Clip Stop, page change = grid + scenes
    only, full resend after `forget`); dropped LEDs switched off; ≤ 30
    updates in 1 s; learned mappings light, generic devices get nothing.
  - `worker.rs` (4): full frame right after the Introduction, a cue
    started from the UI → only its diff at the next update (not before
    33 ms), page change, **2 s idle = only metronome messages**; « Retour
    LED » off → lit LEDs go dark then zero traffic; replug resends
    everything, shutdown clears the rings then gives the device back;
    a generic device gets nothing but the inquiry. Every send checks the
    `Shared` lock is free.
  - `testing.rs` (simulated devices, `led_at`): mkII pad (0,1) green after
    the press, (0,0) white, back to white after the second press; APC40
    pad yellow → green, and the first pad yellow again when another cue
    starts.
  - `api.rs`: `leds` partial update, default on, empty request 400.
  - Two existing worker tests now compare only SysEx (LEDs follow the
    Introduction in the same step).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e `npm --prefix studio/e2e test` (on develop ccaed89): **118 passed**
  with `--workers=2`. With the default 4 workers the machine was loaded
  (other agents building, load ~5–6) and the *first* test of one or two
  spec files (content, cues, apc40-profile — never the same) timed out in
  `openUi` waiting for the UI to boot; the same flakes happen with
  develop's `index.html` swapped in, so they are not from this branch.
  The MIDI specs (`midi`, `apc40-profile`, `midi-learn`) passed
  `--repeat-each 3` (54/54). `midi.spec.ts`: T-209's placeholder `ledAt(0,1) === null` is now
  a real colour check (green 21 while playing, white 3 once stopped, and a
  cue started through `/api/control` lights its pad); new test « Retour
  LED » unticked in the UI → pad dark, `/api/midi` `leds:false`, ticked →
  lit again.

## What you should see on the real APC40 mkII
1. Plug it in with the studio running: within a second the pads of the
   current page turn **white** where a cue exists (dark where none), Scene
   Launch 1 is **green**, Activators 1–4 are lit, the **Metronome** button
   flashes on every beat.
2. Press a pad: it turns **green**; press another: the new one is green,
   the old one white again. Clip Stop 1 is lit while layer 1 plays.
3. Click a cue or a page tab in the UI (or close the tab and use the APC):
   the pads follow within a few hundredths of a second.
4. Scene Launch 3 → page 3: the grid redraws, Scene 3 green. Shift + Scene
   Launch 1 → page 6: Scene 1 **orange**.
5. Activator 2 (mute layer 2): its LED goes off; Solo 3: lit; Record Arm 5
   (audio reaction) follows the checkbox in the UI.
6. Tap tempo: the Metronome LED follows the new tempo.
7. Hold a pad whose cue is in Flash mode: green / dim green alternating in
   time.
8. Top knobs: the rings show the values (Position Y centred « pan » ring,
   the others as a bar) and move when you change them in the UI.
9. Stop All: the 8 Clip Stop LEDs blink until « Réinitialiser ».
10. Untick « Retour LED » in Contrôleur: the APC goes dark (knobs and pads
    still work); tick it again: everything comes back. Quit (Ctrl-C):
    everything goes dark, rings included.

## Risks / for the reviewer
1. mkII colours are T-204/T-205 placeholders (white 3, green 21, dim green
   22, orange 9); T-206 replaces them with per-cue colours.
2. mkII blinking is done by hand (two messages per half-beat for a held
   flash pad, per 250 ms for the e-stop row); the device's own blink
   needs MIDI clock, not sent. The APC40 (mk1) uses its own blink (2).
3. At rest the Metronome LED is the only traffic (2 messages per beat): the
   criterion « no LED message at rest » holds for every other LED.
4. Ring echo: in mode `0x41` the device drives its rings itself; we also
   send the value back after a knob move (same value, harmless), and a
   knob in pickup shows the real value, not the knob's position.
5. The Activator shows « layer heard » (lit = not muted), the Ableton
   convention, for any `layer.N.mute` mapping.
6. Two lines in `index.html` while another agent reorganises it: a trivial
   merge conflict at worst.

## Review

Reviewed by the architect (integrator). Merged after audio-capture;
492 unit + 2 signal tests, e2e 123/123 twice (default workers), clippy
clean. Checked: CoreMIDI never called under the shared lock (asserted on
every send), diff-only updates ≤ 30 Hz, LEDs off on quit/disable, LED
state derived from each profile so learned controls light too. The
earlier 4-worker flakiness did not reproduce here. Hardware checks on the
user's APC40 mkII listed above.
Verdict: APPROVED
