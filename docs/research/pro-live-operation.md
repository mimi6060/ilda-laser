# Research: how professional laserists run shows, and what a pro tool needs

Scope: live operation (busking) and programmed (timecoded) shows in Pangolin
QuickShow / BEYOND and Laserworld Showcontroller LIVE / RealTime, plus newer
tools (PangoBeats, CloudLase, Lazura, Liberation) for the tempo and
auto-programming parts. It goes deeper than the existing reports
(`pangolin.md`, `showcontroller.md`, `madmapper-and-open-content.md`),
which already cover the feature inventories, content licensing, zones and
safety. The last section is an implementation-ready spec for Laser Studio,
turned into task files `tasks/T-140` … `tasks/T-171`.

Public information only: the Pangolin wiki (raw DokuWiki pages), the public
Showcontroller LIVE PDF manual and the online Showcontroller manual, vendor
product pages and forum threads. Nothing was decompiled. No content, names of
effects or UI assets are copied. We take concepts and reimplement them.
Accessed 2026-09-27.

---

## 0. TL;DR

1. **Two ways of working, and one tool for both.** *Busking* (live LJ): a cue
   grid mapped to a controller (APC40 5×8 layout is the de-facto standard in
   both Showcontroller LIVE and Liberation), a tapped tempo, and a small set
   of **master modifiers** the operator rides the whole night. *Programmed*:
   a timeline locked to the audio file or to timecode (MTC/LTC/Art-Net TC),
   built bar by bar with snapping to beats and markers. Pros often mix the two:
   the drops are programmed to timecode and the rest is busked live on an APC40
   ([Starshine guide](https://www.starshinelights.com/blogs/news/laser-show-light-quickshow-beyond),
   [Pangolin blog](https://pangolin.com/blogs/news/synchronize-laser-light-shows-to-music)).
2. **Live modifiers are a separate layer from the cue.** BEYOND has "Live
   Control" objects at four levels (Cue → Master → Projection Zone, plus
   ProTracks), calculated in that fixed order. Each one holds position,
   rotation, size, zoom, colour, visible points, brightness, scan rate and
   animation speed. Its neutral "Reset" state leaves the frame untouched
   ([BEYOND live control](https://wiki.pangolin.com/doku.php?id=beyond:livecontrol)).
   QuickShow has the same idea with two levels (Master / Cue). Right-click
   resets a control (size, brightness, speed → 100 %, position and rotation →
   0, colour → "Normal"), and `~` inverts rotation direction "to the beat"
   ([QuickShow position & rotation](https://wiki.pangolin.com/doku.php?id=quickshow:position_and_rotation_controls)).
3. **Effects go "on top of cues"** in a separate grid (QuickFX / BEYOND FX
   grid): 4–6 lines, one effect per line (or up to four), duration in seconds
   or beats with ¼, ½, 1, 2 and 4 buttons, an **Action** slider (0 = off,
   right = fully applied), and one-shot **Drop effects**
   ([BEYOND live control § Quick FX](https://wiki.pangolin.com/doku.php?id=beyond:livecontrol)).
4. **The tempo engine gives every effect two clocks, seconds and beats.**
   Tap tempo is still the method Pangolin staff trust most ("tap the space bar
   10 times"). Backspace re-syncs the phase
   ([VLJ page](https://wiki.pangolin.com/doku.php?id=quickshow:virtual_laser_jockey),
   [music & beats](https://wiki.pangolin.com/doku.php?id=quickshow:music_and_beats_overview)).
   Automatic BPM arrived only in 2026 with PangoBeats 1.1. It locks with a
   confidence value, "coasts" when unsure, accepts "guide" taps as extra
   evidence and has a "New track" reset
   ([PangoBeats](https://wiki.pangolin.com/doku.php?id=beyond:pangobeats)).
5. **The timeline's primary unit is seconds. Beats are derived from the
   tempo map.** BEYOND explains why: if beats were primary, changing the BPM
   would move every event. Seconds stay primary, beats are the "slave"
   parameter, and effects inside an event choose which clock they run on
   ([timeline & BPM](https://wiki.pangolin.com/doku.php?id=quickshow:timeline_bpm)).
6. **A cue can be a mini-timeline.** Showcontroller LIVE scenes are short
   timelines of Trickfilm and effect events, with layers ("Surface", "layer
   pyramiding"), a *Flash only* flag and per-scene saved fader states
   ("Use Scenesettings") ([LIVE manual PDF](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html)).
   Newer tools (Lazura, CloudLase) push this further with keyframed cue
   timelines and BPM-synced LFOs on a mod matrix.
7. **Auto-programming now exists.** CloudLase "reads the arrangement, cuts
   the song into sections, and lays beat-locked looks for every laser onto the
   timeline" ([CloudLase](https://cloudlase.studio/)). Lazura detects beats and
   song sections and snaps cues to them ([Lazura](https://lazura.app/)). This
   is a real differentiator we can build with public MIR techniques.
8. **For Laser Studio**: add (A) a master/cue live-modifier stage, (B) a
   cue-program model (inner layers + modifier lanes + LFOs in beats, on top
   of the T-111 keyframe engine), (C) a timeline with templates (seconds as
   the primary unit for audio-locked shows, beats for phrases that follow the
   live tempo, like BEYOND's "Follow System BPM"), (D) a tempo engine that gives one
   global beat clock, and (E) four layers with click modes, groups and
   transitions. All of these need (F) a stable **control-id** registry so
   MIDI/OSC/timeline/UI address the same parameters. See § 6.

---

## 1. Live modifiers: what laserists touch constantly

### 1.1 What operators actually do during a set (synthesis)

From the tool designs above and practitioner guides
([Starshine sync guide](https://www.starshinelights.com/blogs/news/laser-music-sync-quickshow-beyond),
[Pangolin blog](https://pangolin.com/blogs/news/synchronize-laser-light-shows-to-music)),
a busked set is a loop of:

1. **Tempo**: tap 4–8 beats, re-align on bar one, and re-tap when the DJ
   changes track. When detection wobbles, "drop to lower divisions (1/1 →
   1/2) while you reset tempo/phase, then scale up again."
2. **Pick a base look** from a page (pages are grouped by energy or style)
   and often **layer** a beam cue over a graphic or tunnel.
3. **Ride the master modifiers**: size and position to fit the venue or
   surface, colour override to match the lighting designer's palette,
   rotation speed with direction flips on the beat, animation speed.
4. **Accents**: strobe or flash on drops, a momentary blackout
   (Showcontroller *Flash2Black*) before the drop, *Freeze* on a hit, one-shot
   "drop" effects, colour chase at 1/4 or 1/8.
5. **Energy plan**: "charge up pre-drop with simpler vectors/cooler palette,
   release on downbeats with layered merges" (Starshine).

So the modifiers must be **one touch away, bipolar where it makes sense,
reset-able in one gesture, and beat-aware**.

### 1.2 How QuickShow exposes them

- The **Live Controls tab** (right side) is "the main tab that is used during
  Live performances", in sections Master/Cue, Size, Position & Rotation,
  Color, Playback ([live controls](https://wiki.pangolin.com/doku.php?id=quickshow:live_controls)).
- **Master vs Cue** buttons: Master affects "the geometric properties of all
  cues simultaneously", Cue only the selected cue. Only visible in Advanced
  user level ([master & cue](https://wiki.pangolin.com/doku.php?id=quickshow:master_and_cue_controls)).
- **Size**: slider plus auto zoom-out / zoom-up buttons; right-click → 100 %
  ([size](https://wiki.pangolin.com/doku.php?id=quickshow:size_controls)).
- **Position / rotation**: position X/Y, rotation angle **and** rotation
  speed. Right-click → 0. `~` inverts rotation direction; the manual's
  example is setting Z speed 45 and tapping `~` on the beat
  ([position & rotation](https://wiki.pangolin.com/doku.php?id=quickshow:position_and_rotation_controls)).
- **Colour**: Brightness, Color, Visible Points. Right-click → 100 % /
  "Normal" ([color](https://wiki.pangolin.com/doku.php?id=quickshow:color_controls)).
  The product page lists "color, color cycling, scan speed and animation
  rate" among the live controls
  ([features](https://quickshowlaser.com/powerful/features/features.html)).
- **Playback**: Scan rate and Animation speed, right-click → 100 %
  ([playback](https://wiki.pangolin.com/doku.php?id=quickshow:playback_controls)).
- **In-cue handles**: while a cue plays, small icons at the bottom of the
  cell can be dragged up/down to change size, position and rotation of that
  cue ([controlling cues](https://wiki.pangolin.com/doku.php?id=quickshow:controlling_cues_during_playback)).
- **QuickFX**: an effect grid applied "on top of all playing cues (if the
  Master button is pressed) or … to only the selected cue". Four layers, so
  four effects can be active at once
  ([QuickFX](https://wiki.pangolin.com/doku.php?id=quickshow:quickfx)).
  Effect families on the product page: colours (preset and changing),
  pulse/flash/strobe, trace, chop, ripple, mirror, double, scroll, rock,
  throb, zoom, bounce.

### 1.3 How BEYOND exposes them

- **Live Control (LC) object** at four levels; order Cue → Master → Zone,
  configurable so Master can run after Zone. Destinations: Master, Selected
  Cues, Selected Zones, Selected ProTracks, **several at once**. RGB and Zoom
  panels are hidden by default. The Scan Rate and Animation Speed slider
  ranges are configurable
  ([live control](https://wiki.pangolin.com/doku.php?id=beyond:livecontrol)).
- **Physics**: every LC parameter can be filtered by a mass-spring filter
  (Mass, Attraction, Friction, plus Reflection), so fader moves glide instead
  of jumping. The same filter exists on Channels
  ([channels](https://wiki.pangolin.com/doku.php?id=beyond:channels)).
- **Time Control tab**: reverse playing cues, Set/Jump (cue point), Set A/Set
  B loop, and a "DJ disk" (distance from centre = speed drop, radial motion =
  time shift).
- **FX speed**: each FX line has its own time accumulator with Clock and Beat
  sliders and ¼ ½ 1 2 4 multipliers. The **Globe/Resync** logic: by default
  everything shares one accumulator (in sync). Touching a speed slider puts
  that line in independent mode, and resetting the slider brings it back in
  sync. Pangolin notes that independent accumulators drift ("rounding error …
  can sneak up on you") and lose phase. **Lesson for us: derive every phase
  from one global beat clock instead of integrating per-effect speeds.**
- **FX grid**: 4–6 lines × 25–100 effects; *One per line* / *Four per line*;
  **Drop effects** (one-shot, "a stone into the pool"); Duration in s or
  beats; **Action slider** morphs from the unaffected frame to the fully
  affected one; **Set Manual Mode** turns a cell into a touch pad for Size,
  Position, Z rotation or Brightness; **"Append Cue FX to Cue Effect"** bakes
  the live FX into the cue permanently. That is the busk-then-save workflow.
- **Effect engine**
  ([effects](https://wiki.pangolin.com/doku.php?id=beyond:effects)):
  oscillating effects (start/finish values, duration) and key effects (keyed
  states with interpolation). Curves: Accelerate, Decelerate, Ping-Pong,
  Discrete steps, Random, Custom. Colour effects apply a gradient "By points,
  By X axis, By Y axis, By Radius, By Time, By Angle". Palette effects replace
  colours. Inputs: MIDI, DMX, audio analyser, Channels.
- **Color channels** (5.2+): named RGB slots that effects reference instead
  of fixed colours, so one live change recolours many cues "without changing
  the intent of the original design"
  ([channels § Color channels](https://wiki.pangolin.com/doku.php?id=beyond:channels)).
  This is the cleanest pro design for "colour override with palettes".

### 1.4 How Showcontroller LIVE exposes them

From the public [LIVE manual PDF](https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html):

- **Master faders 1–7** with defaults; the `M` key resets them all.
- Its Art-Net remote chart shows the minimum pro set of live modifiers:
  scene, bank, **Strobe, Color, Size XY, Size X, Size Y, Shift X, Shift Y,
  Speed, Master Intensity** (channels 1–11).
- **Colour**: a *Colorspectrum* fader (0 = original colours, > 0 = whole scene
  in that hue). **Recolour palettes** (up to 60 colours) in two modes: nearest
  colour, or step through the palette as the frame's colour changes.
  Recolorindex rotates the palette
  ([live colour](https://www.showcontroller.com/en/manual/showcontroller-live/0-11-live-change-color.html)).
- **Buttons**: Multi Sel, Beat Mode, Loop/Flash, **Flash2Black** (blackout
  while held), **Freeze** (freeze while held)
  ([modes](https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-f-modes-effects.html)).
- **Timing**: BPM mode (Tap BPM, Nudge ±, also from the APC40) **or** a speed
  fader, never both. The speed fader "does not work if the software is in BPM
  mode". Hover previews run at the current BPM.
- **"Use Scenesettings"**: fader, chaser and group settings are saved with the
  active scene and recalled on selection. It is a global switch that decides
  whether live modifiers are per-scene or global.

### 1.5 Typical value ranges

Vendors don't publish exact slider ranges. These ranges come from the
documented reset values (100 % / 0 / Normal) and from what works on our
engine (normalised −1..1 coordinates, 30 kpps):

| Modifier | Typical range | Neutral | Notes |
|---|---|---|---|
| Size (XY, X, Y) | 0–200 % | 100 % | Negative X/Y = flip (mirror). Auto zoom in/out buttons |
| Position X/Y | −100..+100 % of the field | 0 | Clamp after calibration, never before safety |
| Rotation angle X/Y/Z | −180..+180° | 0 | X/Y rotate in 3D then project (perspective 0..1) |
| Rotation speed X/Y/Z | −720..+720 °/s, or turns per bar | 0 | Presets slow/medium/fast, direction invert |
| Animation speed | 0–400 % | 100 % | 0 = freeze. BEYOND adds ¼…4× buttons |
| Brightness | 0–100 % | 100 % | Multiplies the look's own brightness |
| Visible points (trace) | 0–100 % | 100 % | Draws only the first N % of the path |
| Dotting / "chop" | 0–100 % gap | 0 | Blanks every k-th lit point → dotted lines/beams |
| Strobe | 1–25 Hz or 1/1…1/16 beat, duty 10–90 % | off | Flash-while-held + latched |
| Colour | Normal / fixed / palette / rainbow / chase | Normal | Chase step in beats. Spread by stroke, point, X, Y, angle |
| Scan rate | 50–150 % of the projector default | 100 % | Clamped by per-projector min/max (hardware limit) |
| Mirror / prism | none, X, Y, XY, prism N = 2..8 | none | Multiplies the point count, so it needs a point budget |

**Beam Brush** is Pangolin/KVANT hardware: a scanner-driven divergence
mechanism with its own timing shift (≈ 4 for 3 mm systems, 6 for 5 mm) and a
slew-rate limiter
([projector settings](https://wiki.pangolin.com/doku.php?id=beyond:projector_settings-new),
[Beam Brush](https://wiki.pangolin.com/doku.php?id=beyond%3Abeambrush)). It
doesn't apply to ShowNET/ILDA projectors. We leave it out and only keep a
future "extra channel" hook in the point format.

---

## 2. Cue model

### 2.1 What a cue is

| Tool | Cue = | Saved with the cue | Global |
|---|---|---|---|
| QuickShow | A frame, an animation, or a QuickTool output (text, shape, targets, a small timeline) | Content, zones, cue live-control values, effects | Master LC, FX grid, BPM |
| BEYOND | Any "Image": frame, animation, shape, abstract, beam/DMX sequence, or a whole **timeline show** | General (name, colour, preview time, zones, *prevent rerouting*), Options (**start mode** override, **transition** override, fixed start/finish time, ignore by VLJ), Playback, **Shift** (time shift across zones), image properties (sample rate), pre/post PangoScript ([cue properties](https://wiki.pangolin.com/doku.php?id=beyond:cue_properties)) | Master/Zone LC, Dynamics limiters, master transition |
| Showcontroller LIVE | A **scene** = a short timeline of Trickfilm events (frame or animation range), effect events, per scanner, with **Surface** (layer) per event, World (zone) and scan parameters, Soft Blank / Soft Color, *Flash only* | Everything in the scene timeline; optionally the master faders ("Use Scenesettings") | BPM/speed, chaser (when "free") |

Conclusion: a pro cue is **(content) + (time behaviour) + (cue-level
modifiers and effects) + (routing) + (trigger behaviour)**. Our current
`Settings` covers only content and a few modifiers.

### 2.2 Trigger / click modes

BEYOND grid toolbar ([grid toolbar](https://wiki.pangolin.com/doku.php?id=beyond:grid_toolbar)):

- **Select**: click selects without playing (for editing).
- **Toggle** (default): click starts, click again stops.
- **Restart**: every click restarts the cue from its start.
- **Flash**: active only while held.
- **Flash-Solo**: while held, temporarily mutes every other cue.
- **One Cue / Multi Cue**: exclusive vs additive.
- **Back** (previous cue) and **Swap** (toggle between current and previous).
- The two grids can use different click modes (e.g. a main grid in Toggle
  and a secondary "hits" grid in Flash)
  ([grids and pages](https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages)).

Showcontroller LIVE: Multi Sel, Loop vs Flash (run once for its duration vs
loop), per-scene *Flash only*, **Groups of scenes** (saved multi-selections),
**Beat Mode** (auto frame change every N beats, optionally random bank
switch).

### 2.3 Exclusivity and limiters

BEYOND's **Dynamics tab** ([dynamics](https://wiki.pangolin.com/doku.php?id=beyond:dynamics_tab))
limits how many cues can play: HOLD cue limit, FLASH cue limit, per-zone
limit, **per-grid limit** ("maximum number of cues playing together"), beam /
DMX / show cue limits. When a limit is exceeded, the oldest non-held cue
stops first. Our equivalents are **groups** (one active cue per group) and a
**max active cues** limit.

### 2.4 Start, finish and transitions

- **Soft start / soft finish** (Dynamics): the cue player has three phases,
  Starting → Playing → Finishing, each with a duration and an optional effect.
  "Soft pause" accelerates or decelerates playback at start and stop. A
  finishing cue keeps drawing while the next one starts. Flash mode can skip
  the start/finish effects, because users "want a fast reaction in Flash
  mode".
- **Transition** between the old output and the new cue: **morph** by
  default, other types from the Transition button's context menu, with a
  **duration in seconds or beats**. The cue can override the master
  transition.
- Showcontroller: frame-change animation **Off / Morph / Fade (out) / Fade
  In** with a global duration
  ([frame change](https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-g-farme-change-animations.html)).
  Timeline morphing needs the same point count in both frames (BEYOND event
  tab), so **we resample both frames to a common count along the path**.

### 2.5 Layering

- QuickShow/BEYOND's "trackless tracks": a player is created per clicked cue
  and outputs are merged. BEYOND adds **ProTracks** (from LivePRO): four
  permanent cue players, each with its own LC, time control and FX. You pick
  the track, then the cue
  ([live control](https://wiki.pangolin.com/doku.php?id=beyond:livecontrol)).
- Showcontroller: layers ("Surface") inside a scene, and multiple scenes with
  Multi Sel.
- **Laser-specific constraint**: layers don't blend with alpha. The point
  lists are concatenated and the scanner draws them one after the other, so
  every extra layer lowers the frame rate (flicker below ~35 fps, see the
  roadmap). A layer system needs a **point budget**.

### 2.6 BPM sync of cue playback

- Every BEYOND player supplies two clocks (seconds and beats). Each
  effect/animation picks Clock or Beat
  ([timeline & BPM](https://wiki.pangolin.com/doku.php?id=quickshow:timeline_bpm)).
- Showcontroller: BPM mode vs speed-fader mode, with Beat Mode stepping frames
  every N beats.
- **Virtual Laser Jockey**: auto-triggers cues on a page, in order or at
  random, every N beats, from the tapped BPM or from audio. You can keep
  triggering by hand while it runs ("like having two laserists"). Cues can be
  excluded with "Ignore by VLJ"
  ([VLJ](https://wiki.pangolin.com/doku.php?id=beyond:virtual_laser_jockey)).
- **Quantised launch** (a new cue starts on the next beat or bar) is not
  explicitly documented by Pangolin but is standard in Ableton-style clip
  launching. We should offer it, default "next beat".

---

## 3. Timeline

### 3.1 BEYOND timeline ([guide](https://wiki.pangolin.com/doku.php?id=beyond:timeline), [event tab](https://wiki.pangolin.com/doku.php?id=beyond:timeline_event_tab))

- **Tracks**: Image tracks (frames, shapes, text, DMX, beam sequences),
  **A/V tracks** (audio, video, pictures), and **Bus tracks** (effects that
  apply to a group of tracks, e.g. fade everything out). Track settings have
  priority over event settings.
- **Events**: start/duration (numeric entry possible), a content time shift,
  a lock, and a reference to a workspace cue, a cue-list cue or a **local
  cue**. Other properties: **time source** (time, beats, refresh, **timeline
  fit** = stretch to event length, input-driven); acceleration curve; loop
  counter; **end action** (stop, hold last frame, continue); morph; a
  **transition to the next event**; masking; colour balance; zone routing;
  On-Enter / On-Leave scripts.
- **Effects in events** sit on lines executed top to bottom. Oscillating
  effects follow Clock/Beat or Timeline/Stretch. Key effects have keyframes
  with an adjustable acceleration.
- **Envelopes**: on A/V events (volume, pan, alpha). Double-click adds a
  node, right-click deletes it. (Laser parameter envelopes are done with key
  effects. We should make **envelopes on any parameter** first-class, as DAWs
  do.)
- **Snapping**: Strong magnet (to time/beat divisions), Medium (within a few
  pixels), Off.
- **Markers**: up to 500, coloured, snap targets. **Moving a marker moves the
  events snapped to it.** A keyboard mode adds markers during playback.
  Showcontroller also sets markers with Space while playing.
- **Loop region** ("User time"): Ctrl+B / Ctrl+E set the start and end, and
  playback loops. This is how operators refine a phrase: "refine
  colour/scale/rotation/stroke in looped regions" (Starshine).
- **Editing**: copy/cut/paste events, copy effects or content separately,
  Ctrl-drag duplicates. Mouse-wheel zoom on the cursor, "view all", an
  overview gauge.
- **Limits by edition**: QuickShow has 1 media track; BEYOND Essentials 40
  tracks / 2 media; Advanced/Ultimate 200 / 4 (`pangolin.md` § 1.3).

### 3.2 Timecode

([BEYOND timecode](https://wiki.pangolin.com/doku.php?id=beyond:timecode))

- Sources: MIDI MTC (in/out), MMC (out), SMPTE/LTC through an external reader,
  Art-Net timecode (in/out). One source at a time.
- **"Keep running even though timecode stops"**: otherwise the show stops
  after **1 s** of timeout.
- **"Time smooth filter"**: follow the detected timecode speed and move
  forward smoothly instead of jumping on each frame. Recommended "in all cases
  except when you need the show to stay in some exact time".
- **TC-IN arm button**, "added because of safety reasons": a timecoded show
  would otherwise restart itself after a blackout as soon as timecode arrives.
  **This matches our arm/disarm rule.** Timecode must never re-arm the laser.
- Timecode outside the show's bounds is ignored. A timecoded show in the grid
  flashes "TC" while it waits for timecode.
- Showcontroller: MTC in/out, can be the timecode master while playing audio.
  LTC needs a converter
  ([Showcontroller timecode](https://www.showcontroller.com/en/manual/7-special-features/7-2-timecode.html)).
  Liberation reads **LTC through any standard audio interface**
  ([Liberation](https://liberationlaser.com/)). LTC decoding in software is
  well within reach (bi-phase mark code on an audio input).

### 3.3 Programming fast

- **Rough-in → refine → energy plan** (Starshine): rough-in cues with snapping
  on, refine in a looped region, then plan energy across sections.
- **Markers on structure** (bars, hooks, vocal stabs) on the waveform, then
  events snap to markers. Move a marker and its events follow.
- **Copy/paste of phrases**: Ctrl-drag and clipboard. Because events snap in
  beats, a 16-bar phrase can be pasted at the next phrase boundary.
- **Record live into the timeline**: not documented for BEYOND. Worth doing,
  because it turns a good busked take into an editable show.
- **Auto-generated timelines**:
  - CloudLase: sections detected, "beat-locked looks for every laser",
    per-section re-roll, "three BPM-synced LFOs on a patchable mod matrix"
    ([CloudLase](https://cloudlase.studio/)).
  - Lazura: "detect beats and song sections automatically, snap cues to
    musical events", keyframed cue timelines, "live modifiers"
    ([Lazura](https://lazura.app/)).
  - Public techniques to reimplement: beat tracking by dynamic programming
    ([Ellis 2007](https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf)),
    onset/novelty-based segmentation
    ([Foote 2000](https://ieeexplore.ieee.org/document/869637)), and the
    [librosa beat tracker](https://librosa.org/doc/main/generated/librosa.beat.beat_track.html)
    (ISC licence) as a readable reference. We write our own Rust code; we
    don't copy code.

---

## 4. Tempo and audio

| Feature | QuickShow / BEYOND | Showcontroller LIVE | Notes for us |
|---|---|---|---|
| Tap tempo | Space / click BPM, average of taps; Backspace = resync | Tap BPM + Nudge ± (APC40 "Tap") | Space is reserved for arm. Use **Enter** for tap and **Backspace** for resync (layout-independent `e.code`) |
| BPM detection | Audio-in for VLJ ("experiment"); **PangoBeats 1.1** (2026): locks with confidence, coasts, guide taps, "New track", detects half/double time, counts bars, sends BPM **and resync** | None documented | Our browser already detects bass beats. Add tempo estimation with confidence |
| Beat/bar phase | Beat counter, resync | Beat Mode every N beats | One global clock: `beat = (t − t0) · bpm / 60`; bar = beat / 4 |
| Ableton Link | BEYOND 5.5+ "LINK" button: "Beat, tempo, and phase across multiple applications" ([grid toolbar](https://wiki.pangolin.com/doku.php?id=beyond:grid_toolbar)) | – | Link SDK is **GPLv2+ or proprietary licence**; the Rust wrappers inherit that ([rusty_link](https://github.com/anzbert/rusty_link), [ableton-link-rs](https://github.com/anweiss/ableton-link-rs), GPL-3.0). Needs a licence decision (task T-154) |
| MIDI clock (task T-207) | Beat manager can follow MIDI Clock ([BLT guide](https://blt-guide.deepsymmetry.org/beat-link-trigger/8.0.0/Integration_BeyondAdvanced.html)) | – | 24 PPQN; Start (0xFA) = downbeat, Stop 0xFC, Continue 0xFB, SPP 0xF2 in 16ths ([MIDI beat clock](https://en.wikipedia.org/wiki/MIDI_beat_clock), [SPP spec](http://midi.teragonaudio.com/tech/midispec/ssp.htm)) |
| Pro DJ link | Beat Link Trigger sends `SetBpm 123.4` to BEYOND (tempo only; phase not addressed) | – | Later: OSC `/tempo/bpm`, `/tempo/downbeat` |
| Audio-reactive params | FFT 512 bands, a per-band auto-amplitude detector, peak events; **Channels** mix manual, DMX and FFT (frequency index) with Physics; a master **Time/Channel mix** slider ([realtime audio](https://wiki.pangolin.com/doku.php?id=beyond%3Arealtime_audio), [channels](https://wiki.pangolin.com/doku.php?id=beyond:channels)) | – | Route bands (low/mid/high/custom) to any control id, with attack/release and amount. Include a "time vs audio" mix |

Practical advice worth building into the UI (PangoBeats doc): the cleanest
input is loopback or a venue AUX feed, and "keep the input peak between −25
and 0 dBFS". Built-in laptop mics with voice enhancement give poor results.
Show an **input level meter with a target band** and a **confidence
indicator**.

---

## 5. Pro output concerns

Zones, geometry and safety are covered in `pangolin.md` § 1.4–1.6 and
`showcontroller.md` § 1.7–1.8, and are tracked in T-003 and T-012. New
points:

- **Order of the modifier stack with zones**: Cue LC → Master LC → Zone LC
  (BEYOND allows Master after Zone). For us: cue → layer → master → zone
  geometry → calibration → **safety last**. Safety is never a live modifier.
- **Projector groups and chases**: Showcontroller has up to 32 named scanner
  groups ("Center", "Satellites", "All") and a **Chaser** that steps output
  through groups in order. A *free* chaser has its own speed and isn't saved
  with the scene; a scene-linked chaser must be programmed into the scene
  timeline
  ([chaser](https://www.showcontroller.com/en/manual/showcontroller-live/0-13-chaser.html)).
  BEYOND does the same with **Zone Chase** / Set / Add / Delete / Replace zone
  effects. Both are beat-synced.
- **Scan rate and point count** ([projector settings](https://wiki.pangolin.com/doku.php?id=beyond:projector_settings-new)):
  - A default sample rate per projector, clamped by **Minimum/Maximum**
    sliders because "a high sample rate may damage the scanners". Bigger size
    → larger angle → lower rate.
  - **Wide-angle compensation**: slows scanning automatically above ~50 %
    size.
  - **Minimum number of points** per frame: 200 by default (128 is the FB3
    floor; 500–700 sometimes). Tiny frames make the software compute
    thousands of FPS and overload USB/network. Padding with blank points
    halves the energy, so it's better to add lit points.
  - Vector content: the point count is computed from spacing so line
    brightness stays constant. That's our optimiser's `lit_step` (T-002).
  - Colour/blanking shift is a positive delay only. laser-dac already applies
    150 µs.
- **Point budget with layers**: total points per frame ≤ `pps / min_fps`
  (30 000 / 40 = 750). When exceeded: raise `lit_step` for all layers, then
  drop the lowest-priority layer, and warn.
- **Timecode and blackout**: see the TC-IN rule in § 3.2.

---

## 6. Implementation-ready spec for Laser Studio

Current state (read from the code): `engine::Settings` = content + colour +
scale + rotation speed + brightness + `AudioReact`; `Animator` integrates
time. `generators.rs` has 20 generators with `GenParams { count, a, b, speed,
color_mode, color2 }`. `presets.rs` has 202 cues on 8 pages. `scenes.rs` has
saved looks and a playlist. The UI is a single French page; every AZERTY
letter `azertyuiopqsdfghjklmwxcvbn` triggers a cue on the current page. Space
= arm and Escape = blackout are reserved. The browser sends
`{level, bass, beat}` to `/api/audio`.

Design principles:

1. **One render pipeline for live and timeline**: `ActiveCue` sources
   (grid, VLJ, timeline) → per-cue program/modifiers → layer modifiers →
   master modifiers → (zone) → calibration → safety → optimiser/output.
2. **All phases derive from one tempo clock** (`TempoClock::beat_at(t)`).
   Nothing integrates its own "beat time", so nothing drifts (the BEYOND
   Globe/Resync lesson).
3. **Everything controllable has a stable control id** (`master.size`,
   `grid.1.3.5`, `tempo.tap`…). UI, MIDI (T-200+), OSC (T-013), timeline
   envelopes and audio routing all address controls through it.
4. **Neutral defaults = identity.** A default `LiveModifiers` must leave the
   frame bit-identical, so old scenes and tests stay valid.
5. **Serde back-compat**: every new field `#[serde(default)]`; `Settings`
   keeps loading.
6. **Keyboard**: letters stay cue keys. New shortcuts use non-letter keys:
   `Enter` = tap, `Backspace` = resync, `<` (held) = reverse rotation,
   `Digit1…Digit0` = pages 1–10, and `Shift` held = temporary Flash mode.

### (F) Control ids (foundation, T-145)

```rust
/// Stable, dotted, lowercase id. Never renamed once shipped (add aliases).
pub struct ControlId(String);          // e.g. "master.rot_z.speed"

pub enum ControlKind {
    Continuous { min: f32, max: f32, default: f32, unit: Unit }, // Unit: Percent, Deg, DegPerSec, Hz, Beats, Bpm, None
    Toggle { default: bool },
    Momentary,                           // true while held (flash, freeze, black)
    Trigger,                             // one-shot (tap, resync, cue restart)
    Choice { options: Vec<&'static str>, default: usize },
}

pub struct ControlDesc {
    pub id: ControlId, pub label_fr: &'static str, pub group: &'static str,
    pub kind: ControlKind,
    pub external: bool,                  // false = UI only (e.g. arming the laser)
}
```

Id grammar: `master.<param>`, `layer.<1-4>.<param>`, `cue.<cue-id>.<param>`,
`grid.<page 1-10>.<row 1-5>.<col 1-8>` (APC40 clip matrix),
`fx.<line 1-4>.<slot 1-8>`, `fx.<line>.action`, `tempo.{tap,resync,bpm,nudge_up,nudge_down,double,half}`,
`page.{next,prev,<n>}`, `transport.{blackout,freeze,black_hold}`,
`timeline.{play,stop,loop}`, `vlj.enabled`. API: `GET /api/controls`,
`POST /api/control {id, value | norm}`, `GET /api/control-values` (for LED
feedback). **Arming is not an external control.**

### (A) Live modifiers panel (T-140 … T-146)

```rust
#[serde(default)]
pub struct LiveModifiers {
    pub brightness: f32,        // 0..1, 1.0
    pub size: f32,              // 0..2, 1.0
    pub size_x: f32, pub size_y: f32,   // -2..2, 1.0 (negative = flip)
    pub pos_x: f32, pub pos_y: f32,     // -1..1, 0.0
    pub rot_angle: [f32; 3],    // X,Y,Z degrees -180..180, 0
    pub rot_speed: [f32; 3],    // deg/s -720..720, 0  (or turns/bar when synced)
    pub rot_sync: bool,         // speeds in turns per bar, phase from tempo clock
    pub rot_reverse: bool,      // momentary invert (key "<")
    pub perspective: f32,       // 0..1, 0.3
    pub speed: f32,             // animation speed 0..4, 1.0
    pub color: ColorOverride,   // Normal
    pub strobe: Strobe,         // off
    pub trace: f32,             // visible points 0..1, 1.0
    pub dots: f32,              // dotting gap 0..1, 0.0
    pub mirror: Mirror,         // None | X | Y | XY
    pub prism: u8,              // 1..8, 1 (= off)
    pub freeze: bool, pub black: bool,   // momentary
}
pub enum ColorOverride {
    Normal,
    Fixed { rgb: [u8; 3] },
    Hue { hue: f32 },                                   // Colorspectrum-style
    Palette { palette: usize, mode: PaletteMode, offset: usize }, // Nearest | Step
    Rainbow { spread: f32 /*cycles along path 0..4*/, rate: Rate },
    Chase { palette: usize, step: Rate, spread: ChaseSpread /*Whole|Stroke|Point*/ },
}
pub struct Strobe { pub on: bool, pub rate: Rate /*Hz(8.0)|Beats(0.25)*/, pub duty: f32 /*0.1..0.9, 0.5*/ }
pub enum Rate { Hz(f32), Beats(f32) }   // Beats(n) = one cycle every n beats
```

UI labels (French): section **« Direct »** with tabs **Maître / Cue / Calque**.
Sliders: *Luminosité, Taille, Taille X, Taille Y, Position X, Position Y,
Rotation X/Y/Z (angle), Vitesse de rotation X/Y/Z, Vitesse d'animation,
Points visibles, Pointillés, Prisme*. Buttons: *Lent / Moyen / Rapide /
Stop* (rotation presets), *Inverser* (hold `<`), *Sync tempo*, *Stroboscope*
(latch) + *Flash strobe* (hold), *Figer* (hold), *Noir* (hold),
*Réinitialiser*. Colour: *Normal / Fixe / Teinte / Palette / Arc-en-ciel /
Chenillard*. Right-click (or double-click) on any control resets it to
neutral. Rotation presets: *Lent* 30 °/s, *Moyen* 90 °/s, *Rapide* 270 °/s;
with *Sync tempo*: *Lent* 1 turn / 4 bars, *Moyen* 1 turn / bar, *Rapide* 1
turn / 2 beats. 8 built-in palettes (our own): *Froid, Chaud, Feu, Océan,
Néon, Forêt, Tricolore, Blanc pur*.

Acceptance (summary; full lists in the tasks): the default `LiveModifiers`
gives an identical frame; every control round-trips through
`/api/control`; strobe duty is exact over 1 s at 60 fps (±1 frame); chase
steps land exactly on beat boundaries of the tempo clock; the stage costs
< 1 ms for 2000 points; blackout and safety still win.

### (B) Evolving cues: cue as a mini-timeline (T-157, T-158, T-164)

The festival-looks work (T-100, T-111) already defines keyframed *content*
(`Content::Evolving { length_beats, loop, keys }` with a per-key generator,
params, colour, gate/strobe and an `Easing`) and a `beat_pos` for
generators. T-157 **extends** that into a full mini-timeline instead of
defining a second engine:

```rust
#[serde(default)]
pub struct CueProgram {
    pub length_beats: Option<f32>,  // default: the evolving content's length, else 16
    pub play: PlayMode,             // Loop | Once | PingPong
    pub layers: Vec<Content>,       // up to 4 extra layers inside the cue (Showcontroller "Surface")
    pub lanes: Vec<Lane>,           // keyframes on the cue's LiveModifiers
    pub lfos: Vec<Modulator>,       // tempo-synced modulation (T-151)
}
pub struct Lane { pub target: String /* "size", "pos_x", "rot_z.speed", "color.hue", "strobe.on", "trace"… */,
                  pub keys: Vec<Key> }
pub struct Key { pub beat: f32, pub value: f32, pub ease: Easing /* T-111's enum: Step|Linear|EaseIn|EaseOut|Smooth */ }
pub struct Modulator { pub target: String, pub wave: Wave /* Sine|Triangle|Square|SawUp|SawDown|Random */,
                       pub rate: Rate, pub depth: f32, pub phase: f32, pub offset: f32 }
pub struct Transition { pub kind: TransitionKind /* Cut|Fade|FadeThroughBlack|Morph */, pub length: Rate }
```

`Settings` gets `#[serde(default)] program: Option<CueProgram>`. When it is
`None`, the cue is today's static look (or a plain `Content::Evolving`).
The cue's local beat is the same `beat_pos` as T-111 (quantised start), so
content keys, modifier lanes and LFOs stay in phase. Editor: **« Éditeur de
cue »** (T-164) reuses the timeline widget (T-162) and edits T-111 content
keys plus *Courbes* and *Modulateurs*.

### (C) Timeline with pre-made templates (T-160 … T-169)

Two time bases, like BEYOND's "Follow System BPM" option: **Seconds**
(a show locked to an audio file or timecode; seconds are primary and beats
come from the tempo map) and **Beats** (a phrase that follows the live
`TempoClock`, launched on the next bar; the festival timelines T-124–T-129
use this).

```rust
pub struct Show {
    pub name: String,
    pub time_base: TimeBase,          // Seconds | Beats
    pub tempo_map: Vec<TempoPoint>,   // { at_s: f64, bpm: f32, beats_per_bar: u8 } — used by Seconds shows
    pub audio: Option<AudioRef>,      // { file, offset_s }
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,         // { at_s, name, color }
    pub loop_region: Option<(f64, f64)>,
}
pub struct Track { pub name: String, pub kind: TrackKind /* Cues|Bus */, pub layer: u8, pub mute: bool, pub solo: bool, pub events: Vec<Event> }
pub struct Event {
    pub id: u64, pub start: f64, pub len: f64,   // in the show's time base
    pub source: EventSource,          // Cue(cue_id) | Inline(Settings)
    pub time: TimeMode,               // Beats | Seconds | Fit
    pub end: EndAction,               // Stop | Hold | Continue
    pub fade_in: Rate, pub fade_out: Rate,
    pub to_next: Option<Transition>,
    pub modifiers: LiveModifiers,
    pub envelopes: Vec<Envelope>,     // { target: ControlId-relative, keys: Vec<Key /*seconds*/> }
}
pub struct Template {                 // T-165
    pub name: String, pub length_bars: u16, pub slots: Vec<String>, // "A","B"…
    pub events: Vec<TemplateEvent>,   // positions in beats, slot references, envelopes in beats
}
```

Audio playback and the waveform are server-side (decode + peaks), with the
transport clock owned by the server. Bus tracks apply their envelopes to all
tracks of their layer. Built-in templates (ours): *Montée 8 mesures, Montée
16 mesures, Drop 16, Pause 8, Couplet 16, Sortie 8, Chenillard couleurs 4,
Stroboscope final 1*. Auto-generation (T-167) places templates on detected
sections.

### (D) Tempo engine (T-150 … T-154, MIDI clock in T-207)

```rust
pub struct TempoClock {
    pub bpm: f64,               // 120.0, clamp 40..=250
    pub beats_per_bar: u8,      // 4
    origin_s: f64,              // engine time where beat 0 happened
    pub source: TempoSource,    // Manual | Tap | Audio | MidiClock | Link
    taps: VecDeque<f64>,        // last 8 taps
}
impl TempoClock {
    fn beat_at(&self, t: f64) -> f64;        // (t - origin) * bpm / 60
    fn set_bpm(&mut self, bpm: f64, t: f64); // re-anchors origin so beat_at(t) is continuous
    fn tap(&mut self, t: f64);               // reset if gap > 2 s; needs ≥ 3 taps; median of intervals
    fn resync(&mut self, t: f64);            // beat_at(t) := next integer bar boundary (downbeat now)
    fn nudge(&mut self, beats: f64);         // shift phase ±1/32 beat per press
}
```

UI **« Tempo »** bar: large BPM, beat LEDs (1-2-3-4), *Tap* (Entrée),
*Resync* (⌫), *÷2*, *×2*, *◀ ▶ nudge*, source selector *Manuel / Tap / Audio
/ Horloge MIDI / Link*, and a confidence gauge for audio.

### (E) Layering and cue modes (T-155, T-156, T-159, T-148)

```rust
pub enum ClickMode { Toggle, Flash, Solo, Restart }
pub struct CueSlot {                  // saved per grid cell
    pub cue: String,                  // preset id or user cue id
    pub mode: Option<ClickMode>,      // override of the grid mode
    pub group: Option<u8>,            // 1..8, one active cue per group
    pub layer: u8,                    // 1..4, default 1
    pub transition: Option<Transition>,
    pub quantize: Quantize,           // Off | Beat | Bar  (default Beat when tempo running)
    pub modifiers: LiveModifiers,     // cue-level, saved
    pub vlj_skip: bool,
}
pub struct Layer { pub dimmer: f32 /*1.0*/, pub mute: bool, pub solo: bool, pub modifiers: LiveModifiers }
pub struct Mixer { pub layers: [Layer; 4], pub max_active: u8 /*4*/, pub point_budget: usize /*750*/ }
```

UI: grid toolbar *Basculer / Flash / Solo / Relancer*, *Un cue / Multi*,
*Groupe*, per-layer strips *Calque 1–4* (dimmer, *Muet*, *Solo*), *Transition
: Coupe / Fondu / Fondu au noir / Morph* + *Durée* (s or beats), *Pilote auto
(VLJ)* with *toutes les N mesures*, *Ordre / Aléatoire*.

### Priorities (task map)

The task files are in `tasks/` (French, generated index `tasks/INDEX.md`).
They were reconciled with the tasks other agents wrote at the same time:

- **T-100** (festival): `GenCtx.beat_pos` with a temporary internal clock →
  replaced by T-150 (one clock only).
- **T-111** (festival): keyframed `Content::Evolving` → T-157 builds on it.
- **T-130**: named palettes → T-141 reuses the same `Palette` type.
- **T-101**: strobe limiter (> 4 Hz for > 5 s → steady output) → also applies
  to the live strobe (T-142) and the FX grid (T-146).
- **T-207**: MIDI clock in/out → T-154 is only Ableton Link.
- **T-208**: MIDI arming is opt-in (Shift + hold 1 s) → T-145 exposes
  `transport.arm` as `external: false` unless that option is on.
- **T-202/T-204** map APC40 controls to the T-145 ids (`live.*` is accepted
  as an alias of `master.*`).

| Id | Title | Prio | Depends |
|---|---|---|---|
| T-145 | Control-id registry | P1 | – |
| T-150 | Tempo engine | P1 | – |
| T-140 | Master live-modifier stage (geometry, brightness, speed) | P1 | T-145 |
| T-141 | Live colour override (fixed, hue, palette, rainbow, chase) | P1 | T-140, T-150 |
| T-143 | « Direct » panel UI | P1 | T-140, T-141, T-150 |
| T-151 | Tempo-synced LFO modulators | P1 | T-150, T-145 |
| T-155 | Click modes, groups, limiter | P1 | T-145 |
| T-156 | Four layers + point budget | P1 | T-155, T-140 |
| T-157 | Cue program: layers, modifier lanes and LFOs on top of T-111 | P1 | T-111, T-151, T-140 |
| T-142 | Strobe, trace, dotting, mirror/prism, freeze, black | P2 | T-140, T-150 |
| T-144 | Cue/layer-level modifiers + smoothing | P2 | T-140, T-156 |
| T-146 | FX grid on top of cues | P2 | T-140, T-151 |
| T-152 | Audio BPM detection with confidence | P2 | T-150 |
| T-153 | Audio band routing to any control | P2 | T-145, T-151 |
| T-158 | Cue transitions (cut/fade/morph) | P2 | T-155, T-150 |
| T-159 | Quantised launch + beat mode | P2 | T-150, T-155 |
| T-148 | Virtual LJ autopilot | P3 | T-159 |
| T-160 | Timeline model + player | P2 | T-150, T-156 |
| T-161 | Audio file + waveform | P2 | T-160 |
| T-162 | Timeline editor UI | P2 | T-160, T-161 |
| T-163 | Parameter envelopes | P2 | T-160, T-145 |
| T-164 | Cue program editor | P2 | T-157, T-162 |
| T-165 | Timeline templates infrastructure | P2 | T-160, T-163 |
| T-166 | Built-in template pack | P3 | T-165 |
| T-167 | Auto-show from audio analysis | P3 | T-165, T-161 |
| T-168 | Timecode in (MTC, LTC) | P3 | T-160 |
| T-169 | Record live performance into timeline | P3 | T-160, T-145 |
| T-154 | Ableton Link (licence study, then optional feature) | P3 | T-150 |
| T-170 | Projector groups and chaser | P3 | T-012, T-150 |
| T-171 | Point budget and scan-rate management | P2 | T-002 |

---

## Sources (accessed 2026-09-27)

Pangolin wiki (public documentation):
- BEYOND Live Control, Time Control, FX speed, QuickFX: https://wiki.pangolin.com/doku.php?id=beyond:livecontrol
- BEYOND grid toolbar (click modes, BPM, LINK): https://wiki.pangolin.com/doku.php?id=beyond:grid_toolbar
- BEYOND cue properties: https://wiki.pangolin.com/doku.php?id=beyond:cue_properties
- BEYOND Dynamics tab (limiters, soft start/finish, transition): https://wiki.pangolin.com/doku.php?id=beyond:dynamics_tab
- BEYOND workspace grids and pages: https://wiki.pangolin.com/doku.php?id=beyond:workspace_grids_and_pages
- BEYOND effects: https://wiki.pangolin.com/doku.php?id=beyond%3Aeffects
- BEYOND channels and colour channels: https://wiki.pangolin.com/doku.php?id=beyond:channels
- BEYOND realtime audio: https://wiki.pangolin.com/doku.php?id=beyond%3Arealtime_audio
- PangoBeats 1.1: https://wiki.pangolin.com/doku.php?id=beyond:pangobeats
- BEYOND timeline guide: https://wiki.pangolin.com/doku.php?id=beyond:timeline
- BEYOND timeline event tab: https://wiki.pangolin.com/doku.php?id=beyond:timeline_event_tab
- Timeline and BPM relation: https://wiki.pangolin.com/doku.php?id=quickshow:timeline_bpm
- BEYOND timecode: https://wiki.pangolin.com/doku.php?id=beyond:timecode
- BEYOND projector settings (scan rate, min points, wide-angle, Beam Brush timing): https://wiki.pangolin.com/doku.php?id=beyond:projector_settings-new
- Beam Brush: https://wiki.pangolin.com/doku.php?id=beyond%3Abeambrush
- BEYOND Virtual Laser Jockey: https://wiki.pangolin.com/doku.php?id=beyond:virtual_laser_jockey
- QuickShow live controls and subpages: https://wiki.pangolin.com/doku.php?id=quickshow:live_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:master_and_cue_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:size_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:position_and_rotation_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:color_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:playback_controls ,
  https://wiki.pangolin.com/doku.php?id=quickshow:controlling_cues_during_playback
- QuickShow QuickFX: https://wiki.pangolin.com/doku.php?id=quickshow:quickfx
- QuickShow VLJ: https://wiki.pangolin.com/doku.php?id=quickshow:virtual_laser_jockey
- QuickShow music and beats: https://wiki.pangolin.com/doku.php?id=quickshow:music_and_beats_overview
- QuickShow features page: https://quickshowlaser.com/powerful/features/features.html

Laserworld Showcontroller:
- Showcontroller LIVE user manual (PDF): https://www.laserworld.com/en/download-file-1700-Showcontroller_LIVE___EN.html
- Modes/effects: https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-f-modes-effects.html
- Live colour: https://www.showcontroller.com/en/manual/showcontroller-live/0-11-live-change-color.html
- Frame change animations: https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-g-farme-change-animations.html
- Timing: https://www.showcontroller.com/en/manual/showcontroller-live/0-9-remote-control-the-software/0-9-e-timing.html
- Chaser: https://www.showcontroller.com/en/manual/showcontroller-live/0-13-chaser.html
- Timecode: https://www.showcontroller.com/en/manual/7-special-features/7-2-timecode.html

Workflow, tempo, other tools:
- Starshine, QuickShow vs BEYOND workflow: https://www.starshinelights.com/blogs/news/laser-show-light-quickshow-beyond
- Starshine, syncing lasers to music: https://www.starshinelights.com/blogs/news/laser-music-sync-quickshow-beyond
- Pangolin blog, synchronize laser shows to music: https://pangolin.com/blogs/news/synchronize-laser-light-shows-to-music
- Beat Link Trigger × BEYOND (SetBpm): https://blt-guide.deepsymmetry.org/beat-link-trigger/8.0.0/Integration_BeyondAdvanced.html
- CloudLase: https://cloudlase.studio/
- Lazura: https://lazura.app/
- Liberation: https://liberationlaser.com/
- MIDI beat clock: https://en.wikipedia.org/wiki/MIDI_beat_clock ; SPP: http://midi.teragonaudio.com/tech/midispec/ssp.htm
- Ableton Link Rust wrappers and licence: https://github.com/anzbert/rusty_link , https://github.com/anweiss/ableton-link-rs
- Ellis, "Beat Tracking by Dynamic Programming" (2007): https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf
- Foote, "Automatic audio segmentation using a measure of audio novelty" (2000): https://ieeexplore.ieee.org/document/869637
- librosa beat tracker (reference only): https://librosa.org/doc/main/generated/librosa.beat.beat_track.html
