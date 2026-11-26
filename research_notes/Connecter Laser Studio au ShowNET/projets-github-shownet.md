# GitHub / open-source projects that talk to the Laserworld ShowNET

*Research agent notes, 2026-10-21. Public sources only (GitHub, laserworld.com,
docs.openlighting.org, Liberation Laser docs/forum, crates.io/docs.rs, trade
press). Nothing was decompiled, no packet captured, no key sought, nothing sent
to any ShowNET. Companion to `docs/research/shownet.md` and
`docs/research/shownet-options.md`.*

**Headline:** As of 2026 there is **no public code repository** — on GitHub,
GitLab or elsewhere — that drives the **Laserworld ShowNET laser DAC**, neither
through the official SDK nor by reverse engineering its live point stream. Every
piece of software that outputs to the ShowNET over LAN (MadMapper/MadLaser,
TouchDesigner, Modulaser, CloudLase, ILD Render, Millumin, Showeditor,
Showcontroller) is **closed-source** and links Laserworld's **binary,
per-partner-keyed, NDA'd SDK**. The only "ShowNet" code that is open source is
for the **unrelated Strand Lighting ShowNet DMX protocol** (name collision). The
notable open-source laser apps (ofxLaser/Libera, laser-dac) deliberately support
**only published protocols** and exclude the ShowNET. The user's own
`mimi6060/ilda-laser` is the one open project that even has a ShowNET output
stub, and it is empty, waiting for the SDK.

---

## KQ1 — Which repos integrate the OFFICIAL ShowNET SDK, and how do they keep the NDA'd keys out of a public repo?

### Takeaway
No public repository integrates the official ShowNET SDK. Every known ShowNET
integration ships as a closed-source commercial binary, so the question of "how
an open-source repo hides the keys" has **no existing precedent to copy** — the
SDK's own distribution model (binary library + per-partner keys under NDA)
effectively bars it from any public repo, which is exactly why none exists.

### Cited Findings
- The ShowNET SDK is "provided in the form of a library, which is available for
  Windows as well as for Mac systems, and even a version for Linux-based systems
  (experimental)" — a **precompiled binary**, not source. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "The API data is only provided to commercial integration partners… We
  unfortunately cannot provide further support for private projects." — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "It is required to use a set of keys, which are individually created per
  integration partner," covered by an NDA that is "meant to make sure that the
  keys are kept within the development project." Applicants send project details
  and a legal business address. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- SDK platforms: Windows x86/x64, macOS Intel and M1 (announced 27 Jan 2022);
  registration via `laser-interface.com/en/sdk`; even 2015-era hardware works
  after the latest firmware update. — [PLSN, 2022](https://plsn.com/newsroom/product-news/laserworld-announces-availability-of-shownet-laser-api/)
- The officially "directly compatible" software list is Showcontroller,
  Showeditor, MadMapper/MadLaser, TouchDesigner, ILD Render, Millumin,
  Modulaser, CloudLase Studio — **all closed-source commercial products**; none
  publishes its ShowNET code or the SDK. — [laserworld.com/en/shownet-compatible](https://www.laserworld.com/en/shownet-compatible.html)
- Direct GitHub-oriented web searches ("github Laserworld ShowNET SDK",
  "Laserworld ShowNET github repository laser DAC", "github ShowNET laserworld
  point/ild DAC code") returned **only Laserworld product pages and retailer
  listings** — no repository. — (negative result across multiple WebSearch queries, 2026-10-21)

### Inferences
- Because the SDK is a binary plus a per-partner key under NDA, an open-source
  project cannot legitimately vendor it into a public repo at all. The only
  pattern that could work (and the one `ilda-laser` already plans per
  `CLAUDE.md`) is to keep the library and key **entirely outside git** — read
  from an env var or an untracked file in the data dir — behind an off-by-default
  build flag (the `shownet` Cargo feature). There is no third-party precedent to
  point to because every integrator is closed-source and simply bundles the
  binary inside its own non-public build.
- `mimi6060/ilda-laser`'s public presence is plausibly an asset for the SDK
  application ("publicly available software project" is the exact category
  Laserworld invites), but that is about *eligibility*, not about hiding keys.

### Gaps
- No way to confirm, from public sources, how MadMapper/Modulaser/etc. store or
  obfuscate the SDK key inside their shipped builds — their repos are private.
- Whether the SDK's own licence/NDA would even permit referencing it from a
  public repo's build scripts (e.g. a documented env-var path) is not stated
  publicly; only an NDA-holder would know.

---

## KQ2 — Any community / reverse-engineering projects that drive the ShowNET WITHOUT the SDK? Do they work for live points? Legality?

### Takeaway
No. No open-source or community project drives the **Laserworld ShowNET**
without the SDK. The only legitimate SDK-free paths (SD-card ILDA files +
Art-Net/DMX file triggering) are documented by Laserworld itself and need no
reverse engineering — but no public repo implements even those against a
ShowNET. Established open-source laser apps explicitly **refuse** to touch the
ShowNET because its live protocol is closed.

### Cited Findings
- Liberation Laser's developer ("Seb" = Seb Lee-Delisle): "the communication
  method used by ShowNET is a closed protocol"; Liberation does **not** connect
  to ShowNET over LAN and "only supports open, documented protocols." Suggested
  alternative: an external open-protocol DAC (Helios) on the projector's ILDA
  input. — [Liberation forum: "Are Laserworld DS 1000 mk4's supported"](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Liberation's FAQ/policy: it "will only support DACs that have a published
  communication protocol." — [Liberation docs, compatible DACs](https://docs.liberationlaser.com/hardware/compatible-lasers-and-controllers-dacs)
- Laserworld's stated rationale for keeping it closed (relayed on the same
  forum): to prevent the ShowNET being "cloned by unscrupulous manufacturers."
  — [Liberation forum thread](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- **ofxLaser** (Seb Lee-Delisle, openFrameworks addon; the mature code
  Liberation grew out of) supports "Etherdream, Helios, Laserdock/LaserCube"
  (and IDN, added years ago); **no mention of Laserworld or ShowNET**. Licence:
  README says "ofxLaser License (Non-Commercial Share-Alike)" while the repo
  sidebar shows Apache-2.0 (the two conflict). — [github.com/sebleedelisle/ofxLaser](https://github.com/sebleedelisle/ofxLaser)
- **Libera** (Liberation's control layer) is advertised as "a free and open
  source C++ library" for discovering/communicating with laser controllers,
  giving developers "the same controller support used by Liberation" — i.e. only
  published-protocol DACs (Helios, Ether Dream, IDN, Sollinger AVB, LaserCube).
  — [liberationlaser.com/free-tools](https://liberationlaser.com/free-tools)
- **laser-dac (JS, Volst/laser-dac)**: supports Ether Dream, Helios, Laserdock,
  LaserCube WiFi, Pangolin Beyond (Win), EasyLase/NetLase (Win), simulator; MIT;
  **no ShowNET**; "legacy and no longer actively maintained." — [github.com/Volst/laser-dac](https://github.com/Volst/laser-dac/)
- **laser-dac (Rust crate, used by `ilda-laser`)**: backends are Helios, Ether
  Dream, IDN — **no ShowNET** (ShowNET speaks none of these open protocols). —
  [docs.rs/laser-dac](https://docs.rs/laser-dac)
- The SDK-free documented paths on the box itself (no reverse engineering): play
  `001.ild`–`229.ild` from the microSD standalone, and trigger/transform those
  files via **DMX or Art-Net** (DJ 19-ch / Professional 34-ch profiles). Art-Net
  carries **DMX only**, never laser points. — [Laserworld ShowNET 2019 manual](https://audioeffetti.com/product/documents/LAS/LASERWORLD%20SHOWN-01.pdf); channel map mirrored in [Open Fixture Library](https://open-fixture-library.org/laserworld/shownet)

### Inferences
- The live point stream over LAN is reachable **only** via the SDK. The open
  SDK-free options are limited to pre-baked ILDA files + a handful of DMX
  parameters (intensity, figure number, size, rotation, colour, safety zones) —
  not arbitrary live geometry. This matches `ilda-laser`'s existing `sdbank.rs`
  / `--shownet-artnet` design.
- **Legality flag:** No leaked-key or protocol-cracking project was found to
  flag. Were one to appear, using leaked per-partner keys or circumventing the
  stream's encryption would breach the SDK NDA and likely anti-circumvention law
  — and is explicitly forbidden by `ilda-laser`'s own IP rules. The *existence*
  of such a project is currently **nil** in public sources; this is reported as
  a factual absence, not an endorsement of any path.

### Gaps
- Closed laser forums (PhotonLexicon members' area) could not be read; a private
  thread discussing ShowNET internals may exist but did not surface publicly.
- Whether anyone has privately reverse-engineered the stream is unknowable from
  public sources; the public signal is uniformly "closed protocol, use the SDK
  or an open DAC."

---

## KQ3 — Is the ShowNET live streaming protocol actually encrypted (multiple sources)?

### Takeaway
Multiple independent public sources agree the ShowNET live protocol is **closed
and gated by per-partner keys**; but the specific word "**encrypted**" is
**Laserworld's "keys"/NDA framing**, not a documented cryptographic claim.
"Keys + NDA" and the anti-cloning rationale strongly imply the structure alone
is useless without the key, but no public source dissects the wire format to
prove encryption vs. mere obscurity/authentication.

### Cited Findings
- Laserworld: integration "is required to use a set of keys, which are
  individually created per integration partner" under NDA. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Independent developer (Seb, Liberation): ShowNET uses "a closed protocol"; all
  third-party support "relies on a manufacturer-controlled SDK (precompiled code
  from Laserworld)." — [Liberation forum thread](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Rationale given publicly: closed "to prevent ShowNET being cloned by
  unscrupulous manufacturers" — consistent with cryptographic gating of the
  stream. — [Liberation forum thread](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- `shownet-options.md` already notes (internal) that "the stream is encrypted" is
  **not stated verbatim** by any public Laserworld page — they say "keys" and
  "NDA." (Consistent with this round: no public page uses the word "encrypted.")

### Inferences
- "Per-partner keys" + "keys kept within the project" + anti-cloning intent
  reads as a cryptographically keyed/encrypted stream rather than a merely
  undocumented one. For `ilda-laser`'s purposes the practical conclusion is the
  same either way: reverse-engineering the structure without the key yields
  nothing usable, so the SDK (or an open DAC / SD+Art-Net) remains the only
  route. The project should keep treating it as encrypted per its hard rules.

### Gaps
- No public source confirms the cipher, key exchange, or that the payload is
  encrypted vs. authenticated/obfuscated. This can't be resolved without
  capturing/analysing the stream — which the IP and safety rules forbid, so it
  stays an accepted unknown.

---

## KQ4 — CRITICAL: Strand Lighting "ShowNet" (DMX-over-IP) vs. Laserworld "ShowNET" (laser DAC)

### Takeaway
These are **two completely unrelated things** that share a name. Almost every
"ShowNet open source" hit — OLA, the OpenLighting wiki, OLA's `shownet` plugin —
is the **Strand Lighting DMX-over-Ethernet lighting protocol**, NOT the
Laserworld laser interface. The reverse-engineering that *has* happened is of
the **Strand** protocol. This collision explains why "ShowNet is reverse
engineered" appears in searches and must be excluded when assessing the laser box.

### Cited Findings
- OpenLighting wiki: "ShowNet is a DMX over IP protocol developed by Strand
  Lighting… not publicly available but has been reverse engineered." (This is
  the Strand protocol — unrelated to Laserworld.) — [wiki.openlighting.org/Shownet](https://wiki.openlighting.org/index.php/Shownet) (page intermittently HTTP 500; content confirmed via [WebSearch snippets] and OLA API docs below)
- OLA ships a `shownet` plugin implementing **Strand's** protocol: packet types
  standard DMX `0x202f` and compressed DMX `0x808f`, up to 8 universes/node,
  config takes an IP/interface + node name. — [OLA API docs, ola::plugin::shownet](https://docs.openlighting.org/ola/doc/latest/namespaceola_1_1plugin_1_1shownet.html)
- OLA lists ShowNet among its lighting protocols alongside Art-Net, E1.31/sACN,
  ESP-Net, KiNET, OSC, Pathport, Sandnet — all **DMX transport**, no lasers. —
  [OLA wiki](https://wiki.openlighting.org/index.php/OLA)
- A published spec exists for the Strand protocol (theatrecrafts archive); Strand
  engineers "found a way to make DMX512 over Ethernet possible." — [theatrecrafts ShowNet spec PDF](https://www.theatrecrafts.com/archive/documents/shownetspecification.pdf); [ESTA Protocol, Winter 2015 p.86](https://protocol.esta.org/winter-2015/page-86)
- Pathport nodes can convert DMX to/from ShowNet (Strand) alongside sACN and
  Art-Net — further confirming it is a stage-lighting DMX protocol. — [ControlBooth forum](https://www.controlbooth.com/goto/post?id=395259) (anecdotal)

### Inferences
- When the report-writer sees "ShowNet reverse engineered / open source /
  OLA-supported," it is almost certainly **Strand DMX**, and tells us nothing
  about the Laserworld laser DAC. The two must be kept strictly separate in the
  final report. (Casing is an unreliable discriminator; Laserworld tends to write
  "ShowNET" and Strand "ShowNet," but sources are inconsistent.)

### Gaps
- None material. The distinction is well-sourced.

---

## KQ5 — Projects BEYOND the user's own `mimi6060/ilda-laser`

### Takeaway
`mimi6060/ilda-laser` (this project, "Laser Studio") is the **only** open-source
project found that even contains a ShowNET output path, and it is a **stub
awaiting the SDK** (feature `shownet`, `shownet.rs`). Beyond it, no other open
project targets the ShowNET; the adjacent open projects all deliberately target
open DACs instead.

### Cited Findings
- Per the project's own notes, `ilda-laser` surfaces in web searches for
  "ShowNET" and is the kind of "publicly available software project" the SDK
  program invites; its `ShowNetOutput` (`shownet.rs`) is a stub behind an
  off-by-default Cargo feature, with SD-card export (`sdbank.rs`) and Art-Net
  file triggering (`--shownet-artnet`, `artnet.rs`) as the SDK-free paths. —
  (`shownet-options.md`, `CLAUDE.md`, internal)
- Adjacent open projects and what they target instead of ShowNET:
  **ofxLaser** (Ether Dream, Helios, LaserCube, IDN) — [github.com/sebleedelisle/ofxLaser](https://github.com/sebleedelisle/ofxLaser);
  **Libera** (open C++ lib, published-protocol DACs only) — [liberationlaser.com/free-tools](https://liberationlaser.com/free-tools);
  **laser-dac JS** (Ether Dream, Helios, Laserdock, LaserCube, Beyond,
  EasyLase/NetLase; MIT) — [github.com/Volst/laser-dac](https://github.com/Volst/laser-dac/);
  **laser-dac Rust** (Helios, Ether Dream, IDN) — [docs.rs/laser-dac](https://docs.rs/laser-dac).
  None of these mention Laserworld/ShowNET.
- **PONK** (`madmappersoftware/Ponk`, Apache-2.0) is the open bridge the project
  already uses: Laser Studio → PONK → MadMapper (which holds the licensed ShowNET
  integration) → box. This is the only *open* way to reach a ShowNET today, and
  it works by delegating to MadMapper's closed SDK link, not by talking to the
  box directly. — [github.com/madmappersoftware/Ponk](https://github.com/madmappersoftware/Ponk)

### Inferences
- The realistic open-source landscape for reaching a ShowNET is: (1) bridge to a
  licensed host over PONK (today's working path), (2) use an open DAC on the
  projector's ILDA input, (3) SD-card ILDA + Art-Net file triggering, (4) the
  official SDK behind an off-by-default, keys-outside-git feature. `ilda-laser`
  already encodes all four. No other open project advances the direct-ShowNET
  problem further.

### Gaps
- GitHub's native code search was not directly queryable via the available
  tools; "none found" rests on web-search coverage plus the strong corroborating
  signal that every ShowNET integrator is closed-source. A manual
  `github.com` code search for "ShowNET" + "laserworld" by a human reviewer
  would be a cheap confirmation step but is unlikely to change the conclusion.
