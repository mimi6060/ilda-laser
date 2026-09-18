# ilda-laser

A small Rust CLI to control a laser projector from a PC, the way Pangolin's
FB3/FB4 does internally: stream ILDA points to a network laser DAC, or send
DMX-512 over Art-Net.

## Why you need a DAC box, not just an RJ45 cable

A PC's network card can only send digital Ethernet frames. It cannot put
analog galvo-control voltages onto a wire by itself. The laser's DB25 ILDA
input expects real analog X/Y/R/G/B signals.

Some laser manuals include a "25 pin interface to Ethernet port" wiring
table (DB25 pin -> RJ45 pin, e.g. `X- -> Pin14`, `R -> Pin5`, ...). That is
**not** a network protocol - it is a trick to carry the same analog ILDA
signals over an Ethernet-style cable's wire pairs, purely to reach a longer
distance than a bulky ILDA cable allows. Plugging that cable straight into a
PC's Ethernet port does nothing useful; a PC cannot generate analog voltages
on those pins.

To actually control the laser from software you need a laser DAC (a small
box with a real digital-to-analog converter) sitting between the PC and the
laser's DB25 ILDA input:

```
PC --(Ethernet, digital)--> Laser DAC box --(DB25 ILDA cable, analog)--> Laser
```

This is exactly what a Pangolin FB4 (or FB3, QM2000, etc.) is. This tool
targets the open **Ether Dream** protocol, which many DIY and commercial DAC
boards implement (it's UDP-based, so it runs over a plain network cable /
switch between the PC and the DAC box - no special driver needed).

If your laser also supports DMX-512 (per its channel tables, e.g. the
18-channel / 22-channel modes), you can alternatively control it with the
`dmx` subcommand over **Art-Net**, but only if there's an Art-Net-capable
node in the chain: either the laser has a native Art-Net/network DMX input,
or you use a separate Art-Net-to-DMX gateway wired into its DMX/XLR input.
Art-Net does travel over a plain Ethernet/RJ45 cable and a PC NIC, unlike
the analog ILDA trick above - but it's a different signal path from the ILDA
DB25 input, and controls different things (the DMX channel functions from
the manual: color, patterns, effects, etc., not raw vector graphics).

## What this tool does

- `discover` - scans the network for Ether Dream DACs.
- `play <file.ild>` - loads a standard ILDA (`.ild`) file and streams it.
- `pattern <circle|square|triangle|cross>` - streams a built-in shape, handy
  to check wiring/orientation before you have real content.
- `dmx <ip> --channels ...` - sends DMX-512 values over Art-Net.

## Building and running (via Docker - no local Rust install needed)

```sh
# Build
docker run --rm -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm cargo build --release

# Discover DACs on the network (needs host networking to see UDP broadcasts)
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser discover

# Play an ILDA file
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser play ./show.ild

# Test pattern (calibration cross)
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm ./target/release/ilda-laser pattern cross

# DMX over Art-Net (single laser fixture at 192.168.1.50, universe 0)
docker run --rm --network host -v "$PWD":/app -w /app -v ilda-laser-cargo:/usr/local/cargo/registry \
  -v ilda-laser-target:/app/target rust:1-bookworm \
  ./target/release/ilda-laser dmx 192.168.1.50 --channels 255,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0
```

`--network host` is required for `discover` and `dmx` (and any command that
talks to the DAC) because Docker Desktop's default bridge network does not
forward UDP broadcast/multicast from the container to your LAN.

If you'd rather install Rust natively (`brew install rustup-init &&
rustup-init`), the same commands work directly as `cargo build --release`
and `./target/release/ilda-laser ...`.

## Usage

```
ilda-laser discover
ilda-laser play <file.ild> [--device <id>] [--pps 30000] [--fps 25]
ilda-laser pattern <circle|square|triangle|cross> [--device <id>] [--pps 30000] [--scale 0.8] [--color 255,0,0]
ilda-laser dmx <ip[:port]> --universe 0 --channels 255,0,0,... [--repeat-ms 1000]
```

Run with `RUST_LOG=debug` for verbose logging from the underlying
`laser-dac` crate when diagnosing a connection problem.

## Coordinate system

Points use normalized coordinates: X -1.0 (left) to 1.0 (right), Y -1.0
(bottom) to 1.0 (top). Colors are 0-65535; the CLI's `--color` flag takes
0-255 per channel and scales it up.

## Status / testing

This has been built and unit-tested (`cargo test`) without physical laser
hardware - the coordinate/color conversion math and DMX packet encoding are
covered by tests, but the actual Ether Dream network exchange has not been
verified against a real DAC. Start with `pattern cross` at a low `--pps` and
make sure your laser's safety interlock / scanner-fail protection is in
place before pointing it at anything you care about.
