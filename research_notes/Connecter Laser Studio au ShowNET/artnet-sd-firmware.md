# Driving the Laserworld ShowNET for free via SD-card ILDA files triggered over Art-Net / DMX

> Scope: the free, standalone, no-MadMapper path — pre-rendered ILDA files on the ShowNET's SD
> card, selected and triggered live over DMX512 / Art-Net from the user's own software (Laser
> Studio). Current as of 2026. User's unit: external ShowNET interface, firmware **2016050502**,
> bootloader 2015052200, HW-ID 4. **Not** Strand Lighting "ShowNet" (an unrelated DMX-over-Ethernet
> lighting protocol); everything here is Laserworld ShowNET, the laser mainboard / network DAC.

## Q1 — How SD-card playback works (files/slots, format, size limits)

### Takeaway
The ShowNET has an internal microSD card that plays standard ILDA `.ild` files named `001.ild`,
`002.ild`, … Pattern selection is by number: DMX value 1 → `001.ild`, 2 → `002.ild`, and so on, so
up to 255 slots are addressable from a single DMX channel. Files must be ILDA format code 4/5 RGB
(frame-based). The card ships preloaded and the content is fully user-replaceable.

### Cited Findings
- The board has a "MicroSD card for ILDA file playback (ILDA format code 4/5 RGB)." — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- The SD card "is equipped ex works with a variety of patterns and animations in ILDA format," and this content can be replaced with the user's own files. — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- Custom files are made with the free Showeditor software, which exports **ILDA standard 5 (`*.ild`)** files for the SD card; the interface also reads standard ILDA files from any laser software that exports them. — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- Pattern selection maps the DMX value directly to the file number: "**1 = 001.ild, 2 = 002.ild … and so on. If no pattern is assigned to a number = blackout.**" Value 0 = blackout and "000.ild must not exist." — [Laserworld DMX Table — ShowNET Laser Mainboard, Professional Mode (PDF, firmware 2021010602)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf)
- Scan speed and frame rate for the SD-card player are set in the **ShowNET Admin Tool → "DataSet" tab**, then "Store Data"; Laserworld warns "Do NOT push these settings too hard as they can damage the scanning system." — [Laserworld FAQ: scan speed / frame rate for the SD-card player](https://www.laserworld.com/en/laserworld-showeditor-faq/2577-how-can-i-set-the-scan-speed-and-frame-rate-for-the-sd-card-player-in-my-shownet.html)
- The pattern/frame rate (playback fps) is also settable live over DMX on channel 3: 128 = standard 50 fps, 0–127 scales 0→50 fps, 129–255 scales 50→100 fps. — [Laserworld DMX Table — Professional Mode (PDF)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf)

### Inferences
- The single-channel "value = file number" scheme caps live-selectable patterns at **255** (values 1–255). This matches the repo's own convention (CLAUDE.md / `sdbank.rs` T-302: export `001.ild`–`229.ild`, keep `000` empty, reserve `230`–`255`); the DMX table independently confirms `000.ild must not exist`.
- "ILDA format code 4/5" = ILDA format 4 (3D true-colour RGB) / format 5 (2D true-colour RGB) frames. Laser Studio should export **format 5** (2D RGB), which is what the repo already targets.

### Gaps
- No published hard limit on per-file point count, file size, or total card capacity was found. The Admin Tool's DataSet scan-speed/fps setting is the only documented playback constraint.
- Whether file numbering must be zero-padded (`001.ild`) or plain (`1.ild`) is not stated explicitly beyond the `001.ild` examples; the repo already uses zero-padded three-digit names.

## Q2 — Selecting/triggering patterns via DMX512 / Art-Net; the channel profile; live switching

### Takeaway
Yes — live, real-time pattern switching is a first-class feature. In **Professional Mode** the
ShowNET exposes a ~34-channel DMX personality where **channel 2 selects the SD-card file by number**
and can be changed on every DMX frame (i.e. on the beat, from a sequence, etc.), while other
channels set intensity, playback speed, position, size, rotation, colour, strobe, scan speed and
safety zones in real time. The same profile is driven identically whether the data arrives as
physical DMX512 or as Art-Net.

### Cited Findings — full Professional-Mode channel map (firmware 2021010602)
All from the [Laserworld DMX Table — ShowNET Laser Mainboard, Professional Mode (PDF)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf):

| Ch | Function | Values |
|----|----------|--------|
| 1 | **Intensity** | 0 = laser off; 1–255 = increase to full (255) |
| 2 | **Pattern selection** | 0 = blackout (`000.ild` must not exist); 1–255 = `001.ild`…`NNN.ild`; unassigned number = blackout |
| 3 | **Pattern speed (framerate)** | 128 = 50 fps standard; 0–127 = 0→50 fps; 129–255 = 50→100 fps |
| 4 | Position X coarse (Setup: Position X) | 0 left → 255 right, 128 center |
| 5 | Position X fine | 0–255 |
| 6 | Position Y coarse (Setup: Position Y) | 0 bottom → 255 top, 128 center |
| 7 | Position Y fine | 0–255 |
| 8 | Rotation coarse | 0–255, 0° = 128 |
| 9 | Rotation fine | 0–255 |
| 10 | Size X-axis | 0 max → 255 min |
| 11 | Size Y-axis | 0 max → 255 min |
| 12 | Inversion | 0–63 none; 64–127 invert X; 128–191 invert Y; 192–255 invert X+Y |
| 13 | **Color selection** | 0–15 original pattern colours; 16–207 hue sweep red(16)→yellow(48)→green(80)→cyan(112)→blue(144)→purple(176); 208–223 white half intensity; 224–239 white full; 240–255 enables ch14–16 custom RGB |
| 14 | Red | 0–255 (only if ch13 = 240–255) |
| 15 | Green | 0–255 (only if ch13 = 240–255) |
| 16 | Blue | 0–255 (only if ch13 = 240–255) |
| 17 | Strobe | 0–15 none; 16–255 increasing strobe |
| 18 | Scan speed | 0–15 default; 16–255 increasing (≈5 kpps → 40 kpps) |
| 19 | Safety zone size | quadrant/size selection (bottom/upper/left/right) |
| 20 | Safety zone intensity | 0 = max intensity → 255 = none |
| 21 | Color balancing Red (white balance; master to ch14) | 0 = max → 255 = none |
| 22 | Color balancing Green (master to ch15) | 0 = max → 255 = none |
| 23 | Color balancing Blue (master to ch16) | 0 = max → 255 = none |
| 24 | Setup: Mode Select (with ch25) | 134–135 Setup; 204–205 Save; 211–212 Reset; else Show mode |
| 25 | Setup: Mode Select (with ch24) | 94–95 Setup; 168–169 Save; 32–33 Reset; else Show mode |
| 26 | Setup: Geo correction trapezoid X | 0–255 |
| 27 | Setup: Geo correction trapezoid Y | 0–255 |
| 28 | Setup: Geo correction barrel X | 0–255 |
| 29 | Setup: Geo correction barrel Y | 0–255 |
| 30–34 | **Reserved** — must not be assigned | — |

Additional:
- The chart's header states these settings (safety zones, colour balancing, geometric corrections) can be configured straight from the DMX console and **hard-stored back onto the ShowNET mainboard over DMX** using the ch24/ch25 Setup/Save combination — effectively a non-volatile calibration write via DMX. — [Laserworld DMX Table — Professional Mode (PDF)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf)
- Laserworld markets exactly this live behaviour: stored files "can be triggered individually over DMX and Art-Net," and intensity, base colour, size, offset etc. can be set over DMX "without saving a separate modified file." — [Laserworld search summary / ShowNET mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- The ShowNET mainboard lists multiple control options including "**Professional DMX (two modes), Professional Art-Net (two modes)**, Stand-Alone Operation, Master-Slave, ILDA Streaming Receiver, LAN." — [Laserworld: ShowNET PRO Interface](https://www.laserworld.com/en/laser-software/laserworld-shownet-pro-interface.html)
- There is also a **"ShowNET trigger mode"** (simple DMX trigger) distinct from Showeditor/Showcontroller streaming control. — [Laserworld: DMX Adapter for external ShowNET](https://www.laserworld.com/en/laser-accessories/dmx-adapter)

### Inferences
- **Live switching is exactly the intended use.** Because ch2 is a plain level that maps directly to a file number, Laser Studio can change the pattern on any Art-Net frame — on the beat, via a chase/sequence, via timecode — the same way it would change any other DMX parameter. Combined with ch1 intensity, ch3 speed, ch13 colour and ch8 rotation, this gives a genuinely "live-ish" show from pre-rendered looks.
- Laserworld advertises "two" professional DMX modes and "two" Art-Net modes. The published 34-channel table is almost certainly the full "Professional Mode"; the second mode is likely a reduced/basic channel set. The exact basic-mode map was not found (see Gaps).
- For the repo: this 34-channel map is the authoritative personality for `artnet.rs` (T-301) and the DMX profile; note it is **firmware 2021010602** — older firmware may expose fewer channels or a different layout.

### Gaps
- The second ("basic/simple") DMX mode and the two Art-Net modes' exact channel counts/maps were not found in public docs. Only the full Professional-Mode table is published.
- The Art-Net universe/net/subnet addressing and start-address configuration for pattern triggering (as opposed to DMX-OUT conversion) is set in the Admin Tool but the exact menu path / default universe was not pinned down publicly.

## Q3 — Firmware: does the Art-Net/DMX trigger mode need newer firmware? Can the old unit update?

### Takeaway
The advanced DMX/Art-Net trigger features (DMX profiles / Professional Mode, safety zones, colour
balancing, RGB picker) are **firmware-dependent and were added in newer firmware**. The published
Professional-Mode table is firmware **2021010602** (2021); the user's unit runs **2016050502**
(2016), which almost certainly predates Professional Mode. Updating is therefore the key
prerequisite. Flashing is done with the Windows-only **ShowNET Admin Tool**, then a power-cycle.
Laserworld states firmware is "suitable for any ShowNET-equipped laser system," but does **not**
publish any HW-ID / bootloader minimum, so whether a 2016/HW-ID-4 unit updates cleanly must be
confirmed with Laserworld.

### Cited Findings
- "Newer firmware adds features, including the DMX profiles, safety zones, colour balancing, and RGB colour picker support" — i.e. the professional DMX/Art-Net feature set is explicitly firmware-gated. — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- The authoritative Professional-Mode DMX chart is labelled **"Firmware version: 2021010602."** — [Laserworld DMX Table — Professional Mode (PDF)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf)
- Latest published firmware is **2025120902** (09.12.2025), "suitable for any ShowNET-equipped laser system"; it fixes a rare full-scale pixel-cut. Prior build **2025043002** (30.04.2025) "changes the behavior of the scaling in DMX / Art-Net mode." — [Laserworld firmware 2025120902](https://www.laserworld.com/en/download-file-2037-application_shownet_2025120902.html); [Laserworld firmware 2025043002](https://www.laserworld.com/en/download-file-2018-application_shownet_2025043002.html)
- Flashing procedure: open the Admin Tool, select the firmware file from the menu, confirm the dialog, then **reboot by disconnecting from mains and powering back on**. The related download is "ShowNET Admin Tool 1.40." — [Laserworld firmware 2025120902](https://www.laserworld.com/en/download-file-2037-application_shownet_2025120902.html)
- Switching the control mode (DMX vs Art-Net, DMX IN vs OUT, data source) is also done in the **Admin Tool**, followed by "Store Data." — [Laserworld manual: more features of ShowNET](https://www.laserworld.com/en/laser-online-user-manual/shownet-display-panel/more-features-of-shownet.html)

### Inferences
- The user's **2016050502** firmware is ~5 years older than the Professional-Mode table (2021). It very likely offers only basic stand-alone/auto playback and possibly a simpler DMX trigger, **not** the full 34-channel Professional Mode. To get live Art-Net pattern switching with the documented channel map, **the unit should be updated to current firmware first.**
- Because each firmware page says builds suit "any ShowNET-equipped laser system," an old external HW-ID-4 unit is probably updatable — but Laserworld publishes no explicit compatibility matrix, so this is not guaranteed.
- Per CLAUDE.md hard rules, flashing is a **user action** performed on Windows with the Admin Tool; agents/Laser Studio never flash. The repo's docs already note the SDK and Art-Net mode need newer firmware than 2016050502.

### Gaps
- No public document states the minimum firmware version that first introduced "Professional Mode" / the 34-channel personality, nor confirms a 2016 HW-ID-4 unit can reach it. Confirm with Laserworld (ties to the open questions in `docs/research/shownet.md`).
- Firmware file extension (`.lff` was asked): the download pages label files as "ShowNET Firmware <date>"; the exact on-disk extension was not shown in the fetched pages.

## Q4 — Is a "ShowNET DMX adapter" hardware required, or can the mainboard take Art-Net directly?

### Takeaway
Critical distinction for the free path: **physical DMX512 cabling requires the Laserworld DMX
Adapter** on an external ShowNET (the external box has no DMX ports). But **Art-Net travels over the
ShowNET's own Ethernet port and needs no adapter** — the board has an integrated Art-Net node. So
Laser Studio sending Art-Net over the LAN can trigger SD-card patterns **without buying any extra
hardware.** The DMX Adapter is only needed if the user wants to feed/emit real DMX512 on XLR.

### Cited Findings
- "The standard ShowNET interface has no DMX connections… To trigger patterns and animations (.ild files) stored on the SD card over DMX512, you need a DMX adapter." The adapter adds DMX512 In/Out and plugs between the ShowNET's ILDA OUT and the short ILDA cable to the laser. — [Laserworld FAQ: use DMX to trigger the external ShowNET](https://www.laserworld.com/en/shownet-faq/5894-how-can-i-use-dmx-to-trigger-the-external-shownet-interface.html); [Laserworld: DMX Adapter](https://www.laserworld.com/en/laser-accessories/dmx-adapter)
- The DMX Adapter "adds a DMX 512 in and out connector to the external ShowNET interface" and "can also be used for ArtNET to DMX and DMX to ArtNET signal conversion" — i.e. its Art-Net role is converting to/from *physical* DMX for other fixtures, not a requirement for receiving Art-Net. — [Laserworld: DMX Adapter](https://www.laserworld.com/en/laser-accessories/dmx-adapter)
- "ShowNET has an integrated ArtNET node, so lasers with integrated ShowNET as mainboard can simply be connected via LAN cable." The default is DMX operation; "to switch it to ArtNET mode, use the ShowNET Admin Tool." — [Laserworld: ShowNET PRO Interface](https://www.laserworld.com/en/laser-software/laserworld-shownet-pro-interface.html)
- Built-in "Art-Net-to-DMX converter" / integrated Art-Net node is a standard ShowNET feature. — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)

### Inferences
- **Free path = Art-Net over Ethernet, no adapter.** The user already reaches the box at an IP on the LAN; sending Art-Net DMX to it (after setting Art-Net mode in the Admin Tool) is the zero-hardware route. The DMX Adapter is only relevant for the SD-card-trigger path via physical DMX512 consoles, or to use the laser as a DMX *controller* for haze/lights.
- Caution: Laserworld's "more features" page describes an Admin Tool "Data source for physical DMX Out → ArtNET to DMX" setting that is about the *DMX-OUT* (fixture control) feature, which is a different thing from putting the board into Art-Net-**input** trigger mode. The operator must select the correct Art-Net control mode, not the DMX-out conversion mode.

### Gaps
- The fetched manual page conflates Art-Net-to-DMX *output* conversion with Art-Net *input* control; the precise Admin Tool steps to put the board into "Professional Art-Net" input/trigger mode (vs DMX mode) were not captured verbatim. Needs the ShowNET Admin Tool manual / Laserworld confirmation.

## Q5 — Physical SD access: is the card user-removable to copy files from a computer?

### Takeaway
The microSD is an internal card preloaded and fully replaceable by the user. Sources indicate card
contents are "accessed via LAN," suggesting files can be managed over the network (likely via the
Admin Tool / Showeditor) rather than only by physical removal. Whether the card is physically
removable on the external box is not clearly documented.

### Cited Findings
- The internal microSD "holds ILDA files for playback… and it can be accessed via LAN." — [Laserworld/retailer product summaries](https://djcity.com.au/product/laserworld-shownet-laser-control-system/)
- Content is user-replaceable: the preloaded patterns/animations "can be replaced" with the user's own `.ild` files. — [Laserworld: ShowNET as laser mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- Scan-speed/fps for SD playback is configured over LAN through the Admin Tool (no card removal needed for that). — [Laserworld FAQ: scan speed / frame rate](https://www.laserworld.com/en/laserworld-showeditor-faq/2577-how-can-i-set-the-scan-speed-and-frame-rate-for-the-sd-card-player-in-my-shownet.html)

### Inferences
- File transfer is most likely done over the network (Admin Tool / Showeditor "upload to SD" workflow), consistent with "accessed via LAN." Physical removal (opening the housing, card reader on a PC) may also be possible but is not the documented method and risks the IP67/rainproof housing seal on outdoor units.

### Gaps
- No source confirms whether the external ShowNET's microSD is reachable without opening the case, nor the exact tool used to push `.ild` files onto it over LAN. Confirm via the Admin Tool manual / Laserworld. This directly affects how Laser Studio's exported `NNN.ild` bank (sdbank.rs, T-302) gets onto the card.

## Q6 — Net verdict: can the user get a usable, live-ish free show this way?

### Takeaway
**Yes.** The ShowNET natively supports exactly this: pre-rendered ILDA looks on the SD card,
selected and modulated live over Art-Net from the user's own software, with no MadMapper and no
extra hardware (Art-Net rides the existing Ethernet). The one real prerequisite is a **firmware
update** from 2016050502 to a current build that supports Professional Art-Net/DMX mode — a
Windows Admin Tool operation the user performs manually.

### Exact steps (free Art-Net path)
1. **Update firmware** (user, Windows): run ShowNET Admin Tool (≥1.40), flash current firmware (e.g. 2025120902), power-cycle. This unlocks Professional Mode / DMX profiles, safety zones, colour balancing. *(Agents never flash — CLAUDE.md.)* — [firmware page](https://www.laserworld.com/en/download-file-2037-application_shownet_2025120902.html)
2. **Load the SD card**: put Laser Studio's exported `001.ild`–`229.ild` (format 5, `000` empty, `230`–`255` reserved) onto the card — over LAN via the Admin Tool/Showeditor, or by card reader. Set scan speed/fps in Admin Tool → DataSet.
3. **Set the box to Art-Net mode** in the Admin Tool (default is DMX), confirm a universe/DMX start address, "Store Data."
4. **Drive it live from Laser Studio** by sending Art-Net DMX on the ShowNET's 34-channel Professional personality: ch1 intensity, **ch2 = file number (change on the beat/sequence/timecode for live switching)**, ch3 speed, ch8 rotation, ch10/11 size, ch13(+14–16) colour, ch17 strobe, ch18 scan speed, ch19/20 safety zones.
5. **Safety**: "disarm/blackout" for this path = **intensity 0 (ch1=0) AND pattern 0 (ch2=0)** every tick; keep sending dark frames. This is not a live point stream — it is file-select + parameter modulation. Hardware stop (interlock/E-stop/key) remains the real stop (CLAUDE.md).

### Cited Findings
- All capabilities above are vendor-confirmed: individual DMX/Art-Net triggering of SD files with live intensity/colour/size/offset — [ShowNET mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html); full channel map — [DMX Table Professional Mode (PDF)](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf); Art-Net node over LAN, no adapter — [ShowNET PRO](https://www.laserworld.com/en/laser-software/laserworld-shownet-pro-interface.html).

### Inferences
- This maps cleanly onto the repo's already-planned path: `sdbank.rs` (T-302) exports the `NNN.ild` bank and `artnet.rs` / `--shownet-artnet` (T-301) picks the file over Art-Net DMX. The research confirms the design is correct and documents the exact channel numbers to implement. It is a legitimate, encryption-free, open-protocol path (Art-Net) — no NDA/SDK needed, no live-stream reverse engineering.
- Limitation vs a true DAC: no per-point live streaming — animation richness is limited to what's pre-baked in the `.ild` files plus the real-time transforms (position/size/rotation/colour/speed/strobe) the firmware applies. "Live-ish" is accurate: you switch and modulate looks, you don't stream arbitrary geometry frame-by-frame.

### Gaps
- Unconfirmed: exact minimum firmware for Professional Mode and that a 2016/HW-ID-4 external unit updates to it; the Art-Net *input* mode's exact Admin Tool steps and default universe; and the SD-over-LAN upload workflow. All three should be settled with Laserworld (align with the 22 open questions in `docs/research/shownet.md`).
