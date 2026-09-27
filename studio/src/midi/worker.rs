//! The "midi" thread: opens every enabled input port (and the output of
//! the same name), identifies the device, applies its profile, and feeds
//! incoming events to the mapping engine (`engine::handle_batch`).
//!
//! midir has no hot-plug notification, so the port list is re-scanned
//! every 2 s. Backend calls (CoreMIDI) never happen under the `Shared`
//! lock, and the lock is taken once per batch of events: the 60 fps
//! engine thread never waits on MIDI.

use super::backend::{Backend, InputCallback, InputHandle, MidirBackend, OutputPort};
use super::decode::Decoder;
use super::detect::{self, Model, DEVICE_INQUIRY, MODE_GENERIC};
use super::profile::Driver;
use super::{Command, MidiEvent, MidiMsg, MidiSender};
use crate::Shared;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const SCAN_EVERY: Duration = Duration::from_secs(2);
pub const INQUIRY_TIMEOUT: Duration = Duration::from_millis(500);
/// How often profile / enabled changes made from the UI are picked up.
const RECONCILE_EVERY: Duration = Duration::from_millis(250);
/// Longest wait for an event before doing periodic work.
const IDLE_WAIT: Duration = Duration::from_millis(20);

/// Starts the MIDI thread on CoreMIDI. Returns `None` (and logs) if the
/// thread can't be created; the studio then simply runs without MIDI.
pub fn spawn(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || Worker::new(MidirBackend, shared).run(&running))
        .map_err(|e| log::warn!("MIDI : impossible de démarrer le thread : {e}"))
        .ok()
}

struct Slot {
    _input: InputHandle,
    output: Option<Box<dyn OutputPort>>,
    /// Waiting for the Device Inquiry reply until then.
    detecting: Option<Instant>,
    model: Model,
    /// Profile slug applied to the device.
    profile: Option<String>,
    /// APC driver and Introduction mode we took the device over with.
    introduced: Option<(Driver, u8)>,
}

enum Plan {
    Close,
    Apply { slug: String, want: Option<(Driver, u8)> },
}

pub struct Worker<B: Backend> {
    backend: B,
    shared: Arc<Mutex<Shared>>,
    events_tx: Sender<MidiEvent>,
    events: Receiver<MidiEvent>,
    commands: Receiver<Command>,
    slots: BTreeMap<String, Slot>,
    /// Ports that failed to open, so the warning is logged once.
    failed: HashSet<String>,
    next_scan: Instant,
    next_reconcile: Instant,
}

impl<B: Backend> Worker<B> {
    pub fn new(backend: B, shared: Arc<Mutex<Shared>>) -> Self {
        let (events_tx, events) = channel();
        let (commands_tx, commands) = channel();
        lock(&shared).midi.sender = Some(MidiSender(commands_tx));
        let now = Instant::now();
        Worker { backend, shared, events_tx, events, commands, slots: BTreeMap::new(), failed: HashSet::new(), next_scan: now, next_reconcile: now }
    }

    pub fn run(mut self, running: &AtomicBool) {
        let mut first = None;
        while running.load(Ordering::SeqCst) {
            self.step(Instant::now(), first.take());
            first = self.events.recv_timeout(IDLE_WAIT).ok();
        }
        self.shutdown();
    }

    /// One pass: events, commands, detection timeouts, rescan, profiles.
    pub fn step(&mut self, now: Instant, first: Option<MidiEvent>) {
        let events: Vec<MidiEvent> = first.into_iter().chain(self.events.try_iter()).collect();
        while let Ok(Command::Send { port, bytes }) = self.commands.try_recv() {
            self.send(&port, &bytes);
        }

        let mut faders = Vec::new();
        for ev in &events {
            let MidiMsg::SysEx { bytes } = &ev.msg else { continue };
            if let Some(slot) = self.slots.get_mut(&ev.port) {
                if slot.detecting.is_some() {
                    if let Some(id) = detect::parse_identity(bytes) {
                        let model = Model::from_identity(&id);
                        slot.model = if model == Model::Unknown { Model::from_port_name(&ev.port) } else { model };
                        slot.detecting = None;
                        self.next_reconcile = now;
                        log::info!("MIDI : {} identifié : {} (id produit {:#04x})", ev.port, slot.model.label(), id.product);
                    }
                }
            }
            if let Some(f) = detect::parse_fader_positions(bytes) {
                faders.push((ev.port.clone(), f));
            }
        }
        for (name, slot) in &mut self.slots {
            if slot.detecting.is_some_and(|until| now >= until) {
                slot.model = Model::from_port_name(name);
                slot.detecting = None;
                self.next_reconcile = now;
                log::info!("MIDI : {name} ne répond pas à l'identification, modèle d'après le nom : {}", slot.model.label());
            }
        }

        if !events.is_empty() {
            let mut s = lock(&self.shared);
            for (port, f) in faders {
                s.midi.device_mut(&port).faders = Some(f);
            }
            for ev in &events {
                s.midi.record(ev);
            }
            super::engine::handle_batch(&mut s, &events);
        }

        if now >= self.next_scan {
            self.scan(now);
            self.next_scan = now + SCAN_EVERY;
            self.next_reconcile = now;
        }
        if now >= self.next_reconcile {
            self.reconcile();
            self.next_reconcile = now + RECONCILE_EVERY;
        }
    }

    fn scan(&mut self, now: Instant) {
        let (inputs, outputs) = match self.backend.ports() {
            Ok(p) => p,
            Err(e) => {
                let mut s = lock(&self.shared);
                if s.midi.error.as_ref() != Some(&e) {
                    log::warn!("MIDI : {e}");
                    s.midi.error = Some(e);
                }
                return;
            }
        };

        let gone: Vec<String> = self.slots.keys().filter(|n| !inputs.contains(n)).cloned().collect();
        for name in gone {
            self.slots.remove(&name);
            log::info!("MIDI : {name} déconnecté");
        }
        self.failed.retain(|n| inputs.contains(n));

        let enabled: Vec<bool> = {
            let s = lock(&self.shared);
            inputs.iter().map(|n| s.midi.store.port_enabled(n)).collect()
        };
        for (name, enabled) in inputs.iter().zip(enabled) {
            if enabled && !self.slots.contains_key(name) {
                self.open(name, outputs.contains(name), now);
            }
        }

        let mut s = lock(&self.shared);
        let midi = &mut s.midi;
        midi.error = None;
        for name in inputs.iter().chain(&outputs) {
            midi.device_mut(name);
        }
        let mut unplugged = Vec::new();
        for i in 0..midi.devices.len() {
            let enabled = midi.store.port_enabled(&midi.devices[i].name);
            let d = &mut midi.devices[i];
            let connected = self.slots.contains_key(&d.name);
            if connected && !d.connected {
                d.connected_at = Some(now);
                d.lost = false;
            } else if !connected {
                if d.connected {
                    d.lost = true;
                    unplugged.push(d.name.clone());
                }
                d.connected_at = None;
            }
            d.input = inputs.contains(&d.name);
            d.output = outputs.contains(&d.name);
            d.connected = connected;
            d.enabled = enabled;
        }
        for name in unplugged {
            log::warn!("MIDI : contrôleur {name} déconnecté");
            super::engine::port_closed(&mut s, &name, true);
        }
    }

    fn open(&mut self, name: &str, has_output: bool, now: Instant) {
        let tx = self.events_tx.clone();
        let port = name.to_string();
        let mut decoder = Decoder::default();
        // Runs on a CoreMIDI thread: decode and push, nothing else (no lock).
        let callback: InputCallback = Box::new(move |bytes| {
            decoder.feed(bytes, |msg| {
                let _ = tx.send(MidiEvent { port: port.clone(), msg, at: Instant::now() });
            })
        });
        let input = match self.backend.open_input(name, callback) {
            Ok(h) => h,
            Err(e) => {
                if self.failed.insert(name.to_string()) {
                    log::warn!("MIDI : impossible d'ouvrir {name} : {e}");
                }
                return;
            }
        };
        self.failed.remove(name);
        let mut output = if has_output {
            self.backend.open_output(name).map_err(|e| log::warn!("MIDI : sortie {name} indisponible : {e}")).ok()
        } else {
            None
        };
        let detecting = output.as_mut().and_then(|out| out.send(&DEVICE_INQUIRY).ok()).map(|_| now + INQUIRY_TIMEOUT);
        let model = if detecting.is_some() { Model::Unknown } else { Model::from_port_name(name) };
        log::info!("MIDI : {name} connecté");
        self.slots.insert(name.to_string(), Slot { _input: input, output, detecting, model, profile: None, introduced: None });
    }

    /// Applies UI changes (enabled, chosen profile) and the profile of
    /// newly identified devices.
    fn reconcile(&mut self) {
        let plans: Vec<(String, Plan)> = {
            let mut s = lock(&self.shared);
            let midi = &mut s.midi;
            let mut plans = Vec::new();
            let mut closed = Vec::new();
            for (name, slot) in &self.slots {
                if !midi.store.port_enabled(name) {
                    let d = midi.device_mut(name);
                    d.enabled = false;
                    d.connected = false;
                    d.connected_at = None;
                    plans.push((name.clone(), Plan::Close));
                    closed.push(name.clone());
                    continue;
                }
                if slot.detecting.is_some() {
                    continue;
                }
                let choice = midi.store.choose(name, slot.model);
                let (driver, mode) = midi.store.get(&choice.slug).map_or((Driver::Generic, MODE_GENERIC), |p| (p.driver, p.host_mode));
                let want = driver.apc_pid().map(|_| (driver, mode));
                let d = midi.device_mut(name);
                d.model = slot.model;
                d.model_label = slot.model.label();
                d.profile_error = choice.error;
                d.profile = choice.slug.clone();
                if slot.profile.as_deref() != Some(choice.slug.as_str()) || slot.introduced != want {
                    plans.push((name.clone(), Plan::Apply { slug: choice.slug, want }));
                }
            }
            for name in closed {
                super::engine::port_closed(&mut s, &name, false);
            }
            plans
        };

        for (name, plan) in plans {
            match plan {
                Plan::Close => {
                    if let Some(mut slot) = self.slots.remove(&name) {
                        goodbye(&mut slot);
                        log::info!("MIDI : {name} désactivé");
                    }
                }
                Plan::Apply { slug, want } => {
                    let Some(slot) = self.slots.get_mut(&name) else { continue };
                    if slot.introduced != want {
                        goodbye(slot);
                        if let (Some((driver, mode)), Some(out)) = (want, slot.output.as_mut()) {
                            if let Some(pid) = driver.apc_pid() {
                                let _ = out.send(&detect::introduction(pid, mode));
                            }
                        }
                        slot.introduced = want;
                    }
                    log::info!("MIDI : {name} utilise le profil « {slug} »");
                    slot.profile = Some(slug);
                }
            }
        }
    }

    fn send(&mut self, port: &str, bytes: &[u8]) {
        if let Some(out) = self.slots.get_mut(port).and_then(|s| s.output.as_mut()) {
            if let Err(e) = out.send(bytes) {
                log::debug!("MIDI : envoi vers {port} impossible : {e}");
            }
        }
    }

    /// Leaves every APC dark and back in its own Generic mode.
    pub fn shutdown(&mut self) {
        for slot in self.slots.values_mut() {
            goodbye(slot);
        }
        self.slots.clear();
    }
}

/// LEDs off, then Introduction `0x40`: the device is as we found it.
fn goodbye(slot: &mut Slot) {
    let Some((driver, _)) = slot.introduced.take() else { return };
    let (Some(out), Some(pid)) = (slot.output.as_mut(), driver.apc_pid()) else { return };
    for msg in detect::leds_off(driver.model()) {
        let _ = out.send(&msg);
    }
    let _ = out.send(&detect::introduction(pid, MODE_GENERIC));
}

/// A panic elsewhere must not take MIDI down with it (and vice versa: the
/// worker never panics while holding the lock).
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::midi::detect::{introduction, MODE_ABLETON, PID_APC40, PID_APC40_MK2};
    use crate::test_support;
    use std::collections::HashMap;

    /// A pretend CoreMIDI: tests add and remove ports, push bytes from a
    /// "device", and read what the studio sent. Optionally answers the
    /// Device Inquiry like a real APC.
    #[derive(Default)]
    pub struct Fake {
        pub inputs: Vec<String>,
        pub outputs: Vec<String>,
        pub callbacks: HashMap<String, InputCallback>,
        pub sent: Vec<(String, Vec<u8>)>,
        pub replies: HashMap<String, Vec<u8>>,
        pub fail_ports: bool,
        pub fail_open: bool,
    }

    #[derive(Clone, Default)]
    pub struct FakeBackend(pub Arc<Mutex<Fake>>);

    impl FakeBackend {
        pub fn plug(&self, name: &str, reply: Option<Vec<u8>>) {
            let mut f = self.0.lock().unwrap();
            f.inputs.push(name.into());
            f.outputs.push(name.into());
            if let Some(r) = reply {
                f.replies.insert(name.into(), r);
            }
        }

        pub fn unplug(&self, name: &str) {
            let mut f = self.0.lock().unwrap();
            f.inputs.retain(|n| n != name);
            f.outputs.retain(|n| n != name);
        }

        pub fn push(&self, name: &str, bytes: &[u8]) {
            let mut f = self.0.lock().unwrap();
            (f.callbacks.get_mut(name).expect("port open"))(bytes);
        }

        pub fn sent(&self, name: &str) -> Vec<Vec<u8>> {
            self.0.lock().unwrap().sent.iter().filter(|(n, _)| n == name).map(|(_, b)| b.clone()).collect()
        }

        pub fn is_open(&self, name: &str) -> bool {
            self.0.lock().unwrap().callbacks.contains_key(name)
        }
    }

    struct FakeInput(Arc<Mutex<Fake>>, String);

    impl Drop for FakeInput {
        fn drop(&mut self) {
            self.0.lock().unwrap().callbacks.remove(&self.1);
        }
    }

    struct FakeOutput(Arc<Mutex<Fake>>, String);

    impl OutputPort for FakeOutput {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            let mut f = self.0.lock().unwrap();
            f.sent.push((self.1.clone(), bytes.to_vec()));
            if bytes == DEVICE_INQUIRY {
                if let Some(reply) = f.replies.get(&self.1).cloned() {
                    if let Some(cb) = f.callbacks.get_mut(&self.1) {
                        cb(&reply);
                    }
                }
            }
            Ok(())
        }
    }

    impl Backend for FakeBackend {
        fn ports(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
            let f = self.0.lock().unwrap();
            if f.fail_ports {
                return Err("CoreMIDI indisponible : test".into());
            }
            Ok((f.inputs.clone(), f.outputs.clone()))
        }

        fn open_input(&mut self, name: &str, callback: InputCallback) -> Result<InputHandle, String> {
            let mut f = self.0.lock().unwrap();
            if f.fail_open {
                return Err("occupé".into());
            }
            f.callbacks.insert(name.into(), callback);
            Ok(Box::new(FakeInput(Arc::clone(&self.0), name.into())))
        }

        fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputPort>, String> {
            Ok(Box::new(FakeOutput(Arc::clone(&self.0), name.into())))
        }
    }

    pub fn identity_reply(pid: u8) -> Vec<u8> {
        vec![0xF0, 0x7E, 0x00, 0x06, 0x02, 0x47, pid, 0x00, 0x19, 0x00, 0x01, 0x00, 0x00, 0x7F, 0x00, 0x00, 0x00, 0x00, 0xF7]
    }

    fn setup() -> (Worker<FakeBackend>, FakeBackend, Arc<Mutex<Shared>>, Instant) {
        let fake = FakeBackend::default();
        let shared = Arc::new(Mutex::new(test_support::shared()));
        let worker = Worker::new(fake.clone(), Arc::clone(&shared));
        (worker, fake, shared, Instant::now())
    }

    fn device(shared: &Arc<Mutex<Shared>>, name: &str) -> crate::midi::MidiDevice {
        shared.lock().unwrap().midi.devices.iter().find(|d| d.name == name).cloned().expect("device listed")
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn apc40_mk2_is_identified_by_the_inquiry_and_introduced() {
        let (mut w, fake, shared, t0) = setup();
        // A neutral port name: the inquiry reply alone identifies it.
        fake.plug("USB MIDI Device", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        w.step(t0 + ms(10), None);
        assert_eq!(fake.sent("USB MIDI Device"), vec![DEVICE_INQUIRY.to_vec(), introduction(PID_APC40_MK2, MODE_ABLETON)]);
        let d = device(&shared, "USB MIDI Device");
        assert_eq!((d.model, d.profile.as_str(), d.connected, d.input, d.output), (Model::Apc40Mk2, "apc40-mk2", true, true, true));
        assert!(d.connected_at.is_some());
    }

    #[test]
    fn silent_apc40_falls_back_to_its_port_name() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("Akai APC40", None);
        w.step(t0, None);
        w.step(t0 + ms(300), None);
        assert_eq!(fake.sent("Akai APC40").len(), 1, "only the inquiry while waiting");
        assert_eq!(device(&shared, "Akai APC40").model, Model::Unknown);
        w.step(t0 + ms(600), None);
        assert_eq!(fake.sent("Akai APC40")[1], introduction(PID_APC40, MODE_ABLETON));
        let d = device(&shared, "Akai APC40");
        assert_eq!((d.model, d.profile.as_str()), (Model::Apc40, "apc40"));
    }

    #[test]
    fn unknown_controller_gets_generic_and_no_apc_sysex() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("nanoKONTROL2 SLIDER/KNOB", None);
        w.step(t0, None);
        w.step(t0 + ms(600), None);
        assert_eq!(fake.sent("nanoKONTROL2 SLIDER/KNOB"), vec![DEVICE_INQUIRY.to_vec()]);
        let d = device(&shared, "nanoKONTROL2 SLIDER/KNOB");
        assert_eq!((d.model, d.profile.as_str(), d.connected), (Model::Unknown, "generic", true));
    }

    #[test]
    fn hot_plug_unplug_and_replug() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        assert!(fake.is_open("APC40 mkII"));
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(1000), None);
        assert!(device(&shared, "APC40 mkII").connected, "not rescanned yet");
        w.step(t0 + ms(2000), None);
        let d = device(&shared, "APC40 mkII");
        assert!(!d.connected && !d.input && d.connected_at.is_none(), "still listed, disconnected");
        assert!(!fake.is_open("APC40 mkII"), "connection dropped");

        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0 + ms(4000), None);
        w.step(t0 + ms(4010), None);
        assert!(device(&shared, "APC40 mkII").connected, "back within one scan (2 s)");
        let intro = introduction(PID_APC40_MK2, MODE_ABLETON);
        assert_eq!(fake.sent("APC40 mkII").iter().filter(|b| **b == intro).count(), 2, "introduced again");
    }

    #[test]
    fn unplugging_flags_the_device_and_optionally_blacks_out() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        shared.lock().unwrap().armed = true;
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(2000), None);
        assert!(device(&shared, "APC40 mkII").lost, "banner shown");
        assert!(shared.lock().unwrap().armed, "state kept by default");

        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0 + ms(4000), None);
        assert!(!device(&shared, "APC40 mkII").lost, "banner gone once back");

        shared.lock().unwrap().midi.store.devices.safety.blackout_on_disconnect = true;
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(6000), None);
        assert!(!shared.lock().unwrap().armed, "opt-in blackout on disconnect");

        // Disabling a port by hand is not a loss.
        fake.plug("Other", None);
        w.step(t0 + ms(8000), None);
        shared.lock().unwrap().midi.store.set_port_enabled("Other", false).unwrap();
        shared.lock().unwrap().armed = true;
        w.step(t0 + ms(8300), None);
        w.step(t0 + ms(10000), None);
        assert!(!device(&shared, "Other").lost);
        assert!(shared.lock().unwrap().armed);
    }

    #[test]
    fn mapped_events_drive_controls_through_the_worker() {
        let (mut w, fake, shared, t0) = setup();
        {
            let mut s = shared.lock().unwrap();
            let p = crate::midi::profile::Profile::parse(
                r#"{ "name": "t", "match": { "port_contains": ["Pad"] },
                    "mappings": [ { "input": { "kind": "note", "number": 81 }, "target": "transport.blackout", "mode": "trigger" },
                                  { "input": { "kind": "cc", "number": 20 }, "target": "master.size", "mode": "absolute" } ] }"#,
            )
            .unwrap();
            s.midi.store.save_profile(None, "pad", p).unwrap();
        }
        fake.plug("Pad", None);
        w.step(t0, None);
        w.step(t0 + ms(600), None); // no inquiry reply: name fallback, profile applied
        assert_eq!(device(&shared, "Pad").profile, "pad");
        shared.lock().unwrap().armed = true;
        fake.push("Pad", &[0xB0, 20, 127, 0x90, 81, 127]);
        w.step(t0 + ms(610), None);
        let mut s = shared.lock().unwrap();
        assert!(!s.armed, "blackout");
        crate::midi::engine::frame(&mut s, t0 + ms(620));
        assert_eq!(s.live.size, 2.0, "fader written at the engine frame");
    }

    #[test]
    fn events_are_recorded_and_never_arm() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", None);
        w.step(t0, None);
        fake.push("APC40 mkII", &[0x90, 0x51, 0x7F, 0xF8, 0xB0, 0x0E, 0x7F]);
        w.step(t0 + ms(5), None);
        let s = shared.lock().unwrap();
        assert_eq!(s.midi.recent.len(), 2, "clock not recorded");
        assert_eq!(s.midi.last.as_ref().unwrap().msg, MidiMsg::Cc { channel: 0, number: 0x0E, value: 0x7F });
        assert_eq!(s.midi.last.as_ref().unwrap().port, "APC40 mkII");
        assert!(!s.armed);
    }

    #[test]
    fn mk2_fader_positions_are_kept() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        let mut reply = vec![0xF0, 0x47, 0x00, 0x29, 0x61, 0x00, 0x09, 1, 2, 3, 4, 5, 6, 7, 8, 127];
        reply.push(0xF7);
        fake.push("APC40 mkII", &reply);
        w.step(t0 + ms(5), None);
        assert_eq!(device(&shared, "APC40 mkII").faders, Some([1, 2, 3, 4, 5, 6, 7, 8, 127]));
    }

    #[test]
    fn disabled_ports_are_not_opened_and_disabling_releases_the_apc() {
        let (mut w, fake, shared, t0) = setup();
        shared.lock().unwrap().midi.store.set_port_enabled("Other", false).unwrap();
        fake.plug("Other", None);
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        assert!(!fake.is_open("Other"));
        let other = device(&shared, "Other");
        assert!(!other.enabled && !other.connected && other.input);

        w.step(t0 + ms(10), None);
        shared.lock().unwrap().midi.store.set_port_enabled("APC40 mkII", false).unwrap();
        w.step(t0 + ms(300), None);
        assert!(!fake.is_open("APC40 mkII"));
        let sent = fake.sent("APC40 mkII");
        assert_eq!(sent.last().unwrap(), &introduction(PID_APC40_MK2, MODE_GENERIC), "given back in Generic mode");
        assert!(sent.contains(&vec![0x90, 0x00, 0x00]), "LEDs turned off first");
        assert!(!device(&shared, "APC40 mkII").connected);
    }

    #[test]
    fn changing_the_profile_from_the_ui_is_applied() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        w.step(t0 + ms(10), None);
        shared.lock().unwrap().midi.store.set_port_profile("APC40 mkII", Some("generic")).unwrap();
        w.step(t0 + ms(300), None);
        assert_eq!(device(&shared, "APC40 mkII").profile, "generic");
        assert_eq!(fake.sent("APC40 mkII").last().unwrap(), &introduction(PID_APC40_MK2, MODE_GENERIC));
        shared.lock().unwrap().midi.store.set_port_profile("APC40 mkII", None).unwrap();
        w.step(t0 + ms(600), None);
        assert_eq!(fake.sent("APC40 mkII").last().unwrap(), &introduction(PID_APC40_MK2, MODE_ABLETON));
    }

    #[test]
    fn shutdown_turns_leds_off_and_restores_generic_mode() {
        let (mut w, fake, _shared, t0) = setup();
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        fake.plug("nano", None);
        w.step(t0, None);
        w.step(t0 + ms(600), None);
        w.shutdown();
        let sent = fake.sent("APC40 mkII");
        assert_eq!(sent.last().unwrap(), &introduction(PID_APC40_MK2, MODE_GENERIC));
        assert!(sent.contains(&vec![0x90, 0x27, 0x00]));
        assert_eq!(fake.sent("nano").len(), 1, "nothing but the inquiry for a generic device");
        assert!(!fake.is_open("APC40 mkII"));
    }

    #[test]
    fn sender_reaches_the_output_port() {
        let (mut w, fake, shared, t0) = setup();
        fake.plug("APC40 mkII", None);
        w.step(t0, None);
        let sender = shared.lock().unwrap().midi.sender.clone().unwrap();
        sender.send("APC40 mkII", &[0x90, 0x20, 0x05]);
        sender.send("Nobody", &[0x90, 0x20, 0x05]);
        w.step(t0 + ms(5), None);
        assert_eq!(fake.sent("APC40 mkII").last().unwrap(), &vec![0x90, 0x20, 0x05]);
    }

    /// Real CoreMIDI, but only on virtual ports this test creates: the
    /// backend is filtered so the user's own controller is never opened.
    /// Run with `cargo test -p laser-studio -- --ignored midi_virtual`.
    #[test]
    #[ignore]
    fn midi_virtual_apc40_mk2_is_detected_over_coremidi() {
        use midir::os::unix::{VirtualInput, VirtualOutput};
        const NAME: &str = "Laser Studio Test APC40 mkII";

        struct OnlyTestPorts(MidirBackend);
        impl Backend for OnlyTestPorts {
            fn ports(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
                let (i, o) = self.0.ports()?;
                Ok((i.into_iter().filter(|n| n == NAME).collect(), o.into_iter().filter(|n| n == NAME).collect()))
            }
            fn open_input(&mut self, name: &str, cb: InputCallback) -> Result<InputHandle, String> {
                assert_eq!(name, NAME);
                self.0.open_input(name, cb)
            }
            fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputPort>, String> {
                assert_eq!(name, NAME);
                self.0.open_output(name)
            }
        }

        // The fake device: a source (what the studio reads) and a
        // destination that answers the inquiry and records the rest.
        let source = Arc::new(Mutex::new(midir::MidiOutput::new("fake apc").unwrap().create_virtual(NAME).unwrap()));
        let received = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let _dest = midir::MidiInput::new("fake apc")
            .unwrap()
            .create_virtual(
                NAME,
                {
                    let (source, received) = (Arc::clone(&source), Arc::clone(&received));
                    move |_, bytes, _| {
                        if bytes == DEVICE_INQUIRY {
                            source.lock().unwrap().send(&identity_reply(PID_APC40_MK2)).unwrap();
                        }
                        received.lock().unwrap().push(bytes.to_vec());
                    }
                },
                (),
            )
            .unwrap();

        let shared = Arc::new(Mutex::new(test_support::shared()));
        let mut w = Worker::new(OnlyTestPorts(MidirBackend), Arc::clone(&shared));
        let t0 = Instant::now();
        w.step(t0, None);
        for _ in 0..50 {
            std::thread::sleep(ms(10));
            w.step(t0 + ms(10), None);
        }
        source.lock().unwrap().send(&[0x90, 0x20, 0x7F]).unwrap();
        std::thread::sleep(ms(50));
        w.step(t0 + ms(20), None);
        let d = device(&shared, NAME);
        assert_eq!((d.model, d.profile.as_str(), d.connected), (Model::Apc40Mk2, "apc40-mk2", true));
        assert_eq!(shared.lock().unwrap().midi.last.as_ref().unwrap().msg, MidiMsg::NoteOn { channel: 0, note: 0x20, velocity: 127 });
        w.shutdown();
        std::thread::sleep(ms(50));
        let got = received.lock().unwrap();
        assert!(got.contains(&introduction(PID_APC40_MK2, MODE_ABLETON)));
        assert_eq!(got.last().unwrap(), &introduction(PID_APC40_MK2, MODE_GENERIC));
    }

    /// Hot-plug over real CoreMIDI: a port that appears and disappears is
    /// seen by the next listing (virtual port only, nothing is opened).
    #[test]
    #[ignore]
    fn midi_virtual_port_hot_plug_is_seen() {
        use midir::os::unix::VirtualOutput;
        const NAME: &str = "Laser Studio Test Hotplug";
        let mut backend = MidirBackend;
        assert!(!backend.ports().unwrap().0.contains(&NAME.to_string()));
        let port = midir::MidiOutput::new("fake").unwrap().create_virtual(NAME).unwrap();
        std::thread::sleep(ms(50));
        assert!(backend.ports().unwrap().0.contains(&NAME.to_string()), "plugged");
        drop(port);
        std::thread::sleep(ms(50));
        assert!(!backend.ports().unwrap().0.contains(&NAME.to_string()), "unplugged");
    }

    #[test]
    fn backend_failures_never_panic() {
        let (mut w, fake, shared, t0) = setup();
        fake.0.lock().unwrap().fail_ports = true;
        w.step(t0, None);
        assert!(shared.lock().unwrap().midi.error.is_some());
        {
            let mut f = fake.0.lock().unwrap();
            f.fail_ports = false;
            f.fail_open = true;
        }
        fake.plug("APC40 mkII", None);
        w.step(t0 + ms(2000), None);
        let d = device(&shared, "APC40 mkII");
        assert!(d.input && !d.connected);
        assert!(shared.lock().unwrap().midi.error.is_none());
        fake.0.lock().unwrap().fail_open = false;
        w.step(t0 + ms(4000), None);
        assert!(device(&shared, "APC40 mkII").connected, "retried on the next scan");
    }
}
