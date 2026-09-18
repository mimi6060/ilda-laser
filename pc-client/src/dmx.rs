//! Sends DMX-512 channel values to the laser over the network using the
//! Art-Net protocol (UDP port 6454).
//!
//! This is *not* the same thing as the DB25-to-RJ45 wiring table found in
//! some laser manuals, which just carries the analog ILDA signal over a
//! network cable's wire pairs. Art-Net is a real digital protocol; it only
//! works if something on the network actually speaks it — either the laser
//! itself (if its manual advertises an Art-Net/network DMX input) or a
//! separate Art-Net-to-DMX gateway box wired to the laser's DMX/XLR input.
//! Check your device's manual before relying on this.

use anyhow::{Context, Result};
use artnet_protocol::{ArtCommand, Output, PortAddress};
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// One Art-Net "Output" (ArtDmx) packet: the full channel value list for a
/// given universe.
pub struct DmxFrame {
    pub universe: PortAddress,
    pub channels: Vec<u8>,
}

impl DmxFrame {
    pub fn new(universe: u16, channels: Vec<u8>) -> Result<Self> {
        let universe: PortAddress = universe
            .try_into()
            .map_err(|_| anyhow::anyhow!("universe {universe} is out of range (0-32767)"))?;
        Ok(Self { universe, channels })
    }

    fn to_bytes(&self) -> Result<Vec<u8>> {
        let command = ArtCommand::Output(Output {
            port_address: self.universe,
            data: self.channels.clone().into(),
            ..Output::default()
        });
        command
            .write_to_buffer()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("failed to encode Art-Net packet")
    }
}

/// Sends a single DMX frame to `target` (e.g. "192.168.1.50:6454" or just
/// "192.168.1.50", in which case the standard Art-Net port 6454 is used).
pub fn send_once(target: &str, frame: &DmxFrame) -> Result<()> {
    let addr = resolve_target(target)?;
    let socket = UdpSocket::bind(("0.0.0.0", 0)).context("failed to bind UDP socket")?;
    socket.set_broadcast(true).ok();
    let bytes = frame.to_bytes()?;
    socket
        .send_to(&bytes, addr)
        .with_context(|| format!("failed to send Art-Net packet to {addr}"))?;
    Ok(())
}

/// Sends the same DMX frame repeatedly (Art-Net nodes and fixtures
/// typically expect a refresh at least once every few seconds, otherwise
/// some implementations revert to a fallback state) until interrupted.
pub fn send_repeating(target: &str, frame: &DmxFrame, interval: Duration) -> Result<()> {
    let addr = resolve_target(target)?;
    let socket = UdpSocket::bind(("0.0.0.0", 0)).context("failed to bind UDP socket")?;
    socket.set_broadcast(true).ok();
    let bytes = frame.to_bytes()?;
    loop {
        socket
            .send_to(&bytes, addr)
            .with_context(|| format!("failed to send Art-Net packet to {addr}"))?;
        std::thread::sleep(interval);
    }
}

fn resolve_target(target: &str) -> Result<std::net::SocketAddr> {
    let with_port = if target.contains(':') {
        target.to_string()
    } else {
        format!("{target}:6454")
    };
    with_port
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve target '{target}'"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no address found for '{target}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encodes_without_error() {
        let frame = DmxFrame::new(0, vec![255, 0, 128, 64]).unwrap();
        let bytes = frame.to_bytes().unwrap();
        // Art-Net header starts with "Art-Net\0".
        assert_eq!(&bytes[0..8], b"Art-Net\0");
    }

    #[test]
    fn out_of_range_universe_is_rejected() {
        assert!(DmxFrame::new(40_000, vec![0]).is_err());
    }

    #[test]
    fn resolve_target_defaults_to_artnet_port() {
        let addr = resolve_target("127.0.0.1").unwrap();
        assert_eq!(addr.port(), 6454);
    }

    #[test]
    fn resolve_target_keeps_explicit_port() {
        let addr = resolve_target("127.0.0.1:1234").unwrap();
        assert_eq!(addr.port(), 1234);
    }
}
