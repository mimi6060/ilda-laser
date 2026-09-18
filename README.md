# ilda-laser

A Rust workspace to control a laser projector like Pangolin's FB3/FB4 does
internally: stream ILDA points to a network laser DAC, or send DMX-512 over
Art-Net. Two crates:

- **`pc-client`** - CLI that runs on your PC/Mac. Plays `.ild` files, streams
  built-in test patterns, or sends DMX over Art-Net.
- **`pi-receiver`** - runs on a Raspberry Pi 3. Turns it into the actual
  laser DAC: receives the point stream over Ethernet and drives three
  MCP4922 SPI DAC chips to produce the analog X/Y/R/G/B signals the laser's
  ILDA input expects.

```
PC (pc-client)  --Ethernet, IDN protocol-->  Raspberry Pi 3 (pi-receiver)
                                                --SPI-->
                                    3x MCP4922 DAC chips (analog X/Y/R/G/B)
                                                --DB25 ILDA cable-->
                                               Laser
```

## Why you need this at all, not just an RJ45 cable

A PC's (or Pi's) network card only sends digital Ethernet frames. It cannot
put analog galvo-control voltages on a wire by itself. The laser's DB25 ILDA
input expects real analog X/Y/R/G/B signals, and Raspberry Pi GPIO pins are
digital 3.3V on/off - they can't drive that input directly either.

Some laser manuals include a "25 pin interface to Ethernet port" wiring
table (DB25 pin -> RJ45 pin, e.g. `X- -> Pin14`, `R -> Pin5`, ...). That is
**not** a network protocol - it's a trick to carry the same analog ILDA
signals over an Ethernet-style cable's wire pairs, purely to reach a longer
distance than a bulky ILDA cable allows. It is unrelated to what this
project does.

`pi-receiver` is the missing piece: an actual digital-to-analog converter,
built from a Pi plus three cheap SPI DAC chips, so the Pi can generate real
analog voltages on its own DB25 ILDA output.

## Protocol choice: IDN, not Ether Dream

`pc-client` and `pi-receiver` talk **IDN** (ILDA Digital Network) over UDP,
not Ether Dream. Both are supported by the underlying `laser-dac` crate, but
IDN was chosen for the Pi side because the crate ships a complete, tested
IDN server implementation (`laser_dac::receiver`) - `pi-receiver` just
implements the callback that receives points, instead of having to
hand-roll a DAC-side network protocol from scratch. `pc-client` still has
the `ether-dream` feature enabled too, in case you ever add a real
commercial Ether Dream DAC to the mix.

## Hardware you need for `pi-receiver`

- A Raspberry Pi 3 (any variant with the 40-pin header), with Raspberry Pi
  OS and its Ethernet port on the same network as your PC.
- **Three MCP4922** dual-channel, 12-bit SPI DAC chips (~3-5€ each, widely
  available as breakout boards). Six analog channels total: X, Y, R, G, B,
  and one spare.
- Jumper wires, breadboard or perfboard, and eventually a DB25 connector to
  wire the DAC outputs into the laser's ILDA input pin-for-pin (X, Y, R, G,
  B, GND - see your laser's ILDA pinout, not the RJ45 wiring table).

### Wiring

| DAC | Bus / Chip Select | Channel A | Channel B |
|-----|--------------------|-----------|-----------|
| DAC1 | SPI0, CE0 (GPIO8) | X | Y |
| DAC2 | SPI0, CE1 (GPIO7) | R | G |
| DAC3 | SPI1, CE0 (GPIO18) | B | unused |

SPI0 is enabled by default on Raspberry Pi OS (`sudo raspi-config` ->
Interface Options -> SPI, if not). SPI1 needs one line added to
`/boot/firmware/config.txt`, then a reboot:

```
dtoverlay=spi1-1cs
```

Each MCP4922's `VDD`/`VREF` go to the Pi's 3.3V or 5V rail (check your DAC
breakout board's specs for which reference voltage it needs, and whether
that matches the voltage range your laser's ILDA input expects), `VSS` to
ground, `SCK`/`SDI`/`CS` to the matching SPI pins, and `VOUTA`/`VOUTB` to
the corresponding ILDA signal lines.

**Known limitation:** each DAC channel update is a separate SPI
transaction, so X and Y (and R/G, and B) update a few microseconds apart
rather than perfectly simultaneously. This is usually invisible at moderate
point rates; a shared hardware LDAC line across all three chips would
remove it entirely if it turns out to matter.

**Point-rate caveat:** Raspberry Pi OS is not a real-time OS, and each
laser point costs 5 blocking SPI syscalls here. Expect this to comfortably
handle test patterns and simple content, but not necessarily the full
20-30k points/second of a complex ILDA show without visible jitter. If that
turns out to be a problem in practice, the fix is offloading the
time-critical DAC clocking to a microcontroller (e.g. a Raspberry Pi Pico
using its PIO peripheral) that the Pi feeds over UART - not implemented
here.

## Safety

You're building a hobby laser DAC. Before pointing it at anything:
- Start with `pc-client pattern cross` at a low `--pps` (e.g. 2000) and
  verify X/Y orientation and center point are correct.
- Make sure your laser's safety interlock / scanner-fail protection is
  functional, so a frozen or garbage point stream doesn't leave the beam
  stuck on and bright at one spot.
- Never point a laser at eyes; keep the beam below eye level or in a
  controlled/enclosed setup while testing.

## Building and running (via Docker - no local Rust install needed on the PC/Mac)

```sh
# Build both crates
docker run --rm -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm cargo build --workspace --release

# pc-client: discover DACs on the network (needs host networking for UDP)
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser discover

# pc-client: test pattern
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser pattern cross

# pc-client: play an ILDA file
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser play ./show.ild

# pc-client: DMX over Art-Net (if your laser/gateway speaks it)
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm \
  ./target/release/ilda-laser dmx 192.168.1.50 --channels 255,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0
```

`--network host` is required for anything that talks to the network (IDN
discovery/streaming, Art-Net) because Docker Desktop's default bridge
network does not forward UDP broadcast/multicast to your LAN.

### `pi-receiver`: build and run directly on the Raspberry Pi

Cross-compiling `rppal` (it needs Linux `/dev/spidev` access) is more
hassle than it's worth for a one-off. Simplest path: install Rust directly
on the Pi and build there.

```sh
# On the Pi:
curl https://sh.rustup.rs -sSf | sh -s -- -y
source "$HOME/.cargo/env"

git clone <this repo's URL> ilda-laser
cd ilda-laser
cargo build --release -p pi-receiver

# Enable SPI0 (raspi-config -> Interface Options -> SPI) and add
# `dtoverlay=spi1-1cs` to /boot/firmware/config.txt, then reboot.

sudo ./target/release/pi-receiver pi-laser
# (sudo, or add your user to the `spi`/`gpio` groups, so it can open
# /dev/spidev*)
```

It prints the IDN service name and address it's listening on. From the PC:

```sh
./target/release/ilda-laser discover      # should list idn:pi-laser
./target/release/ilda-laser pattern cross --device idn:pi-laser
```

## Usage (`pc-client`)

```
ilda-laser discover
ilda-laser play <file.ild> [--device <id>] [--pps 30000] [--fps 25]
ilda-laser pattern <circle|square|triangle|cross> [--device <id>] [--pps 30000] [--scale 0.8] [--color 255,0,0]
ilda-laser dmx <ip[:port]> --universe 0 --channels 255,0,0,... [--repeat-ms 1000]
```

Run with `RUST_LOG=debug` for verbose logging from the underlying
`laser-dac` crate when diagnosing a connection problem, on either side.

## Coordinate system

Points use normalized coordinates: X -1.0 (left) to 1.0 (right), Y -1.0
(bottom) to 1.0 (top). Colors are 0-65535 in `pc-client`'s internal
representation (0.0-1.0 once received by `pi-receiver`); the CLI's
`--color` flag takes 0-255 per channel and scales it up.

## Status / testing

Everything here has been built, unit-tested (`cargo test --workspace`) and
clippy-clean without physical hardware - the coordinate/color/DAC-word
conversion math and DMX packet encoding are covered by tests, but the
actual IDN network exchange and SPI/DAC output have not been verified
against a real Pi + DAC chips + laser. Go slowly, verify wiring and
orientation with `pattern cross` at a low point rate first, and check the
DAC's analog output with a multimeter/scope before trusting it near a real
laser diode driver.
