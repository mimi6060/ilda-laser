# Pi Laser HAT - KiCad files

Generated from `../DESIGN.md`'s schematic-level spec via the two Python
scripts here (using KiCad 8's `pcbnew` scripting API, run inside the
`kicad/kicad:8.0` Docker image - no local KiCad install needed to
regenerate these). **Honest status: placement is solid and verified;
routing is not clean and needs a human pass in KiCad before ordering.**
See "What's actually verified" below before trusting anything here.

## Files

| File | What it is | Status |
|------|------------|--------|
| `board.kicad_pcb` (+ `.kicad_pro`/`.kicad_prl`) | All 40 parts placed, every net assigned, **not routed** (ratsnest only) | DRC-clean for the things that matter before ordering: zero shorts, zero courtyard overlaps, zero parts outside the board outline. 9 remaining violations are cosmetic silkscreen text clipping. **This is the file to open and route by hand in KiCad.** |
| `board_routed.kicad_pcb` (+ `.kicad_pro`) | Same board, with an automatic first-pass routing attempt | **Not DRC-clean** - 417 violations remain (mostly trace clearance/crossings my simple router couldn't avoid). Electrically, every net IS connected (0 unconnected items) - it's a real starting point, not gerber-ready. Load this in KiCad and use it to see what's already wired vs. what still needs cleanup, rather than starting from a blank ratsnest. |
| `build_board.py` | Places every component and wires every net from `DESIGN.md`, computed from each footprint's real (measured, not guessed) courtyard geometry so components can't overlap by an arithmetic mistake | Re-run: `docker run --rm -v $PWD:/work -w /work kicad/kicad:8.0 python3 build_board.py` |
| `route_board.py` | Loads `board.kicad_pcb`, attempts a naive Manhattan (L-shaped) auto-route of every net, hopping to the back copper layer via vias where a straight shot would cross something already placed | Good enough to get full connectivity (0 unconnected) but not full DRC cleanliness - see below |
| `board_placed.pos` | Component placement/position (CPL) file exported from the *placed* board | Valid and usable now - doesn't depend on routing |
| `placement_drc.json`, `routed_drc.json` | Full DRC reports (`kicad-cli pcb drc --format json`) for the placement-only and routed boards respectively | Read these for exact violation details rather than trusting a summary |
| `placement_render.png`, `board_routed.png` | Top-view renders (SVG exported via `kicad-cli`, rasterized with `rsvg-convert`) | For visual review without opening KiCad |

## What's actually verified

- **Board outline and mounting holes**: cross-checked against both the
  official `hat-board-mechanical.pdf` (raspberrypi/hats repo) and the
  official KiCad team's own `raspberrypi_hat` template project - both
  agree exactly (65x56mm, 3mm corners, holes at (3.5,3.5)/(61.5,3.5)/
  (3.5,52.5)/(61.5,52.5)). High confidence this fits a real Pi.
- **GPIO header placement**: reused the exact same footprint, relative
  position, rotation, and layer as the official KiCad template (rather
  than re-deriving the rotated pad geometry by hand, which went wrong on
  the first attempt - a footprint flip/rotate ordering bug put it
  entirely outside the board until this was caught by checking the actual
  pad coordinates the API reported, not just eyeballing a render).
- **MCP4922 and MCP6004 pinouts**: read directly from Microchip's real
  datasheets (DS22250A and DS20001733L), not recalled from memory. This
  caught two things `DESIGN.md`'s original text missed: the MCP4922 has a
  hardware `/SHDN` pin (9) separate from the per-channel SHDN bit in the
  SPI command word, and two separate VREFA/VREFB pins, not one shared
  VREF. Both are now wired correctly (SHDN tied to VDD_3V3, both VREFs to
  VDD_3V3); `DESIGN.md` is updated with a note.
- **Placement**: zero courtyard overlaps, zero shorted pads, zero parts
  outside the board outline, confirmed by `kicad-cli pcb drc` on
  `board.kicad_pcb`, not just visual inspection.
- **RJ45 footprint**: standard-library `RJ45_Amphenol_54602-x08_Horizontal`
  used as a stand-in for the actual BOM part (EVERCOM 5301-8P8C) - both
  are generic unshielded right-angle THT 8P8C jacks, but their exact pin
  spacing/mounting-peg positions were not cross-checked against the real
  5301-8P8C datasheet. **Check this specifically before ordering** - if it
  doesn't match, only J1's footprint needs to change, nothing else.

## What's not verified / what to do before ordering

- **Routing is not DRC-clean.** `route_board.py` is a from-scratch, naive
  router (pad-to-pad Manhattan paths, layer-hop on same-layer crossing)
  written because I don't have a reliable autorouter available in this
  environment - it is not a substitute for KiCad's interactive router,
  which does real-time clearance checking this script doesn't replicate.
  Open `board_routed.kicad_pcb` in KiCad, run DRC, and clean up the
  reported clearance/crossing violations by hand - with placement already
  solid, this is a cleanup pass, not a from-scratch route.
- **No gerbers, drill files, or final BOM export yet** - deliberately, since
  generating them from a non-DRC-clean board would misrepresent the board
  as order-ready. Once routing is clean, export via `kicad-cli pcb export
  gerbers` / `drill` and hand-build the BOM from `../DESIGN.md`'s table
  (JLCPCB part numbers are already there).
- **Nothing here has touched real hardware.** Placement and pinouts are
  verified against primary sources (datasheets, official mechanical
  drawings); the actual analog design (gain values, differential mirror
  topology) is still the unbench-tested design from `DESIGN.md` - follow
  its bring-up/trim procedure once assembled.
