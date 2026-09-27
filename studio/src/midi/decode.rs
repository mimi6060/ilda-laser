//! Raw MIDI bytes -> `MidiMsg`. Pure and allocation-light: it runs inside
//! the CoreMIDI callback, so it never locks anything and never panics on
//! garbage input (truncated messages and stray data bytes are dropped).

use serde::{Deserialize, Serialize};

/// Largest SysEx we keep. Controllers answer the Device Inquiry in ~40
/// bytes; anything this big is a firmware dump we don't care about.
const MAX_SYSEX: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MidiMsg {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    /// Also produced by a Note On with velocity 0.
    NoteOff { channel: u8, note: u8 },
    Cc { channel: u8, number: u8, value: u8 },
    /// 14-bit value, 8192 = centre.
    PitchBend { channel: u8, value: u16 },
    /// Needed by generic controllers (T-211).
    ProgramChange { channel: u8, program: u8 },
    /// Whole message, `F0` .. `F7` included.
    SysEx { bytes: Vec<u8> },
    Clock,
    Start,
    Stop,
    Continue,
}

impl MidiMsg {
    /// MIDI clock and transport: high-rate, not worth showing in the monitor.
    pub fn is_realtime(&self) -> bool {
        matches!(self, MidiMsg::Clock | MidiMsg::Start | MidiMsg::Stop | MidiMsg::Continue)
    }
}

/// Stateful decoder for one input port: keeps running status and a SysEx
/// that is split over several packets.
#[derive(Default)]
pub struct Decoder {
    running: Option<u8>,
    /// Data bytes of the channel message being assembled.
    data: [u8; 2],
    have: usize,
    /// Bytes still to skip for a system common message we ignore.
    skip: usize,
    sysex: Option<Vec<u8>>,
}

impl Decoder {
    /// Feeds one packet and calls `out` for every complete message in it.
    pub fn feed(&mut self, bytes: &[u8], mut out: impl FnMut(MidiMsg)) {
        for &b in bytes {
            if b >= 0xF8 {
                // Real-time bytes may appear anywhere, even inside a SysEx.
                match b {
                    0xF8 => out(MidiMsg::Clock),
                    0xFA => out(MidiMsg::Start),
                    0xFB => out(MidiMsg::Continue),
                    0xFC => out(MidiMsg::Stop),
                    _ => {} // active sensing, reset, undefined
                }
                continue;
            }
            if let Some(buf) = self.sysex.as_mut() {
                if b == 0xF7 {
                    buf.push(b);
                    let bytes = self.sysex.take().unwrap_or_default();
                    out(MidiMsg::SysEx { bytes });
                    continue;
                }
                if b < 0x80 {
                    if buf.len() < MAX_SYSEX {
                        buf.push(b);
                    } else {
                        self.sysex = None; // too big: drop it
                    }
                    continue;
                }
                // Any other status byte aborts the SysEx (truncated: dropped).
                self.sysex = None;
            }
            if b >= 0x80 {
                self.have = 0;
                self.skip = 0;
                match b {
                    0xF0 => {
                        self.running = None;
                        self.sysex = Some(vec![b]);
                    }
                    0xF1 | 0xF3 => {
                        self.running = None;
                        self.skip = 1;
                    }
                    0xF2 => {
                        self.running = None;
                        self.skip = 2;
                    }
                    0xF4..=0xF7 => self.running = None,
                    _ => self.running = Some(b),
                }
                continue;
            }
            // Data byte.
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let Some(status) = self.running else { continue };
            self.data[self.have] = b;
            self.have += 1;
            if self.have < data_len(status) {
                continue;
            }
            self.have = 0;
            if let Some(msg) = channel_msg(status, self.data) {
                out(msg);
            }
        }
    }
}

/// Decodes one packet with a fresh decoder.
#[cfg(test)]
pub fn decode(bytes: &[u8]) -> Vec<MidiMsg> {
    let mut msgs = Vec::new();
    Decoder::default().feed(bytes, |m| msgs.push(m));
    msgs
}

fn data_len(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

fn channel_msg(status: u8, d: [u8; 2]) -> Option<MidiMsg> {
    let channel = status & 0x0F;
    Some(match status & 0xF0 {
        0x80 => MidiMsg::NoteOff { channel, note: d[0] },
        0x90 if d[1] == 0 => MidiMsg::NoteOff { channel, note: d[0] },
        0x90 => MidiMsg::NoteOn { channel, note: d[0], velocity: d[1] },
        0xB0 => MidiMsg::Cc { channel, number: d[0], value: d[1] },
        0xC0 => MidiMsg::ProgramChange { channel, program: d[0] },
        0xE0 => MidiMsg::PitchBend { channel, value: d[0] as u16 | (d[1] as u16) << 7 },
        _ => return None, // aftertouch: not used
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes() {
        assert_eq!(decode(&[0x90, 0x35, 0x7F]), vec![MidiMsg::NoteOn { channel: 0, note: 0x35, velocity: 127 }]);
        assert_eq!(decode(&[0x87, 0x39, 0x40]), vec![MidiMsg::NoteOff { channel: 7, note: 0x39 }]);
    }

    #[test]
    fn note_on_velocity_zero_is_note_off() {
        assert_eq!(decode(&[0x92, 0x10, 0x00]), vec![MidiMsg::NoteOff { channel: 2, note: 0x10 }]);
    }

    #[test]
    fn cc_and_program_change() {
        assert_eq!(decode(&[0xB3, 0x07, 0x64]), vec![MidiMsg::Cc { channel: 3, number: 7, value: 100 }]);
        assert_eq!(decode(&[0xC1, 0x05]), vec![MidiMsg::ProgramChange { channel: 1, program: 5 }]);
    }

    #[test]
    fn pitch_bend_is_14_bits() {
        assert_eq!(decode(&[0xE0, 0x00, 0x40]), vec![MidiMsg::PitchBend { channel: 0, value: 8192 }]);
        assert_eq!(decode(&[0xEF, 0x7F, 0x7F]), vec![MidiMsg::PitchBend { channel: 15, value: 16383 }]);
        assert_eq!(decode(&[0xE0, 0x01, 0x00]), vec![MidiMsg::PitchBend { channel: 0, value: 1 }]);
    }

    #[test]
    fn complete_sysex() {
        let reply = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47, 0x29, 0x00, 0x19, 0xF7];
        assert_eq!(decode(&reply), vec![MidiMsg::SysEx { bytes: reply.to_vec() }]);
    }

    #[test]
    fn sysex_split_over_packets() {
        let mut d = Decoder::default();
        let mut got = Vec::new();
        d.feed(&[0xF0, 0x7E, 0x00], |m| got.push(m));
        assert!(got.is_empty());
        d.feed(&[0x06, 0x02, 0xF7], |m| got.push(m));
        assert_eq!(got, vec![MidiMsg::SysEx { bytes: vec![0xF0, 0x7E, 0x00, 0x06, 0x02, 0xF7] }]);
    }

    #[test]
    fn truncated_bytes_are_ignored() {
        assert!(decode(&[0x90, 0x35]).is_empty());
        assert!(decode(&[0xB0]).is_empty());
        assert!(decode(&[0xF0, 0x7E, 0x00, 0x06]).is_empty());
        assert!(decode(&[0x35, 0x7F]).is_empty(), "data bytes without a status");
        assert!(decode(&[]).is_empty());
        // A SysEx cut by a new status is dropped, the new message still decodes.
        assert_eq!(decode(&[0xF0, 0x47, 0x90, 0x01, 0x02]), vec![MidiMsg::NoteOn { channel: 0, note: 1, velocity: 2 }]);
    }

    #[test]
    fn realtime_messages() {
        assert_eq!(
            decode(&[0xF8, 0xFA, 0xFB, 0xFC, 0xFE, 0xFF]),
            vec![MidiMsg::Clock, MidiMsg::Start, MidiMsg::Continue, MidiMsg::Stop]
        );
        // Clock inside a SysEx does not break it.
        assert_eq!(
            decode(&[0xF0, 0x01, 0xF8, 0x02, 0xF7]),
            vec![MidiMsg::Clock, MidiMsg::SysEx { bytes: vec![0xF0, 0x01, 0x02, 0xF7] }]
        );
    }

    #[test]
    fn several_messages_and_running_status() {
        assert_eq!(
            decode(&[0xB0, 0x07, 0x10, 0x07, 0x11, 0x90, 0x01, 0x7F]),
            vec![
                MidiMsg::Cc { channel: 0, number: 7, value: 0x10 },
                MidiMsg::Cc { channel: 0, number: 7, value: 0x11 },
                MidiMsg::NoteOn { channel: 0, note: 1, velocity: 127 },
            ]
        );
    }

    #[test]
    fn system_common_and_aftertouch_are_skipped() {
        assert_eq!(decode(&[0xF2, 0x10, 0x20, 0xA0, 0x01, 0x02, 0xD0, 0x40, 0xF8]), vec![MidiMsg::Clock]);
    }

    #[test]
    fn serializes_with_a_kind_tag() {
        let json = serde_json::to_value(MidiMsg::Cc { channel: 0, number: 14, value: 3 }).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "cc", "channel": 0, "number": 14, "value": 3 }));
    }
}
