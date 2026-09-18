//! Runs on the Raspberry Pi. Two ways in, one shared output:
//!
//! - An IDN (ILDA Digital Network) receiver on the network, for real
//!   content streamed from `pc-client` or other laser show software.
//! - A tiny local web control panel (see `web.rs`), for picking a
//!   pattern/color/speed from a phone or PC browser on the same WiFi,
//!   without needing `pc-client` at all.
//!
//! Both funnel through the same `DacSink`, which writes X/Y/R/G/B out to
//! three MCP4922 SPI DACs feeding the laser's ILDA DB25 input.
//!
//! See ../../README.md for the wiring diagram and why this needs external
//! DAC chips rather than driving the ILDA input straight from GPIO.

mod dac_sink;
mod mcp4922;
mod patterns;
mod web;

use anyhow::{Context, Result};
use dac_sink::DacSink;
use laser_dac::receiver::{IdnServer, ReceivedPoint, ServerBehavior, ServerConfig, Service};
use log::info;
use mcp4922::Mcp4922;
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Conservative SPI clock: the MCP4922 supports up to 20 MHz, but breadboard
/// jumper wiring is prone to ringing/reflections at high speed. Raise this
/// once the wiring is solid and you've checked the waveform on a scope.
const SPI_CLOCK_HZ: u32 = 1_000_000;

/// Bridges the IDN server's point callback to the shared `DacSink`.
struct IdnBridge {
    dac: Arc<Mutex<DacSink>>,
    points_written: u64,
}

impl ServerBehavior for IdnBridge {
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
        let mut dac = self.dac.lock().unwrap();
        for p in points {
            dac.write_point(p.x, p.y, p.r, p.g, p.b);
        }
        drop(dac);

        self.points_written += points.len() as u64;
        if self.points_written % 50_000 < points.len() as u64 {
            info!("{} points written so far (IDN)", self.points_written);
        }
    }

    fn on_client_connected(&mut self, addr: SocketAddr) {
        info!("IDN client connected: {addr}");
    }

    fn on_client_disconnected(&mut self) {
        info!("IDN client disconnected");
    }
}

fn open_dac(bus: Bus, select: SlaveSelect, context: &'static str) -> Result<Mcp4922> {
    let spi = Spi::new(bus, select, SPI_CLOCK_HZ, Mode::Mode0).with_context(|| context)?;
    Ok(Mcp4922::new(spi))
}

fn main() -> Result<()> {
    env_logger::init();

    let hostname = std::env::args().nth(1).unwrap_or_else(|| "pi-laser".to_string());

    // Wiring (see README.md / hardware/DESIGN.md):
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

    let dac = Arc::new(Mutex::new(DacSink::new(xy, rg, b)));

    let config = ServerConfig::new_on_standard_port(&hostname)
        .with_services(vec![Service::laser_projector(1, "Pi Laser").with_dsid()]);

    let idn_behavior = IdnBridge { dac: Arc::clone(&dac), points_written: 0 };
    let idn_server = IdnServer::new(config, idn_behavior).context("failed to bind IDN UDP server")?;
    let running = idn_server.running_handle();
    let idn_addr = idn_server.addr();
    let idn_handle = idn_server.spawn();

    ctrlc::set_handler({
        let running = Arc::clone(&running);
        move || running.store(false, Ordering::SeqCst)
    })
    .context("failed to install Ctrl+C handler")?;

    println!("IDN receiver '{hostname}' listening on {idn_addr}");
    println!("On the PC, run: ilda-laser discover   (should list idn:{hostname})");

    let web_addr = "0.0.0.0:8080";
    println!("Web control panel: http://<this Pi's IP>:8080/  - Ctrl+C to stop everything");

    web::run(dac, web_addr, running)?;

    drop(idn_handle); // stops and joins the IDN server thread
    Ok(())
}
