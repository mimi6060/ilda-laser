//! Wi-Fi warning for ShowNET outputs (T-472).
//!
//! Laserworld's manual warns that running the ShowNET over Wi-Fi lowers
//! its performance and is not safe operation. While the output is a
//! ShowNET path, a background thread asks macOS which network interface
//! leads to the target (`route -n get`) and whether that interface is the
//! Wi-Fi one (`networksetup -listallhardwareports`). Both only read this
//! Mac's own tables: nothing is sent on the network, nothing is scanned.
//! The top bar then shows an orange « Wi-Fi » pill for as long as it is so.
//!
//! The target is the PONK receiver when it is on another machine; when
//! MadMapper runs on this Mac (loopback), the box's link is MadMapper's,
//! so the default route stands for it. A warning only: it never arms,
//! never disarms and never blocks arming.
//!
//! Tests and e2e never run the commands: `simulate` sets the state.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the route is checked while a ShowNET output is connected.
const PERIOD: Duration = Duration::from_secs(5);

/// Where the ShowNET traffic goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The default route (MadMapper on this Mac).
    Default,
    /// A remote host (PONK receiver on another machine).
    Host(IpAddr),
}

impl Target {
    /// The target for a PONK receiver address: the default route when it
    /// is this Mac.
    pub fn for_receiver(ip: Option<IpAddr>) -> Self {
        match ip {
            Some(ip) if !ip.is_loopback() && !ip.is_unspecified() => Target::Host(ip),
            _ => Target::Default,
        }
    }
}

/// What the last check found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiStatus {
    /// The interface leading to the target is the Wi-Fi.
    pub wireless: bool,
    /// That interface (`en0`...), when known.
    pub interface: Option<String>,
}

#[derive(Default)]
struct Inner {
    /// None: not a ShowNET output, nothing watched.
    target: Option<Target>,
    status: Option<WifiStatus>,
}

#[derive(Default)]
pub struct WifiWatch {
    inner: Mutex<Inner>,
    /// Tests and e2e: no thread, no command; `simulate` only.
    simulated: bool,
}

impl WifiWatch {
    pub fn new(simulated: bool) -> Self {
        Self { simulated, ..Default::default() }
    }

    /// Watches `target` (None: stop; the warning goes away).
    pub fn set_target(&self, target: Option<Target>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.target != target {
            inner.target = target;
            if !self.simulated || target.is_none() {
                inner.status = None;
            }
        }
    }

    /// The warning to show: Some only while watching and on Wi-Fi.
    pub fn warning(&self) -> Option<WifiStatus> {
        let inner = self.inner.lock().unwrap();
        inner.target.and(inner.status.clone()).filter(|s| s.wireless)
    }

    /// `POST /api/test/wifi` (`--test-hooks`).
    pub fn simulate(&self, status: Option<WifiStatus>) {
        self.inner.lock().unwrap().status = status;
    }

    /// The checking thread (never in simulation).
    pub fn spawn(self: &Arc<Self>, running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
        if self.simulated {
            return None;
        }
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("wifi-watch".into())
            .spawn(move || {
                let mut waited = PERIOD;
                while running.load(Ordering::SeqCst) {
                    if waited >= PERIOD {
                        waited = Duration::ZERO;
                        let target = me.inner.lock().unwrap().target;
                        if let Some(target) = target {
                            let status = detect(target);
                            let mut inner = me.inner.lock().unwrap();
                            if inner.target == Some(target) {
                                inner.status = status;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    waited += Duration::from_millis(200);
                }
            })
            .map_err(|e| log::warn!("wifi watch thread: {e}"))
            .ok()
    }
}

/// Asks macOS which interface leads to `target` and whether it is the
/// Wi-Fi. None when it can't tell (other systems, a command failing).
fn detect(target: Target) -> Option<WifiStatus> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let dest = match target {
        Target::Default => "default".to_string(),
        Target::Host(ip) => ip.to_string(),
    };
    let run = |cmd: &str, args: &[&str]| -> Option<String> {
        let out = std::process::Command::new(cmd).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let interface = parse_route_interface(&run("/sbin/route", &["-n", "get", &dest])?)?;
    let wifi = parse_wifi_devices(&run("/usr/sbin/networksetup", &["-listallhardwareports"])?);
    Some(WifiStatus { wireless: wifi.contains(&interface), interface: Some(interface) })
}

/// The `interface:` line of `route -n get`.
pub fn parse_route_interface(out: &str) -> Option<String> {
    out.lines().find_map(|l| l.trim().strip_prefix("interface:")).map(|i| i.trim().to_string()).filter(|i| !i.is_empty())
}

/// The devices of the Wi-Fi (or AirPort, older systems) hardware ports in
/// `networksetup -listallhardwareports`.
pub fn parse_wifi_devices(out: &str) -> Vec<String> {
    let mut devices = Vec::new();
    let mut in_wifi = false;
    for line in out.lines().map(str::trim) {
        if let Some(port) = line.strip_prefix("Hardware Port:") {
            let port = port.trim();
            in_wifi = port.eq_ignore_ascii_case("Wi-Fi") || port.eq_ignore_ascii_case("AirPort");
        } else if let Some(dev) = line.strip_prefix("Device:") {
            if in_wifi {
                devices.push(dev.trim().to_string());
            }
            in_wifi = false;
        }
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str = "   route to: default\ndestination: default\n       mask: default\n    gateway: 192.168.1.1\n  interface: en0\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n";
    const PORTS: &str = "\nHardware Port: Ethernet\nDevice: en7\nEthernet Address: 00:00:00:00:00:01\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 00:00:00:00:00:02\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: N/A\n";

    #[test]
    fn the_route_and_the_wifi_port_are_read_from_the_commands() {
        assert_eq!(parse_route_interface(ROUTE).as_deref(), Some("en0"));
        assert_eq!(parse_route_interface("route: writing to routing socket: not in table\n"), None);
        assert_eq!(parse_wifi_devices(PORTS), ["en0"]);
        assert!(parse_wifi_devices("Hardware Port: Ethernet\nDevice: en0\n").is_empty());
        assert_eq!(parse_wifi_devices("Hardware Port: AirPort\nDevice: en1\n"), ["en1"]);
    }

    #[test]
    fn a_local_receiver_is_judged_by_the_default_route() {
        assert_eq!(Target::for_receiver(Some("127.0.0.1".parse().unwrap())), Target::Default);
        assert_eq!(Target::for_receiver(None), Target::Default);
        let remote: IpAddr = "10.0.0.5".parse().unwrap();
        assert_eq!(Target::for_receiver(Some(remote)), Target::Host(remote));
    }

    #[test]
    fn the_warning_shows_only_while_watching_and_on_wifi() {
        let w = Arc::new(WifiWatch::new(true));
        assert!(w.spawn(Arc::new(AtomicBool::new(true))).is_none(), "no thread in simulation");
        let wifi = WifiStatus { wireless: true, interface: Some("en0".into()) };
        w.simulate(Some(wifi.clone()));
        assert_eq!(w.warning(), None, "not a ShowNET output");
        w.set_target(Some(Target::Default));
        assert_eq!(w.warning(), Some(wifi));
        w.simulate(Some(WifiStatus { wireless: false, interface: Some("en7".into()) }));
        assert_eq!(w.warning(), None, "wired");
        w.simulate(Some(WifiStatus { wireless: true, interface: None }));
        w.set_target(None);
        assert_eq!(w.warning(), None);
    }
}
