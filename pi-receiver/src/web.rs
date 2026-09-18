//! Local control panel: a tiny HTTP server (no JS framework, no build step)
//! so you can pick a pattern/color/speed from a phone or PC browser on the
//! same WiFi network as the Pi, without needing `pc-client` at all.
//!
//! A dedicated player thread owns actually walking the active pattern's
//! points out to the DACs at the chosen rate; HTTP handlers just replace
//! the shared "what to play right now" state.

use crate::dac_sink::DacSink;
use crate::patterns::{self, Point};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};

struct PatternState {
    points: Vec<Point>,
    pps: u32,
    epoch: u64,
}

impl PatternState {
    fn empty() -> Self {
        Self { points: Vec::new(), pps: 4000, epoch: 0 }
    }
}

/// Starts the background player thread and the HTTP server. Blocks forever
/// serving requests (call from a dedicated thread, or last, in `main`).
pub fn run(dac: Arc<Mutex<DacSink>>, bind_addr: &str, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(PatternState::empty()));

    spawn_player(Arc::clone(&dac), Arc::clone(&state), Arc::clone(&running));

    let server = Server::http(bind_addr).map_err(|e| anyhow::anyhow!("{e}"))?;
    log::info!("web control panel listening on http://{bind_addr}/");

    while running.load(Ordering::SeqCst) {
        let request = match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(r)) => r,
            Ok(None) => continue, // timed out, loop back to check `running`
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

        let response = match (&method, path) {
            (Method::Get, "/") => html_response(INDEX_HTML),
            (Method::Post, "/api/pattern") => handle_pattern(query, &state),
            (Method::Post, "/api/stop") => handle_stop(&dac, &state),
            _ => text_response(404, "not found"),
        };

        if let Err(e) = request.respond(response) {
            log::warn!("failed to send HTTP response: {e}");
        }
    }

    dac.lock().unwrap().blank();
    Ok(())
}

fn spawn_player(dac: Arc<Mutex<DacSink>>, state: Arc<Mutex<PatternState>>, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            let (points, pps, epoch) = {
                let s = state.lock().unwrap();
                (s.points.clone(), s.pps.max(1), s.epoch)
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
                if state.lock().unwrap().epoch != epoch {
                    break; // pattern changed mid-loop, restart from the top
                }
                dac.lock().unwrap().write_point(p.x, p.y, p.r, p.g, p.b);
                thread::sleep(frame_delay);
            }
        }
    });
}

fn handle_pattern(query: &str, state: &Arc<Mutex<PatternState>>) -> Response<std::io::Cursor<Vec<u8>>> {
    let params = parse_query(query);
    let shape = params.get("shape").map(String::as_str).unwrap_or("circle");
    let r = params.get("r").and_then(|v| v.parse::<u8>().ok()).unwrap_or(255) as f32 / 255.0;
    let g = params.get("g").and_then(|v| v.parse::<u8>().ok()).unwrap_or(255) as f32 / 255.0;
    let b = params.get("b").and_then(|v| v.parse::<u8>().ok()).unwrap_or(255) as f32 / 255.0;
    let scale = params
        .get("scale")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(80.0)
        .clamp(0.0, 100.0)
        / 100.0;
    let pps = params
        .get("pps")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(4000)
        .clamp(200, 30_000);

    match patterns::by_name(shape, scale, r, g, b) {
        Some(points) => {
            let mut s = state.lock().unwrap();
            s.points = points;
            s.pps = pps;
            s.epoch += 1;
            text_response(200, "ok")
        }
        None => text_response(400, "unknown shape"),
    }
}

fn handle_stop(dac: &Arc<Mutex<DacSink>>, state: &Arc<Mutex<PatternState>>) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut s = state.lock().unwrap();
    s.points.clear();
    s.epoch += 1;
    drop(s);
    dac.lock().unwrap().blank();
    text_response(200, "ok")
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn text_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_status_code(status)
}

fn html_response(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("static header is valid ASCII");
    Response::from_string(body).with_header(header)
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Pi Laser</title>
<style>
  body { font-family: system-ui, sans-serif; background: #111; color: #eee; margin: 0; padding: 16px; }
  h1 { font-size: 1.2rem; margin: 0 0 16px; }
  label { display: block; margin: 16px 0 4px; font-size: 0.9rem; color: #aaa; }
  select, input[type=range] { width: 100%; box-sizing: border-box; }
  input[type=color] { width: 100%; height: 44px; border: none; background: none; }
  .row { display: flex; gap: 8px; margin-top: 20px; }
  button { flex: 1; padding: 14px; font-size: 1rem; border: none; border-radius: 8px; }
  #apply { background: #2d6; color: #032; font-weight: bold; }
  #stop { background: #d33; color: #fff; font-weight: bold; }
  .val { color: #6cf; font-variant-numeric: tabular-nums; }
</style>
</head>
<body>
<h1>Pi Laser control</h1>

<label>Shape</label>
<select id="shape">
  <option value="circle">Circle</option>
  <option value="square">Square</option>
  <option value="triangle">Triangle</option>
  <option value="cross">Cross (calibration)</option>
</select>

<label>Color</label>
<input type="color" id="color" value="#ffffff">

<label>Size: <span class="val" id="scaleVal">80</span>%</label>
<input type="range" id="scale" min="0" max="100" value="80">

<label>Speed: <span class="val" id="ppsVal">4000</span> pts/sec</label>
<input type="range" id="pps" min="500" max="20000" step="100" value="4000">

<div class="row">
  <button id="apply">Apply</button>
  <button id="stop">Stop</button>
</div>

<script>
const $ = id => document.getElementById(id);
$('scale').oninput = () => $('scaleVal').textContent = $('scale').value;
$('pps').oninput = () => $('ppsVal').textContent = $('pps').value;

function hexToRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

$('apply').onclick = () => {
  const [r, g, b] = hexToRgb($('color').value);
  const params = new URLSearchParams({
    shape: $('shape').value,
    r, g, b,
    scale: $('scale').value,
    pps: $('pps').value,
  });
  fetch('/api/pattern?' + params.toString(), { method: 'POST' });
};

$('stop').onclick = () => fetch('/api/stop', { method: 'POST' });
</script>
</body>
</html>
"##;

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
}
