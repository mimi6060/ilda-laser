//! Controller detection: the universal Device Inquiry, port-name hints, and
//! the Akai APC40 / APC40 mkII SysEx we need to take them over (and give
//! them back). Byte layouts come from Akai's public communication protocol
//! documents (docs/research/midi-apc40.md §1.1, §2.1, §2.5). All pure.

use serde::{Deserialize, Serialize};

/// Universal Non-Realtime Device Inquiry, to every device (`7F`).
pub const DEVICE_INQUIRY: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];

pub const AKAI: u32 = 0x47;
pub const PID_APC40: u8 = 0x73;
pub const PID_APC40_MK2: u8 = 0x29;
pub const PID_APC_MINI: u8 = 0x28;

/// Introduction modes. The APC starts in Generic mode; 0x41 is what we use.
pub const MODE_GENERIC: u8 = 0x40;
pub const MODE_ABLETON: u8 = 0x41;
pub const MODE_ALTERNATE: u8 = 0x42;

/// Detected hardware model. Only the APC40s get a dedicated driver; an APC
/// mini is recognised but uses the generic profile for now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Model {
    Apc40,
    Apc40Mk2,
    ApcMini,
    #[default]
    Unknown,
}

impl Model {
    pub fn from_identity(id: &Identity) -> Model {
        if id.manufacturer != AKAI {
            return Model::Unknown;
        }
        match id.product {
            PID_APC40 => Model::Apc40,
            PID_APC40_MK2 => Model::Apc40Mk2,
            PID_APC_MINI => Model::ApcMini,
            _ => Model::Unknown,
        }
    }

    /// Hint from the CoreMIDI port name. Names vary with OS and firmware,
    /// so the Device Inquiry wins when the device answers it.
    pub fn from_port_name(name: &str) -> Model {
        let n = name.to_lowercase().replace(['_', '-'], " ");
        let compact = n.replace(' ', "");
        if compact.contains("apc40mkii") || compact.contains("apc40mk2") {
            Model::Apc40Mk2
        } else if compact.contains("apc40") {
            Model::Apc40
        } else if compact.contains("apcmini") {
            Model::ApcMini
        } else {
            Model::Unknown
        }
    }

    /// Akai product id used in the APC SysEx frame.
    pub fn product_id(self) -> Option<u8> {
        match self {
            Model::Apc40 => Some(PID_APC40),
            Model::Apc40Mk2 => Some(PID_APC40_MK2),
            Model::ApcMini => Some(PID_APC_MINI),
            Model::Unknown => None,
        }
    }

    /// French label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Model::Apc40 => "APC40",
            Model::Apc40Mk2 => "APC40 mkII",
            Model::ApcMini => "APC mini",
            Model::Unknown => "inconnu",
        }
    }
}

/// What a Device Inquiry reply tells us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// One-byte id (`0x47` = Akai), or `0x00_hh_ll` for three-byte ids.
    pub manufacturer: u32,
    /// First byte after the manufacturer id: Akai puts the product id there.
    pub product: u8,
}

/// Parses `F0 7E <ch> 06 02 <manufacturer> <product> … F7`. `None` for
/// anything else, including truncated replies.
pub fn parse_identity(bytes: &[u8]) -> Option<Identity> {
    if bytes.len() < 8 || bytes[0] != 0xF0 || bytes[1] != 0x7E || bytes[3] != 0x06 || bytes[4] != 0x02 || *bytes.last()? != 0xF7 {
        return None;
    }
    let (manufacturer, at) = if bytes[5] == 0x00 {
        (u32::from(*bytes.get(6)?) << 8 | u32::from(*bytes.get(7)?), 8)
    } else {
        (u32::from(bytes[5]), 6)
    };
    let product = *bytes.get(at)?;
    // The product byte must be a data byte, not the closing F7.
    if product >= 0x80 || at + 1 >= bytes.len() {
        return None;
    }
    Some(Identity { manufacturer, product })
}

/// Introduction message: `F0 47 7F <pid> 60 00 04 <mode> <verHi> <verLo> <bugfix> F7`.
/// Mode `0x41` makes every button momentary and hands the LEDs to us;
/// `0x40` gives the device back its own behaviour.
pub fn introduction(pid: u8, mode: u8) -> Vec<u8> {
    let (major, minor, bugfix) = (1, 0, 0);
    vec![0xF0, 0x47, 0x7F, pid, 0x60, 0x00, 0x04, mode & 0x7F, major, minor, bugfix, 0xF7]
}

/// The mkII answers the Introduction with message `0x61` carrying the
/// positions of its 9 faders (tracks 1–8, master): what pickup needs.
pub fn parse_fader_positions(bytes: &[u8]) -> Option<[u8; 9]> {
    if bytes.len() != 17 || bytes[0] != 0xF0 || bytes[1] != 0x47 || bytes[3] != PID_APC40_MK2 || bytes[4] != 0x61 || bytes[16] != 0xF7 {
        return None;
    }
    if (u16::from(bytes[5]) << 7 | u16::from(bytes[6])) != 9 {
        return None;
    }
    let mut faders = [0u8; 9];
    faders.copy_from_slice(&bytes[7..16]);
    faders.iter().all(|&v| v < 0x80).then_some(faders)
}

/// Note On velocity 0 for every host-driven LED, to leave the controller
/// dark when we quit. T-205 will reuse this on disconnect.
pub fn leds_off(model: Model) -> Vec<[u8; 3]> {
    let mut msgs = Vec::new();
    let mut note = |channel: u8, note: u8| msgs.push([0x90 | channel, note, 0]);
    let track_rows: &[u8] = match model {
        Model::Apc40 => &[0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39],
        Model::Apc40Mk2 => &[0x30, 0x31, 0x32, 0x33, 0x34, 0x42],
        _ => &[],
    };
    for &n in track_rows {
        for ch in 0..8 {
            note(ch, n);
        }
    }
    let singles: Vec<u8> = match model {
        Model::Apc40 => (0x3A..=0x41).chain([0x50]).chain(0x52..=0x56).chain(0x57..=0x5A).collect(),
        Model::Apc40Mk2 => (0x00..=0x27).chain(0x3A..=0x41).chain([0x50]).chain(0x52..=0x56).chain(0x57..=0x5B).chain([0x5D, 0x66]).collect(),
        _ => Vec::new(),
    };
    for n in singles {
        note(0, n);
    }
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(product: u8) -> Vec<u8> {
        // F0 7E ch 06 02 47 <pid> 00 19 <version x4> <dev> <serial x4> F7 (manufacturer data trimmed)
        let mut r = vec![0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47, product, 0x00, 0x19, 0x00, 0x01, 0x00, 0x00, 0x7F, 0x00, 0x00, 0x00, 0x00];
        r.push(0xF7);
        r
    }

    #[test]
    fn identity_reply_models() {
        let model = |pid| Model::from_identity(&parse_identity(&reply(pid)).unwrap());
        assert_eq!(model(0x73), Model::Apc40);
        assert_eq!(model(0x29), Model::Apc40Mk2);
        assert_eq!(model(0x28), Model::ApcMini);
        assert_eq!(model(0x11), Model::Unknown);
    }

    #[test]
    fn identity_of_other_manufacturers() {
        // Novation uses a three-byte id 00 20 29; its product byte 0x29 must not read as an APC40 mkII.
        let novation = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x29, 0x13, 0x01, 0xF7];
        let id = parse_identity(&novation).unwrap();
        assert_eq!(id, Identity { manufacturer: 0x2029, product: 0x13 });
        assert_eq!(Model::from_identity(&id), Model::Unknown);
        let roland = [0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0x29, 0x00, 0xF7];
        assert_eq!(Model::from_identity(&parse_identity(&roland).unwrap()), Model::Unknown);
    }

    #[test]
    fn truncated_or_foreign_replies_are_rejected() {
        assert_eq!(parse_identity(&[0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47, 0xF7]), None);
        assert_eq!(parse_identity(&[0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47]), None);
        assert_eq!(parse_identity(&reply(0x29)[..10]), None, "no F7");
        assert_eq!(parse_identity(&DEVICE_INQUIRY), None, "our own request is not a reply");
        assert_eq!(parse_identity(&[]), None);
        assert_eq!(parse_identity(&[0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0xF7]), None);
    }

    #[test]
    fn port_name_hints() {
        assert_eq!(Model::from_port_name("APC40 mkII"), Model::Apc40Mk2);
        assert_eq!(Model::from_port_name("Akai APC40 MK2 Port 1"), Model::Apc40Mk2);
        assert_eq!(Model::from_port_name("APC40"), Model::Apc40);
        assert_eq!(Model::from_port_name("Akai APC40"), Model::Apc40);
        assert_eq!(Model::from_port_name("APC MINI"), Model::ApcMini);
        assert_eq!(Model::from_port_name("nanoKONTROL2 SLIDER/KNOB"), Model::Unknown);
        assert_eq!(Model::from_port_name("IAC Driver Bus 1"), Model::Unknown);
    }

    #[test]
    fn introduction_bytes() {
        assert_eq!(introduction(PID_APC40_MK2, MODE_ABLETON), vec![0xF0, 0x47, 0x7F, 0x29, 0x60, 0x00, 0x04, 0x41, 0x01, 0x00, 0x00, 0xF7]);
        assert_eq!(introduction(PID_APC40, MODE_ABLETON), vec![0xF0, 0x47, 0x7F, 0x73, 0x60, 0x00, 0x04, 0x41, 0x01, 0x00, 0x00, 0xF7]);
        assert_eq!(introduction(PID_APC40, MODE_ALTERNATE)[7], 0x42);
        assert_eq!(introduction(PID_APC40_MK2, MODE_GENERIC)[7], 0x40);
    }

    #[test]
    fn fader_positions_reply() {
        let mut msg = vec![0xF0, 0x47, 0x7F, 0x29, 0x61, 0x00, 0x09];
        msg.extend([0, 10, 20, 30, 40, 50, 60, 70, 127]);
        msg.push(0xF7);
        assert_eq!(parse_fader_positions(&msg), Some([0, 10, 20, 30, 40, 50, 60, 70, 127]));
        assert_eq!(parse_fader_positions(&msg[..12]), None);
        msg[4] = 0x60;
        assert_eq!(parse_fader_positions(&msg), None);
    }

    #[test]
    fn leds_off_covers_the_grid() {
        let mk2 = leds_off(Model::Apc40Mk2);
        assert!(mk2.contains(&[0x90, 0x00, 0]) && mk2.contains(&[0x90, 0x27, 0]));
        assert!(mk2.contains(&[0x97, 0x34, 0]), "clip stop 8");
        let mk1 = leds_off(Model::Apc40);
        assert!(mk1.contains(&[0x90, 0x35, 0]) && mk1.contains(&[0x97, 0x39, 0]));
        assert!(mk1.iter().all(|m| m[2] == 0 && m[0] & 0xF0 == 0x90));
        assert!(leds_off(Model::Unknown).is_empty());
    }
}
