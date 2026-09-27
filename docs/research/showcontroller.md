# Research: Laserworld Showcontroller & Showeditor

*Research agent report, 2026-09-27. Sources: public pages only (laserworld.com,
showcontroller.com, showeditor.com, public PDF manuals, Open Fixture Library).
No Laserworld software was downloaded, installed, run or decompiled.*

---

## 0. TL;DR

- **Showcontroller** (Windows only, current v3.4, 2026) is a suite: **LIVE**
  (a 5 × 8 scene grid × 10 banks = 400 scenes, built around the Akai APC40),
  **RealTime** (a timeline with per-scanner tracks, effect events and an
  "Animator" that modulates any parameter), **PicEdit** (a frame editor),
  **Tracer / SVG Tool** (image, video and SVG conversion) and a **Control Center**
  (routing, geometry, colour correction, 5 safety zones and a "horizon" dimmer).
- **Standard vs PLUS**: standard = up to 3 ShowNETs. PLUS = up to 20 DACs
  (16 content channels), non-Laserworld DACs, Realizzer/Depence²/Capture
  visualisers, video events, **ILDA export** and (since 3.4) "Node Mode".
- **Showeditor** is the free ShowNET companion: figure editor, effects window,
  timeline (16 channels × 3 tracks), a live keyboard grid, a DMX editor and a
  playlist.
- **Content**: "250+ free shows" ship with or come for Showcontroller, sorted by
  scanner count (1/2/3/4/5/7) plus graphics shows. Showeditor ships some shows,
  with more on showeditor.com. Standard frame/animation libraries and "Fix
  figures" are included too.
- **Licensing verdict: we must not bundle, convert or ship any of it.** No
  public page grants a redistribution licence. Laserworld's imprint reserves
  all rights ("reproduction ... only with written permission"). Showeditor
  enforces per-show **export protection** and says export rights "must be
  granted by the creator". The ShowNET SD-card ILDA sets carry no licence at
  all, so under our rules the answer is still no. What we *can* do: let the
  user load their own `.ild` files at runtime, and reimplement the effect
  *types* procedurally.
- **ShowNET facts**: SD playback uses ILDA **format 5** (the 2021+ mainboard
  docs also say 4/5), files are named `000.ild`–`255.ild` (0 = blackout, avoid
  `000`), and 230–255 are reserved for beams and test patterns. The hardware DMX
  modes are **DJ (19 ch)** and **Professional (34 ch)**. The
  **"11-channel" chart is Showcontroller LIVE's Art-Net/DMX remote-control
  chart**, not a ShowNET hardware mode. The SDK is for commercial partners only,
  under NDA, with per-partner keys, on Windows, Mac and Linux (experimental).

---

## 1. Feature inventory

### 1.1 Product structure

| Component | Role | Source |
|---|---|---|
| Showcontroller **LIVE** | Live scene grid for DJs/VJs, MIDI/DMX/Art-Net/keyboard control | [LIVE manual](https://www.showcontroller.com/en/manual/showcontroller-live.html), [LIVE PDF](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html) |
| Showcontroller **RealTime** | Timeline programming synced to audio/video/timecode | [Manual §3](https://www.showcontroller.com/en/manual.html) |
| **PicEdit** | 2D/3D frame and animation editor | [Manual §4](https://www.showcontroller.com/en/manual/4-showcontroller-picedit/4-4-special-features.html) |
| **Tracer** / **SVG Tool** | JPG/BMP → vector, AVI → animation, (animated) SVG → frames | [Software page](https://www.showcontroller.com/en/showcontroller-software.html) |
| **Control Center** | Output routing, geometry, colour, safety zones | [Manual §5.1](https://www.showcontroller.com/en/manual/5-control-center/5-1-control-center-features.html) |
| **Player** | Playlist playback of finished shows (revised in 3.x) | [Download notes](https://www.laserworld.com/en/download-file-2191-Showcontroller.html) |
| **Showeditor** (separate product) | Free ShowNET software: figure editor, effects, timeline, live, DMX editor, playlist | [showeditor.com](https://www.showeditor.com/en/), [manual](https://www.showeditor.com/en/manual.html) |

Platform: **Windows 7/8/10 only**, OpenGL GPU, 4 GB RAM. There is no Mac version
([hardware & licensing](https://www.showcontroller.com/en/showcontroller-software/hardware-and-licensing)).
The licence normally sits on a USB dongle. For the ShowNET it can also sit on
the interface itself. Per project memory, the user's ShowNET now lists
`/Showeditor /Showcontroller`. "No forced updates, no mandatory registration"
is a selling point ([showcontroller.com](https://www.showcontroller.com/en/)).

### 1.2 Standard vs PLUS

| Feature | Showcontroller | PLUS |
|---|---|---|
| Live and timeline control, MIDI/DMX, logo/graphics import, drag & drop | ✓ | ✓ |
| 250+ free shows | ✓ | ✓ |
| Hardware interfaces | up to **3 ShowNET** | up to **20 DACs, 16 individual content channels**; ShowNET, Netlase, Netlase LC, Easylase II/LC, Phoenix Micro USB V1 |
| Larger LIVE preview (all scanners at once) | – | ✓ |
| Realizzer 3D, Capture, Depence² visualiser output | – | ✓ |
| Video events on the timeline | – | ✓ |
| **ILDA file export** | – (import only) | ✓ |
| Node Mode: separate DMX/Art-Net control set per output channel (v3.4) | – | ✓ |

Sources: [PLUS upgrade](https://www.laserworld.com/en/laser-software/showcontroller-plus-upgrade.html),
[Showcontroller licence](https://www.laserworld.com/en/laser-software/showcontroller-license.html),
[hardware & licensing](https://www.showcontroller.com/en/showcontroller-software/hardware-and-licensing),
[v3.4 notes](https://www.laserworld.com/en/download-file-2191-Showcontroller.html).
Note: the licence product page is inconsistent. It lists "ILDA import/export"
and "up to 20 ShowNET" in the standard text, while the upgrade page puts both
under PLUS.

### 1.3 Live control (Showcontroller LIVE)

From the [public LIVE manual PDF](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html):

- **Grid**: 5 × 8 scenes × **10 banks = 400 scenes**. The layout mirrors the
  **Akai APC40 / APC40 mkII / APC mini**, which are auto-detected with LED
  feedback. Other controllers use MIDI teach-in (no LED feedback), with up to 2
  MIDI inputs.
- Hovering a scene shows its animation in a preview at the current BPM.
- **A scene is a short timeline**: Trickfilm events (frames or animations) plus
  effect events, per scanner (up to 8 scanners in LIVE), with a World
  (projection zone) and scan parameters per event. Layers can overlap
  ("layer pyramiding").
- **Modes**: *Multi Sel* (several scenes at once), *Beat Mode* (auto frame
  change every N beats, optionally random bank switch), *Loop / Flash*,
  *Flash2Black*, *Freeze*, and a per-scene *Flash only* option.
  ([modes](https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-f-modes-effects.html))
- **Timing**: a *Tap BPM* button with nudge ±, **or** a speed fader, never
  both. The manual documents no automatic audio beat detection; BPM is tapped.
  ([timing](https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-e-timing.html),
  [speed](https://www.showcontroller.com/en/manual/showcontroller-live/0-6-call-scenes/0-6-a-control-the-speed.html))
- **Frame change animations**: Off, Morph, Fade (out), Fade In, with a
  configurable duration
  ([link](https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-g-farme-change-animations.html)).
- **Groups of scenes** (saved multi-selections), **scanner groups** (up to 32
  named groups over scanners 1–8, e.g. "Center", "Satellites", "All") and a
  **Chaser** that steps output across scanner groups
  ([routing](https://www.showcontroller.com/en/manual/showcontroller-live/0-12-scanner-routing-and-groups.html),
  [chaser](https://www.showcontroller.com/en/manual/showcontroller-live/0-13-chaser.html)).
- **Live colour**: a *Colorspectrum* fader recolours everything. There are
  preset **recolour palettes** (up to 60 colours) in two modes: nearest-colour
  mapping, or step to the next palette colour on each colour change. A
  *Recolorindex* offset slider rotates the palette.
- **Static beam table**: up to 40 named, coloured beam targets per scanner,
  for mirror bounces and hot beams. Scenes that contain static beams get a grey
  beam icon "for safety reasons".
- **External devices**: a fog machine panel (pump, fan and heat channels) plus
  DMX on the timeline, output over the ShowNET DMX port or Art-Net.
- **Run-Text** in LIVE (v3.0) and a **Stars event** (v3.4).

### 1.4 Timeline (RealTime)

- Up to **16 edit scanners** (E1–E16) plus overlay scanners for reference.
  There are beam-show and graphics-show views, markers set with Space while
  playing, and mouse-wheel zoom
  ([interface](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-1-realtime-interface.html)).
- **Event types**: Trickfilm (frame or animation range, optional morph),
  effect events placed under a Trickfilm track (one effect can target
  several tracks, or all), DMX events, video events (PLUS), and SFX/loop events.
- Events can be dragged and resized **while the show plays**.
- Per show there can be up to **32 configurations** each of World,
  Colorbuffer, Rotation Order and ScanParameter. This lets graphics and beams
  with different scan speeds share one show
  ([showfiles](https://www.showcontroller.com/en/manual/2-first-steps-with-showcontroller/2-5-details-on-showfiles-in-showcontroller.html)).
- **Timecode**: MTC in and out (SMPTE/LTC needs an external converter).
  Showcontroller can act as a timecode master, playing audio and sending MTC
  ([timecode](https://www.showcontroller.com/en/manual/7-special-features/7-2-timecode.html)).
- **Audio**: WAV/MP3, plus AIFF since 3.1. Video on the timeline since 3.1
  (PLUS). "Global effects" arrived in 3.1.

### 1.5 Effects (RealTime and LIVE)

The central idea is **effect events + the Animator**. The Animator is not an
effect itself; it is a parameter modulator shared by all effects
([Animator](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-2-the-animator.html)):

- It has start and end values, discrete *steps*, *repeats*, *phase*, and a
  waveform: **Linear, Sine, Square, Exponential (exponent), Expression**.
- Expressions read external sources such as `MouseX`, `dmx(1)` and `midi(1)`.
- A **Curve window** lets the user hand-draw an envelope instead
  ([curve](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-3-the-curve-window.html)).

| Effect | What it does (public description) |
|---|---|
| Move X/Y, Rotate (X/Y/Z), Scale | Basic transforms, all driven by the Animator |
| **RGB effect** | Recolours to a target and interpolates the target frame for smooth results ([link](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-4-rgb-effect.html)) |
| **HSV / HUE** | Hue rotation. Start/end values 0–1 map to 0–360°; values 1→2 shift the existing colours by the animated angle |
| **Scanlimit** | Progressive draw-on / erase along the path, using begin and end curves (0–100 %) ([link](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-5-scanlimit.html)) |
| **Prism** | N copies on a circle of animated radius, with optional auto-rotation facing the centre ([link](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-7-prism-effect.html)) |
| **Morphing** | Interpolates between start and end frame; "Auto Morph" runs over a whole range |
| **Parts** | Applies any effect to only the points tagged as part *n* in PicEdit ([link](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-9-parts.html)) |
| **GeoNet** | Per-frame grid warp, pre- or post-effects; drag points, Shift = row, Ctrl = column ([link](https://www.showcontroller.com/en/manual/3-showcontroller-realtime/3-5-effects-in-realtime/3-5-10-geonet-effect.html)) |
| **Sparkle / HotSpot** | Random or travelling hot spots (colour or white) on interpolated paths. The manual warns this can exceed the MPE |
| Color effect (v3.0), Recolor event, Stars event (v3.4) | Colour and particle-like effects |
| Chaser | Output alternates across scanner groups |

**Showeditor effects window**
([ch. 7](https://www.showeditor.com/en/manual/7-effects-animation.html)):
rotation (±180°/360°+), displacement with FlipFlop, **multiplication (prism)**
with mirroring, mirror X/Y, soft colour (gradient fades), **shadow** (hides
part of a figure relative to its centre) and perspective.

**Text**: Showeditor has morphing text, scrolling text (two methods) and
hand-drawn special characters
([graphics features](https://www.showeditor.com/en/manual/6-figure-editor-main-window/6-2-graphics-features.html)).
Showcontroller has text creation plus Run-Text in LIVE.

**Abstracts and beam generators**: neither product publicly documents a
Pangolin-style "abstract generator". Beam looks are built from frames plus
Animator effects, the static beam table, and the free beam shows.

### 1.6 Content creation (PicEdit, Tracer, SVG)

- PicEdit handles 2D and 3D drawing. Its Transform menu has ToPoints (turn
  corners into beam points), Gradient, Accent (colour at corners), Center,
  Maximize, Flip/Flop, Rainbow (64-colour palette) and Interpolate
  ([link](https://www.showcontroller.com/en/manual/4-showcontroller-picedit/4-4-special-features.html)).
- It can import 3D animations from Blender
  ([§7.6](https://www.showcontroller.com/en/manual.html)).
- Tracer converts bitmaps and AVI; the SVG Tool converts static and animated SVG.
- v3.4 adds "automatic patch optimization for scanner performance", i.e.
  automatic path ordering.

### 1.7 Multi-projector, zones and geometry

- Each hardware interface maps to a **track index 1–16**. There is
  **scanner routing** with invert-X/Y and swap, and routing can override the
  timeline's addressing
  ([Control Center](https://www.showcontroller.com/en/manual/5-control-center/5-1-control-center-features.html)).
- **Worlds**, i.e. named projection zones: Audience (full area), Beam (upper
  area for hot beams), Screen (graphics), Raster (narrow). Custom worlds can be
  added
  ([worlds](https://www.showcontroller.com/en/manual/showcontroller-live/0-4-words-and-scanning-parameters.html)).
- **Scan-parameter presets**: Default 28K (beams), Graphics 30K, Raster 50K.
  *Dynamic Scanspeed* lowers the repeat rate when a frame has few points.
- **Geometry**: per-output size, offset, invert and swap in the Control
  Center. There is frame-level warping through GeoNet. The ShowNET firmware
  itself adds trapezoid and barrel correction in professional DMX setup mode.
- Showcontroller's free multi-scanner shows can be played **X-mirrored on
  satellites** for symmetry
  ([free shows](https://www.showcontroller.com/en/downloads/free-laser-shows.html)).

### 1.8 Safety

- **Up to 5 safety zones** with adjustable intensity and soft edges, plus a
  **Horizon** function (a top-to-bottom power ramp across the full width)
  ([Control Center](https://www.showcontroller.com/en/manual/5-control-center/5-1-control-center-features.html)).
- Master brightness limit, per-colour gamma and curves, a minimum point
  threshold, and multicolour (yellow/cyan) remapping.
- Static-beam scenes are flagged in the UI. The manual carries MPE warnings on
  sparkle and static beams.
- The LIVE Start/Stop buttons control only the output; the preview always runs.
  This matches our arm/disarm model.
- ShowNET firmware: hardware safety zones (DMX ch 16–17 in DJ mode, 19–20 in
  pro mode) and a zone editor in Admin Tool 1.38+
  ([video](https://www.laserworld.com/en/video-detail-kjyMg5DPnyM.html)).

### 1.9 DMX, Art-Net, MIDI, timecode, OSC

- **DMX in/out** through the ShowNET (external units need the DMX adapter).
  **Art-Net in/out**: DMX values per timeline track go out over UDP/Art-Net;
  "Enable Control via Artnet" allows remote control. The software appears as an
  Art-Net node.
- **LIVE Art-Net/DMX remote chart (11 channels)**:
  1 scene, 2 bank, 3 strobe, 4 colour, 5 size XY, 6 size X, 7 size Y,
  8 shift X, 9 shift Y, 10 speed, 11 master intensity
  ([LIVE PDF §9.b](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html)).
  This is very likely the "11-channel mode" in our notes.
- **Node Mode (PLUS, 3.4)** gives each output channel its own DMX/Art-Net
  control set.
- **MIDI**: APC presets, teach-in, and `midi(n)` in Animator expressions.
  **MTC** timecode in and out.
- **OSC** appears in showcontroller.com marketing, but no manual chapter
  documents it.
- Showeditor offers DMX input routing, remote control via DMX/MIDI (figures on
  keys, F0–F12 via DMX ch 19), and a DMX editor (EasyDMX faders)
  ([ch. 11/12](https://www.showeditor.com/en/manual.html)).

### 1.10 File formats

| Format | Product | Notes |
|---|---|---|
| `.pic` | Showcontroller | Single frame, browsed with the PicBrowser |
| `.ani` | Showcontroller | Multi-frame animation for Trickfilm events |
| `.cat` | Showcontroller | Catalogue bundling many frames into one file |
| Show files | Showcontroller | Timeline show plus up to 32 World/ScanParameter configs |
| `.heb`, `.bin` | Showeditor | Figures and test pictures; `FixFiguren` folder holds global "fix figures" |
| `.ild` (ILDA) | both | **Import** of formats 0/1 (needs a `.pal`, default `ILDA.pal`, then "Recolor all") and RGB formats. **Export**: Showeditor (subject to per-show rights) and Showcontroller PLUS ([§7.5](https://www.showcontroller.com/en/manual/7-special-features/7-5-ilda-import-and-export.html)) |
| `.pal` | both | ILDA palette |
| SVG, JPG/BMP, AVI, Blender | Showcontroller | Via the SVG Tool, Tracer and the Blender route |
| Realizzer, Depence², Capture | PLUS | Visualiser links |

Colour model: the legacy **colour buffer** (64 indexed entries, each driving up
to 6 channels) or direct RGB, chosen per show.

---

## 2. Preset and show library

| Library | Size / organisation | Where |
|---|---|---|
| Showcontroller free shows | **"250+ free shows"**. Categories: Beam shows for 1, 2, 3, 4, 5 and 7 scanners, plus Graphics shows. Examples: *Proximus, Reality, Reload* (1 scanner); *Feeling Good, Maze Runner, Spitfire* (4); *Pyrophantastica* (5) | [showcontroller.com free shows](https://www.showcontroller.com/en/downloads/free-laser-shows.html), [licence page](https://www.laserworld.com/en/laser-software/showcontroller-license.html) |
| Showcontroller built-in frames | "Large set of preset frames and animations". The standard shape library was extended in 3.x; PicBrowser/CAT catalogues | [licence page](https://www.laserworld.com/en/laser-software/showcontroller-license.html) |
| Showeditor | Some shows bundled with the installer, about 20 downloadable free shows/packs (2015–2024, e.g. *Peace on Earth* content pack by Living Lines), commercial shows in the Showeditor shop, plus test pictures and fix figures | [free shows](https://www.showeditor.com/en/downloads-en/free-laser-shows.html), [manual §14.2](https://www.showeditor.com/en/manual/14-important-hints/124-14-2-free-laser-shows.html) |
| ShowNET SD card | Factory "standard pattern set (gobos)" of numbered `.ild` files, a **"ILDA Files for ShowNET – Extended Set"** ZIP (30.4 MB, 2023+), a "ShowNET ILDA Standard Fileset" (2025), and gobo preview pictures | [Extended set](https://www.laserworld.com/en/download-file-1274-ILDA_Set_Extended_ShowNET.html), [gobo previews](https://www.laserworld.com/en/download-file-1717-Gobo_Preview_Picture_Set_ShowNET_content_2022.html) |

The shows appear to be distributed **without music**; the user supplies the
audio track. None of the download pages mention audio files.

---

## 3. Licensing of that content: verdict

### Evidence

1. **Website copyright**. The Laserworld imprint states: *"All texts, pictures
   and published information on this website are copyrighted to the website
   owner ... Reproduction, publication or playback only with written permission
   of the respective copyright owner."*
   ([Legal disclaimer / imprint](https://www.laserworld.com/en/legal-disclaimer-imprint.html))
2. **Technical export protection**. The Showeditor FAQ says *"The rights for
   exporting figures from Laserworld Showeditor to ILDA files must be granted by
   the creator of the figure / show"* and that most show programmers protect
   their content. Protection spreads to new figures made in a protected
   show's folder
   ([FAQ](https://www.showeditor.com/en/tutorials-faq/faq/195-i-want-to-export-figures-as-ilda-ild-file-and-its-doesn-t-work-what-is-the-problem.html),
   [ShowNET content guide](https://www.laserworld.com/en/laser-online-user-manual/how-to-create-and-upload-custom-laser-content.html)).
   The manual also says opening third-party shows requires that "the rights for
   opening the show file are granted"
   ([§14.3](https://www.showeditor.com/en/manual/14-important-hints/125-14-3-shows-created-with-third-party-software-compatibility.html)).
3. **Free shows and ShowNET ILDA sets**. The download pages carry **no licence
   text at all**: no CC, no "free to redistribute". "Free" means free of
   charge, not a redistribution licence. Some packs credit third-party authors
   (Living Lines, "Bankaifan"), so copyright may not even belong to Laserworld.
4. **No public EULA** grants rights to the bundled library. Showcontroller
   ILDA export is a paid PLUS feature, which suggests they treat export as a
   licensed privilege.

### Verdict

**We cannot copy, convert or redistribute any Laserworld content.** That
covers Showcontroller/Showeditor frames, shows, `.pic/.ani/.cat/.heb`
libraries, and the ShowNET standard or extended ILDA sets. This applies even
if the user can technically export it to `.ild`: export for personal
playback is not a licence to put it in our repo or ship it. Per our rules
(no licence found = don't add it), nothing from Laserworld goes into
`docs/CONTENT_SOURCES.md`. If we ever want a specific pack, the only route is
**written permission** from Laserworld or the named author.

### What we CAN legitimately do

| Allowed | How |
|---|---|
| User loads their **own** files at runtime | ILDA import (formats 0/1/4/5 plus `.pal`) from any path the user picks, kept in `studio-data/`, never committed. This includes ILDA the user exports from their licensed Showeditor/Showcontroller, or the Laserworld SD sets they downloaded. |
| Write ILDA **format 5** files the user copies to their ShowNET SD card | Export *our own* procedural content as `000`–`229.ild`. This is a genuine interop feature. |
| Reimplement **effect types** procedurally | Animator-style modulators, prism, scanlimit, hue shift, parts, GeoNet warp, chaser and so on. These are ideas and techniques described in public manuals, not copyrightable content. Use our own code, parameters and presets. |
| Mirror **workflow and UX concepts** | Scene grid with banks, APC40 mapping, beat mode, recolour palettes, worlds and safety zones. Write our own UI text and icons; don't copy screenshots or icons. |
| Interoperate with **public protocols** | ILDA, DMX/Art-Net channel charts (public PDFs and the Open Fixture Library), MTC. |

---

## 4. ShowNET facts useful to Laser Studio

### 4.1 Hardware (external ShowNET, 2015–2019 manual)

- 12-bit X/Y, up to 6 × 8-bit colour outputs (R, G, B, intensity, user 1, user 2).
- 10/100 Ethernet with fixed IP, DHCP or AutoIP. Up to **16 ShowNETs in
  parallel**, and up to 150 kpps (the 2021+ mainboard docs say about 100 kpps).
- Built-in figures, a microSD slot, and a 10-way DIP switch that selects the
  mode. Changing mode requires a power cycle, and switching DIPs while running
  can cause "random and dangerous laser output".
  ([manual PDF](https://audioeffetti.com/product/documents/LAS/LASERWORLD%20SHOWN-01.pdf))
- The standard external ShowNET has **no DMX port**. It needs the Laserworld
  DMX adapter; the ShowNET PRO and laser mainboards have DMX built in
  ([FAQ](https://www.laserworld.com/en/shownet-faq/5894-how-can-i-use-dmx-to-trigger-the-external-shownet-interface.html)).
- Our unit runs **firmware 2016050502**, older than the documented 2019/2021
  feature sets, so the DMX profiles, zones and Art-Net trigger may differ.

### 4.2 SD-card playback

- Files must be **ILDA format 5 (RGB true colour)**. The 2021 mainboard page
  also accepts 4; format 5 is the safest choice.
  ([custom content guide](https://www.laserworld.com/en/laser-online-user-manual/how-to-create-and-upload-custom-laser-content.html),
  [FAQ](https://www.laserworld.com/en/shownet-faq/5880-why-are-my-ilda-files-not-recognised-by-the-shownet-interface.html))
- Names must be **`000.ild` – `255.ild` only**; any other name is ignored. The
  number is the DMX value on the pattern channel.
  - `000` is best left empty: DMX 0 = blackout, and `000` makes the scanners
    move even at intensity 0.
  - **230–255 are reserved** for hot beams and test patterns and are skipped by
    the auto, demo and sound modes. The standard set has a grid test pattern at
    255.
- Recommended limits: ≤ 8 MB per file (Admin Tool upload ≤ 6 MB, use a card
  reader above that), ≤ 50 MB per upload batch, and standard SD ≤ 2 GB
  recommended.
- **Modes**:
  - *Stand-alone/auto*: loops through all files. DIP 4 loops a single file;
    DIP 1/2 step next/previous.
  - *Demo*: adds automatic internal animation.
  - *DMX/Art-Net trigger*: DIP 10 on, DIP 1–9 set the binary start address.
  - *Master/slave*: over DMX, with identical SD sets on every unit.
  - *Sound-to-light*: mainboard with a mic only.
- Art-Net trigger requires DHCP, plus Admin Tool → Settings → "Data source
  for internal DMX effects = ArtNet input".

### 4.3 DMX profiles (firmware-dependent)

| Profile | Channels | Key functions | Source |
|---|---|---|---|
| **DJ mode** (default) | 19 | 1 intensity, 2 pattern (file no.), 3 fps (0–15 = 50 fps; up to 100 fps), 4 size, 5 auto-size, 6 rotate, 7–10 X/Y coarse and fine, 11 colour effects, 12 colour fades, 13 strobe, 14 operation mode (DMX/auto-pos/demo/sound), 15 scan speed (5–30 kpps), 16 safety-zone size and side, 17 zone intensity, 18 blanking, 19 blank shift | [DJ chart PDF](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_DJ_Mode.pdf) |
| **Professional** | 34 (about 11 reserved) | 16-bit position and rotation, separate X/Y size, inversion, colour select plus direct R/G/B, strobe, scan speed, safety zone and intensity, white-balance R/G/B, **Setup mode** (ch 24–25 magic values) with trapezoid and barrel geo-correction stored to the board | [Pro chart PDF](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf), [OFL](https://open-fixture-library.org/laserworld/shownet) |
| **Showcontroller LIVE remote** (software, not hardware) | 11 | scene, bank, strobe, colour, size XY/X/Y, shift X/Y, speed, master | [LIVE PDF](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html) |

I found no public **11-channel ShowNET hardware** profile. Before building a DMX
path, the user should check in the Admin Tool which profile firmware
2016050502 actually exposes (this is a read-only check).

### 4.4 Admin Tool

This is a Windows tool downloaded from laser-interface.com, which now redirects
to laserworld.com. Current version 1.39
([download](https://www.laser-interface.com/en/downloads/firmware-tools/download/2-firmware/39-shownet-admin-tool-1-39)).
It handles:

- test output
- SD-card management (upload, download, delete, format)
- settings: DJ/Pro profile, Art-Net source, playback behaviour
- zone setting and a Setup & Store mode (v1.38)
- licence codes
- firmware updates

It cannot connect while Showeditor or Showcontroller is using the box
([video](https://www.laserworld.com/en/video-detail-Sqw4h0_uEiQ.html)).

### 4.5 API / SDK programme

- A library for **Windows, Mac and Linux (experimental)**. It uses per-partner
  keys under a "little NDA", and is given **only to commercial integration
  partners**: publicly available software or industrial use cases. The contact
  is Norbert via laserworld.com. No price is published
  ([API page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html),
  [PLSN](https://plsn.com/newsroom/product-news/laserworld-announces-availability-of-shownet-laser-api/)).
- Existing integrations: Showcontroller, Showeditor, MadMapper/MadLaser,
  TouchDesigner, ILD Render (Blender/Inkscape), Millumin, Modulaser and
  CloudLase Studio
  ([compatible software](https://www.laserworld.com/en/shownet-compatible.html)).
- For us: the application should present Laser Studio as a **publicly
  available** project, since private hobby projects are explicitly out of
  scope. The per-box licence also matters: third-party SDK apps only connected
  after Laserworld added the licence code (see project memory).

### 4.6 Multi-ShowNET

- Showeditor supports up to 16 DACs (16 program channels × 3 tracks).
  Showcontroller standard supports 3 ShowNETs; PLUS supports 20 DACs and 16
  content channels.
- The units sit on one switch (TCP/IP) and are addressed individually. In
  stand-alone use they are chained over DMX master/slave.

---

## 5. Showcontroller vs Pangolin QuickShow

What Showcontroller does **better or differently**:

| Aspect | Showcontroller | QuickShow (for contrast) |
|---|---|---|
| Scene model | Each live scene is a **mini timeline** with per-scanner tracks and effect events, so a cue can hold a full choreography | Cues are mostly frame + effect stacks; per-cue timeline is limited |
| Parameter modulation | One unified **Animator** (linear/sine/square/exp/expression, steps, repeats, phase) plus hand-drawn curves on every effect parameter | Many fixed effect generators with their own parameters |
| Expressions | `dmx(n)`, `midi(n)` and `MouseX` can drive any effect parameter directly | Mapping through control assignments |
| Hardware controller | Designed around the APC40/APC mini with LED feedback out of the box | Supports MIDI controllers but the grid is not tied to one layout |
| Multi-projector | Scanner groups (32), chaser over groups, X-mirrored satellite playback, per-event World, 16–20 DACs | QuickShow targets fewer projectors; multi-projector is mainly BEYOND territory |
| Worlds / scan presets | Named projection zones and scan-parameter configs **per event** (beam 28K vs graphics 30K in one show) | Global projection zones |
| Partial effects | **Parts**: effects on tagged subsets of a frame's points | Not a headline QuickShow feature |
| Colour | Recolour palettes with two mapping strategies and an index offset; legacy 64-colour buffer mode | Colour palettes and colour effects |
| DMX/fog | Built-in fog-machine panel, DMX on the timeline, Art-Net node, 11-ch remote chart, Node Mode | DMX/Art-Net input, less focus on driving other fixtures |
| Licensing | One-time dongle or on-box licence, **no forced updates** | Bundled with FB3/FB4 hardware |
| Weak spots | Windows-only, dated UI, **no automatic beat detection** (tap only), OSC barely documented, ILDA export only in PLUS | QuickShow has a huge effect/abstract library, a polished UI, and audio/beat features |

(QuickShow reference: [pangolin.com/pages/quickshow](https://pangolin.com/pages/quickshow). A
dedicated Pangolin research report should confirm the QuickShow column.)

---

## 6. Recommendations for Laser Studio

### 6.1 Top 10 features to prioritise (inspired by Showcontroller)

| # | Feature | Why | Difficulty |
|---|---|---|---|
| 1 | **Scene grid 5 × 8 with banks** (40 pads × N banks), hover/preview thumbnails, Loop/Flash/Freeze/Flash-to-black, Multi-select | The core live workflow; maps onto APC-style controllers | **Low–Med**: extends `scenes.rs`, plus a UI grid in `index.html` and thumbnails rendered from the engine |
| 2 | **Unified Animator / modulator** on every `Settings` parameter: start/end, waveform (lin/sin/square/exp), steps, repeats, phase | One mechanism gives "endless" effects; very high leverage | **Med**: a `Modulator` struct evaluated in `Animator`, `serde(default)` for backwards compatibility |
| 3 | **Tap BPM + Beat Mode** (auto-advance scene every N beats, random bank) combined with our audio beat detection | Showcontroller only taps; we can beat it with real detection | **Med**: tap/nudge is easy, robust onset detection is medium |
| 4 | **Safety zones + horizon dimmer** (≥ 5 zones, soft edges, per-zone intensity) applied after all effects in the output path | Audience safety; must honour arm/disarm | **Med**: polygon mask on points before `densify`, needs good tests |
| 5 | **ILDA import (0/1/4/5 + .pal) and ILDA format 5 export** with ShowNET-safe naming (`001`–`229.ild`, warn on 000/230+) | User-owned content at runtime, and our content on the SD card while the SDK is pending | **Low–Med**: ILDA import partly exists; export is a straightforward writer |
| 6 | **Effect set**: Prism (N copies, radius, face centre), Scanlimit (draw-on/erase), Hue shift (rotate existing colours), Sparkle hot spots (with an MPE warning), Mirror/FlipFlop | Classic laser vocabulary, all procedural | **Low–Med** each: pure point transforms |
| 7 | **Multi-output routing**: outputs as tracks, invert/swap per output, scanner groups, **chaser** across groups, mirrored satellites | Needed once there are 2+ DACs; also works in preview | **Med–High**: the `Output` trait needs N outputs and per-output arm state |
| 8 | **Live recolour palettes** (nearest-colour vs step-on-change modes, index offset) + colour-spectrum fader | Fast way to match venue lighting | **Low**: per-point colour map |
| 9 | **MIDI (APC40/APC mini with LED feedback) + Art-Net/DMX remote chart** (scene, bank, strobe, colour, size, shift, speed, master) + `dmx(n)`/`midi(n)` as modulator sources | Hands-on live control and console integration | **Med**: `midir` + Art-Net UDP listener; LED feedback per device profile |
| 10 | **Per-output geometry**: size, offset, keystone (trapezoid), and later a GeoNet-style grid warp | Real rooms are not square; the ShowNET only does it through DMX setup | **Med** for keystone/barrel, **High** for an editable grid-warp UI |

Next tier: a timeline per scene with effect events and MTC in/out; "Parts"
(effects on tagged point subsets); worlds and per-event scan presets
(beam vs graphics point rate); a static beam table with a safety flag; frame
transitions (morph/fade); a DMX fog panel; SVG/bitmap tracing.

### 6.2 Process and IP guidance

1. **Ship only procedural presets.** Name them ourselves. Do not recreate the
   named Laserworld shows or figures 1:1 from screenshots.
2. **ILDA import is the user's door** to Laserworld and other content. Keep
   loaded files in `studio-data/`, never in the repo, and state this in the UI
   and README.
3. **ShowNET output**: meanwhile, keep preview and IDN/Ether Dream output
   through `laser-dac`, and add "Export to ShowNET SD" (format 5, numeric
   names) as a sanctioned offline path. When the NDA SDK arrives, add a
   `ShowNetOutput` behind the existing `Output` trait. The SDK application
   should describe Laser Studio as publicly available software.
4. **DMX path**: target the documented DJ (19 ch) / Pro (34 ch) profiles, but
   first have the user confirm which profile firmware 2016050502 exposes.
   The external unit needs the DMX adapter, or Art-Net where the firmware
   supports it.
5. Do not add anything from Laserworld to `docs/CONTENT_SOURCES.md`. No
   redistributable licence exists.
