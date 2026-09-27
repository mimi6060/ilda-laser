//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.

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
                s.settings = settings;
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
                shared.lock().unwrap().armed = req.on;
                ok()
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
                        s.settings = scene.settings;
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
                    s.settings = first.settings;
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
                let mut s = shared.lock().unwrap();
                match s.presets.iter().find(|p| p.id == req.id).map(|p| p.settings.clone()) {
                    Some(mut settings) => {
                        // A cue sets the look, not the operator's safety and music choices:
                        // keep the current brightness, and the current music settings
                        // unless the cue is built around the music.
                        settings.brightness = s.settings.brightness;
                        if !settings.audio.enabled {
                            settings.audio = s.settings.audio.clone();
                        }
                        s.settings = settings;
                        s.playlist = None;
                        ok()
                    }
                    None => text(404, "no such preset"),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/playlist/stop") => {
            shared.lock().unwrap().playlist = None;
            ok()
        }
        _ => text(404, "not found"),
    }
}

#[derive(Deserialize)]
struct ArmRequest {
    on: bool,
}

#[derive(Deserialize)]
struct IdRequest {
    id: String,
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
