# ShowNET SDK integrators — who actually got it, and how small can they be

Scope note: this is Laserworld **ShowNET** (the laser DAC / mainboard by
Laserworld Group, Germany), not Strand/Vari-Lite "ShowNet" DMX networking.
All findings are from public sources as of 2026-10-21.

## Who are ALL the known ShowNET SDK integrators?

### Takeaway
Laserworld's official "ShowNET compatible" page lists **eight** applications
with direct ShowNET output: Showcontroller, Showeditor, MadMapper/MadLaser,
TouchDesigner, ILD Render, Millumin, **Modulaser**, and **CloudLase Studio**.
Two of them (Showcontroller, Showeditor) are Laserworld's own in-house /
distributed brands; the other six are genuine third-party SDK integrations.
Crucially, the newest two (Modulaser, CloudLase Studio) are tiny indie
operations — direct proof the SDK is not reserved for large companies.

### Cited Findings
- The official compatibility page lists eight apps with direct ShowNET
  output: Showcontroller, Showeditor, MadMapper/MadLaser, TouchDesigner,
  ILD Render, Millumin, Modulaser, and CloudLase Studio — [Laserworld ShowNET compatible](https://laserworld.com/en/shownet-compatible.html)
- Showcontroller and Showeditor appear under Laserworld's "Our Distributed
  Brands"; every ShowNET-equipped system ships with a free Showeditor
  licence — [Laserworld ShowNET compatible](https://laserworld.com/en/shownet-compatible.html)
- Laserworld confirms the third-party set "currently support direct output
  through the ShowNET laser mainboard": TouchDesigner, MadLaser,
  Showcontroller, Showeditor, ILD Render and Millumin (Modulaser and
  CloudLase are the later additions on the page) — [Laserworld newslist / compatibility](https://laserworld.com/en/shownet-compatible.html)

### Inferences
- Showcontroller/Showeditor are not "external SDK partners" in the relevant
  sense — they are Laserworld's own software, so they tell us nothing about
  how an outsider gets the SDK. The six useful data points are Derivative,
  GarageCUBE/1024, Anomes, (likely) Automatic Brain, RS Product Studio, and
  BX Precision Engineering.

### Gaps
- Laserworld earlier said "many more are currently in the integration
  process," but no public named list of pending/in-progress integrators was
  found beyond the eight shipping ones.

## For EACH integrator: size and nature

### Takeaway
The integrators span the full range: from established small-to-mid software
companies (Derivative, GarageCUBE+1024, Anomes) down to apparently
one-person micro-companies (Modulaser = a Dutch B.V.; CloudLase = an
Australian sole trading name). The smallest confirmed SDK holders are
clearly solo/indie commercial entities, not corporations.

### Cited Findings

**TouchDesigner — Derivative (Toronto, Canada)**
- Software company based in Toronto, Canada, established 2000; also has a
  Los Angeles presence — [bitscale.ai Derivative profile](https://bitscale.ai/directory/derivative); [docs.derivative.ca](https://docs.derivative.ca/TouchDesigner)
- No reliable public headcount; directory profiles show placeholder/"no
  data" for employees and an unreliable "$335K" revenue figure — [bitscale.ai Derivative profile](https://bitscale.ai/directory/derivative)
- Nature: established, well-known small-to-mid independent software vendor
  (the pro reference among the integrators).

**MadMapper / MadLaser — GarageCUBE (CH) + 1024 architecture (FR)**
- MadMapper is a joint project of GarageCube (Geneva, Switzerland — the
  engineering side, also makers of modul8/miniMad) and 1024 architecture
  (Paris, France — the artistic label); the dev team is based in Brussels,
  Belgium; product dates to 2010 — [madmapper.com/aboutus](https://madmapper.com/aboutus); [videomapping.store guide](https://videomapping.store/madmapper-projection-mapping-software/)
- Nature: small but established multi-company collaboration; the most
  commercially prominent integration (this is the T-300 PONK→MadMapper path
  Laser Studio already uses).

**Millumin — Anomes SARL (France)**
- Millumin is video show-control / mapping software made by Anomes SARL, a
  French limited-liability company; development began in 2010 — [benkuper.notion Millumin](https://benkuper.notion.site/Millumin-6b10cabc5077435d8e0a468c87e27a42); [Laserworld Millumin page](https://laserworld.com/en/shownet-compatible/millumin)
- Nature: small French software company (SARL). No public headcount found.

**ILD Render — likely "Automatic Brain" (country unconfirmed)**
- Turns Blender/Inkscape vector art into real-time laser output and ILDA
  files; latest version adds direct ShowNET output; described as Windows —
  [Laserworld ILD Render](https://www.laserworld.com/ild-render)
- Laserworld's page does not name the developer; the only lead is the
  "more info" link `ildrender.automatic-brain.de`, suggesting the developer
  is **Automatic Brain** — [Laserworld ILD Render](https://www.laserworld.com/ild-render)
- Nature: niche single-product tool, almost certainly a small/indie
  developer (the "automatic-brain.de" personal-style domain fits an indie).

**Modulaser — RS Product Studio B.V. (Netherlands)** ← very small/indie
- Footer: "© 2026 RS Product Studio B.V." — a Dutch (B.V.) company form; no
  individual named on the site — [modulaser.app](https://modulaser.app/)
- GitHub org "Modulaser" is listed as based in the Netherlands, contact
  info@modulaser.app, **2 followers**, 3 repos, "no public members" — [github.com/ModulaserApp](https://github.com/ModulaserApp)
- Pricing is indie-scale one-time licences: free trial (15-min output cap),
  Standard €179, Pro €499 — [modulaser.app](https://modulaser.app/)
- Site lists "Laserworld / ShowNET" as a supported DAC, flagged **"New"**,
  macOS + Windows only; the generic DAC drivers are open source
  (laser-dac-rs) — [modulaser.app](https://modulaser.app/)

**CloudLase Studio — BX Precision Engineering (Australia)** ← very small/indie
- Footer: "© 2026 BX Precision Engineering · Made in Australia"; no
  individual or team named — [cloudlase.studio](https://cloudlase.studio)
- Desktop laser-show software for macOS/Windows/Linux; lists ShowNET
  alongside Ether Dream, Helios etc. as supported hardware — [cloudlase.studio](https://cloudlase.studio); [Laserworld CloudLase page](https://laserworld.com/en/shownet-compatible/cloudlase-studio)
- Indie-scale pricing: Free tier (1 DAC, ILDA/MP4 export), Pro $59 one-time
  or $9/mo, Cloud Designer $4.99/mo, students/teachers free for 12 months —
  [cloudlase.studio](https://cloudlase.studio)
- No ABN/ASIC/company registry entry found for "BX Precision Engineering" in
  Australian directory searches — [search result](https://www.australianmanufacturing.com.au/business-directory/infinite-engineering/) (no match found)

### Inferences
- "BX Precision Engineering" with no company-registry footprint and a "Made
  in Australia" one-person-style product strongly reads as a sole trader /
  micro-business trading name, not a company — i.e. about the smallest
  possible legal structure that still got the SDK.
- RS Product Studio B.V. (2 GitHub followers, no named staff, one product)
  reads as essentially a one-person Dutch company.

### Gaps
- No employee counts, funding or revenue figures could be confirmed for any
  integrator from reliable primary sources — all are privately held and most
  are small enough to publish nothing.
- The individual people behind Modulaser, CloudLase and ILD Render are not
  publicly named on their sites.

## CRITICAL: Are any of them small/indie/one-or-two-person? Smallest entity + its legal structure

### Takeaway
Yes — decisively. The two newest integrators are micro-entities:
**Modulaser = RS Product Studio B.V.** (a one-person-scale Dutch private
company) and **CloudLase Studio = BX Precision Engineering, "Made in
Australia"** (an apparent sole-trader trading name with no visible company
registration). Both hold working ShowNET integrations. The SDK is gated on
being a *commercial entity with a legal business address for the NDA*, not
on being large. A solo dev with a minimal company structure (e.g. a
sole proprietorship, an EI/micro-entreprise, or a one-person Ltd/B.V.)
clearly qualifies; only purely private/hobbyist projects with no business
structure are excluded.

### Cited Findings
- SDK eligibility, verbatim intent: "The API data is only provided to
  commercial integration partners." Public software projects and industrial
  / custom-control use cases are also named as good fits — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Private use is explicitly excluded: "We unfortunately cannot provide
  further support for private projects"; hobbyists are steered to ready-made
  integrations like TouchDesigner — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- The NDA needs a legal business address: to request access you "reach out
  to Norbert" with your project details and "the legal business address
  needed for the NDA" — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Smallest confirmed holders: Modulaser (RS Product Studio **B.V.**,
  Netherlands private company, one-product, 2 GitHub followers) — [modulaser.app](https://modulaser.app/); [github.com/ModulaserApp](https://github.com/ModulaserApp); and CloudLase Studio (BX Precision Engineering, "Made in Australia", $59 one-time, student-free) — [cloudlase.studio](https://cloudlase.studio)

### Inferences
- The gate is legal/commercial form, not scale: a B.V. and an apparent
  Australian sole-trader both passed. A solo developer who registers even a
  minimal commercial structure (sole proprietorship with a business address)
  realistically meets the stated bar; the barrier is "be a business, sign the
  NDA," not "be big."
- For the user's own case: Laser Studio is a commercial/public software
  project (not a purely private hobby), so framing the request as a
  commercial integration partner with a registered business address and a
  concrete integration plan is the path the small integrators appear to have
  used.

### Gaps
- No public statement of a minimum revenue, user count, or company age
  requirement — none appears to exist; the only stated requirement is
  "commercial integration partner" + legal business address + NDA.

## Is there any public account of HOW an integrator obtained the SDK (onboarding, timeline, NDA, cost)?

### Takeaway
There is an official onboarding description from Laserworld, but no
integrator has published a first-person "how we got in" story (interview,
blog, release note). The process: contact Laserworld's Norbert (Stangl,
CMO), describe the project, provide a legal business address, sign a small
per-partner NDA, and receive partner-specific keys. No cost is published.

### Cited Findings
- Process: request access by contacting Norbert with project details + legal
  business address; each partner gets individually created keys and signs an
  NDA to keep the keys inside their project — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Earlier phrasing from a press/search source: "Individual keys are issued
  per partner, and we make a little NDA with each integration partner" —
  [search summary of Laserworld SDK pages](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Official registration/landing for SDK requests is the Laserworld-run
  laser-interface.com SDK page — [laser-interface.com/en/sdk (referenced)](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Platforms: library for Windows (x86/x64) and macOS (Intel + Apple
  Silicon/M1); Linux version offered but "experimental" — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- The contact person "Norbert" is Norbert Stangl, CMO of the Laserworld
  Group (author of Laserworld's SDK-opening essay) — [Laserworld essay by Norbert Stangl](https://www.laserworld.com/en/newslist/6106-fundamental-evolution-in-the-show-laser-industry.html)
- No cost/fee/pricing is mentioned anywhere on the SDK page — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- The "little NDA" and per-partner keys match exactly what the user's own
  CLAUDE.md already assumes (SDK under NDA, per-partner keys). The onboarding
  is lightweight and relationship-based (email Norbert), consistent with
  small partners getting in.

### Gaps
- No integrator interview/blog/forum post describing their own onboarding
  timeline was found (photonlexicon and general searches returned only
  Laserworld's own pages). The exact SDK cost (if any) is not public.

## Are any integrators open source? (expected: none)

### Takeaway
Confirmed: no ShowNET integration is open source. The SDK's NDA/per-partner
keys make an open-source ShowNET path impossible. Modulaser is the only
integrator that open-sources anything — but only its *generic* DAC drivers
and PONK codec (MIT), not the ShowNET piece, which necessarily stays closed.

### Cited Findings
- Modulaser open-sources (MIT) generic components only: laser-dac
  (TypeScript, legacy, 129 stars), laser-dac-rs (Rust), and ponk-protocol
  (Rust PONK codec) — [github.com/ModulaserApp](https://github.com/ModulaserApp)
- laser-dac-rs makes no mention of ShowNET or Laserworld and names no
  Laserworld SDK/NDA/closed component — its only third-party SDK is
  Steinberg ASIO; the ShowNET integration is not in the open repo — [github.com/ModulaserApp/laser-dac-rs](https://github.com/ModulaserApp/laser-dac-rs)
- CloudLase Studio's site makes no open-source claim and states no licence;
  described as "local-first" proprietary software — [cloudlase.studio](https://cloudlase.studio)
- The SDK itself requires per-partner keys kept private under NDA, which is
  structurally incompatible with publishing the integration — [ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- Modulaser is the closest analogue to Laser Studio's own architecture
  (open PONK + Rust DAC drivers, with the ShowNET/SDK part kept closed behind
  the NDA). It is a working proof that a tiny Rust-based laser app can keep
  its core open and still hold a closed, NDA'd ShowNET SDK integration —
  exactly the split Laser Studio already plans (feature `shownet`, SDK out of
  git).

### Gaps
- Could not inspect Modulaser's/CloudLase's closed source to confirm how they
  wrap the SDK; only that the public repos exclude it.
