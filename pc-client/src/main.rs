use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use laser_dac::{list_devices, open_device, Frame, FrameSessionConfig, StreamControl};
use std::time::Duration;

mod dmx;
mod ilda_player;
mod patterns;

/// Control a laser projector from this PC: either an Ether Dream-compatible
/// network DAC (ILDA output), or a DMX fixture reachable over Art-Net.
#[derive(Parser)]
#[command(name = "ilda-laser", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List Ether Dream DACs found on the network.
    Discover,
    /// Play an ILDA (.ild) file on a DAC, looping until Ctrl+C.
    Play {
        file: String,
        /// DAC id from `discover`; defaults to the first one found.
        #[arg(long)]
        device: Option<String>,
        /// Points per second sent to the DAC.
        #[arg(long, default_value_t = 30_000)]
        pps: u32,
        /// Frames per second when the file has more than one frame.
        #[arg(long, default_value_t = 25.0)]
        fps: f32,
    },
    /// Stream a built-in calibration/test shape, looping until Ctrl+C.
    Pattern {
        #[arg(value_enum)]
        shape: Shape,
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 30_000)]
        pps: u32,
        /// Half-extent of the shape, 0.0-1.0.
        #[arg(long, default_value_t = 0.8)]
        scale: f32,
        /// "r,g,b" with each component 0-255.
        #[arg(long, default_value = "255,255,255")]
        color: String,
    },
    /// Send DMX-512 channel values over Art-Net.
    ///
    /// This needs either a laser that natively speaks Art-Net, or a separate
    /// Art-Net-to-DMX gateway wired to the laser's DMX/XLR input - see
    /// README.md. It is unrelated to the DB25-to-RJ45 analog ILDA wiring
    /// some laser manuals describe.
    Dmx {
        /// Target IP, e.g. 192.168.1.50 or 192.168.1.50:6454.
        target: String,
        #[arg(long, default_value_t = 0)]
        universe: u16,
        /// Comma-separated DMX channel values (0-255), channel 1 first.
        #[arg(long, value_delimiter = ',')]
        channels: Vec<u8>,
        /// Keep re-sending the same frame every N milliseconds until Ctrl+C.
        #[arg(long)]
        repeat_ms: Option<u64>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum Shape {
    Circle,
    Square,
    Triangle,
    Cross,
}

fn parse_color(s: &str) -> Result<(u16, u16, u16)> {
    let parts: Vec<_> = s.split(',').collect();
    anyhow::ensure!(
        parts.len() == 3,
        "color must be 'r,g,b' with values 0-255, got '{s}'"
    );
    let mut vals = [0u16; 3];
    for (i, p) in parts.iter().enumerate() {
        let v: u16 = p
            .trim()
            .parse()
            .with_context(|| format!("invalid color component '{p}'"))?;
        anyhow::ensure!(v <= 255, "color component {v} out of range 0-255");
        vals[i] = v * 257;
    }
    Ok((vals[0], vals[1], vals[2]))
}

fn pick_device(explicit: Option<String>) -> Result<String> {
    if let Some(id) = explicit {
        return Ok(id);
    }
    let devices = list_devices()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("failed to scan for DACs")?;
    let first = devices.first().ok_or_else(|| {
        anyhow::anyhow!("no DAC found on the network - is it powered on and reachable?")
    })?;
    println!(
        "Using device: {} ({}, id={})",
        first.name, first.kind, first.id
    );
    Ok(first.id.clone())
}

fn install_ctrlc_stop(control: StreamControl) {
    let _ = ctrlc::set_handler(move || {
        let _ = control.stop();
    });
}

fn open_session(
    device: Option<String>,
    pps: u32,
) -> Result<(laser_dac::FrameSession, laser_dac::DacInfo)> {
    let device_id = pick_device(device)?;
    let dac = open_device(&device_id)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("failed to open device")?;
    let config = FrameSessionConfig::new(pps);
    let (session, info) = dac
        .start_frame_session(config)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("failed to start frame session")?;
    Ok((session, info))
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Discover => {
            let devices = list_devices()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context("failed to scan for DACs")?;
            if devices.is_empty() {
                println!(
                    "No DACs found. Make sure the Ether Dream device is powered on and on \
                     the same network/subnet as this PC."
                );
            } else {
                for d in devices {
                    println!("{}  {}  ({})", d.id, d.name, d.kind);
                }
            }
        }

        Command::Play {
            file,
            device,
            pps,
            fps,
        } => {
            let frames = ilda_player::load_frames(&file)?;
            anyhow::ensure!(!frames.is_empty(), "'{file}' contains no frames");
            println!("Loaded {} frame(s) from {file}", frames.len());

            let (session, info) = open_session(device, pps)?;
            println!("Streaming to {} - Ctrl+C to stop", info.name);

            session.control().arm().map_err(|e| anyhow::anyhow!("{e}"))?;
            install_ctrlc_stop(session.control());

            let frame_duration = Duration::from_secs_f32(1.0 / fps.max(1.0));
            'outer: loop {
                for pts in &frames {
                    session.send_frame(Frame::new(pts.clone()));
                    std::thread::sleep(frame_duration);
                    if session.control().is_stop_requested() {
                        break 'outer;
                    }
                }
            }
            session.join().map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        Command::Pattern {
            shape,
            device,
            pps,
            scale,
            color,
        } => {
            let (r, g, b) = parse_color(&color)?;
            let points = match shape {
                Shape::Circle => patterns::circle(scale, 120, r, g, b),
                Shape::Square => patterns::square(scale, r, g, b),
                Shape::Triangle => patterns::triangle(scale, r, g, b),
                Shape::Cross => patterns::cross(scale, r, g, b),
            };

            let (session, info) = open_session(device, pps)?;
            println!("Streaming to {} - Ctrl+C to stop", info.name);

            session.control().arm().map_err(|e| anyhow::anyhow!("{e}"))?;
            install_ctrlc_stop(session.control());
            session.send_frame(Frame::new(points));

            while !session.control().is_stop_requested() {
                std::thread::sleep(Duration::from_millis(200));
            }
            session.join().map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        Command::Dmx {
            target,
            universe,
            channels,
            repeat_ms,
        } => {
            anyhow::ensure!(!channels.is_empty(), "provide at least one --channels value");
            let frame = dmx::DmxFrame::new(universe, channels)?;
            match repeat_ms {
                Some(ms) => {
                    println!("Sending DMX to {target} every {ms}ms - Ctrl+C to stop");
                    dmx::send_repeating(&target, &frame, Duration::from_millis(ms))?;
                }
                None => {
                    dmx::send_once(&target, &frame)?;
                    println!("Sent one DMX frame to {target}");
                }
            }
        }
    }

    Ok(())
}
