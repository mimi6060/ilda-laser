//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.
//!
//! One thread receives requests; `POST /api/estop` is handled right there,
//! lock-free, ahead of everything queued. All other requests go, in order,
//! to a single worker thread, so a slow handler never delays a stop.

use crate::controls;
use crate::cues::{ClickMode, CueSlot};
use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::generators::GENERATOR_NAMES;
use crate::interlock::{ArmSource, DisarmReason, EStop};
use crate::patterns::SHAPE_NAMES;
use crate::presets::CATEGORIES;
use crate::scenes::Scene;
use crate::{Playlist, Shared};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX_HTML: &str = include_str!("index.html");
/// The 3D beam view (preview only), an ES module the page loads on demand.
const BEAM3D_JS: &str = include_str!("beam3d.js");

/// Files the UI loads besides the page itself: (body, content type).
/// Everything is compiled in, so the studio works offline.
fn static_asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/" => Some((INDEX_HTML, "text/html; charset=utf-8")),
        "/beam3d.js" => Some((BEAM3D_JS, "text/javascript; charset=utf-8")),
        _ => None,
    }
}

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

pub fn run(addr: &str, shared: Arc<Mutex<Shared>>, estop: Arc<EStop>, calibration_path: PathBuf, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let server = Server::http(addr).map_err(|e| anyhow::anyhow!("failed to start web server on {addr}: {e}"))?;
    serve(server, shared, estop, calibration_path, running);
    Ok(())
}

fn serve(server: Server, shared: Arc<Mutex<Shared>>, estop: Arc<EStop>, calibration_path: PathBuf, running: Arc<AtomicBool>) {
    let (queue, pending) = std::sync::mpsc::channel::<Request>();
    let worker = std::thread::spawn({
        let shared = Arc::clone(&shared);
        move || {
            for mut request in pending {
                let response = route(&mut request, &shared, &calibration_path);
                if let Err(e) = request.respond(response) {
                    log::debug!("failed to send HTTP response: {e}");
                }
            }
        }
    });

    while running.load(Ordering::SeqCst) {
        let request = match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("HTTP server error: {e}");
                continue;
            }
        };
        if is_estop(request.method(), request.url()) {
            emergency_stop(request, &estop, &shared);
        } else if let Err(e) = queue.send(request) {
            log::warn!("HTTP worker gone: {e}");
        }
    }
    drop(queue);
    worker.join().ok();
}

fn is_estop(method: &Method, url: &str) -> bool {
    *method == Method::Post && url.split('?').next() == Some("/api/estop")
}

/// `POST /api/estop[?source=keyboard|ui]`: the body is never read or
/// validated. Trips the latch (which also disarms the DAC directly), then
/// records it in the gate only if the lock is free right now; otherwise
/// the engine does so at the top of its next frame.
fn emergency_stop(request: Request, estop: &EStop, shared: &Mutex<Shared>) {
    let source = request
        .url()
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("source=")))
        .map(ArmSource::parse)
        .unwrap_or(ArmSource::Api);
    // The source is only a label: a stop is accepted from anyone.
    estop.trip(source);
    if let Ok(mut s) = shared.try_lock() {
        let s = &mut *s;
        s.gate.sync_estop(&s.estop);
    }
    if let Err(e) = request.respond(ok()) {
        log::debug!("failed to send HTTP response: {e}");
    }
}

fn route(request: &mut Request, shared: &Arc<Mutex<Shared>>, calibration_path: &Path) -> HttpResponse {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    match (method, path.as_str()) {
        (Method::Get, p) if static_asset(p).is_some() => {
            let (body, content_type) = static_asset(p).expect("checked by the guard");
            with_type(Response::from_string(body), content_type)
        }
        (Method::Get, "/api/state") => state(shared),
        (Method::Get, "/api/frame") => frame(shared),
        (Method::Post, "/api/settings") => match body::<Settings>(request) {
            Ok(settings) => {
                let mut s = shared.lock().unwrap();
                // `?rev=N`: the look the page edited. If it has been replaced
                // since (playlist, cue, MIDI), the page's copy is stale and
                // must not be sent back over what plays now.
                if let Some(rev) = query_u64(request.url(), "rev") {
                    if rev != s.settings_rev {
                        return with_type(Response::from_string(json!({ "rev": s.settings_rev }).to_string()).with_status_code(409), "application/json");
                    }
                }
                controls::set_look(&mut s, settings);
                json_response(json!({ "rev": s.settings_rev }))
            }
            Err(e) => e,
        },
        (Method::Post, "/api/audio") => match body::<AudioFeatures>(request) {
            Ok(audio) => {
                let mut s = shared.lock().unwrap();
                s.audio = audio;
                s.audio_at = Instant::now();
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/arm") => {
            let s = shared.lock().unwrap();
            json_response(json!(s.gate.status(&s.estop)))
        }
        (Method::Post, "/api/arm") => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or(""));
                let mut s = shared.lock().unwrap();
                // A toggle is decided here, against the real state, so two quick
                // presses always mean on-then-off (never on-on from a stale page).
                let want = match (req.get("on").and_then(|v| v.as_bool()), req.get("toggle").and_then(|v| v.as_bool())) {
                    (Some(on), _) => on,
                    (None, Some(true)) => !s.gate.is_armed(),
                    _ => return text(400, "expected \"on\" or \"toggle\""),
                };
                if want {
                    match s.request_arm(source) {
                        Ok(()) => json_response(json!({ "armed": s.gate.is_armed() })),
                        Err(blocking) => with_status(json_response(json!({ "armed": false, "blocking": blocking })), 409),
                    }
                } else {
                    // Disarming is always accepted, whatever else the body says.
                    s.gate.disarm(DisarmReason::User, source);
                    json_response(json!({ "armed": false }))
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/estop/reset") => {
            let mut s = shared.lock().unwrap();
            let s = &mut *s;
            s.gate.reset_estop(&s.estop);
            ok()
        }
        (Method::Get, "/api/safety") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "settings": s.safety.get(), "defaults": crate::safety::SafetySettings::default(), "status": s.strobe }))
        }
        // Tighten-only for the strobe: looser values than the safe
        // defaults are refused with a French message (400).
        (Method::Post, "/api/safety") => match body::<crate::safety::SafetySettings>(request) {
            Ok(cfg) => match shared.lock().unwrap().safety.set(cfg) {
                Ok(()) => ok(),
                Err(e) => text(400, &e.to_string()),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/calibration") => match body::<Calibration>(request) {
            Ok(cal) => {
                let cal = Calibration {
                    offset_x: cal.offset_x.clamp(-1.0, 1.0),
                    offset_y: cal.offset_y.clamp(-1.0, 1.0),
                    scale_x: cal.scale_x.clamp(0.1, 2.0),
                    scale_y: cal.scale_y.clamp(0.1, 2.0),
                    rotation_deg: cal.rotation_deg.clamp(-180.0, 180.0),
                };
                shared.lock().unwrap().calibration = cal;
                save_calibration(calibration_path, &cal);
                ok()
            }
            Err(e) => e,
        },
        (Method::Post, "/api/scenes/save") => match body::<SaveScene>(request) {
            Ok(req) if !req.name.trim().is_empty() => {
                let mut s = shared.lock().unwrap();
                let scene = Scene {
                    name: req.name.trim().to_string(),
                    settings: s.settings.clone(),
                    duration_secs: req.duration_secs.clamp(0.5, 3600.0),
                };
                match s.scenes.upsert(scene) {
                    Ok(()) => ok(),
                    Err(e) => text(500, &format!("failed to save: {e}")),
                }
            }
            Ok(_) => text(400, "missing name"),
            Err(e) => e,
        },
        (Method::Post, "/api/scenes/delete") => match body::<NameRequest>(request) {
            Ok(req) => match shared.lock().unwrap().scenes.remove(&req.name) {
                Ok(()) => ok(),
                Err(e) => text(500, &format!("failed to delete: {e}")),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/scenes/play") => match body::<NameRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match s.scenes.get(&req.name).cloned() {
                    Some(scene) => {
                        controls::show_look(&mut s, scene.settings);
                        s.playlist = None;
                        ok()
                    }
                    None => text(404, "no such scene"),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/playlist/start") => {
            let mut s = shared.lock().unwrap();
            match s.scenes.list().first().cloned() {
                Some(first) => {
                    controls::show_look(&mut s, first.settings);
                    s.playlist = Some(Playlist { index: 0, started: Instant::now() });
                    ok()
                }
                None => text(400, "no saved scenes"),
            }
        }
        (Method::Get, "/api/presets") => {
            let s = shared.lock().unwrap();
            let list: Vec<_> = s.presets.iter().map(|p| json!({ "id": p.id, "name": p.name, "category": p.category })).collect();
            json_response(json!({ "categories": CATEGORIES, "presets": list }))
        }
        (Method::Post, "/api/presets/play") => match body::<IdRequest>(request) {
            Ok(req) => {
                if controls::play_preset(&mut shared.lock().unwrap(), &req.id) {
                    ok()
                } else {
                    text(404, "no such preset")
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/cue") => match body::<CueRequest>(request) {
            Ok(req) => {
                if controls::press_cue(&mut shared.lock().unwrap(), &req.id, req.mode, req.down) {
                    ok()
                } else {
                    text(404, "no such preset")
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/cues") => json_response(json!(shared.lock().unwrap().deck)),
        (Method::Post, "/api/cues/slot") => match body::<SlotRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                if !s.presets.iter().any(|p| p.id == req.id) {
                    return text(404, "no such preset");
                }
                s.deck.set_slot(&req.id, req.slot);
                s.deck.save();
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/layers") => json_response(json!(shared.lock().unwrap().mixer)),
        (Method::Post, "/api/layers") => match body::<crate::layers::Mixer>(request) {
            Ok(mut mixer) => {
                mixer.sanitize();
                let mut s = shared.lock().unwrap();
                s.mixer = mixer;
                s.mixer_dirty = true;
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/live") => json_response(json!(shared.lock().unwrap().live)),
        (Method::Post, "/api/live") => match body::<crate::live::LiveModifiers>(request) {
            Ok(live) => {
                let mut s = shared.lock().unwrap();
                s.live = live;
                s.live_dirty = true;
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/palettes") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "builtin": crate::live::builtin_palettes(), "user": s.palettes.list() }))
        }
        (Method::Post, "/api/palettes") => match body::<Vec<crate::live::Palette>>(request) {
            Ok(list) => match shared.lock().unwrap().palettes.set(list) {
                Ok(()) => ok(),
                Err(e) => text(400, &e.to_string()),
            },
            Err(e) => e,
        },
        (Method::Get, "/api/lfos") => {
            let s = shared.lock().unwrap();
            let targets: Vec<_> = s
                .controls
                .list()
                .iter()
                .filter_map(|d| {
                    let (min, max) = crate::lfo::modulatable(&s.controls, &d.id)?;
                    Some(json!({ "id": d.id, "label": d.label_fr, "group": d.group, "min": min, "max": max }))
                })
                .collect();
            json_response(json!({ "lfos": s.lfos.list(), "targets": targets, "max": crate::lfo::MAX_MODULATORS }))
        }
        (Method::Post, "/api/lfos") => match body::<Vec<crate::lfo::Modulator>>(request) {
            Ok(list) => {
                let s = &mut *shared.lock().unwrap();
                match s.lfos.set(list, &s.controls) {
                    Ok(()) => ok(),
                    Err(e) => text(400, &e.to_string()),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/controls") => json_response(json!(shared.lock().unwrap().controls.list())),
        (Method::Get, "/api/control-values") => {
            let s = shared.lock().unwrap();
            let values: serde_json::Map<String, serde_json::Value> = s
                .controls
                .list()
                .iter()
                .filter_map(|d| controls::current(&s, d).map(|v| (d.id.clone(), v)))
                .collect();
            json_response(json!({ "values": values, "cue_page": s.cue_page + 1, "active_cue": s.active_cue }))
        }
        (Method::Post, "/api/control") => match body::<ControlRequest>(request) {
            Ok(req) => {
                let input = match (req.value, req.norm) {
                    (Some(v), _) => controls::ControlInput::Value(v.as_f32()),
                    (None, Some(n)) => controls::ControlInput::Norm(n),
                    (None, None) => controls::ControlInput::Value(1.0),
                };
                // Treated like a controller: arming stays on /api/arm (the laser
                // button and Space), never on the generic control endpoint.
                match controls::apply(&mut shared.lock().unwrap(), &req.id, input, true) {
                    Ok(()) => ok(),
                    Err(controls::ControlError::Unknown(id)) => text(404, &format!("contrôle inconnu : {id}")),
                    Err(controls::ControlError::Refused(why)) => text(403, why),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/playlist/stop") => {
            shared.lock().unwrap().playlist = None;
            ok()
        }
        (method, p) if p.starts_with("/api/midi") => midi_route(request, shared, method == Method::Post, p),
        _ => text(404, "not found"),
    }
}

fn midi_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, post: bool, path: &str) -> HttpResponse {
    let mut raw = String::new();
    if post {
        if let Err(e) = request.as_reader().read_to_string(&mut raw) {
            return text(400, &format!("unreadable body: {e}"));
        }
    }
    match crate::midi::api::route(&mut shared.lock().unwrap(), post, path, &raw) {
        Some(crate::midi::api::Reply::Json(v)) => json_response(v),
        Some(crate::midi::api::Reply::Text(code, t)) => text(code, &t),
        None => text(404, "not found"),
    }
}

/// A cue button, key or pad: `down` true on press, false on release.
#[derive(Deserialize)]
struct CueRequest {
    id: String,
    #[serde(default = "yes")]
    down: bool,
    mode: Option<ClickMode>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct SlotRequest {
    id: String,
    #[serde(flatten)]
    slot: CueSlot,
}

#[derive(Deserialize)]
struct IdRequest {
    id: String,
}

/// `value` is a number or a boolean (native units); `norm` is 0..1.
#[derive(Deserialize)]
struct ControlRequest {
    id: String,
    value: Option<NumOrBool>,
    norm: Option<f32>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumOrBool {
    Num(f32),
    Bool(bool),
}

impl NumOrBool {
    fn as_f32(&self) -> f32 {
        match *self {
            NumOrBool::Num(v) => v,
            NumOrBool::Bool(b) => b as u8 as f32,
        }
    }
}

#[derive(Deserialize)]
struct NameRequest {
    name: String,
}

#[derive(Deserialize)]
struct SaveScene {
    name: String,
    duration_secs: f32,
}

fn state(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    let s = &mut *s;
    s.gate.sync_estop(&s.estop);
    let arm = s.gate.status(&s.estop);
    json_response(json!({
        "settings": s.settings,
        "settings_rev": s.settings_rev,
        "calibration": s.calibration,
        "safety": s.safety.get(),
        "armed": arm.armed,
        "estop": arm.estop.is_some(),
        "arm": arm,
        "output": s.output_name,
        "pps": s.pps,
        "shapes": SHAPE_NAMES,
        "generators": GENERATOR_NAMES,
        "scenes": s.scenes.list(),
        "tempo": s.tempo.state(s.now_s()),
        "playlist": s.playlist.as_ref().map(|p| p.index),
    }))
}

/// The current frame for the preview, as `[x, y, r, g, b]` rows rounded
/// to 3 decimals to keep the payload small at 30 requests a second.
fn frame(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    let s = &mut *s;
    s.gate.sync_estop(&s.estop);
    let arm = s.gate.status(&s.estop);
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let points: Vec<[f32; 5]> = s.frame.iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect();
    json_response(json!({
        "points": points,
        "output_lit": s.output_lit,
        "armed": arm.armed,
        "estop": arm.estop.is_some(),
        "arm": arm,
        "output": s.output_name,
        "output_error": s.output_error,
        "pps": s.pps,
        "playlist": s.playlist.as_ref().map(|p| p.index),
        "cue_page": s.cue_page,
        "active_cue": s.active_cue,
        "settings_rev": s.settings_rev,
        "cues": {
            "active": s.deck.active.iter().map(|a| json!({ "cue": a.cue, "held": a.held, "layer": a.layer })).collect::<Vec<_>>(),
            "shown": s.deck.visible().iter().map(|a| a.cue.as_str()).collect::<Vec<_>>(),
            "click_mode": s.deck.click_mode,
            "multi": s.deck.multi,
            "max_active": s.deck.max_active,
        },
        "layers": { "mixer": s.mixer, "mix": s.mix },
        "tempo": s.tempo.state(s.now_s()),
        "live": s.live,
        "lfos": lfo_positions(s),
        "strobe": s.strobe,
    }))
}

/// Where each modulator is in its cycle and its wave value, for the UI's
/// animated mini graphs.
fn lfo_positions(s: &Shared) -> Vec<serde_json::Value> {
    let t = s.now_s();
    let beat = s.tempo.beat_at(t);
    s.lfos.list().iter().map(|m| json!({ "phase": m.position(t, beat).1, "value": m.wave_at(t, beat) })).collect()
}

fn body<T: serde::de::DeserializeOwned>(request: &mut Request) -> Result<T, HttpResponse> {
    let mut raw = String::new();
    request.as_reader().read_to_string(&mut raw).map_err(|e| text(400, &format!("unreadable body: {e}")))?;
    serde_json::from_str(&raw).map_err(|e| text(400, &format!("invalid JSON: {e}")))
}

pub fn load_calibration(path: &Path) -> Calibration {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_calibration(path: &Path, cal: &Calibration) {
    match serde_json::to_string_pretty(cal) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                log::warn!("failed to save calibration to {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("failed to serialize calibration: {e}"),
    }
}

/// `key`'s value in the URL's query string, as a number.
fn query_u64(url: &str, key: &str) -> Option<u64> {
    url.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

fn ok() -> HttpResponse {
    text(200, "ok")
}

fn text(status: u16, body: &str) -> HttpResponse {
    Response::from_string(body).with_status_code(status)
}

fn with_status(response: HttpResponse, status: u16) -> HttpResponse {
    response.with_status_code(status)
}

fn json_response(value: serde_json::Value) -> HttpResponse {
    with_type(Response::from_string(value.to_string()), "application/json")
}

fn with_type(response: HttpResponse, content_type: &str) -> HttpResponse {
    let header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("content type is valid ASCII");
    response.with_header(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::TEST;
    use crate::test_support;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};

    /// A studio API on a free localhost port, without engine or output.
    struct TestServer {
        addr: SocketAddr,
        shared: Arc<Mutex<Shared>>,
        running: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start(test_interlock: bool) -> Self {
            let mut state = test_support::shared();
            if test_interlock {
                state.gate.register(TEST, "Verrou de test", false);
            }
            let estop = Arc::clone(&state.estop);
            let shared = Arc::new(Mutex::new(state));
            let server = Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let running = Arc::new(AtomicBool::new(true));
            let thread = std::thread::spawn({
                let (shared, running) = (Arc::clone(&shared), Arc::clone(&running));
                let calibration = std::env::temp_dir().join("laser-studio-test-unused/calibration.json");
                move || serve(server, shared, estop, calibration, running)
            });
            Self { addr, shared, running, thread: Some(thread) }
        }

        fn request(&self, method: &str, path: &str, body: &str) -> (u16, String) {
            http(self.addr, method, path, body)
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.running.store(false, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                t.join().ok();
            }
        }
    }

    fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let status = raw.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
    }

    fn arm_status(t: &TestServer) -> serde_json::Value {
        serde_json::from_str(&t.request("GET", "/api/arm", "").1).unwrap()
    }

    #[test]
    fn starts_disarmed_and_arms_on_request() {
        let t = TestServer::start(false);
        let st = arm_status(&t);
        assert_eq!(st["armed"], false);
        assert_eq!(st["last_disarm"]["reason_fr"], "Démarrage");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#).0, 200);
        let st = arm_status(&t);
        assert_eq!((st["armed"].clone(), st["source"].clone()), (json!(true), json!("keyboard")));
    }

    #[test]
    fn a_blocking_interlock_refuses_with_409_and_its_label() {
        let t = TestServer::start(true);
        let (status, body) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(status, 409);
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["blocking"], json!(["Verrou de test"]));
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn disarm_is_accepted_with_extra_fields_and_unknown_source() {
        let t = TestServer::start(false);
        t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false,"source":"??","foo":[1,2]}"#).0, 200);
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn midi_claimed_over_http_cannot_arm() {
        let t = TestServer::start(false);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"midi"}"#).0, 409);
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn estop_latches_until_reset_and_reset_never_arms() {
        let t = TestServer::start(false);
        t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#);
        // No body at all, then a body that is not JSON: both stop.
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert_eq!(t.request("POST", "/api/estop", "{not json").0, 200);
        let st = arm_status(&t);
        assert_eq!(st["armed"], false);
        assert_eq!(st["estop"]["source"], "keyboard");
        assert_eq!(st["last_disarm"]["reason"], "estop");
        let (status, body) = t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#);
        assert_eq!(status, 409);
        assert!(body.contains("Arrêt d'urgence"));

        assert_eq!(t.request("POST", "/api/estop/reset", "").0, 200);
        let st = arm_status(&t);
        assert_eq!((st["armed"].clone(), st["estop"].clone()), (json!(false), json!(null)));
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#).0, 200);
        assert_eq!(arm_status(&t)["armed"], true);
    }

    /// The stop is served, and latched, while another handler is stuck
    /// holding the shared lock (here: the test thread holds it).
    #[test]
    fn estop_jumps_ahead_of_a_busy_handler() {
        let t = TestServer::start(false);
        t.request("POST", "/api/arm", r#"{"on":true}"#);
        let estop = Arc::clone(&t.shared.lock().unwrap().estop);
        let guard = t.shared.lock().unwrap();
        let addr = t.addr;
        let slow = std::thread::spawn(move || http(addr, "POST", "/api/arm", r#"{"on":true}"#));
        std::thread::sleep(Duration::from_millis(100)); // the worker is now blocked on the lock
        let started = Instant::now();
        assert_eq!(t.request("POST", "/api/estop?source=ui", "").0, 200);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(estop.is_latched(), "latched while the lock was still held");
        drop(guard);
        // The queued arm runs after the stop and is refused.
        assert_eq!(slow.join().unwrap().0, 409);
        let st = arm_status(&t);
        assert_eq!(st["armed"], false);
        assert_eq!(st["last_disarm"]["reason"], "estop");
    }

    #[test]
    fn frame_reports_no_lit_output_while_disarmed_but_keeps_the_preview() {
        let t = TestServer::start(false);
        {
            let mut s = t.shared.lock().unwrap();
            s.frame = vec![crate::patterns::Point::lit(0.0, 0.0, 1.0, 0.0, 0.0)];
            s.output_lit = 0;
        }
        let f: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(f["points"].as_array().unwrap().len(), 1);
        assert_eq!(f["output_lit"], 0);
        assert_eq!(f["armed"], false);
    }

    #[test]
    fn only_post_estop_takes_the_fast_path() {
        assert!(is_estop(&Method::Post, "/api/estop"));
        assert!(is_estop(&Method::Post, "/api/estop?source=keyboard"));
        assert!(!is_estop(&Method::Get, "/api/estop"));
        assert!(!is_estop(&Method::Post, "/api/estop/reset"));
    }

    #[test]
    fn query_numbers() {
        assert_eq!(query_u64("/api/settings?rev=12", "rev"), Some(12));
        assert_eq!(query_u64("/api/settings?x=1&rev=3", "rev"), Some(3));
        assert_eq!(query_u64("/api/settings?revision=3", "rev"), None);
        assert_eq!(query_u64("/api/settings", "rev"), None);
    }

    #[test]
    fn serves_the_page_and_the_beam_view_module() {
        let (page, page_type) = static_asset("/").unwrap();
        assert!(page_type.starts_with("text/html"));
        assert!(page.contains("/beam3d.js"), "the page loads the 3D view from our own server");

        // Browsers refuse ES modules served with a non-JavaScript type.
        let (module, module_type) = static_asset("/beam3d.js").unwrap();
        assert!(module_type.starts_with("text/javascript"));
        assert!(module.contains("export class BeamView"));
        assert!(static_asset("/vendor/nothing.js").is_none());
    }

    #[test]
    fn the_beam_view_has_no_way_to_reach_the_server() {
        // Preview only (CLAUDE.md): the 3D view gets frames from the page and
        // must never fetch, post or arm anything itself.
        let (module, _) = static_asset("/beam3d.js").unwrap();
        for forbidden in ["fetch(", "XMLHttpRequest", "WebSocket", "/api/", "sendBeacon", "import("] {
            assert!(!module.contains(forbidden), "beam3d.js must not use {forbidden}");
        }
    }

    #[test]
    fn safety_settings_are_tighten_only_over_http() {
        let t = TestServer::start(false);
        let get = |t: &TestServer| -> serde_json::Value { serde_json::from_str(&t.request("GET", "/api/safety", "").1).unwrap() };
        assert_eq!(get(&t)["settings"]["strobe_max_hz"], 4.0);
        let (code, msg) = t.request("POST", "/api/safety", r#"{"strobe_max_hz":10}"#);
        assert_eq!(code, 400, "{msg}");
        assert!(msg.contains("Strobe max"), "{msg}");
        assert_eq!(get(&t)["settings"]["strobe_max_hz"], 4.0, "unchanged");
        let (code, _) = t.request("POST", "/api/safety", r#"{"strobe_max_hz":3,"beam_floor_y":0.25}"#);
        assert_eq!(code, 200);
        let st = get(&t);
        assert_eq!(st["settings"]["strobe_max_hz"], 3.0);
        assert_eq!(st["settings"]["beam_floor_y"], 0.25);
        assert_eq!(st["settings"]["strobe_burst_s"], 5.0, "missing fields keep the safe default");
        assert_eq!(st["status"]["active"], false);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["strobe"]["active"], false);
    }
}
