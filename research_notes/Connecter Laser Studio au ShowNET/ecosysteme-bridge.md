# ShowNET software ecosystem and bridge options for an external open-source app

Scope: which software drives the **Laserworld ShowNET** (network DAC, not the
Strand/ETC "ShowNet" DMX protocol) in real time, and whether any of them can act
as a **bridge** that receives a live frame stream from an external app ("Laser
Studio") and forwards it to the ShowNET — ideally for free. Current as of 2026-10.

Note on terminology: "Laserworld ShowNET" is a laser network DAC/mainboard.
"Strand ShowNet" is an unrelated lighting/DMX-over-Ethernet protocol. All findings
below are about the Laserworld product.

---

## Key Question 1 — Every software that drives the ShowNET live, and whether each accepts EXTERNAL live input

### Takeaway
Laserworld's own compatibility page lists eight apps that drive the ShowNET
directly over Ethernet: Showcontroller, Showeditor, MadMapper/MadLaser,
TouchDesigner, ILD Render, Millumin, Modulaser, and CloudLase Studio. Of these,
the ones that accept a **live frame stream from another app** are MadMapper
(PONK in and out), TouchDesigner (PONK), and Modulaser (NDI/Syphon video).
The Laserworld-native tools (Showeditor, Showcontroller) and the others appear to
run their own content, not an external live vector feed.

### Cited Findings
- Laserworld's ShowNET compatibility page lists eight apps: **Showcontroller, Showeditor, MadMapper/MadLaser, TouchDesigner, ILD Render, Millumin, Modulaser, CloudLase Studio**; it says compatible software "sends laser-control data directly to ShowNET over Ethernet" — [Laserworld ShowNET compatible](https://laserworld.com/en/shownet-compatible.html)
- **MadMapper/MadLaser** natively supports ShowNET; MadLaser "instantly detects" ShowNET-equipped lasers over LAN, multiple units via a switch, no extra license beyond MadMapper/MadLaser — [Laserworld: MadLaser supports ShowNET](https://www.laserworld.com/en/newslist/5605-madlaser-natively-supports-shownet-laser-mainboard.html)
- **MadMapper accepts and emits PONK as a live laser-path stream**: the Outputs tab has a "Publish to PONK" option that sends laser data over the network; PONK ("Pathes Over NetworK") is "a minimal protocol to transfer 2D colored pathes from a source to a receiver," developed with the MadMapper team — [MadMapper Outputs docs](https://docs.madmapper.com/madmapper/6/6.-outputs); [PONK on derivative.ca](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438)
- MadMapper for laser output "natively supports Etherdream, ShowNet, Helios USB, and Pangolin FB4" — [MadMapper Outputs tab](https://docs.madmapper.com/madmapper/6/6.-outputs)
- **TouchDesigner** has "integrated direct laser output support" for ShowNET mainboards: it auto-detects ShowNET on the network, needs only a standard LAN, and "eliminates the need for additional licenses or extra hardware components"; up to 40 mainboards per LAN — [Laserworld: TouchDesigner supports direct output to ShowNET](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- TouchDesigner's **Laser Device CHOP** sends points to "EtherDream, Helios, and ShowNET"; for ShowNET the device menu auto-fills with units found on the network — [Laser Device CHOP docs](https://docs.derivative.ca/Laser_Device_CHOP)
- **TouchDesigner implements PONK**: the PONK TouchDesigner integration "enables exchange of laser paths between TouchDesigner and MadMapper, and between multiple instances of either software"; it was built "in close collaboration with the MadMapper team, who fully funded the work on the TouchDesigner version," written in C++, free on GitHub (Apache-2.0) for macOS and Windows — [PONK on derivative.ca](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438); [Ponk GitHub](https://github.com/madmappersoftware/Ponk)
- The PONK GitHub repo contains a **"Touch Plugin" folder**, indicating the TouchDesigner PONK support is distributed as a plugin (likely a compiled C++ custom operator) rather than documented as a native TOX in the readme — [Ponk GitHub](https://github.com/madmappersoftware/Ponk)
- **Modulaser** accepts live video input: "Pipe any NDI or Syphon video source into your laser patch"; it supports ShowNET on macOS and Windows (not Linux); it is node-based with multi-laser routing, cues, and timeline — [modulaser.app](https://modulaser.app)
- **ILD Render** is described by Laserworld as "real-time laser output from Blender and Inkscape workflows," i.e. it takes output from those external tools — [Laserworld ShowNET compatible](https://laserworld.com/en/shownet-compatible.html)
- **Showeditor / Showcontroller / Millumin / CloudLase Studio**: the Laserworld page does not state that any accepts an external live frame stream; Showeditor/Showcontroller descriptions point to their own content creation and timeline/live control — [Laserworld ShowNET compatible](https://laserworld.com/en/shownet-compatible.html)
- Third-party confirmation PONK is an open cross-app standard: Liberation Laser "implemented PONK, the Paths Over NetworK protocol made by Madmapper"; and Derivative hosts the TouchDesigner version — [Liberation Laser forum](https://forum.liberationlaser.com/t/using-4-x-etherdream-with-madmapper-and-liberation/2144); [PONK on derivative.ca](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438)

### Inferences
- Three distinct **live-input "bridge" mechanisms** exist in the ShowNET ecosystem: (a) **PONK** (vector laser paths, UDP) into MadMapper or TouchDesigner; (b) **NDI/Syphon** (raster video) into Modulaser; (c) **ILD Render** consuming Blender/Inkscape. PONK is the only one that carries native vector laser frames; NDI/Syphon would force Laser Studio's vector frames through a raster video round-trip (lossy for laser).
- For an external vector-frame app like Laser Studio, the natural bridges are **MadMapper (PONK → ShowNET)** — the user's existing working path per project notes — and **TouchDesigner (PONK → ShowNET)**. TouchDesigner is the main *alternative* bridge not yet exploited.
- Pangolin BEYOND/QuickShow is **not** in Laserworld's ShowNET-compatible list; Pangolin drives its own FB3/FB4 hardware, so it is not a ShowNET bridge.

### Gaps
- Laserworld publishes no single authoritative statement of which listed apps accept external live input; the "accepts external input" column is inferred from each app's own docs, not from Laserworld.
- Whether Showcontroller can ingest any live frame stream (vs only its own timeline/figures) is not documented in sources found.

---

## Key Question 2 — Pricing and free-tier limits for LIVE laser output to a ShowNET

### Takeaway
No app offers unrestricted free live ShowNET output. MadMapper's free demo blacks
out laser every 120 s and can't save projects. Modulaser's free trial is the most
generous (ShowNET works, no expiry) but caps output at 15 min per run and 2 lasers.
TouchDesigner's free Non-Commercial edition drives ShowNET natively with no extra
license, but is licensed for non-commercial use only. Showeditor is free but ties
you to Laserworld's own content/tools.

### Cited Findings
- **MadMapper + MadLaser demo (free):** includes all features, but **video output/input is watermarked, DMX channels blackout every 30 s, and laser output is blacked out every 120 s**; projects can't be saved/loaded and SVG/PNG/PDF export is disabled (per the general MadMapper FAQ) — [MadMapper madbundle page](https://www.madmapper.com/madbundle); [MadMapper FAQ](https://docs.madmapper.com/madmapper/6/faqs)
- **MadMapper + MadLaser paid:** rental from **49€/month (excl. tax)**; perpetual "Artists" license from **519€ (excl. tax)**, one-time, valid forever for the current 6.xx version; both include all MadLaser features — [MadMapper madbundle page](https://www.madmapper.com/madbundle)
- MadLaser requires a MadMapper license (MadLaser is not usable from the free demo as a full product) — [MadMapper FAQ](https://docs.madmapper.com/madmapper/6/faqs)
- **Modulaser trial (free):** no expiry, full editor, **output limited to 15 minutes per run (restart to continue), up to 2 lasers**; ShowNET supported on macOS/Windows; the page does not mention a watermark — [modulaser.app](https://modulaser.app)
- **Modulaser paid:** **Standard €179 one-time** (unlimited output time, up to 2 lasers, 2 device activations); **Pro €499 one-time** (unlimited output and lasers, 5 activations) — [modulaser.app](https://modulaser.app)
- **TouchDesigner Non-Commercial (free):** free for **non-commercial use only** (if paid for your work, a paid license is required); output capped at 1280×1280; excludes some Pro operators (Shared Memory, C++ CHOP/TOP, SDI OPs); single-use keys, 10 per account; forum support only — [TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)
- TouchDesigner's direct ShowNET output "eliminates the need for additional licenses" — i.e. no laser-specific surcharge on top of the TouchDesigner license — [Laserworld: TouchDesigner + ShowNET](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- TouchDesigner paid: Educational **$300 USD**, Commercial **$600 USD** (lifts resolution cap, adds streaming) — [TouchDesigner Non-Commercial / licensing](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)
- **Showeditor (free):** Laserworld bundles a free Showeditor licence with every ShowNET-equipped system and external ShowNET interface, and it's downloadable free from the Showeditor website — [Laserworld Showeditor set](https://www.laserworld.com/laser-software/showeditor-set)

### Inferences
- The ~1280×1280 resolution cap on TouchDesigner Non-Commercial affects raster/video TOPs; a laser path is a vector/CHOP stream, so that cap is unlikely to meaningfully limit laser output — but this is not explicitly confirmed for the Laser Device CHOP.
- Confirmed: Modulaser's "15 min per run" free tier is real and ShowNET works on it; MadMapper demo's periodic blackout is real and explicitly applies to laser (every 120 s), not only DMX.

### Gaps
- Whether the TouchDesigner Non-Commercial license excludes or restricts the Laser Device CHOP / ShowNET output is not stated anywhere found (the exclusion list names Shared Memory, C++, SDI OPs — not laser). Needs direct confirmation from Derivative.
- Modulaser's trial page is ambiguous about whether "Export and record" is actually enabled on the trial (conflicting wording on its own pricing table).

---

## Key Question 3 — Do Showeditor/Showcontroller take live frame streams, or only triggers of pre-made figures?

### Takeaway
The Laserworld-native playback path (ShowNET standalone + Showeditor/Showcontroller)
is built around **pre-made ILDA figures/shows triggered or modulated by DMX/Art-Net**,
plus the box's own timeline/live control — not an external live vector frame stream
from a third-party app. The only live third-party frame path onto a ShowNET is via
PONK (MadMapper/TouchDesigner) or NDI/Syphon (Modulaser), not via Showeditor.

### Cited Findings
- ShowNET can store ILDA files on its microSD card and play them back; playback works in automatic mode **and allows individual DMX triggering of patterns**, with intensity/color/size/offset adjustable over **DMX512** without saving a modified figure — [Laserworld ShowNET + Showeditor listing (Thomann)](https://www.thomannmusic.com/laserworld_shownet_showeditor_software.htm)
- ShowNET control features advertised: "Stand alone operation, DMX, ArtNET, ILDA Streaming, and direct computer control via LAN" — [Laserworld: MadLaser supports ShowNET](https://www.laserworld.com/en/newslist/5605-madlaser-natively-supports-shownet-laser-mainboard.html)
- Showeditor "supports DMX input and output: it can be remote controlled through a DMX controller and it can control lighting fixtures of any kind" — i.e. DMX is a *control/trigger* channel, not a frame feed — [Laserworld Showeditor set](https://www.laserworld.com/laser-software/showeditor-set)
- A ShowNET can act as "DMX IN - to - LAN - to DMX OUT adapter" — a bridge for DMX, not for laser frames — [ShowNET + Showeditor listing](https://www.thomannmusic.com/laserworld_shownet_showeditor_software.htm)
- "Fully compatible to the professional Showcontroller laser software" is the only Showcontroller detail found; no source describes Showcontroller ingesting an external live frame stream — [ShowNET listing](https://www.thomannmusic.com/laserworld_shownet_showeditor_software.htm)
- "ILDA Streaming" in the ShowNET feature list is the box's own live point feed when driven by the controlling app over LAN — not an open input other apps can push into — [Laserworld: MadLaser supports ShowNET](https://www.laserworld.com/en/newslist/5605-madlaser-natively-supports-shownet-laser-mainboard.html)

### Inferences
- "ArtNet / DMX" on the ShowNET carries **DMX control and figure-selection/parameter data**, consistent with the project's own docs (Art-Net picks an SD-card file; DMX modulates it). This matches the user's T-301/T-302 SD-card + Art-Net path, which is a trigger model, not a live external vector stream.
- Showeditor/Showcontroller therefore cannot serve as a bridge for Laser Studio's live frames; they only consume their own content plus external DMX/Art-Net *triggers*.
- "ILDA Streaming" is ambiguous marketing wording; in context it is the LAN live-output mode the single controlling application uses, encrypted, reachable only through Laserworld's licensed integrations/SDK — not an open socket other software can stream into.

### Gaps
- No primary Showcontroller manual was found confirming definitively that it cannot ingest any live external frame stream; conclusion rests on absence of any such feature in all sources plus the DMX/Art-Net trigger model.

---

## Key Question 4 — Definitive verdict: any FREE/open way to get an external app's live frames onto the ShowNET?

### Takeaway
There is **no fully free, unrestricted, officially-open live path** to the ShowNET
from a third-party app today, because the ShowNET's own live stream is encrypted and
reachable only through Laserworld-licensed integrations or the (NDA) SDK. BUT there
are two legitimate **free-to-use (with strings) bridges** that already accept an
external live vector stream (PONK) and drive the ShowNET: **TouchDesigner
Non-Commercial** (free for non-commercial use, native ShowNET output, PONK input) and,
with interruptions, **Modulaser's free trial** (NDI/Syphon input, 15-min-per-run cap)
and **MadMapper's demo** (PONK input but laser blacks out every 120 s). A genuinely
unlimited, commercial-grade path requires a paid license (Modulaser €179, MadMapper
519€) or the official Laserworld SDK.

### Cited Findings
- The open, cross-app live-frame protocol in this ecosystem is **PONK** (Apache-2.0, free, macOS+Windows), carrying 2D colored laser paths over UDP, implemented by MadMapper/MadLaser and TouchDesigner — [Ponk GitHub](https://github.com/madmappersoftware/Ponk); [PONK on derivative.ca](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438)
- **Both PONK-capable bridges also drive the ShowNET natively**: MadMapper (ShowNet native output) and TouchDesigner (direct ShowNET output, no extra license) — [MadMapper Outputs](https://docs.madmapper.com/madmapper/6/6.-outputs); [Laserworld: TouchDesigner + ShowNET](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- **Free-with-strings today:**
  - TouchDesigner Non-Commercial = **free but non-commercial use only**; drives ShowNET with no laser surcharge — [TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial); [Laserworld: TouchDesigner + ShowNET](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
  - Modulaser trial = **free, no expiry, ShowNET works, but 15 min per run + 2 lasers**, input via NDI/Syphon video — [modulaser.app](https://modulaser.app)
  - MadMapper demo = **free but laser blacks out every 120 s**, no save — [MadMapper madbundle](https://www.madmapper.com/madbundle)
- **Unlimited requires payment or SDK:** Modulaser Standard €179 / Pro €499; MadMapper+MadLaser 519€ perpetual or 49€/mo — [modulaser.app](https://modulaser.app); [MadMapper madbundle](https://www.madmapper.com/madbundle)
- The ShowNET's direct live protocol is encrypted/closed; only Laserworld-licensed apps (MadMapper via its licensed ShowNET integration, TouchDesigner, etc.) or the official SDK reach it — consistent with Laserworld releasing a separate ShowNET API/SDK for third-party developers (Windows x86/x64 and macOS Intel/M1) — [Laserworld ShowNET API/SDK available](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html)

### Inferences
- **The single best free live-vector bridge for Laser Studio is TouchDesigner Non-Commercial**: Laser Studio emits PONK → a small TouchDesigner patch receives the PONK paths → Laser Device CHOP outputs to ShowNET. This keeps the user on the ShowNET they already own, requires no new DAC hardware, and costs nothing for non-commercial use. The one unverified link is whether the PONK TouchDesigner plugin (a compiled custom operator) loads in the Non-Commercial edition — the Non-Commercial exclusion list names C++ CHOP/TOP, Shared Memory, SDI, which may or may not block a third-party precompiled custom operator; this must be tested.
- **Modulaser** is the cheapest *paid* unlimited route (€179) and its free trial is genuinely usable for testing, but feeding Laser Studio's vector frames in requires a Syphon/NDI raster round-trip (Laser Studio would publish video, Modulaser rasterizes to laser) — a quality downgrade versus PONK vectors, and still output-time-limited on the free tier.
- **MadMapper remains the proven path** (the project's current working route: PONK → MadMapper → ShowNET) but is only free in a non-show-usable demo (120 s laser blackouts); real use needs the 519€/49€-mo license.
- Net: **"free + unlimited + commercial-legal + lossless vector"** does not exist for ShowNET without the Laserworld SDK. The closest free option is TouchDesigner Non-Commercial (free, PONK vectors, non-commercial only); the closest cheap-unlimited is Modulaser €179. No open-source app can talk to the ShowNET's live protocol directly — a licensed bridge or the SDK is mandatory because the stream is encrypted.

### Gaps
- Unconfirmed whether the PONK TouchDesigner custom-operator plugin runs under the free Non-Commercial license (needs a hands-on test or a Derivative/author statement). This is the pivotal unknown for the "free bridge" verdict.
- No pricing/terms found for CloudLase Studio, Millumin, or ILD Render specific to live ShowNET output, nor whether any of them accept a PONK/NDI/Syphon external live input (ILD Render clearly takes Blender/Inkscape, but not a generic network stream).
- Modulaser's input is documented as NDI/Syphon video only; no evidence it accepts PONK vector paths, so a direct lossless vector bridge via Modulaser is not currently supported.
