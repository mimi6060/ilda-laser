//! OSC 1.0 over UDP: a small hand-written decoder (messages and bundles)
//! and our own address space, `/ls/…`, onto the stable control ids.
//!
//! Addresses (all under `/ls`):
//! - `/ls/control/<id> [v]`: a control in native units (`master/brightness`
//!   and `master.brightness` are the same). No argument = 1 (a press).
//! - `/ls/norm/<id> v`: the same control, `v` in 0..1.
//! - `/ls/cue/play ,s <cue id>`: plays a cue from the start.
//! - `/ls/cue/stop`: stops every cue.
//! - `/ls/blackout`: blackout (latches the emergency stop).
//! - `/ls/arm`: always refused (and logged). Nothing external arms.
//! - `/ls/ping`: does nothing; shows as activity.
//! - `/ls/access 1|0`: raises or lowers this client's access (T-408); it
//!   drops by itself 10 s after the client's last message.
//! - `/ls/subscribe <port>`, `/ls/unsubscribe`: OSC feedback (T-416,
//!   feedback.rs) to this client's address and that port.
//! - `/ls/<id with / for .> [v]` (T-416): a control in native units, the
//!   address `feedback::address_of` lists (`/ls/master/size`,
//!   `/ls/tempo/bpm 128`, `/ls/tempo/resync`). The words above come first;
//!   no control id starts with one of them (checked by a test).
//!
//! Bundle time tags are ignored: everything runs on arrival.

use crate::controls::ControlInput;
use serde::Serialize;

/// Longest packet we read (a UDP datagram on a LAN is far smaller).
pub const MAX_PACKET: usize = 8192;
/// Nested bundles deeper than this are refused.
const MAX_DEPTH: usize = 4;
/// Messages taken from one packet at most.
pub const MAX_MESSAGES: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OscArg {
    Int(i32),
    Float(f32),
    Double(f64),
    Long(i64),
    Str(String),
    Bool(bool),
    /// Nil, impulse, blob, time tag…: no value we use (`null`).
    Other,
}

impl OscArg {
    fn number(&self) -> Option<f32> {
        match *self {
            OscArg::Int(i) => Some(i as f32),
            OscArg::Float(f) if f.is_finite() => Some(f),
            OscArg::Double(d) if d.is_finite() => Some(d as f32),
            OscArg::Long(l) => Some(l as f32),
            OscArg::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscMessage {
    pub address: String,
    pub args: Vec<OscArg>,
}

/// Decodes one UDP packet: a message, or a bundle of them (flattened).
pub fn decode(packet: &[u8]) -> Result<Vec<OscMessage>, &'static str> {
    let mut out = Vec::new();
    decode_into(packet, 0, &mut out)?;
    Ok(out)
}

fn decode_into(p: &[u8], depth: usize, out: &mut Vec<OscMessage>) -> Result<(), &'static str> {
    if p.starts_with(b"#bundle\0") {
        if depth >= MAX_DEPTH {
            return Err("paquets OSC imbriqués trop profondément");
        }
        let mut r = Reader { p, at: 16 }; // "#bundle\0" + 8-byte time tag
        if p.len() < 16 {
            return Err("paquet OSC tronqué");
        }
        while r.at < p.len() {
            let size = r.i32()?;
            let size = usize::try_from(size).map_err(|_| "taille d'élément OSC négative")?;
            let elem = r.take(size)?;
            decode_into(elem, depth + 1, out)?;
        }
        return Ok(());
    }
    if out.len() >= MAX_MESSAGES {
        return Err("trop de messages OSC dans un paquet");
    }
    let mut r = Reader { p, at: 0 };
    let address = r.string()?;
    if !address.starts_with('/') {
        return Err("adresse OSC invalide");
    }
    // A message without a type tag string is allowed (old senders): no args.
    let mut args = Vec::new();
    if r.at < p.len() {
        let tags = r.string()?;
        let tags = tags.strip_prefix(',').ok_or("étiquettes de type OSC invalides")?;
        for t in tags.chars() {
            args.push(match t {
                'i' => OscArg::Int(r.i32()?),
                'f' => OscArg::Float(f32::from_bits(r.i32()? as u32)),
                'd' => OscArg::Double(f64::from_bits(r.i64()? as u64)),
                'h' => OscArg::Long(r.i64()?),
                's' | 'S' => OscArg::Str(r.string()?),
                'b' => {
                    let n = usize::try_from(r.i32()?).map_err(|_| "taille de blob OSC négative")?;
                    r.take(n.div_ceil(4) * 4)?;
                    OscArg::Other
                }
                't' => {
                    r.i64()?;
                    OscArg::Other
                }
                'c' | 'r' | 'm' => {
                    r.i32()?;
                    OscArg::Other
                }
                'T' => OscArg::Bool(true),
                'F' => OscArg::Bool(false),
                'N' | 'I' => OscArg::Other,
                _ => return Err("type d'argument OSC inconnu"),
            });
        }
    }
    out.push(OscMessage { address, args });
    Ok(())
}

struct Reader<'a> {
    p: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.p.len()).ok_or("paquet OSC tronqué")?;
        let s = &self.p[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn i32(&mut self) -> Result<i32, &'static str> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, &'static str> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// A NUL-terminated string padded to 4 bytes.
    fn string(&mut self) -> Result<String, &'static str> {
        let rest = &self.p[self.at.min(self.p.len())..];
        let len = rest.iter().position(|&b| b == 0).ok_or("chaîne OSC sans fin")?;
        let s = std::str::from_utf8(&rest[..len]).map_err(|_| "chaîne OSC invalide (UTF-8)")?.to_string();
        self.take((len + 1).div_ceil(4) * 4)?;
        Ok(s)
    }
}

/// What a message asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum OscCommand {
    Control { id: String, input: ControlInput },
    PlayCue(String),
    StopCues,
    Blackout,
    /// Always refused; kept so the refusal is logged.
    Arm,
    Ping,
    /// Raise (true) or lower this client's access (T-408).
    Access(bool),
    /// Feedback to the sender's address and this port (T-416).
    Subscribe(u16),
    Unsubscribe,
}

/// The first words of `/ls/…` that are commands, not control ids.
pub const RESERVED: [&str; 10] = ["control", "norm", "cue/play", "cue/stop", "blackout", "arm", "ping", "access", "subscribe", "unsubscribe"];

/// Our address space → a command. Errors are French, for the monitor.
pub fn command(msg: &OscMessage) -> Result<OscCommand, String> {
    let rest = msg.address.strip_prefix("/ls/").ok_or_else(|| format!("adresse hors de /ls : {}", msg.address))?;
    let first = msg.args.first();
    let number = || first.map_or(Some(1.0), OscArg::number);
    let (head, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let id = tail.replace('/', ".");
    match (head, tail) {
        ("control", t) if !t.is_empty() => {
            let v = number().ok_or("argument numérique attendu")?;
            Ok(control_or_safety(id, ControlInput::Value(v)))
        }
        ("norm", t) if !t.is_empty() => {
            let v = first.and_then(OscArg::number).ok_or("valeur 0..1 attendue")?;
            Ok(control_or_safety(id, ControlInput::Norm(v)))
        }
        ("cue", "play") => match first {
            Some(OscArg::Str(cue)) if !cue.is_empty() => Ok(OscCommand::PlayCue(cue.clone())),
            _ => Err("identifiant de cue attendu (,s)".into()),
        },
        ("cue", "stop") => Ok(OscCommand::StopCues),
        ("blackout", "") => Ok(OscCommand::Blackout),
        ("arm", "") => Ok(OscCommand::Arm),
        ("ping", "") => Ok(OscCommand::Ping),
        ("access", "") => {
            let v = number().ok_or("argument 0 ou 1 attendu")?;
            Ok(OscCommand::Access(v >= 0.5))
        }
        ("subscribe", "") => {
            let port = first.and_then(OscArg::number).filter(|p| p.fract() == 0.0 && (1.0..=65535.0).contains(p));
            Ok(OscCommand::Subscribe(port.ok_or("port de retour attendu (1 à 65535)")? as u16))
        }
        ("unsubscribe", "") => Ok(OscCommand::Unsubscribe),
        // T-416: `/ls/master/size v`, the address listed for the control.
        _ if !rest.ends_with('/') && !rest.contains("//") && !RESERVED.iter().any(|r| rest == *r || rest.starts_with(&format!("{r}/"))) => {
            let id = super::feedback::control_of(&msg.address).ok_or_else(|| format!("adresse inconnue : {}", msg.address))?;
            let v = number().ok_or("argument numérique attendu")?;
            Ok(control_or_safety(id, ControlInput::Value(v)))
        }
        _ => Err(format!("adresse inconnue : {}", msg.address)),
    }
}

/// Blackout and arming have their own paths whatever the address used. A
/// blackout pad's release (0) does nothing.
fn control_or_safety(id: String, input: ControlInput) -> OscCommand {
    let pressed = matches!(input, ControlInput::Value(v) | ControlInput::Norm(v) if v >= 0.5);
    match id.as_str() {
        "transport.blackout" | "safety.estop" if pressed => OscCommand::Blackout,
        "transport.blackout" | "safety.estop" => OscCommand::Ping,
        "transport.arm" => OscCommand::Arm,
        _ => OscCommand::Control { id, input },
    }
}

fn pad(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(s);
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// Encodes one message (feedback, T-416). Supports i, f, s, T, F; any
/// other argument goes out as nil.
pub fn encode(address: &str, args: &[OscArg]) -> Vec<u8> {
    let mut out = Vec::new();
    pad(&mut out, address.as_bytes());
    let mut tags = String::from(",");
    let mut data = Vec::new();
    for a in args {
        match a {
            OscArg::Int(i) => {
                tags.push('i');
                data.extend_from_slice(&i.to_be_bytes());
            }
            OscArg::Float(f) => {
                tags.push('f');
                data.extend_from_slice(&f.to_bits().to_be_bytes());
            }
            OscArg::Str(s) => {
                tags.push('s');
                pad(&mut data, s.as_bytes());
            }
            OscArg::Bool(b) => tags.push(if *b { 'T' } else { 'F' }),
            _ => tags.push('N'),
        }
    }
    pad(&mut out, tags.as_bytes());
    out.extend(data);
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Encodes a message (tests and the UDP round trip).
    pub fn encode(address: &str, args: &[OscArg]) -> Vec<u8> {
        super::encode(address, args)
    }

    pub fn bundle(elems: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"#bundle\0".to_vec();
        out.extend_from_slice(&1u64.to_be_bytes());
        for e in elems {
            out.extend_from_slice(&(e.len() as i32).to_be_bytes());
            out.extend_from_slice(e);
        }
        out
    }

    #[test]
    fn decodes_the_spec_example_message() {
        // OSC 1.0 spec: "/oscillator/4/frequency" ,f 440.0
        let mut p = b"/oscillator/4/frequency\0".to_vec();
        p.extend_from_slice(b",f\0\0");
        p.extend_from_slice(&[0x43, 0xdc, 0x00, 0x00]);
        assert_eq!(decode(&p).unwrap(), vec![OscMessage { address: "/oscillator/4/frequency".into(), args: vec![OscArg::Float(440.0)] }]);
    }

    #[test]
    fn round_trips_args_and_flattens_bundles() {
        let a = encode("/ls/control/master.brightness", &[OscArg::Float(0.5)]);
        let b = encode("/ls/cue/play", &[OscArg::Str("tunnels-001".into()), OscArg::Int(7), OscArg::Bool(true)]);
        let msgs = decode(&bundle(&[a, bundle(&[b])])).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].args, vec![OscArg::Float(0.5)]);
        assert_eq!(msgs[1].args, vec![OscArg::Str("tunnels-001".into()), OscArg::Int(7), OscArg::Bool(true)]);
        // No type tag at all: an old sender, no arguments.
        assert_eq!(decode(b"/ls/ping\0\0\0\0").unwrap()[0].args, vec![]);
    }

    #[test]
    fn garbage_never_panics_and_is_refused() {
        let good = encode("/ls/control/master.size", &[OscArg::Float(1.0), OscArg::Str("abc".into())]);
        for n in 0..good.len() {
            let _ = decode(&good[..n]); // every truncation: an error, never a panic
        }
        assert!(decode(&good[..good.len() - 4]).is_err());
        assert!(decode(b"").is_err());
        assert!(decode(b"no-slash\0\0\0\0").is_err());
        assert!(decode(b"/x\0\0,z\0\0").is_err(), "unknown type");
        let mut neg = b"#bundle\0".to_vec();
        neg.extend_from_slice(&[0; 8]);
        neg.extend_from_slice(&(-4i32).to_be_bytes());
        assert!(decode(&neg).is_err());
        let mut deep = encode("/ls/ping", &[]);
        for _ in 0..=MAX_DEPTH {
            deep = bundle(&[deep]);
        }
        assert!(decode(&deep).is_err(), "nesting is bounded");
        let many: Vec<Vec<u8>> = (0..=MAX_MESSAGES).map(|_| encode("/ls/ping", &[])).collect();
        assert!(decode(&bundle(&many)).is_err(), "messages per packet are bounded");
        // Random bytes.
        let mut x: u32 = 12345;
        for _ in 0..2000 {
            let p: Vec<u8> = (0..(x % 64) as usize)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (x >> 16) as u8
                })
                .collect();
            let _ = decode(&p);
            let mut b = b"#bundle\0".to_vec();
            b.extend(p);
            let _ = decode(&b);
        }
    }

    fn cmd(address: &str, args: &[OscArg]) -> Result<OscCommand, String> {
        command(&decode(&encode(address, args)).unwrap()[0])
    }

    #[test]
    fn addresses_map_onto_control_ids() {
        assert_eq!(cmd("/ls/control/master/brightness", &[OscArg::Float(0.25)]), Ok(OscCommand::Control { id: "master.brightness".into(), input: ControlInput::Value(0.25) }));
        assert_eq!(cmd("/ls/control/master.size", &[OscArg::Int(2)]), Ok(OscCommand::Control { id: "master.size".into(), input: ControlInput::Value(2.0) }));
        assert_eq!(cmd("/ls/control/tempo.tap", &[]), Ok(OscCommand::Control { id: "tempo.tap".into(), input: ControlInput::Value(1.0) }));
        assert_eq!(cmd("/ls/control/layer.1.mute", &[OscArg::Bool(false)]), Ok(OscCommand::Control { id: "layer.1.mute".into(), input: ControlInput::Value(0.0) }));
        assert_eq!(cmd("/ls/norm/master.speed", &[OscArg::Float(0.5)]), Ok(OscCommand::Control { id: "master.speed".into(), input: ControlInput::Norm(0.5) }));
        assert_eq!(cmd("/ls/cue/play", &[OscArg::Str("tunnels-001".into())]), Ok(OscCommand::PlayCue("tunnels-001".into())));
        assert_eq!(cmd("/ls/cue/stop", &[]), Ok(OscCommand::StopCues));
        assert_eq!(cmd("/ls/ping", &[]), Ok(OscCommand::Ping));
        assert_eq!(cmd("/ls/access", &[OscArg::Int(1)]), Ok(OscCommand::Access(true)));
        assert_eq!(cmd("/ls/access", &[OscArg::Float(0.0)]), Ok(OscCommand::Access(false)));
        assert_eq!(cmd("/ls/access", &[]), Ok(OscCommand::Access(true)), "no argument: a press");
        // T-416: the listed addresses, and feedback subscriptions.
        assert_eq!(cmd("/ls/master/size", &[OscArg::Float(1.5)]), Ok(OscCommand::Control { id: "master.size".into(), input: ControlInput::Value(1.5) }));
        assert_eq!(cmd("/ls/tempo/bpm", &[OscArg::Int(128)]), Ok(OscCommand::Control { id: "tempo.bpm".into(), input: ControlInput::Value(128.0) }));
        assert_eq!(cmd("/ls/tempo/resync", &[]), Ok(OscCommand::Control { id: "tempo.resync".into(), input: ControlInput::Value(1.0) }));
        assert_eq!(cmd("/ls/cue/mode", &[OscArg::Int(1)]), Ok(OscCommand::Control { id: "cue.mode".into(), input: ControlInput::Value(1.0) }));
        assert_eq!(cmd("/ls/unknown", &[]), Ok(OscCommand::Control { id: "unknown".into(), input: ControlInput::Value(1.0) }), "refused later: unknown control");
        assert_eq!(cmd("/ls/subscribe", &[OscArg::Int(9000)]), Ok(OscCommand::Subscribe(9000)));
        assert_eq!(cmd("/ls/unsubscribe", &[]), Ok(OscCommand::Unsubscribe));
    }

    #[test]
    fn blackout_and_arm_have_their_own_commands_whatever_the_address() {
        for a in ["/ls/blackout", "/ls/control/transport.blackout", "/ls/control/safety/estop"] {
            assert_eq!(cmd(a, &[]), Ok(OscCommand::Blackout), "{a}");
        }
        assert_eq!(cmd("/ls/control/transport.blackout", &[OscArg::Int(0)]), Ok(OscCommand::Ping), "a pad release is no blackout");
        assert_eq!(cmd("/ls/transport/blackout", &[]), Ok(OscCommand::Blackout));
        assert_eq!(cmd("/ls/safety/estop", &[]), Ok(OscCommand::Blackout));
        for a in ["/ls/arm", "/ls/control/transport.arm", "/ls/norm/transport/arm", "/ls/transport/arm"] {
            assert_eq!(cmd(a, &[OscArg::Float(1.0)]), Ok(OscCommand::Arm), "{a}");
        }
    }

    #[test]
    fn bad_addresses_and_args_are_explained() {
        assert!(cmd("/beyond/master/livecontrol/brightness", &[OscArg::Float(1.0)]).unwrap_err().contains("hors de /ls"));
        assert!(cmd("/ls/", &[]).is_err());
        assert!(cmd("/ls/control", &[]).is_err(), "a command word is never a control");
        assert!(cmd("/ls/cue/play/x", &[]).is_err());
        assert!(cmd("/ls/master//size", &[]).is_err());
        assert!(cmd("/ls/subscribe", &[]).is_err());
        assert!(cmd("/ls/subscribe", &[OscArg::Int(70000)]).is_err());
        assert!(cmd("/ls/master/size", &[OscArg::Str("x".into())]).is_err());
        assert!(cmd("/ls/control/", &[]).is_err());
        assert!(cmd("/ls/control/master.size", &[OscArg::Str("x".into())]).is_err());
        assert!(cmd("/ls/norm/master.size", &[]).is_err());
        assert!(cmd("/ls/cue/play", &[]).is_err());
        assert!(cmd("/ls/control/master.size", &[OscArg::Float(f32::NAN)]).is_err(), "NaN is not a value");
    }
}
