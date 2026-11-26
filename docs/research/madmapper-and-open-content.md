# Research: MadMapper/MadLaser, free laser content, open-source laser software, ILDA, point optimisation

Research agent report for Laser Studio. Checked 2026-09-27. Public sources
only: product pages, public manuals and PDFs, public GitHub repositories and
crate metadata. Nothing was decompiled. The code in `studio/src` was read so
the recommendations fit what already exists. It has `densify` with
`MAX_STEP = 0.03`, `CORNER_DWELL = 3` and `CORNER_ANGLE_DEG = 30`, uses
laser-dac `FrameSession` at 30 kpps, and has a 7-segment-style font.

> IP reminder (CLAUDE.md): this document describes **ideas and public
> specifications**. Code from GPL/LGPL/non-commercial projects listed below
> is for *reading to understand concepts only*. Do not copy it into the repo.
> Content marked "do not bundle" must not be committed. Users can load it at
> runtime from their own copy.

---

## A. MadMapper / MadLaser

### A.1 What it is

MadLaser is a paid extension for MadMapper 5+ (from €19/month, or €199
perpetual). The free trial watermarks the output and blacks out DMX every
30 s. It brings the video-mapping workflow to lasers. You create
**Laser Outputs** (projectors), add **Laser Surfaces** to them, and each
output turns the coloured polylines it receives from its surfaces into an
ILDA point frame.
Sources: [MadLaser product page](https://madmapper.com/extensions/madlaser),
[MadLaser Guide (PDF)](https://madmapper.com/files/MadLaser%20Guide.pdf),
[CDM article](https://cdm.link/2021/10/madmapper-will-now-let-you-control-lasers-with-madlaser-beta/).

### A.2 Feature inventory

| Area | MadLaser feature (public docs) | Notes for us |
|---|---|---|
| Outputs | Unlimited laser outputs. Each one has a destination DAC (Ether Dream, **ShowNET**, Helios USB, FB3/FB4 via Beyond, LaserCube, IDN, AVB/Dante), output size, flip X/Y. | ShowNET support confirms Laserworld has an SDK for third parties. This fits our plan to wait for the official API. |
| Output timing | **PPS** (manufacturer value; the guide says ">45 kpps generally doesn't make sense" except for far/small-angle projection). **Desired FPS**: "we don't notice the scan over 45 FPS, under 35 we really see the blinking". Live **Point Count** and **ILDA FPS = PPS / point count** readouts (e.g. 30 kpps / 500 pts = 60 FPS). | Cheap to add and very useful. |
| Safety | **Safety Area**: a rectangle the beam never leaves. **Masks**: polygons the beam may not enter. **Mask opacity** < 100% dims instead of blanking, for places where audience scanning is allowed below a set level. **Invert mask** turns a mask into a "safe to shoot" zone. Option to render the safety area and mask outlines on the laser for alignment. | Our calibration clamp is a rectangle only. Masks and zones are the next safety step. |
| Output colour | **Color Levels** (per-channel R/G/B gain). Per-diode **minimum voltage / cut-off** so near-black doesn't show as red. **Time Shift** per colour channel, in ILDA points (colour delay versus XY, and between diodes). | Maps directly to a per-output "colour calibration" panel. |
| Blanking | **Blank Delay** (100% = a tuned default, adjustable per device). | |
| Surfaces | **Laser Quad** (perspective + mesh warp; vectorises video/images or hosts vector content), **Laser Lines** (hand-drawn/bezier paths, SVG import, preset quad/triangle/circle, convert text or quad content to editable lines), **Laser Text**, groups, soft edge on quads, 3D OBJ render (5.2+). | |
| Per-surface render settings | **Max Speed** (scan time spread evenly over paths by length, so long and short paths are equally bright). **Optimize Angles** with **Angle Min** (0–90°) and **Angle Delay**. **End Repeat**. **In Fade / Out Fade**, which remove the "hot spot" at path start and end caused by galvo inertia. **Point Intensity**: dwell for zero-length paths (beams). **Min Points** per path (Lines). | A good checklist for our `densify` settings. |
| Draw order | Optimises draw order per frame by default, and warns that the order can change between frames and cause flicker. Offers **Preserve Order**. | Worth copying: stable ordering plus an opt-out. |
| Text | Single-stroke **stick fonts** (bundled OneLineFonts, which are commercial) or system outline fonts. Italic, horizontal/vertical align, direction (RTL), fit-to-box, font size, wrap modes. Text can be set over OSC. | We need a real single-stroke font (see section B). |
| Video → laser | Two GPU modes. **Find Paths**: threshold, use colour, skeletonise to a "thickness", denoise, max resolution. **Find Contours**: Canny with threshold, blur size and Canny size. **Path filtering**: min/max length as % of the media size, and "keep the N longest/shortest paths". Gives up when there are more than 2000 paths. Surface FX run *before* vectorisation. | Longer term. Path filtering is useful for any generated content. |
| Generators | **Laser Materials**: GLSL functions called N times per frame (default 8192), `laserMaterialFunc(pointNumber, pointCount) → pos, color, shapeNumber, userData`. A change in `shapeNumber` starts a new path. ISF-style JSON parameters become UI controls. The previous frame is available (`mm_LastFrameData`) for feedback and damping. Render hints: `POINT_COUNT`, `MAX_SPEED`, `SKIP_BLACK`, `PRESERVE_ORDER`, angle optimisation, `FIRST/LAST_POINT_REPEAT`, `POLY_FADE_IN`. ([LaserMaterialsDoc.md](https://github.com/madmappersoftware/MadMapper-Materials/blob/main/LaserMaterialsDoc.md); the repo has **no licence**, so read it for ideas only) | A good model for our generator API (see Recommendations). |
| Multi-laser | "Laser dispatch": a composition projector publishes an internal loopback, and **Dispatch Count** splits its paths across N projectors. | |
| ILDA | ILDA import/export. **Store ILDA frame** snapshot. **Record ILDA movie** as "fixed frame rate" (for replay in MadMapper) or "ILDA stream" (for SD-card playback on hardware, where a frame's duration depends on its point count). | This matters for our exporter. |
| Control | MIDI (notes, CC, 14-bit pitch bend, motorised feedback), OSC with OSCQuery and "Copy OSC Address" on any parameter, DMX in (Art-Net, sACN, USB), gamepads, Ableton Link. **Learn mode** colour-codes mappable items (keyboard blue, MIDI green, OSC yellow, DMX purple). Right-click any slider → "Add Control". Central **Control List** (Cmd+L). ([docs](https://docs.madmapper.com/madmapper/6/11.-live-performance-and-control)) | |
| Audio | Amplitude, bass, mid, treble, **bpm**, **beatCount**, beat divisions (`1_beats`, `4_beats`, `16_beats`…). | We have level, bass and beat. Adding beat divisions and a BPM clock is cheap. |
| Show structure | MadMapper 6: **Timelines** replace cues. Every parameter can be animated. Audio tracks, OSC/MIDI out tracks, markers, **BPM sync and quantised launch**. A **Conductor** timeline syncs to MTC, LTC or Art-Net timecode. ([what's new](https://docs.madmapper.com/madmapper/6/what's-new), [CDM v6](https://cdm.link/madmapper-timelines-clips-v6/)) | |

### A.3 UX ideas worth borrowing (reimplemented, no copying)

1. **Output vs content separation.** Settings for the projector (PPS,
   FPS target, safety area, colour levels, time shift, flip) are kept
   apart from the look (shapes, effects). Our `Calibration` is already
   close. Add PPS, target FPS, colour and time-shift settings there.
2. **Live health readout**: points per frame, real frame rate
   (PPS / points), and a warning when the frame rate drops under 35 fps
   (flicker).
3. **Per-content render settings** with good defaults and an "expert"
   unlock: max speed, corner angle and dwell, end repeat, fade in/out,
   beam intensity.
4. **Safety zones as first-class objects.** Show them in the preview,
   optionally draw them with the laser for alignment, and support opacity
   (dimming) as well as hard blanking.
5. **Right-click → Learn** on every control, with the protocol shown by
   colour, and one list of all mappings.
6. **Quantised launch.** Scenes and cues switch on the next beat or bar.
7. **Stable draw order** by default, so the optimiser doesn't cause
   flicker.
8. **Path filtering** (min/max length, keep the N longest) as a general
   post-process for heavy content (imported ILDA, SVG, vectorised video).

---

## B. Freely usable laser content

Strict rule: if the licence isn't explicit, **do not bundle**. "Bundle"
means we may commit the file (or data derived from it) and ship it, after
listing it in `docs/CONTENT_SOURCES.md`.

### B.1 Fonts (single-stroke / vector)

| Source | URL | Contents | Licence (exact) | Bundle? |
|---|---|---|---|---|
| **Hershey fonts** (USENET distribution by James Hurt, `.jhf`) | e.g. [kamalmostafa/hershey-fonts](https://github.com/kamalmostafa/hershey-fonts) (`hershey-fonts/*.jhf`); background: [Paul Bourke](https://paulbourke.net/dataformats/hershey/), [Wikipedia](https://en.wikipedia.org/wiki/Hershey_fonts) | ~2000 single-stroke glyphs: Roman simplex/duplex/triplex, script, Gothic, Greek, Cyrillic, symbols. Ideal for lasers. | Hershey Font License ([Fedora wiki](https://fedoraproject.org/wiki/Licensing:HersheyFontLicense), [Ghostscript](https://web.mit.edu/ghostscript/www/Hershey.htm)): "may be used by anyone for any purpose, commercial or otherwise, providing that: 1. The following acknowledgements must be distributed with the font data: The Hershey Fonts were originally created by Dr. A. V. Hershey while working at the U. S. National Bureau of Standards. The format of the Font data in this distribution was originally created by James Hurt, Cognition, Inc. … 2. The font data in this distribution may be converted into any other format *EXCEPT* the format distributed by the U.S. NTIS." | **Yes**, with the acknowledgement text in `CONTENT_SOURCES.md` and next to the data. Take only the `.jhf` data files, **not** the kamalmostafa C library (GPL-2). Converting to our own Rust/JSON format is allowed. |
| **Hershey SVG fonts (EMS / Hershey Text v3)** | [gitlab.com/oskay/svg-fonts](https://gitlab.com/oskay/svg-fonts) | SVG 1.1 stroke fonts: classic Hershey faces plus "EMS" single-line derivatives of OFL Google fonts (EMS Readability, EMS Tech, EMS Osmotron…). | Per-file metadata. Hershey faces fall under the Hershey licence. EMS fonts are "derivatives created from fonts licensed under the SIL Open Font License". | **Yes, per font**, after reading each file's embedded licence. Hershey and EMS OFL faces are OK. **Not** EMS SpaceRocks (Asteroids font; see below). |
| **Relief SingleLine** | [isdat-type/Relief-SingleLine](https://github.com/isdat-type/Relief-SingleLine) | Modern sans-serif single-line font. SVG font (`ReliefSingleLineSVG-Regular.svg`), open-path TTF/OTF, UFO source. Made "for a pen, laser, or milling tool". | `OFL.txt`: "Copyright 2022 The Relief SingleLine Project Authors … licensed under the SIL Open Font License, Version 1.1". No Reserved Font Name is declared. | **Yes**. Ship `OFL.txt` alongside. The OFL forbids selling the font on its own; bundling it in software is fine. Best-looking option for text. |
| Mistral SingleLine | [isdat-type/Mistral-SingleLine](https://github.com/isdat-type/Mistral-SingleLine) | Cursive single-line font. | OFL per repo (same project family). **Verify `OFL.txt` before use.** | Probably yes, after checking. |
| Asteroids vector font (Trammell Hudson / EMS SpaceRocks) | [trmm.net/Asteroids_font](https://trmm.net/Asteroids_font) | Arcade-style stroke font. | No explicit licence on the page; derived from Atari's design notes. | **Do not bundle.** |
| OneLineFonts (bundled with MadMapper) | onelinefonts.com | Stick fonts. | Commercial. | **Do not bundle.** |
| `hershey` crate | [codeberg kicad-rs/hershey](https://codeberg.org/kicad-rs/hershey) | Parser only. | Apache-2.0 OR LGPL-3.0 | Could be used as a dependency, but a `.jhf` parser is about 50 lines. Writing our own is simpler. |

### B.2 ILDA files and test patterns

| Source | URL | Contents | Licence | Bundle? |
|---|---|---|---|---|
| **ILDA Test Pattern** (official) | [ilda.com/technical.htm](https://www.ilda.com/technical.htm) (zip with 11 sample/test files, including 12K and 30K patterns). Procedure: [ILDA_TestPattern95_rev002.pdf](https://www.ilda.com/resources/StandardsDocs/ILDA_TestPattern95_rev002.pdf) | Scanner tuning frames. | "©1995 International Laser Display Association. All rights reserved. For reproduction permission contact ILDA's Executive Director." The site says: "No reproduction … without written permission". | **Do not bundle.** Let users download it and load it through ILDA import. We may generate our **own** tuning pattern procedurally, using our own geometry and not a copy of ILDA's frame. |
| OpenLase `test_patterns/ILDA12K.ild` | [marcan/openlase](https://github.com/marcan/openlase/tree/master/test_patterns) | The ILDA 12K test pattern. | Repo is GPL-2/3, but the frame is ILDA's copyrighted work, so its provenance is unclear. | **Do not bundle.** |
| rorosaurus/ild-archive | [github](https://github.com/rorosaurus/ild-archive) | Links to and copies of ~500 `.ild` files "found on the interweb" (laser-am.com, laserfx.com archive, cuttingedgesamples). | No licence. Third-party files. | **Do not bundle.** Fine for local manual testing of the importer if the user downloads them. |
| laserfx.com archive, laser-am.com, photonlexicon FTP | see ild-archive README | Classic ILDA frames. | Unclear or none. | **Do not bundle.** |
| Showeditor free shows (Laserworld) | showeditor.com | Shows. | Laserworld content (IP rule). | **Never.** |
| animated-lines.com | | Royalty-free animations. | Commercial marketplace; the licence covers use, not redistribution. | **Do not bundle** (a user may buy and load them). |

**Conclusion:** no ILDA frame library we found has a redistribution
licence. Our bundled content should stay **procedural** (our generators)
plus text in Hershey or Relief fonts. For importer tests, **generate test
`.ild` files in code** (write formats 0, 1, 2, 4 and 5 from our own
points). That avoids licensing entirely.

---

## C. Open-source laser software to learn from

| Project | Licence (verified) | Worth learning | Can we copy code? |
|---|---|---|---|
| **laser-dac** 0.13.1 (we use it) — [ModulaserApp/laser-dac-rs](https://github.com/ModulaserApp/laser-dac-rs) | MIT (crate metadata; no LICENSE file in repo root) | Frame mode already does **inter-frame transition blanking** (`default_transition`): 100 µs end dwell + quintic ease-in-out transit of `ceil(32 · L∞ distance)` points (≤64) + 400 µs start dwell, using the **L∞ distance** because the galvo axes are independent. **Colour delay** default 150 µs. **Startup blank** 1 ms after arming. Output filter hook runs after blanking and colour delay. Catmull-Rom resampler for audio DACs. Supports Helios, Ether Dream, IDN, LaserCube (USB/net), oscilloscope, AVB. | Yes (MIT), but we already depend on it. **Don't double-apply** colour delay or frame-to-frame blanking in `engine.rs`. |
| **lasy** — [nannou-org/lasy](https://github.com/nannou-org/lasy) | MIT OR Apache-2.0 | Complete implementation of *Accurate and Efficient Drawing Method for Laser Projection* (Abderyim et al., [paper](https://art-science.org/journal/v7n4/v7n4pp155/artsci-v7n4pp155.pdf)): segment graph → Euler circuit (draws shared edges once, minimises blanks), draw order optimisation, blank delay, angle-based corner delay. Defaults: `distance_per_point = 0.1`, `blank_delay_points = 10`, `radians_per_point = 0.6`, minimum blank of 3 points. | **Yes** (permissive). A candidate dependency, or a reference to port from with attribution. |
| **ilda-idtf** — [nannou-org/ilda-idtf](https://github.com/nannou-org/ilda-idtf) | MIT OR Apache-2.0 | "A complete implementation of … IDTF Revision 011". Reads and writes all formats. Zero-copy section reader. | **Yes**. The best replacement for the `ilda` crate (see D.3). |
| **nannou_laser** 0.20 / **ether-dream** crate — [nannou](https://github.com/nannou-org/nannou), [nannou-org/ether-dream](https://github.com/nannou-org/ether-dream) | MIT OR Apache-2.0 | Streaming architecture, lasy integration, Ether Dream protocol plus a DAC emulator (useful for tests without hardware). | Yes. |
| **OpenLase** — [marcan/openlase](https://github.com/marcan/openlase) | GPL-2/3 as a whole; some files LGPL-2.1/3 | The classic realtime laser renderer. `OLRenderParams`: `on_speed`, `off_speed`, `start_wait`, `start_dwell`, `corner_dwell`, `curve_dwell`, `end_dwell`, `end_wait`, `curve_angle`, `flatness`, `snap`, `max_framelen`. Example values at 48 kpps: on_speed 2/100, off_speed 2/20, start_wait 8, start_dwell 3, corner_dwell 8, end_dwell 3, end_wait 7, curve angle 30°. Also has a video tracer (`trace.c`, edge following) and audio-reactive tools. | **No** (GPL). Parameter names and numbers are facts we can use. |
| **lzr** — [brendan-w/lzr](https://github.com/brendan-w/lzr) | LGPL-3.0 | Optimiser pipeline: 1) decimate existing interpolation and blanks, 2) split into lit paths at blanks or hard angles (≥45°), 3) solve a min-time traversal where closed cycles can be entered at any point, 4) reassemble, 5) interpolate. Defaults: lit step = 1% of full range, blank step = 20% of full range, anchors 1 lit + 2 blanked. ILDA reader/writer, Ether Dream driver. | **No** (LGPL; avoid copying into a static Rust binary). Take the algorithm idea. |
| **Helios DAC SDK** — [Grix/helios_dac](https://github.com/Grix/helios_dac) | `sdk/`: MIT. firmware/hardware: MIT + Commons Clause (non-commercial) | Limits: max 4095 points/frame (USB), PPS 7–65535. IDN mode: 8192 points, 100 kpps. Frame-swap model, not FIFO. | SDK yes (MIT). laser-dac already covers it. |
| **j4cDAC** (Ether Dream firmware) — [j4cbo/j4cDAC](https://github.com/j4cbo/j4cDAC) | No licence detected | Protocol reference. | **No** (no licence). Use the nannou crate or laser-dac. |
| **LaserCube / Laserdock** — [Wickedlasers/libLaserdockCore](https://github.com/Wickedlasers/libLaserdockCore) | GPL-3.0 | Many built-in visualisers and audio-reactive generators. LaserOS app design. | **No** (GPL). Ideas only. |
| **ofxLaser** — [sebleedelisle/ofxLaser](https://github.com/sebleedelisle/ofxLaser) | Custom **non-commercial share-alike** licence; font files under Apache-2.0 (`FONT_LICENSE.txt`) | Very good multi-laser UX: per-laser zones, zone warping, scanner presets (speed and acceleration per scanner type), colour calibration, canvas → zones. | **No** for code. Its Apache-2.0 font *might* be usable, but check which font it is and its upstream licence first. |
| **lukasjapan/ilda-tools** | MIT | CLI tools for ILDA files (convert, inspect). | Yes. |
| **colouredmirrorball/Ilda** (Processing) | No licence detected | ILDA read/write/render. | No. |
| **roymacdonald/ofxIldaFile** | No SPDX detected | ILDA read/write. | No. |
| **laser-dac (Node, legacy)** — [ModulaserApp/laser-dac](https://github.com/ModulaserApp/laser-dac) | MIT | JS scene graph for lasers: shapes, SVG, text via Hershey, ILDA, simulator in the browser. Close to our browser-preview idea. | Yes (MIT), for reading and porting ideas. |
| **ilda** crate 0.2.0 (we use it in pc-client) — [echelon/ilda.rs](https://github.com/echelon/ilda.rs) | **BSD-4-Clause** (crate metadata; no LICENSE file in repo). The 4th ("advertising") clause is GPL-incompatible and awkward. | See D.3: has bugs. | Replace it. |

---

## D. ILDA Image Data Transfer Format (IDTF rev. 011, 2014)

Source: [ILDA_IDTF14_rev011.pdf](https://www.ilda.com/resources/StandardsDocs/ILDA_IDTF14_rev011.pdf)
(© ILDA. We summarise it and must not copy it into the repo.)

### D.1 Structure

A file is a sequence of **sections**. Each section is a 32-byte header
followed by N records. **All multi-byte values are big-endian.** The file
ends with a header that has a frame format code and **0 records**.

Header (byte numbers 1-based as in the spec):

| Bytes | Field |
|---|---|
| 1–4 | ASCII `ILDA` |
| 5–7 | reserved (write 0, don't test on read) |
| 8 | **format code** (0, 1, 2, 4, 5) |
| 9–16 | frame/palette name (8 ASCII chars, stop at first NUL) |
| 17–24 | company name (8 chars) |
| 25–26 | number of records (u16). 0 = end of file. Palettes: 2–256 |
| 27–28 | frame / palette number (0–65534) |
| 29–30 | total frames in sequence (1–65535; 0 for palettes) |
| 31 | **projector number** (0–255) |
| 32 | reserved |

Records:

| Format | Size | Layout |
|---|---|---|
| 0 – 3D indexed | 8 B | X i16, Y i16, Z i16, status u8, colour index u8 |
| 1 – 2D indexed | 6 B | X, Y, status, colour index |
| 2 – palette | 3 B | R, G, B |
| 4 – 3D true colour | 10 B | X, Y, Z, status, **B, G, R** |
| 5 – 2D true colour | 8 B | X, Y, status, **B, G, R** |

- Coordinates are signed 16-bit. X: −32768 is left, +32767 right. Y:
  −32768 is bottom, **+32767 top** (y up). Z: +32767 is towards the viewer.
- Status byte: **bit 7 = last point** of the image, **bit 6 = blanking**
  (1 = laser off). Bits 0–5 are 0. When reading, **blanking takes
  precedence over colour**: blanked points are treated as RGB 0.
- Format 3 was never approved. Readers **SHALL** read 0, 1, 2, 4 and 5. A
  file may mix formats.
- Palettes are **per projector**. A format 2 section applies to all
  following indexed frames for that projector until another palette
  replaces it. Without a palette, use a default. Appendix A gives a
  64-colour "suggested default palette" (LFI/Aura): reds→yellow 0–15,
  16–23 yellow→green, 24–30 green→cyan, 31–39 cyan→blue, 40–47
  blue→magenta, 48–55 magenta→white, 56–63 white→pink. The ILDA test
  pattern uses it.
- The format carries **no timing**: no PPS, no frame duration. It is
  point-sampled data meant to go straight to the scanners ("NOT raw
  vector information").

### D.2 Import pitfalls

1. **No PPS or FPS in the file.** Ask the user, or default to 30 kpps. A
   frame lasts `points / pps`. That is "stream" playback, which is how SD
   players behave (see the MadLaser guide). Offer "fixed FPS" playback as
   an alternative.
2. **The data is already optimised** for some scanner. Don't run it
   through full corner dwell and interpolation again. At most apply output
   transforms, colour and safety, and optionally resample when the PPS
   differs.
3. **The end-of-file header** has 0 records. Don't turn it into an empty
   frame.
4. **Truncated or garbage files** are common. Never index slices without
   bounds checks. Stop at the first bad header and keep the frames read so
   far.
5. **Palettes**: apply them per projector and in order. Palettes may have
   fewer than 256 entries. Indices past the palette end fall back to the
   default palette or white.
6. **BGR order** in formats 4/5, which is easy to get backwards.
7. **Black but not blanked** points: some exporters write RGB 0 without the
   blank bit. Treat them as blanked for travel optimisation.
8. **Projector number**: multi-projector files interleave frames for
   several projectors. Filter by projector, or play projector 0 by
   default.
9. **Names are not NUL-terminated** when all 8 bytes are used. Headers may
   hold non-ASCII bytes.
10. **Frame number / total frames** are often wrong. Rely on file order.
11. **Y is up** in ILDA. Our normalised space should match (+y = top).
    Check the preview canvas flip.
12. Max 65535 points per frame, while Helios USB accepts 4095. Large
    imported frames may need decimation, or a lower frame rate, per output.
13. Some old software uses **format 1 with the colour index only** and
    never sets the blank bit correctly. Offer a "treat index 0/black as
    blank" import option.

### D.3 Does the `ilda` crate 0.2.0 cover it? Partly, with bugs

I read the source in `~/.cargo/registry/.../ilda-0.2.0/src`:

| Item | Status |
|---|---|
| Formats 0, 1, 4, 5 parsed | Yes. Blank bit = `status & 64`. BGR order is right for format 5. |
| **Format 4 colour bug** | `TrueColorPoint3d::read_bytes` reads `b: bytes[7], g: bytes[8], r: bytes[9]`. The record offset `j` is missing, so **every point in a format 4 section gets the colour of the first point.** |
| **Format 2 palettes** | Parsed, but `Animation::read_*` → `ilda_entry_to_point` returns `IldaError::Unsupported` for palette entries. **Any file that contains a palette fails to load completely.** |
| Default palette | Uses the 64-entry Appendix A palette (matches the spec). Index ≥ 64 → white. |
| Projector number | Reads byte index 31, but the spec puts it at byte 31 one-based, which is index **30**. Index 31 is the reserved byte. Not exposed in `Animation` anyway. |
| End-of-file header | Becomes a trailing **empty frame** (the header opens a new frame). |
| Truncated files | `&bytes[i..i+end]` **panics** on a short file. That is fine for a CLI, but a panic inside the studio server is not. |
| Z coordinate, last-point bit | Ignored (fine). |
| Writing | Not supported. |
| Licence | BSD-4-Clause (advertising clause). |
| Maintenance | Last push 2019. |

**Verdict:** don't use it for the studio. Either use **`ilda-idtf`**
(MIT/Apache, full rev 011, reads and writes), or write our own reader and
writer of about 200 lines in `studio/src/ilda.rs`. The spec is small, and
owning it lets us handle the pitfalls above and fuzz it. Unit tests should
generate files in all five formats in code.

---

## E. Point optimisation: best practice and concrete numbers

### E.1 Reference numbers found

| Parameter | Value | Source |
|---|---|---|
| Scanner tuning speed | **30 kpps at ≤ 8° optical** (or 12 kpps at ≤ 15°) with the ILDA test pattern, ±1% PPS | ILDA Test Pattern rev 002 |
| Sensible PPS ceiling | "Over 45 kpps generally doesn't make sense" unless far away or at a small angle. MadLaser allows up to 100 kpps in expert mode | MadLaser Guide |
| Flicker threshold | ≥ 45 fps invisible; < 35 fps clearly flickers | MadLaser Guide |
| Points per frame budget | pps / fps → 30 000 / 60 = **500**, / 40 = 750, / 30 = 1000 | derived |
| Lit step (max distance per point) | OpenLase 2/100 of full range (0.02 normalised, at 48 kpps). lzr 1% of range (0.02). lasy 0.1. **Ours 0.03** | code/docs above |
| Blank step | OpenLase 2/20 (0.1 normalised). lzr 20% of range (0.4). laser-dac `ceil(32·L∞)` eased points (≤64) | above |
| Blank dwell before the move (end of lit path) | OpenLase `end_dwell` 3 + `end_wait` 7 @48k. laser-dac 100 µs (3 pts @30k). lzr 2 blanked anchors | above |
| Blank dwell after the move (before lighting) | OpenLase `start_wait` 8 + `start_dwell` 3 @48k. laser-dac **400 µs** (12 pts @30k). lasy 10 pts | above |
| Corner dwell | OpenLase `corner_dwell` 8 @48k for angles > 30°. lasy ≈ `ceil(angle_rad / 0.6)` (90° → 3, 180° → 6). **Ours: flat 3 for > 30°** | above |
| Colour (blanking) delay | laser-dac default **150 µs** (≈ 4–5 pts @30k). Typically 0–300 µs depending on diode driver/modulation. MadLaser lets each colour have its own shift in points | laser-dac, MadLaser |
| Startup blank | laser-dac 1 ms after arm | laser-dac |
| Helios frame limit | 4095 points (USB), 8192 (IDN) | Helios SDK |

To convert OpenLase's 48 kpps counts to 30 kpps, multiply by 0.625:
start_wait ≈ 5, corner_dwell ≈ 5, end_wait ≈ 4.

### E.2 Recommended pipeline for `engine.rs`

Content produces a list of **paths**, each a polyline with a colour and a
closed/open flag. Then:

1. **Clip to the safety area and masks.** Split paths at zone edges. Dim
   instead of cutting for "opacity" zones.
2. **Order the paths.** Greedy nearest-neighbour from the current beam
   position. For open paths, allow reversal. For closed paths, allow
   starting at any vertex (rotate the loop). Measure distance with **L∞**
   (the axes move independently). Keep the order **stable between frames**:
   seed the order from the previous frame and only re-optimise when the
   cost improves by more than about 20%, with a "preserve order" option.
   Option for later: a lasy-style Euler-circuit graph optimisation for
   meshes of shared edges (grids, wireframes).
3. **Lit segments.** Interpolate at `lit_step` (keep **0.02–0.03**; make it
   a setting derived from the scanner speed: `lit_step ≈ v_max / pps`).
   Split evenly so the beam speed is constant, which keeps brightness even.
4. **Corners.** Dwell for `ceil(turn_angle_rad / 0.6)` repeats when the
   angle is above 30°, capped at about 8. This replaces the flat 3.
   Curves made of many tiny angles get no dwell.
5. **Path ends.** Repeat the first point 1–3 times (lit) and the last point
   2–3 times (lit, optionally with out-fade). An optional **in-fade** over
   the first few points removes hot spots.
6. **Blank travel.** Dwell blanked at the source for about 100 µs (3 pts),
   travel with an **eased** profile at a larger step (0.1–0.2, or
   laser-dac's 32·L∞ rule), then dwell blanked at the destination for about
   300–400 µs (9–12 pts @30k). *Today `densify` moves blanked at the same
   0.03 step with no ease. That is safe but wastes points.*
7. **Beams / zero-length paths.** Hold them for `beam_points` (a
   user-controlled "intensity").
8. **Budget check.** If the total exceeds `pps / min_fps` (e.g.
   30 000 / 35 ≈ 850), increase `lit_step` by up to 2× and warn in the UI.
9. **Output stage (laser-dac).** Colour delay (150 µs default, make it a
   setting), inter-frame transition blanking and startup blank are
   **already done by laser-dac `FrameSession`**. Don't add them again.
   Add per-channel gain and **minimum-level cut-off** (diode threshold)
   in `to_laser_point`.

Scale all counts from **microseconds** × pps, not fixed point counts, so
the settings stay correct when PPS changes (laser-dac already does this).

---

## Recommendations for Laser Studio

Prioritised, and small enough to be roadmap items:

1. **Replace ILDA import** (the `ilda` crate is buggy: format 4 colours,
   palette files fail, panics on truncation, BSD-4). Either add
   `ilda-idtf` (MIT/Apache) or write `studio/src/ilda.rs` reading all of
   formats 0, 1, 2, 4 and 5 with the pitfalls in D.2 handled. Write support
   (format 5 plus the EOF header) enables "record/export ILDA". Tests
   generate their own `.ild` bytes, so no third-party files are needed.
   ILDA playback **bypasses** corner dwell and interpolation.
2. **Make `densify` parameters settings, not constants**, and time-based:
   `lit_step` (0.03 default), `blank_step` (0.15), blank pre/post dwell
   (100/400 µs), angle-proportional corner dwell (0.6 rad per point,
   30° min, cap 8), end repeat, in/out fade. Put them behind an
   "Avancé" panel in the UI. Keep `#[serde(default)]`.
3. **Path ordering with stable order** (greedy + reversal + loop rotation,
   L∞ cost, hysteresis). Start with text: glyph strokes are the worst case
   today.
4. **Live stats in the UI**: points per frame, effective FPS
   (pps / points), and a warning under 35 fps. Show the PPS setting.
5. **Output colour calibration**: per-channel gain, minimum diode level
   cut-off, and a configurable colour delay (pass it through to laser-dac
   instead of re-implementing it).
6. **Safety zones** after the rectangle clamp: polygon masks (blank) and
   dimming zones (opacity), shown in the preview, with an optional "draw
   zone outline" alignment mode. It must never be possible to disable them
   while armed without confirmation.
7. **Real single-stroke font**: import **Hershey** (`.jhf`, Roman simplex
   and duplex first) and/or **Relief SingleLine** (OFL, SVG font) into a
   generated Rust table. Add both to `docs/CONTENT_SOURCES.md` with the
   Hershey acknowledgement text and `OFL.txt`. Keep the 7-segment font as
   a fallback. Add lowercase and accents (the UI is French).
8. **Generator API modelled on Laser Materials** (idea, not code):
   `fn sample(i, n, t, audio, params) -> (x, y, rgb, path_id)` with
   declarative parameters that become UI sliders automatically, plus access
   to the previous frame. It is a clean way to grow the effect library
   procedurally, which is IP-safe.
9. **Audio clock**: add beat divisions (1/4/16), a BPM estimate or tap,
   and **quantised scene switching** on beat or bar. Add MIDI/OSC learn via
   right-click later.
10. **Own tuning pattern**: a procedurally generated calibration frame
    (square + inscribed circle + centre cross + blanking marks) drawn at a
    fixed PPS for scanner checks. **Don't ship ILDA's test pattern file.**
    Tell users where to download it (ilda.com) and load it via import.
11. **Consider `lasy`** (MIT/Apache) as a dependency or port reference for
    items 2–3, instead of re-deriving the Abderyim algorithms.

### Content licence summary

| Can bundle (with attribution) | Do not bundle |
|---|---|
| Hershey fonts (`.jhf`, Hershey licence, acknowledgement required, not in NTIS format) | ILDA test pattern files (© ILDA, all rights reserved) |
| Relief SingleLine (OFL 1.1) | OpenLase `ILDA12K.ild` (provenance) |
| EMS SVG fonts derived from OFL fonts (check each file) | ild-archive, laserfx, laser-am, photonlexicon collections (no licence) |
| Our own procedural content and generated test `.ild` files | Asteroids / EMS SpaceRocks font, OneLineFonts, Showeditor/Pangolin/MadMapper content |
