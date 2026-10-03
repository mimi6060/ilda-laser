//! The DACs the studio can see (T-303): a read-only list for
//! `GET /api/devices`, *Réglages › Sortie* and `--list-devices`.
//!
//! Scanning never connects, arms or sends a frame: it only lists what
//! `laser-dac` discovers (Ether Dream broadcasts, IDN scan replies, Helios
//! USB descriptors). The Helios scan briefly opens the USB device to read
//! its name, so USB is left out of a rescan while the studio already drives
//! a DAC: the running output is never touched. Connecting one is the
//! output manager's job (`output_manager.rs`, T-401).
//!
//! A scan takes about 1.5 s (the Ether Dream listens for broadcasts), so
//! it runs on its own thread and only when asked (the *Rafraîchir* button),
//! never at start-up. Tests and e2e inject a fixed list instead
//! (`DeviceList::fixed`, `--test-devices`): no test ever scans the network
//! or the USB bus.

use anyhow::Result;
use laser_dac::{list_devices_filtered, DacType, EnabledDacTypes};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One DAC found by a scan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// "helios", "etherdream", "idn", ...
    pub kind: String,
    /// What `--device` takes, e.g. `helios:left` or `idn:heliospro.local`.
    pub id: String,
    pub name: String,
}

/// What a scan may look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    /// Helios on USB (the scan opens the device briefly to read its name).
    pub usb: bool,
}

/// The scan itself: the real one (`scan_dacs`) or a test double.
pub type Scanner = Arc<dyn Fn(Scope) -> Result<Vec<DeviceInfo>> + Send + Sync>;

/// The DAC types the studio is built with, as `--device` ids name them.
pub fn kind_name(kind: &DacType) -> String {
    match kind {
        DacType::Helios => "helios".into(),
        DacType::EtherDream => "etherdream".into(),
        DacType::Idn => "idn".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// The real scan through `laser-dac`: Ether Dream and IDN on the network,
/// plus Helios on USB when `scope.usb`.
pub fn scan_dacs(scope: Scope) -> Result<Vec<DeviceInfo>> {
    let mut types = EnabledDacTypes::none();
    types.enable(DacType::EtherDream).enable(DacType::Idn);
    if scope.usb {
        types.enable(DacType::Helios);
    }
    let found = list_devices_filtered(&types).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(found.into_iter().map(|d| DeviceInfo { kind: kind_name(&d.kind), id: d.id, name: d.name }).collect())
}

/// `--list-devices`: one line per DAC (id, kind, name), or a note when
/// none is found.
pub fn print_list(devices: &[DeviceInfo]) -> String {
    if devices.is_empty() {
        return "No DAC found (Helios on USB, Ether Dream or IDN on the network).\n".into();
    }
    let mut out = String::new();
    for d in devices {
        out.push_str(&format!("{}\t{}\t{}\n", d.id, d.kind, d.name));
    }
    out.push_str("Start the studio with --device <id> to drive one (it starts disarmed).\n");
    out
}

#[derive(Default)]
struct ListState {
    devices: Vec<DeviceInfo>,
    scanning: bool,
    /// At least one scan has finished.
    scanned: bool,
    error: Option<String>,
}

/// The last scan's result, shared by the HTTP handlers and the scan thread.
pub struct DeviceList {
    scanner: Scanner,
    /// USB in the next scan: follows the connected output (T-401).
    usb: AtomicBool,
    state: Mutex<ListState>,
}

impl DeviceList {
    pub fn new(scanner: Scanner, scope: Scope) -> Self {
        Self { scanner, usb: AtomicBool::new(scope.usb), state: Mutex::default() }
    }

    /// The real scanner. `driving_dac`: the studio already outputs to a DAC,
    /// so USB is left out (see the module doc).
    pub fn real(driving_dac: bool) -> Self {
        Self::new(Arc::new(scan_dacs), Scope { usb: !driving_dac })
    }

    /// Always "finds" these devices, without touching any hardware.
    pub fn fixed(devices: Vec<DeviceInfo>) -> Self {
        Self::new(Arc::new(move |_| Ok(devices.clone())), Scope { usb: true })
    }

    /// What the next scan may look at.
    pub fn scope(&self) -> Scope {
        Scope { usb: self.usb.load(Ordering::SeqCst) }
    }

    /// The studio started or stopped driving a DAC (T-401): USB is left
    /// out of the scans while it does (see the module doc).
    pub fn set_driving_dac(&self, driving: bool) {
        self.usb.store(!driving, Ordering::SeqCst);
    }

    /// The devices of the last scan.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.state.lock().unwrap().devices.clone()
    }

    /// The JSON for `GET /api/devices`.
    pub fn view(&self) -> serde_json::Value {
        let s = self.state.lock().unwrap();
        serde_json::json!({
            "devices": s.devices,
            "scanning": s.scanning,
            "scanned": s.scanned,
            "error": s.error,
            "usb": self.scope().usb,
        })
    }

    /// Starts a scan on its own thread, unless one is running. Returns
    /// whether a scan was started.
    pub fn refresh(self: &Arc<Self>) -> bool {
        {
            let mut s = self.state.lock().unwrap();
            if s.scanning {
                return false;
            }
            s.scanning = true;
        }
        let list = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("dac-scan".into()).spawn(move || list.scan_now());
        if let Err(e) = spawned {
            let mut s = self.state.lock().unwrap();
            s.scanning = false;
            s.error = Some(format!("scan impossible : {e}"));
        }
        true
    }

    /// Runs one scan on this thread and stores the result.
    fn scan_now(&self) {
        // The lock is not held during the scan: views stay instant.
        let result = (self.scanner)(self.scope());
        let mut s = self.state.lock().unwrap();
        match result {
            Ok(devices) => {
                s.devices = devices;
                s.error = None;
            }
            Err(e) => {
                s.devices.clear();
                s.error = Some(format!("{e:#}"));
            }
        }
        s.scanning = false;
        s.scanned = true;
    }
}

/// `--test-devices` (one argument, so not a bare `Vec` for clap).
#[derive(Clone, Debug, PartialEq)]
pub struct TestDevices(pub Vec<DeviceInfo>);

/// `--test-devices`: a JSON list of `DeviceInfo`.
pub fn parse_test_devices(s: &str) -> Result<TestDevices, String> {
    serde_json::from_str(s).map(TestDevices).map_err(|e| format!("expected a JSON list of {{kind, id, name}}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn helios() -> DeviceInfo {
        DeviceInfo { kind: "helios".into(), id: "helios:left".into(), name: "Helios DAC Left (1:2.3)".into() }
    }

    fn wait_scanned(list: &DeviceList) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let v = list.view();
            if v["scanned"] == true && v["scanning"] == false {
                return v;
            }
            assert!(Instant::now() < deadline, "the scan never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn device_info_serialises_as_kind_id_name() {
        let v = serde_json::to_value(helios()).unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "helios", "id": "helios:left", "name": "Helios DAC Left (1:2.3)" }));
        let back: DeviceInfo = serde_json::from_value(v).unwrap();
        assert_eq!(back, helios());
    }

    #[test]
    fn kind_names_match_the_device_id_prefixes() {
        assert_eq!(kind_name(&DacType::Helios), "helios");
        assert_eq!(kind_name(&DacType::EtherDream), "etherdream");
        assert_eq!(kind_name(&DacType::Idn), "idn");
    }

    #[test]
    fn nothing_is_scanned_until_asked() {
        let calls = Arc::new(Mutex::new(0));
        let list = DeviceList::new(
            Arc::new({
                let calls = Arc::clone(&calls);
                move |_| {
                    *calls.lock().unwrap() += 1;
                    Ok(vec![])
                }
            }),
            Scope { usb: true },
        );
        let v = list.view();
        assert_eq!(v["devices"], serde_json::json!([]));
        assert_eq!(v["scanned"], false);
        assert_eq!(*calls.lock().unwrap(), 0, "a view never scans");
    }

    #[test]
    fn refresh_scans_once_and_keeps_the_result() {
        let list = Arc::new(DeviceList::fixed(vec![helios()]));
        assert!(list.refresh());
        let v = wait_scanned(&list);
        assert_eq!(v["devices"][0]["id"], "helios:left");
        assert_eq!(v["error"], serde_json::Value::Null);
    }

    #[test]
    fn a_second_refresh_waits_for_the_running_scan() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let rx = Mutex::new(rx);
        let list = Arc::new(DeviceList::new(
            Arc::new(move |_| {
                rx.lock().unwrap().recv().ok();
                Ok(vec![])
            }),
            Scope { usb: true },
        ));
        assert!(list.refresh());
        assert!(!list.refresh(), "one scan at a time");
        assert_eq!(list.view()["scanning"], true);
        tx.send(()).unwrap();
        wait_scanned(&list);
        assert!(list.refresh(), "free again once done");
        tx.send(()).unwrap();
        wait_scanned(&list);
    }

    #[test]
    fn a_failed_scan_reports_the_error_and_no_devices() {
        let list = Arc::new(DeviceList::new(Arc::new(|_| Err(anyhow::anyhow!("réseau absent"))), Scope { usb: true }));
        list.refresh();
        let v = wait_scanned(&list);
        assert_eq!(v["devices"], serde_json::json!([]));
        assert_eq!(v["error"], "réseau absent");
    }

    #[test]
    fn usb_is_left_out_while_driving_a_dac() {
        assert!(DeviceList::real(false).scope().usb);
        assert!(!DeviceList::real(true).scope().usb, "the running Helios is never reopened by a scan");
        assert_eq!(DeviceList::real(true).view()["usb"], false);
        // The scanner receives the scope.
        let seen = Arc::new(Mutex::new(None));
        let list = Arc::new(DeviceList::new(
            Arc::new({
                let seen = Arc::clone(&seen);
                move |scope| {
                    *seen.lock().unwrap() = Some(scope);
                    Ok(vec![])
                }
            }),
            Scope { usb: false },
        ));
        list.refresh();
        wait_scanned(&list);
        assert_eq!(*seen.lock().unwrap(), Some(Scope { usb: false }));
    }

    #[test]
    fn list_printout() {
        assert!(print_list(&[]).starts_with("No DAC found"));
        let out = print_list(&[helios()]);
        assert!(out.starts_with("helios:left\thelios\tHelios DAC Left (1:2.3)\n"), "{out}");
        assert!(out.contains("--device <id>"));
    }

    #[test]
    fn test_devices_parse_from_json() {
        assert_eq!(parse_test_devices("[]").unwrap().0, vec![]);
        let list = parse_test_devices(r#"[{"kind":"helios","id":"helios:left","name":"Helios DAC Left (1:2.3)"}]"#).unwrap();
        assert_eq!(list.0, vec![helios()]);
        assert!(parse_test_devices("{}").is_err());
        assert!(parse_test_devices(r#"[{"id":"x"}]"#).is_err());
    }
}
