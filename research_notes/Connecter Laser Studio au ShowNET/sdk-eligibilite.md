# Laserworld ShowNET SDK/API — Eligibility & Application for a Small/Individual Open-Source Developer

> Scope: Can a solo/hobbyist dev or a small public open-source project (e.g. "Laser Studio") legitimately get the official ShowNET SDK (library + per-partner key), and exactly how? Current as of Oct 2026. Official channels only.

> **Note on source integrity:** The Laserworld SDK page (`shownet-api-sdk.html`), when fetched, contained embedded text that tried to instruct the fetcher to follow formatting rules — i.e. a prompt-injection artifact. It was ignored; only the page's actual content about the SDK was used. The verbatim quotes below are from that page's real content, which was consistent across multiple fetches.

---

## Q1. Exact eligibility criteria stated on Laserworld's ShowNET SDK page (verbatim)

### Takeaway
Access is explicitly limited to "commercial integration partners," but the page carves out a clear welcome for **publicly available software projects** ("this is exactly for you"), while **private projects are explicitly not supported**. So a public open-source project like Laser Studio is, on the page's own wording, inside the intended audience — the dividing line Laserworld draws is public vs. private, not company vs. individual.

### Cited Findings
- "The API data is only provided to commercial integration partners." — [Laserworld ShowNET API/SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "If you have a publicly available software project, this is exactly for you." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "We unfortunately cannot provide further supprot for private projects." (sic) — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Hobbyist steer: "If you just want to play with the API fo your private prjects, you probably want to check out ... ready made integrations, like in TouchDesigner, that give you a lot of freedom in what you can do." (sic) — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- The same page also mentions "industrial use cases with custom software control" as a category Laserworld is willing to discuss. — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html); [Launch announcement, 2022](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html)

### Inferences
- The two decisive filters are (a) the project must be **publicly available** (not private/personal experiments) and (b) Laserworld frames partners as "commercial integration partners." A public, downloadable open-source app plausibly satisfies (a); whether Laserworld reads "commercial" strictly (requires a business) or loosely (any publicly shipped software) is the crux and is not resolved by the page text — see Q2/Q3.
- "Private projects" in their wording means closed personal tinkering, not "non-corporate." A GitHub-published app is the opposite of a "private project" in their framing.

### Gaps
- The laser-interface.com/en/sdk registration page (where the announcement says to "register for receiving more details and individual access keys") could not be retrieved — the URL 301-redirects to a broken/mangled Laserworld manual URL. The actual registration form content is unverified.

---

## Q2. Does an individual/hobbyist with only a public GitHub project qualify, or is a registered company required?

### Takeaway
The page wording welcomes a **public** software project and rejects **private** ones; it never states in words that a registered company is mandatory. However, the NDA requirement of a "legal business address" (Q3) plus the label "commercial integration partners" is the practical barrier that may exclude a pure hobbyist with no business entity. No source definitively confirms an individual has been let in.

### Cited Findings
- Public project = intended audience; private project = unsupported (quotes under Q1). — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- The application asks for "some details about your project as well as the legal business address for the NDA." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- "Publicly available software project, this is exactly for you" strongly implies a solo dev shipping an open-source app is in-scope on the project-type test. The open question is purely the "legal business address for the NDA" — whether a private individual's address satisfies it.
- Net: eligibility is not flatly closed to individuals by the stated criteria; it hinges on the NDA address requirement and on Laserworld's discretionary approval.

### Gaps
- No source states whether Laserworld has ever granted keys to a developer with no registered company. Unverified either way.

---

## Q3. The NDA and "legal business address" — does it require a registered business? Any cost?

### Takeaway
Each partner signs a "little NDA" and provides a "legal business address" so keys stay inside the project. Whether a personal/sole-trader address is accepted is **not stated anywhere public** — this is the single most important unknown for the user and must be asked of Laserworld directly. No fee is mentioned on any page; "free of charge" is an internal assumption, not confirmed by the source pages.

### Cited Findings
- "To make sure that the keys are kept withing the development project, we make a little NDA with each integration partner." (sic) — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- "To integrate the API, it is required to use a set of keys, which are individually created per integration partner." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Application requires "the legal business address for the NDA." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- No pricing/fee is stated on the SDK page or the 2022 announcement. Multiple targeted searches for "free of charge" returned nothing confirming the SDK is free. — [Launch announcement](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html); [PLSN coverage](https://plsn.com/?p=352037)

### Inferences
- "Legal business address" literally reads as a business address, which points toward needing some registered entity (in the EU/DE context, that could be a sole proprietorship / Einzelunternehmen rather than a full company). But the term may be used loosely to mean "a real, legally valid postal address for the signatory." This ambiguity is exactly what the user must clarify with Norbert.
- A sole-proprietor / self-employed registration (cheap or free in many jurisdictions) would likely satisfy a "legal business address" if a plain personal address does not — a plausible low-cost path, but unconfirmed.
- Because keys are per-partner and NDA-bound, they **cannot be committed to a public repo** — consistent with the project's own IP rules (SDK material stays out of git, feature-gated).

### Gaps
- Whether a private individual's home address (no business) is accepted for the NDA: unknown, no public source.
- Whether the SDK/keys carry any fee: unconfirmed. Not stated as free anywhere found.

---

## Q4. Step-by-step application process

### Takeaway
Two stated entry points: (1) the laser-interface.com/en/sdk page to "register ... for individual access keys," and (2) emailing Laserworld's named contact "Norbert" with project details + legal business address for the NDA. Laserworld is widely reported (and reflected in the project's own notes) as slow/unresponsive.

### Cited Findings
- "register for receiving more details and individual access keys" at laser-interface.com/en/sdk. — [Launch announcement, 2022](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html); [PLSN](https://plsn.com/?p=352037)
- Direct route: "please feel free to reach out to Norbert and provide some details about your project ... as well as the legal business address for the NDA." (email address is obfuscated on the page). — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- Info to provide: (a) details about your project, (b) legal business address for the NDA. Partner then receives individually-created keys after signing the NDA. — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- Practical sequence: visit laser-interface.com/en/sdk and/or email "Norbert" at Laserworld → send a short description of the public open-source project + a legal/postal address → Laserworld sends a short NDA to sign → on signature, they issue per-partner keys + the platform library.
- Expect delays and possibly no reply (consistent with the project's CLAUDE.md note that "Laserworld has not answered so far"). Persistence / following up is likely necessary.

### Gaps
- No public figure for expected response time. The registration form's exact required fields are unverified (page unreachable).
- The contact email is obfuscated on the page; it must be read directly on the live site (likely Norbert @ the laserworld/laser-interface domain).

---

## Q5. Public evidence of a small/individual/open-source developer actually receiving the SDK + keys

### Takeaway
**No positive evidence found.** No forum post, GitHub repo, blog, or interview shows an individual or small open-source project receiving ShowNET SDK keys. The one named, credible independent laser developer found — Seb Lee-Delisle (Liberation laser software) — deliberately chose **not** to build on the SDK because of its closed, approval-dependent nature. The only confirmed integrators are commercial products (TouchDesigner/Derivative, MadLaser/MadMapper).

### Cited Findings
- Seb Lee-Delisle (author of **Liberation** laser software), on his support forum: "the communication method used by ShowNET is a closed protocol." — [Liberation Laser forum thread](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- He describes third-party support (TouchDesigner, MadLaser) as relying on a "manufacturer-controlled SDK" (precompiled code from Laserworld), and that "Distribution and access depend on their approval, and those terms can change, be restricted, or withdrawn at any time." — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- He states a "deliberate decision not to build Liberation on top of that kind of dependency," and would integrate only if Laserworld openly documents the protocol or adopts an open one like IDN. — [Liberation Laser forum](https://forum.liberationlaser.com/t/are-laserworld-ds-1000-mk4-s-supported/2126)
- Confirmed commercial integrator: TouchDesigner/Derivative added native ShowNET support via a new "Laser Device CHOP"; "no additional licenses need to be purchased on top." — [Derivative community post](https://derivative.ca/community-post/shownet-laser-support/68339); [Laserworld TouchDesigner page](https://www.laserworld.com/en/shownet-compatible/touchdesigner); [Laserworld news 5805](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- Searches of GitHub, photonlexicon, reddit r/lasers, and TouchDesigner forums returned **no** instance of an individual/open-source dev obtaining the SDK. No public ShowNET SDK repo exists. — [search results, Laserworld SDK pages only](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)

### Inferences
- The absence of any individual success story, plus a respected independent developer publicly declining the SDK on principle, suggests the realistic/common path for small devs is **not** the SDK but established commercial integrations — and that getting keys as a solo dev is, at minimum, undemonstrated in public.
- The TouchDesigner route being license-free to the end user does not mean the SDK itself is free to the integrator; it means Derivative (the commercial partner) absorbed the integration.

### Gaps
- Could not reach photonlexicon.com directly (search surfaced nothing relevant). Possible but unconfirmed that private forum discussions exist.
- No interview/testimonial found from any SDK partner (large or small) describing the onboarding experience.

---

## Q6. Platforms covered and cost

### Takeaway
Library ships for Windows (x86 + x64) and macOS (Intel + Apple Silicon/M1); a Linux build exists but is marked experimental. Cost to the developer/partner is **not stated** on any public page — "free" is unconfirmed.

### Cited Findings
- "The ShowNET API / SDK is available for Windows x86 and x64 Systems as well as for Mac OSX with Intel and M1 processors." — [Launch announcement, 2022](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html); [PLSN](https://plsn.com/?p=352037)
- SDK page adds a Linux version "is available (experimental)." — [Laserworld SDK page](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html)
- macOS Apple Silicon (M1) is explicitly covered — relevant to Laser Studio's macOS target. — [Launch announcement](https://laserworld.com/en/newslist/5625-shownet-api-sdk-available.html)

### Inferences
- Platform coverage matches Laser Studio's needs (macOS Intel + Apple Silicon), so the SDK is technically viable for this project if keys are granted.

### Gaps
- Pricing/fee: not published. The internal assumption that the SDK is "free" is not corroborated by any source found; treat as unknown until Laserworld confirms.
