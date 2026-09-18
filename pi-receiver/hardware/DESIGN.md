# Pi Laser HAT - hardware design

A Raspberry Pi HAT-style board: plugs onto the Pi's 40-pin GPIO header, has
an onboard RJ45 jack wired to the analog X/Y/R/G/B pinout from the laser's
own manual, and adjustable-gain/offset output stages so it can be tuned to
whatever voltage the laser's ILDA input actually expects (undocumented -
see the "why adjustable, not fixed" note below).

**Status: schematic-level design, not yet laid out as a PCB.** Everything
below is precise enough to enter directly into schematic capture (KiCad or
otherwise) using each part's standard symbol - part names/pin *functions*
are given rather than raw pin numbers for the ICs, since the correct
numbered pinout comes from each part's library symbol, and I have no way to
render/DRC-check a hand-authored PCB layout file myself. The Raspberry Pi
GPIO header pin numbers below are the one fixed physical standard and are
given directly (unchanged across Pi 2/3/4/5).

## Why adjustable, not a fixed design

The laser's manual documents the RJ45<->DB25 pin mapping but never states
the actual voltage range or whether X-/Y- are true differential or just a
ground reference. Guessing wrong on a fixed-value design risks either no
output or, worse, feeding the laser's input a voltage outside what it
tolerates. So every output stage here has a trimmer, and the bring-up
procedure at the bottom starts at minimum gain and works up while measuring
with a multimeter - nothing is sent to the laser itself until the levels
are confirmed safe.

## Block diagram

```
Pi 40-pin GPIO
  |  SPI0 (CE0, CE1, SCK, MOSI)      SPI1 (CE0, SCK, MOSI)
  v                                    v
+----------+  +----------+        +----------+
| DAC1     |  | DAC2     |        | DAC3     |
| MCP4922  |  | MCP4922  |        | MCP4922  |
| A=X  B=Y |  | A=R  B=G |        | A=B  B=- |   (chan B unused)
+----+--+--+  +----+--+--+        +----+-----+
     |  |          |  |                |
     v  v          v  v                v
   [X+][Y+]      [R] [G]              [B]     <- non-inverting adjustable
     |  |          |  |                |         gain buffers (5x, on two
     |  |          |  |                |         MCP6004 quad op-amps)
     v  v          |  |                |
  [X- mirror]       |  |                |
  [Y- mirror]  <-- uses shared Vbias trimmer + buffer (also on the
                    MCP6004s) to mirror X+/Y+ around an adjustable center

  X+ X- Y+ Y- R G B  --(470R series each)-->  RJ45 pins 8 2 1 7 4 5 6
                                               (GND on pin 3, per manual)
```

## Bill of materials

| Ref | Part | Qty | JLCPCB part # | Notes |
|-----|------|-----|----------------|-------|
| U1, U2, U3 | MCP4922-E/SL (dual 12-bit SPI DAC, SOIC-14) | 3 | C39851 | X/Y, R/G, B |
| U4, U5 | MCP6004-I/ST (quad rail-to-rail op-amp, TSSOP-14) | 2 | C185811 | 8 stages total, all used |
| J1 | RJ45 jack, 8P8C, unshielded, right-angle | 1 | C3097717 | Wired per the laser manual's DB25<->RJ45 table |
| J2 | 2x20 female header, 2.54mm, stacking/tall | 1 | *source separately* | Standard HAT connector; specialty mechanical part, not reliably in general SMT libraries - buy from a Pi parts retailer (e.g. an "assembled Pi HAT header") and hand-solder |
| RV1-RV5 | 10k trim potentiometer (X+, Y+, R, G, B gain) | 5 | search "10K trimmer potentiometer" in JLCPCB parts | Sets each channel's output gain, 1.0x-1.5x |
| RV6 | 10k trim potentiometer (Vbias center) | 1 | same as above | Sets the X-/Y- mirror center voltage |
| R1-R5 | 20k, 1% | 5 | generic 0603 1% | Gain-stage ground resistor (paired with RV1-RV5) |
| R6-R9 | 20k, 1% | 4 | generic 0603 1% | X-/Y- mirror input+feedback resistors (2 per mirror) |
| R10-R16 | 470R | 7 | generic 0603 | Series protection on every RJ45 output line |
| R17 | 10k | 1 | generic 0603 | Pull-down on shared LDAC net (defined-low at boot, before firmware runs) |
| C1-C5 | 100nF ceramic | 5 | generic 0603 | Decoupling, one per IC, placed close to each VDD pin |
| C6 | 10uF, 5V+ | 1 | generic tantalum/ceramic | Bulk decoupling at the power entry from J2 |

Total IC cost is a few euros; the header and RJ45 jack are the only
mechanically-specific parts.

## Power

- Pull 5V from J2 pin 2 or 4, GND from any GND pin (e.g. 6, 9, 14, 20...).
  This 5V rail feeds the MCP6004 op-amps directly (rail-to-rail I/O, so
  their output can swing close to 0-5V).
- Pull 3.3V from J2 pin 1 or 17 for the MCP4922s' VDD **and** VREF pins.
  Powering the DACs at 3.3V (not 5V) matters: it keeps their SPI logic
  thresholds matched to the Pi's 3.3V GPIO output levels. Powering them at
  5V while driving them from 3.3V GPIO risks the chip not reliably reading
  the Pi's "high" as a logic high. The 0-3.3V DAC output is then scaled up
  to 0-5V (adjustable) by the op-amp gain stage anyway, so nothing is lost.

## Signal connections

### SPI (Pi -> DACs)

| Signal | Pi physical pin | Pi GPIO | Goes to |
|--------|------------------|---------|---------|
| SPI0 SCK | 23 | GPIO11 | U1.SCK, U2.SCK |
| SPI0 SDI (MOSI) | 19 | GPIO10 | U1.SDI, U2.SDI |
| SPI0 CE0 | 24 | GPIO8 | U1.CS (X/Y chip) |
| SPI0 CE1 | 26 | GPIO7 | U2.CS (R/G chip) |
| SPI1 SCK | 40 | GPIO21 | U3.SCK |
| SPI1 SDI (MOSI) | 38 | GPIO20 | U3.SDI |
| SPI1 CE0 | 12 | GPIO18 | U3.CS (B chip) |
| LDAC (shared, all 3 DACs) | 16 | GPIO23 | U1.LDAC, U2.LDAC, U3.LDAC, and R17 (10k) to GND |

`pi-receiver`'s firmware ties LDAC low in the immediate-update mode
already implemented (see `../src/main.rs`); R17 just guarantees it reads
low during boot before the program starts driving GPIO23 (avoids a
floating pin outputting garbage on power-up). MCP4922 has no data-out pin,
so nothing connects to the Pi's MISO.

### DAC analog outputs -> op-amp gain stages

For each of the 5 non-inverting buffers (X+, Y+, R, G, B - same topology,
different op-amp instances across U4/U5):

```
DAC VOUTx --------------------+--> op-amp non-inverting input (+)
                               |
        op-amp output --------+--> RJ45 line (via 470R)
              |
              +---[ RVn wiper, 10k trimmer, other 2 legs to op-amp
              |     output and to the inverting input side ]
              |
   inverting input (-) ---[ Rn, 20k 1% ]--- GND
```

Concretely: `Gain = 1 + Rf/Rg` where `Rg` = the fixed 20k resistor to
ground and `Rf` = the trimmer's resistance between the output and the
inverting input. Trimmer at 0: gain 1.0x. Trimmer at max (10k): gain 1.5x.
DAC output 0-3.3V -> stage output 0V to 4.95V, continuously adjustable.

### X-/Y- differential mirror stages

```
X+ stage output ---[ R6, 20k 1% ]---+--- op-amp inverting input (-)
                                     |
                    op-amp output --+--- (feedback, R7 = 20k 1%, same value as R6)
                                     |
                                     +--> RJ45 X- line (via 470R)

Vbias (buffered) -------------------> op-amp non-inverting input (+)
```

Same topology again for Y- with R8/R9 (20k 1% each) from the Y+ stage's
output. Because R6=R7 (and R8=R9) exactly, `Vout = 2*Vbias - Vin`: a mirror
image of the "+" signal around Vbias. Since it's built from the *already
gain-adjusted* X+/Y+ output rather than the raw DAC signal, it automatically
tracks whatever gain RV1/RV2 end up set to - the mirror stays centered
without needing to match two independent trimmers.

### Vbias reference

```
VDD (5V) ---[ RV6, 10k trimmer, 3-terminal ]--- GND
                        |
                      wiper --> spare op-amp, wired as a unity-gain
                                follower (output tied straight back to
                                its own inverting input) --> Vbias
```

The follower gives Vbias a low output impedance so it can drive both
mirror stages without sagging. This uses the 8th (otherwise spare)
op-amp channel across U4/U5 - all 8 channels end up used.

### RJ45 pinout

Wired exactly per the laser's own manual (DB25<->RJ45 table), **not** a
standard Ethernet pinout:

| RJ45 pin | Signal | Source |
|----------|--------|--------|
| 1 | X- | mirror stage output (via 470R) |
| 2 | Y- | mirror stage output (via 470R) |
| 3 | GND | board ground |
| 4 | R | buffer stage output (via 470R) |
| 5 | G | buffer stage output (via 470R) |
| 6 | B | buffer stage output (via 470R) |
| 7 | Y+ | buffer stage output (via 470R) |
| 8 | X+ | buffer stage output (via 470R) |

## Bring-up and trim procedure

Do this **before** ever plugging the RJ45 cable into the laser.

1. Assemble/solder the board, double-check no shorts with a multimeter in
   continuity mode (VDD-to-GND especially) before applying any power.
2. Set all trimmers (RV1-RV6) to minimum.
3. Power the Pi, run `pi-receiver`, and from the PC run
   `ilda-laser pattern cross --device idn:<hostname> --pps 2000`.
4. With a multimeter on each RJ45 pin (referenced to pin 3/GND), confirm:
   - R, G, B pins swing between roughly 0V and a small positive voltage as
     the pattern's color changes (or hold steady if using a single solid
     color).
   - X+/Y+ read close to their expected center voltage when the beam is at
     the cross's center, and swing symmetrically as it moves to the edges.
   - X-/Y- mirror X+/Y+: as X+ rises, X- should fall by roughly the same
     amount around Vbias.
5. Slowly raise RV6 (Vbias) until X-/Y-'s center point lines up with X+/Y+'s
   own center voltage (this is the "match the mirror" calibration step).
6. Only once levels look sane and stable: connect the RJ45 cable to the
   laser (via whatever DB25 breakout the manual describes), with the laser
   itself powered but the beam pointed somewhere safe. Slowly raise RV1/RV2
   (X/Y gain) from minimum and check the projected pattern's size and
   orientation. Adjust RV3-RV5 for color response.
7. If nothing lights up or the pattern looks wrong (mirrored, wrong axis,
   clipped) - stop, disconnect, and recheck the RJ45 pin assignment/voltage
   readings before continuing. Don't guess at the laser by raising gain
   further.

## What isn't verified

This is a from-scratch analog design against an undocumented ILDA input -
none of it has been bench-tested, and there's no PCB layout yet (copper
routing/DRC needs an actual EDA tool with visual review, which I don't
have). Before ordering fabrication+assembly: build this schematic in
KiCad (free), then lay out the board using an existing Raspberry Pi HAT
KiCad template for the mechanical outline/mounting holes (getting that
wrong means the board doesn't fit the Pi), and have it DRC-checked. Order
JLCPCB's cheap low-quantity prototype tier first, not a big batch.
