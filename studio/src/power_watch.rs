//! Battery and overheating warnings during a show (T-495).
//!
//! An unplugged MacBook, or one that heats up (sun, closed booth), slows
//! down: frames arrive late, then at critical battery the system puts the
//! Mac to sleep, which the assertion of `keep_awake.rs` cannot prevent.
//! Every 10 s a thread of its own reads the power source and battery level
//! (IOKit `IOPSCopyPowerSourcesInfo` / `IOPSGetProvidingPowerSourceType`,
//! public API) and the thermal state (`NSProcessInfo.thermalState`, public
//! API), and `classify` turns each change into a warning for the operator:
//! the notification centre (`notices.rs`), a pill in the top bar, a line
//! in the application log and in the safety log.
//!
//! Information only: nothing here arms, disarms, blanks, cuts the output
//! or touches the brightness (a test reads this file to make sure). The
//! engine never reads the system: `/api/state` takes the last snapshot.
//! On other systems, or when a call fails, the state is « inconnu » and
//! nothing is said.
//!
//! Tests and e2e never read the real machine: a fake source, or
//! `POST /api/test/power` (`--test-hooks`) that sets the state.

use crate::notices::Level;
use crate::safety_log::SafetyLog;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How often the system is read.
const PERIOD: Duration = Duration::from_secs(10);
/// Battery at or below this: « Batterie faible » (orange).
pub const LOW_PCT: u8 = 20;
/// Battery at or below this: the Mac may go to sleep soon (red).
pub const CRITICAL_PCT: u8 = 10;
/// The notification centre's source for these notices.
pub const SOURCE: &str = "Mac";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    #[default]
    Unknown,
    Ac,
    Battery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Thermal {
    #[default]
    Unknown,
    Nominal,
    Fair,
    Serious,
    Critical,
}

/// What the last reading found (`/api/state` → `power_watch`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerStatus {
    pub source: PowerSource,
    /// The internal battery's charge, when there is one.
    pub battery_pct: Option<u8>,
    pub thermal: Thermal,
}

/// How bad the battery side is; only grows while on battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BatteryBand {
    Normal,
    OnBattery,
    Low,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ThermalBand {
    Normal,
    Serious,
    Critical,
}

impl PowerStatus {
    fn battery_band(&self) -> BatteryBand {
        match (self.source, self.battery_pct) {
            (PowerSource::Battery, Some(p)) if p <= CRITICAL_PCT => BatteryBand::Critical,
            (PowerSource::Battery, Some(p)) if p <= LOW_PCT => BatteryBand::Low,
            (PowerSource::Battery, _) => BatteryBand::OnBattery,
            _ => BatteryBand::Normal,
        }
    }

    fn thermal_band(&self) -> ThermalBand {
        match self.thermal {
            Thermal::Serious => ThermalBand::Serious,
            Thermal::Critical => ThermalBand::Critical,
            _ => ThermalBand::Normal,
        }
    }

    /// The Mac is known to be on mains power (None: not known).
    pub fn on_ac(&self) -> Option<bool> {
        match self.source {
            PowerSource::Unknown => None,
            s => Some(s == PowerSource::Ac),
        }
    }
}

/// One warning for the notification centre.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Notice {
    /// The condition's key: `power` or `thermal`.
    pub key: &'static str,
    pub level: Level,
    /// French.
    pub text: String,
    /// Back to normal: the condition `key` ends, `text` announces it.
    pub resolved: bool,
}

fn pct(s: &PowerStatus) -> String {
    s.battery_pct.map(|p| format!(" ({p} %)")).unwrap_or_default()
}

/// The warnings for going from `prev` to `now` (pure). Nothing when the
/// state of a side does not change, and nothing for a side that is
/// `Unknown` now (no reading is not a change).
pub fn classify(prev: &PowerStatus, now: &PowerStatus) -> Vec<Notice> {
    let mut out = Vec::new();
    if now.source != PowerSource::Unknown {
        let (was, is) = (prev.battery_band(), now.battery_band());
        if was != is {
            let warn = |level, text: String| Notice { key: "power", level, text, resolved: false };
            out.push(match is {
                BatteryBand::Normal => Notice { key: "power", level: Level::Info, text: "Mac de nouveau sur secteur".into(), resolved: true },
                BatteryBand::OnBattery => warn(Level::Warning, format!("Sur batterie{}", pct(now))),
                BatteryBand::Low => warn(Level::Warning, format!("Batterie faible{} : branchez le secteur", pct(now))),
                BatteryBand::Critical => warn(Level::Error, format!("Batterie faible{} : branchez le secteur, le Mac va se mettre en veille", pct(now))),
            });
        }
    }
    if now.thermal != Thermal::Unknown {
        let (was, is) = (prev.thermal_band(), now.thermal_band());
        if was != is {
            out.push(match is {
                ThermalBand::Normal => Notice { key: "thermal", level: Level::Info, text: "Température du Mac revenue à la normale".into(), resolved: true },
                ThermalBand::Serious => Notice { key: "thermal", level: Level::Warning, text: "Le Mac chauffe : images en retard possibles".into(), resolved: false },
                ThermalBand::Critical => Notice { key: "thermal", level: Level::Error, text: "Le Mac surchauffe : images en retard, le système ralentit".into(), resolved: false },
            });
        }
    }
    out
}

/// The state `classify` compares the next reading with: an `Unknown` side
/// keeps the last known one, and on battery the level only goes down (a
/// reading wavering between 21 and 20 % warns once).
fn reference(prev: &PowerStatus, now: &PowerStatus) -> PowerStatus {
    let mut r = *now;
    if now.source == PowerSource::Unknown {
        r.source = prev.source;
        r.battery_pct = prev.battery_pct;
    } else if now.source == PowerSource::Battery && prev.source == PowerSource::Battery && now.battery_band() < prev.battery_band() {
        r.battery_pct = prev.battery_pct;
    }
    if now.thermal == Thermal::Unknown {
        r.thermal = prev.thermal;
    }
    r
}

/// Where the readings come from: the system, or a fake in tests.
pub trait Reader: Send + 'static {
    fn read(&mut self) -> PowerStatus;
}

/// The system's reading (macOS; `Unknown` elsewhere).
pub struct SystemReader;

impl Reader for SystemReader {
    fn read(&mut self) -> PowerStatus {
        read_system()
    }
}

#[derive(Default)]
struct Inner {
    /// The last reading, as shown.
    status: PowerStatus,
    /// What the next reading is compared with.
    reference: PowerStatus,
    /// Warnings not yet in the notification centre.
    pending: Vec<Notice>,
}

#[derive(Default)]
pub struct PowerWatch {
    inner: Mutex<Inner>,
    /// Tests and e2e: no thread, no system call; `simulate` only.
    simulated: bool,
    log: OnceLock<SafetyLog>,
}

impl PowerWatch {
    pub fn new(simulated: bool) -> Self {
        Self { simulated, ..Default::default() }
    }

    /// The safety log the changes are recorded in.
    pub fn set_log(&self, log: SafetyLog) {
        let _ = self.log.set(log);
    }

    /// The last snapshot (a short lock, never a system call).
    pub fn status(&self) -> PowerStatus {
        self.inner.lock().unwrap().status
    }

    /// The warnings since the last call, for the notification centre.
    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut self.inner.lock().unwrap().pending)
    }

    /// `POST /api/test/power` (`--test-hooks`).
    pub fn simulate(&self, status: PowerStatus) {
        self.update(status);
    }

    /// A new reading: kept, and every change logged and queued.
    fn update(&self, now: PowerStatus) {
        let notices = {
            let mut inner = self.inner.lock().unwrap();
            let next = reference(&inner.reference, &now);
            let notices = classify(&inner.reference, &next);
            inner.reference = next;
            inner.status = now;
            inner.pending.extend(notices.iter().cloned());
            notices
        };
        for n in notices {
            match n.level {
                Level::Info => log::info!("Mac : {}", n.text),
                _ => log::warn!("Mac : {}", n.text),
            }
            if let Some(log) = self.log.get() {
                log.record("mac_power", Some("system"), serde_json::json!({ "key": n.key, "level": n.level, "text": n.text, "status": now }));
            }
        }
    }

    /// The reading thread (never in simulation).
    pub fn spawn(self: &Arc<Self>, running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
        if self.simulated {
            return None;
        }
        self.spawn_with(SystemReader, running, PERIOD)
    }

    fn spawn_with(self: &Arc<Self>, mut reader: impl Reader, running: Arc<AtomicBool>, period: Duration) -> Option<std::thread::JoinHandle<()>> {
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("power-watch".into())
            .spawn(move || {
                let tick = Duration::from_millis(200).min(period);
                let mut waited = period;
                while running.load(Ordering::SeqCst) {
                    if waited >= period {
                        waited = Duration::ZERO;
                        // Read without any lock held: the state stays readable.
                        let status = reader.read();
                        me.update(status);
                    }
                    std::thread::sleep(tick);
                    waited += tick;
                }
            })
            .map_err(|e| log::warn!("power watch thread: {e}"))
            .ok()
    }
}

#[cfg(not(target_os = "macos"))]
fn read_system() -> PowerStatus {
    PowerStatus::default()
}

#[cfg(target_os = "macos")]
fn read_system() -> PowerStatus {
    let (source, battery_pct) = mac::power();
    PowerStatus { source, battery_pct, thermal: mac::thermal() }
}

#[cfg(target_os = "macos")]
mod mac {
    //! IOKit power sources (IOPowerSources.h, IOPSKeys.h) and the
    //! Objective-C runtime for `NSProcessInfo.thermalState`, all public.
    //! Any failure gives `Unknown`.

    use super::{PowerSource, Thermal};
    use std::ffi::{c_char, c_void, CStr, CString};

    type CFTypeRef = *const c_void;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT32_TYPE: isize = 3;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(n: CFTypeRef, kind: isize, out: *mut c_void) -> bool;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, enc: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: isize, enc: u32) -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFTypeRef;
        fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
        fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// An owned CF object, released on drop.
    struct Owned(CFTypeRef);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a +1 reference from a Create/Copy call.
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn cf_string(s: &str) -> Option<Owned> {
        let c = CString::new(s).ok()?;
        // SAFETY: a valid C string; the result is owned.
        let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) };
        (!r.is_null()).then_some(Owned(r))
    }

    fn rust_string(s: CFTypeRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 128];
        // SAFETY: `s` is a CFString, `buf` is writable for its length.
        let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8) };
        // SAFETY: on success the buffer holds a NUL-terminated string.
        ok.then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    fn number(dict: CFTypeRef, key: &str) -> Option<i32> {
        let key = cf_string(key)?;
        // SAFETY: `dict` is a CFDictionary; the value follows the Get rule.
        let n = unsafe { CFDictionaryGetValue(dict, key.0) };
        if n.is_null() {
            return None;
        }
        let mut v: i32 = 0;
        // SAFETY: `n` is a CFNumber, `v` a valid i32 out pointer.
        unsafe { CFNumberGetValue(n, K_CF_NUMBER_SINT32_TYPE, &mut v as *mut i32 as *mut c_void) }.then_some(v)
    }

    fn string(dict: CFTypeRef, key: &str) -> Option<String> {
        let key = cf_string(key)?;
        // SAFETY: as above; the value is a CFString (Get rule).
        rust_string(unsafe { CFDictionaryGetValue(dict, key.0) })
    }

    /// The providing source and the internal battery's charge.
    pub fn power() -> (PowerSource, Option<u8>) {
        // SAFETY: no arguments; the snapshot is owned.
        let info = Owned(unsafe { IOPSCopyPowerSourcesInfo() });
        if info.0.is_null() {
            return (PowerSource::Unknown, None);
        }
        // SAFETY: a valid snapshot; the string follows the Get rule.
        let source = match rust_string(unsafe { IOPSGetProvidingPowerSourceType(info.0) }).as_deref() {
            Some("AC Power") => PowerSource::Ac,
            Some("Battery Power") => PowerSource::Battery,
            // A UPS is not this Mac's battery: unknown, say nothing.
            _ => PowerSource::Unknown,
        };
        // SAFETY: a valid snapshot; the list is owned.
        let list = Owned(unsafe { IOPSCopyPowerSourcesList(info.0) });
        let mut pct = None;
        if !list.0.is_null() {
            // SAFETY: `list` is a CFArray.
            let n = unsafe { CFArrayGetCount(list.0) };
            for i in 0..n {
                // SAFETY: index in range; description follows the Get rule.
                let desc = unsafe { IOPSGetPowerSourceDescription(info.0, CFArrayGetValueAtIndex(list.0, i)) };
                if desc.is_null() || string(desc, "Type").as_deref() != Some("InternalBattery") {
                    continue;
                }
                if let (Some(cur), Some(max)) = (number(desc, "Current Capacity"), number(desc, "Max Capacity")) {
                    if max > 0 {
                        pct = Some((cur.clamp(0, max) as f32 * 100.0 / max as f32).round() as u8);
                    }
                }
                break;
            }
        }
        (source, pct)
    }

    /// `[[NSProcessInfo processInfo] thermalState]`.
    pub fn thermal() -> Thermal {
        // SAFETY: plain runtime calls on a class Foundation provides; the
        // message signatures match the methods (id; NSInteger). A pool
        // holds what they autorelease.
        unsafe {
            let class = objc_getClass(c"NSProcessInfo".as_ptr());
            if class.is_null() {
                return Thermal::Unknown;
            }
            let pool = objc_autoreleasePoolPush();
            let send_id: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_int: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let info = send_id(class, sel_registerName(c"processInfo".as_ptr()));
            let state = if info.is_null() { -1 } else { send_int(info, sel_registerName(c"thermalState".as_ptr())) };
            objc_autoreleasePoolPop(pool);
            match state {
                0 => Thermal::Nominal,
                1 => Thermal::Fair,
                2 => Thermal::Serious,
                3 => Thermal::Critical,
                _ => Thermal::Unknown,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bat(p: u8) -> PowerStatus {
        PowerStatus { source: PowerSource::Battery, battery_pct: Some(p), thermal: Thermal::Nominal }
    }
    fn ac() -> PowerStatus {
        PowerStatus { source: PowerSource::Ac, battery_pct: Some(80), thermal: Thermal::Nominal }
    }
    fn heat(t: Thermal) -> PowerStatus {
        PowerStatus { thermal: t, ..ac() }
    }
    fn one(prev: PowerStatus, now: PowerStatus) -> Notice {
        let v = classify(&prev, &now);
        assert_eq!(v.len(), 1, "{prev:?} → {now:?}: {v:?}");
        v.into_iter().next().unwrap()
    }

    #[test]
    fn each_battery_transition_warns_once() {
        let n = one(ac(), bat(64));
        assert_eq!((n.key, n.level, n.text.as_str(), n.resolved), ("power", Level::Warning, "Sur batterie (64 %)", false));
        assert!(classify(&bat(64), &bat(21)).is_empty(), "still on battery");
        let n = one(bat(21), bat(20));
        assert_eq!((n.level, n.text.as_str()), (Level::Warning, "Batterie faible (20 %) : branchez le secteur"));
        assert!(classify(&bat(20), &bat(11)).is_empty());
        let n = one(bat(11), bat(10));
        assert_eq!(n.level, Level::Error);
        assert!(n.text.starts_with("Batterie faible (10 %) : branchez le secteur"), "{}", n.text);
        assert!(n.text.contains("veille"));
        let n = one(bat(9), ac());
        assert_eq!((n.level, n.resolved, n.text.as_str()), (Level::Info, true, "Mac de nouveau sur secteur"));
        assert!(classify(&ac(), &ac()).is_empty());
        // Straight to a low battery (the studio started unplugged).
        assert_eq!(one(PowerStatus::default(), bat(9)).level, Level::Error);
    }

    #[test]
    fn each_thermal_transition_warns_once() {
        assert!(classify(&heat(Thermal::Nominal), &heat(Thermal::Fair)).is_empty());
        let n = one(heat(Thermal::Fair), heat(Thermal::Serious));
        assert_eq!((n.key, n.level, n.text.as_str()), ("thermal", Level::Warning, "Le Mac chauffe : images en retard possibles"));
        assert!(classify(&heat(Thermal::Serious), &heat(Thermal::Serious)).is_empty());
        assert_eq!(one(heat(Thermal::Serious), heat(Thermal::Critical)).level, Level::Error);
        let n = one(heat(Thermal::Critical), heat(Thermal::Nominal));
        assert!(n.resolved);
        assert_eq!(n.level, Level::Info);
    }

    #[test]
    fn unknown_says_nothing() {
        let unknown = PowerStatus::default();
        assert!(classify(&unknown, &unknown).is_empty());
        assert!(classify(&bat(5), &unknown).is_empty(), "a failed reading is not a change");
        assert!(classify(&heat(Thermal::Critical), &unknown).is_empty());
        assert!(classify(&unknown, &ac()).is_empty(), "on mains: nothing to say");
        assert_eq!(unknown.on_ac(), None);
        assert_eq!(ac().on_ac(), Some(true));
        assert_eq!(bat(50).on_ac(), Some(false));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(read_system(), unknown);
    }

    #[test]
    fn the_watch_does_not_repeat_while_the_state_holds() {
        let w = PowerWatch::new(true);
        for s in [bat(64), bat(63), bat(21), bat(20), bat(21), bat(20), bat(19), PowerStatus::default(), bat(19), bat(10), bat(11), bat(9)] {
            w.simulate(s);
        }
        let texts: Vec<String> = w.take_notices().into_iter().map(|n| n.text).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(texts[0].starts_with("Sur batterie") && texts[1].starts_with("Batterie faible (20 %)") && texts[2].starts_with("Batterie faible (10 %)"));
        assert!(w.take_notices().is_empty(), "taken once");
        assert_eq!(w.status(), bat(9));
        w.simulate(ac());
        assert!(w.take_notices()[0].resolved);
    }

    #[test]
    fn reading_never_holds_the_snapshot() {
        // A reader stuck in the system call: the snapshot stays readable.
        struct Stuck(std::sync::mpsc::Receiver<()>, Arc<AtomicBool>);
        impl Reader for Stuck {
            fn read(&mut self) -> PowerStatus {
                self.1.store(true, Ordering::SeqCst);
                let _ = self.0.recv();
                bat(50)
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let reading = Arc::new(AtomicBool::new(false));
        let w = Arc::new(PowerWatch::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let t = w.spawn_with(Stuck(rx, Arc::clone(&reading)), Arc::clone(&running), Duration::from_millis(10)).unwrap();
        while !reading.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let start = std::time::Instant::now();
        assert_eq!(w.status(), PowerStatus::default());
        assert!(start.elapsed() < Duration::from_millis(100));
        running.store(false, Ordering::SeqCst);
        drop(tx);
        t.join().unwrap();
        assert_eq!(w.status(), bat(50));
        assert_eq!(w.take_notices().len(), 1);
    }

    #[test]
    fn simulation_runs_no_thread() {
        let w = Arc::new(PowerWatch::new(true));
        assert!(w.spawn(Arc::new(AtomicBool::new(true))).is_none());
    }

    #[test]
    fn a_warning_never_touches_the_output() {
        // This module only informs: no arming, blanking or brightness call.
        let src = include_str!("power_watch.rs");
        let code: String = src[..src.find("#[cfg(test)]\nmod tests").unwrap()].lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        for word in ["arm(", "disarm", "blank", "blackout", "estop", "brightness", "kill_switch", "output_lit", "settings"] {
            assert!(!code.contains(word), "power_watch.rs mentions {word}");
        }
        // Through the studio: a critical battery and overheating while
        // armed leave the laser armed and the brightness as it was.
        let mut s = crate::test_support::shared();
        s.gate.set_output_ready(true);
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        let brightness = s.settings.brightness;
        s.power_watch.simulate(PowerStatus { source: PowerSource::Battery, battery_pct: Some(3), thermal: Thermal::Critical });
        s.sync_notices();
        assert!(s.gate.is_armed());
        assert_eq!(s.settings.brightness, brightness);
        let texts: Vec<String> = s.notices.since(0).into_iter().filter(|n| n.source == SOURCE).map(|n| n.text).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
    }
}
