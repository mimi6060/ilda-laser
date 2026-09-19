# Pi Laser HAT - KiCad files

Generated from `../DESIGN.md`'s schematic-level spec via KiCad 8's
`pcbnew` Python API (placement + netlist) and Freerouting 2.4.1 (routing),
both run inside Docker - no local KiCad or Java install needed to
regenerate these. **Status: DRC-clean, gerbers exported, ready for
review before ordering.**

## Files

| File | What it is |
|------|------------|
| `board.kicad_pcb` (+ `.kicad_pro`/`.kicad_prl`) | All 40 parts placed, every net assigned, **not routed** (ratsnest only). DRC-clean for placement: zero shorts, zero courtyard overlaps, zero parts outside the outline. |
| `board_freerouted.kicad_pcb` (+ `.kicad_pro`) | **The final routed board.** DRC-clean: 0 errors, 7 warnings (all cosmetic silkscreen-text clipping, e.g. a reference designator slightly overlapping a pad's solder mask - doesn't affect fabrication or function). This is the file the gerbers below were exported from. |
| `build_board.py` | Places every component and wires every net from `DESIGN.md`, computed from each footprint's real (measured, not guessed) courtyard geometry. Produces `board.kicad_pcb`. |
| `export_dsn.py` | Exports `board.kicad_pcb` as Specctra DSN (`board.dsn`) for Freerouting, and widens the default trace width/clearance from KiCad's 0.2mm default to 0.25mm (this board has plenty of room; no reason to push fab tolerances). |
| `import_ses.py` | Imports Freerouting's routed session (`board_routed.ses`) back onto `board.kicad_pcb`, saving `board_freerouted.kicad_pcb`. |
| `board_placed.pos` | Component placement (CPL) file exported from the placement-only board - superseded by `../gerbers/board_freerouted-CPL.csv`, kept for reference. |
| `placement_drc.json`, `freerouted_drc.json` | Full DRC reports (`kicad-cli pcb drc --format json`) for the placement-only and final routed boards. |
| `placement_render.png`, `board_freerouted.png` | Top-view renders (SVG via `kicad-cli`, rasterized with `rsvg-convert`). |

`../gerbers/` has the fabrication output: Gerbers, drill files, CPL, and a
BOM, ready to hand to JLCPCB (or any fab). See that directory's own notes
below.

## How the routing was actually done

The first attempt (now removed) was a hand-written naive Manhattan router
- it achieved full connectivity but left 417 DRC violations (mostly
clearance/crossings) because it didn't do real clearance checking, just
avoided literal trace-to-trace intersections. That wasn't good enough to
order against.

This version uses **Freerouting** (github.com/freerouting/freerouting), a
real autorouter built for exactly this, via the standard KiCad workflow:

1. `export_dsn.py`: `board.kicad_pcb` -> Specctra DSN (`board.dsn`), with
   trace width/clearance widened to 0.25mm.
2. Freerouting run headless (needs Java 25 - the released jar is compiled
   for a newer class file version than Java 21/23 support; a Java 21 or 23
   JRE fails with `UnsupportedClassVersionError`):
   ```
   java -Djava.awt.headless=true -jar freerouting.jar \
     -de board.dsn -do board_routed.ses -mp 20
   ```
   Completed in ~7 seconds: fanout, then 6 auto-routing passes, finishing
   with **0 unrouted, 0 violations**, score 999.99/1000.
3. `import_ses.py`: the resulting `.ses` imported back onto
   `board.kicad_pcb`, saved as `board_freerouted.kicad_pcb`.
4. `kicad-cli pcb drc` on the result: **0 errors, 7 warnings** (cosmetic
   silkscreen only - see `freerouted_drc.json` for exact details).

The `freerouting.jar` itself (64MB) isn't committed here - re-download
from the GitHub releases page (v2.4.1) to reproduce.

## What's actually verified

- **Board outline and mounting holes**: cross-checked against both the
  official `hat-board-mechanical.pdf` (raspberrypi/hats repo) and the
  official KiCad team's own `raspberrypi_hat` template project - both
  agree exactly (65x56mm, 3mm corners, holes at (3.5,3.5)/(61.5,3.5)/
  (3.5,52.5)/(61.5,52.5)).
- **GPIO header placement**: reused the exact same footprint, relative
  position, rotation, and layer as the official KiCad template.
- **MCP4922 and MCP6004 pinouts**: read directly from Microchip's real
  datasheets (DS22250A and DS20001733L). This caught two things
  `DESIGN.md`'s original text missed: the MCP4922 has a hardware
  `/SHDN` pin (9) separate from the per-channel SHDN bit in the SPI
  command word, and two separate VREFA/VREFB pins, not one shared VREF -
  both now wired correctly; see `DESIGN.md`'s note.
- **Placement**: zero courtyard overlaps, zero shorted pads, zero parts
  outside the board outline.
- **Routing**: 0 DRC errors after Freerouting + KiCad DRC, as above.

## What's not verified

- **RJ45 footprint**: standard-library `RJ45_Amphenol_54602-x08_Horizontal`
  used as a stand-in for the actual BOM part (EVERCOM 5301-8P8C) - both
  are generic unshielded right-angle THT 8P8C jacks, but their exact pin
  spacing/mounting-peg positions were not cross-checked against the real
  5301-8P8C datasheet. **Check this specifically before ordering** - if it
  doesn't match, only J1's footprint needs to change.
- **Nothing here has touched real hardware.** Placement, pinouts, and
  routing are verified against primary sources and DRC; the actual analog
  design (gain values, differential mirror topology) is still the
  unbench-tested design from `DESIGN.md` - follow its bring-up/trim
  procedure once assembled.
- The 7 remaining cosmetic silkscreen warnings weren't chased to zero
  (diminishing returns - they don't affect fabrication).
