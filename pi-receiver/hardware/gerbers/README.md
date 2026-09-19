# Fabrication files - Pi Laser HAT

Exported from `../kicad/board_freerouted.kicad_pcb` (DRC-clean: 0 errors,
7 cosmetic silkscreen warnings - see `../kicad/README.md`).

- **`gerbers.zip`** - Gerbers + drill files (F/B copper, F/B silkscreen,
  F/B soldermask, F/B paste, edge cuts, PTH+NPTH drill, job file). Upload
  this directly to JLCPCB's Gerber viewer to check the board before
  ordering.
- **`board_freerouted-CPL.csv`** - component placement (position) file,
  for the PCBA/assembly step.
- **`board_freerouted-BOM.csv`** - bill of materials with JLCPCB part
  numbers where known (MCP4922, MCP6004, RJ45 - all confirmed in stock at
  the time `DESIGN.md` was written). The trimmer potentiometers, generic
  0603 resistors/caps, and the GPIO stacking header don't have a part
  number filled in - either search JLCPCB's parts library for an
  in-stock match (10k trimmer, 0603 1% resistors, 0603 ceramic caps), or
  source them yourself and hand-solder (recommended for the GPIO header
  regardless - it's a mechanically specific through-hole part not
  reliably available in general SMT assembly libraries).

## Before ordering

1. **Verify the RJ45 footprint** (see `../kicad/README.md`'s "What's not
   verified" section) - a stand-in footprint was used, not the exact BOM
   part's own datasheet dimensions.
2. **Order the cheap low-quantity prototype tier first**, not a big batch
   - this design has never touched real hardware (see `../DESIGN.md`'s
     bring-up procedure, which assumes exactly that).
3. Open `gerbers.zip` in JLCPCB's online Gerber viewer (or KiCad's 3D
   viewer / `kicad-cli pcb export svg` on `board_freerouted.kicad_pcb`)
   for one more visual pass before paying.
