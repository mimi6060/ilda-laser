//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.

use crate::controls;
use crate::cues::{ClickMode, CueSlot};
use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::generators::GENERATOR_NAMES;
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

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

pub fn run(addr: &str, shared: Arc<Mutex<Shared>>, calibration_path: PathBuf, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let server = Server::http(addr).map_err(|e| anyhow::anyhow!("failed to start web server on {addr}: {e}"))?;

    while running.load(Ordering::SeqCst) {
        let mut request = match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("HTTP server error: {e}");
                continue;
            }
        };
        let response = route(&mut request, &shared, &calibration_path);
        if let Err(e) = request.respond(response) {
            log::debug!("failed to send HTTP response: {e}");
        }
    }
    Ok(())
}

fn route(request: &mut Request, shared: &Arc<Mutex<Shared>>, calibration_path: &Path) -> HttpResponse {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    match (method, path.as_str()) {
        (Method::Get, "/") => with_type(Response::from_string(INDEX_HTML), "text/html; charset=utf-8"),
        (Method::Get, "/api/state") => state(shared),
        (Method::Get, "/api/frame") => frame(shared),
        (Method::Post, "/api/settings") => match body::<Settings>(request) {
            Ok(settings) => {
                let mut s = shared.lock().unwrap();
                // With cues playing this edits the newest one; otherwise it
                // is the manual look, shown again.
                s.settings = settings;
                s.look_on |= s.deck.active.is_empty();
                s.playlist = None; // a manual change takes over from the playlist
                ok()
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
        (Method::Post, "/api/arm") => match body::<ArmRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                // A toggle is decided here, against the real state, so two quick
                // presses always mean on-then-off (never on-on from a stale page).
                s.armed = match (req.on, req.toggle) {
                    (Some(on), _) => on,
                    (None, true) => !s.armed,
                    (None, false) => return text(400, "expected \"on\" or \"toggle\""),
                };
                json_response(json!({ "armed": s.armed }))
            }
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

#[derive(Deserialize)]
struct ArmRequest {
    on: Option<bool>,
    #[serde(default)]
    toggle: bool,
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
    let s = shared.lock().unwrap();
    json_response(json!({
        "settings": s.settings,
        "calibration": s.calibration,
        "armed": s.armed,
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
    let s = shared.lock().unwrap();
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let points: Vec<[f32; 5]> = s.frame.iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect();
    json_response(json!({
        "points": points,
        "armed": s.armed,
        "output": s.output_name,
        "output_error": s.output_error,
        "pps": s.pps,
        "playlist": s.playlist.as_ref().map(|p| p.index),
        "cue_page": s.cue_page,
        "active_cue": s.active_cue,
        "cues": {
            "active": s.deck.active.iter().map(|a| json!({ "cue": a.cue, "held": a.held })).collect::<Vec<_>>(),
            "shown": s.deck.visible().iter().map(|a| a.cue.as_str()).collect::<Vec<_>>(),
            "click_mode": s.deck.click_mode,
            "multi": s.deck.multi,
            "max_active": s.deck.max_active,
        },
        "tempo": s.tempo.state(s.now_s()),
        "live": s.live,
    }))
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

fn ok() -> HttpResponse {
    text(200, "ok")
}

fn text(status: u16, body: &str) -> HttpResponse {
    Response::from_string(body).with_status_code(status)
}

fn json_response(value: serde_json::Value) -> HttpResponse {
    with_type(Response::from_string(value.to_string()), "application/json")
}

fn with_type(response: HttpResponse, content_type: &str) -> HttpResponse {
    let header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("content type is valid ASCII");
    response.with_header(header)
}
