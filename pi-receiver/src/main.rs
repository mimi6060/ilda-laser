//! Runs on the Raspberry Pi. Advertises itself as an IDN (ILDA Digital
//! Network) laser receiver on the network, and for every point it receives,
//! writes the X/Y/R/G/B values out to three MCP4922 SPI DACs whose analog
//! outputs feed the laser's ILDA DB25 input.
//!
//! See ../../README.md for the wiring diagram and why this needs external
//! DAC chips rather than driving the ILDA input straight from GPIO.

mod mcp4922;

use anyhow::{Context, Result};
use laser_dac::receiver::{IdnServer, ReceivedPoint, ServerBehavior, ServerConfig, Service};
use log::{info, warn};
use mcp4922::{normalized_to_12bit, Channel, Mcp4922};
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;

/// Conservative SPI clock: the MCP4922 supports up to 20 MHz, but breadboard
/// jumper wiring is prone to ringing/reflections at high speed. Raise this
/// once the wiring is solid and you've checked the waveform on a scope.
const SPI_CLOCK_HZ: u32 = 1_000_000;

fn log_dac_write(result: Result<()>, label: &str) {
    if let Err(e) = result {
        warn!("DAC write failed ({label}): {e}");
    }
}

struct LaserOutput {
    xy: Mcp4922,
    rg: Mcp4922,
    b: Mcp4922,
    points_written: u64,
}

impl ServerBehavior for LaserOutput {
    fn should_respond(&self, _command: u8) -> bool {
        true
    }

    fn get_status_byte(&self) -> u8 {
        laser_dac::receiver::IDNFLG_STATUS_REALTIME
    }

    fn get_ack_result_code(&self) -> u8 {
        0x00
    }

    fn on_points_received(&mut self, points: &[ReceivedPoint]) {
        for p in points {
            let x = normalized_to_12bit(p.x, -1.0, 1.0);
            let y = normalized_to_12bit(p.y, -1.0, 1.0);
            let r = normalized_to_12bit(p.r, 0.0, 1.0);
            let g = normalized_to_12bit(p.g, 0.0, 1.0);
            let b = normalized_to_12bit(p.b, 0.0, 1.0);

            // Each write is its own SPI transaction (see mcp4922.rs). If a
            // write fails mid-point we still attempt the rest, rather than
            // aborting the frame over one bad transaction.
            log_dac_write(self.xy.write(Channel::A, x), "xy/x");
            log_dac_write(self.xy.write(Channel::B, y), "xy/y");
            log_dac_write(self.rg.write(Channel::A, r), "rg/r");
            log_dac_write(self.rg.write(Channel::B, g), "rg/g");
            log_dac_write(self.b.write(Channel::A, b), "b/b");

            self.points_written += 1;
        }

        if self.points_written % 50_000 < points.len() as u64 {
            info!("{} points written so far", self.points_written);
        }
    }

    fn on_client_connected(&mut self, addr: SocketAddr) {
        info!("client connected: {addr}");
    }

    fn on_client_disconnected(&mut self) {
        info!("client disconnected");
    }
}

fn open_dac(bus: Bus, select: SlaveSelect, context: &'static str) -> Result<Mcp4922> {
    let spi = Spi::new(bus, select, SPI_CLOCK_HZ, Mode::Mode0).with_context(|| context)?;
    Ok(Mcp4922::new(spi))
}

fn main() -> Result<()> {
    env_logger::init();

    let hostname = std::env::args().nth(1).unwrap_or_else(|| "pi-laser".to_string());

    // Wiring (see README.md):
    //   SPI0 CE0 -> DAC1: channel A = X, channel B = Y
    //   SPI0 CE1 -> DAC2: channel A = R, channel B = G
    //   SPI1 CE0 -> DAC3: channel A = B, channel B = unused
    // SPI0 is enabled by default on Raspberry Pi OS. SPI1 needs
    // `dtoverlay=spi1-1cs` added to /boot/firmware/config.txt (reboot after).
    let xy = open_dac(
        Bus::Spi0,
        SlaveSelect::Ss0,
        "failed to open SPI0 CE0 for the X/Y DAC - is SPI enabled? (sudo raspi-config)",
    )?;
    let rg = open_dac(
        Bus::Spi0,
        SlaveSelect::Ss1,
        "failed to open SPI0 CE1 for the R/G DAC",
    )?;
    let b = open_dac(
        Bus::Spi1,
        SlaveSelect::Ss0,
        "failed to open SPI1 CE0 for the B DAC - add 'dtoverlay=spi1-1cs' to \
         /boot/firmware/config.txt and reboot",
    )?;

    let behavior = LaserOutput {
        xy,
        rg,
        b,
        points_written: 0,
    };

    let config = ServerConfig::new_on_standard_port(&hostname)
        .with_services(vec![Service::laser_projector(1, "Pi Laser").with_dsid()]);

    let server = IdnServer::new(config, behavior).context("failed to bind IDN UDP server")?;
    let running = server.running_handle();

    println!(
        "IDN receiver '{hostname}' listening on {} - Ctrl+C to stop",
        server.addr()
    );
    println!("On the PC, run: ilda-laser discover   (should list idn:{hostname})");

    ctrlc::set_handler(move || {
        running.store(false, Ordering::SeqCst);
    })
    .context("failed to install Ctrl+C handler")?;

    server.run();
    Ok(())
}
