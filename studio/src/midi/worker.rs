//! The "midi" thread: opens every enabled input port (and the output of
//! the same name), identifies the device, applies its profile, and feeds
//! incoming events to the mapping engine (`engine::handle_batch`).
//!
//! midir has no hot-plug notification, so the port list is re-scanned
//! every 2 s. Backend calls (CoreMIDI) never happen under the `Shared`
//! lock, and the lock is taken once per batch of events: the 60 fps
//! engine thread never waits on MIDI. LED feedback (T-205, `led.rs`) is
//! rendered under the lock at most 30 times a second and sent after it.

use super::backend::{Backend, InputCallback, InputHandle, MidirBackend, OutputPort};
use super::decode::Decoder;
use super::detect::{self, Model, DEVICE_INQUIRY, MODE_GENERIC};
use super::led::{self, LedFrame, LedState};
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
/// A generic device with no LED to drive is looked at this often (a LED
/// added to one of its mappings shows within that time).
const GENERIC_IDLE: Duration = Duration::from_millis(250);

/// Starts the MIDI thread on CoreMIDI. Returns `None` (and logs) if the
/// thread can't be created; the studio then simply runs without MIDI.
pub fn spawn(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
    spawn_on(MidirBackend, shared, running)
}

/// Same on another backend: `--midi-test` runs it on simulated devices.
pub fn spawn_on<B: Backend + Send + 'static>(backend: B, shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || Worker::new(backend, shared).run(&running))
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
    /// What its LEDs show (T-205).
    leds: LedState,
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
            first = self.events.recv_timeout(self.wait(Instant::now())).ok();
        }
        self.shutdown();
    }

    /// How long to wait for an event: until the next LED update at most.
    fn wait(&self, now: Instant) -> Duration {
        let next_led = self.slots.values().filter(|s| s.profile.is_some()).filter_map(|s| s.leds.next_at()).min();
        next_led.map_or(IDLE_WAIT, |t| t.saturating_duration_since(now).clamp(Duration::from_millis(1), IDLE_WAIT))
    }

    /// One pass: events, commands, detection timeouts, rescan, profiles, LEDs.
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
        self.update_leds(now);
    }

    /// LED feedback: one lock for every device that is due, frames rendered
    /// from `Shared`, then only the differences sent, outside the lock.
    fn update_leds(&mut self, now: Instant) {
        // APCs once introduced; any other device once it has a profile
        // (generic LED feedback, T-211).
        let due: Vec<(String, Driver)> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.output.is_some() && slot.profile.is_some() && slot.leds.due(now))
            .map(|(name, slot)| (name.clone(), slot.introduced.map_or(Driver::Generic, |(driver, _)| driver)))
            .collect();
        if due.is_empty() {
            return;
        }
        let frames: Vec<(String, LedFrame)> = {
            let s = lock(&self.shared);
            let t = s.now_s();
            due.into_iter()
                .map(|(name, driver)| {
                    // « Retour LED » unticked: an empty frame switches off what we lit.
                    let frame = if s.midi.store.port_leds(&name) { led::render(driver, &s, &name, t) } else { LedFrame::default() };
                    (name, frame)
                })
                .collect()
        };
        for (name, frame) in frames {
            let Some(slot) = self.slots.get_mut(&name) else { continue };
            // Nothing lit nor to light (most generic devices): look again later.
            let idle = slot.introduced.is_none() && frame.0.is_empty() && slot.leds.last_sent.0.is_empty();
            let msgs = slot.leds.update(frame, now);
            if idle {
                slot.leds.pause_until(now + GENERIC_IDLE);
            }
            let Some(out) = slot.output.as_mut() else { continue };
            for msg in msgs {
                if let Err(e) = out.send(&msg) {
                    log::debug!("MIDI : LED vers {name} impossible : {e}");
                    break;
                }
            }
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
            let leds = midi.store.port_leds(&midi.devices[i].name);
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
            d.leds = leds;
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
        self.slots.insert(name.to_string(), Slot { _input: input, output, detecting, model, profile: None, introduced: None, leds: LedState::default() });
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
                        slot.leds.forget();
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

/// LEDs and knob rings off, then (APC) Introduction `0x40`: the device is
/// as we found it. A generic device only gets the LEDs we lit switched off.
fn goodbye(slot: &mut Slot) {
    let last = std::mem::take(&mut slot.leds.last_sent);
    slot.leds.forget();
    let introduced = slot.introduced.take();
    let Some(out) = slot.output.as_mut() else { return };
    let apc = introduced.and_then(|(driver, _)| driver.apc_pid().map(|pid| (driver, pid)));
    let extra = apc.map(|(driver, _)| detect::leds_off(driver.model())).unwrap_or_default();
    for msg in LedFrame::default().diff(&last).into_iter().chain(extra) {
        let _ = out.send(&msg);
    }
    if let Some((_, pid)) = apc {
        let _ = out.send(&detect::introduction(pid, MODE_GENERIC));
    }
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
    use crate::midi::testing::FakeApc;
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
        /// Every send checks this lock is free: CoreMIDI is never called
        /// while `Shared` is held (T-205).
        pub unlocked: Option<Arc<Mutex<Shared>>>,
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
            if let Some(shared) = &f.unlocked {
                assert!(shared.try_lock().is_ok(), "sent while holding the Shared lock");
            }
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

    /// Only the SysEx messages (inquiry, Introductions), without LEDs.
    fn sysex(sent: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        sent.into_iter().filter(|m| m.first() == Some(&0xF0)).collect()
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
        assert_eq!(sysex(fake.sent("USB MIDI Device")), vec![DEVICE_INQUIRY.to_vec(), introduction(PID_APC40_MK2, MODE_ABLETON)]);
        let sent = fake.sent("USB MIDI Device");
        assert_eq!(sent[1], introduction(PID_APC40_MK2, MODE_ABLETON), "Introduction before any LED");
        assert!(sent[2..].iter().all(|m| m.len() == 3), "then LEDs (T-205)");
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
        fake.plug("USB MIDI Keyboard", None);
        w.step(t0, None);
        w.step(t0 + ms(600), None);
        assert_eq!(fake.sent("USB MIDI Keyboard"), vec![DEVICE_INQUIRY.to_vec()]);
        let d = device(&shared, "USB MIDI Keyboard");
        assert_eq!((d.model, d.profile.as_str(), d.connected), (Model::Unknown, "generic", true));
    }

    /// T-211: a nanoKONTROL2 picks its starter template by name, gets its
    /// mapped button LEDs as plain CCs (no SysEx) and has them switched off
    /// when disabled.
    #[test]
    fn template_device_gets_generic_led_feedback() {
        const NANO: &str = "nanoKONTROL2 SLIDER/KNOB";
        let (mut w, fake, shared, t0) = setup();
        fake.plug(NANO, None);
        w.step(t0, None);
        w.step(t0 + ms(600), None);
        assert_eq!(device(&shared, NANO).profile, "nanokontrol2");
        let sent = fake.sent(NANO);
        assert_eq!(sysex(sent.clone()), vec![DEVICE_INQUIRY.to_vec()], "no APC Introduction");
        assert!(sent.contains(&vec![0xB0, 48, 0]), "M1 dark: layer 1 not muted");
        // M1 pressed: layer 1 muted, its LED lit within one LED period.
        fake.push(NANO, &[0xB0, 48, 127, 0xB0, 48, 0]);
        w.step(t0 + ms(700), None);
        assert!(shared.lock().unwrap().mixer.layer(1).mute);
        w.step(t0 + ms(800), None);
        assert_eq!(fake.sent(NANO).last(), Some(&vec![0xB0, 48, 127]));
        // Disabled: what we lit goes dark.
        shared.lock().unwrap().midi.store.set_port_enabled(NANO, false).unwrap();
        w.step(t0 + ms(1100), None);
        assert_eq!(fake.sent(NANO).last(), Some(&vec![0xB0, 48, 0]));
    }

    /// T-211: two controllers at once, each through its own profile: the
    /// same CC 1 moves a layer dimmer on one and the size on the other, and
    /// blackout works from either.
    #[test]
    fn two_controllers_each_drive_their_own_controls() {
        const NANO: &str = "nanoKONTROL2";
        const XTM: &str = "X-TOUCH MINI";
        let (mut w, fake, shared, t0) = setup();
        fake.plug(NANO, None);
        fake.plug(XTM, None);
        w.step(t0, None);
        w.step(t0 + ms(600), None);
        assert_eq!((device(&shared, NANO).profile, device(&shared, XTM).profile), ("nanokontrol2".into(), "x-touch-mini".into()));
        {
            let mut s = shared.lock().unwrap();
            s.mixer.layer_mut(2).dimmer = 0.5;
            s.live.size = 1.0;
        }
        // Both at mid-travel (pickup catches), then moved.
        fake.push(NANO, &[0xB0, 1, 64, 0xB0, 1, 100]);
        fake.push(XTM, &[0xB0, 1, 64, 0xB0, 1, 20]);
        w.step(t0 + ms(700), None);
        {
            let mut s = shared.lock().unwrap();
            crate::midi::engine::frame(&mut s, t0 + ms(710));
            assert!((s.mixer.layer(2).dimmer - 100.0 / 127.0).abs() < 1e-3, "{}", s.mixer.layer(2).dimmer);
            assert!((s.live.size - 2.0 * 20.0 / 127.0).abs() < 1e-3, "{}", s.live.size);
            s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        }
        fake.push(XTM, &[0x9A, 23, 127]); // lower button 8: blackout
        w.step(t0 + ms(720), None);
        assert!(!shared.lock().unwrap().gate.is_armed(), "blackout from the X-Touch Mini");
        {
            let mut guard = shared.lock().unwrap();
            let s = &mut *guard;
            s.gate.reset_estop(&s.estop);
            s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        }
        fake.push(NANO, &[0xB0, 42, 127]); // Stop: blackout
        w.step(t0 + ms(730), None);
        assert!(!shared.lock().unwrap().gate.is_armed(), "blackout from the nanoKONTROL2");
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
        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(2000), None);
        assert!(device(&shared, "APC40 mkII").lost, "banner shown");
        assert!(shared.lock().unwrap().gate.is_armed(), "state kept by default");

        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0 + ms(4000), None);
        assert!(!device(&shared, "APC40 mkII").lost, "banner gone once back");

        shared.lock().unwrap().midi.store.devices.safety.blackout_on_disconnect = true;
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(6000), None);
        assert!(!shared.lock().unwrap().gate.is_armed(), "opt-in blackout on disconnect");

        // Disabling a port by hand is not a loss.
        fake.plug("Other", None);
        w.step(t0 + ms(8000), None);
        shared.lock().unwrap().midi.store.set_port_enabled("Other", false).unwrap();
        {
            let mut g = shared.lock().unwrap();
            let s = &mut *g;
            s.gate.reset_estop(&s.estop);
        }
        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        w.step(t0 + ms(8300), None);
        w.step(t0 + ms(10000), None);
        assert!(!device(&shared, "Other").lost);
        assert!(shared.lock().unwrap().gate.is_armed());
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
        shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        fake.push("Pad", &[0xB0, 20, 127, 0x90, 81, 127]);
        w.step(t0 + ms(610), None);
        let mut s = shared.lock().unwrap();
        assert!(!s.gate.is_armed(), "blackout");
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
        assert!(!s.gate.is_armed());
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
        assert_eq!(sysex(fake.sent("APC40 mkII")).last().unwrap(), &introduction(PID_APC40_MK2, MODE_ABLETON));
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

    /// An introduced mkII with LED feedback, plugged at `t0`.
    fn led_setup() -> (Worker<FakeBackend>, FakeBackend, Arc<Mutex<Shared>>, Instant) {
        let (mut w, fake, shared, t0) = setup();
        fake.0.lock().unwrap().unlocked = Some(Arc::clone(&shared));
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        w.step(t0, None);
        w.step(t0 + ms(10), None);
        (w, fake, shared, t0)
    }

    fn leds(sent: &[Vec<u8>]) -> Vec<Vec<u8>> {
        sent.iter().filter(|m| m.len() == 3).cloned().collect()
    }

    #[test]
    fn leds_follow_the_studio_as_diffs_at_most_30_times_a_second() {
        let (mut w, fake, shared, t0) = led_setup();
        let first = leds(&fake.sent("APC40 mkII"));
        assert!(first.contains(&vec![0x90, 32, led::MK2_WHITE]), "full frame after the Introduction");
        assert!(first.contains(&vec![0x90, 0x52, led::MK2_GREEN]), "page 1 on Scene Launch 1");

        // A cue started from the UI: at the next update (≤ 33 ms later)
        // only what changed is sent.
        let cue = {
            let mut s = shared.lock().unwrap();
            let cat = crate::presets::CATEGORIES[0];
            let id = s.presets.iter().find(|p| p.category == cat).unwrap().id.clone();
            crate::controls::press_cue(&mut s, &id, None, true);
            id
        };
        assert!(!cue.is_empty());
        let before = fake.sent("APC40 mkII").len();
        w.step(t0 + ms(20), None);
        assert_eq!(fake.sent("APC40 mkII").len(), before, "not yet: 30 per second at most");
        w.step(t0 + ms(45), None);
        let new = leds(&fake.sent("APC40 mkII")[before..]);
        assert!(new.contains(&vec![0x90, 32, led::MK2_GREEN]), "{new:?}");
        assert!(new.len() < 10, "a diff, not a full frame: {new:?}");

        // Page change: the grid is redrawn at the next update.
        shared.lock().unwrap().cue_page = 1;
        let before = fake.sent("APC40 mkII").len();
        w.step(t0 + ms(80), None);
        let new = leds(&fake.sent("APC40 mkII")[before..]);
        assert!(new.contains(&vec![0x90, 0x53, led::MK2_GREEN]) && new.contains(&vec![0x90, 0x52, 0]), "{new:?}");

        // Idle: over two seconds, only the metronome LED (2 per beat at 120 BPM).
        let before = fake.sent("APC40 mkII").len();
        for i in 0..200 {
            w.step(t0 + ms(100 + i * 10), None);
        }
        let idle = leds(&fake.sent("APC40 mkII")[before..]);
        assert!(idle.iter().all(|m| m[1] == led::NOTE_METRONOME_MK2), "{idle:?}");
        assert!(idle.len() <= 10, "{idle:?}");
        assert!(w.wait(t0 + ms(2100)) <= IDLE_WAIT);
    }

    #[test]
    fn leds_can_be_turned_off_per_device() {
        let (mut w, fake, shared, t0) = led_setup();
        shared.lock().unwrap().midi.store.set_port_leds("APC40 mkII", false).unwrap();
        let before = fake.sent("APC40 mkII").len();
        w.step(t0 + ms(50), None);
        let off = leds(&fake.sent("APC40 mkII")[before..]);
        assert!(!off.is_empty() && off.iter().all(|m| m[2] == 0), "what was lit goes dark: {off:?}");
        let before = fake.sent("APC40 mkII").len();
        for i in 0..100 {
            w.step(t0 + ms(100 + i * 10), None);
        }
        assert_eq!(fake.sent("APC40 mkII").len(), before, "then nothing, not even the beat");
        w.step(t0 + ms(2100), None);
        assert!(!device(&shared, "APC40 mkII").leds, "shown in /api/midi after the next scan");
    }

    #[test]
    fn replug_resends_everything_and_leaving_clears_the_rings() {
        let (mut w, fake, shared, t0) = led_setup();
        shared.lock().unwrap().live.perspective = 1.0;
        w.step(t0 + ms(50), None);
        assert!(fake.sent("APC40 mkII").contains(&vec![0xB0, 0x36, 127]), "ring value");
        fake.unplug("APC40 mkII");
        w.step(t0 + ms(2000), None);
        fake.plug("APC40 mkII", Some(identity_reply(PID_APC40_MK2)));
        let before = fake.sent("APC40 mkII").len();
        w.step(t0 + ms(4000), None);
        w.step(t0 + ms(4010), None);
        let again = leds(&fake.sent("APC40 mkII")[before..]);
        assert!(again.len() > 60 && again.contains(&vec![0x90, 32, led::MK2_WHITE]), "full frame after a replug");

        w.shutdown();
        let sent = fake.sent("APC40 mkII");
        assert!(sent.contains(&vec![0xB0, 0x36, 0]), "ring cleared on the way out");
        assert_eq!(sent.last().unwrap(), &introduction(PID_APC40_MK2, MODE_GENERIC));
    }

    #[test]
    fn generic_devices_get_no_leds() {
        let (mut w, fake, _shared, t0) = setup();
        fake.plug("nano", None);
        for i in 0..100 {
            w.step(t0 + ms(i * 10), None);
        }
        assert_eq!(fake.sent("nano"), vec![DEVICE_INQUIRY.to_vec()]);
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
                let (i, o) = self.0.all_ports()?;
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

        // The fake device (T-209 `FakeApc`) behind two virtual ports: a
        // source (what the studio reads) and a destination that answers
        // like an APC40 mkII and records the rest.
        let source = Arc::new(Mutex::new(midir::MidiOutput::new("fake apc").unwrap().create_virtual(NAME).unwrap()));
        let apc = Arc::new(Mutex::new(FakeApc::new(Model::Apc40Mk2)));
        let _dest = midir::MidiInput::new("fake apc")
            .unwrap()
            .create_virtual(
                NAME,
                {
                    let (source, apc) = (Arc::clone(&source), Arc::clone(&apc));
                    move |_, bytes, _| {
                        for reply in apc.lock().unwrap().receive(bytes) {
                            source.lock().unwrap().send(&reply).unwrap();
                        }
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
        let pad = apc.lock().unwrap().pad(0, 0, true);
        source.lock().unwrap().send(&pad).unwrap();
        std::thread::sleep(ms(50));
        w.step(t0 + ms(20), None);
        let d = device(&shared, NAME);
        assert_eq!((d.model, d.profile.as_str(), d.connected), (Model::Apc40Mk2, "apc40-mk2", true));
        assert_eq!(d.faders, Some([0; 9]), "0x61 reply received");
        assert_eq!(shared.lock().unwrap().midi.last.as_ref().unwrap().msg, MidiMsg::NoteOn { channel: 0, note: 0x20, velocity: 127 });
        assert_eq!(apc.lock().unwrap().mode(), Some(MODE_ABLETON));

        // A LED sent to the pad reaches the device.
        shared.lock().unwrap().midi.sender.clone().unwrap().send(NAME, &[0x90, 0x20, 21]);
        w.step(t0 + ms(30), None);
        std::thread::sleep(ms(50));
        assert_eq!(apc.lock().unwrap().led_at(0, 0), Some(21));

        w.shutdown();
        std::thread::sleep(ms(50));
        assert_eq!(apc.lock().unwrap().mode(), Some(MODE_GENERIC));
    }

    /// Hot-plug over real CoreMIDI: a port that appears and disappears is
    /// seen by the next listing (virtual port only, nothing is opened).
    #[test]
    #[ignore]
    fn midi_virtual_port_hot_plug_is_seen() {
        use midir::os::unix::VirtualOutput;
        const NAME: &str = "Laser Studio Test Hotplug";
        let mut backend = MidirBackend;
        assert!(!backend.all_ports().unwrap().0.contains(&NAME.to_string()));
        let port = midir::MidiOutput::new("fake").unwrap().create_virtual(NAME).unwrap();
        std::thread::sleep(ms(50));
        assert!(backend.all_ports().unwrap().0.contains(&NAME.to_string()), "plugged");
        assert!(!backend.ports().unwrap().0.contains(&NAME.to_string()), "hidden from a running studio");
        drop(port);
        std::thread::sleep(ms(50));
        assert!(!backend.all_ports().unwrap().0.contains(&NAME.to_string()), "unplugged");
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
