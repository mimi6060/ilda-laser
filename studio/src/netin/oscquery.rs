//! OSCQuery (T-499): a read-only listing of the `/ls/…` OSC addresses, in
//! JSON over HTTP, so TouchOSC, Chataigne, Vezér or Open Stage Control can
//! offer the addresses, types and ranges without typing them. Written from
//! the public proposal only (github.com/Vidvox/OSCQueryProposal).
//!
//! - Off by default; a setting of this machine (`netin.json`, section
//!   `oscquery`). It listens only while the OSC input is on too, on the OSC
//!   input's address (never an implicit 0.0.0.0), on its own port (default:
//!   the OSC port + 1).
//! - `GET /` (or any node's path): the tree (`FULL_PATH`, `CONTENTS`,
//!   `TYPE`, `ACCESS`, `RANGE`, `DESCRIPTION`, `VALUE` when readable);
//!   `GET /path?ATTRIBUTE`: one attribute; `GET /?HOST_INFO`: where to send
//!   the OSC (UDP).
//! - Read-only: every other method is answered 405, there is no WebSocket
//!   and no `LISTEN`. Nothing received here changes the studio.
//! - The addresses listed are the ones the OSC input accepts (`osc::command`,
//!   checked by a test): the controls `feedback::has_address` lets through
//!   and the command words. Arming, the hold-to-run pad, the e-stop reset and
//!   the safety settings have no address, so none is listed; the local gate
//!   of the external inputs (T-408) still decides at reception.
//! - « Annoncer sur le réseau » (Bonjour `_oscjson._tcp`, off by default)
//!   runs the system's `dns-sd` while the server listens on an address other
//!   than loopback. Never in tests.

use super::feedback::{self, address_of};
use super::{parse_bind, NetHub, NetInConfig, Proto};
use crate::controls::{ControlDesc, ControlKind, ControlRegistry, Unit};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::net::{IpAddr, SocketAddr};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Response, Server};

/// The Bonjour service type of the proposal.
pub const SERVICE: &str = "_oscjson._tcp";
const POLL: Duration = Duration::from_millis(100);
const RETRY: Duration = Duration::from_secs(2);
/// Clients served at once.
const MAX_CONNECTIONS: usize = 8;
/// Answer bytes waiting at once.
const OUTPUT_BUDGET: usize = 4 << 20;

/// In `<data-dir>/netin.json` (section `oscquery`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscQueryConfig {
    pub enabled: bool,
    /// `None`: the OSC port + 1. 0: the system picks one (tests).
    pub port: Option<u16>,
    /// Bonjour `_oscjson._tcp`.
    pub announce: bool,
}

impl NetInConfig {
    /// Where the OSCQuery server listens: on, and the OSC input on too.
    pub fn oscquery_listen(&self) -> Option<SocketAddr> {
        if !(self.oscquery.enabled && self.osc.enabled) {
            return None;
        }
        let port = self.oscquery.port.unwrap_or(match self.osc.port {
            0 => 0,
            p => p.checked_add(1).unwrap_or(0),
        });
        parse_bind(&self.osc.bind, port).ok()
    }
}

/// One published address.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// OSC type tags; `None`: no argument.
    pub types: Option<&'static str>,
    /// 1 read, 2 write, 3 both.
    pub access: u8,
    pub range: Option<Value>,
    pub description: String,
    pub value: Option<Value>,
}

fn unit_fr(u: Unit) -> &'static str {
    match u {
        Unit::None => "",
        Unit::Percent => " (%)",
        Unit::DegPerSec => " (°/s)",
        Unit::Bpm => " (BPM)",
        Unit::Deg => " (°)",
        Unit::Hz => " (Hz)",
        Unit::Ms => " (ms)",
    }
}

/// A control's entry; `value`: its current base value (`feedback::value_of`).
fn control_entry(d: &ControlDesc, value: Option<f32>) -> Entry {
    let path = address_of(&d.id);
    let label = d.label_fr.clone();
    match &d.kind {
        ControlKind::Continuous { min, max, unit, .. } => Entry {
            path,
            types: Some("f"),
            access: 3,
            range: Some(json!([{ "MIN": min, "MAX": max }])),
            description: format!("{label}{}", unit_fr(*unit)),
            value: value.map(|v| json!([v])),
        },
        ControlKind::Toggle { .. } => Entry {
            path,
            types: Some("T"),
            access: 3,
            range: None,
            description: format!("{label} (vrai / faux)"),
            value: value.map(|v| json!([v >= 0.5])),
        },
        ControlKind::Choice { options, .. } => {
            let list: Vec<String> = options.iter().enumerate().map(|(i, o)| format!("{i} = {o}")).collect();
            let vals: Vec<usize> = (0..options.len()).collect();
            Entry {
                path,
                types: Some("i"),
                access: 3,
                range: Some(json!([{ "MIN": 0, "MAX": options.len().saturating_sub(1), "VALS": vals }])),
                description: format!("{label} ({})", list.join(", ")),
                value: value.map(|v| json!([v.round() as i64])),
            }
        }
        ControlKind::Momentary => Entry { path, types: Some("T"), access: 2, range: None, description: format!("{label} (vrai = appuyé, faux = relâché)"), value: None },
        ControlKind::Trigger => Entry { path, types: None, access: 2, range: None, description: format!("{label} (sans argument)"), value: None },
    }
}

/// The command words of `osc::command` that are published. `/ls/arm`
/// (always refused) is not.
fn commands() -> Vec<Entry> {
    let cmd = |path: &str, types: Option<&'static str>, range: Option<Value>, description: &str| Entry { path: path.into(), types, access: 2, range, description: description.into(), value: None };
    vec![
        cmd("/ls/cue/play", Some("s"), None, "Lancer un cue depuis le début (identifiant du cue)"),
        cmd("/ls/cue/stop", None, None, "Arrêter tous les cues"),
        cmd("/ls/blackout", None, None, "Noir : enclenche l'arrêt d'urgence (toujours accepté)"),
        cmd("/ls/ping", None, None, "Ne fait rien : signale l'activité de ce client"),
        cmd("/ls/access", Some("i"), Some(json!([{ "MIN": 0, "MAX": 1 }])), "Demander (1) ou rendre (0) l'accès de ce client aux entrées externes"),
        cmd("/ls/subscribe", Some("i"), Some(json!([{ "MIN": 1, "MAX": 65535 }])), "Recevoir le retour OSC sur ce port"),
        cmd("/ls/unsubscribe", None, None, "Ne plus recevoir le retour OSC"),
    ]
}

/// Every published address: the commands, then the controls.
pub fn entries(reg: &ControlRegistry, value: impl Fn(&ControlDesc) -> Option<f32>) -> Vec<Entry> {
    let mut out = commands();
    out.extend(reg.list().iter().filter(|d| feedback::has_address(d)).map(|d| control_entry(d, value(d))));
    out
}

fn container(path: &str, description: &str) -> Value {
    json!({ "FULL_PATH": path, "CONTENTS": {}, "ACCESS": 0, "DESCRIPTION": description })
}

/// The whole tree, from the root.
pub fn tree(entries: &[Entry]) -> Value {
    let mut root = container("/", "Laser Studio");
    root["CONTENTS"]["ls"] = container("/ls", "Laser Studio : adresses OSC (UDP)");
    for e in entries {
        let mut node = &mut root;
        let mut full = String::new();
        for part in e.path.trim_start_matches('/').split('/') {
            full.push('/');
            full.push_str(part);
            if node.get("CONTENTS").is_none() {
                node["CONTENTS"] = json!({});
            }
            let contents = node["CONTENTS"].as_object_mut().expect("CONTENTS is an object");
            node = contents.entry(part.to_string()).or_insert_with(|| container(&full, ""));
        }
        let n = node.as_object_mut().expect("a node is an object");
        if n.get("CONTENTS").and_then(Value::as_object).is_some_and(Map::is_empty) {
            n.remove("CONTENTS");
        }
        n.insert("ACCESS".into(), json!(e.access));
        n.insert("DESCRIPTION".into(), json!(e.description));
        if let Some(t) = e.types {
            n.insert("TYPE".into(), json!(t));
        }
        if let Some(r) = &e.range {
            n.insert("RANGE".into(), r.clone());
        }
        if let Some(v) = &e.value {
            n.insert("VALUE".into(), v.clone());
        }
    }
    root
}

/// The node at an OSC path (`/`, `/ls/master`).
pub fn node<'a>(tree: &'a Value, path: &str) -> Option<&'a Value> {
    let mut n = tree;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        n = n.get("CONTENTS")?.get(part)?;
    }
    Some(n)
}

/// The attributes a query may ask for.
const ATTRIBUTES: [&str; 7] = ["FULL_PATH", "CONTENTS", "TYPE", "ACCESS", "RANGE", "DESCRIPTION", "VALUE"];

/// `HOST_INFO`: `osc` is where the OSC input listens (its IP left out when
/// it is 0.0.0.0: the client uses the address it reached us on).
pub fn host_info(osc: SocketAddr) -> Value {
    let mut v = json!({
        "NAME": "Laser Studio",
        "OSC_PORT": osc.port(),
        "OSC_TRANSPORT": "UDP",
        "EXTENSIONS": {
            "ACCESS": true, "VALUE": true, "RANGE": true, "DESCRIPTION": true,
            "TAGS": false, "EXTENDED_TYPE": false, "UNIT": false, "CRITICAL": false,
            "CLIPMODE": false, "LISTEN": false, "PATH_CHANGED": false,
        },
    });
    if !osc.ip().is_unspecified() {
        v["OSC_IP"] = json!(osc.ip().to_string());
    }
    v
}

/// One request: (status, JSON body). Only GET; `tree` and `host` are built
/// only when needed.
pub fn answer(method: &str, url: &str, tree: impl FnOnce() -> Value, host: impl FnOnce() -> Value) -> (u16, Option<Value>) {
    if method != "GET" {
        return (405, None);
    }
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if query == "HOST_INFO" {
        return (200, Some(host()));
    }
    if !query.is_empty() && !ATTRIBUTES.contains(&query) {
        return (400, None);
    }
    if !path.starts_with('/') || path.contains("//") && path != "//" {
        return (404, None);
    }
    let tree = tree();
    let Some(n) = node(&tree, path) else { return (404, None) };
    if query.is_empty() {
        return (200, Some(n.clone()));
    }
    match n.get(query) {
        Some(v) => (200, Some(json!({ query: v }))),
        // A readable node without a value yet: an empty object (the proposal).
        None if query == "VALUE" && n["ACCESS"].as_u64().is_some_and(|a| a & 1 == 1) => (200, Some(json!({}))),
        None => (204, None),
    }
}

/// The tree as the studio stands now.
pub fn tree_of(s: &Shared) -> Value {
    tree(&entries(&s.controls, |d| feedback::value_of(s, d)))
}

/// The "oscquery" thread: idle (no socket) until switched on.
pub fn spawn(shared: Arc<Mutex<Shared>>, hub: Arc<NetHub>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("oscquery".into())
        .spawn(move || run(&shared, &hub, &running))
        .map_err(|e| log::warn!("OSCQuery : impossible de démarrer le thread : {e}"))
        .ok()
}

fn run(shared: &Mutex<Shared>, hub: &NetHub, running: &AtomicBool) {
    let mut server: Option<(SocketAddr, Server)> = None;
    let mut failed: Option<(SocketAddr, Instant)> = None;
    let mut bonjour: Option<(u16, Child)> = None;
    while running.load(Ordering::SeqCst) {
        let cfg = hub.effective();
        let want = cfg.oscquery_listen();
        if server.as_ref().map(|(a, _)| *a) != want {
            if server.take().is_some() {
                log::info!("OSCQuery : arrêté");
            }
            hub.update_oscquery(|st| (st.local, st.error) = (None, None));
            if let Some(addr) = want.filter(|a| failed.is_none_or(|(f, at)| f != *a || at.elapsed() >= RETRY)) {
                match crate::web::bind_with(addr, 0, 0, MAX_CONNECTIONS, OUTPUT_BUDGET) {
                    Ok(s) => {
                        let local = s.server_addr().to_ip().unwrap_or(addr);
                        log::info!("OSCQuery : en lecture seule sur http://{local}/");
                        hub.update_oscquery(|st| (st.local, st.error) = (Some(local.to_string()), None));
                        failed = None;
                        server = Some((addr, s));
                    }
                    Err(e) => {
                        let error = format!("impossible d'écouter sur {addr} : {e}");
                        log::warn!("OSCQuery : {error}");
                        hub.update_oscquery(|st| st.error = Some(error));
                        failed = Some((addr, Instant::now()));
                    }
                }
            }
        }
        // Bonjour: only when asked, never for loopback, never in tests.
        let local = server.as_ref().and_then(|(_, s)| s.server_addr().to_ip());
        let announce = local.filter(|l| cfg.oscquery.announce && !l.ip().is_loopback() && !cfg!(test)).map(|l| l.port());
        if bonjour.as_ref().map(|(p, _)| *p) != announce {
            if let Some((_, mut child)) = bonjour.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            bonjour = announce.and_then(|port| match Command::new("/usr/bin/dns-sd").args(["-R", "Laser Studio", SERVICE, "local", &port.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(child) => Some((port, child)),
                Err(e) => {
                    log::warn!("OSCQuery : annonce Bonjour impossible : {e}");
                    None
                }
            });
        }
        let Some((_, s)) = &server else {
            std::thread::sleep(POLL);
            continue;
        };
        let Ok(Some(request)) = s.recv_timeout(POLL) else { continue };
        let method = match request.method() {
            Method::Get => "GET",
            _ => "other",
        };
        let osc = hub.status(Proto::Osc).local.and_then(|l| l.parse().ok()).or_else(|| cfg.listen(Proto::Osc)).unwrap_or_else(|| SocketAddr::new(IpAddr::from([127, 0, 0, 1]), cfg.osc.port));
        let url = request.url().to_string();
        let (status, body) = answer(method, &url, || tree_of(&shared.lock().unwrap_or_else(|e| e.into_inner())), || host_info(osc));
        hub.update_oscquery(|st| {
            st.packets += 1;
            st.rejected += u64::from(status == 405);
            st.last_from = request.remote_addr().map(|a| a.ip().to_string());
            st.last_at = Some(Instant::now());
        });
        let response = match body {
            Some(v) => Response::from_string(v.to_string()).with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("valid header")),
            None => Response::from_string(String::new()),
        };
        let response = response.with_status_code(status);
        let response = if status == 405 { response.with_header(Header::from_bytes(&b"Allow"[..], &b"GET"[..]).expect("valid header")) } else { response };
        if let Err(e) = request.respond(response) {
            log::debug!("OSCQuery : réponse : {e}");
        }
    }
    if let Some((_, mut child)) = bonjour {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netin::osc::{self, OscArg, OscCommand};
    use crate::test_support::shared;

    fn all(s: &Shared) -> Vec<Entry> {
        entries(&s.controls, |d| feedback::value_of(s, d))
    }

    /// The arguments a client would send for an entry's TYPE.
    fn args_for(e: &Entry) -> Vec<OscArg> {
        match e.types {
            None => vec![],
            Some("f") => vec![OscArg::Float(e.range.as_ref().unwrap()[0]["MAX"].as_f64().unwrap() as f32)],
            Some("i") => vec![OscArg::Int(e.range.as_ref().unwrap()[0]["MAX"].as_i64().unwrap() as i32)],
            Some("T") => vec![OscArg::Bool(true)],
            Some("s") => vec![OscArg::Str("tunnels-001".into())],
            Some(t) => panic!("type {t}"),
        }
    }

    #[test]
    fn every_listed_address_is_accepted_by_the_decoder() {
        let s = shared();
        let list = all(&s);
        assert!(list.len() > 50, "{}", list.len());
        for e in &list {
            let msgs = osc::decode(&osc::encode(&e.path, &args_for(e))).unwrap();
            let cmd = osc::command(&msgs[0]).unwrap_or_else(|err| panic!("{} : {err}", e.path));
            assert_ne!(cmd, OscCommand::Arm, "{}", e.path);
            if let OscCommand::Control { id, .. } = &cmd {
                let d = s.controls.get(id).unwrap_or_else(|| panic!("{} → {id}", e.path));
                assert!(feedback::has_address(d), "{id}");
            }
        }
        // Every control with an address is listed, with its type and range.
        for d in s.controls.list().iter().filter(|d| feedback::has_address(d)) {
            let e = list.iter().find(|e| e.path == address_of(&d.id)).unwrap_or_else(|| panic!("{} missing", d.id));
            match d.kind {
                ControlKind::Continuous { min, max, .. } => {
                    assert_eq!(e.types, Some("f"));
                    assert_eq!(e.range, Some(json!([{ "MIN": min, "MAX": max }])));
                }
                ControlKind::Trigger => assert_eq!(e.types, None),
                _ => assert!(e.types.is_some(), "{}", d.id),
            }
        }
    }

    #[test]
    fn nothing_that_arms_resets_or_loosens_safety_is_listed() {
        let s = shared();
        let t = tree_of(&s);
        for e in all(&s) {
            let p = e.path.to_lowercase();
            assert!(!p.contains("arm"), "{p}");
            assert!(!p.split('/').any(|seg| seg == "enable" || seg == "armer" || seg == "reset_estop"), "{p}");
            assert!(!p.starts_with("/ls/safety/") || p == "/ls/safety/estop", "{p}: only the stop of the safety group");
            assert!(!p.starts_with("/ls/interlock") && !p.contains("estop/reset"), "{p}");
        }
        assert!(node(&t, "/ls/arm").is_none());
        assert!(node(&t, "/ls/transport/arm").is_none());
        assert!(node(&t, "/ls/safety/hold").is_none());
        assert!(node(&t, "/ls/blackout").is_some(), "stopping is always listed");
    }

    #[test]
    fn the_tree_follows_the_proposal() {
        let s = shared();
        let t = tree_of(&s);
        assert_eq!(t["FULL_PATH"], "/");
        let size = node(&t, "/ls/master/size").unwrap();
        assert_eq!(size["FULL_PATH"], "/ls/master/size");
        assert_eq!((size["TYPE"].as_str(), size["ACCESS"].as_u64()), (Some("f"), Some(3)));
        assert_eq!(size["RANGE"][0]["MAX"], 2.0);
        assert!(size["VALUE"][0].is_number());
        assert!(size.get("CONTENTS").is_none(), "a method");
        assert_eq!(node(&t, "/ls/master").unwrap()["FULL_PATH"], "/ls/master");
        assert!(node(&t, "/ls/master").unwrap()["CONTENTS"]["size"].is_object());
        let play = node(&t, "/ls/cue/play").unwrap();
        assert_eq!((play["TYPE"].as_str(), play["ACCESS"].as_u64()), (Some("s"), Some(2)));
        assert!(play["DESCRIPTION"].as_str().unwrap().contains("cue"));
        assert!(node(&t, "/ls/cue/stop").unwrap().get("TYPE").is_none(), "no argument: no TYPE");
    }

    #[test]
    fn queries_methods_and_host_info() {
        let s = shared();
        let t = tree_of(&s);
        let get = |url: &str| answer("GET", url, || t.clone(), || host_info("127.0.0.1:9000".parse().unwrap()));
        assert_eq!(get("/").0, 200);
        assert_eq!(get("/ls/master/size?TYPE"), (200, Some(json!({ "TYPE": "f" }))));
        assert_eq!(get("/ls/master/size/").0, 200);
        assert_eq!(get("/ls/nope").0, 404);
        assert_eq!(get("/ls/master/size?LISTEN").0, 400);
        assert_eq!(get("/ls/cue/stop?VALUE").0, 204, "write-only");
        assert_eq!(get("/ls/master?TYPE").0, 204, "a container has no type");
        let (code, host) = get("/?HOST_INFO");
        let host = host.unwrap();
        assert_eq!(code, 200);
        assert_eq!((host["OSC_PORT"].as_u64(), host["OSC_TRANSPORT"].as_str(), host["OSC_IP"].as_str()), (Some(9000), Some("UDP"), Some("127.0.0.1")));
        assert_eq!(host["EXTENSIONS"]["LISTEN"], false);
        assert!(host_info("0.0.0.0:8000".parse().unwrap()).get("OSC_IP").is_none());
        for m in ["POST", "PUT", "DELETE", "PATCH", "HEAD"] {
            assert_eq!(answer(m, "/", || panic!("never built"), || panic!("never built")), (405, None), "{m}");
        }
    }

    #[test]
    fn off_by_default_and_on_the_osc_address() {
        let mut cfg = NetInConfig::default();
        assert!(!cfg.oscquery.enabled && !cfg.oscquery.announce);
        assert_eq!(cfg.oscquery_listen(), None);
        cfg.oscquery.enabled = true;
        assert_eq!(cfg.oscquery_listen(), None, "only with the OSC input on");
        cfg.osc.enabled = true;
        assert_eq!(cfg.oscquery_listen(), Some("127.0.0.1:8001".parse().unwrap()), "OSC port + 1, OSC address");
        cfg.oscquery.port = Some(9123);
        assert_eq!(cfg.oscquery_listen(), Some("127.0.0.1:9123".parse().unwrap()));
        cfg.osc.port = 0;
        cfg.oscquery.port = None;
        assert_eq!(cfg.oscquery_listen().unwrap().port(), 0);
        // Older files without the section still load, switched off.
        let old: NetInConfig = serde_json::from_str(r#"{"osc":{"enabled":true}}"#).unwrap();
        assert_eq!(old.oscquery, OscQueryConfig::default());
    }

    /// The thread on 127.0.0.1 (port picked by the system): GET answers,
    /// POST is 405, switching off closes it.
    #[test]
    fn serves_on_loopback_only_get() {
        use std::io::{Read, Write};
        let s = shared();
        let hub = Arc::clone(&s.netin.hub);
        let mut cfg = NetInConfig::default();
        (cfg.osc.enabled, cfg.osc.port, cfg.oscquery.enabled, cfg.oscquery.port) = (true, 0, true, Some(0));
        hub.store(cfg.clone()).unwrap();
        let (shared, running) = (Arc::new(Mutex::new(s)), Arc::new(AtomicBool::new(true)));
        let thread = spawn(Arc::clone(&shared), Arc::clone(&hub), Arc::clone(&running)).unwrap();
        let start = Instant::now();
        let local = loop {
            if let Some(l) = hub.oscquery_status().local {
                break l.parse::<SocketAddr>().unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(5), "never listened");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(local.ip().is_loopback());
        let http = |req: &str| {
            let mut c = std::net::TcpStream::connect(local).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        let got = http("GET /ls/master/size?TYPE HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
        assert!(got.contains(r#"{"TYPE":"f"}"#), "{got}");
        let got = http("POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        assert!(got.starts_with("HTTP/1.1 405"), "{got}");
        let got = http("PUT /ls/master/size HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(got.starts_with("HTTP/1.1 405"), "{got}");
        cfg.oscquery.enabled = false;
        hub.store(cfg).unwrap();
        let start = Instant::now();
        while hub.oscquery_status().local.is_some() {
            assert!(start.elapsed() < Duration::from_secs(5), "never closed");
            std::thread::sleep(Duration::from_millis(20));
        }
        running.store(false, Ordering::SeqCst);
        thread.join().unwrap();
    }
}
