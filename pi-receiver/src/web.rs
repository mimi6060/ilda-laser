//! Local control panel: a tiny HTTP server (no JS framework, no build step)
//! so you can pick a pattern/color/speed, type laser text, save/play named
//! scenes, run them as an auto-advancing playlist, and adjust projector
//! calibration - all from a phone or PC browser on the same WiFi as the
//! Pi, without needing `pc-client` at all.
//!
//! A dedicated player thread walks the active pattern's points out to the
//! DACs at the chosen rate; a separate playlist thread advances through
//! saved scenes on a timer. Both just replace the shared "what to play
//! right now" state - HTTP handlers do the same thing manual controls do.

use crate::dac_sink::{Calibration, DacSink};
use crate::font;
use crate::patterns::{self, Point};
use crate::scenes::{Scene, SceneContent, SceneStore};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Response, Server};

struct ControlState {
    pattern_points: Vec<Point>,
    pattern_pps: u32,
    pattern_epoch: u64,
    scenes: SceneStore,
    playlist_active: bool,
    playlist_index: usize,
    playlist_scene_started: Instant,
}

impl ControlState {
    fn new(scenes: SceneStore) -> Self {
        Self {
            pattern_points: Vec::new(),
            pattern_pps: 4000,
            pattern_epoch: 0,
            scenes,
            playlist_active: false,
            playlist_index: 0,
            playlist_scene_started: Instant::now(),
        }
    }

    fn set_pattern(&mut self, points: Vec<Point>, pps: u32) {
        self.pattern_points = points;
        self.pattern_pps = pps.max(1);
        self.pattern_epoch += 1;
    }

    /// Manual controls (pattern/text/single-scene play) take over from
    /// whatever the playlist was doing, same as hitting "Go" in a cue-list
    /// based show controller overrides the running list.
    fn stop_playlist(&mut self) {
        self.playlist_active = false;
    }
}

pub struct Config {
    pub bind_addr: String,
    pub scenes_path: std::path::PathBuf,
    pub calibration_path: std::path::PathBuf,
}

/// Starts the background player + playlist threads and the HTTP server.
/// Blocks serving requests until `running` is cleared (e.g. by Ctrl+C).
pub fn run(dac: Arc<Mutex<DacSink>>, config: Config, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let scenes = SceneStore::load_or_create(config.scenes_path);
    let state = Arc::new(Mutex::new(ControlState::new(scenes)));

    spawn_player(Arc::clone(&dac), Arc::clone(&state), Arc::clone(&running));
    spawn_playlist_driver(Arc::clone(&state), Arc::clone(&running));

    let server = Server::http(&config.bind_addr).map_err(|e| anyhow::anyhow!("{e}"))?;
    log::info!("web control panel listening on http://{}/", config.bind_addr);

    while running.load(Ordering::SeqCst) {
        let request = match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("HTTP server error: {e}");
                continue;
            }
        };

        let method = request.method().clone();
        let url = request.url().to_string();
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url.as_str(), ""),
        };
        let params = parse_query(query);

        let response = match (&method, path) {
            (Method::Get, "/") => html_response(&build_index_html()),
            (Method::Post, "/api/pattern") => handle_pattern(&params, &state),
            (Method::Post, "/api/text") => handle_text(&params, &state),
            (Method::Post, "/api/stop") => handle_stop(&dac, &state),
            (Method::Get, "/api/scenes") => handle_list_scenes(&state),
            (Method::Post, "/api/scenes/save") => handle_save_scene(&params, &state),
            (Method::Post, "/api/scenes/delete") => handle_delete_scene(&params, &state),
            (Method::Post, "/api/scenes/play") => handle_play_scene(&params, &state),
            (Method::Post, "/api/playlist/start") => handle_playlist_start(&state),
            (Method::Post, "/api/playlist/stop") => handle_playlist_stop(&state),
            (Method::Get, "/api/calibration") => handle_get_calibration(&dac),
            (Method::Post, "/api/calibration") => {
                handle_set_calibration(&params, &dac, &config.calibration_path)
            }
            _ => text_response(404, "not found"),
        };

        if let Err(e) = request.respond(response) {
            log::warn!("failed to send HTTP response: {e}");
        }
    }

    dac.lock().unwrap().blank();
    Ok(())
}

/// Loads a previously-saved calibration from disk, or the identity
/// transform if there isn't one yet.
pub fn load_calibration(path: &std::path::Path) -> Calibration {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_calibration(path: &std::path::Path, cal: &Calibration) {
    if let Ok(json) = serde_json::to_string_pretty(cal) {
        if let Err(e) = std::fs::write(path, json) {
            log::warn!("failed to persist calibration to {}: {e}", path.display());
        }
    }
}

fn spawn_player(dac: Arc<Mutex<DacSink>>, state: Arc<Mutex<ControlState>>, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            let (points, pps, epoch) = {
                let s = state.lock().unwrap();
                (s.pattern_points.clone(), s.pattern_pps, s.pattern_epoch)
            };

            if points.is_empty() {
                thread::sleep(Duration::from_millis(200));
                continue;
            }

            let frame_delay = Duration::from_secs_f32(1.0 / pps as f32);
            for p in &points {
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                if state.lock().unwrap().pattern_epoch != epoch {
                    break; // pattern changed mid-loop, restart from the top
                }
                dac.lock().unwrap().write_point(p.x, p.y, p.r, p.g, p.b);
                thread::sleep(frame_delay);
            }
        }
    });
}

fn spawn_playlist_driver(state: Arc<Mutex<ControlState>>, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));

            let mut s = state.lock().unwrap();
            if !s.playlist_active || s.scenes.list().is_empty() {
                continue;
            }

            let duration = s.scenes.list()[s.playlist_index % s.scenes.list().len()].duration_secs;
            if s.playlist_scene_started.elapsed().as_secs_f32() < duration.max(0.5) {
                continue;
            }

            let count = s.scenes.list().len();
            s.playlist_index = (s.playlist_index + 1) % count;
            let scene = s.scenes.list()[s.playlist_index].clone();
            let points = scene.resolve();
            s.set_pattern(points, scene.pps);
            s.playlist_scene_started = Instant::now();
            log::info!("playlist advanced to '{}'", scene.name);
        }
    });
}

fn color_params(params: &HashMap<String, String>) -> (f32, f32, f32) {
    let get = |k: &str| params.get(k).and_then(|v| v.parse::<u8>().ok()).unwrap_or(255) as f32 / 255.0;
    (get("r"), get("g"), get("b"))
}

fn scale_param(params: &HashMap<String, String>) -> f32 {
    params
        .get("scale")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(80.0)
        .clamp(0.0, 100.0)
        / 100.0
}

fn pps_param(params: &HashMap<String, String>) -> u32 {
    params
        .get("pps")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(4000)
        .clamp(200, 30_000)
}

fn handle_pattern(params: &HashMap<String, String>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let shape = params.get("shape").map(String::as_str).unwrap_or("circle");
    let (r, g, b) = color_params(params);
    let scale = scale_param(params);
    let pps = pps_param(params);

    match patterns::by_name(shape, scale, r, g, b) {
        Some(points) => {
            let mut s = state.lock().unwrap();
            s.stop_playlist();
            s.set_pattern(points, pps);
            text_response(200, "ok")
        }
        None => text_response(400, "unknown shape"),
    }
}

fn handle_text(params: &HashMap<String, String>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let text = params.get("text").cloned().unwrap_or_default();
    let (r, g, b) = color_params(params);
    let scale = scale_param(params);
    let pps = pps_param(params);

    let points = font::text_to_points(&text, scale, r, g, b);
    let mut s = state.lock().unwrap();
    s.stop_playlist();
    s.set_pattern(points, pps);
    text_response(200, "ok")
}

fn handle_stop(dac: &Arc<Mutex<DacSink>>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let mut s = state.lock().unwrap();
    s.stop_playlist();
    s.pattern_points.clear();
    s.pattern_epoch += 1;
    drop(s);
    dac.lock().unwrap().blank();
    text_response(200, "ok")
}

fn handle_list_scenes(state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let s = state.lock().unwrap();
    match serde_json::to_string(s.scenes.list()) {
        Ok(json) => json_response(200, json),
        Err(_) => text_response(500, "failed to serialize scenes"),
    }
}

fn handle_save_scene(params: &HashMap<String, String>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let Some(name) = params.get("name").filter(|n| !n.is_empty()) else {
        return text_response(400, "missing name");
    };
    let (r8, g8, b8) = (
        params.get("r").and_then(|v| v.parse().ok()).unwrap_or(255u8),
        params.get("g").and_then(|v| v.parse().ok()).unwrap_or(255u8),
        params.get("b").and_then(|v| v.parse().ok()).unwrap_or(255u8),
    );
    let scale = scale_param(params);
    let pps = pps_param(params);
    let duration_secs = params.get("duration").and_then(|v| v.parse().ok()).unwrap_or(5.0);

    let content = match params.get("kind").map(String::as_str) {
        Some("text") => SceneContent::Text { text: params.get("text").cloned().unwrap_or_default() },
        _ => SceneContent::Shape { shape: params.get("shape").cloned().unwrap_or_else(|| "circle".into()) },
    };

    let scene = Scene { name: name.clone(), content, r: r8, g: g8, b: b8, scale, pps, duration_secs };

    let mut s = state.lock().unwrap();
    match s.scenes.upsert(scene) {
        Ok(()) => text_response(200, "ok"),
        Err(e) => text_response(500, &format!("failed to save: {e}")),
    }
}

fn handle_delete_scene(params: &HashMap<String, String>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let Some(name) = params.get("name") else {
        return text_response(400, "missing name");
    };
    let mut s = state.lock().unwrap();
    match s.scenes.remove(name) {
        Ok(()) => text_response(200, "ok"),
        Err(e) => text_response(500, &format!("failed to delete: {e}")),
    }
}

fn handle_play_scene(params: &HashMap<String, String>, state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let Some(name) = params.get("name") else {
        return text_response(400, "missing name");
    };
    let mut s = state.lock().unwrap();
    let Some(scene) = s.scenes.get(name).cloned() else {
        return text_response(404, "no such scene");
    };
    s.stop_playlist();
    let points = scene.resolve();
    s.set_pattern(points, scene.pps);
    text_response(200, "ok")
}

fn handle_playlist_start(state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    let mut s = state.lock().unwrap();
    if s.scenes.list().is_empty() {
        return text_response(400, "no saved scenes");
    }
    s.playlist_active = true;
    s.playlist_index = 0;
    let scene = s.scenes.list()[0].clone();
    let points = scene.resolve();
    s.set_pattern(points, scene.pps);
    s.playlist_scene_started = Instant::now();
    text_response(200, "ok")
}

fn handle_playlist_stop(state: &Arc<Mutex<ControlState>>) -> HttpResponse {
    state.lock().unwrap().stop_playlist();
    text_response(200, "ok")
}

fn handle_get_calibration(dac: &Arc<Mutex<DacSink>>) -> HttpResponse {
    let cal = dac.lock().unwrap().calibration();
    match serde_json::to_string(&cal) {
        Ok(json) => json_response(200, json),
        Err(_) => text_response(500, "failed to serialize calibration"),
    }
}

fn handle_set_calibration(
    params: &HashMap<String, String>,
    dac: &Arc<Mutex<DacSink>>,
    path: &std::path::Path,
) -> HttpResponse {
    let f = |k: &str, default: f32| params.get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    let cal = Calibration {
        offset_x: f("offset_x", 0.0).clamp(-1.0, 1.0),
        offset_y: f("offset_y", 0.0).clamp(-1.0, 1.0),
        scale_x: f("scale_x", 1.0).clamp(0.1, 2.0),
        scale_y: f("scale_y", 1.0).clamp(0.1, 2.0),
        rotation_deg: f("rotation_deg", 0.0).clamp(-180.0, 180.0),
    };
    dac.lock().unwrap().set_calibration(cal);
    save_calibration(path, &cal);
    text_response(200, "ok")
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), url_decode(v)))
        .collect()
}

/// Minimal `application/x-www-form-urlencoded`-style decoding: `+` as
/// space and `%XX` escapes. Good enough for the query strings this UI
/// sends (URLSearchParams-encoded text included).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

fn text_response(status: u16, body: &str) -> HttpResponse {
    Response::from_string(body).with_status_code(status)
}

fn json_response(status: u16, body: String) -> HttpResponse {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static header is valid ASCII");
    Response::from_string(body).with_status_code(status).with_header(header)
}

fn html_response(body: &str) -> HttpResponse {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("static header is valid ASCII");
    Response::from_string(body).with_header(header)
}

const INDEX_HTML_TEMPLATE: &str = include_str!("web_index.html");

/// The HTML template has a `{{SHAPES}}` placeholder filled in from
/// `patterns::SHAPE_NAMES`, so the shape list only needs to be maintained
/// in one place. Recomputed per request - it's a cheap string replace, and
/// this is a low-traffic personal control panel, not worth caching.
fn build_index_html() -> String {
    let options: String = patterns::SHAPE_NAMES
        .iter()
        .map(|name| format!(r#"<option value="{name}">{}</option>"#, capitalize(name)))
        .collect();
    INDEX_HTML_TEMPLATE.replace("{{SHAPES}}", &options)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_reads_key_value_pairs() {
        let params = parse_query("shape=circle&r=255&g=0&b=10&scale=80&pps=4000");
        assert_eq!(params.get("shape").map(String::as_str), Some("circle"));
        assert_eq!(params.get("r").map(String::as_str), Some("255"));
        assert_eq!(params.get("pps").map(String::as_str), Some("4000"));
    }

    #[test]
    fn parse_query_handles_empty_string() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn url_decode_handles_plus_and_percent_escapes() {
        assert_eq!(url_decode("HELLO+WORLD"), "HELLO WORLD");
        assert_eq!(url_decode("100%25"), "100%");
    }

    #[test]
    fn build_index_html_embeds_all_shape_names() {
        let html = build_index_html();
        for name in patterns::SHAPE_NAMES {
            assert!(html.contains(&format!(r#"value="{name}""#)), "missing option for '{name}'");
        }
    }
}
