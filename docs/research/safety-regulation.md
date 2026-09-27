# Laser show safety and regulation, turned into software features

Research report for **Laser Studio**. Scope: what a (currently
non-commercial, later possibly public-facing) laser show operator in
**Belgium** has to respect, how other countries compare, what professional
software offers for safety, and the concrete features we should build.
Tasks derived from this report: **T-250 … T-264** (see section 8).

Public information only. Date of research: 2026-09-27.

> **Not legal advice.** Where a point is not confirmed by a primary source
> (a law, an official authority page or an official form) it is marked
> **(unverified)**. Before any public show, confirm with the authorities
> named below; rules change and several secondary sources are old.

---

## 1. Summary (what matters for us)

1. **Belgium has no specific law for indoor laser shows** that we could
   find. Audience scanning is not prohibited and needs no permit (secondary
   source, 2014–2020, **unverified** against a recent official text). The
   general duties still apply: product safety (IEC/EN 60825-1 classified
   and labelled projectors), worker protection when there are workers
   (Royal Decree of 22 April 2010 / Codex book V title 6), civil liability,
   and the municipality's event permit.
2. **FANC/AFCN is not the competent authority**: it covers *ionising*
   radiation. Laser light is non-ionising.
3. **Outdoor: any laser beam projected into airspace always needs an
   authorisation from the DGTA** (Direction générale Transport aérien, SPF
   Mobilité et Transports), under circular **GDF-12**, requested **at least
   20 and at most 60 working days before** the event, fee €129 (2026). This
   is the one hard, confirmed Belgian requirement.
4. The DGTA form asks for **beam count and horizontal/vertical angle
   ranges, WGS84 position, source height, laser class, power, divergence,
   wavelength**, and requires a **competent technician present the whole
   time and able to stop the beam at any moment**; the beam must be
   **deflected immediately if an aircraft is hit**; lasers must never
   dazzle pilots. These map directly to software features (sky-beam angle
   mask, instant "sky off", event log, exportable show sheet).
5. **MPE for audience exposure** (IEC 60825-1, visible light, 0.25 s
   aversion time): about **2.5 mW/cm²** for a stationary beam. ILDA uses
   **10 mW/cm²** for continuously scanned beams with no hot spots, and has
   *proposed* 100 mW/cm² under strict conditions (hardware scan-fail,
   trained operator). A raw show beam of a few watts is **hundreds to
   thousands of times** over the MPE: audience scanning is only possible
   with heavy attenuation, divergence and a **hardware** scan-fail
   system. Software alone cannot make it safe.
6. Laser Studio's job: make the safe path the default (disarmed, beams
   above the audience, per-output power caps), make risky modes explicit
   and hard to reach, stop instantly from every input, detect lost
   operator/UI and engine stalls, and keep a log.

---

## 2. Standards and technical basis

### 2.1 Standards

| Document | What it is | Use for us |
|---|---|---|
| **IEC 60825-1:2014** (Ed. 3), in the EU **EN 60825-1:2014 + A11:2021** | Product classification (Class 1, 1M, 1C, 2, 2M, 3R, 3B, 4), AEL, labelling, key switch, emission indicator, remote interlock for 3B/4 | Projector must be classified by its maker; our software never changes its class. Class 4 = beam, diffuse reflection and fire hazard. |
| **IEC 60825-3** "Guidance for laser displays and shows" (Ed. 2 2008; a newer edition exists, believed 2022 — **unverified**) | Guidance for show design: audience exposure, show zones, operator, documentation | Structure of our checklist and show sheet. |
| **IEC TR 60825-14** | User's guide (risk assessment, MPE use) | Background for the MPE helper. |
| **Directive 2006/25/EC** (artificial optical radiation) | Exposure limit values for workers | Transposed in Belgium by the Royal Decree of 22 April 2010. |
| **ICAO Annex 11 + Doc 9815** (Manual on Laser Emitters and Flight Safety) | Protected flight zones around airports | Outdoor shows. |
| **ILDA** audience scanning guidance | Practical industry guidance (not law) | Numbers and good practice. |

Class meaning for show projectors (IEC 60825-1): visible CW **Class 2 ≤ 1
mW**, **3R ≤ 5 mW**, **3B ≤ 500 mW**, **Class 4 > 500 mW**. Almost every
show projector is Class 4.

### 2.2 MPE for audience exposure

For visible light (400–700 nm), exposure time *t* between 18 µs and 10 s,
small source, IEC 60825-1 gives a corneal radiant exposure MPE of
**H = 18 · t^0.75 J/m²**. With the aversion response time t = 0.25 s:

- H = 18 × 0.25^0.75 ≈ 6.4 J/m² → irradiance E = H / t ≈ **25.5 W/m² =
  2.55 mW/cm²** (static beam).
- Through a 7 mm pupil (0.385 cm²) that is ≈ 1 mW: the same as the Class 2
  limit, which is why Class 2 is "safe by blink reflex".

ILDA values ([ILDA safe audience scanning](https://www.ilda.com/audiencescanningsafety.htm)):

| Case | ILDA value | Notes |
|---|---|---|
| Static beam / hot spot | **2.5 mW/cm²** | 0.25 s aversion time |
| Continuously scanned, no hot spots | **10 mW/cm²** | "1× MPE" scanning practice |
| "Level 2 / 10×" proposal | **100 mW/cm²** | *Proposal only*: requires a scan-fail system cutting the beam within **10 ms**, trained operator with E-stop, no hot spots, signage, approvals |

Measurement (ILDA): calibrated meter, ~1 cm² aperture (older 7 mm), at the
**closest audience position**, all colours at full show intensity, beam
static. Scanned effects must be evaluated for **single pulse,
multiple pulse and average power** MPE ([ILDA overview PDF](https://www.ilda.com/resources/Safety/audience-scanning_overview_latest.pdf)).
The worst case of a scanned figure is where the beam moves **slowest**:
corners, edges, turning points and dwell points — exactly what our
`densify` corner dwell produces. A figure that shrinks to a point is a
static beam.

**Consequence for software:** a shape's "safety" depends on its *size,
speed and dwell*, not only on brightness. An effect that is fine at 40 %
size can be dangerous at 2 % size. A scanner failure (galvo stuck) turns
any figure into a static beam: only hardware (scan-fail circuit such as
Pangolin's PASS, which needs position feedback) can catch this in < 10 ms.
We have no galvo feedback from ShowNET/IDN/Ether Dream, so **Laser
Studio must never claim to provide scan-fail protection**.

---

## 3. Belgium

### 3.1 Who is (and is not) competent

| Body | Role for laser shows | Status |
|---|---|---|
| **FANC/AFCN** (Agence fédérale de contrôle nucléaire) | Ionising radiation only. Not competent for show lasers | Confirmed: FANC's mission is ionising radiation; laser is non-ionising ([Beswic on radiation types](https://www.beswic.be/nl/themas/beschermingsmiddelen/collectieve-beschermingsmiddelen-cbm/categorieen-van-cbm/straling)) |
| **DGTA / DGLV** (SPF Mobilité et Transports, Service Airspace) | **Authorisation for any laser beam into airspace** | Confirmed, primary source ([mobilit.belgium.be — Skytracers et lasers](https://mobilit.belgium.be/fr/aviation/autorisations-et-agrements-organismes/organiser-un-evenement/skytracers-et-lasers)) |
| **skeyes** | Air navigation service; not the authority for ground-based lights ("to be obtained from DGTA, not from SPACC") | Confirmed (same page) |
| **SPF Emploi, Travail et Concertation sociale** | Workers' exposure (KB 22/04/2010, Codex book V title 6) | Confirmed ([werk.belgie.be — kunstmatige optische straling](https://werk.belgie.be/nl/themas/welzijn-op-het-werk/omgevingsfactoren-en-fysische-agentia/kunstmatige-optische-straling), [Codex V.6](https://werk.belgie.be/sites/default/files/content/documents/Welzijn%20op%20het%20werk/Regelgeving/Codex%20boek%20V%20titel%206%20Kunstmatige%20optische%20straling.pdf)) |
| **SPF Économie** | Market surveillance of laser *products* (EU product safety rules; laser pointers) | General knowledge; details for show projectors **unverified** |
| **Municipality (bourgmestre, police, fire service)** | Event permit, public order, fire safety; the DGTA page itself says to contact the commune for "ground safety, public order, environment" | Confirmed (DGTA page). Content of municipal conditions varies per commune |
| **Flanders VLAREM II / Brussels / Wallonia environmental rules** | Cover **sound** levels at music events, not lasers | Confirmed for sound ([Flemish musical activities rules](https://publicaties.vlaanderen.be/view-file/71525)); nothing on lasers found |

### 3.2 Indoor shows and audience scanning

- No Belgian law specific to laser shows or audience scanning was found.
  lasershowsafety.info (compiled 2014, updated to 2020) states:
  *"Audience scanning is allowed without a permit (no specific laws or
  regulations)"* ([lasershowsafety.info](https://www.lasershowsafety.info/audscan.html)).
  **Unverified** against a recent official source; we found nothing
  contradicting it.
- The absence of a permit does **not** remove liability: an injury is a
  civil (and possibly criminal) matter. Insurers and venues may impose
  their own conditions (risk assessment, MPE measurement, certified
  operator).
- Belgian laser show companies publicly state their own rules: only
  EN/IEC 60825-1 certified projectors, CW lasers, MPE respected, "location
  rules depend on country, province, city, municipality or venue"
  ([Laser XL](https://www.laserxl.be/regelementering/)). Their claim that an
  operator must hold a "5-year LSO certification" is **not** backed by any
  law we found — treat as marketing.

### 3.3 Workers and the "laser safety officer"

- **KB 22 avril 2010** (M.B. 6 May 2010), now Codex book V title 6,
  transposes Directive 2006/25/EC: the **employer** must assess optical
  radiation risk, keep exposure below the exposure limit values, inform
  and train workers, and involve the prevention advisor.
- Belgium does **not** appear to define a legally named "laser safety
  officer" role the way Germany does (**unverified**). A Belgian training
  provider claims an LSO is mandatory as soon as one employee *or* a
  self-employed manager is exposed to 3B/4 radiation
  ([LaserCollege](https://lasercollege.be/regelgeving-voor-het-gebruik-van-lasers-in-belgie/));
  we could not confirm this for a self-employed person with no staff.
- For a hobbyist with no workers, the Codex does not formally apply, but
  the DGTA form (outdoor) requires "un technicien compétent et formé aux
  risques spécifiques des lasers … présent pendant toute la durée … et à
  tout moment en mesure d'arrêter la projection".

### 3.4 Outdoor: DGTA authorisation (confirmed, primary sources)

From the DGTA page and the official form "Installation d'un laser ou d'un
projecteur" ([form, FR, v1.0](https://mobilit.belgium.be/sites/default/files/documents/publications/2023/Lasers_Application_Form_1105_FR_v1.0.docx)):

- *"Projeter un rayon laser dans l'espace aérien … il faut **toujours**
  obtenir une autorisation."* (Searchlights only in "Zone 2" or above
  3000 W; lasers always.)
- Submit **≥ 20 and ≤ 60 working days** before the event, to
  `BCAA.aerialactivities@mobilit.fgov.be`. Fee (2026): **€129**, invoiced.
- The form asks for: applicant; **a person present during the projection
  and reachable on mobile at all times**; event description; address and
  **WGS84 coordinates**; **height of the source above ground**; dates,
  start/end times, duration; **number of beams and, for each beam, the
  horizontal and vertical angle range (from … to …)**; IEC 60825 class;
  CW/pulsed; laser type, colour, **max power (W)**, **beam diameter (cm)**,
  **divergence (mrad)**, **wavelength (nm)**.
- Safety measures (annex required): *"si un aéronef est touché par le
  faisceau, le faisceau doit être immédiatement dévié"*; no dazzle risk for
  drivers or people; for lasers, *"le demandeur doit démontrer … que le
  laser ne peut à aucun moment éblouir les pilotes"*.
- Declarations (GDF-12 §5.2): for light beams, **max 45° from vertical**;
  landowner consent; all other permits obtained; competent technician
  present throughout and able to stop at any time.
- Mandatory attachments: full technical description, full description of
  the safety measures, landowner authorisation, **insurance certificate**.
- The authorisation covers **airspace only**; everything else (municipal
  permit etc.) remains the applicant's responsibility.

Open question (**unverified**): whether a beam that is fully terminated on
a building or screen outdoors counts as "projected into airspace". Safe
reading: any outdoor beam that can reach the sky needs the authorisation;
ask the DGTA when in doubt.

### 3.5 Aviation limits (ICAO, used by EU states)

ICAO Annex 11 / Doc 9815 protected zones
([CASA AC 139.E-03](https://www.casa.gov.au/laser-emissions-which-may-endanger-safety-aircraft),
[FAA AC 70-1B](https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC.70-1B_Outdoor.Laser.Operations.pdf)):

| Zone | Max irradiance | Extent (typical) |
|---|---|---|
| Laser-free flight zone (LFFZ) | 50 nW/cm² | ≤ 600 m (2000 ft) AGL, 3.7 km (2 NM) around runways + 5.6 km (3 NM) approach extensions |
| Critical flight zone (CFZ) | 5 µW/cm² | ~18.5 km (10 NM), ≤ 3000 m (10 000 ft) |
| Sensitive flight zone (SFZ) | 100 µW/cm² | defined by the state |
| Normal flight zone | MPE (≈ 2.5 mW/cm²) | elsewhere |

A multi-watt beam stays above 50 nW/cm² for many kilometres, so near an
airport an outdoor laser show may simply be refused or restricted to
terminated beams. Software can help by enforcing **declared angle ranges**
and an instant **sky-beams-off** control.

---

## 4. Comparison with other countries

| Country | Indoor / audience scanning | Operator | Outdoor / aviation | Sources |
|---|---|---|---|---|
| **Netherlands** | No specific law except for laser pointers; Arbo (workplace) rules; practice: beams ≥ 3 m above floor (**unverified**) | Municipality may require "laser-deskundige" | **Lichtshow must be notified to ILT (aviation)**, form "Aanmelden lichtshow", ~3 weeks processing | [ILT lichtshows](https://www.ilent.nl/onderwerpen/luchtvaart/luchtvaartinfrastructuur/lichtshows), [VNG FAQ](https://vng.nl/artikelen/veelgestelde-vragen-lasershows), [lasershowsafety.info](https://www.lasershowsafety.info/audscan.html) |
| **Germany** | Allowed within MPE; OStrV + TROS Laser; DGUV Information 203-036 (2021) is the reference for show lasers | **Written appointment of a Laserschutzbeauftragter** (trained per DGUV Grundsatz 303-005) for 3R/3B/4 | State aviation authority approval for sky beams (**unverified** detail) | [DGUV 203-036](https://publikationen.dguv.de/regelwerk/dguv-informationen/99/laser-einrichtungen-fuer-show-oder-projektionsanwendungen), [lasershowsafety.info](https://www.lasershowsafety.info/audscan.html) |
| **UK** | No prior permission needed for audience exposure; venues require risk assessment and method statement; PLASA "Safety of Display Lasers" replaced HSE HS(G)95; many venues demand PASS or divergence lenses | Competent person | **CAA notification** (CAP 736, form DAP1918); NOTAM issued | [HSE INDG224](https://www.hse.gov.uk/pubns/indg224.htm), [PLASA](https://www.plasa.org/guidance-for-display-lasers/), [CAA outdoor lasers](https://www.caa.co.uk/commercial-industry/airspace/event-and-obstacle-notification/commercial-displays-and-events/outdoor-laser-lights-and-fireworks/), [CAP 736](https://www.caa.co.uk/data-and-publications/publications/documents/content/cap-736/) |
| **France** | New arrêté of **18 June 2026** (replacing 11 Dec 2009) for ERP: 3B/4 no shots at the public; keep **≥ 3 m above and 2.5 m around** the public zone; key switch; safety officer present all show long; approval dossier incl. plans | "Responsable de sécurité laser" present | Outdoor: GPS coordinates and aeronautical risk assessment | Secondary source only ([Grimedif](https://www.grimedif.com/actualites/laser-spectacle-reglementation-larrete-du-18-juin-2026/)) — **verify on Légifrance** |
| **USA** | Class IIIb/IV shows need an **FDA variance** (Form FDA 3147) from 21 CFR 1040.11(c); audience scanning **not permitted** unless the variance specifically allows it | Variance holder responsible | **FAA** notification (AC 70-1B, Form 7140-1) | [FDA laser light shows](https://www.fda.gov/radiation-emitting-products/home-business-and-entertainment-products/laser-light-shows), [Form 3147](https://www.fda.gov/media/72256/download), [FAA AC 70-1B](https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC.70-1B_Outdoor.Laser.Operations.pdf) |

Takeaway: Belgium is among the most permissive for indoor use, but the
common denominator everywhere is **MPE + a competent operator who can stop
the show at once + documentation**; outdoors, **aviation authorisation**.
Our features should produce the evidence (logs, show sheet) that any of
these regimes asks for.

---

## 5. What professional software and hardware offer

| Feature | Who | Notes |
|---|---|---|
| **Safety zones / masks** (blank or dim polygons, soft edges), horizon dimmer | Pangolin (projection/safety zones), Showcontroller (up to 5 zones, see `showcontroller.md` §1.8), MadLaser (safety area + masks with opacity, see `madmapper-and-open-content.md`) | Already our T-003. |
| **Beam Attenuation Map (BAM)** | Pangolin, all versions (patented) | Grid of attenuation over the field; e.g. −50/−70 % in audience areas ([Pangolin](https://pangolin.com/blogs/news/creating-safe-laser-shows)). Our T-003 "Dim" zones cover the idea; we do not copy the BAM implementation. |
| **Output enable / master blackout** in the toolbar, disabled at start | Pangolin, Showcontroller | We have arm/disarm + Escape. |
| **Timecode arm button** "for safety reasons" | Showcontroller (see `pro-live-operation.md`) | A timecoded show must not start output on its own. |
| **Per-projector max output / colour limits** | Pangolin, Showcontroller | Our T-254. |
| **PASS** — hardware scan-fail and system supervisor in the projector, redundant circuits (power, light level, scanner dynamics, logic) | Pangolin (hardware) | Needs galvo feedback; **not** doable in our software. |
| **SafetyScan lens** — divergence lens for the lower half of the field | Pangolin (hardware) | Reduces irradiance in audience; checklist item. |
| **Measurement tools** (LOBO, Scanguard) | Third party | We can only provide an *estimator* (T-257). |
| **Hardware safety zones in the DAC** | ShowNET firmware (DMX ch 16–17/19–20) | Out of reach until the official API; do not depend on it. |

Professional practice also includes: key switch and **remote interlock /
E-stop** on the projector (IEC 60825-1 for 3B/4), signage and
announcements, video recording of the setup, **MPE measurement at the
closest audience point**, a written **risk assessment**, and logs.

---

## 6. Gaps in Laser Studio today

Reading `studio/src` (2026-09-27):

1. **Escape and arming live only in the browser.** If the tab is closed,
   the browser freezes or the Mac sleeps, the engine keeps sending the
   last look while `armed = true`. There is no server-side heartbeat.
2. **Arming is a single boolean** (`POST /api/arm {on}`); no reasons, no
   interlocks, no record of *who/what* armed or disarmed.
3. **No power ceiling per output**: `brightness` 0..1 is a look setting;
   anything (MIDI, cues, LFOs) can drive it to 1.0. T-003 adds zones and
   colour gain, T-208 bounds MIDI brightness "by the safety maximum", but
   no task defines that maximum per output.
4. **No guard against a figure collapsing to a point** (size 0, zoom
   modifiers, a paused beam effect at full power). Corner dwell makes it
   worse at corners.
5. **No watchdog**: an engine stall or panic leaves the DAC with whatever
   it last had (DAC behaviour differs per device).
6. **No log, no checklist, no outdoor constraints, no safety profiles.**

Existing tasks: T-003 (zones, horizon, colour gain, min diode level),
T-101 (strobe limiter, provisional beam floor), T-208 (MIDI safety),
T-171 (scan speed limits). The new tasks below **extend** these and do not
redo them.

---

## 7. Proposed features

Pipeline order (extends `pro-live-operation.md`): generators → live
modifiers → calibration → **T-003 zones/horizon** → **power caps (T-254)**
→ **dwell guard (T-256)** → **sky mask (T-262)** → **gate (T-250:
armed + no e-stop + interlocks OK, else blank)** → output. Preview shows
the same frame, plus overlays.

1. **Interlocks and arm reasons (T-250).** Arming becomes a request that
   succeeds only if every interlock is satisfied; each disarm carries a
   cause (`Échap`, `MIDI`, `UI perdue`, `moteur bloqué`, …). The UI shows
   why arming is refused.
2. **Latching emergency stop (T-251).** `POST /api/estop` (no body, handled
   first), Escape, a permanent red button, MIDI (T-208). Unlike a simple
   disarm it **latches**: re-arming needs an explicit "Réinitialiser l'arrêt
   d'urgence". Blank frame sent within one engine tick.
3. **Operator presence (T-252).** UI heartbeat every 500 ms; no heartbeat
   for 2 s while armed → blank + disarm. Optional **hold-to-run** (dead-man)
   mode: output only while a key/pedal/pad is held; mandatory for
   audience-scanning mode.
4. **Engine/output watchdog and clean shutdown (T-253).** Stall > 100 ms or
   panic → blank and disarm; SIGINT/SIGTERM/panic hook → blank frames then
   close; startup always disarmed (already a rule; test it).
5. **Per-output power caps (T-254).** Hard ceilings per output (global and
   per colour), plus projector data (class, mW per colour, divergence,
   aperture). Nothing downstream of settings can exceed them.
6. **Audience-scanning mode, locked by default (T-255).** Default
   "faisceaux au-dessus du public": a mandatory horizon/blank zone for the
   audience. Unlocking requires configured audience zones with an
   attenuation, completed checklist incl. hardware items (scan-fail,
   divergence lens, measured MPE), typed confirmation, hold-to-run on; it
   expires at session end and is never restored at startup.
7. **Dwell / minimum size / minimum speed guard (T-256).** Per-frame
   footprint and a 250 ms sliding "exposure" grid; a lit figure that
   collapses below a minimum extent, or a cell that stays lit too long, is
   dimmed then blanked. Strict inside audience zones and in audience mode,
   relaxed (only a cap) for intended static beams above the horizon.
8. **MPE / NOHD estimator (T-257).** Irradiance at a distance from power,
   aperture, divergence, attenuation; compared with 2.5 / 10 mW/cm²;
   NOHD. Always labelled "estimation — ne remplace pas une mesure".
9. **Pre-show checklist (T-258).** Required once per session before the
   first arm on a real device; editable list; stored in the log.
10. **Safety event log (T-259).** Append-only JSONL per day: arm/disarm with
    source and reason, e-stops, interlock trips, limiter activations, cue
    launches while armed, safety setting changes, checklist, mode unlocks.
    Viewer and export.
11. **Safety profiles per venue (T-260) and settings lock (T-261).**
    Profiles bundle zones, caps, strobe limits, audience-mode permission,
    outdoor settings and checklist; "Aperçu sûr" default. A PIN lock
    prevents changing safety settings during a show; changing profile
    disarms.
12. **Outdoor / airspace mode (T-262).** Projector mounting (scan angle,
    tilt, azimuth, height) to convert points to angles; declared beam
    angle ranges (as on the DGTA form) enforced as a mask; max 45° from
    vertical for sky beams; a **"Ciel coupé"** instant control (blank all
    sky beams, keep the rest) for aircraft; outdoor mode requires an
    authorisation reference in the checklist.
13. **Show safety sheet export (T-263).** Printable HTML with everything
    the DGTA form and a venue risk assessment ask for.
14. **Safety invariants test suite (T-264).** Property/fuzz tests: no lit
    point while disarmed or e-stopped, in Blank zones, above caps, outside
    declared sky angles; e2e for heartbeat loss and e-stop latch.

Things we deliberately **do not** do: claim scan-fail protection, compute a
"certified safe" verdict, or read ShowNET hardware safety zones (encrypted
protocol, CLAUDE.md).

---

## 8. Task list

| id | title | depends on |
|---|---|---|
| T-250 | Verrous d'armement (interlocks) et raisons de désarmement | — |
| T-251 | Arrêt d'urgence verrouillé (clavier, bouton, API, MIDI) | T-250 |
| T-252 | Présence opérateur : battement de l'interface et mode maintien | T-250 |
| T-253 | Chien de garde du moteur et extinction propre | T-250 |
| T-254 | Plafonds de puissance par sortie et fiche projecteur | T-250 |
| T-255 | Mode balayage public verrouillé par défaut | T-003, T-250, T-252, T-254, T-256, T-258 |
| T-256 | Garde anti-point fixe (taille minimum, vitesse, temps de pose) | T-250, T-003 |
| T-257 | Estimateur d'exposition (EMP) et distance de danger (DNRO) | T-254 |
| T-258 | Liste de contrôle avant show | T-250, T-259 |
| T-259 | Journal des événements de sécurité | — |
| T-260 | Profils de sécurité par lieu | T-003, T-254, T-101 |
| T-261 | Verrouillage des réglages de sécurité par code | T-260, T-259 |
| T-262 | Mode extérieur : angles déclarés, « Ciel coupé », limite 45° | T-003, T-250, T-254, T-259 |
| T-263 | Fiche sécurité du show exportable | T-254, T-258, T-262 |
| T-264 | Suite de tests des invariants de sécurité | T-250, T-251, T-252, T-253, T-254, T-256 |

---

## 9. Sources

Belgium
- DGTA — Skytracers et lasers: https://mobilit.belgium.be/fr/aviation/autorisations-et-agrements-organismes/organiser-un-evenement/skytracers-et-lasers
- DGTA form "Installation d'un laser ou d'un projecteur" (FR v1.0): https://mobilit.belgium.be/sites/default/files/documents/publications/2023/Lasers_Application_Form_1105_FR_v1.0.docx
- SPF Emploi — kunstmatige optische straling: https://werk.belgie.be/nl/themas/welzijn-op-het-werk/omgevingsfactoren-en-fysische-agentia/kunstmatige-optische-straling
- Codex boek V titel 6: https://werk.belgie.be/sites/default/files/content/documents/Welzijn%20op%20het%20werk/Regelgeving/Codex%20boek%20V%20titel%206%20Kunstmatige%20optische%20straling.pdf
- Prevent — regelgeving kunstmatige optische straling: https://www.prevent.be/nl/kennisbank/regelgeving-over-kunstmatige-optische-straling
- Beswic — straling: https://www.beswic.be/nl/themas/beschermingsmiddelen/collectieve-beschermingsmiddelen-cbm/categorieen-van-cbm/straling
- LaserCollege — regelgeving lasers in België: https://lasercollege.be/regelgeving-voor-het-gebruik-van-lasers-in-belgie/
- Laser XL — reglementering: https://www.laserxl.be/regelementering/
- Flanders — noise rules for musical activities: https://publicaties.vlaanderen.be/view-file/71525

Standards, ILDA, industry
- ILDA — safe audience scanning: https://www.ilda.com/audiencescanningsafety.htm
- ILDA — Scanning audiences at laser shows (PDF): https://www.ilda.com/resources/Safety/audience-scanning_overview_latest.pdf
- lasershowsafety.info — audience scanning by country: https://www.lasershowsafety.info/audscan.html
- Pangolin — creating safe laser shows: https://pangolin.com/blogs/news/creating-safe-laser-shows
- Pangolin — audience scanning safety: https://fr.pangolin.com/blogs/education/audience-scanning-safety
- Wikipedia — Audience scanning: https://en.wikipedia.org/wiki/Audience_scanning

Other countries and aviation
- ILT (NL) — lichtshows: https://www.ilent.nl/onderwerpen/luchtvaart/luchtvaartinfrastructuur/lichtshows
- VNG (NL) — FAQ lasershows: https://vng.nl/artikelen/veelgestelde-vragen-lasershows
- DGUV Information 203-036 (DE): https://publikationen.dguv.de/regelwerk/dguv-informationen/99/laser-einrichtungen-fuer-show-oder-projektionsanwendungen
- HSE INDG224 (UK): https://www.hse.gov.uk/pubns/indg224.htm
- PLASA — guidance for display lasers (UK): https://www.plasa.org/guidance-for-display-lasers/
- UK CAA — outdoor lasers: https://www.caa.co.uk/commercial-industry/airspace/event-and-obstacle-notification/commercial-displays-and-events/outdoor-laser-lights-and-fireworks/
- UK CAA — CAP 736: https://www.caa.co.uk/data-and-publications/publications/documents/content/cap-736/
- France — arrêté du 18 juin 2026 (secondary summary): https://www.grimedif.com/actualites/laser-spectacle-reglementation-larrete-du-18-juin-2026/
- FDA — laser light shows: https://www.fda.gov/radiation-emitting-products/home-business-and-entertainment-products/laser-light-shows
- FDA Form 3147: https://www.fda.gov/media/72256/download
- FAA AC 70-1B: https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC.70-1B_Outdoor.Laser.Operations.pdf
- CASA AC 139.E-03 (ICAO zones): https://www.casa.gov.au/laser-emissions-which-may-endanger-safety-aircraft
