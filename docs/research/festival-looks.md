# Research: festival laser looks, a design reference for Laser Studio

*Research agent report. Date checked: 2026-09-27. Public sources only: trade
press (TPi, PLSN), manufacturer and rental-house pages (Pangolin, Kvant,
Laserworld, ER Productions), laser-safety pages, festival write-ups and forum
threads. No show files, cues or frames were copied from anyone. Everything
below describes looks in words and maths so we can build them ourselves.*

**What the sources support, and what is our own practice.** The trade press
tells us which companies, fixtures, projector counts and placements were used
(for example 138 lasers at Creamfields Steel Yard, more than 30 at EDC Kinetic
Field, 12 AT-30s on the Swedish House Mafia ring). It also gives us the effect
vocabulary (fans, tunnels, sheets, liquid sky, finger beams, slats, hot beams)
and the design philosophy ("a distinct special effect for certain moments").
It almost never publishes numbers such as degrees per second or beats per
step. Those numbers (sections 4 to 6) are **our own starting values**, worked
out from galvo physics, tempo maths and how EDM tracks are built. They are
meant to be tuned by eye in the preview, not quoted as facts. I found no
public case studies for **Lightwerk** or **Tarm** (the only Tarm mention is
Laserworld's Flight Festival reference, with "two Tarm 11" projectors).

---

## 0. TL;DR

- Festival laser work is **beam work in haze**, not pictures. About 80% of
  what you see at Tomorrowland, EDC, Defqon.1 or Awakenings is a small set of
  aerial looks: **fans, tunnels/cones, sheets (liquid sky), single hot beams
  and chases**. It is played **in sync with the tempo** and in **symmetry
  across projectors**. Graphics, logos and text are rare special moments.
- The "pro" feel comes from **restraint and contrast**: darkness or one beam
  in the breakdown, a build that speeds up with the snare roll, a
  **blackout on the last beat before the drop**, then everything at full
  power on beat 1. Designers describe lasers as "a very distinct special
  effect for certain moments" (Derek Abbott, SHM/ER Productions, PLSN), used
  "to create a dramatic moment in a set" (Awakenings founder, VICE).
- **Colour**: one colour per phrase (often green, white or cyan), two-colour
  splits for contrast, full rainbow only as a short deliberate moment.
  Changing colour on every beat with no musical reason looks cheap.
- **Everything is measured in beats**: rotations in turns per bar, sweeps in
  1, 2 or 4 bars per cycle, chases in 1/2 or 1/4 beat per step. Looks change
  on 8, 16 or 32-beat phrase boundaries.
- **Missing engine piece**: to encode any of this, Laser Studio needs a
  **beat clock** (BPM plus phase: tap, auto-detect or manual). Today it has
  beat events (`AudioFeatures.beat`) but no phase. The evolving cues in
  section 5 and the timelines in section 6 are written in beats, so they
  need `beat_pos: f64` (beats since the cue started) and `bpm`.
- With **one projector** (our current case) most multi-projector looks can
  still be approximated. Split the beam set into **virtual heads** (left
  group, centre group, right group) and chase or mirror between them inside
  one frame. Section 1 gives the one-projector version of every look.

---

## 1. Catalogue of festival looks

### Conventions used in this catalogue

- **Frame**: our normalized scan field, x and y in -1..1, **+y = up**. The
  projector sits at stage level and points over the audience, so the
  audience-safe zone is "above a horizon line". We call that line `y_h`
  (typically 0.0 to 0.2 after the zone and calibration are set). All beam
  looks below are designed to stay at or above `y_h`. Downward beams (mirror
  bounces, beams onto the stage) need a separate, calibrated zone.
- **Angles**: with a typical ±30° optical scan, **1.0 unit ≈ 30°**. So
  "±0.6" is roughly a 36° half-width fan.
- **Beams** = dwell points (our `dots: true` geometry). **Sheets/lines** =
  continuous scanned strokes (`dots: false`).
- **Beat maths**: `T_beat = 60 / BPM`. At 128 BPM a beat lasts 0.469 s, a bar
  (4 beats) 1.875 s and an 8-bar phrase (32 beats) 15 s.
- Existing generators in `studio/src/generators.rs` are named in
  `backticks` where one already gets close.

Each entry lists **what it looks like in haze**, **motion over time**,
**palette**, **when in a track**, **one projector vs several**, and
**starting parameters**.

---

### A. Beam fans (the backbone)

**1. Static fan ("finger fan", "beam fan", "fingers")**
- *Haze*: N discrete beams spread horizontally like the fingers of a hand
  or a peacock tail, from the projector up and out over the crowd. At
  festivals, 6 to 16 beams per head is typical.
- *Motion*: none, or a very slow breath of the spread. It is a "position"
  look: it holds, then snaps to the next position on the beat.
- *Palette*: single colour; white or green reads as the most powerful.
- *Track*: intros, and the first bars of a drop when it is held and strobed.
- *One projector*: N dots on the line `y = y0`, x from -w to +w. *Several*:
  each head shows the same fan, so they read as one wall of beams. Or use
  alternate colours per head.
- *Params*: N = 8 (range 5 to 16), w = 0.5 to 0.8, y0 = y_h + 0.25. Close
  to `beam_fan` with spread animation off.

**2. Fan sweep / pan ("sweeping fan", "wiper")**
- *Haze*: the whole fan pans left and right like a windscreen wiper over the
  audience.
- *Motion*: sinusoidal ping-pong. One full left-right-left cycle every 2 or
  4 beats in a drop, 8 or 16 beats in a breakdown. Sine easing (slows at the
  ends) looks smoother and more "moving head"-like than a linear ramp.
- *Palette*: any. A two-colour alternate (green and blue fingers) moves very
  well.
- *Track*: the main groove of a drop, and progressive house grooves.
- *One projector*: offset the fan by `x = A·sin(2π·beat_pos/P)`. *Several*:
  **mirror** (left heads pan the opposite way to right heads, so the fans
  cross in the middle) or **phase-offset** (each head 1/N cycle later: a
  travelling wave across the stage).
- *Params*: A = 0.25 to 0.45, P = 2, 4 or 8 beats.

**3. Fan tilt / lift ("rise", "sunrise lift")**
- *Haze*: the fan starts flat just above the crowd and tilts up towards the
  sky, or drops down from the sky.
- *Motion*: a slow ramp over 8 to 16 beats in build-ups (beams "rise with
  the riser"), or a fast snap back down on the drop.
- *Palette*: often warm (amber, red to white) at sunset sets; cold (cyan to
  white) in trance builds.
- *Track*: build-up. Very readable, because the crowd sees the beams go up
  as the riser pitch goes up.
- *One projector*: animate y0 from y_h to y_h + 0.7.
- *Params*: travel 0.4 to 0.7 over 8 or 16 beats, ease-in.

**4. Fan open/close ("spread", "fan out", "accordion")**
- *Haze*: beams start bundled into one thick beam, then spread into a wide
  fan, like opening a hand.
- *Motion*: open on the beat and close on the off-beat (pumping), or one
  slow open over a phrase.
- *Track*: the pump version is for drops, synced to the kick. The slow
  version is an intro reveal.
- *One projector*: w = w_min + (w_max - w_min)·envelope. `beam_fan` already
  does a sine version of this.
- *Params*: w from 0.02 to 0.7. Pump envelope = fast attack (≤ 1/16 beat),
  exponential decay over 1/2 beat.

**5. Scissor fan / crossing X ("scissors", "crossfire", "X-beams")**
- *Haze*: two fans or two beam groups pan in opposite directions so they
  cross, making an X that opens and closes.
- *Motion*: counter-phase sine, crossing every 1 or 2 beats in drops.
- *Palette*: two-colour split is the classic (left group red, right group
  blue, white where they cross).
- *Track*: drops and big chorus hits.
- *One projector*: two virtual groups of N/2 beams. Group L at
  `x = -c + A·sin(φ)`, group R at `x = +c - A·sin(φ)`. For true diagonal
  "X" beams, give each group a tilt too (y offset proportional to x).
- *Several*: stage-left and stage-right heads aim across the stage at each
  other (true crossfire). This is the signature look of wide mainstages.
- *Params*: A = 0.4 to 0.6, c = 0.3, cycle = 2 beats.

**6. Fan wave ("wave", "ripple", "snake")**
- *Haze*: beams in a line, each one's height following a travelling sine, so
  a wave rolls through the fan.
- *Motion*: wave speed about 1 wavelength per 2 beats. Amplitude follows the
  energy of the track.
- *Track*: grooves, melodic breakdowns (slow, low amplitude) and drops
  (fast).
- *One projector*: `beam_wave`. *Several*: continue the wave from head to
  head with a phase offset, so the wave travels across the whole rig.
- *Params*: N = 10 to 16, amplitude 0.15 to 0.4, wavelength = 1 fan width,
  speed 0.5 cycles/beat.

**7. Chaser ("chase", "running light", "knight rider", "ping-pong")**
- *Haze*: a fan where only one beam (or a small group) is on at a time. The
  lit beam runs left to right, bounces or wraps.
- *Motion*: 1 step per 1/2 beat (groove), 1/4 beat (build-up), 1/8 beat
  (just before the drop, as short bursts). Variants: **fill** (beams stay on
  until all are lit, then reset), **bounce**, **random**, **centre-out**.
- *Palette*: single colour, or white head with a coloured tail (tail = the
  previous 2 beams at 40% and 15%).
- *Track*: build-ups (accelerating), techno grooves (steady 1/2-beat chase).
- *One projector*: blank all dots except the chase index. The galvo still
  visits all positions (so the geometry stays stable), only the colour gate
  changes. *Several*: the most famous multi-head look. The chase runs **from
  head to head** across the stage, one head per step.
- *Params*: N = 8, step = 1/2 beat, tail = 2.

**8. Beam stabs on the kick ("hits", "pops", "flash on beat")**
- *Haze*: beams appear sharply on each kick drum and decay or cut before the
  next.
- *Motion*: attack on the beat, gate time 1/8 to 1/4 beat, or a fast
  exponential fade (hardstyle and hard techno love the hard gate).
- *Track*: drops of techno, hardstyle and hard dance. At Defqon.1 and
  Awakenings, beams on the kick are the main vocabulary.
- *One projector*: brightness = envelope(beat phase). Combine with any fan
  or tunnel. The existing `AudioReact.flash` is a start, but it should be
  **clocked to the beat phase**, not only to detected onsets.
- *Params*: gate = 0.2 beat, or decay τ = 0.12 beat. On off-beats: none.
  Variant "on-beat plus off-beat position change": a different position on
  every kick (see 9).

**9. Position hits / snap positions ("position chase", "busking positions")**
- *Haze*: every beat (or every 2), the whole fan **snaps** to a new position
  or shape: fan high, fan low, fan left, V shape, inverted V, a single beam.
  There is no visible travel between them because the beam is blanked
  during the move.
- *Motion*: a step sequence of 4 or 8 positions looped over 1 or 2 bars.
- *Track*: drops of big-room, hardstyle and techno. This is the core of
  "busking to the beat".
- *One projector*: a list of 4 to 8 geometric presets. On each beat, pick
  the next one (sequential or ping-pong), blanked between them.
- *Params*: 4 positions, 1 per beat. For "double time" at the end of the
  build, 1 per 1/2 beat.

**10. Strobing beams ("beam strobe", "flicker fan")**
- *Haze*: the fan strobes on and off at a fixed rate, which freezes any
  motion into a series of stills.
- *Motion*: 1/2 or 1/4 beat rate. 1/8 beat only for short bursts.
- *Track*: the last 2 to 4 bars of a build and the first bar of the drop.
- *One projector*: gate the whole frame. Duty 30 to 50%.
- *Params*: see the strobe safety note in section 4 (≤ 4 Hz sustained,
  faster only for a few seconds).

**11. Single hot beam ("hot beam", "pencil beam", "lightsaber")**
- *Haze*: one intense beam, very bright because all the projector's power
  goes into one point. It pierces the whole venue.
- *Motion*: slow pan or a slow circle, or a static beam with slow
  brightness breathing. Pangolin warns hot beams must **keep moving** over
  audiences (all the energy sits in one spot).
- *Palette*: white, pure green, or deep red for drama.
- *Track*: breakdowns, the "one moment of silence" before the drop, intro
  of the headliner.
- *One projector*: 1 dot, with a long dwell. *Several*: all heads converge
  on one point in the sky (see 12).
- *Params*: N = 1, pan amplitude 0.3, P = 16 beats. Safety: our engine
  already moves beams, but keep the look **above y_h** only.

**12. Converging beams / focal point ("shoot the point", "star point",
"pyramid")**
- *Haze*: beams from many heads (or many beams from one) meet at a single
  point above the stage or crowd, forming a pyramid or tent of light.
- *Motion*: the focal point moves slowly, or the beams "collapse" into the
  point on the drop and then explode outwards.
- *Track*: grand moments, the end-show, or the first hit of a big drop.
  ER's Kinekt array at Creamfields formed a giant inverted "V" logo from 104
  lasers (TPi), a large-scale version of this idea.
- *One projector*: impossible literally (all beams come from one origin).
  Approximate it as a **tight bundle opening into a wide fan** (4 reversed),
  or as a spoke star (see 15).
- *Params*: collapse over 1/2 beat, hold, burst over 1/4 beat on beat 1.

---

### B. Tunnels and cones

**13. Solid tunnel / cone ("tunnel", "cone", "funnel")**
- *Haze*: a circle scanned fast becomes a hollow cone of light coming out of
  the projector. The crowd sees a tube over their heads (from the side) or
  a ring on the ceiling or haze wall.
- *Motion*: slow drift of position, and the size breathing. The circle
  itself can rotate (visible when it has gaps or a colour gradient).
- *Palette*: single colour, or a rainbow ring as a deliberate moment.
- *Track*: breakdowns and trance builds (the "flying through the tunnel"
  feeling), and big trance drops.
- *One projector*: `tunnel` (circle, radius r). *Several*: several cones
  angled outward (a "fan of cones"), or all cones aimed at one spot.
- *Params*: r = 0.15 to 0.4, above `y_h`. Point rate: keep the circle ≥ 40
  Hz so the cone looks solid and does not flicker.

**14. Spikey / finger tunnel ("finger tunnel", "star tunnel", "crown")**
- *Haze*: N beams arranged on a circle, making a cone built of separate
  beams (like the ribs of an umbrella).
- *Motion*: rotates around its axis. This is the "rotating spokes" look
  when seen from the audience.
- *Track*: drops (fast rotation) and builds (slow, speeding up).
- *One projector*: `beam_circle`, but with a true circle (the current
  generator squashes y by 0.35, which reads as a tilted ring; keep that as
  an option).
- *Params*: N = 8 to 16, r = 0.25 to 0.4, rotation 1 turn per 4 beats
  (groove), 1 per beat (peak).

**15. Sunburst / rotating spokes ("sunburst", "star", "radial fan")**
- *Haze*: beams radiating from one origin in all directions within the
  upper half, like sun rays, often rotating or counter-rotating.
- *Motion*: slow rotation in intros, fast in drops. Alternating spokes can
  flash in two groups (odd/even) on the beat.
- *Palette*: warm (yellow, amber, white) for the "sunrise" moment, which is
  a Tomorrowland and Ultra staple at sunset or closing.
- *One projector*: a semicircle of N dots at radius 0.8 (upper half only,
  clipped at y_h), rotating while staying inside the upper half-plane
  (wrap spokes that leave the zone back in, or blank them).
- *Params*: N = 12 to 24, rotation 10 to 30°/s (slow), 180°/s (fast).

**16. Zooming / pumping tunnel ("tunnel zoom", "breathing tunnel")**
- *Haze*: the cone's radius pumps: it widens on each kick and shrinks after
  it, or it grows over a whole build and bursts open on the drop.
- *Motion*: kick pump (attack 1/16 beat, release 1/2 beat), or a 16-beat
  ramp from r = 0.05 (almost a single beam) to r = 0.5.
- *Track*: the tunnel that "zooms on the drop" is one of the most
  recognisable trance and big-room moments: a tiny, tight tunnel through the
  build, then the drop blows it wide open with a strobe.
- *Params*: r_min = 0.05, r_max = 0.45, pump depth 30%.

**17. Multi-tunnel / twisting tunnels ("double tunnel", "vortex", "helix")**
- *Haze*: two or more concentric or side-by-side cones, rotating in
  opposite directions, or a spiral drawn so the cone looks twisted.
- *Track*: psy-trance and trance peaks, and techno "hypnotic" sections.
- *One projector*: two circles (r1, r2), or a spiral (`vortex`, `helix`).
- *Params*: r1 = 0.2, r2 = 0.35, opposite rotation at 1 turn per 2 bars.

**18. Polygon tunnels ("triangle tunnel", "square tunnel")**
- *Haze*: a cone with a triangular or square cross-section, which reads as
  sharper and more "techno" than a round one.
- *Motion*: rotation, often stepping by 1/N turn per beat (a triangle
  snapping 120° on every kick).
- *One projector*: `polygon_tunnel`.
- *Params*: sides 3 or 4, snap rotation 360°/sides per beat.

---

### C. Sheets, ceilings and walls

**19. Liquid sky ("liquid sky", "laser ceiling", "sheet", "flat scan
ceiling")**
- *Haze*: a flat, continuous sheet of light just above the crowd's heads,
  a false ceiling. The haze swirling in it looks like liquid. Pangolin
  defines liquid sky as "the effect that happens when you can see fog or haze
  swirling in an aerial effect", best shown with sheet effects.
- *Motion*: very slow lowering and raising of the sheet (the ceiling comes
  down during the breakdown), a gentle ripple, and slow colour drifts.
- *Palette*: deep blue, cyan, violet, or green. It is often the "calm"
  colour of the set.
- *Track*: breakdowns, melodic moments, intros and the closing track.
  Lowering the ceiling during a breakdown and lifting it on the drop works
  very well.
- *One projector*: `liquid_sky` (a horizontal line at y0). A line scan
  across the full width. **Safety**: the whole point of the look is to be
  close above heads, so it must respect the zone (y_h) strictly.
- *Params*: y0 = y_h + 0.05 to 0.2, ripple amplitude 0.01 to 0.03, ripple
  speed 0.25 cycles/beat, height drift over 16 or 32 beats.

**20. Tilted sheet / "blade" ("sheet", "blade", "light plane")**
- *Haze*: a sheet angled up (like a ramp) or rotating around the projector
  axis, cutting through the room like a blade.
- *Motion*: a slow roll: the plane rotates 30 to 90° over 8 beats, or snaps
  between angles on the beat.
- *One projector*: a line through the centre, rotated by angle θ (a
  "propeller" limited to the upper half).
- *Params*: θ from -30° to +30°, 1 cycle per 4 bars.

**21. Laser curtain / wall ("curtain", "wall", "vertical sheet")**
- *Haze*: a vertical plane of light (a line scanned up and down) standing
  between stage and crowd like a see-through wall. Several curtains side by
  side make "corridors".
- *Motion*: curtains slide sideways (see 23) or stay static and pulse.
- *Track*: intros ("the stage behind a laser curtain"), and the DJ
  entrance.
- *One projector*: N short vertical strokes at fixed x values, from y_h to
  y_h + 0.6. The galvo can only do a few before flicker, so N ≤ 5.
- *Params*: N = 3 to 5, x spacing 0.35.

**22. Waterfall ("waterfall", "rain", "curtain drop")**
- *Haze*: lines or beams falling from high to low, one after another, like
  water pouring down, or a sheet that lowers itself in steps.
- *Motion*: each element falls from y = 0.9 to y = y_h over 1 to 2 beats,
  new elements start every 1/4 or 1/2 beat, staggered across x.
- *Palette*: cyan, blue or white (water). Rainbow waterfall as a finale.
- *Track*: breakdowns and the moment right after the drop (the "release").
- *One projector*: stroke list whose y positions scroll downward, wrapping.
  Similar to `grid_scan` turned vertical.
- *Params*: 4 to 8 elements, fall time 2 beats, spawn every 1/2 beat.

**23. Scanner line ("scan line", "scanner", "bar sweep")**
- *Haze*: one flat sheet (or a vertical curtain) moving up/down or
  left/right through the haze, like a scanning bar.
- *Motion*: linear, 1 pass per bar in grooves, 1 per beat in builds.
- *One projector*: `sweep` (vertical line moving in x), or a horizontal
  line moving in y (above y_h only).
- *Params*: travel over the full width, ping-pong or wrap.

**24. Slats / blanked sheet ("slats", "venetian", "dashed sheet")**
- *Haze*: a sheet broken into several segments by blanking, which reads
  like many beams but with the speed of a flat scan. LVR Optical describes
  slats as flat-scan speed with blanked areas.
- *Motion*: the gaps travel along the sheet (a moving "barcode").
- *One projector*: one line split into k segments, with the gap pattern
  offset over time.
- *Params*: k = 6 to 12, 50% duty, gap travel 1 segment per 1/2 beat.

**25. Aurora / morphing sheet ("aurora", "northern lights", "silk")**
- *Haze*: a sheet whose shape slowly bends and flows. Reviews of the
  Tomorrowland mainstage describe "aurora-style tunnels".
- *Motion*: very slow (one full morph per 8 to 16 beats), with smooth
  gradients (green to cyan, or violet to blue).
- *Track*: intros, ambient moments, melodic techno (Afterlife-style sets).
- *One projector*: a line whose y follows a sum of 2 or 3 slow sines with
  different phases (a "liquid_sky" with much more amplitude and slower
  speed), plus a gradient colour mode.
- *Params*: amplitude 0.1 to 0.25, component speeds 0.03, 0.05 and 0.08
  cycles/beat.

**26. Grid / net ("grid", "net", "lattice", "matrix")**
- *Haze*: crossing sheets forming a net or cage of light over the crowd.
- *Motion*: the grid lines scroll, the net tilts, or it is revealed line by
  line on the beat.
- *Track*: techno (Awakenings "laser geometry"), and the build before a
  drop, where the net "tightens".
- *One projector*: horizontal and vertical line sets (`grid_scan`). Keep
  line count low (≤ 6 + 6) so it stays bright.
- *Params*: 4 x 4 lines, scroll 1 line per beat.

---

### D. Mirrors, geometry and stage-bound looks

**27. Mirror-bounce ("mirror effects", "bounce", "cage", "web")**
- *Haze*: beams hit mirrors on the truss, stage or around the venue and
  bounce into new directions, making triangles, webs and cages.
- *Motion*: mostly static geometry that pulses and changes colour. The
  movement comes from switching which mirror is hit, on the beat.
- *Track*: special moments and intros. Very common in clubs and techno
  halls, less at open-air mainstages.
- *One projector*: requires physical mirrors. Our software can support it
  with a **"mirror target" list**: named calibrated points the user sets.
  Cues then address targets by index (hit target 1, 3, 5 on the beat). This
  is a good future feature (beam-to-target mapping), but not a pure
  procedural look.
- *Params*: per-target dwell; chase through targets at 1 per beat.

**28. Stage-bound beams / drawing the DJ in ("stage hits", "focus")**
- *Haze*: beams aimed down at the stage or DJ booth, framing the performer.
  TPi describes nine BB3 lasers on motors focused down onto the stage at
  Creamfields, "drawing" the DJs into their positions.
- *Track*: DJ entrance and intros.
- *One projector*: needs a separate calibrated "stage zone" (below y_h
  but only onto the stage). This belongs in a zone and safety feature
  first. Flag it as an advanced, zone-gated look.

**29. Geometric array shapes ("V", "triangle", "diamond", "prism")**
- *Haze*: beams from several heads meeting to form a large 3D shape: a
  pyramid, a V, a diamond. The Axwell ^ Ingrosso inverted V at Creamfields
  was a 104-laser logo made purely of beams (TPi).
- *One projector*: small version: a beam "V" (two fan legs rising from the
  centre) or "Λ" (beams converging upward). Our x-y frame can represent the
  fingerprint of the shape as seen from the crowd.
- *Params*: 2 x 5 beams, legs at ±30° from vertical.

**30. Beam rain / stars ("sparkle", "stars", "random beams", "rain")**
- *Haze*: random single beams flickering on and off across the whole field,
  like stars or crackles. The positions change quickly.
- *Motion*: random new positions every 1/4 beat, each beam lives 1/8 to 1/4
  beat.
- *Track*: breakdown ambience (slow, sparse) or pre-drop tension (dense,
  fast).
- *One projector*: K random dots per frame from a seeded PRNG, reseeded on
  each step, so it stays beat-synced and deterministic.
- *Params*: K = 4 to 10, step = 1/4 beat.

**31. Lightning / crackle ("lightning", "zap")**
- *Haze*: a jagged beam or line that flashes for a split second, often
  paired with thunder sounds or a riser impact.
- *One projector*: a random zig-zag polyline, drawn for 2 to 4 frames then
  blanked.
- *Track*: impacts, "white-noise down-sweeps" after the drop.

**32. Beam brush / fat beam wash ("beam brush", "fat beam", "wash")**
- *Haze*: soft, thick beams instead of razor-thin ones. Kvant's BeamBrush
  changes divergence on demand, from a tight beam to a volumetric wash.
- *One projector*: our hardware cannot change divergence. Fake it by
  drawing a **small circle or line at each beam position** (a mini-tunnel
  per beam), so each beam looks thicker in haze.
- *Params*: per-beam circle r = 0.01 to 0.03, fewer beams (4 to 6).

---

### E. Graphics, text and abstracts (projected on screens, scrims, smoke)

**33. Logos and festival branding ("logo", "ident")**
- On screens, mesh, or the back of a haze wall. It is used a few times per
  night (DJ intros, the festival logo at the end-show). EDC ran a 60-minute
  timecoded show for 9 DJ intros (Laserworld news).
- *One projector*: our ILDA import plus text (`font.rs`).
- *Colour*: the artist's brand colour. Hold still for at least 2 bars,
  then break it apart (explode into beams) on the drop.

**34. Text and countdowns ("text", "count-in", "3-2-1")**
- A big countdown before the drop (4, 3, 2, 1, one per beat or per bar),
  the artist name, or a crowd message.
- *One projector*: `font.rs`. One number per beat, blank on the last
  half-beat before the drop.

**35. Abstract graphics ("abstracts", "lissajous", "spirograph")**
- Continuously morphing curves (lissajous, roses, spirographs) projected
  flat on a surface. At festivals they are mostly screen or scrim fillers
  during melodic tracks. The "3D" versions rotate in space.
- *One projector*: `lissajous`, `spirograph`, `rose`, `flower`.
- *Params*: slow morph (phase drift of 1 cycle per 8 to 16 beats).

**36. 3D wireframes ("rotating cube", "sphere", "wire globe")**
- A rotating wireframe object reads as 3D even though it is 2D. Pangolin's
  own effect list mentions 2D shows that give a 3D illusion through object
  rotation.
- *One projector*: simple 3D-to-2D projection of a cube or icosahedron,
  rotated at 1 turn per 2 bars.

**37. Spectrum / audio-reactive bars ("spectrum", "VU")**
- Less common at mainstages (it looks "home DJ" if overused), but works in
  short bursts on screens. Existing `spectrum`.

**38. Clock / countdown ring ("progress ring")**
- A ring that fills over a build (it closes as the drop approaches). Very
  readable as "something is about to happen". Our `clock` generator is a
  starting point. Drive it from the cue's beat position instead of wall
  time.

---

### F. Audience scanning (note only)

**39. Audience scanning ("crowd scan", "PASS scanning")** exists at big US
festivals (Coachella stages use PASS-equipped Kvant units, per Pangolin). It
needs certified hardware, a variance or permit, and per-venue measurement.
**Laser Studio must not ship audience-scanning cues.** All looks above are
designed to stay above `y_h`.

---

## 2. How festival shows evolve over time

### 2.1 Track structure (the grammar lasers follow)

EDM, techno, trance and hardstyle are in 4/4 time and built from **8-bar
phrases** (32 beats). Most sections are 16 or 32 bars long. A typical
mainstage track:

| Section | Length | Laser role |
|---|---|---|
| Intro | 16–32 bars | DJ-friendly, beat only. Minimal: one slow look, or none. Graphics and logos during DJ intros. |
| Groove/verse | 16–32 bars | Main groove look (fan sweep, chase), steady and on the beat. |
| Breakdown | 8–32 bars | No kick. **Pull back**: darkness, one hot beam, liquid sky lowering, slow tunnel, soft colour. |
| Build-up | 8–16 bars | Tension rises: faster rotations, chases that double speed, tilt up, strobes in the last bars. |
| Pre-drop gap | 1 beat–1 bar | **Blackout** (or a single frozen beam). This silence is the key moment. |
| Drop | 16–32 bars | Everything on beat 1: full width, full brightness, fastest motion, white or a strong colour. |
| Outro | 16–32 bars | Mirror of the intro, fading down for the next track. |

Common tempos: house and tech house 122–128, big-room and EDM 126–130,
techno 125–140 (hard techno 140–155), trance 136–140, hardstyle 150–160,
drum & bass 172–176 (lasers often run at half time, 86–88), dubstep 140–150
(with a half-time feel).

### 2.2 Build-up grammar (the drop "recipe")

The same pattern shows up in almost every EDM build and should be encoded as
a reusable envelope:

1. **Density doubles on each 4 or 8-bar block**, following the snare roll:
   quarter notes, then eighths, then sixteenths. The lasers copy it: chase
   step 1 beat, then 1/2, then 1/4. Strobe 1/2 beat, then 1/4.
2. **Something rises**: the fan tilts up, the tunnel shrinks to a point, the
   sheet lifts, or brightness ramps from 30% to 100%.
3. **Colour desaturates towards white** in the last 4 bars, or stays locked
   to one colour. It must not keep changing.
4. **Last beat (or last bar): cut to black.** Sometimes a single white beam
   is held instead.
5. **Drop, beat 1**: the widest look at full brightness, with a strobe or
   position hit, usually in the track's "hero" colour. Then it settles into
   the groove look after 1 to 2 bars.

### 2.3 How LJs vary a look while it plays

A good laser jockey rarely leaves a look untouched for more than 8 bars.
They change **one parameter at a time**, on phrase boundaries:

- **Speed**: normal, then double on the second half of the drop, then half
  in the break.
- **Size/width**: from narrow to full width to build energy. Narrow again to
  focus.
- **Position**: centre, left, right, high, low. Snapped on the beat, or
  mirrored across heads.
- **Rotation direction**: reverse the rotation on each 8-bar phrase, or on
  a big snare hit. Direction change is the cheapest way to make a look feel
  new.
- **Colour**: change on phrase boundaries (8 or 16 bars). Two-colour
  alternation on the beat inside a drop is fine. See section 3.
- **Beam count**: 4 beams in the groove, 12 in the drop.
- **Gating**: add or remove beat stabs or strobe over the same geometry.
- **Symmetry**: switch between mirror, phase-offset and unison across heads
  (or virtual heads).

### 2.4 Tempo relationships to encode

- **On the beat**: steps and hits on every beat (quarter notes).
- **Double time**: 1/2-beat steps. Used in builds and drop peaks.
- **Quadruple time**: 1/4-beat steps. Only for 1–4 bars before a drop, and
  mind the strobe safety rules.
- **Half time**: 2-beat steps. Used in breakdowns and in dubstep/DnB half-
  time feels.
- **Phrase time**: 1 cycle per 4, 8 or 16 bars (slow sweeps, ceiling moves,
  colour drifts).
- **Motion periods** should be **whole numbers of beats** (1, 2, 4, 8, 16,
  32), so a look "lands" on the downbeat. Free-running speeds (in Hz,
  unrelated to BPM) are what makes amateur shows look disconnected from the
  music.

---

## 3. Colour practice

### 3.1 Palettes that read as "pro"

| Palette | Where you see it | Notes |
|---|---|---|
| **Pure green** | Everywhere, especially techno | Brightest colour for the eye. Green beams read as "the laser colour". |
| **White (RGB full)** | Drop peaks, big-room hits | Strongest impact. Save it for peaks so it keeps meaning something. |
| **Cyan / ice blue** | Trance, progressive, sunrise | Clean and "cold". Works well with white. |
| **Deep red** | Hardstyle, hard techno, dark sets | Aggressive. Red is dim in haze, so it needs more power. |
| **Red + white** | Hardstyle/Q-dance (Defqon.1's red identity), hard techno | Classic two-colour aggression. |
| **Blue + magenta/violet** | Melodic techno, afterhours | Moody, the "Afterlife" look (screen-led palettes). |
| **Green + blue** | Groove sections | Two cool colours that stay calm. |
| **Amber / gold + white** | Sunset, "sunrise" moments, anthem finales | Warm and emotional. |
| **Magenta + cyan** | EDM pop drops | Bright and festival-poster-like. |
| **Brand colour** | Festival or artist identity | Match the LED wall palette of the moment. |

### 3.2 Colour patterns in time

- **One colour per phrase** (8 or 16 bars), switching on the downbeat of the
  new phrase.
- **Two-colour alternate**: odd and even beams, left and right groups, or
  alternating on each beat. Keep the geometry steady while the colours swap.
- **Colour chase across beams**: a colour "front" travelling through the fan
  (beam i gets colour B from step i). Good in builds.
- **Build to white**: saturation goes down over the last 4 bars of the build
  (colour to white), then the drop hits in white or in the hero colour.
- **Rainbow**: works as a **short deliberate moment** (a rainbow tunnel in a
  euphoric break, a rainbow fan for one phrase at the end of the night), or
  as a slow hue drift over a whole sheet. It looks cheap as a default mode,
  or with fast hue cycling on every look.

### 3.3 "Cheap" vs "pro"

| Looks cheap | Looks pro |
|---|---|
| Constant motion, everything always on | Contrast: darkness, stillness, then impact |
| Random colour change on every beat | Colour tied to sections and phrases |
| Rainbow everywhere | One or two colours, rainbow as a special moment |
| Motion speed not related to BPM | Motion periods in whole beats and bars |
| Too many beams (dim, flickering) | Fewer, brighter beams (tight and crisp) |
| Graphics and beams at the same time from one projector | One idea at a time |
| Asymmetric, messy multi-head chaos | Mirror or phase-offset symmetry |
| Drop starts late or before the beat | Changes on beat 1, blackout on the beat before |
| Same look for a whole track | One parameter change per 8 bars |
| Visible travel lines between beams | Clean blanking, stable dwell |

### 3.4 Brightness per colour

Pure red and blue look much dimmer in haze than green at the same power
(LVR Optical notes that blue needs 3–4x the power of green to look equally
bright). For balanced two-colour looks, give the dimmer colour more beams or
more dwell, or scale green down (for example green 0.6, red 1.0, blue 1.0
as a "perceived balance" preset, tuned by eye).

---

## 4. Parameter guidance to encode

All values are **starting points** to tune by eye in the preview. Beat-based
units are preferred. `B` = BPM.

### 4.1 Rotation (tunnels, spokes, star)

| Name | Turns per beat | deg/s at 128 BPM | Use |
|---|---|---|---|
| Glacial | 1/64 (1 turn per 16 bars) | 12°/s | Ambient, intros, liquid-sky drift |
| Slow | 1/32 (1 turn per 8 bars) | 24°/s | Breakdowns |
| Medium | 1/16 (1 turn per 4 bars) | 48°/s | Groove |
| Fast | 1/4 (1 turn per bar) | 192°/s | Drops |
| Very fast | 1/2 to 1 | 384–768°/s | Short peaks. Beams start to blur into a solid cone above ~1 turn/beat. |

Formula: `deg_per_s = turns_per_beat * 360 * B / 60`.
For symmetric N-spoke shapes, the look repeats every `360/N` degrees. A
"snap rotation" of `360/N` per beat makes a stepped rotation that reads as
perfectly on-beat.

### 4.2 Sweeps / pans

| Setting | Value |
|---|---|
| Width (half-amplitude) | Small ±0.15 (≈ ±5°), medium ±0.35 (≈ ±10°), wide ±0.6 (≈ ±18°) around the look centre |
| Period | 1 bar (drop), 2 bars (groove), 4–8 bars (breakdown) |
| Easing | Sine (default). Linear triangle for "scanner" looks. Hold-at-ends (trapezoid) for "hit" looks |
| Multi-head phase | Mirror (180°), or 1/N-cycle offset per head for a travelling wave |

### 4.3 Beam counts (one projector)

A galvo projector draws beams one after another. Each beam needs dwell
points plus blanked travel, so **more beams means dimmer, flickerier
beams**. Rough budget at 30 kpps and ≥ 30 frames per second: about 1,000
points per frame.

| Look | Recommended N | Max |
|---|---|---|
| Hot beam | 1 | 1 |
| Stabs / hits | 4–8 | 12 |
| Fan (groove) | 6–10 | 16 |
| Fan (drop, wide) | 10–16 | 24 |
| Finger tunnel | 8–12 | 16 |
| Sunburst | 12–18 | 24 |
| Curtain lines | 3–5 | 6 |
| Grid lines | 4+4 | 6+6 |

### 4.4 Strobe and gating

- **Safety rule (photosensitivity)**: keep sustained strobing **at or below
  about 4 flashes per second**, and limit faster strobing (10–20 Hz) to a
  few seconds, around 5 s maximum (Ticket Fairy's festival guidance; general
  photosensitive-epilepsy practice). At 128 BPM, 1/2-beat strobe = 4.3 Hz
  (fine), 1/4-beat = 8.5 Hz (burst only), 1/8-beat = 17 Hz (1 bar max).
  **Encode a hard limiter**: strobe above 4 Hz auto-releases after 5 s.
- **Duty cycle**: 30–50% for "strobe", 15–25% for "stab" (hard gate on the
  kick).
- **Stab envelope**: attack 0 (instant on the beat), decay τ = 0.1–0.15
  beat, or a gate of 0.2 beat.

### 4.5 Chase steps

| Phase of track | Step length |
|---|---|
| Breakdown | 2 beats |
| Groove | 1 beat |
| Build block 1 | 1 beat |
| Build block 2 | 1/2 beat |
| Build block 3 | 1/4 beat |
| Last bar before drop | 1/8 beat, or blackout |
| Drop | 1/2 or 1 beat, plus position hits every beat |

Chase shapes: forward, backward, bounce (ping-pong), centre-out, outside-in,
odd/even, random (seeded), fill-then-empty.

### 4.6 Phrase and change cadence

- Change **one** parameter every **8 bars** (32 beats) at the latest in a
  drop. Change the whole look every 16–32 bars.
- Reverse rotation direction every 4 or 8 bars.
- Swap colour on 8 or 16-bar boundaries.
- Changes land on beat 1 of the bar (quantize cue launches to the next beat
  or bar, as Pangolin's BPM-synced cue playback does).

### 4.7 Engine features these parameters imply

1. **Beat clock**: `bpm`, `beat_pos` (fractional beats since cue start or
   since a global downbeat), tap tempo, and a "re-sync downbeat" button.
   Auto-BPM from audio can feed it later.
2. **Quantized launch**: a cue starts on the next beat, bar or phrase.
3. **Envelopes in beats**: attack, decay and gate lengths as fractions of a
   beat.
4. **Virtual heads**: split a generator's beams into groups with their own
   phase, colour, mirror or offset.
5. **Beam colour gate per index**, for chases, tails and odd/even patterns.
6. **Strobe limiter** (section 4.4) and **zone horizon `y_h`** enforced
   after every generator.

---

## 5. Twelve pre-made "evolving cues"

Notation: `b` = beats since cue start (float), `bar = floor(b/4)`,
`ph = b mod 1` (beat phase), `lerp`, `ease_in(x) = x²`,
`smooth(x) = x²(3-2x)`. Positions are in our -1..1 frame. `y_h` is the
horizon. All cues loop after their length unless noted.

**E1. "Rising Fan" (16 beats, build-up helper)**
- Geometry: fan of N = 10 beams, width `w = lerp(0.15, 0.7, smooth(b/16))`,
  height `y0 = y_h + lerp(0.05, 0.6, ease_in(b/16))`.
- Chase: one-by-one fill. Step length 1 beat for b < 8, 1/2 beat for
  8 ≤ b < 12, 1/4 beat for b ≥ 12. When all beams are lit, the fill
  restarts.
- Colour: main colour, saturation `lerp(1, 0, smooth((b-8)/8))` (to white
  in the last 8 beats).
- End: brightness 0 for `b ∈ [15.5, 16)` (a half-beat blackout).

**E2. "Tunnel Zoom Drop" (32 beats: 16 build + 16 drop)**
- b < 16: tunnel radius `r = lerp(0.35, 0.04, smooth(b/16))`, rotation
  speed ramps from 1/32 to 1/4 turn/beat, strobe gate at 1/2 beat from
  b = 12, 1/4 beat from b = 14. Blackout for the last 1/2 beat.
- b ≥ 16 (drop): `r` jumps to 0.45 on beat 16, then pumps on each kick
  (`r = 0.35 + 0.1·exp(-ph/0.15)`), rotation 1/4 turn/beat, reversing
  direction every 4 beats. Colour switches to color2 at b = 16.

**E3. "Scissor Crossfire" (32 beats)**
- Two groups of 5 beams. Group L centre `-0.3 + 0.45·sin(2π·b/2)`, group R
  centre `+0.3 - 0.45·sin(2π·b/2)`. Each group is tilted ±15° so it reads as
  an X when they cross.
- Colours: L = main, R = color2. On beats where the groups cross
  (b mod 1 = 0), flash both to white for 1/8 beat.
- Every 8 beats the cycle doubles in speed for 4 beats (2 → 1 beat period),
  then goes back.

**E4. "Kick Stabs, Four Positions" (16 beats)**
- Four stored positions: P0 = wide fan high, P1 = narrow fan left-tilted,
  P2 = narrow fan right-tilted, P3 = "V" (two legs of 4 beams).
- Beat k shows `P[k mod 4]`, gated: on for 0.2 beat, off after.
- From b = 8: add the off-beat (the position changes every 1/2 beat, gate
  0.15 beat).
- Colour: alternate main/color2 every bar.

**E5. "Liquid Sky Descent" (32 beats, breakdown)**
- A horizontal sheet with a ripple. `y0 = y_h + lerp(0.6, 0.08,
  smooth(b/32))`, ripple amplitude `0.01 + 0.02·smooth(b/32)`, ripple speed
  0.25 cycles/beat.
- Colour: gradient from main to color2 along x, slow hue drift of ±15° over
  the cue.
- Brightness breathing: `0.7 + 0.3·sin(2π·b/8)`.
- On b = 31.5 to 32: the sheet snaps back up to `y_h + 0.6` (ready for
  the drop).

**E6. "Sunburst Sunrise" (32 beats, intro or anthem)**
- Semicircle sunburst of N = 16 spokes at r = 0.85, upper half only.
- b < 16: spokes appear one by one from the horizon (left and right
  together, towards the top), 1 pair per beat. Rotation 0.
- b ≥ 16: slow rotation of 1/64 turn/beat, and odd/even spokes alternate
  brightness 100%/40% on each beat.
- Colour: amber (255,140,0) to white along the spoke index, full white at
  b = 31.

**E7. "Chase Accelerator" (32 beats)**
- Fan N = 8, one lit beam with a 2-beam tail (40%, 15%).
- Step length: 1 beat (b 0–8), 1/2 (8–16), 1/4 (16–24), 1/8 (24–28), then a
  full-fan strobe at 1/4 beat (28–31.5) and blackout (31.5–32).
- Direction: bounce. Colour: main, with the tail in color2.

**E8. "Rotating Spokes Reverse" (32 beats, drop groove)**
- Finger tunnel N = 12, r = 0.3.
- Rotation 1/4 turn/beat. The direction reverses every 8 beats with a
  1/2-beat ease through zero.
- Radius pumps on every kick (+25%, decay 0.5 beat).
- Colour: odd/even beams in main/color2. The two colours swap on each
  8-beat reversal.

**E9. "Wave Rider" (16 beats)**
- `beam_wave` N = 14, amplitude `lerp(0.05, 0.35, b/8)` for b < 8, then
  held. Wavelength = fan width. Speed 0.5 cycles/beat.
- At b = 8 the wave direction flips, and the colour becomes a colour chase
  (a colour front travels 1 beam per 1/4 beat).
- At b = 12 to 16, the amplitude drops back to 0.05 (loop-ready).

**E10. "Curtain Corridor" (32 beats, intro or DJ entrance)**
- 4 vertical curtains at x = -0.6, -0.2, 0.2, 0.6, from `y_h` to
  `y_h + 0.6`.
- b < 16: curtains appear one per 4 beats from the outside in. They are
  static.
- b 16 to 32: the curtains slide towards the centre in pairs
  (`x_i ← x_i·(1 - 0.5·smooth((b-16)/16))`), and gates on every beat (on
  0.5 beat). At b = 31 the curtains merge into one centre line, then the cue
  stops.

**E11. "Starfield to Burst" (16 beats, pre-drop tension)**
- Random beams (seeded PRNG, reseeded every step) above `y_h`. Count
  `K = round(lerp(3, 12, b/12))`, step length `lerp(1, 1/4, b/12)` beats.
- Beats 12 to 15: all beams converge to a tight bundle at the centre
  (lerp positions to (0, y_h + 0.3)).
- Beat 15 to 15.5: a single white hot beam. 15.5 to 16: blackout. The cue
  hands over to a wide drop look.

**E12. "Grid Tighten" (32 beats, techno)**
- A 5 x 5 grid (5 horizontal + 5 vertical lines) in the upper field.
- b 0–16: lines are revealed one per beat (alternating horizontal and
  vertical). Colour main.
- b 16–28: the grid spacing shrinks (`s = lerp(1, 0.4, (b-16)/12)`), and
  the grid scrolls 1 line per beat.
- b 28–32: the grid strobes at 1/4 beat (≤ 1 bar, within the strobe limit
  with 1/2-beat pairs), then snaps to a single horizontal sheet at b = 32.

---

## 6. Six pre-made full timelines

Each timeline is a sequence of looks on a beat grid. **Bar numbers start at
1.** Quantize the start to the next bar. They assume 4/4 at the tempo of the
beat clock. "→" means an instant change on beat 1 of that bar.

**T1. "Build-up 16 bars → Drop" (EDM/big-room, 32 bars = 128 beats)**
1. Bars 1–4: E7 "Chase Accelerator" at step 1 beat (fan N = 8, main
   colour, tilt low).
2. Bars 5–8: the same fan, step 1/2 beat, fan tilts up over 4 bars
   (look 3).
3. Bars 9–12: E1 "Rising Fan" last half (width grows, desaturate to white),
   1/4-beat steps.
4. Bars 13–15: tunnel shrinking to a point (E2 build part), 1/4-beat
   strobe from bar 14 (≤ 8 beats of fast strobe).
5. Bar 16: beats 1–3 single white hot beam, beat 4 **blackout**.
6. → Bar 17 (drop): wide fan N = 16, white, full brightness, 1/2-beat
   strobe for 1 bar.
7. Bars 18–24: E3 "Scissor Crossfire" (hero colour + white).
8. Bars 25–32: E8 "Rotating Spokes Reverse", colour swap on bar 25.

**T2. "Techno Hour Block" (Awakenings-style, 64 bars, loops)**
1. Bars 1–16: E12 "Grid Tighten" (green), no strobe.
2. Bars 17–32: kick stabs (look 8) on a narrow fan N = 6, positions
   stepping every beat (E4 with only P1/P2, mirror), green.
3. Bars 33–40: breakdown: single hot beam (look 11), slow pan (16 beats),
   red.
4. Bars 41–48: slats (look 24) with travelling gaps at 1/2 beat, red + white.
5. Bars 49–64: a 12-spoke finger tunnel rotating 1/8 turn/beat, reversing
   every 8 bars, with stabs on each kick. Colour back to green on bar 49.

**T3. "Trance Breakdown → Euphoric Drop" (48 bars)**
1. Bars 1–16: E5 "Liquid Sky Descent" twice (cyan → blue gradient).
2. Bars 17–24: aurora sheet (look 25), violet-blue, very slow.
3. Bars 25–32: tunnel radius shrinking from 0.35 to 0.05, rotation
   speeding up from 1/32 to 1/4 turn/beat, colour to white. 1/2-beat
   strobe in bar 31, blackout on the last beat of bar 32.
4. → Bar 33: E2 drop half (pumping tunnel, wide), cyan + white.
5. Bars 41–48: rainbow finger tunnel, slow hue drift (this is the one place
   where rainbow is intended), rotation 1/4 turn/beat.

**T4. "Hardstyle Kick Attack" (150 BPM, 32 bars)**
1. Bars 1–8 (reverse-bass intro): E4 "Kick Stabs, Four Positions", red,
   hard gate 0.15 beat.
2. Bars 9–16: add the off-beat (positions every 1/2 beat), red/white
   alternating each bar.
3. Bars 17–24 (build): E7 chase steps 1/2 → 1/4, fan tilt up, then the last
   2 beats blackout.
4. → Bars 25–32 (drop): wide white fan N = 16, a stab on every kick,
   position change every beat, colour red-white alternating per beat.

**T5. "DJ Intro / Headliner Entrance" (32 bars, timecode-style)**
1. Bars 1–8: darkness with a starfield (look 30, sparse, K = 3, 2-beat
   steps), white.
2. Bars 9–16: E10 "Curtain Corridor" (curtains in the brand colour).
3. Bars 17–20: the artist name (text) drawn and held, brand colour.
4. Bars 21–23: countdown 4-3-2-1, one number per bar (or per beat in bar
   23), then blackout on the last beat of bar 24.
5. → Bar 25: E6 "Sunburst Sunrise" second half (spokes rotating, amber to
   white), full brightness.

**T6. "Sunset Anthem / Closing" (64 bars)**
1. Bars 1–16: E6 "Sunburst Sunrise" (amber → white).
2. Bars 17–32: E5 "Liquid Sky Descent" in gold/amber, slow breathing.
3. Bars 33–40: E9 "Wave Rider" in amber + magenta.
4. Bars 41–48: build: E1 "Rising Fan" (to white).
5. → Bars 49–60: huge white fan N = 16, slow sweep (2-bar period), mirrored
   virtual heads, then the colour goes to a slow rainbow over bars 57–60.
6. Bars 61–64: all beams collapse into one hot white beam straight up, which
   fades out over the last 4 bars.

---

## 7. Implementation notes for Laser Studio

- Add a **`BeatClock`** in `engine.rs` (bpm, beat_pos, tap, resync) and pass
  `beat_pos` to generators next to `t`. Keep wall-time `t` for free-running
  looks, so current presets do not change.
- Extend `GenParams` (with `#[serde(default)]`) with beat-based fields:
  `period_beats`, `steps_per_beat`, `gate_beats`, `direction`, and
  `groups` for virtual heads.
- An **evolving cue** can be a small list of keyframes over `b`: each has a
  beat time, a generator, `GenParams` overrides, a colour and an easing.
  The engine interpolates numeric fields and switches discrete ones on the
  key's beat. Timelines are the same structure at a larger scale (lists of
  cues with bar start times).
- Enforce the **horizon `y_h`** and the **strobe limiter** after every
  generator, so no preset can break them.
- New generators suggested by this catalogue: `chase_fan` (per-beam gate),
  `scissor`, `sunburst`, `finger_tunnel` (true circle), `curtain`,
  `waterfall`, `slats`, `aurora`, `starfield`, `positions` (step through
  stored beam layouts), `wire3d`.

---

## Sources

- Pangolin, "Types of Laser Shows & Effects" (hot beams, tunnels, fans,
  sheets, liquid sky, beam brush, 3D illusion):
  https://pangolin.com/blogs/news/types-of-laser-shows
- Pangolin, "Synchronize Laser Light Shows To Music" (tap BPM, auto-BPM,
  timecode from DJ software, timeline markers):
  https://pangolin.com/blogs/news/synchronize-laser-light-shows-to-music
- Pangolin, "Lasers for Festivals, Concerts and Tours":
  https://pangolin.com/pages/lasers-for-festivals-concerts-and-tours
- Pangolin, "Coachella 2025: Quasar, Sahara, and Yuma Stages" (Polar
  Productions, Kvant Atom 42, BeamBrush, PASS):
  https://pangolin.com/blogs/news/coachella-2025-quasar-sahara-and-yuma-stages
- PLSN, "Swedish House Mafia: Designing A Circular Paradise" (Derek Abbott,
  ER Productions, 12 AT-30, "distinct special effect for certain moments"):
  https://plsn.com/archives/november-2022/swedish-house-mafia-2/
- TPi, "Kinekt Debuts at Creamfields Steel Yard" (138 lasers, 104-laser
  inverted V, BB3s drawing the DJs in):
  https://www.tpimagazine.com/kinekt-debuts-at-creamfields-steel-yard/
- TPi, "Calvin Harris at Creamfields 2016" (downstage laser line,
  Laserblades on the riser, grandMA2 + Beyond):
  https://www.tpimagazine.com/calvin-harris-at-creamfields-2016/
- Laserworld news, "EDC Las Vegas / 30 laser systems" (60-min timecode show
  for 9 DJ intros, 60+ hazers):
  https://www.laserworld.com/en/newslist/106-laserworld-news-en/1872-edc-las-vegas-30-laser-systems.html
- Laserworld, "In Action: Lasers at Festivals" (UNTOLD, Flight Festival with
  Tarm 11, Ultra Europe):
  https://www.laserworld.com/en/in-action/festivals.html
- VICE, "The Founder of Awakenings on How Lighting, Lasers, and the Grateful
  Dead Shaped One of the World's Biggest Techno Parties":
  https://www.vice.com/en/article/the-founder-of-awakenings-on-how-lighting-lasers-and-the-grateful-dead-shaped-one-of-the-worlds-biggest-techno-parties/
- Kvant, "Lasers at Choral music festival" (fan effect over the audience,
  uniform colours for a calm look):
  https://www.kvantlasers.co.uk/blogs/case-studies/lasers-at-choral-music-festival
- LVR Optical, "Laser Effects: Movement and Colour" (static beam, finger
  fan, spikey tunnel, flat scan, solid tunnel, slats; colour vs power):
  https://www.lvroptical.com/blog-laser-effects.html
- Starshine, "How Tomorrowland & Glastonbury Create Epic Festival Laser
  Shows" (dealer blog, low authority: "aurora-style tunnels", beat-matched
  effects): https://www.starshinelights.com/blogs/news/tomorrowland-laser-shows
- EDM House Network, "The Evolution of Tomorrowland's Mainstage From 2005 to
  2026" (56 lasers on the Consciencia mainstage):
  https://edmhousenetwork.com/the-evolution-of-tomorrowlands-mainstage-from-2005-to-2026/
- Ticket Fairy, "Visual Language for Bass Music Festivals" (strobe ≤ 4 Hz
  sustained, short bursts only, ~5 s limit):
  https://www.ticketfairy.com/blog/visual-language-for-bass-music-festivals-lasers-low-light-and-safety
- Wikipedia, "Laserface" (Gareth Emery, Anthony Garcia; per-track laser
  cues with live adjustments): https://en.wikipedia.org/wiki/Laserface
- Photonlexicon, "Beam angles, projector placement, modulation control"
  (elevated mounting, masking below a plane, beam and graphics zones):
  https://photonlexicon.com/forums/showthread.php/2663-Beam-angles-projector-placement-modulation-control-etc
- Photonlexicon, "Laser production for Eric Prydz show at Madison Square
  Garden" (Lightwave International):
  https://photonlexicon.com/forums/showthread.php/23127-Laser-production-for-Eric-Prydz-show-at-Madison-Square-garden
- ControlBooth, "Liquid sky effect?":
  https://www.controlbooth.com/threads/liquid-sky-effect.43787/
- ILDA, "Haze and fog for laser shows": https://www.ilda.com/hazefoglasers.htm
