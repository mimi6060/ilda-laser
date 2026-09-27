# Research: MIDI control with the Akai APC40 / APC40 mkII

*Research agent report. Date checked: 2026-09-27. Public information only:
Akai's official "Communications Protocol" PDFs, the Showcontroller LIVE manual,
Pangolin wiki/forum pages, crate docs and the Chrome developer blog. No
Pangolin or Laserworld software was downloaded, run or inspected. Their
default layouts are described here only as inspiration; our default layout
(§5) is our own design.*

Tasks written from this report: **T-200 to T-210** (`tasks/`).

---

## 0. TL;DR

- **The model matters.** The original **APC40** (2009, product id `0x73`) and
  the **APC40 mkII** (2015, product id `0x29`) share most CC/note numbers for
  faders, knobs and transport, but the **clip grid is addressed differently**
  and the **LEDs are different** (mk1: green/red/yellow + blink; mkII: 128-colour
  RGB palette with pulse/blink modes). We ship **two profiles** and pick one
  automatically (port name + SysEx Device Inquiry).
- To get host-controlled LEDs and have every button send MIDI, the host must
  send the **Introduction SysEx** `F0 47 7F <pid> 60 00 04 <mode> 01 00 00 F7`
  with `<mode>` = `0x41` (Ableton Live mode, recommended) or `0x42`
  (Alternate Ableton mode, host also drives the knob rings). Without it the
  device stays in **Generic mode** (`0x40`): buttons toggle locally and the
  track-select buttons send nothing.
- **Architecture: native MIDI in Rust (`midir` 0.11, CoreMIDI), not Web MIDI.**
  The laser must keep responding with the tab closed; LED feedback must follow
  engine state, not UI state; Safari has no Web MIDI; Chrome ≥ 124 prompts for
  *all* MIDI access; background tabs are throttled; headless e2e tests cannot
  easily grant MIDI permission. The browser only shows MIDI state and drives
  **MIDI learn** through the HTTP API.
- **MIDI learn**: right-click any control carrying a control id (T-145) →
  "Apprendre MIDI" → move a hardware control → mapping saved to
  `studio-data/midi/`. Profiles per device, a built-in default profile per APC
  model, user copies override it.
- **Safety**: MIDI can always blackout; arming the laser from MIDI is **off by
  default** (opt-in, Shift + hold); absolute faders use **pickup (soft
  takeover)** so a fader left at 100 % can never jump the brightness.

---

## 1. APC40 (original) — MIDI implementation

Source: *Generic Communication Protocol for Akai APC40 Controller*, Rev 1,
1 May 2009 (Akai Professional, official PDF [1]).

### 1.1 SysEx and modes

| Item | Bytes |
|---|---|
| General SysEx | `F0 47 <dev=7F> 73 <msg> <lenMS> <lenLS> <data…> F7` (Akai = `0x47`, APC40 = `0x73`) |
| Device Inquiry (host → device) | `F0 7E 00 06 01 F7` |
| Inquiry response | `F0 7E <ch> 06 02 47 73 00 19 <ver×4> <dev> <serial×4> <manuf×16> F7` |
| Introduction (host → device) | `F0 47 7F 73 60 00 04 <mode> <verHi> <verLo> <bugfix> F7` |

Modes (`<mode>` byte), the unit starts in mode 0:

| Mode | Id | Behaviour |
|---|---|---|
| 0 Generic | `0x40` | Clip launch/stop momentary, lit locally when held. Activator/Solo/Rec-arm toggle locally. **Track Select 1–8 + Master are radio buttons that send no MIDI**; they bank the 8 device knobs/buttons onto MIDI channel 0–8. Rings all "single". |
| 1 Ableton Live | `0x41` | **All buttons momentary**, device knobs not banked, knob rings driven by the device (host may update), **all other LEDs driven by the host**. |
| 2 Alternate Ableton Live | `0x42` | Same as 1 but **all LEDs, rings included, driven by the host**. |

### 1.2 Inbound messages (device → host)

Buttons send Note On (vel `0x7F`) on press and Note Off on release. For notes
`0x30`–`0x39` the **channel is the track (0–7)**; other notes ignore the channel.

| Control | Message | Notes |
|---|---|---|
| **Clip launch grid 5 × 8** | Note `0x35`…`0x39` (rows 1–5, top → bottom), **channel 0–7 = column** | e.g. top-left = `90 35 7F`, bottom-right = `97 39 7F` |
| Clip stop 1–8 | Note `0x34`, ch 0–7 | |
| Track select 1–8 | Note `0x33`, ch 0–7 | mode 1/2 only |
| Activator 1–8 | Note `0x32`, ch 0–7 | |
| Solo / Cue 1–8 | Note `0x31`, ch 0–7 | |
| Record arm 1–8 | Note `0x30`, ch 0–7 | |
| Master (track select) | Note `0x50` | |
| Stop all clips | Note `0x51` | |
| Scene launch 1–5 | Notes `0x52`…`0x56` | |
| Device buttons 1–8 (Clip/Track, Device on/off, ◀, ▶, Detail view, Rec quant, MIDI overdub, Metronome) | Notes `0x3A`…`0x41` | ch 0–8 in mode 0 (banked), ch 0 in modes 1/2 |
| Pan / Send A / Send B / Send C | Notes `0x57`…`0x5A` | |
| Play / Stop / Record | Notes `0x5B` / `0x5C` / `0x5D` | |
| Up / Down / Right / Left | Notes `0x5E` / `0x5F` / `0x60` / `0x61` | |
| Shift | Note `0x62` | |
| Tap tempo | Note `0x63` | |
| **Nudge + / Nudge −** | Notes **`0x64` / `0x65`** | ⚠ reversed on the mkII |
| Track faders 1–8 | CC `0x07`, ch 0–7, absolute 0–127 | |
| Master fader | CC `0x0E` | |
| Crossfader | CC `0x0F` | |
| Device knobs 1–8 | CC `0x10`…`0x17` | ch 0–8 in mode 0, ch 0 in modes 1/2 |
| Track-control knobs 1–8 (top row) | CC `0x30`…`0x37` | absolute |
| Cue level | CC `0x2F`, **relative** | 1–63 = +n, 127…64 = −1…−64 |
| Footswitch 1 / 2 | CC `0x40` / `0x43` | 127 pressed, 0 released |

### 1.3 Outbound (host → device): LEDs and rings

LEDs are set with Note On (Note Off = off). Velocities:

| LED | Note, channel | Velocity |
|---|---|---|
| Clip launch (grid) | `0x35`…`0x39`, ch = column | 0 off, **1 green, 2 green blink, 3 red, 4 red blink, 5 yellow, 6 yellow blink**, 7–127 green |
| Clip stop | `0x34`, ch | 0 off, 1 on, 2 blink |
| Scene launch 1–5 | `0x52`…`0x56` | 0 off, 1 on, 2 blink |
| Rec arm / Solo / Activator / Track select | `0x30`…`0x33`, ch | 0 off, 1–127 on |
| Device buttons 1–8 | `0x3A`…`0x41` | 0 off, 1–127 on |
| Master, Pan, Send A–C | `0x50`, `0x57`…`0x5A` | 0 off, 1–127 on |
| Play/Stop/Rec, arrows, Shift, Tap, Nudge, Stop All | — | **no LED control** |

Knob rings: send CC with the control id to set the value
(`0x10`…`0x17`, `0x30`…`0x37`); the ring **type** is set with CC
`0x18`…`0x1F` (device knobs) and `0x38`…`0x3F` (track knobs):
0 off, 1 single, 2 volume (bar), 3 pan (centre). Fader positions cannot be
set (no motors). Blinking on the mk1 runs at the device's own rate.

---

## 2. APC40 mkII — MIDI implementation

Source: *Akai APC40 Mk2 Communications Protocol*, Version 1.2, 19 January 2015
(official PDF [2]).

### 2.1 SysEx and modes

Same frame as the mk1 with **product id `0x29`**:

- Device Inquiry `F0 7E 7F 06 01 F7` → response `F0 7E <ch> 06 02 47 29 00 19 … F7`.
- Introduction `F0 47 7F 29 60 00 04 <mode> <verHi> <verLo> <bugfix> F7`,
  modes `0x40` / `0x41` / `0x42` exactly as the mk1 (§1.1).
- The device answers the Introduction with message type `0x61` carrying the
  **current positions of the 9 faders**, which is exactly what pickup (soft
  takeover) needs at start-up.

### 2.2 Inbound messages — differences from the mk1

| Control | mkII message | Differs from mk1? |
|---|---|---|
| **Clip launch grid 5 × 8** | **Notes `0x00`…`0x27`**, channel ignored. Numbered **from the bottom-left**: bottom row `0x00`–`0x07`, top row `0x20`–`0x27` (note = 32 + column − 8 × row-from-top; confirmed by Ableton's APC40_MkII remote script [6]) | **yes** |
| Clip stop / Track select / Activator / Solo / Rec arm | Notes `0x34` / `0x33` / `0x32` / `0x31` / `0x30`, ch 0–7 | same |
| Crossfader A/B 1–8 | Note `0x42`, ch 0–7 | new |
| Device ◀ ▶, Bank ◀ ▶, Device on/off, Device lock, Clip/Device view, Detail view | Notes `0x3A`…`0x41` | relabelled |
| Master / Stop all clips | `0x50` / `0x51` | same |
| Scene launch 1–5 | `0x52`…`0x56` | same |
| Pan / Sends / User / Metronome | `0x57` / `0x58` / `0x59` / `0x5A` | relabelled |
| Play / Record / Session | `0x5B` / `0x5D` / `0x66` | no physical Stop (doc still lists `0x5C`) |
| Up / Down / Right / Left, Shift, Tap tempo | `0x5E`…`0x61`, `0x62`, `0x63` | same |
| **Nudge − / Nudge +** | **`0x64` / `0x65`** | **reversed** vs mk1 |
| Bank (lock) | `0x67` | new |
| Track faders, Master, Crossfader | CC `0x07` ch 0–7, `0x0E`, `0x0F` | same |
| Device knobs 1–8 | CC `0x10`…`0x17` | same |
| Top knobs 1–8 | CC `0x30`…`0x37` | same |
| **Tempo knob** | CC `0x0D`, **relative** | new (the mk1 has none) |
| Cue level | CC `0x2F`, relative | same |
| Footswitch | CC `0x40` | one only |

### 2.3 Outbound: RGB pads and other LEDs

- **Clip grid (`0x00`–`0x27`) and scene launch (`0x52`–`0x56`) are RGB.**
  Velocity = colour index 0–127 (palette in §2.4). **Channel = LED mode**:

  | Ch | Mode | Ch | Mode | Ch | Mode |
  |---|---|---|---|---|---|
  | 0 | primary colour (solid) | 6–10 | secondary colour, **pulsing** 1/24, 1/16, 1/8, 1/4, 1/2 | 11–15 | secondary colour, **blinking** 1/24, 1/16, 1/8, 1/4, 1/2 |
  | 1–5 | secondary colour, one-shot 1/24 … 1/2 | | | | |

  To pulse/blink, send the primary colour on ch 0, then the secondary colour
  on ch 6–15. The rates are musical divisions. The document says blinking
  "will sync to TEMPO"; in Ableton the tempo comes from the **MIDI clock** the
  host sends. **To verify on the user's unit:** send MIDI clock (`F8`, 24 per
  quarter note) from our tempo (T-150) to make the pads pulse in time.
- Clip stop `0x34` ch 0–7: 0 off, 1 on, 2 blink (1/8 of tempo).
- Crossfader A/B `0x42` ch 0–7: 0 off, 1 yellow, 2 orange.
- Track select / Activator / Solo / Rec arm, device buttons `0x3A`–`0x41`,
  Master, Pan/Sends/User/Metronome, Play, Record, Session: 0 off, 1–127 on.
- Arrows, Shift, Tap, Nudge, Stop All, Bank: no LED.
- Rings: same CCs and ring types as the mk1.

### 2.4 mkII RGB palette (velocity → sRGB, from [2] pp. 18–21)

```
0:#000000 1:#1E1E1E 2:#7F7F7F 3:#FFFFFF 4:#FF4C4C 5:#FF0000 6:#590000 7:#190000
8:#FFBD6C 9:#FF5400 10:#591D00 11:#271B00 12:#FFFF4C 13:#FFFF00 14:#595900 15:#191900
16:#88FF4C 17:#54FF00 18:#1D5900 19:#142B00 20:#4CFF4C 21:#00FF00 22:#005900 23:#001900
24:#4CFF5E 25:#00FF19 26:#00590D 27:#001902 28:#4CFF88 29:#00FF55 30:#00591D 31:#001F12
32:#4CFFB7 33:#00FF99 34:#005935 35:#001912 36:#4CC3FF 37:#00A9FF 38:#004152 39:#001019
40:#4C88FF 41:#0055FF 42:#001D59 43:#000819 44:#4C4CFF 45:#0000FF 46:#000059 47:#000019
48:#874CFF 49:#5400FF 50:#190064 51:#0F0030 52:#FF4CFF 53:#FF00FF 54:#590059 55:#190019
56:#FF4C87 57:#FF0054 58:#59001D 59:#220013 60:#FF1500 61:#993500 62:#795100 63:#436400
64:#033900 65:#005735 66:#00547F 67:#0000FF 68:#00454F 69:#2500CC 70:#7F7F7F 71:#202020
72:#FF0000 73:#BDFF2D 74:#AFED06 75:#64FF09 76:#108B00 77:#00FF87 78:#00A9FF 79:#002AFF
80:#3F00FF 81:#7A00FF 82:#B21A7D 83:#402100 84:#FF4A00 85:#88E106 86:#72FF15 87:#00FF00
88:#3BFF26 89:#59FF71 90:#38FFCC 91:#5B8AFF 92:#3151C6 93:#877FE9 94:#D31DFF 95:#FF005D
96:#FF7F00 97:#B9B000 98:#90FF00 99:#835D07 100:#392B00 101:#144C10 102:#0D5038 103:#15152A
104:#16205A 105:#693C1C 106:#A8000A 107:#DE513D 108:#D86A1C 109:#FFE126 110:#9EE12F 111:#67B50F
112:#1E1E30 113:#DCFF6B 114:#80FFBD 115:#9A99FF 116:#8E66FF 117:#404040 118:#757575 119:#E0FFFF
120:#A00000 121:#350000 122:#1AD000 123:#074200 124:#B9B000 125:#3F3100 126:#B35F00 127:#4B1502
```

Useful families: full colours sit at 5, 9, 13, 17, 21, 25 … (step 4), with a
dim variant at +1 and +2 (e.g. red 5 → 6 → 7). We map a cue colour to the
nearest entry (in a perceptual space; plain RGB distance is fine to start) and
use the dim variant for "cue present but not playing".

### 2.5 Model detection

1. **Port name** (CoreMIDI): contains `APC40 mkII` → mkII; `APC40` alone →
   mk1. Names can vary by OS/firmware, so this is only a hint.
2. **Device Inquiry** (`F0 7E 7F 06 01 F7`): byte 7 of the answer is `0x73`
   (mk1) or `0x29` (mkII). Authoritative; `0x28` would be an APC mini
   (future profile).

---

## 3. How other laser software uses the APC40 (inspiration only)

**Laserworld Showcontroller LIVE** (manual [3], forum [4]):
- The whole LIVE screen is built around the APC: a **5 × 8 scene grid** with
  **10 banks** (400 scenes). APC40, APC40 mkII and APC mini are **auto-detected
  with LED feedback and hard-coded mappings**; other controllers use a
  **Teach-In** table *without* LED feedback. Up to two MIDI inputs.
- **Scene launch buttons select banks** (mkII: banks 1–5 directly; older
  firmware used Shift + bank for 6–10; the bank LED intensity differs between
  banks 1–5 and 6–10). Active scene is marked; "flash only" scenes play while
  the pad is held; a Multi-select mode layers scenes.
- **Tap** on the APC sets the BPM, **Nudge ±** adjusts it; a BPM mode vs a
  speed-fader mode (Shift toggles between them on newer versions).
- Faders drive the live parameters; its Art-Net remote chart lists the same
  set we want on faders: scene, bank, strobe, colour, size XY, size X, size Y,
  shift X, shift Y, speed, master intensity. "M" resets faders 1–7.

**Pangolin BEYOND** (wiki [5], forums [7][8]):
- Ships `.BeyondMidiMap` templates ("Advanced APC40 Template v3.4" for the mk1,
  "Advanced APC40MKII Template v1.9" for the mkII) plus a picture of the
  layout in `BEYOND\MIDI`. Loading the template **reconfigures the cue grid to
  match the pads** and sends an **init string** (the Introduction SysEx) so
  LEDs work without Ableton.
- MIDI maps have **layers** (Shift-like pages switched by PangoScript
  `SetMidiLayer n`); knobs can drive master physics etc. Custom map editor.
- **QuickShow** officially supports only the **APC mini** (auto-recognised
  with lit pads); the APC40 needs manual mapping of faders/knobs, and cue
  triggering from it was not possible natively (users convert notes to
  keystrokes with third-party tools).

**Take-aways for us**: auto-detect + built-in profile with LED feedback is the
expected baseline (Showcontroller); learn/teach-in for everything else; a
Shift layer doubles the control count (BEYOND layers); pickup and colour
feedback are where we can be better than both.

---

## 4. Architecture: native Rust MIDI vs Web MIDI

| Criterion | Web MIDI in the browser | Native (`midir` + CoreMIDI in Rust) |
|---|---|---|
| Laser keeps responding with the tab closed / asleep | ❌ No tab = no MIDI | ✅ |
| Background tab throttling (timers ≥ 1 s) → LED beat blink, LED refresh | ❌ | ✅ |
| Browser support | Chrome/Edge/Firefox; **Safari: none** | n/a |
| Permission | Chrome ≥ 124 prompts for **all** MIDI access, SysEx included [9] | none |
| Latency hardware → engine | MIDI → JS → `fetch` POST → HTTP → lock (~2–10 ms, jittery) | CoreMIDI callback → channel → lock (< 1 ms) |
| LED feedback from engine state (cue ended, playlist, timeline, tempo) | needs polling the server | direct |
| Tests | Headless Chromium + fake MIDI is awkward | virtual CoreMIDI ports (`midir::os::unix::VirtualInput/VirtualOutput`) and pure parsing tests |
| Complexity | no new crate | one crate (MIT), a thread, hot-plug polling |

**Recommendation: native.** `midir` 0.11 (MIT, Apr 2026) wraps CoreMIDI;
`MidiInput::ignore(Ignore::None)` is needed to receive SysEx (Device Inquiry
answer). `midir` has **no hot-plug notification**: poll the port list every
~2 s and reconnect (or use the `coremidi` crate's notifications later). The
input callback runs on a CoreMIDI thread: it must only parse and push events
into a channel; a **MIDI worker thread** applies them to `Shared` and, at
≤ 30 Hz, diffs the desired LED state against what was last sent and sends
only changes (the APC takes ~3 bytes per LED; 40 pads = 120 bytes).

The browser gets: `GET /api/midi` (devices, active profile, last message,
learn state), `POST /api/midi/learn`, `POST /api/midi/mapping/delete`,
`POST /api/midi/profile`. Keyboard shortcuts stay in the browser.

---

## 5. Our default APC40 layout (proposal, same for both models)

Targets use the control ids of **T-145**; names below are illustrative.
Everything is overridable by MIDI learn.

| Hardware | Action (Shift = while Shift is held) | LED feedback |
|---|---|---|
| **Grid 5 × 8** | Play cue *n* of the current page (slots numbered row-major from the **top-left** on both models). Cue flash mode (T-155) = play while held. Shift + pad = flash (momentary) | mk1: yellow = cue present, green = playing, green blink = playing on a flash/beat cue, off = empty. mkII: cue colour dim = present, full + pulse = playing |
| **Scene launch 1–5** | Cue page 1–5; Shift → 6–10 | current page lit (mk1 blink when page ≥ 6); mkII colour per page |
| **Up / Down** | Previous / next cue page | — |
| **Left / Right** | Previous / next 40-cue slice if a page has more than 40 cues | — |
| **Stop all clips** | **Blackout** (same as Échap: disarm + stop) | — |
| **Shift + Stop all (hold 1 s)** | Arm the laser — **only if "Armer depuis le MIDI" is enabled** (off by default) | — |
| **Clip stop 1–8** | Stop the cue on layer 1–8 (T-155); 1 = stop current cue when there are no layers | lit when that layer plays |
| **Track select 1–8 / Master** | Choose which layer the device knobs edit (Master = global) | radio LED |
| **Activator 1–8** | Mute / unmute layer 1–8 | lit = audible |
| **Solo 1–8** | Solo layer | lit |
| **Record arm 1–8** | Toggle modifiers (T-140): strobe, colour cycle, mirror X, mirror Y, audio reaction, beat mode, invert, freeze | lit = on |
| **Track faders 1–8** | Size, size X, size Y, animation speed, rotation speed, strobe rate (0 = off), colour/hue shift (0 = cue colour), audio sensitivity | — |
| **Master fader** | Master brightness (never above the safety maximum, pickup) | — |
| **Crossfader** | Position X (centre = 0); with layers later: A/B mix (T-155) | — |
| **Top knobs 1–8** | Position Y, rotation angle, colour hue, colour cycle speed, wave/distortion amount, zoom pulse, points density, transition fade | ring = value |
| **Pan / Sends / User** (mk1: Pan / Send A / B / C) | Knob banks for the top knobs (position / colour / effects) | current bank lit |
| **Device knobs 1–8** | Parameters of the selected layer's cue: count, a, b, speed, colour 2, … (generator `GenParams`) | ring = value, re-sent on cue change |
| **Tap tempo** | Tap (T-150) | beat LED (below) |
| **Nudge − / +** | Beat phase −/+ (hold) or BPM ∓ 0.1 | — |
| **Tempo knob** (mkII) / Shift + Cue level (mk1) | BPM ± | — |
| **Cue level** | Position Y fine (relative) | — |
| **Metronome** (mkII) / device button 8 (mk1) | Toggle "cues on the beat" | **blinks on each beat** |
| **Play / Stop / Record** (mkII: Shift + Play = stop) | Timeline play-pause / stop / record live actions (T-160) | Play lit while playing |
| **Footswitch** | Tap tempo | — |

---

## 6. MIDI learn and mapping storage

- Every mappable UI control carries `data-control="<id>"` (T-145).
- **Learn mode** button ("Apprendre MIDI"; no single-letter shortcut, every
  letter is already a cue key on the AZERTY grid) outlines mappable controls; clicking one (or right-click → "Apprendre
  MIDI" at any time) arms the backend; the next Note/CC received is bound.
  Esc cancels; learning a message already bound asks to replace.
- The backend infers the kind: Note → trigger (buttons), toggle or momentary
  per target; CC with values 1–63/65–127 around 0/64 from a known encoder or
  from the profile → relative; else absolute.
- Right-click → "Oublier MIDI" removes the binding. A "Contrôleur" panel lists
  all bindings (message, target, mode, range) with delete buttons.
- Storage (`studio-data/midi/`):
  - `profiles/<slug>.json`: user profiles (full mapping list).
  - `devices.json`: port name → profile slug, "armer depuis le MIDI" flag.
  - Built-in profiles (`apc40`, `apc40-mk2`, `generic`) are compiled in
    (`include_str!`) and never written; editing one creates a user copy
    (`apc40-mk2-perso`) that takes precedence.
- JSON sketch:

```json
{
  "version": 1,
  "name": "APC40 mkII — Laser Studio",
  "driver": "apc40mk2",
  "match": { "port_contains": ["APC40 mkII"], "product_id": 41 },
  "mappings": [
    { "input": { "kind": "note", "channel": null, "number": 81 }, "shift": false,
      "target": "safety.blackout", "mode": "trigger" },
    { "input": { "kind": "cc", "channel": null, "number": 14 }, "shift": false,
      "target": "live.brightness", "mode": "absolute", "pickup": true, "min": 0.0, "max": 1.0 },
    { "input": { "kind": "cc", "channel": null, "number": 13 }, "shift": false,
      "target": "tempo.bpm", "mode": "relative", "step": 0.5 }
  ]
}
```

---

## 7. Safety notes (CLAUDE.md)

- MIDI can **always** blackout; blackout has priority over any other message
  in the same batch.
- **Arming from MIDI is off by default**; when enabled, Shift + hold 1 s, and
  never while the studio started without `--device`.
- **Pickup** for absolute controls; brightness never exceeds the safety
  maximum (T-003) whatever the fader says.
- Controller unplugged mid-show: keep the state, show a warning; an opt-in
  "blackout à la déconnexion".
- Tests and e2e must run with `--no-midi` (don't grab or light the user's real
  APC40 while their own instance runs) and use a test-only injection endpoint
  or virtual ports.

---

## Sources

1. Akai Professional, *Communications Protocol for Akai APC40 Controller*, Rev 1 (2009): <https://cdn.inmusicbrands.com/akai/apc40/APC40_Communications_Protocol_rev_1.pdf_1db97c1fdba23bacf47df0f9bf64e913.pdf>
2. Akai Professional, *Akai APC40 Mk2 Communications Protocol*, v1.2 (2015): <https://cdn.inmusicbrands.com/akai/attachments/apc40II/APC40Mk2_Communications_Protocol_v1.2.pdf>
3. Laserworld, *User Manual Showcontroller LIVE*: <https://www.showcontroller.com/en/manual/showcontroller-live.html> (PDF: <https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html>)
4. Showcontroller forum, "AKAI40 MKII and MIDI question": <https://www.showcontroller.com/en/forum/showcontroller-live/89-akai40-mkii-and-midi-question.html>
5. Pangolin wiki, "Akai APC40 MKII Layout V1.9": <https://wiki.pangolin.com/doku.php?id=beyond:akaiapc40mkiilayout> and "Akai APC40 Advanced Layout V3.4": <https://wiki.pangolin.com/doku.php?id=beyond:akaiapc40advancedlayout>
6. Ableton APC40_MkII MIDI remote script (grid note formula `32 + track − 8 × scene`), as mirrored at <https://github.com/xnamahx/APC40_MkIIx/blob/master/APC40_MkII.py> and summarised at <https://deepwiki.com/gluon/AbletonLive12_MIDIRemoteScripts/3.5-apc40-mkii-(akai)>
7. Pangolin forum, "APC40 Midi Questions": <https://forums.pangolin.com/threads/apc40-midi-questions.13248/>; "layers in beyond on APC40": <https://forums.pangolin.com/threads/layers-in-beyond-on-apc40.2141/>; "Akai APC40 MK2 Midi Template for BEYOND": <https://forums.pangolin.com/threads/akai-apc40-mk2-midi-template-for-beyond.2674/>
8. Pangolin forum, "Setting up the Akai APC 40 MK2 for Pangolin Quickshow": <https://forums.pangolin.com/threads/setting-up-the-akai-apc-40-mk2-for-pangolin-quickshow.25957/>
9. Chrome for Developers, "Web MIDI permission prompt" (Chrome 124): <https://developer.chrome.com/blog/web-midi-permission-prompt>
10. `midir` crate (0.11.0, MIT): <https://docs.rs/midir/latest/midir/>, <https://crates.io/crates/midir>
11. DrivenByMoss documentation, APC40 / APC40 mkII differences: <https://github.com/git-moss/DrivenByMoss-Documentation/blob/master/Akai/Akai-APC40-APC40mkII.md>
