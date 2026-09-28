//! MIDI without hardware (T-209). `FakeApc` plays an APC40 / APC40 mkII:
//! it builds the bytes of a pad, fader, encoder or Shift press, answers the
//! Device Inquiry (and, for the mkII, the Introduction with its fader
//! positions) and records what the studio sends it, so a test can ask
//! « is pad (0,0) lit? ». `SimMidi` is a `Backend` that hosts fake devices
//! instead of CoreMIDI: unit tests use it, and `--midi-test` runs the real
//! worker on it so e2e tests can inject bytes over HTTP
//! (`/api/midi/inject`, `/api/midi/sent`).
//!
//! Nothing here touches CoreMIDI. Injected bytes take the same path as a
//! real controller's (decoder → worker → mapping engine → T-208 rules), so
//! they can't do anything a real APC couldn't: in particular they can't
//! arm the laser unless the user opted in (Shift + 1 s hold, 5 s guard).

use super::backend::{Backend, InputCallback, InputHandle, OutputPort};
use super::detect::{Model, DEVICE_INQUIRY, PID_APC40, PID_APC40_MK2};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

/// Name of the simulated mkII that `--midi-test` plugs in.
pub const TEST_MK2_PORT: &str = "Test APC40 mkII";
/// Messages a fake device remembers (oldest dropped first).
const RECEIVED_MAX: usize = 4096;

/// APC40 mkII / APC40 button notes (Akai's public protocol documents).
pub const NOTE_STOP_ALL: u8 = 0x51;
pub const NOTE_SHIFT: u8 = 0x62;
/// Track faders: CC 7 on channel 0–7; master: CC 14 on channel 0.
pub const CC_TRACK_FADER: u8 = 0x07;
pub const CC_MASTER_FADER: u8 = 0x0E;
pub const GRID_ROWS: u8 = 5;
pub const GRID_COLS: u8 = 8;

/// A pretend APC40 (mkII or original).
#[derive(Clone, Debug)]
pub struct FakeApc {
    pub model: Model,
    /// Every message the studio sent it, in order.
    pub received: VecDeque<Vec<u8>>,
    /// Physical fader positions (tracks 1–8, master), as the mkII reports
    /// them after the Introduction.
    pub faders: [u8; 9],
}

impl FakeApc {
    /// `model` is `Apc40Mk2` or `Apc40`.
    pub fn new(model: Model) -> Self {
        assert!(matches!(model, Model::Apc40 | Model::Apc40Mk2), "FakeApc plays an APC40 or APC40 mkII");
        FakeApc { model, received: VecDeque::new(), faders: [0; 9] }
    }

    fn pid(&self) -> u8 {
        if self.model == Model::Apc40Mk2 {
            PID_APC40_MK2
        } else {
            PID_APC40
        }
    }

    /// Clip-launch pad `(row, col)`, row 0 at the top: (MIDI channel, note).
    /// mkII: notes 0–39 on channel 0, note 0 at the bottom left. APC40:
    /// notes 0x35–0x39 (top to bottom), channel = column.
    pub fn pad_note(&self, row: u8, col: u8) -> (u8, u8) {
        assert!(row < GRID_ROWS && col < GRID_COLS, "pad ({row},{col}) is off the grid");
        match self.model {
            Model::Apc40Mk2 => (0, (GRID_ROWS - 1 - row) * GRID_COLS + col),
            _ => (col, 0x35 + row),
        }
    }

    fn note(channel: u8, note: u8, down: bool) -> Vec<u8> {
        if down {
            vec![0x90 | channel, note, 0x7F]
        } else {
            vec![0x80 | channel, note, 0x00]
        }
    }

    pub fn pad(&self, row: u8, col: u8, down: bool) -> Vec<u8> {
        let (channel, note) = self.pad_note(row, col);
        Self::note(channel, note, down)
    }

    pub fn button(&self, note: u8, down: bool) -> Vec<u8> {
        Self::note(0, note, down)
    }

    pub fn shift(&self, down: bool) -> Vec<u8> {
        self.button(NOTE_SHIFT, down)
    }

    pub fn stop_all(&self, down: bool) -> Vec<u8> {
        self.button(NOTE_STOP_ALL, down)
    }

    /// Fader `n` (0–7 = tracks 1–8, 8 = master) moved to `value` (0–127).
    pub fn fader(&mut self, n: u8, value: u8) -> Vec<u8> {
        assert!(n < 9, "fader {n}");
        let value = value.min(127);
        self.faders[n as usize] = value;
        if n == 8 {
            vec![0xB0, CC_MASTER_FADER, value]
        } else {
            vec![0xB0 | n, CC_TRACK_FADER, value]
        }
    }

    /// An endless encoder on `cc` turned by `delta` steps (two's
    /// complement, as the APCs send it).
    pub fn knob(&self, cc: u8, delta: i8) -> Vec<u8> {
        let delta = delta.clamp(-64, 63);
        vec![0xB0, cc & 0x7F, (delta as u8) & 0x7F]
    }

    /// The APC's answer to the Device Inquiry.
    pub fn identity_reply(&self) -> Vec<u8> {
        vec![0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47, self.pid(), 0x00, 0x19, 0x00, 0x01, 0x00, 0x00, 0x7F, 0x00, 0x00, 0x00, 0x00, 0xF7]
    }

    /// The mkII's `0x61` message: its 9 fader positions.
    pub fn fader_positions(&self) -> Vec<u8> {
        let mut m = vec![0xF0, 0x47, 0x00, PID_APC40_MK2, 0x61, 0x00, 0x09];
        m.extend_from_slice(&self.faders);
        m.push(0xF7);
        m
    }

    /// The studio sent `bytes` (one message): record it and return what the
    /// device answers.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        if self.received.len() >= RECEIVED_MAX {
            self.received.pop_front();
        }
        self.received.push_back(bytes.to_vec());
        if bytes == DEVICE_INQUIRY {
            return vec![self.identity_reply()];
        }
        if self.model == Model::Apc40Mk2 && self.introduction_mode(bytes).is_some() {
            return vec![self.fader_positions()];
        }
        Vec::new()
    }

    fn introduction_mode(&self, bytes: &[u8]) -> Option<u8> {
        (bytes.len() == 12 && bytes[..7] == [0xF0, 0x47, 0x7F, self.pid(), 0x60, 0x00, 0x04] && bytes[11] == 0xF7).then(|| bytes[7])
    }

    /// Mode of the last Introduction received (0x41 = taken over by the
    /// studio, 0x40 = given back), `None` before any.
    pub fn mode(&self) -> Option<u8> {
        self.received.iter().rev().find_map(|b| self.introduction_mode(b))
    }

    /// Last LED value sent to pad `(row, col)`: the velocity (mkII: colour
    /// index; APC40: 0 off, 1 green, 3 red, 5 yellow…), 0 after a Note
    /// Off, `None` if the studio never addressed it.
    pub fn led_at(&self, row: u8, col: u8) -> Option<u8> {
        let (channel, note) = self.pad_note(row, col);
        self.received.iter().rev().find_map(|b| {
            let [status, n, v] = b[..] else { return None };
            // mkII pads: the channel is the LED behaviour (solid, pulse…).
            let same_channel = self.model == Model::Apc40Mk2 || status & 0x0F == channel;
            (n == note && same_channel).then_some(match status & 0xF0 {
                0x90 => Some(v),
                0x80 => Some(0),
                _ => None,
            })?
        })
    }

    /// The 5 × 8 grid as `led_at` sees it (row 0 at the top).
    pub fn pads(&self) -> Vec<Vec<Option<u8>>> {
        (0..GRID_ROWS).map(|r| (0..GRID_COLS).map(|c| self.led_at(r, c)).collect()).collect()
    }
}

#[derive(Default)]
struct Sim {
    devices: BTreeMap<String, FakeApc>,
    /// Open inputs: where injected bytes go.
    inputs: BTreeMap<String, InputCallback>,
}

/// A MIDI "system" with only fake devices. Cloning shares it: the worker
/// owns one clone as its backend, tests / the HTTP route keep another.
#[derive(Clone, Default)]
pub struct SimMidi(Arc<Mutex<Sim>>);

impl SimMidi {
    fn lock(&self) -> MutexGuard<'_, Sim> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Plugs `device` in as port `name` (input and output).
    pub fn plug(&self, name: &str, device: FakeApc) {
        self.lock().devices.insert(name.to_string(), device);
    }

    pub fn unplug(&self, name: &str) {
        self.lock().devices.remove(name);
    }

    pub fn ports(&self) -> Vec<String> {
        self.lock().devices.keys().cloned().collect()
    }

    /// Bytes "played" on the device, as if they came from its USB port.
    /// Delivered like CoreMIDI would: to the studio's open input.
    pub fn inject(&self, port: &str, bytes: &[u8]) -> Result<(), InjectError> {
        let mut sim = self.lock();
        if !sim.devices.contains_key(port) {
            return Err(InjectError::NoSuchPort);
        }
        let input = sim.inputs.get_mut(port).ok_or(InjectError::NotOpen)?;
        input(bytes);
        Ok(())
    }

    /// Runs `f` on the fake device behind `port`.
    pub fn with_device<R>(&self, port: &str, f: impl FnOnce(&mut FakeApc) -> R) -> Option<R> {
        self.lock().devices.get_mut(port).map(f)
    }

    /// Whether the studio has the port's input open.
    pub fn is_open(&self, port: &str) -> bool {
        self.lock().inputs.contains_key(port)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InjectError {
    NoSuchPort,
    /// The port exists but the studio doesn't listen to it (disabled).
    NotOpen,
}

struct SimInput(SimMidi, String);

impl Drop for SimInput {
    fn drop(&mut self) {
        self.0.lock().inputs.remove(&self.1);
    }
}

struct SimOutput(SimMidi, String);

impl OutputPort for SimOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut sim = self.0.lock();
        let sim = &mut *sim;
        let replies = sim.devices.get_mut(&self.1).ok_or("port disparu")?.receive(bytes);
        // The device answers on its own output, i.e. the studio's input.
        if let Some(input) = sim.inputs.get_mut(&self.1) {
            for reply in replies {
                input(&reply);
            }
        }
        Ok(())
    }
}

impl Backend for SimMidi {
    fn ports(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
        let names = SimMidi::ports(self);
        Ok((names.clone(), names))
    }

    fn open_input(&mut self, name: &str, callback: InputCallback) -> Result<InputHandle, String> {
        let mut sim = self.lock();
        if !sim.devices.contains_key(name) {
            return Err("port disparu".into());
        }
        sim.inputs.insert(name.to_string(), callback);
        Ok(Box::new(SimInput(self.clone(), name.to_string())))
    }

    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputPort>, String> {
        if !self.lock().devices.contains_key(name) {
            return Err("port disparu".into());
        }
        Ok(Box::new(SimOutput(self.clone(), name.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::ArmSource;
    use crate::midi::detect::{introduction, parse_fader_positions, parse_identity, MODE_ABLETON, MODE_GENERIC};
    use crate::midi::profile::Profile;
    use crate::midi::worker::Worker;
    use crate::midi::MidiMsg;
    use crate::test_support;
    use crate::Shared;
    use std::time::{Duration, Instant};

    /// Scene launch 1.
    const NOTE_SCENE: u8 = 0x52;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn mk2_bytes_follow_akai_layout() {
        let mut apc = FakeApc::new(Model::Apc40Mk2);
        assert_eq!(apc.pad(0, 0, true), vec![0x90, 32, 0x7F], "top-left pad");
        assert_eq!(apc.pad(4, 0, true), vec![0x90, 0, 0x7F], "bottom-left pad");
        assert_eq!(apc.pad(4, 7, false), vec![0x80, 7, 0], "release");
        assert_eq!(apc.shift(true), vec![0x90, 0x62, 0x7F]);
        assert_eq!(apc.stop_all(true), vec![0x90, 0x51, 0x7F]);
        assert_eq!(apc.fader(2, 100), vec![0xB2, 0x07, 100]);
        assert_eq!(apc.fader(8, 200), vec![0xB0, 0x0E, 127], "master, clamped");
        assert_eq!(apc.faders[2], 100);
        assert_eq!(apc.knob(0x2F, 3), vec![0xB0, 0x2F, 3]);
        assert_eq!(apc.knob(0x2F, -1), vec![0xB0, 0x2F, 127], "two's complement");
        assert_eq!(apc.knob(0x2F, -100), vec![0xB0, 0x2F, 64], "clamped to -64");
    }

    #[test]
    fn apc40_pads_use_the_channel_for_the_column() {
        let apc = FakeApc::new(Model::Apc40);
        assert_eq!(apc.pad(0, 0, true), vec![0x90, 0x35, 0x7F]);
        assert_eq!(apc.pad(4, 7, true), vec![0x97, 0x39, 0x7F]);
    }

    #[test]
    fn answers_the_inquiry_with_its_product_id() {
        for (model, pid) in [(Model::Apc40Mk2, 0x29), (Model::Apc40, 0x73)] {
            let mut apc = FakeApc::new(model);
            let reply = apc.receive(&DEVICE_INQUIRY);
            let id = parse_identity(&reply[0]).expect("a valid identity reply");
            assert_eq!((id.manufacturer, id.product), (0x47, pid));
            assert_eq!(Model::from_identity(&id), model);
        }
    }

    #[test]
    fn mk2_reports_its_faders_after_the_introduction() {
        let mut apc = FakeApc::new(Model::Apc40Mk2);
        apc.fader(0, 64);
        assert_eq!(apc.mode(), None);
        let reply = apc.receive(&introduction(PID_APC40_MK2, MODE_ABLETON));
        assert_eq!(parse_fader_positions(&reply[0]), Some([64, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(apc.mode(), Some(MODE_ABLETON));
        let mut old = FakeApc::new(Model::Apc40);
        assert!(old.receive(&introduction(PID_APC40, MODE_ABLETON)).is_empty(), "only the mkII sends 0x61");
        assert!(apc.receive(&introduction(PID_APC40, MODE_ABLETON)).is_empty(), "another model's introduction");
    }

    #[test]
    fn leds_are_read_back_per_pad() {
        let mut apc = FakeApc::new(Model::Apc40Mk2);
        assert_eq!(apc.led_at(0, 0), None);
        apc.receive(&[0x90, 32, 21]); // green
        apc.receive(&[0x97, 0, 5]); // bottom-left, pulsing red
        assert_eq!(apc.led_at(0, 0), Some(21));
        assert_eq!(apc.led_at(4, 0), Some(5), "any channel on the mkII");
        apc.receive(&[0x80, 32, 0]);
        assert_eq!(apc.led_at(0, 0), Some(0));
        assert_eq!(apc.pads()[4][0], Some(5));

        let mut old = FakeApc::new(Model::Apc40);
        old.receive(&[0x92, 0x36, 1]);
        assert_eq!(old.led_at(1, 2), Some(1));
        assert_eq!(old.led_at(1, 3), None, "channel = column on the APC40");
    }

    #[test]
    fn received_history_is_bounded() {
        let mut apc = FakeApc::new(Model::Apc40Mk2);
        for i in 0..RECEIVED_MAX + 10 {
            apc.receive(&[0x90, 0, (i % 128) as u8]);
        }
        assert_eq!(apc.received.len(), RECEIVED_MAX);
    }

    /// A profile in the spirit of T-204, for the tests: grid, Stop All,
    /// Shift layer on scene 1, a fader with pickup, an arm button.
    pub fn test_profile() -> Profile {
        let mut mappings: Vec<serde_json::Value> = (0..40u8)
            .map(|note| {
                let slot = (4 - note / 8) * 8 + note % 8;
                serde_json::json!({ "input": { "kind": "note", "channel": 0, "number": note }, "mode": "grid", "args": { "slot": slot } })
            })
            .collect();
        mappings.extend([
            serde_json::json!({ "input": { "kind": "note", "channel": 0, "number": NOTE_STOP_ALL }, "target": "transport.blackout", "mode": "trigger" }),
            serde_json::json!({ "input": { "kind": "note", "channel": 0, "number": NOTE_SCENE }, "target": "page.1", "mode": "trigger" }),
            serde_json::json!({ "input": { "kind": "note", "channel": 0, "number": NOTE_SCENE }, "shift": true, "target": "page.2", "mode": "trigger" }),
            serde_json::json!({ "input": { "kind": "cc", "channel": 0, "number": CC_TRACK_FADER }, "target": "master.size", "mode": "absolute", "pickup": true }),
            serde_json::json!({ "input": { "kind": "cc", "channel": 0, "number": 0x2F }, "target": "master.pos_x", "mode": "relative", "step": 0.1 }),
            serde_json::json!({ "input": { "kind": "note", "channel": 0, "number": 0x5B }, "shift": true, "target": "transport.arm", "mode": "trigger" }),
        ]);
        let json = serde_json::json!({
            "name": "Test", "driver": "apc40mk2", "match": { "port_contains": [] },
            "shift_key": { "kind": "note", "channel": 0, "number": NOTE_SHIFT },
            "mappings": mappings,
        });
        Profile::parse(&json.to_string()).unwrap()
    }

    /// The worker on a simulated mkII. Events carry real timestamps (the
    /// input callback stamps them), so the rig runs on the real clock.
    struct Rig {
        worker: Worker<SimMidi>,
        sim: SimMidi,
        shared: Arc<Mutex<Shared>>,
    }

    impl Rig {
        fn new(profile: bool) -> Rig {
            let sim = SimMidi::default();
            sim.plug(TEST_MK2_PORT, FakeApc::new(Model::Apc40Mk2));
            let shared = Arc::new(Mutex::new(test_support::shared()));
            if profile {
                shared.lock().unwrap().midi.store.save_profile(Some(TEST_MK2_PORT), "test", test_profile()).unwrap();
            }
            let mut worker = Worker::new(sim.clone(), Arc::clone(&shared));
            // Open + inquiry, identity reply + Introduction, 0x61 reply.
            for _ in 0..3 {
                worker.step(Instant::now(), None);
            }
            Rig { worker, sim, shared }
        }

        /// As if the controller had been plugged in 10 s ago (past the
        /// T-208 plug guard).
        fn plugged_long_ago(&self) {
            let mut s = self.shared.lock().unwrap();
            let d = s.midi.devices.iter_mut().find(|d| d.name == TEST_MK2_PORT).unwrap();
            d.connected_at = Instant::now().checked_sub(Duration::from_secs(10));
        }

        /// Plays `bytes` on the fake device, then runs the worker and one
        /// engine frame.
        fn play(&mut self, bytes: &[u8]) {
            self.sim.inject(TEST_MK2_PORT, bytes).unwrap();
            let now = Instant::now();
            self.worker.step(now, None);
            crate::midi::engine::frame(&mut self.shared.lock().unwrap(), now);
        }

        /// An engine frame `later` from now (arming holds are checked there).
        fn frame_in(&self, later: Duration) {
            crate::midi::engine::frame(&mut self.shared.lock().unwrap(), Instant::now() + later);
        }

        fn apc<R>(&self, f: impl FnOnce(&mut FakeApc) -> R) -> R {
            self.sim.with_device(TEST_MK2_PORT, f).unwrap()
        }
    }

    #[test]
    fn simulated_mk2_is_detected_introduced_and_released() {
        let mut rig = Rig::new(false);
        let d = rig.shared.lock().unwrap().midi.devices.iter().find(|d| d.name == TEST_MK2_PORT).cloned().unwrap();
        assert_eq!((d.model, d.profile.as_str(), d.connected), (Model::Apc40Mk2, "apc40-mk2", true));
        assert_eq!(d.faders, Some([0; 9]), "0x61 reply seen by the worker");
        assert_eq!(rig.apc(|a| a.received.front().cloned()), Some(DEVICE_INQUIRY.to_vec()));
        assert_eq!(rig.apc(|a| a.mode()), Some(MODE_ABLETON));
        rig.worker.shutdown();
        assert_eq!(rig.apc(|a| a.mode()), Some(MODE_GENERIC), "given back at shutdown");
        assert_eq!(rig.apc(|a| a.led_at(0, 0)), Some(0), "LEDs switched off");
        assert!(!rig.sim.is_open(TEST_MK2_PORT));
    }

    #[test]
    fn simulated_apc40_is_detected_by_its_reply() {
        let sim = SimMidi::default();
        sim.plug("USB MIDI", FakeApc::new(Model::Apc40));
        let shared = Arc::new(Mutex::new(test_support::shared()));
        let mut w = Worker::new(sim.clone(), Arc::clone(&shared));
        let t0 = Instant::now();
        w.step(t0, None);
        w.step(t0 + ms(5), None);
        let d = shared.lock().unwrap().midi.devices[0].clone();
        assert_eq!((d.model, d.profile.as_str()), (Model::Apc40, "apc40"));
        assert_eq!(sim.with_device("USB MIDI", |a| a.mode()).unwrap(), Some(MODE_ABLETON));
    }

    #[test]
    fn pad_plays_a_grid_cue_and_leds_reach_the_device() {
        let mut rig = Rig::new(true);
        let pad = rig.apc(|a| a.pad(0, 1, true));
        rig.play(&pad);
        let active = rig.shared.lock().unwrap().active_cue.clone().expect("a cue plays");
        let second = {
            let s = rig.shared.lock().unwrap();
            s.presets.iter().filter(|p| p.category == crate::presets::CATEGORIES[0]).nth(1).map(|p| p.id.clone())
        };
        assert_eq!(Some(active), second, "pad (0,1) = second cue of page 1");
        assert_eq!(rig.shared.lock().unwrap().midi.last.as_ref().unwrap().msg, MidiMsg::NoteOn { channel: 0, note: 33, velocity: 127 });

        // LED feedback (T-205) will go through the sender: it reaches the pad.
        let sender = rig.shared.lock().unwrap().midi.sender.clone().unwrap();
        sender.send(TEST_MK2_PORT, &[0x90, 33, 21]);
        rig.worker.step(Instant::now(), None);
        assert_eq!(rig.apc(|a| a.led_at(0, 1)), Some(21));
    }

    #[test]
    fn stop_all_latches_the_estop_and_nothing_rearms() {
        let mut rig = Rig::new(true);
        rig.shared.lock().unwrap().request_arm(ArmSource::Ui).unwrap();
        let stop = rig.apc(|a| a.stop_all(true));
        rig.play(&stop);
        let s = rig.shared.lock().unwrap();
        assert!(!s.gate.is_armed());
        assert!(s.estop.is_latched());
    }

    #[test]
    fn shift_layer_and_fader_pickup() {
        let mut rig = Rig::new(true);
        let (scene, shift_on, shift_off) = rig.apc(|a| (a.button(NOTE_SCENE, true), a.shift(true), a.shift(false)));
        rig.play(&shift_on);
        rig.play(&scene);
        assert_eq!(rig.shared.lock().unwrap().cue_page, 1, "Shift + scene 1 = page 2");
        rig.play(&shift_off);
        rig.play(&[0x80, NOTE_SCENE, 0]);
        rig.play(&scene);
        assert_eq!(rig.shared.lock().unwrap().cue_page, 0, "scene 1 alone = page 1");

        // master.size 1.5 sits at 75 % of the fader; the device reported 0.
        rig.shared.lock().unwrap().live.size = 1.5;
        let low = rig.apc(|a| a.fader(0, 64));
        rig.play(&low);
        assert_eq!(rig.shared.lock().unwrap().live.size, 1.5, "not caught yet");
        let near = rig.apc(|a| a.fader(0, 96));
        rig.play(&near);
        let size = rig.shared.lock().unwrap().live.size;
        assert!((size - 2.0 * 96.0 / 127.0).abs() < 1e-4, "caught within 3 %: {size}");
    }

    #[test]
    fn injection_errors() {
        let sim = SimMidi::default();
        assert_eq!(sim.inject("nope", &[0x90, 0, 1]), Err(InjectError::NoSuchPort));
        sim.plug(TEST_MK2_PORT, FakeApc::new(Model::Apc40Mk2));
        assert_eq!(sim.inject(TEST_MK2_PORT, &[0x90, 0, 1]), Err(InjectError::NotOpen));
    }

    #[test]
    fn nothing_injected_arms_by_default() {
        let mut rig = Rig::new(true);
        rig.plugged_long_ago();
        // Every note and CC on 16 channels with Shift held, then Shift +
        // the arm button held 2 s.
        let shift = rig.apc(|a| a.shift(true));
        rig.play(&shift);
        for ch in 0..16u8 {
            for n in 0..128u8 {
                if n == NOTE_STOP_ALL || n == NOTE_SHIFT {
                    continue; // a blackout would hide an arming
                }
                rig.play(&[0x90 | ch, n, 127, 0xB0 | ch, n, 127]);
                rig.play(&[0x80 | ch, n, 0, 0xB0 | ch, n, 0]);
            }
        }
        rig.play(&[0x90, 0x5B, 127]);
        rig.frame_in(Duration::from_secs(2));
        let s = rig.shared.lock().unwrap();
        assert!(!s.gate.is_armed(), "allow_arm is off by default");
        assert!(!s.estop.is_latched());
    }

    #[test]
    fn opt_in_gesture_still_arms_through_the_simulator() {
        // Positive control for the test above: the same path works once
        // the user allows it, so the refusal really comes from the option.
        let mut rig = Rig::new(true);
        rig.plugged_long_ago();
        rig.shared.lock().unwrap().midi.store.devices.safety.allow_arm = true;
        let shift = rig.apc(|a| a.shift(true));
        rig.play(&shift);
        rig.play(&[0x90, 0x5B, 127]);
        rig.frame_in(ms(500));
        assert!(!rig.shared.lock().unwrap().gate.is_armed(), "0.5 s is not enough");
        rig.frame_in(ms(1100));
        assert!(rig.shared.lock().unwrap().gate.is_armed());
    }
}
