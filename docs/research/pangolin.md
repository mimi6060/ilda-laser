# Research: Pangolin QuickShow and BEYOND

*Research agent report. Date checked: 2026-09-27. Based only on public
information: pangolin.com, the Pangolin wiki, the Pangolin forums, and dealer
pages that quote Pangolin. No Pangolin software was downloaded, run or
decompiled.*

---

## 0. TL;DR

- **QuickShow** is free with Pangolin FB3/FB4 hardware. It is built for
  beginners and DJs: a 10x6 cue grid, simple live controls, "QuickTools",
  Virtual Laser Jockey, a basic timeline, up to 9 projectors and 30 zones.
- **BEYOND** (Essentials / Advanced / Ultimate) is the professional product.
  It adds a variable grid (up to 255 cells), an FX grid (up to 8 lines x 100
  effects), live control at cue, master and zone level, a multi-track timeline
  with timecode in and out, Art-Net, sACN, OSC, CITP, PangoScript, NDI, 3D,
  up to 40 projectors and 250 zones.
- **Bundled content**: about 2,000 cues (graphics, animations, abstracts,
  beams, BeamBrush) plus the online Pangolin Cloud library.
- **Licence verdict**: **no.** Under the Pangolin EULA the bundled content
  counts as "Pangolin-Provided Show Elements" (PPSEs). It may only be projected
  through genuine Pangolin hardware and only moved around in Pangolin file
  formats. Pangolin also removed ILDA export on purpose to stop piracy. We may
  not ship it, and a user may not legally export it and play it through
  Laser Studio and the ShowNET either.

---

## 1. Feature inventory: QuickShow vs BEYOND

Main source: the official comparison table
[pangolin.com/pages/compare-beyond-versions](https://pangolin.com/pages/compare-beyond-versions),
plus the [QuickShow page](https://pangolin.com/pages/quickshow),
[BEYOND page](https://pangolin.com/pages/beyond) and the
[QuickShow wiki manual](https://wiki.pangolin.com/doku.php?id=quickshow:start).

Legend: QS = QuickShow, B-E/A/U = BEYOND Essentials/Advanced/Ultimate.

### 1.1 Content creation

| Feature | QS | B-E | B-A | B-U | Notes |
|---|---|---|---|---|---|
| Frame/animation editor | ✅ | ✅ | ✅ | ✅ | Vector drawing, per-point colour. The "Advanced Frame Editor" needs A/U |
| Text editor (static and scrolling) | ✅ | ✅ | ✅ | ✅ | QuickText is the quick version ([wiki](https://wiki.pangolin.com/doku.php?id=quickshow:quicktext)) |
| Shape / abstract editor | ✅ | ✅ | ✅ | ✅ | Layered oscillators, modulators and colour cycling that make "spirograph-type" images ([Kvant](https://www.kvantlasers.co.uk/blogs/news/focus-on-quickshow)) |
| Parametric image editor | ✅ | ✅ | ✅ | ✅ | More than 20 simple parametric forms (QS 4.0, [latest features](https://wiki.pangolin.com/doku.php?id=quickshow%3Alatest_features)) |
| Clock editor | ✅ | ✅ | ✅ | ✅ | Live clock or countdown shown in laser |
| Effect editor / effect generator | ✅ (QuickFX) | ✅ | ✅ | ✅ | See the BEYOND effect model in §5 |
| Beam sequences (QuickTargets) | ✅ | ✅ | ✅ | ✅ | Beams aimed at "targets" (mirror balls, points in the air) |
| Picture tracer (bitmap to vector) | QuickTrace | ✅ | ✅ | ✅ | Realtime video tracer: Ultimate only |
| Synth editor, Abstraction editor (LivePRO-style) | ❌ | ✅ | ✅ | ✅ | |
| LD2000 abstract editor, Particle / Node / FIFO image editors | ❌ | ❌ | ✅ | ✅ | |
| Q-Shift (delayed oscillation across lasers) | ✅ | ✅ | ✅ | ✅ | Added in 5.5 |
| 3D objects, Object Animator, 3D preview | ❌ | ❌ | ❌ | ✅ | |
| DCC plugins (Blender, Cinema4D, 3ds Max) | ❌ | Blender | +C4D | +Max | |
| BeamBrush (real-time beam divergence) | ❌ | ✅ | ✅ | ✅ | Needs BeamBrush-capable projectors |

### 1.2 Live control

| Feature | QS | BEYOND | Notes |
|---|---|---|---|
| Cue grid | 10x6 = 60 cues per page, up to 80 pages (the wiki says 32) | Variable grid, 100 cues per page, 128 to 256 pages, plus a **secondary grid** | [QS cue grid](https://wiki.pangolin.com/doku.php?id=quickshow:cue_grid), [BEYOND grids](https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages) |
| Keyboard triggering | Each cell shows its key letter. Shift is used for more rows. F-keys switch pages | Same | |
| Hover preview of a cue | ✅ | ✅ | |
| Click modes | – | Select / Toggle / Restart / Flash / Solo-Flash | BEYOND |
| Multiple cues at once | ✅ | ✅, with Groups mode ("one cue per group") | |
| Live controls (size, position, rotation, colour, brightness, speed) | ✅ Master and Cue | Cue, Master, **Zone** and ProTrack levels, applied in a fixed order | [BEYOND live control](https://wiki.pangolin.com/doku.php?id=beyond:livecontrol) |
| Scan rate and visible-points controls | limited | ✅ | |
| "Physics" smoothing of sliders (mass, spring, friction) | ❌ | ✅ | |
| Time control (reverse, A-B loop, DJ jog disk) | ❌ | Ultimate | |
| FX grid | 12 effects, 4 rows | up to 8 lines x 100 effects; one / multi / drop modes | |
| BPM and tap tempo | ✅ Space key taps, Backspace re-syncs | ✅ | [Music & beats](https://wiki.pangolin.com/doku.php?id=quickshow:music_and_beats_overview) |
| Ableton Link | ✅ | ✅ | 5.5 |
| Audio beat detection | Audio input for VLJ (Pangolin calls it unreliable), PangoBeats app | PangoBeats + realtime FFT (512 bands) + peak events that drive effects | [Realtime audio](https://wiki.pangolin.com/doku.php?id=beyond%3Arealtime_audio) |
| Virtual Laser Jockey (auto-triggers cues on the beat) | ✅ | ✅ | Plays a page in order or at random, every N beats ([VLJ](https://wiki.pangolin.com/doku.php?id=quickshow:virtual_laser_jockey)) |
| Mobile remote (MoboLaser) | ✅ | ✅ | |
| UI access levels (Beginner / Intermediate / Advanced) | ✅ | – | Hides risky tools from novice operators |

### 1.3 Show programming

| Feature | QS | B-E | B-A/U | Notes |
|---|---|---|---|---|
| Timeline | QuickTimeline, 1 media track | 40 tracks, 2 media tracks | 200 tracks, 4 media tracks | |
| Dual clock (seconds and beats) | ✅ | ✅ | ✅ | Events sit at fixed times. Effects inside them run on seconds or on beats ([timeline_bpm](https://wiki.pangolin.com/doku.php?id=quickshow:timeline_bpm)) |
| Audio track in timeline | ✅ | ✅ | ✅ | |
| Video playback in timeline | ❌ | ✅ | ✅ | Laser/video masking needs A/U |
| BUS tracks, cue and effect lists, playlist | ❌ | ✅ | ✅ | |
| Timecode in: MTC/MMC, Art-Net TC, SMPTE via TC2000/TC4000 | ❌ | ✅ | ✅ | [BEYOND timecode](https://wiki.pangolin.com/doku.php?id=beyond%3Atimecode) |
| Timecode out: MTC, Art-Net TC | ❌ | ✅ | ✅ | |
| CSV marker import/export | ❌ | ✅ | ✅ | |
| Scripting (PangoScript, MIDI/DMX/OSC/keyboard-to-code) | ❌ | ❌ | ✅ | |

### 1.4 Multi-projector and projection zones

| | QS | B-E | B-A | B-U |
|---|---|---|---|---|
| Max projectors | 9 | 10 | 25 | 40 |
| Max zones | 30 | 60 | 200 | 250 |
| Distributed scanning (one image split across projectors) | – | 1 of 2 | 2 of 4 | 4 of 8 |
| Zone "Also to", static zone effects | ❌ | ❌ | ✅ | ✅ |
| Zone-routing effects (Set / Add / Delete / Replace zone, Zone Chase) | ❌ | ✅ | ✅ | ✅ |

A **projection zone** is Pangolin's key idea. It bundles:

- a target projector (scanner),
- its own geometric correction,
- its own preview appearance,
- its own Beam Attenuation Map.

Each cue stores which zones it goes to. Pangolin suggests a zone layout by
purpose: zones 1-4 are the main output of each projector, 5 is secondary
graphics, 6 is raster, 7 is high-intensity beams, 8 is atmospheric and
audience-scanning effects, and 30 is targeted beams
([projection zones](https://wiki.pangolin.com/quickshow:projection_zones)).
Because of this, one content library can play correctly on a graphics
surface, in the air, or over the audience.

### 1.5 Geometric correction and calibration

- **Per projector** (global settings): size, position, sample rate
  (scan speed), colour shift (colour/galvo timing delay), and test patterns
  for tuning them
  ([projector settings](https://wiki.pangolin.com/doku.php?id=beyond%3Aprojector_settings-new)).
- **Per zone**: X/Y size, X/Y position, Z rotation, keystone, pincushion,
  bow, shear and linearity. "Auto mesh" adds four-corner keystoning
  (5.2).
- **Wide-angle compensation**: the scan speed drops automatically at large
  scan angles (5.0).
- **Advanced colour tuning** (BEYOND). Built-in test patterns in every
  version.

### 1.6 Safety

- **Beam Attenuation Map (BAM)**, in every version. It is a 2D map over the
  scan field that reduces output power by a set amount, for example 50-70%
  over the audience, or blocks areas entirely (above or below a line,
  building windows). One BAM per zone. BEYOND 5.2 adds gradients and blending
  ([Pangolin safety](https://pangolin.com/pages/laser-show-safety),
  [BAM video](https://www.youtube.com/watch?v=89xLGkicUjI),
  [audience scanning](https://fr.pangolin.com/blogs/education/audience-scanning-safety)).
- **Zone masking**: a dedicated zone for audience scanning, whose BAM keeps
  power down in the audience area.
- **PASS** (Professional Audience Safety System) is **hardware** built into
  the projector, not software. It watches beam power, scanner position
  signals and power supplies, and checks that no light comes out when
  blanked. On a fault it blanks the colours, closes the shutter and opens the
  interlock. It is designed to stay safe through five simultaneous failures
  ([PASS](http://pangolin.com/PASS/),
  [PASS manual](https://pangolinlegacy.com/PASS/PASSmanual.pdf)).
  **SafetyScan lens**: a divergence lens for audience scanning.
- Blackout/pause and "Enable laser output" arming in the toolbar
  ([QS toolbar docs](https://wiki.pangolin.com/doku.php?id=quickshow:blackout_and_pause)).

### 1.7 Integrations

| Protocol / device | QS | BEYOND |
|---|---|---|
| DMX in (control from a lighting console) and QuickDMX out to fixtures | ✅ | ✅. A/U add a "DMX server" mode and custom DMX profiles that map the whole effects engine |
| Art-Net, sACN | ❌ | ✅ |
| MIDI | ✅ (APC Mini / APC Mini MK2 profiles) | ✅ (APC40 MK2 and custom map editor) |
| OSC | ❌ | ✅ |
| CITP (media-server thumbnails to consoles), UDP broadcast, PangoScript TCP server | ❌ | A/U |
| NDI input, Kinect, webcam | ❌ | Ultimate |
| Ableton Link | ✅ | ✅ |
| Gamepad | ❌ | ✅ |
| External visualisers (for example Capture, WYSIWYG) | ❌ | ✅ |

### 1.8 File formats

- **QuickShow native formats**: `.qsw` workspace, `.qsbframes` frames,
  `.qeff` effects, `.qtxt` text, `.qabs` shapes, `.qshw` timeline shows,
  `.qclk`, `.qbem` beam sequences, `.qfx`
  ([files & extensions](https://wiki.pangolin.com/doku.php?id=quickshow:files_and_file_extensions)).
- **Import**: ILDA `.ild` import works in QuickShow and BEYOND ("Import
  picture"), including 3D ILDA
  ([forum](https://forums.pangolin.com/threads/any-kind-of-3d-import-possible-in-quickshow.756/)).
  Pangolin's converter tools import DXF, DWG and AI. The Blender, C4D and Max
  plugins also feed content in.
- **Export**: **no ILDA export**, on purpose. Export goes to Pangolin
  formats: LD2000 frames, `.lds` "Lasershow Designer Secure", and FB4 SD-card
  export. Frames can carry protection flags
  ([Benner 2010](https://forums.pangolin.com/threads/missing-ilda-export-function-in-quickshow.799/),
  [Bob@Pangolin 2016](https://forums.pangolin.com/threads/exporting-ild-from-5-6.2755/),
  [Benner 2018](https://forums.pangolin.com/threads/beyond-frame-protection-export-to-ld2000-export-to-ild.15413/)).
  Pangolin's president gives piracy as the reason: "unauthorized
  distribution of Pangolin content on cheap Chinese laser projectors".
- BEYOND also imports ST2000 shows and CSV markers, and can mix down to
  video and audio (A/U).

---

## 2. The built-in content library

- **Size**: "nearly 2,000" cues in the default workspace. QuickShow 5.0 and
  BEYOND 5.0 each added "more than 1,000 new cues"
  ([QS wiki: 60 cues x 32 pages "nearly 2,000"](https://wiki.pangolin.com/doku.php?id=quickshow:cue_grid),
  [Kvant: "2000 pre programmed cues"](https://www.kvantlasers.co.uk/blogs/news/focus-on-quickshow),
  [BEYOND 5.0 release](https://pangolin.com/blogs/news/beyond-5-0-anniversary-edition)).
- **Organisation**: a workspace holds pages, and each page is one screen of
  the grid (QS: 60 cells; BEYOND: 100). Pages are grouped into
  **categories** shown above the page tabs, so you can show only
  "graphics", for example. Page names follow the content type, for example
  "Abstracts 1" with 60 animated abstracts. An icon on each cell shows the
  cue type.
- **Cue types** (per the icons in the wiki): frames/animations, text, shapes
  (abstracts), timelines, beam sequences, clocks, DMX, synthesised images,
  sequences, captures. BEYOND adds particle, node and FIFO images, 3D objects
  and multi-effects.
- **Content kinds**:
  - static graphics (logos, icons, emoticons, outlines)
  - frame animations (dancers, animals, objects)
  - abstracts (spirographs, Lissajous and oscillator shapes)
  - beam shows (fans, tunnels, cones, lines meant for haze)
  - BeamBrush content
  - effects (QuickFX / FX grid presets)
  - QuickTargets beam sequences
- **Pangolin Cloud**: you browse, preview and download more shows, cues and
  effects inside the software. There is also a third-party paid market, for
  example "Garrett's Workspace" for BEYOND
  ([nicelasers](https://nicelasers.com/products/garrett-workspace-beyond)).

---

## 3. Licensing of the bundled content: verdict

### What the EULA says

Source: [Pangolin License Agreement and Limited Warranty](https://pangolin.com/pages/license-agreement)
(last updated January 2023; it covers all Pangolin software).

- **Definition**: "PPSEs" means the Pangolin-Provided Show Elements,
  "including frames and animations, layouts, bitmap backgrounds, emoticons,
  show instructions, workspaces and other support items". The bundled cue
  library is PPSE.
- **§1**: Pangolin keeps title. You may not "modify, create derivative works
  from, distribute or sublicense the Pangolin Materials". Reverse
  engineering and decompiling are prohibited.
- **§10 (quoted in full)**:
  > "In general, PPSEs may be included in productions that are for your own
  > use, and in productions that you perform for audiences. PPSEs may also be
  > included in frame files, workspace files and similar files that you
  > create and transfer to others who are licensed to use Pangolin software,
  > as long as those files are transferred in original Pangolin file formats.
  > **Any laser projection of PPSEs must be output from Genuine Pangolin
  > Hardware**, such as QM2000, FB3, FB4, and other future Pangolin hardware
  > products.
  > All other uses of PPSEs are prohibited, including but not limited to:
  > (a) directly transferring Pangolin PPSE files to others; (b) indirectly
  > transferring PPSEs via files you create and send to others who are not
  > licensed to use Pangolin software; and **(c) recording PPSEs into
  > laser-projectable formats for later output via non-Pangolin hardware.**
  > This paragraph about PPSEs does not apply to: (a) show elements you
  > create; (b) display of PPSEs in video and film; (c) PPSEs owned by others
  > when transferred with the owner's permission; and (d) PPSEs in the public
  > domain such as ILDA test pattern frames."
- **§16**: prohibits changes made to evade copy protection, and "converting
  image data into formats not reasonably contemplated and expressly
  authorized in writing by Pangolin".
- **Technical measures**: there is no ILDA export, frames carry protection
  flags, and the president of Pangolin states that stopping piracy is the
  purpose (forum links in §1.8).

### Verdict

| Question | Answer |
|---|---|
| Can we copy or ship Pangolin's bundled cues, frames, abstracts, fonts or icons in Laser Studio? | **No.** Redistribution and derivative works are prohibited (§1, §10a/b). |
| Can the user export the library to ILDA and load it into Laser Studio themselves? | **No, not legally.** There is no ILDA export at all. Any workaround would convert PPSEs into a laser-projectable format for non-Pangolin hardware (§10c, §16), and any projection of PPSEs must go through Pangolin hardware. The ShowNET is not Pangolin hardware. |
| Can we trace or redraw their cues "by eye"? | **No.** That is a derivative work (§1). Our CLAUDE.md rules forbid it too. |
| Can we study their manuals, videos and feature lists and reimplement the *ideas*? | **Yes.** Features, UI concepts and generic effect kinds (spirographs, fans, tunnels, scrolling text) are not covered by the EULA, and our rules allow this. |
| Can the user load content **they created themselves** in Pangolin software? | The EULA allows it (§10 exclusion a). In practice it is hard: Pangolin won't export ILDA, and protection flags block the LD2000 route. |
| ILDA test patterns? | Public domain (§10 exclusion d). We can still redraw them ourselves from the ILDA spec rather than copy any file. |

Laser Studio must **never** ship or advertise any "import Pangolin
library / .qsw / .lds" feature, and must not include Pangolin-derived frames
in tests or fixtures.

### What we CAN legitimately do

1. **Generic ILDA import at runtime** (we already have it). The user can load
   `.ild` files they own the rights to, or whose licence allows use with
   other hardware: their own drawings, content bought from ILDA-friendly
   vendors with a licence that allows it, and freely licensed files listed in
   `docs/CONTENT_SOURCES.md`. The UI text should state that the user is
   responsible for the rights. Don't add special handling for Pangolin files.
2. **Reimplement each kind of content as procedural generators**, with our
   own maths and our own parameter names:
   - abstracts: stacked oscillators, Lissajous, hypotrochoid/epitrochoid,
     rose curves
   - beam effects: fans, cones, tunnels, sweeping lines, "liquid sky"
     sheets, chases
   - text: our own Hershey-style font, or a font with an open licence
   - clocks and countdowns
   - parametric shapes: polygon, star, circle, spiral, grid, wave
   - FFT spectrum shapes (bars, rings, dots)
3. **Build our own preset library** of generated looks (the "~2,000 cues"
   experience) as `Settings` JSON. That content is our own.
4. **Tracing**: a bitmap/SVG-to-vector tracer lets users make content from
   their own artwork.

---

## 4. What makes Pangolin "pro": top 10, ranked

Difficulty is for our Rust + browser stack. S = days, M = 1-2 weeks,
L = multiple weeks.

| # | Feature | Why it matters for pro shows | Difficulty for us |
|---|---|---|---|
| 1 | **Projection zones** (projector + geometry + BAM + routing per cue) | One library plays correctly on any surface, beam or audience target, and across many projectors | **M.** Data model plus a per-zone transform pipeline. It fits our `Settings`-to-frame design |
| 2 | **Beam Attenuation Map and safety arming** | Audience scanning is legal only with power limits by area. Operators and venues expect it | **S-M.** A 2D grid or polygons sampled per point, then scale RGB. It must sit last in the pipeline, after all effects |
| 3 | **Geometric correction per zone** (size, position, rotation, keystone, pincushion, bow, shear, linearity, 4-corner mesh) | Every venue is off-axis. Without it graphics look distorted | **S-M.** Point transform maths, plus a UI with test patterns |
| 4 | **Cue grid with instant keyboard/MIDI triggering, pages, categories, click modes** | This is the live operator's main tool (Pangolin: "80% of time is spent in the grid") | **S-M.** Mostly UI. Layering multiple cues needs mixing/concatenating frames |
| 5 | **Layered live control** (cue, master and zone level: size, position, rotation, colour, brightness, speed, scan rate) | The operator adjusts the show "as an instrument" without editing cues | **S.** A stack of affine transforms and colour multipliers. We already have part of it in `Settings` |
| 6 | **Effect engine with beat/time clocks and stacking** (FX grid, waveforms, delay/phase across zones, drop effects) | Turns a small library into endless variation. Everything stays synced to the music | **M-L.** A generic modulator system (LFO shapes, envelopes, beat clock) that targets any parameter, plus ordered effect chains |
| 7 | **Timeline with audio track and timecode** (MTC / Art-Net TC / LTC in, dual seconds/beats clock) | Pre-programmed shows, synced with pyro, video and lighting | **L.** Timeline UI plus audio playback in the browser. MTC in via CoreMIDI and Art-Net TC via UDP are M. LTC decoding is M |
| 8 | **BPM system**: tap tempo, re-sync, Ableton Link, audio beat detection, FFT-driven effects | Every club and DJ use case | **S-M.** Tap/resync is S. FFT and onset detection are M. Link is M (the Rust `rusty_link` crate exists; check its licence) |
| 9 | **Console/controller integration**: DMX/Art-Net/sACN in, MIDI (APC-style map learning), OSC | Integration into a real production with a lighting desk. Remote operation | **M.** Art-Net/sACN UDP listeners and OSC are simple. MIDI learn via `midir`. DMX profile design is the real work |
| 10 | **Scanner-aware output optimisation**: scan rate vs angle, colour shift, blanking/corner dwell, wide-angle compensation, test patterns | Clean, flicker-free images and protected galvos. This is what separates pro output from amateur output | **M.** We already have densify and corner dwell. Add colour-shift delay, angle-dependent point rate, and an ILDA-style test pattern of our own |

Runners-up: content editors (frame/shape/text), bitmap tracing, a 3D
preview, scripting (PangoScript), CITP/NDI, distributed scanning, and
Virtual Laser Jockey (easy, S, and high fun value).

---

## 5. UI/UX patterns worth copying conceptually

1. **Keyboard-mapped cue grid**
   - Each cell shows a thumbnail, a type icon and its key letter.
   - Rows map to keyboard rows (QS: 10x6, using shifted letters for rows
     4-6). F-keys or number keys switch pages.
   - Hovering a cell previews it in a small window without output.
   - Pages are grouped into categories in a filter bar.
2. **Click modes per grid**: Toggle, Flash (momentary), Solo-Flash, Restart,
   Select (edit without triggering). This is essential for live busking.
3. **Groups mode**: cues belong to groups, and only one cue per group plays
   at a time. It gives natural "layers", for example one background abstract
   plus one beam effect plus one text.
4. **Two previews**: a "preview" of the selected cue next to the "live"
   output (in BEYOND, per zone). Previews simulate the look of the laser
   (glow, beam mode for haze).
5. **Effect stacking model** (BEYOND):
   - Effects apply at cue level, then master, then zone, in a fixed order.
   - Each effect has an amplitude or "action" setting, a waveform (linear,
     accelerate, ping-pong, steps, random, custom), damping, delay/phase and
     a clock (seconds or beats).
   - The FX grid has lines. Each line is exclusive ("one"), additive
     ("multi") or one-shot ("drop" = a ripple that decays).
   - Multi-zone time shift runs the same effect with a growing phase delay
     per zone or laser (Q-Shift). This is a cheap, spectacular trick.
6. **Live-control panel** on the right: sliders for size, position,
   rotation, colour and speed. Right-click resets a slider. Speed buttons
   (¼, ½, 1, 2, 4). An optional "physics" setting smooths slider movement.
7. **Global BPM widget**: tap with Space, Backspace re-syncs to beat 1,
   right-click opens settings. The timeline can follow the global BPM or its
   own.
8. **Safety in the toolbar**: a clear Enable Output toggle, and a Blackout
   and Pause that are always visible. UI access levels (beginner /
   intermediate / advanced) hide dangerous settings.
9. **Test patterns and alignment tools** placed right next to the zone
   geometry settings, with a four-corner handle UI ("auto mesh").
10. **Virtual Laser Jockey**: auto-trigger cues from a page, in order or at
    random, every N beats. The operator can still play over it by hand.

---

## 6. Recommendations for Laser Studio

Priority order, sized for the architect to turn into roadmap items:

1. **Keep the IP line clean.**
   - No Pangolin import, no Pangolin-derived fixtures, no "Pangolin-style"
     preset names copied from their library.
   - Add a note in the ILDA import UI that the user must hold rights to
     play the file on non-Pangolin hardware.
   - Put §3 of this report in `docs/CONTENT_SOURCES.md` as the reason
     Pangolin libraries are excluded.
2. **Zones + geometry + BAM as one pipeline stage** (items #1-#3 in §4).
   - Model: `Zone { projector, transform (size/pos/rot/keystone/pincushion/bow/shear), mask/BAM grid, preview style }`.
   - Cues and scenes list their target zones. The BAM is applied last,
     after all effects and live control, and before the brightness scaling
     and -1..1 clamp. Arm/blackout rules stay the same.
   - Start with 1 projector and N zones (preview-only today, ShowNET later).
3. **Cue grid v1**
   - Pages of 10x6, keyboard-mapped, thumbnails rendered from our own
     frames.
   - Toggle and flash modes, groups (one active cue per group), and a
     layered mix of active cues.
4. **Modulator/effect engine**
   - Generic LFO plus envelope objects (waveforms: sine, triangle, saw,
     square, random, step), with a clock set to seconds or beats and a phase
     offset per zone or laser ("Q-Shift").
   - Any numeric `Settings` field can be a target.
   - FX lines with one / multi / drop modes.
5. **Tempo**
   - Tap tempo, re-sync, and beat and bar phase exposed to effects.
   - Next: audio FFT (bands, peaks with smooth fall) as another modulation
     source. Ableton Link and MIDI clock after that.
6. **Procedural content library**, our own. Target a few hundred generated
   presets across categories:
   - abstracts: oscillator stacks, trochoids, roses
   - beams: fans, cones, tunnels, sweeps
   - text and clock
   - parametric shapes
   - spectrum shapes

   Each preset is a saved `Settings` document, so it is ours to ship.
7. **Output quality**: add a colour-shift (colour/galvo delay) setting, a
   point rate that depends on scan angle, and our own ILDA-style test
   pattern for alignment.
8. **Integrations**: OSC in and MIDI learn first (cheap and used by
   MadMapper users too), then Art-Net/sACN DMX in with a simple channel
   profile, then MTC/Art-Net timecode for the timeline.
9. **Timeline v1**: an audio file plus cue and effect events on tracks, and
   dual seconds/beats positioning. Build it after the cue grid and effects
   are stable.
10. **Later / optional**:
    - Virtual Laser Jockey (cheap, fun)
    - UI access levels
    - bitmap/SVG tracing of user artwork
    - distributed scanning
    - 3D preview
    - scripting

---

### Sources (all accessed 2026-09-27)

- Pangolin – QuickShow: https://pangolin.com/pages/quickshow
- Pangolin – BEYOND: https://pangolin.com/pages/beyond
- Pangolin – Compare BEYOND versions: https://pangolin.com/pages/compare-beyond-versions
- Pangolin – License Agreement and Limited Warranty: https://pangolin.com/pages/license-agreement
- Pangolin – QuickShow 5.0 release: https://pangolin.com/blogs/news/quickshow-5-0-official-release
- Pangolin – BEYOND 5.0 release: https://pangolin.com/blogs/news/beyond-5-0-anniversary-edition
- Pangolin – Laser show safety: https://pangolin.com/pages/laser-show-safety
- Pangolin – Audience scanning safety: https://fr.pangolin.com/blogs/education/audience-scanning-safety
- Pangolin – PASS: http://pangolin.com/PASS/ and manual https://pangolinlegacy.com/PASS/PASSmanual.pdf
- Pangolin – BAM video: https://www.youtube.com/watch?v=89xLGkicUjI
- Wiki – QuickShow manual index: https://wiki.pangolin.com/doku.php?id=quickshow:start
- Wiki – QuickShow cue grid: https://wiki.pangolin.com/doku.php?id=quickshow:cue_grid
- Wiki – QuickShow projection zones: https://wiki.pangolin.com/quickshow:projection_zones
- Wiki – QuickShow files and extensions: https://wiki.pangolin.com/doku.php?id=quickshow:files_and_file_extensions
- Wiki – QuickShow music & beats: https://wiki.pangolin.com/doku.php?id=quickshow:music_and_beats_overview
- Wiki – QuickShow timeline & BPM: https://wiki.pangolin.com/doku.php?id=quickshow:timeline_bpm
- Wiki – QuickShow Virtual Laser Jockey: https://wiki.pangolin.com/doku.php?id=quickshow:virtual_laser_jockey
- Wiki – QuickShow latest features: https://wiki.pangolin.com/doku.php?id=quickshow%3Alatest_features
- Wiki – BEYOND latest features: https://wiki.pangolin.com/doku.php?id=beyond%3Alatest_features
- Wiki – BEYOND effects: https://wiki.pangolin.com/doku.php?id=beyond%3Aeffects
- Wiki – BEYOND live control: https://wiki.pangolin.com/doku.php?id=beyond:livecontrol
- Wiki – BEYOND workspace grids and pages: https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages
- Wiki – BEYOND timecode: https://wiki.pangolin.com/doku.php?id=beyond%3Atimecode
- Wiki – BEYOND realtime audio: https://wiki.pangolin.com/doku.php?id=beyond%3Arealtime_audio
- Wiki – BEYOND projector settings: https://wiki.pangolin.com/doku.php?id=beyond%3Aprojector_settings-new
- Forum – Missing ILDA export in QuickShow (W. Benner, 2010): https://forums.pangolin.com/threads/missing-ilda-export-function-in-quickshow.799/
- Forum – Exporting .ild from 5.6 (2016): https://forums.pangolin.com/threads/exporting-ild-from-5-6.2755/
- Forum – BEYOND frame protection (W. Benner, 2018): https://forums.pangolin.com/threads/beyond-frame-protection-export-to-ld2000-export-to-ild.15413/
- Forum – 3D import in QuickShow: https://forums.pangolin.com/threads/any-kind-of-3d-import-possible-in-quickshow.756/
- Kvant – Focus on QuickShow: https://www.kvantlasers.co.uk/blogs/news/focus-on-quickshow
- Photonlexicon – QuickShow to ILDA thread: https://www.photonlexicon.com/forums/archive/index.php/t-11750.html
