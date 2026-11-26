# Obtaining the Laserworld ShowNET SDK — NDA, open-source compatibility, and the practical path for an EU/Belgian individual

Scope: how to actually obtain the ShowNET SDK, get a reply from Laserworld, satisfy the
"legal business address" requirement as an individual, and whether the NDA is compatible with
keeping Laser Studio open source. Current as of 2026-10. Legitimate channels only.

## 1. Best channel to actually reach Laserworld and get a reply

### Takeaway
The "Norbert" named on the SDK page is **Norbert Stangl, Sales/Marketing Director** at Laserworld
(Switzerland) AG — a marketing/sales contact, which likely explains the non-response to a developer
request. The only official company address and phone are the Swiss HQ. There is no published
developer/support-ticket channel for the SDK, and no identified Belgian/Benelux Laserworld
distributor. Best bets: phone the Swiss HQ directly, reframe the request as a "publicly available
software project / commercial integration partner" (the exact qualifying phrase), and reach Stangl
through a professional channel (LinkedIn / trade-show follow-up) rather than the generic marketing route.

### Cited Findings
- The SDK page instruction is literally "please feel free to reach out to Norbert" with project details and a legal business address — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "Norbert" is **Norbert Stangl**, listed as sales/marketing director at Laserworld (Switzerland) AG — [LaserFocusWorld directory](https://laserfocusworld.com/directory/laser-sources/laser-diodes/company/14048090/laserworld-switzerland-ag); he is also the company's public-facing interview spokesperson — [Prolight+Sound blog](https://prolight-sound-blog.de/laserworld-pls23/)
- Official company / legal entity: **Laserworld AG**, Kreuzlingerstrasse 5, CH-8574 Lengwil, Switzerland; Phone **+41 (0)71 677 80 80**, Fax +41 (0)71 677 80 88; a UK line is also listed, **+44 161 872 0272** — [Laserworld imprint](https://www.laserworld.com/en/imprint.html)
- Authorized person named in the imprint: Mr Martin Werner (supervisory board); no managing director or developer contact is named — [Laserworld imprint](https://www.laserworld.com/en/imprint.html)
- Email addresses on both the SDK page and imprint are spam-obfuscated and not machine-readable; no direct developer/support email or ticket system was found on the official pages — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html), [Laserworld imprint](https://www.laserworld.com/en/imprint.html)
- German group entities exist (Ray Technologies GmbH in Valley, Germany; tarm laser technologies GmbH & Co. KG) but no street address/phone for a German SDK contact was found — [About the Laserworld Group](https://laserworld.com/en/about-laserworld/about-the-laserworld-group); the UK has a trade distributor (Leisuretec) — [Leisuretec](https://leisuretec.co.uk/brand/laserworld)

### Inferences
- Because "Norbert" is a sales/marketing director, a plain SDK/API email likely gets deprioritised against sales leads. Phoning the Swiss HQ (+41 71 677 80 80) and asking for the person handling ShowNET SDK integration partnerships, or reaching Stangl on LinkedIn / at a trade show (Prolight + Sound), is more likely to get a human reply than the obfuscated marketing email.
- Framing matters: leading with the exact qualifying language from their own page ("publicly available software project", "commercial integration partner") rather than "hobby/personal project" materially changes eligibility (see Q4). Supplying the legal business address up front removes their first objection.
- Ray Technologies GmbH (Valley, Germany) is the group's high-end/engineering arm; a German-language technical approach to the group may route better than the generic contact, but no direct contact was confirmed.

### Gaps
- No working developer-support email, ticket system, or official forum for the SDK was found.
- No Belgian/Benelux Laserworld distributor identified (a local distributor could be an intermediary but none confirmed).
- Norbert Stangl's direct email/phone not published.

## 2. Does laser-interface.com/en/sdk have a working registration form?

### Takeaway
**No. The reported broken redirect is confirmed.** `laser-interface.com/en/sdk` issues a 301 to a
malformed URL (`laserworld.com/en/laser-online-user-manual.htmlen/sdk`) that does not resolve. There
is no working self-service registration form; the real process is to email/contact Norbert (Stangl)
directly with project details + legal business address.

### Cited Findings
- `https://www.laser-interface.com/en/sdk` returns **HTTP 301** redirecting to `https://www.laserworld.com/en/laser-online-user-manual.htmlen/sdk` — a broken, concatenated URL — confirmed on fetch (2026-10) — [laser-interface.com/en/sdk redirect]
- Laserworld's news item and PLSN still advertise that developers "can find more details here and register for receiving more details and individual access keys" at that same dead link — [PLSN article](https://plsn.com/?p=352037)
- The current actual process per the live SDK page: contact Norbert, "provide some details about your project as well as the legal business address for the NDA" — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- The registration page has been broken for some time (the same dead link is echoed by third-party articles), reinforcing that the only live route is direct human contact, not a form. Worth mentioning the broken link explicitly when contacting them — it signals a real integrator and gives them a reason to reply.

### Gaps
- Could not determine whether a registration form ever worked or what fields it collected.

## 3. The "legal business address" requirement — can a private individual qualify, and cheapest Belgian structure

### Takeaway
The NDA requires a **legal business address**, which points to a registered business rather than a
bare private individual. For a Belgian individual the cheapest/simplest fit is a **sole
proprietorship (entreprise individuelle / eenmanszaak)**: no minimum capital, no notary, registered
via an accredited business counter ("guichet d'entreprises"), and the **home address can legally
serve as the registered seat**. Cost is roughly **€105.50 CBE registration + ~€70 admin** (some
official procedures are free if done yourself), completed within about a week.

### Cited Findings
- Laserworld requires "the legal business address for the NDA" — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- A Belgian sole proprietorship's registered seat "is usually your home address" — [Deel: sole proprietorship Belgium](https://www.deel.com/blog/sole-proprietorship-belgium/)
- Registration cost: "€105.50 (2024)" to register with the Crossroads Bank for Enterprises (CBE/BCE), plus ~€70 for the rest of the administration (VAT activation, advice), varying by business counter — [Accountable.eu](https://www.accountable.eu/?p=5423)
- Conflicting note: official guidance says the procedures are free if you handle them yourself, with a fee if filed through a social-security service — [business.belgium.be](https://business.belgium.be/4)
- No minimum capital and no notary required for a sole proprietorship; registration goes through an accredited business counter (guichet d'entreprises / ondernemingsloket) or online, often completed within a week — [Deel](https://www.deel.com/blog/sole-proprietorship-belgium/), [business.belgium.be enterprise counter steps](https://economie.fgov.be/en/themes/enterprises/starting-business/steps-starting-business/steps-take-business-counter)
- Liability caveat: with a sole proprietorship, personal assets can be seized for business debts unless the primary residence is protected via a notarial declaration of unseizability (covers only future debts) — [Deel](https://www.deel.com/blog/sole-proprietorship-belgium/)

### Inferences
- A Belgian sole proprietorship almost certainly satisfies "legal business address": it yields a real CBE/BCE enterprise number and a registered seat (the home address), which is what the NDA needs — a counterparty with a registered legal identity to sign, not a limited-liability company. The user does not need to form a company (SRL/BV, ~€3,280+ and notary) for this purpose.
- An ASBL/VZW (non-profit association) is an alternative but is heavier (multiple founders historically, statutes, publication) and offers no advantage here over a sole proprietorship; the sole proprietorship is the cheapest/simplest.
- Before registering, worth asking Laserworld whether they will accept an individual's registered sole-proprietorship address, or even a private address — their page does not explicitly say a limited company is required, only a "legal business address." The sole proprietorship is the low-cost way to guarantee acceptance.

### Gaps
- Exact current (2026) Belgian fees vary by business counter and route; the €105.50 figure is 2024 and conflicts with "free if done yourself." User should confirm with a guichet d'entreprises.
- Not confirmed whether Laserworld would accept a bare private home address without any business registration — the wording ("legal business address") implies a registered entity.

## 4. Open-source compatibility — can you sign the NDA and still publish Laser Studio as open source?

### Takeaway
**Yes — this is the strongest positive finding.** Laserworld's own SDK page explicitly welcomes
"a publicly available software project" as a qualifying integration-partner use case, and the NDA's
stated purpose is narrow: to keep the **keys** within the development project, not to keep the app's
source closed. That is fully compatible with publishing the app under MIT/GPL while keeping the SDK
library and per-partner keys out of the public repo (the planned off-by-default Cargo feature). The
general legal practice for NDA + OSS co-existence supports exactly this split.

### Cited Findings
- Verbatim from the SDK page: **"If you have a publicly available software project, this is exactly for you."** — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Verbatim: **"The API data is only provided to commercial integration partners."** — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Verbatim NDA purpose: **"To make sure that the keys are kept withing [sic] the development project, we make a little NDA with each integration partner."** — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Verbatim on hobby use (what to avoid being classed as): "If you just want to play with the API fo [sic] your private prjects [sic], you probably want to check out ready made integrations"; "We unfortunately cannot provide further supprot [sic] for private projects." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- The SDK is a native **library** for Windows (x86/x64) and macOS (Intel + M1), with an experimental Linux build — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- General OSS/NDA practice: an "open core" split is standard — core product open source, while premium/proprietary pieces stay closed and NDA-protected; and NDAs over mixed codebases should explicitly carve out the OSS parts from confidentiality — [terms.law OSS NDA guide](https://terms.law/NDA/scenarios/software-dev/open-source-nda.html) (via search summary)
- Vendor precedent for the same pattern: Realtek and MediaTek keep most SDK code under open-source licenses and gate only specific proprietary modules/keys behind an NDA — [MediaTek Genio community](https://genio-community.mediatek.com/t/what-license-model-is-required-to-access-the-aosp-sdk/364)

### Inferences
- The NDA scope as stated ("keep the keys within the development project") restricts **distribution of the keys and the SDK binary**, not publication of your own application source. Keeping the SDK library and keys out of the public GitHub repo behind an off-by-default feature (the repo's existing `shownet` Cargo feature plan) satisfies that clause while the rest of Laser Studio stays MIT/GPL. This is the textbook open-core arrangement.
- The decisive eligibility lever is self-classification: present Laser Studio as a "publicly available software project" (their qualifying phrase), not a "private project" (their disqualifying phrase). The project being open-source on GitHub is an asset here, not an obstacle — it is literally the described fit.
- Two cautions to verify against the actual NDA text once received: (1) the NDA must not impose confidentiality on the app's own source or on the mere fact of integration in a way that conflicts with public development — ask for an explicit carve-out that the applicant's own published code is not confidential; (2) if the SDK is linked as a native library into a GPL/AGPL build, confirm that distribution of the combined binary is handled correctly (the project avoids this by never shipping the SDK binary — users supply their own under their own NDA, or the feature stays off). MIT avoids the copyleft-combination question entirely.
- The NDA is described as "a little NDA" — i.e. lightweight/standard, consistent with a key-protection NDA rather than a broad source-confidentiality agreement.

### Gaps
- The actual NDA text is not public, so the precise confidentiality scope and any redistribution/sublicensing clauses cannot be verified until Laserworld provides it. The "keys within the development project" summary is the only public statement of intent.
- No public precedent found of a specifically open-source MIT/GPL project that signed the ShowNET SDK NDA; the compatibility conclusion rests on the page's own wording + general OSS/NDA practice.

## 5. Is there a published fee for the SDK?

### Takeaway
**No fee is published anywhere found**, and multiple pages describe the SDK/keys/NDA without
mentioning any charge — consistent with the project's assumption that the SDK is free (cost is the
NDA + business-address hurdle, not money). This is not explicitly confirmed as "free" by Laserworld.

### Cited Findings
- Neither the official SDK page, the news announcement, nor PLSN states any SDK fee or cost; they describe only the keys + NDA + business-address process — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html), [PLSN article](https://plsn.com/?p=352037)
- For reference, the ShowNET **hardware** mainboard set had an MSRP around €249 excl. VAT (2019 era) — this is hardware, not the SDK — [PLSN](https://plsn.com/?p=314502)

### Inferences
- Absence of any price across three sources, combined with the "commercial integration partner" framing (Laserworld's commercial interest is selling ShowNET hardware and growing its compatible-software ecosystem, not monetising the SDK), strongly suggests the SDK itself is provided free under NDA. Treat as "no published fee / presumed free, unconfirmed."

### Gaps
- No explicit Laserworld statement that the SDK is free; a fee (one-off or per-key) cannot be fully ruled out until confirmed in the exchange.

## Bottom-line actionable plan (synthesis for the report writer)
1. **Register a Belgian sole proprietorship** (entreprise individuelle / eenmanszaak) via an accredited business counter — ~€105–€175, no notary, no capital, home address as seat, ~1 week. This produces the "legal business address" (a CBE/BCE enterprise number) the NDA demands.
2. **Contact Laserworld directly, not via the dead form.** Phone the Swiss HQ (+41 71 677 80 80) and/or reach Norbert Stangl (Sales/Marketing Director) via LinkedIn; note the broken `laser-interface.com/en/sdk` link in the message.
3. **Frame the request in their own qualifying words:** "a publicly available software project" seeking "commercial integration partner" access — never "private/hobby project." Supply the registered business address and a short project description up front.
4. **Open source is compatible:** the NDA protects the keys/SDK binary, not the app source. Keep the SDK + per-partner keys out of the public repo behind the off-by-default `shownet` feature; publish everything else under MIT/GPL. Request an explicit NDA carve-out confirming the applicant's own published code is not confidential.
5. **Fee:** none published; proceed expecting the SDK to be free, but confirm in writing.
