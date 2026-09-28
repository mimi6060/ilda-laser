//! Native MIDI (CoreMIDI through `midir`): every controller port is opened
//! by a dedicated "midi" thread, identified (APC40, APC40 mkII or generic)
//! and given a profile. The browser only shows MIDI state (`/api/midi`);
//! the laser keeps answering the controller with the tab closed.
//!
//! Threads: the CoreMIDI callback only decodes bytes and pushes a
//! `MidiEvent` into a channel (no lock). The worker thread (`worker.rs`)
//! drains the channel, takes the `Shared` lock briefly per batch and hands
//! the batch to the mapping engine (`engine.rs`, T-202), which drives
//! controls through `controls::apply(…, true)`; the engine thread flushes
//! coalesced fader writes once per frame (`engine::frame`). Safety rules
//! (blackout first, opt-in arming, capped brightness) are in `safety.rs`.

pub mod api;
pub mod backend;
pub mod decode;
pub mod detect;
pub mod engine;
pub mod learn;
pub mod led;
pub mod mapping;
pub mod profile;
pub mod safety;
pub mod testing;
pub mod worker;

pub use decode::MidiMsg;
pub use detect::Model;

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// Messages kept for the MIDI monitor.
pub const RECENT_LEN: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub struct MidiEvent {
    pub port: String,
    pub msg: MidiMsg,
    pub at: Instant,
}

impl MidiEvent {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "port": self.port, "msg": self.msg, "age_ms": self.at.elapsed().as_millis() as u64 })
    }
}

/// One MIDI port as the UI sees it. Ports stay listed (disconnected) after
/// being unplugged, so the UI can warn about it.
#[derive(Clone, Debug, Serialize)]
pub struct MidiDevice {
    pub name: String,
    /// An input / output port with this name currently exists.
    pub input: bool,
    pub output: bool,
    /// We have its input open.
    pub connected: bool,
    pub enabled: bool,
    pub model: Model,
    pub model_label: &'static str,
    /// Slug of the profile in use.
    pub profile: String,
    /// Why the preferred profile could not be used, if so.
    pub profile_error: Option<String>,
    /// APC40 mkII fader positions (tracks 1–8, master) from its reply to
    /// the Introduction, for pickup (T-202).
    pub faders: Option<[u8; 9]>,
    /// When its input was opened (T-208: no arming in the first seconds).
    #[serde(skip)]
    pub connected_at: Option<Instant>,
    /// Unplugged while in use: the UI shows « Contrôleur MIDI déconnecté »
    /// until it comes back (not set when disabled by hand).
    pub lost: bool,
    /// « Retour LED » ticked (T-205).
    pub leds: bool,
}

impl MidiDevice {
    pub fn new(name: &str) -> Self {
        MidiDevice {
            name: name.to_string(),
            input: false,
            output: false,
            connected: false,
            enabled: true,
            model: Model::Unknown,
            model_label: Model::Unknown.label(),
            profile: profile::GENERIC.to_string(),
            profile_error: None,
            faders: None,
            connected_at: None,
            lost: false,
            leds: true,
        }
    }
}

/// Commands other threads send to the worker (it owns the connections).
#[derive(Debug, PartialEq)]
pub enum Command {
    Send { port: String, bytes: Vec<u8> },
}

/// Cheap, clonable way to send bytes to a controller from any thread.
#[derive(Clone)]
pub struct MidiSender(Sender<Command>);

impl MidiSender {
    /// Queues `bytes` for the output port named `port`. Never blocks; a
    /// port that is gone just drops them.
    pub fn send(&self, port: &str, bytes: &[u8]) {
        let _ = self.0.send(Command::Send { port: port.to_string(), bytes: bytes.to_vec() });
    }
}

/// `Shared.midi`: never saved in scenes.
pub struct MidiState {
    /// False with `--no-midi`: no port is ever opened.
    pub enabled: bool,
    pub devices: Vec<MidiDevice>,
    pub last: Option<MidiEvent>,
    pub recent: VecDeque<MidiEvent>,
    pub store: profile::ProfileStore,
    /// CoreMIDI-level problem (e.g. unavailable), for the UI.
    pub error: Option<String>,
    pub sender: Option<MidiSender>,
    /// Mapping engine runtime state (Shift, held buttons, pickup…).
    pub map: engine::MapState,
    /// `--midi-test` only: the simulated devices behind
    /// `/api/midi/inject` and `/api/midi/sent` (T-209). `None` in normal
    /// runs, and those routes then don't exist.
    pub sim: Option<testing::SimMidi>,
    /// MIDI learn (T-203): pending request, last result.
    pub learn: learn::LearnState,
}

impl MidiState {
    pub fn new(enabled: bool, store: profile::ProfileStore) -> Self {
        MidiState { enabled, devices: Vec::new(), last: None, recent: VecDeque::new(), store, error: None, sender: None, map: engine::MapState::default(), sim: None, learn: learn::LearnState::default() }
    }

    pub fn device_mut(&mut self, name: &str) -> &mut MidiDevice {
        let i = match self.devices.iter().position(|d| d.name == name) {
            Some(i) => i,
            None => {
                self.devices.push(MidiDevice::new(name));
                self.devices.len() - 1
            }
        };
        &mut self.devices[i]
    }

    /// Keeps the last message and a short history for the monitor. MIDI
    /// clock (24 per beat) would flush everything else, so real-time
    /// messages are not recorded.
    pub fn record(&mut self, event: &MidiEvent) {
        if event.msg.is_realtime() {
            return;
        }
        self.last = Some(event.clone());
        if self.recent.len() >= RECENT_LEN {
            self.recent.pop_front();
        }
        self.recent.push_back(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(msg: MidiMsg) -> MidiEvent {
        MidiEvent { port: "APC40 mkII".into(), msg, at: Instant::now() }
    }

    #[test]
    fn recent_history_is_capped_and_skips_clock() {
        let mut m = MidiState::new(true, profile::ProfileStore::in_memory());
        for i in 0..30 {
            m.record(&ev(MidiMsg::Cc { channel: 0, number: 7, value: i }));
            m.record(&ev(MidiMsg::Clock));
        }
        assert_eq!(m.recent.len(), RECENT_LEN);
        assert_eq!(m.recent.front().unwrap().msg, MidiMsg::Cc { channel: 0, number: 7, value: 10 });
        assert_eq!(m.last.as_ref().unwrap().msg, MidiMsg::Cc { channel: 0, number: 7, value: 29 });
    }

    #[test]
    fn event_json_has_port_msg_and_age() {
        let json = ev(MidiMsg::NoteOn { channel: 0, note: 0x51, velocity: 127 }).to_json();
        assert_eq!(json["port"], "APC40 mkII");
        assert_eq!(json["msg"]["kind"], "note_on");
        assert!(json["age_ms"].is_u64());
    }
}
