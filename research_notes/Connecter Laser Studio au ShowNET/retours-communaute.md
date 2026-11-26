# Community experiences connecting custom/open software to the Laserworld ShowNET

Scope: real-world, lived community wisdom (not vendor marketing) about ShowNET openness,
workarounds, SDK onboarding, and budget advice for someone who already owns a ShowNET and
wants live, audio-reactive output from their own software. Current as of 2026-10-21.

Note on source depth: the laser-hobbyist web is thin and fragmented on this exact topic.
The single most substantive first-hand developer statement found is on the **Liberation Laser
forum** (Seb Lee-Delisle, a well-known laser-software developer). Photonlexicon, Reddit r/lasers
and Facebook groups did not surface in searches with specific ShowNET-vs-custom-software threads
(see Gaps). Retailer review sections (Thomann) gave the richest first-hand end-user sentiment.

---

## Q1. What does the community say about using ShowNET with non-Laserworld / custom / open-source software?

### Takeaway
The clearest community verdict is that ShowNET's live-streaming protocol is **closed/proprietary**:
you cannot talk to it directly from open software. Third-party apps that do support it
(TouchDesigner, MadMapper/MadLaser) only do so through a **Laserworld-controlled SDK**, which at
least one prominent independent developer deliberately refuses to depend on. The practical
community consensus is to bypass the ShowNET LAN port and drive the laser through its **standard
ILDA input with an external open DAC**.

### Cited Findings
- Seb Lee-Delisle (developer of Liberation laser software) on whether a Laserworld DS-1000 Mk4 can connect over its ShowNET LAN port: "not directly via the ShowNET LAN port" and "the communication method used by ShowNET is a closed protocol." — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- He states TouchDesigner and MadLaser support ShowNET only through a "manufacturer-controlled SDK," access to which "can change, be restricted, or withdrawn at any time," and "I've made a deliberate decision not to build Liberation on top of that kind of dependency." — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- He says Liberation supports only "open, documented protocols," to encourage interoperability and avoid vendor lock-in, and would integrate ShowNET if Laserworld "openly document their protocol, or implement an open protocol like IDN on their ShowNET firmware." — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Laserworld's stated reason for keeping the protocol closed, per Seb, is to prevent "ShowNET being cloned by unscrupulous manufacturers"; he respects the decision while disagreeing on the risk. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- The open-source `laser-dac` Rust crate (Modulaser) and the openFrameworks `ofxLaser` addon support Helios (USB) and Ether Dream (network), LaserCube/Laserdock — but **not** ShowNET; these outputs can only reach a laser through its ILDA input, not the ShowNET LAN port. — [laser-dac docs.rs](https://docs.rs/crate/laser-dac/latest); [ofxLaser GitHub](https://github.com/sebleedelisle/ofxLaser)
- Laserworld markets ShowNET as natively compatible with TouchDesigner, MadLaser/MadMapper, Millumin, ILD Render etc., plus its own Showeditor/Showcontroller — all of which use the licensed SDK path, not an open one. — [audioeffetti / Laserworld](https://audioeffetti.com/en/blog/shownet-becomes-mainboard-for-most-laser-systems-of-the-laserworld-group-n90726)
- Laserworld describes ShowNET's transport as "standard UDP data transfer protocol" over normal switches — i.e. the transport is ordinary UDP, but the **payload/point format is the undocumented closed part**. — [Laserworld distributed-brand page](https://laserworld.com/en/distributed-brands/distributed-brand-shownet.html)

### Inferences
- The community/developer framing matches the project's own CLAUDE.md position: the ShowNET live point stream is effectively off-limits to open software, and the only legitimate direct route is Laserworld's SDK. There is no evidence of anyone legally reaching the live stream without it.
- "Standard UDP" wording in marketing can mislead newcomers into thinking the protocol is open; the actual point encoding is not, which is why no open library implements it.

### Gaps
- No photonlexicon.com or Reddit r/lasers thread was located that directly debates custom software + ShowNET. The developer-side view is well represented (Liberation forum); the hobbyist-forum chatter specifically on this is a gap.

---

## Q2. Has anyone in the community obtained the ShowNET SDK? Onboarding, response times, NDA details?

### Takeaway
The SDK/API officially exists and is obtainable, but **only for commercial integration partners or
publicly-available software projects** — explicitly **not** for private/hobby projects. Access is
gated by a per-partner NDA and individually issued keys. No first-hand community account of a
hobbyist successfully onboarding was found; the policy text itself tells private users to use
ready-made integrations instead.

### Cited Findings
- Official policy: "The API data is only provided to commercial integration partners." Eligible cases are publicly-available software projects and industrial/custom-control use cases. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "Private projects are not eligible for the API." Laserworld "cannot provide further support for private projects" and suggests ready-made integrations such as TouchDesigner instead. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- NDA + keys: integration "requires a set of keys, created individually for each integration partner," and Laserworld "signs an NDA with each partner to keep the keys within the partner's development project." — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- How to apply: contact a named person ("Norbert") with project details and the legal business address needed for the NDA; press releases also point to a registration portal at laser-interface.com/en/sdk. — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html); [PLSN](https://plsn.com/?p=352037)
- Platforms: library for Windows (x86/x64) and macOS (Intel + Apple Silicon/M1); a Linux version exists but is marked "experimental." — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Older 2015+ hardware is made compatible "if the latest firmware is applied." — [audioeffetti / Laserworld](https://audioeffetti.com/en/blog/shownet-becomes-mainboard-for-most-laser-systems-of-the-laserworld-group-n90726)

### Inferences
- A legitimate SDK route exists for Laser Studio **only if** it qualifies as "publicly available software" (which an open-source/public product plausibly could) — not as a private hobby project. This is the lever to pursue with Laserworld, consistent with CLAUDE.md task T-015/T-470/T-471.
- The firmware caveat matters for the user's unit (reported firmware `2016050502`): SDK/Art-Net modes need newer firmware, flashable only via the Windows-only Admin Tool.

### Gaps
- No public first-hand report of SDK response times, actual NDA wording, cost, or a hobbyist/open-source author being accepted or rejected. CLAUDE.md notes Laserworld has not answered the project's own requests — consistent with the absence of community onboarding stories.
- Could not confirm current terms on laser-interface.com/en/sdk (page not retrieved).

---

## Q3. What do experienced hobbyists recommend for a budget user who owns a ShowNET and wants live, audio-reactive output from their own software?

### Takeaway
The dominant, repeated recommendation is to **ignore the ShowNET LAN port for live custom output
and instead feed the laser's standard ILDA input with a cheap external DAC — the Helios DAC being
the specific budget pick** — which then works with virtually all open/free software. For those
willing to stay inside the ShowNET ecosystem, **TouchDesigner** is the recommended ready-made,
free-for-non-commercial route that is genuinely audio-reactive and natively drives ShowNET. No
cracking/piracy workaround was endorsed by anyone (none found; any such route is NOT recommended).

### Cited Findings
- Seb Lee-Delisle recommends adding an external controller (Ether Dream or Helios) on the laser's standard ILDA input; he calls the Helios DAC "the quickest and most cost-effective way" to do this. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- The original asker (Jonathan) called a Helios + USB-to-ILDA adapter "the path of least resistance," but ultimately kept ShowNET because it was "handy / cost effective for td [TouchDesigner] interactivity." — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Liberation's hardware docs: USB controllers like the Helios DAC are "the most affordable option for home use or small shows with 4 or fewer lasers"; network DACs like Ether Dream are "the best option for professional laserists running many lasers." — [Liberation Laser docs](https://docs.liberationlaser.com/en-US/hardware/compatible-lasers-and-controllers-dacs.html)
- Helios is sold on Amazon or direct from Bitlasers (a ~10% discount code was mentioned); it is the commonly cited cheap, open-ecosystem DAC. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Ether Dream is open-hardware with an open-source software library; openFrameworks `ofxLaser` and `laser-dac` both target Helios + Ether Dream. — [candlepowerforums](https://www.candlepowerforums.com/threads/question-about-ilda.150850/); [ofxLaser GitHub](https://github.com/sebleedelisle/ofxLaser)
- TouchDesigner natively detects ShowNET on the LAN and outputs via the Laser Device CHOP; it is "free to use for private and non-commercial projects" but has "limited output resolution if no license is purchased." — [Derivative community post](https://derivative.ca/community-post/shownet-laser-support/68339); [Laserworld](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- Setup for ShowNET in TouchDesigner: create a Laser Device CHOP, set device type to ShowNET, select the device; generate points with a Laser CHOP connected to it. A plain LAN cable (even direct) is enough, no extra license/hardware for the connection itself. — [Derivative community post](https://derivative.ca/community-post/shownet-laser-support/68339)

### Inferences
- For the user's exact goal (live, audio-reactive, from their own code, on a budget), the cleanest legal path is: keep the laser, add a ~Helios-class DAC on the ILDA input, and drive it from their own software via `laser-dac`/`ofxLaser`-style open libraries. This sidesteps the SDK/NDA entirely. Trade-off: the ShowNET box becomes unused for that signal path, and it's a USB (single-laser-friendly) rather than networked route.
- If the user wants to keep using the ShowNET box itself for live output without their own low-level code, TouchDesigner is the realistic free-for-non-commercial bridge and is strongly audio-reactive — but it is not "their own software" and the free tier caps resolution.

### Gaps
- No current (2026) price points for Helios/Ether Dream found in sources; the Helios "cheapest" claim is qualitative.
- No first-hand account of someone running audio-reactive output from fully self-written software into a ShowNET laser via the ILDA-DAC route (plausible and recommended, but undocumented in the sources found).

---

## Q4. Legal community tools, scripts, or workflows for getting more out of the ShowNET (SD/Art-Net path, Showeditor tricks, MIDI/DMX triggering)

### Takeaway
The ShowNET has a fully legitimate, SDK-free operating mode: **play ILDA files from the SD card and
trigger/modulate them live over DMX or Art-Net**, including a standalone auto/demo mode. This is the
practical "no-SDK" live path, but it triggers pre-made files rather than streaming arbitrary live
points, and community reviews flag the authoring software (Showeditor) as basic and the hardware
setup as fiddly.

### Cited Findings
- Standalone mode: a DIP-switch setting makes the ShowNET auto-play the SD card's ILDA files consecutively in a loop; a "demo mode" adds automatic animation to static frames. — [Laserworld online manual](https://laserworld.com/en/194-online-user-manual/3785-external-shownet-auto.html); [djbox.ie](https://djbox.ie/products/laserworld-shownet-incl-showeditor)
- DMX/Art-Net triggering: individual patterns can be triggered and modulated (intensity, base colour, size, offset, etc.) over standard DMX512 "without having to save a specifically modified figure"; the unit has a built-in Art-Net-to-DMX converter. — [leisuretec](https://leisuretec.co.uk/products/shownet-network-interface-inc-showeditor-licence)
- SD card accepts ILDA format code 4/5 RGB files; 100 Mbit Ethernet, DMX in/out, ~100,000 pps max scan speed. — [Laserworld ShowNET datasheet (Leisuretec PDF)](https://leisuretec.co.uk/cdn/shop/files/Laserworld_ShowNET_Network_Interface_DataSheet.pdf)
- Master-slave multi-unit operation is supported; all units in a chain should carry the same SD files. — [Laserworld: ShowNET as mainboard](https://www.laserworld.com/en/technical-explanations-overview/shownet-as-laser-mainboard.html)
- Showeditor (free, bundled) can author/export content; "ShowNET comes with free software that allows creation of custom content." — [Laserworld](https://www.laserworld.com/en/software/weitere-shownet-kompatible-software/shownet-api-sdk.html)
- End-user sentiment on the SD/standalone experience is mixed: one Thomann reviewer said the SD card "held very little" and that demo mode "did not match the product's presentation." — [Thomann reviews](https://www.thomannmusic.com/laserworld_shownet_showeditor_software_reviews.htm)

### Inferences
- This SD + Art-Net/DMX workflow is exactly the path Laser Studio already implements (CLAUDE.md T-302 `sdbank.rs` export of `001.ild`–`229.ild`, T-301 `--shownet-artnet` DMX file selection). Community evidence confirms it is the correct, legal, SDK-free way to use the box — and that "live" here means triggering/modulating pre-exported files, not streaming, with "disarm" = intensity 0 + pattern 0.
- It is not audio-reactive at the point level; audio reactivity would have to be expressed as pre-rendered file variants plus live DMX parameter changes, or be done in a parallel DAC path.

### Gaps
- No community tutorial/script found that chains a custom app → Art-Net → ShowNET SD selection in practice (beyond the project's own implementation). Real-world latency/reliability of the Art-Net trigger path is undocumented in sources found.

---

## Q5. General sentiment: is the ShowNET considered "locked down"? Common advice?

### Takeaway
Among developers, yes — the ShowNET's live protocol is seen as deliberately closed and SDK-gated,
and the common independent-developer advice is to avoid depending on it and use the open ILDA-DAC
route. Among end users, sentiment is more about value and usability than openness: ShowNET is widely
seen as a cheap, decent entry point whose bundled Showeditor is "basic," with Pangolin as the common
"spend more, get more" upgrade.

### Cited Findings
- Developer sentiment: ShowNET is a "closed protocol" and building on its SDK is a dependency risk that "can change, be restricted, or withdrawn at any time"; the recommended stance is open/documented protocols only. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- The exchange was notably cordial — the asker called Seb's transparent explanation "Very refreshing!" and Seb said Laserworld "have always been gracious and respectful"; this is a respectful-disagreement tone, not a flame war. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- End-user value sentiment: "Great functionality at a good price"; praised as free software, low cable cost vs USB, "a good start for beginners" (~$215 at Thomann at time of review). — [Thomann reviews](https://www.thomannmusic.com/laserworld_shownet_showeditor_software_reviews.htm)
- Usability complaints: hardware "difficult to use" with "old fashioned DIP switches"; "ShowEditor is very basic"; "no web interface to manage or verify status"; half the UI in German regardless of language; and a flat "this DOES NOT WORK ON Mac OS !!" (reviewer resorted to Windows on a MacBook). — [Thomann reviews](https://www.thomannmusic.com/laserworld_shownet_showeditor_software_reviews.htm)
- Upgrade advice recurs: multiple reviewers suggest Pangolin ("Would suggest spending the extra and go for Pangolin any day!"); one user bought Laserworld's paid Showcontroller because Showeditor is basic. — [Thomann reviews](https://www.thomannmusic.com/laserworld_shownet_showeditor_software_reviews.htm)
- Counter-sentiment on software: one German reviewer found "Showeditor is easy to handle" and was disappointed Showcontroller was not more user-friendly — so "clunky" is not universal. — [Thomann (Showcontroller reviews)](https://www.thomannmusic.com/laserworld_showcontroller.htm)

### Inferences
- The "locked down" characterization is real and comes from credible developers, but it is framed as a business/interoperability choice rather than malice; Laserworld is seen as cooperative-but-closed. "User demand can make a difference" (Seb) implies the community lever is to ask Laserworld for IDN/open-protocol support.
- For a budget macOS user writing their own audio-reactive software, the community's collective advice converges on: don't fight the closed LAN protocol — use the ILDA input + an open DAC, or use TouchDesigner if staying on the ShowNET box, and pursue the SDK only if the project can qualify as "publicly available software."

### Gaps
- No quantified community poll/consensus on "locked down" (the view rests on one authoritative developer post plus the policy page). Broader forum sentiment (photonlexicon, Reddit, Facebook laser groups) specifically on ShowNET openness was not located and remains a gap.

---

## Cross-cutting conclusion (validates the project's premise)
The community evidence **supports** the CLAUDE.md conclusion that there is **no free/open live
point-stream path to the ShowNET**: the live protocol is closed, open libraries (`laser-dac`,
`ofxLaser`) cannot reach it, and the SDK is NDA-gated and off-limits to private projects. The
legitimate community-endorsed options for a budget owner are, in order: (1) an external open DAC
(Helios/Ether Dream) on the laser's ILDA input driven by open software — the real "open live" route,
at the cost of not using the ShowNET box for that signal; (2) TouchDesigner's free-non-commercial
native ShowNET output (audio-reactive, but not the user's own code, resolution-capped on the free
tier); (3) the SD-card + DMX/Art-Net trigger workflow for SDK-free use of the box itself (legal,
already implemented in Laser Studio, but file-trigger not live-stream); (4) the official SDK, only
if the project qualifies as publicly-available software. No legal shortcut around the closed live
protocol was found, and no cracking/leaked-key route is endorsed by anyone (any such route is NOT
recommended and likely violates the project's IP rules).
