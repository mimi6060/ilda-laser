//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.
//!
//! One thread receives requests; `POST /api/estop` is handled right there,
//! lock-free, ahead of everything queued. All other requests go, in order,
//! to a single worker thread, so a slow handler never delays a stop. The
//! requests that may decode a song (import, waveform, attach: seconds, up
//! to the decoding time limit) get their own thread, so they never hold up
//! the heartbeats queued behind them (T-298: a slow or hanging decode must
//! not disarm the laser as « Interface perdue »).

use crate::controls;
use crate::cues::{ClickMode, CueSlot};
use crate::engine::{Calibration, Settings};
use crate::generators::GENERATOR_NAMES;
use crate::interlock::{ArmSource, DisarmReason, EStop};
use crate::patterns::SHAPE_NAMES;
use crate::presets::CATEGORIES;
use crate::scenes::Scene;
use crate::{Playlist, Shared};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    // Draw the cue pictures now, beside the worker, so the first page
    // doesn't hold the heartbeat queue while they render.
    let catalogue: Vec<_> = shared.lock().unwrap().presets.iter().map(|p| (p.id.clone(), p.settings.clone())).collect();
    std::thread::spawn(move || cue_thumbnails(&catalogue));
    let worker = std::thread::spawn({
        let shared = Arc::clone(&shared);
        let decoding = Arc::new(AtomicUsize::new(0));
        move || {
            for mut request in pending {
                if may_decode(request.method(), request.url()) {
                    decode_aside(request, &shared, &calibration_path, &decoding);
                    continue;
                }
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

/// Requests whose handler may run for seconds (audio decoding, figure
/// import): handled beside the single HTTP worker.
fn may_decode(method: &Method, url: &str) -> bool {
    let path = url.split('?').next().unwrap_or("");
    // Figure import (SVG/image vectorisation) can take seconds too: kept off
    // the single worker so heartbeats queued behind it aren't delayed into a
    // « Interface perdue » disarm.
    matches!(
        (method, path),
        (Method::Post, "/api/media/audio")
            | (Method::Get, "/api/timeline/waveform")
            | (Method::Post, "/api/timeline/audio")
            | (Method::Post, "/api/figures/import")
            // ILDA import parses up to 64 MB; an export renders and encodes
            // up to `MAX_EXPORT_FRAMES` images (T-011).
            | (Method::Post, "/api/media/ilda")
            | (Method::Get, "/api/media/ilda/export")
            // A whole SD bank: up to 229 cues rendered and written (T-302).
            | (Method::Post, "/api/sdbank/export")
    )
}

/// At most this many decoding requests at once (each may run a decoder).
const MAX_DECODING: usize = 4;

/// Handles a request that may decode on its own thread (see the module
/// doc). Beyond `MAX_DECODING` at once: 503.
fn decode_aside(mut request: Request, shared: &Arc<Mutex<Shared>>, calibration_path: &Path, decoding: &Arc<AtomicUsize>) {
    /// One decoding request in flight, released however its thread ends.
    struct Slot(Arc<AtomicUsize>);
    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let slot = Slot(Arc::clone(decoding));
    if decoding.fetch_add(1, Ordering::SeqCst) >= MAX_DECODING {
        drop(slot);
        let _ = request.respond(text(503, "trop de traitements longs en cours (décodage audio, import), réessayez"));
        return;
    }
    let (shared, calibration_path) = (Arc::clone(shared), calibration_path.to_path_buf());
    let spawned = std::thread::Builder::new().name("http-decode".into()).spawn(move || {
        let response = route(&mut request, &shared, &calibration_path);
        drop(slot);
        if let Err(e) = request.respond(response) {
            log::debug!("failed to send HTTP response: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("no thread for a decoding request: {e}");
    }
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
        s.sync_audience();
    }
    if let Err(e) = request.respond(ok()) {
        log::debug!("failed to send HTTP response: {e}");
    }
}

fn route(request: &mut Request, shared: &Arc<Mutex<Shared>>, calibration_path: &Path) -> HttpResponse {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    // Show mode (T-283): only playback and safety routes run (showmode.rs).
    if method == Method::Post && !crate::showmode::allowed(&path) && shared.lock().unwrap().show_mode.on {
        return locked();
    }
    match (method, path.as_str()) {
        (Method::Get, p) if static_asset(p).is_some() => {
            let (body, content_type) = static_asset(p).expect("checked by the guard");
            with_type(Response::from_string(body), content_type)
        }
        (Method::Get, "/api/state") => state(shared),
        (Method::Get, "/api/diagnostics/report") => diagnostic_report(request.url(), shared),
        (Method::Get, "/api/logs/tail") => {
            let n = query_u64(request.url(), "n").unwrap_or(200).clamp(1, 1000) as usize;
            let app = Arc::clone(&shared.lock().unwrap().applog);
            json_response(json!({ "lines": app.tail(n), "status": app.status() }))
        }
        (Method::Get, "/api/frame") => frame(shared),
        (Method::Get, "/api/data/warnings") => {
            let mut s = shared.lock().unwrap();
            s.sync_data_warnings();
            json_response(data_warnings_view(&s))
        }
        // The operator has seen a damaged-file warning (T-402): `{file}` or
        // `{all: true}`. It restores nothing; it reopens `data_ack` once
        // every safety file's warning is acknowledged. Never arms.
        (Method::Post, "/api/data/warnings/ack") => match body::<DataAck>(request) {
            Ok(DataAck { file, all }) if all || file.is_some() => {
                let mut s = shared.lock().unwrap();
                let n = s.ack_data_warnings(if all { None } else { file.as_deref() });
                let mut v = data_warnings_view(&s);
                v["acknowledged"] = json!(n);
                json_response(v)
            }
            Ok(_) => text(400, "expected \"file\" or \"all\": true"),
            Err(e) => e,
        },
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
        (Method::Post, "/api/audio") => match body::<serde_json::Value>(request) {
            // The page's features: `{level, bass, beat}` as before, or the
            // whole v2 snapshot (T-237).
            Ok(v) => {
                let mut s = shared.lock().unwrap();
                match crate::audio::browser_features(&v, s.now_s()) {
                    Ok(audio) => {
                        s.audio = audio;
                        s.audio_at = Instant::now();
                        ok()
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/audio/state") => {
            // `/api/state.audio` alone: what the « Musique » panel polls
            // (T-243), a few hundred bytes instead of the whole state.
            let s = shared.lock().unwrap();
            json_response(s.audio_in.view(s.audio, s.audio_at, Instant::now()))
        }
        (Method::Get, "/api/audio/spectrum") => {
            let hub = Arc::clone(&shared.lock().unwrap().audio_in);
            json_response(hub.spectrum_view(Instant::now()))
        }
        (Method::Get, "/api/audio/devices") => {
            let hub = Arc::clone(&shared.lock().unwrap().audio_in);
            // The capture thread's last scan: this handler never calls CoreAudio.
            json_response(json!({ "capture": hub.capture_enabled(), "devices": hub.devices() }))
        }
        (Method::Get, "/api/devices") => {
            // The last scan (devices.rs): this handler never scans or opens a DAC.
            let list = Arc::clone(&shared.lock().unwrap().devices);
            json_response(list.view())
        }
        (Method::Post, "/api/devices/scan") => {
            // On its own thread (~1.5 s): poll GET /api/devices for the result.
            let list = Arc::clone(&shared.lock().unwrap().devices);
            list.refresh();
            json_response(list.view())
        }
        (Method::Get, "/api/outputs") => {
            // The manager's own state and the last scan: never scans or opens.
            let mgr = Arc::clone(&shared.lock().unwrap().output_mgr);
            json_response(mgr.view(Instant::now()))
        }
        (Method::Post, "/api/outputs/scan") => {
            // Same scan as /api/devices/scan, on its own thread.
            let mgr = Arc::clone(&shared.lock().unwrap().output_mgr);
            mgr.devices().refresh();
            json_response(mgr.view(Instant::now()))
        }
        (Method::Post, "/api/outputs/select") => match body::<serde_json::Value>(request) {
            Ok(req) => select_output(&req, shared),
            Err(e) => e,
        },
        (Method::Post, "/api/outputs/config") => match body::<OutputConfigBody>(request) {
            Ok(b) => {
                let mut s = shared.lock().unwrap();
                let mgr = Arc::clone(&s.output_mgr);
                if let Some(on) = b.auto_reconnect {
                    mgr.set_auto_reconnect(on, Instant::now());
                    log::info!("Reconnexion automatique de la sortie : {}", if on { "activée" } else { "désactivée" });
                }
                if let Err(e) = s.outputs.set_connection(mgr.prefs()) {
                    log::warn!("{e:#}");
                }
                json_response(mgr.view(Instant::now()))
            }
            Err(e) => e,
        },
        (Method::Get, "/api/outputs/artnet") => json_response(artnet_view(&shared.lock().unwrap())),
        (Method::Post, "/api/outputs/artnet") => match body::<crate::artnet::ShowNetDmxConfig>(request) {
            Ok(cfg) => set_artnet(cfg, shared),
            Err(e) => e,
        },
        (Method::Post, "/api/test/output") => {
            // `--test-hooks` only (e2e of T-401): unplugs or plugs back the
            // fake outputs. A real output has no such switch.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<serde_json::Value>(request) {
                Ok(req) => {
                    let dropped = match req.get("action").and_then(|v| v.as_str()) {
                        Some("drop") => true,
                        Some("restore") => false,
                        _ => return text(400, "expected \"action\": \"drop\" or \"restore\""),
                    };
                    let mgr = Arc::clone(&shared.lock().unwrap().output_mgr);
                    if mgr.simulate_drop(dropped) {
                        ok()
                    } else {
                        text(404, "no fake output")
                    }
                }
                Err(e) => e,
            }
        }
        (Method::Get, "/api/audio/config") => {
            let hub = Arc::clone(&shared.lock().unwrap().audio_in);
            json_response(json!(hub.config().0))
        }
        (Method::Post, "/api/audio/config") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let hub = Arc::clone(&shared.lock().unwrap().audio_in);
                // Only the fields sent change; the reply is what was kept.
                match hub.config().0.patched(&patch).and_then(|c| hub.set_config(c)) {
                    Ok(config) => json_response(json!(config)),
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/audio/tempo/new_track") => {
            // *Nouveau morceau*: the native tempo estimator forgets its
            // history and guide (applied by the analysis thread; never
            // blocks), the clock its follower history.
            shared.lock().unwrap().tempo_new_track();
            ok()
        }
        (Method::Post, "/api/heartbeat") => match body::<Heartbeat>(request) {
            Ok(hb) => {
                let mut s = shared.lock().unwrap();
                if hb.gone {
                    s.presence.leave(&hb.client_id);
                } else {
                    s.presence.beat(&hb.client_id, Instant::now(), hb.visible, hb.hold);
                }
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/presence") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "settings": s.presence.settings, "status": s.presence.status() }))
        }
        (Method::Post, "/api/presence") => match body::<crate::presence::PresenceSettings>(request) {
            Ok(settings) => {
                let mut s = shared.lock().unwrap();
                // Out-of-range values are clamped (ui_timeout_ms ≤ 10 000);
                // the reply is what was kept.
                let before = s.presence.settings.clone();
                s.presence.set_settings(settings);
                if let Some(change) = crate::safety_log::settings_change(&before, &s.presence.settings) {
                    s.gate.log().record("presence_settings", Some("ui"), change);
                }
                // Hold-to-run off ends audience scanning (T-255).
                s.sync_audience();
                json_response(json!(s.presence.settings))
            }
            Err(e) => e,
        },
        (Method::Get, "/api/lock") => json_response(lock_view(&shared.lock().unwrap())),
        // T-283: `{"show_mode": true|false, "source": "ui"}`. Never arms or
        // disarms; entering and leaving go to the safety log.
        (Method::Post, "/api/lock") => match body::<LockRequest>(request) {
            Ok(req) => match crate::showmode::set(shared, req.show_mode, req.source.as_deref().unwrap_or("ui")) {
                Ok(()) => json_response(lock_view(&shared.lock().unwrap())),
                Err(e) => with_status(json_response(json!({ "error": format!("{e:#}"), "lock": lock_view(&shared.lock().unwrap()) })), 500),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/keep_awake") => match body::<KeepAwakeRequest>(request) {
            // RÉGLAGES › Général (T-412): the setting only; never arms.
            Ok(req) => match crate::keep_awake::set_enabled(shared, req.enabled) {
                Ok(()) => json_response(json!(shared.lock().unwrap().keep_awake)),
                Err(e) => text(500, &format!("{e:#}")),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/test/wifi") => {
            // `--test-hooks` only (e2e of T-472): what the Wi-Fi check of a
            // ShowNET output finds, `{ "wireless": bool, "interface"? }`.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<crate::wifi::WifiStatus>(request) {
                Ok(status) => {
                    shared.lock().unwrap().wifi.simulate(Some(status));
                    ok()
                }
                Err(e) => e,
            }
        }
        (Method::Post, "/api/test/stall") => {
            let ms = query_u64(request.url(), "ms").unwrap_or(200).min(2_000);
            let mut s = shared.lock().unwrap();
            if !s.test_hooks {
                return text(404, "not found");
            }
            s.test_stall_ms = ms;
            ok()
        }
        (Method::Post, "/api/test/tempo_estimate") => {
            // `--test-hooks` only (e2e of Tempo auto, T-234): publishes a
            // simulated detector estimate as if from the native analysis,
            // its beats on the grid `offset_s + n·60/bpm` (studio time).
            // Fresh for 500 ms, like a real one.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<SimEstimate>(request) {
                Ok(sim) => {
                    let s = shared.lock().unwrap();
                    s.audio_in.publish(sim.snapshot(s.now_s()));
                    ok()
                }
                Err(e) => e,
            }
        }
        (Method::Post, "/api/test/native_audio") => {
            // `--test-hooks` only (e2e of the « Musique » panel, T-243): a
            // simulated native input: capture state, device list and/or one
            // analysis snapshot (fresh for 500 ms, like a real one).
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<SimNative>(request) {
                Ok(sim) => {
                    let s = shared.lock().unwrap();
                    match sim.apply(&s.audio_in, s.now_s()) {
                        Ok(()) => ok(),
                        Err(e) => text(400, &format!("{e:#}")),
                    }
                }
                Err(e) => e,
            }
        }
        (Method::Get, "/api/arm") => {
            let s = shared.lock().unwrap();
            json_response(json!(s.gate.status(&s.estop)))
        }
        (Method::Post, "/api/arm") => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or(""));
                let mut s = shared.lock().unwrap();
                // The page asking is alive, even if its first heartbeat is
                // still on its way.
                if let Some(id) = req.get("client_id").and_then(|v| v.as_str()) {
                    s.presence.touch(id, Instant::now());
                }
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
            json_response(json!({
                "settings": s.safety.get(),
                "defaults": crate::safety::SafetySettings::default(),
                "status": s.strobe,
                "dwell": s.dwell,
                "load_error": s.safety.load_error(),
                "limits": { "max_zones": crate::zones::MAX_ZONES, "max_vertices": crate::zones::MAX_VERTICES },
            }))
        }
        // Strobe limits never looser than the safe defaults (400). Anything
        // looser than the current settings (zone removed or moved, horizon
        // lowered, gain raised...) needs `"confirm_loosen": true`, which
        // only the operator's confirmation in the UI sends (409 otherwise,
        // with the French list of what would be loosened). Missing fields
        // are the defaults, so a partial body that drops zones is refused.
        (Method::Post, "/api/safety") => match body::<serde_json::Value>(request) {
            Ok(mut v) => {
                let confirm = v.get("confirm_loosen").and_then(|c| c.as_bool()).unwrap_or(false);
                if let Some(o) = v.as_object_mut() {
                    o.remove("confirm_loosen");
                }
                match serde_json::from_value::<crate::safety::SafetySettings>(v) {
                    Err(e) => text(400, &format!("invalid JSON: {e}")),
                    Ok(cfg) => {
                        let mut s = shared.lock().unwrap();
                        let before = s.safety.get();
                        match s.safety.set(cfg, confirm) {
                            Ok(stored) => {
                                // Before and after, and whether a loosening was confirmed (T-259).
                                if let Some(mut change) = crate::safety_log::settings_change(&before, &stored) {
                                    change["confirmed_loosen"] = json!(confirm);
                                    s.gate.log().record("safety_settings", Some("ui"), change);
                                }
                                // Public zones: the `audience_zone` interlock and the audience mode (T-255).
                                s.sync_audience();
                                json_response(json!({ "settings": stored }))
                            }
                            Err(crate::safety::SetError::Invalid(msg)) => text(400, &msg),
                            Err(crate::safety::SetError::Loosens(list)) => with_status(
                                json_response(json!({ "error": "Ces changements assouplissent la sécurité : confirmation requise", "loosen": list })),
                                409,
                            ),
                        }
                    }
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/audience") => {
            let mut s = shared.lock().unwrap();
            s.sync_audience();
            json_response(audience_view(&s))
        }
        // Audience scanning (T-255). `{"mode": "above"}` always applies (it
        // only tightens, even armed). `{"mode": "scanning", "phrase": ..}`
        // unlocks only disarmed (409 `{error, armed}`), with every
        // condition met (409 `{error, missing}`) and the exact phrase (409
        // `{error, needs_phrase}`), never in show mode (423). Logged; never
        // arms.
        (Method::Post, "/api/audience") => match body::<serde_json::Value>(request) {
            Ok(v) => {
                let source = ArmSource::parse(v.get("source").and_then(|s| s.as_str()).unwrap_or("ui"));
                let mut s = shared.lock().unwrap();
                match v.get("mode").and_then(|m| m.as_str()) {
                    Some("above") => {
                        s.end_audience_scanning("user", source, &[]);
                        json_response(audience_view(&s))
                    }
                    Some("scanning") => {
                        if s.show_mode.on {
                            return locked();
                        }
                        if !matches!(source, ArmSource::Ui | ArmSource::Keyboard | ArmSource::Api) {
                            return text(403, "le balayage public se déverrouille depuis l'interface seulement");
                        }
                        let phrase = v.get("phrase").and_then(|p| p.as_str()).unwrap_or("");
                        match s.unlock_audience_scanning(phrase, source) {
                            Ok(()) => json_response(audience_view(&s)),
                            Err(e) => {
                                let mut reply = audience_view(&s);
                                reply["error"] = json!(e.message_fr());
                                match &e {
                                    crate::audience::UnlockError::Armed => reply["armed"] = json!(true),
                                    crate::audience::UnlockError::Missing(m) => reply["missing"] = json!(m),
                                    crate::audience::UnlockError::Phrase => reply["needs_phrase"] = json!(true),
                                }
                                with_status(json_response(reply), 409)
                            }
                        }
                    }
                    _ => text(400, "expected \"mode\": \"above\" or \"scanning\""),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/outputs/limits") => {
            let s = shared.lock().unwrap();
            json_response(outputs_view(&s))
        }
        // Power caps and projector sheet of one output (T-254). Body:
        // `{id?, limits?, projector?, confirm_loosen?}` (a missing part is
        // kept). Lowering applies at once. Raising a cap is 409 `{error,
        // loosen}` without `"confirm_loosen": true` (sent only by the
        // operator's confirmation in the UI), and 409 `{error, loosen,
        // armed: true}` while the laser is armed, confirmed or not.
        (Method::Post, "/api/outputs/limits") => match body::<OutputsBody>(request) {
            Ok(b) => {
                let mut s = shared.lock().unwrap();
                let s = &mut *s;
                s.gate.sync_estop(&s.estop);
                let armed = s.gate.is_armed();
                let id = b.id.as_deref().unwrap_or(crate::power::MAIN_OUTPUT);
                let before = s.outputs.get(id).cloned();
                match s.outputs.set(id, b.limits, b.projector, armed, b.confirm_loosen) {
                    Ok(out) => {
                        // Power caps and projector sheet, before/after (T-259).
                        if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out)) {
                            change["output"] = json!(id);
                            change["confirmed_loosen"] = json!(b.confirm_loosen);
                            s.gate.log().record("power_caps", Some("ui"), change);
                        }
                        // The sheet may declare a ShowNET (T-472).
                        s.sync_checklist();
                        json_response(json!({ "output": out, "max_brightness_effective": s.outputs.max_brightness_effective() }))
                    }
                    Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                    Err(crate::power::SetError::Raises(list)) => with_status(
                        json_response(json!({ "error": "Relever un plafond de puissance : confirmation requise", "loosen": list })),
                        409,
                    ),
                    Err(crate::power::SetError::Armed(list)) => with_status(
                        json_response(json!({ "error": "Désarmez le laser pour relever un plafond de puissance", "loosen": list, "armed": true })),
                        409,
                    ),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/outputs/color") => {
            let s = shared.lock().unwrap();
            match color_output(request.url(), &s) {
                Ok(id) => json_response(color_view(&s, &id)),
                Err(e) => e,
            }
        }
        // Colour rendering of one output (T-414), `?output=<id or index>`
        // (default the main output). Body: `{color, confirm_loosen?}`; a
        // missing field of `color` is its identity value. Darker or equal
        // applies at once; brighter is 409 `{error, loosen}` without
        // `"confirm_loosen": true` (sent only by the operator's
        // confirmation in the UI), and 409 `{error, loosen, armed: true}`
        // while the laser is armed, confirmed or not (like raising a cap).
        // The caps still apply after it. Refused in show mode (not on the
        // allow-list).
        (Method::Post, "/api/outputs/color") => {
            let id = match color_output(request.url(), &shared.lock().unwrap()) {
                Ok(id) => id,
                Err(e) => return e,
            };
            match body::<ColorBody>(request) {
                Ok(b) => {
                    let mut s = shared.lock().unwrap();
                    let s = &mut *s;
                    s.gate.sync_estop(&s.estop);
                    let armed = s.gate.is_armed();
                    let before = s.outputs.get(&id).map(|o| o.color);
                    match s.outputs.set_color(&id, b.color, armed, b.confirm_loosen) {
                        Ok(out) => {
                            if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out.color)) {
                                change["output"] = json!(id);
                                change["confirmed_loosen"] = json!(b.confirm_loosen);
                                s.gate.log().record("output_color", Some("ui"), change);
                            }
                            json_response(color_view(s, &id))
                        }
                        Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                        Err(crate::power::SetError::Raises(list)) => with_status(
                            json_response(json!({ "error": "Ce réglage augmente la lumière émise : confirmation requise", "loosen": list })),
                            409,
                        ),
                        Err(crate::power::SetError::Armed(list)) => with_status(
                            json_response(json!({ "error": "Désarmez avant d'augmenter les couleurs", "loosen": list, "armed": true })),
                            409,
                        ),
                    }
                }
                Err(e) => e,
            }
        }
        (Method::Get, "/api/testpattern") => json_response(test_pattern_view(&shared.lock().unwrap())),
        // T-404: `{"pattern": "grid" | null, "brightness": 0.2}`, both
        // optional (a missing key keeps the current value). Shows or leaves
        // a test pattern; never arms, never touches the cue deck.
        (Method::Post, "/api/testpattern") => match body::<serde_json::Value>(request) {
            Ok(v) => {
                let pattern = match v.get("pattern") {
                    None => None,
                    Some(serde_json::Value::Null) => Some(None),
                    Some(serde_json::Value::String(name)) => match crate::testpattern::parse(name) {
                        Some(p) => Some(Some(p)),
                        None => return text(400, &format!("mire inconnue : {name}")),
                    },
                    Some(_) => return text(400, "pattern: nom de mire ou null"),
                };
                let brightness = match v.get("brightness") {
                    None => None,
                    Some(b) => match b.as_f64().filter(|b| b.is_finite()) {
                        Some(b) => Some((b as f32).clamp(0.0, 1.0)),
                        None => return text(400, "brightness: nombre entre 0 et 1"),
                    },
                };
                let mut s = shared.lock().unwrap();
                // Show mode (T-283): leaving a pattern only.
                if s.show_mode.on && (matches!(pattern, Some(Some(_))) || brightness.is_some()) {
                    return locked();
                }
                if let Some(b) = brightness {
                    s.test_pattern.brightness = b;
                }
                if let Some(p) = pattern {
                    if p != s.test_pattern.active {
                        let label = p.and_then(|p| crate::testpattern::LABELS_FR.iter().find(|(q, _)| *q == p)).map(|(_, l)| *l);
                        s.gate.log().record("test_pattern", Some("ui"), json!({ "pattern": p, "label": label }));
                    }
                    crate::testpattern::select(&mut s, p);
                }
                json_response(test_pattern_view(&s))
            }
            Err(e) => e,
        },
        (Method::Get, "/api/checklist") => json_response(shared.lock().unwrap().checklist.view()),
        (Method::Post, p) if p.starts_with("/api/checklist/") => checklist_route(request, shared, &p["/api/checklist/".len()..]),
        (Method::Get, "/api/safety/log") | (Method::Get, "/api/safety/log.csv") => safety_log_view(request.url(), shared, path.ends_with(".csv")),
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
        (Method::Get, "/api/projection") => json_response(projection_view(&shared.lock().unwrap())),
        (Method::Post, "/api/projection") => match body::<crate::projection::ProjectionConfig>(request) {
            Ok(cfg) => {
                let mut s = shared.lock().unwrap();
                match s.projection.set(cfg) {
                    Ok(()) => json_response(projection_view(&s)),
                    Err(e) => text(400, &e.to_string()),
                }
            }
            Err(e) => e,
        },
        // « Aligner » : the test pattern goes through this zone's geometry.
        (Method::Post, "/api/projection/align") => match body::<AlignRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                if req.zone.is_some_and(|id| s.projection.get().zone(id).is_none()) {
                    return text(404, "zone inconnue");
                }
                s.projection.align = req.zone;
                json_response(projection_view(&s))
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
                        log_scene(&s, &scene.name);
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
                    log_scene(&s, &first.name);
                    ok()
                }
                None => text(400, "no saved scenes"),
            }
        }
        (Method::Get, "/api/presets") => {
            let s = shared.lock().unwrap();
            // `beats`: an evolving cue's length, the timeline editor's default event length (T-162).
            let beats = |p: &crate::presets::Preset| match (&p.settings.program, &p.settings.content) {
                (Some(program), content) => Some(program.length(content) as f32),
                (None, crate::engine::Content::Evolving(e)) => Some(e.length_beats),
                _ => None,
            };
            // `program`: the cue has a program of its own (T-157; the grid's
            // programs are in `/api/cues`).
            let list: Vec<_> = s
                .presets
                .iter()
                .map(|p| {
                    let mut v = json!({ "id": p.id, "name": p.name, "category": p.category, "beats": beats(p), "program": p.settings.program.is_some() });
                    // A user cue (T-405): editable, with its cell colour.
                    if let Some(c) = s.user_cues.get(&p.id) {
                        v["user"] = json!(true);
                        v["color"] = json!(c.color);
                        v["master"] = json!(c.master.is_some());
                    }
                    v
                })
                .collect();
            json_response(json!({ "categories": CATEGORIES, "presets": list }))
        }
        (Method::Get, "/api/presets/thumbs") => {
            let presets: Vec<_> = shared.lock().unwrap().presets.iter().map(|p| (p.id.clone(), p.settings.clone())).collect();
            json_response(json!(cue_thumbnails(&presets)))
        }
        // Hover preview of a cue (T-157): its program at the current tempo.
        (Method::Get, "/api/presets/preview") => {
            let Some(id) = query_str(request.url(), "id") else { return text(400, "expected ?id=<cue>") };
            let (look, bpm, beats_per_bar) = {
                let s = shared.lock().unwrap();
                let Some(p) = s.presets.iter().find(|p| p.id == id) else { return text(404, "no such preset") };
                (s.deck.with_program(&id, p.settings.clone()), s.tempo.bpm, s.tempo.beats_per_bar)
            };
            json_response(json!(crate::presets::preview(&look, bpm, beats_per_bar)))
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
                if controls::press_cue_from(&mut shared.lock().unwrap(), &req.id, req.mode, req.down, Some("ui")) {
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
                // The program is set on its own route: the properties
                // dialog never sends it, and keeps it.
                let program = s.deck.slot(&req.id).program;
                s.deck.set_slot(&req.id, CueSlot { program, ..req.slot });
                s.deck.save();
                ok()
            }
            Err(e) => e,
        },
        // A cue program (T-157) on a cue of the grid: `null` removes it.
        // Takes effect at the cue's next start.
        (Method::Post, "/api/cues/program") => match body::<ProgramRequest>(request) {
            Ok(req) => {
                if let Some(Err(e)) = req.program.as_ref().map(crate::program::CueProgram::validate) {
                    return text(400, &e.to_string());
                }
                let mut s = shared.lock().unwrap();
                if !s.presets.iter().any(|p| p.id == req.id) {
                    return text(404, "no such preset");
                }
                let slot = s.deck.slot(&req.id);
                s.deck.set_slot(&req.id, CueSlot { program: req.program, ..slot });
                s.deck.save();
                ok()
            }
            Err(e) => e,
        },
        // The cue grid's size and pages (T-272). Every POST is an edit: locked in show mode.
        (Method::Get, "/api/grid") => json_response(grid_json(&shared.lock().unwrap())),
        (Method::Post, "/api/grid") => match body::<GridRequest>(request) {
            Ok(req) => grid_edit(shared, |g, _| {
                // An empty list = back to the default pages.
                if let Some(pages) = req.pages {
                    g.pages = pages;
                }
                g.set_size(req.cols.unwrap_or(g.cols), req.rows.unwrap_or(g.rows))?;
                g.sanitize();
                Ok(None)
            }),
            Err(e) => e,
        },
        (Method::Post, "/api/grid/page") => match body::<GridPageRequest>(request) {
            Ok(req) => grid_edit(shared, |g, presets| match req.action.as_str() {
                "new" => g.new_page(req.name.as_deref().unwrap_or(""), presets).map(Some),
                "rename" => g.rename_page(req.page.unwrap_or(usize::MAX), req.name.as_deref().unwrap_or(""), presets).map(|()| None),
                "delete" => g.delete_page(req.page.unwrap_or(usize::MAX), presets).map(|()| None),
                other => anyhow::bail!("action inconnue : « {other} »"),
            }),
            Err(e) => e,
        },
        // T-405: user pages, the cell menu and drag and drop on the grid.
        (Method::Post, "/api/grid/pages") => match body::<GridPagesRequest>(request) {
            Ok(req) => grid_edit(shared, |g, presets| match req.op.as_str() {
                "add" => g.new_page(req.name.as_deref().unwrap_or(""), presets).map(Some),
                "rename" => g.rename_page(req.page.unwrap_or(usize::MAX), req.name.as_deref().unwrap_or(""), presets).map(|()| None),
                "delete" => g.delete_page(req.page.unwrap_or(usize::MAX), presets).map(|()| None),
                other => anyhow::bail!("opération inconnue : « {other} »"),
            }),
            Err(e) => e,
        },
        (Method::Post, "/api/grid/edit") => match body::<crate::usercues::GridEdit>(request) {
            Ok(op) => {
                let mut s = shared.lock().unwrap();
                match crate::usercues::edit(&mut s, op) {
                    Ok(_) => json_response(grid_json(&s)),
                    Err((code, msg)) => text(code, &msg),
                }
            }
            Err(e) => e,
        },
        // « Enregistrer dans une case » / « Mettre à jour » (T-405): the
        // current look into a user cue. A built-in cue is refused (409).
        (Method::Post, "/api/cues/store") => match body::<StoreRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match crate::usercues::store(&mut s, req.page, req.slot, req.name.as_deref(), req.filter, req.with_master) {
                    Ok(id) => {
                        let mut g = grid_json(&s);
                        g["cue"] = json!(id);
                        json_response(g)
                    }
                    Err((code, msg)) => text(code, &msg),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/cues/update") => match body::<UpdateRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match crate::usercues::update(&mut s, req.id.as_deref(), req.filter, req.with_master) {
                    Ok(id) => {
                        let name = s.user_cues.get(&id).map(|c| c.name.clone()).unwrap_or_default();
                        json_response(json!({ "id": id, "name": name }))
                    }
                    Err((code, msg)) => text(code, &msg),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/grid/move") => match body::<GridMoveRequest>(request) {
            Ok(req) => grid_edit(shared, |g, presets| g.move_cell(req.from, req.to, presets).map(|()| None)),
            Err(e) => e,
        },
        (Method::Post, "/api/grid/clear") => match body::<GridCellRequest>(request) {
            Ok(req) => grid_edit(shared, |g, presets| g.clear_cell(req.page, req.slot, presets).map(|()| None)),
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
        (Method::Get, "/api/audio/routes") => {
            let s = shared.lock().unwrap();
            let targets: Vec<_> = s
                .controls
                .list()
                .iter()
                .filter_map(|d| {
                    let target = crate::audio::shape::Target::bind(&s.controls, &d.id)?;
                    let (min, max) = target.range();
                    Some(json!({ "id": d.id, "label": d.label_fr, "group": d.group, "min": min, "max": max }))
                })
                .collect();
            let sources: Vec<_> = crate::audio::routes::SOURCES
                .iter()
                .map(|(id, label)| {
                    let event = crate::audio::shape::Source::parse(id).is_some_and(|src| src.is_event());
                    json!({ "id": id, "label": label, "event": event })
                })
                .collect();
            let r = s.routes.routing();
            // T-239: the preset these routes are exactly (null: custom).
            let preset = crate::audio::presets::matching(r, &s.controls);
            // Per route: high frequencies on a brightness (it flickers).
            let flicker: Vec<bool> = r.routes.iter().map(crate::audio::presets::flickers).collect();
            json_response(json!({ "routes": r.routes, "mix": r.mix, "sources": sources, "targets": targets, "max": crate::audio::routes::MAX_ROUTES, "preset": preset, "flicker": flicker }))
        }
        (Method::Post, "/api/audio/routes") => match body::<crate::audio::routes::AudioRouting>(request) {
            Ok(routing) => {
                let s = &mut *shared.lock().unwrap();
                match s.routes.set(routing, &s.controls) {
                    Ok(()) => ok(),
                    Err(e) => text(400, &e.to_string()),
                }
            }
            Err(e) => e,
        },
        // Audio reactivity presets (T-239): sets of routes, applied in one click.
        (Method::Get, "/api/audio/presets") => {
            let s = shared.lock().unwrap();
            let list: Vec<_> = crate::audio::presets::builtin_presets()
                .into_iter()
                .map(|p| json!({ "id": p.id, "name": p.name_fr, "hint": p.hint_fr, "routes": p.routes }))
                .collect();
            json_response(json!({ "presets": list, "current": crate::audio::presets::matching(s.routes.routing(), &s.controls) }))
        }
        // `{"id": "punch"}`: replaces the routes (crossfader and everything
        // else kept). An edit: locked in show mode.
        (Method::Post, "/api/audio/preset") => match body::<serde_json::Value>(request) {
            Ok(v) => {
                let s = &mut *shared.lock().unwrap();
                let id = v.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                match crate::audio::presets::apply(id, s.routes.routing(), &s.controls) {
                    None => text(400, &format!("préréglage audio inconnu : « {id} »")),
                    Some((routing, warnings)) => match s.routes.set(routing, &s.controls) {
                        Ok(()) => {
                            for w in &warnings {
                                log::warn!("préréglage audio {id} : {w}");
                            }
                            json_response(json!({ "preset": id, "routes": s.routes.routing().routes, "warnings": warnings }))
                        }
                        Err(e) => text(400, &e.to_string()),
                    },
                }
            }
            Err(e) => e,
        },
        // Audio event triggers (T-240). The rules are an edit (locked in
        // show mode); the switch is the control `audio.triggers.enabled`.
        (Method::Get, "/api/audio/triggers") => json_response(crate::audio::triggers::view(&shared.lock().unwrap())),
        (Method::Post, "/api/audio/triggers") => match body::<crate::audio::triggers::TriggerConfig>(request) {
            Ok(cfg) => {
                let s = &mut *shared.lock().unwrap();
                match crate::audio::triggers::set_config(s, cfg) {
                    Ok(_) => json_response(crate::audio::triggers::view(s)),
                    Err(e) => text(400, &format!("{e:#}")),
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
        (Method::Get, "/api/shows") => json_response(json!(shared.lock().unwrap().shows.list())),
        (Method::Post, "/api/shows") => match body::<crate::timeline::Show>(request) {
            Ok(show) => match shared.lock().unwrap().shows.save(&show) {
                Ok(()) => ok(),
                Err(e) => text(400, &format!("{e:#}")),
            },
            Err(e) => e,
        },
        (Method::Get, "/api/figures") => {
            let s = shared.lock().unwrap();
            let list: Vec<_> = s
                .figures
                .list()
                .iter()
                .map(|f| json!({ "name": f.name, "id": crate::figures::cue_id(&f.name), "frames": f.frames.len(), "points": f.point_count() }))
                .collect();
            json_response(json!(list))
        }
        (Method::Post, "/api/figures") => match body::<crate::figures::Figure>(request) {
            Ok(fig) => {
                let mut s = shared.lock().unwrap();
                match s.figures.save(fig) {
                    Ok(kept) => {
                        crate::figures::refresh(&mut s);
                        json_response(json!({ "name": kept.name, "id": crate::figures::cue_id(&kept.name) }))
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/figures/load") => match body::<NameRequest>(request) {
            Ok(req) => match shared.lock().unwrap().figures.get(&req.name) {
                Some(fig) => json_response(json!(fig)),
                None => text(404, &format!("figure introuvable : {}", req.name.trim())),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/figures/delete") => match body::<NameRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match s.figures.remove(&req.name) {
                    Ok(()) => {
                        crate::figures::refresh(&mut s);
                        ok()
                    }
                    Err(e) => text(404, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        // Import (T-297): the raw file in the body, settings in the query.
        // Returns a preview; nothing is saved (« Créer la figure » does).
        (Method::Post, "/api/figures/import") => import_figure(request, shared),
        // The editor's text tool: the laser font as strokes. No state.
        (Method::Post, "/api/figures/text") => match body::<TextRequest>(request) {
            Ok(req) => {
                let text: String = req.text.to_uppercase().chars().take(64).collect();
                let size = if req.size.is_finite() { req.size.clamp(0.02, 2.0) } else { 0.3 };
                json_response(json!({ "strokes": crate::font::text_strokes(&text, size) }))
            }
            Err(e) => e,
        },
        (Method::Get, "/api/timeline") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "state": s.timeline.state(&s.timeline_clock()), "show": s.timeline.show, "audio": song_view(&s) }))
        }
        (Method::Get, "/api/media/audio") => {
            let media = Arc::clone(&shared.lock().unwrap().media);
            json_response(json!({ "files": media.list(), "extensions": crate::audio::media::EXTENSIONS }))
        }
        (Method::Post, "/api/media/audio") => import_song(request, shared),
        // ILDA media library (T-011): list, import (raw file in the body),
        // delete, and export of a look as format 5 for the ShowNET SD card.
        (Method::Get, "/api/media/ilda") => {
            let ilda = Arc::clone(&shared.lock().unwrap().ilda);
            json_response(json!({
                "files": ilda.list(),
                "loading": ilda.loading(),
                "max_bytes": crate::ilda::MAX_READ_BYTES,
                "default_fps": crate::ilda_library::DEFAULT_FPS,
                "max_export_frames": crate::ilda_library::MAX_EXPORT_FRAMES,
                "shownet_max_bytes": crate::ilda::SHOWNET_MAX_FILE_BYTES,
            }))
        }
        (Method::Post, "/api/media/ilda") => import_ilda(request, shared),
        (Method::Post, "/api/media/ilda/delete") => match body::<NameRequest>(request) {
            Ok(req) => {
                let ilda = Arc::clone(&shared.lock().unwrap().ilda);
                match ilda.remove(&req.name) {
                    Ok(()) => {
                        crate::ilda_library::refresh(&mut shared.lock().unwrap());
                        ok()
                    }
                    Err(e) => text(404, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/media/ilda/export") => export_ilda(request.url(), shared),
        // The ShowNET SD card bank (T-302): edited here, exported to a
        // folder beside the single worker (`may_decode`). Show mode locks
        // the POSTs (not in its allow-list).
        (Method::Get, "/api/sdbank") => json_response(sdbank_view(&shared.lock().unwrap())),
        (Method::Post, "/api/sdbank") => match body::<crate::sdbank::SdBank>(request) {
            Ok(bank) => {
                let mut s = shared.lock().unwrap();
                match s.sdbank.set(bank) {
                    Ok(()) => json_response(sdbank_view(&s)),
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(r) => r,
        },
        (Method::Post, "/api/sdbank/add") => match body::<SdAddRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let Some(id) = req.id.filter(|i| !i.is_empty()).or_else(|| s.active_cue.clone()) else {
                    return text(400, "Aucun cue en cours : lancez un cue d'abord");
                };
                if !s.presets.iter().any(|p| p.id == id) {
                    return text(404, "cue introuvable");
                }
                let mut bank = s.sdbank.get().clone();
                let added = bank.add(&id, req.seconds.unwrap_or(crate::sdbank::DEFAULT_SECONDS)).and_then(|n| s.sdbank.set(bank).map(|()| n));
                match added {
                    Ok(n) => {
                        let mut v = sdbank_view(&s);
                        v["added"] = json!(n);
                        json_response(v)
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(r) => r,
        },
        (Method::Post, "/api/sdbank/renumber") => {
            let mut s = shared.lock().unwrap();
            let mut bank = s.sdbank.get().clone();
            bank.renumber();
            match s.sdbank.set(bank) {
                Ok(()) => json_response(sdbank_view(&s)),
                Err(e) => text(400, &format!("{e:#}")),
            }
        }
        (Method::Post, "/api/sdbank/export") => export_sdbank(request, shared),
        (Method::Get, "/api/timeline/waveform") => waveform(request.url(), shared),
        (Method::Post, "/api/timeline/audio") => match body::<SongPatch>(request) {
            Ok(patch) => attach_song(patch, shared),
            Err(e) => e,
        },
        (Method::Post, p) if p.starts_with("/api/timeline/") => timeline_route(request, shared, &p["/api/timeline/".len()..]),
        (method, p) if p.starts_with("/api/midi") => midi_route(request, shared, method == Method::Post, p),
        (Method::Get, "/api/netin") => json_response(crate::netin::view(&shared.lock().unwrap(), Instant::now())),
        (Method::Post, "/api/netin") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let (result, hub) = {
                    let mut s = shared.lock().unwrap();
                    (crate::netin::update(&mut s, &patch), Arc::clone(&s.netin.hub))
                };
                match result {
                    Ok(changed) => {
                        if !changed.is_empty() {
                            hub.save();
                        }
                        json_response(crate::netin::view(&shared.lock().unwrap(), Instant::now()))
                    }
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (method, p) if p.starts_with("/api/external") || p.starts_with("/api/dmx/") => external_route(request, shared, method, p),
        (method, p) if p == "/api/project" || p.starts_with("/api/project/") => project_route(request, shared, method == Method::Post, p),
        _ => text(404, "not found"),
    }
}

/// `POST /api/media/audio?name=<file name>`, the file's bytes as the body:
/// decoded first (a damaged file is refused with a clear message and
/// nothing is written), then stored in `media/audio/` under a safe name.
/// The lock is not held while reading, decoding or writing.
/// Cue pictures already drawn, by cue id, with the look they were drawn
/// from: each is rendered once (a figure edited under the same name is
/// drawn again), so the request stays instant for every page.
static THUMBS: Mutex<Option<std::collections::HashMap<String, (String, String)>>> = Mutex::new(None);

fn cue_thumbnails(presets: &[(String, Settings)]) -> serde_json::Map<String, serde_json::Value> {
    let mut cache = THUMBS.lock().unwrap();
    let cache = cache.get_or_insert_with(Default::default);
    presets
        .iter()
        .map(|(id, settings)| {
            let mut key = serde_json::to_string(settings).unwrap_or_default();
            // An ILDA cue is drawn again once its file is (re)loaded.
            if let crate::engine::Content::Ilda { file, .. } = &settings.content {
                key += &crate::ilda_library::clip(file).map_or(0, |c| c.stamp).to_string();
            }
            let hex = match cache.get(id) {
                Some((k, hex)) if *k == key => hex.clone(),
                _ => {
                    let hex = crate::presets::thumbnail(settings);
                    cache.insert(id.clone(), (key, hex.clone()));
                    hex
                }
            };
            (id.clone(), json!(hex))
        })
        .collect()
}

fn import_song(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::audio::media::MAX_IMPORT_BYTES;
    use std::io::Read;
    let too_big = || text(413, &format!("fichier trop gros ({} Mo au plus)", MAX_IMPORT_BYTES >> 20));
    let Some(name) = query_str(request.url(), "name") else { return text(400, "expected ?name=<nom du fichier>") };
    if request.body_length().is_some_and(|n| n as u64 > MAX_IMPORT_BYTES) {
        return too_big();
    }
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_IMPORT_BYTES + 1).read_to_end(&mut bytes) {
        return text(400, &format!("unreadable body: {e}"));
    }
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return too_big();
    }
    let media = Arc::clone(&shared.lock().unwrap().media);
    match media.import(&name, &bytes) {
        Ok(imported) => json_response(json!(imported)),
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `POST /api/media/ilda?name=<file name>`, the file's bytes as the body:
/// parsed first (a damaged file is refused with a clear message and
/// nothing is written), then stored in `media/ilda/` under a safe name and
/// shown as a cue of the « ILDA » page. Runs beside the HTTP worker; the
/// lock is not held while reading, parsing or writing. Never arms.
fn import_ilda(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::ilda::MAX_READ_BYTES;
    use std::io::Read;
    let too_big = || text(413, &format!("fichier trop gros ({} Mo au plus)", MAX_READ_BYTES >> 20));
    let Some(name) = query_str(request.url(), "name") else { return text(400, "expected ?name=<nom du fichier>") };
    if request.body_length().is_some_and(|n| n as u64 > MAX_READ_BYTES) {
        return too_big();
    }
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_READ_BYTES + 1).read_to_end(&mut bytes) {
        return text(400, &format!("unreadable body: {e}"));
    }
    if bytes.len() as u64 > MAX_READ_BYTES {
        return too_big();
    }
    let ilda = Arc::clone(&shared.lock().unwrap().ilda);
    match ilda.import(&name, &bytes) {
        Ok(imported) => {
            crate::ilda_library::refresh(&mut shared.lock().unwrap());
            json_response(json!(imported))
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `GET /api/media/ilda/export?number=1..229&frames=&fps=[&id=<cue>][&output=<id>]`:
/// the look on show (or that cue) as an ILDA format 5 download named for
/// the ShowNET SD card (`001.ild`-`229.ild`), with the safety chain of
/// output `output` (default `main`) baked in (T-355). The ShowNET plays it
/// alone: the page says the hardware stop stays the real safety. A GET
/// that only builds a download, so show mode lets it through.
fn export_ilda(url: &str, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    // Missing: the default; present but not a number: refused (NaN).
    let num = |k: &str, default: f64| query_str(url, k).map_or(default, |v| v.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(f64::NAN));
    let number = num("number", 1.0);
    let name = match crate::ilda::shownet_sd_name(if number >= 0.0 && number.fract() == 0.0 { number as u32 } else { u32::MAX }) {
        Ok(n) => n,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    let frames = num("frames", 60.0);
    let fps = num("fps", 30.0) as f32;
    let output = query_str(url, "output").filter(|o| !o.is_empty()).unwrap_or_else(|| crate::power::MAIN_OUTPUT.to_string());
    let (settings, bpm, chain) = {
        let s = shared.lock().unwrap();
        let look = match query_str(url, "id").filter(|id| !id.is_empty()) {
            Some(id) => match s.presets.iter().find(|p| p.id == id) {
                Some(p) => p.settings.clone(),
                None => return text(404, "cue introuvable"),
            },
            None => s.settings.clone(),
        };
        let chain = match crate::ilda_library::ExportSafety::of(&s, &output) {
            Ok(c) => c,
            Err(e) => return text(400, &format!("{e:#}")),
        };
        (look, s.tempo.bpm, chain)
    };
    let frames = if frames >= 1.0 && frames.fract() == 0.0 { frames as usize } else { 0 };
    // Rendered and put through the safety chain with the lock released.
    match crate::ilda_library::export_look(&settings, frames, fps, bpm, &chain) {
        Ok(exported) => {
            let bytes = exported.bytes;
            log::info!(
                "Look exporté en ILDA format 5 : {name} ({frames} images, {} Ko, chaîne de sécurité de la sortie « {} »)",
                bytes.len().div_ceil(1024),
                chain.output
            );
            let disposition = format!("attachment; filename=\"{name}\"");
            let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
            with_type(Response::from_data(bytes), "application/octet-stream").with_header(header)
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct SdAddRequest {
    /// The cue to add; none: the cue playing now.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    seconds: Option<f32>,
}

#[derive(Deserialize)]
struct SdExportRequest {
    dir: String,
    #[serde(default)]
    output: Option<String>,
}

/// The bank with each cue's name (null: no such cue any more), and the
/// outputs whose safety chain an export can bake in.
fn sdbank_view(s: &Shared) -> serde_json::Value {
    use crate::sdbank as sd;
    let slots: Vec<_> = s
        .sdbank
        .get()
        .slots
        .iter()
        .map(|slot| {
            let name = s.presets.iter().find(|p| p.id == slot.cue_id).map(|p| p.name.clone());
            json!({ "number": slot.number, "cue_id": slot.cue_id, "seconds": slot.seconds, "name": name, "file": format!("{:03}.ild", slot.number) })
        })
        .collect();
    json!({
        "slots": slots,
        "fps": sd::SD_FPS,
        "min_seconds": sd::MIN_SECONDS,
        "max_seconds": sd::MAX_SECONDS,
        "default_seconds": sd::DEFAULT_SECONDS,
        "first": sd::FIRST,
        "last": sd::LAST,
        "outputs": s.outputs.list().iter().map(|o| json!({ "id": o.id, "name": o.projector.name, "max_power": o.limits.max_power })).collect::<Vec<_>>(),
        "output": crate::power::MAIN_OUTPUT,
    })
}

/// `POST /api/sdbank/export {dir, output?}`: renders every cue of the bank
/// with the safety chain of `output` (default `main`) and writes
/// `NNN.ild` + `banque.json` into `dir`. Runs beside the single worker;
/// the engine lock is only held to copy the cues out.
fn export_sdbank(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let req: SdExportRequest = match body(request) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let dir = match crate::sdbank::resolve_dir(&req.dir) {
        Ok(d) => d,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    let output = req.output.filter(|o| !o.is_empty()).unwrap_or_else(|| crate::power::MAIN_OUTPUT.to_string());
    let job = match crate::sdbank::prepare(&shared.lock().unwrap(), &output) {
        Ok(j) => j,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    match crate::sdbank::export(&job, &dir) {
        Ok(report) => {
            log::info!("Banque SD ShowNET exportée : {} fichiers dans {} (sortie « {} »)", report.files.len(), report.dir, report.output);
            json_response(json!(report))
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `GET /api/timeline/waveform?from=&to=&px=[&file=]`: the song's
/// waveform over show seconds `[from, to[` in `px` columns (min and max per
/// column, 3 decimals). Default: the loaded show's song over the whole show.
fn waveform(url: &str, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let (media, song, length) = {
        let s = shared.lock().unwrap();
        let show = s.timeline.show.as_ref();
        (Arc::clone(&s.media), show.and_then(|sh| sh.song()).cloned(), show.map_or(0.0, |sh| sh.end()))
    };
    let (file, offset_s) = match (query_str(url, "file"), song) {
        (Some(file), _) => (file, 0.0),
        (None, Some(a)) => (a.file, a.offset_s),
        (None, None) => return text(404, "aucun morceau dans le show chargé"),
    };
    let peaks = match media.peaks(&file) {
        Ok(p) => p,
        Err(e) => return text(404, &format!("{e:#}")),
    };
    let num = |k: &str| query_str(url, k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let from = num("from").unwrap_or(0.0);
    let to = num("to").unwrap_or_else(|| length.max(offset_s + peaks.duration_s()));
    let px = num("px").map_or(1000, |v| v.max(1.0) as usize).clamp(1, 8192);
    let (min, max) = peaks.view(offset_s, from, to, px);
    let round = |v: Vec<f32>| v.into_iter().map(|x| (x * 1000.0).round() / 1000.0).collect::<Vec<_>>();
    json_response(json!({
        "file": file,
        "duration_s": peaks.duration_s(),
        "offset_s": offset_s,
        "sample_rate": peaks.sample_rate,
        "block": peaks.block,
        "from": from,
        "to": to,
        "px": px,
        "min": round(min),
        "max": round(max),
    }))
}

/// `POST /api/timeline/audio`: the loaded show's song. `file` (a song of
/// the library, `null` = none), `offset_s` (±0.5 s), `gain` (0..1); fields
/// left out don't change. The show is saved at once when it has a name.
#[derive(Deserialize)]
struct SongPatch {
    #[serde(default, deserialize_with = "some_string")]
    file: Option<Option<String>>,
    offset_s: Option<f64>,
    gain: Option<f32>,
}

/// Tells an explicit `null` (Some(None)) from a missing field (None).
fn some_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

fn attach_song(patch: SongPatch, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let media = Arc::clone(&shared.lock().unwrap().media);
    // The song's length, read (maybe decoded) without the lock.
    let duration = match &patch.file {
        Some(Some(file)) => match media.peaks(file) {
            Ok(p) => Some(p.duration_s()),
            Err(e) => return text(400, &format!("{e:#}")),
        },
        _ => None,
    };
    let mut s = shared.lock().unwrap();
    let s = &mut *s;
    let Some(show) = s.timeline.show.as_mut() else { return text(409, "aucun show chargé") };
    if show.time_base != crate::timeline::TimeBase::Seconds {
        return text(400, "un morceau ne va qu'avec un show en secondes");
    }
    if patch.file.is_none() && show.song().is_none() && (patch.offset_s.is_some() || patch.gain.is_some()) {
        return text(400, "pas de morceau dans ce show");
    }
    match (&patch.file, duration) {
        (Some(None), _) => show.audio = None,
        (Some(Some(file)), Some(duration_s)) => {
            let keep = show.audio.take().unwrap_or_default();
            show.audio = Some(crate::timeline::AudioRef { file: file.clone(), duration_s, ..keep });
        }
        _ => {}
    }
    if let Some(audio) = show.audio.as_mut() {
        if let Some(v) = patch.offset_s {
            audio.offset_s = v;
        }
        if let Some(v) = patch.gain {
            audio.gain = v;
        }
    }
    show.sanitize();
    let saved = crate::timeline::valid_show_name(&show.name) && s.shows.save(show).is_ok();
    if saved {
        s.timeline.saved();
    } else {
        s.timeline.touch();
    }
    let c = s.timeline_clock();
    json_response(json!({ "state": s.timeline.state(&c), "audio": song_view(s), "saved": saved }))
}

/// The song player's status, and which clock the timeline follows.
fn song_view(s: &Shared) -> serde_json::Value {
    let mut v = json!(s.song.status());
    v["clock"] = json!(s.song_sync.clock);
    v
}

/// `POST /api/timeline/{load,play,pause,stop,seek,loop}` (transport) and
/// `{new,edit,save}` (the editor, T-162). Nothing here can arm the laser.
fn timeline_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, action: &str) -> HttpResponse {
    let req = match action {
        "load" | "seek" | "loop" | "new" | "edit" | "save" => match body::<TimelineRequest>(request) {
            Ok(req) => req,
            Err(e) => return e,
        },
        _ => TimelineRequest::default(),
    };
    let mut s = shared.lock().unwrap();
    let c = s.timeline_clock();
    match action {
        "load" => {
            let show = match (req.show, req.name) {
                (Some(show), _) => show,
                (None, Some(name)) => match s.shows.load(&name) {
                    Ok(show) => show,
                    Err(e) => return text(404, &format!("{e:#}")),
                },
                (None, None) => return text(400, "expected \"name\" or \"show\""),
            };
            s.timeline.load(show);
        }
        "play" => {
            if let Err(why) = controls::timeline_play(&mut s) {
                return text(409, why);
            }
        }
        "pause" => s.timeline.pause(&c),
        "stop" => s.timeline.stop(),
        "seek" => match req.position {
            Some(p) => s.timeline.seek(p, &c),
            None => return text(400, "expected \"position\""),
        },
        "loop" => {
            if let Some(on) = req.on {
                s.timeline.loop_on = on;
            }
            if let (Some(region), Some(show)) = (req.region, s.timeline.show.as_mut()) {
                show.loop_region = region;
                show.sanitize();
                s.timeline.touch();
            }
        }
        "new" => {
            let name = req.name.unwrap_or_default().trim().to_string();
            if !crate::timeline::valid_show_name(&name) {
                return text(400, "nom de show invalide (lettres, chiffres, espaces, - et _ seulement)");
            }
            if s.shows.list().iter().any(|i| i.name == name) {
                return text(409, &format!("un show s'appelle déjà « {name} »"));
            }
            let show = crate::timeline::Show::new_empty(&name, req.time_base.unwrap_or_default());
            if let Err(e) = s.shows.save(&show) {
                return text(500, &format!("{e:#}"));
            }
            s.timeline.load(show);
        }
        "edit" => {
            let Some(show) = req.show else { return text(400, "expected \"show\"") };
            let Some(cur) = s.timeline.show.as_ref() else { return text(409, "aucun show chargé") };
            // A cue must exist, or already be in the show (a figure deleted
            // since keeps its events until the operator removes them).
            let before = cur.cue_ids();
            if let Err(e) = show.validate_edit(|id| before.contains(id) || s.presets.iter().any(|p| p.id == id)) {
                return text(400, &format!("{e:#}"));
            }
            s.timeline.edit(show);
            return json_response(json!({ "state": s.timeline.state(&c), "show": s.timeline.show }));
        }
        "save" => {
            let st = &mut *s;
            let Some(show) = st.timeline.show.as_mut() else { return text(409, "aucun show chargé") };
            if let Some(name) = req.name.map(|n| n.trim().to_string()).filter(|n| *n != show.name) {
                if !crate::timeline::valid_show_name(&name) {
                    return text(400, "nom de show invalide (lettres, chiffres, espaces, - et _ seulement)");
                }
                show.name = name;
            }
            if let Err(e) = st.shows.save(show) {
                return text(400, &format!("{e:#}"));
            }
            st.timeline.saved();
        }
        _ => return text(404, "not found"),
    }
    json_response(json!(s.timeline.state(&c)))
}

#[derive(Deserialize, Default)]
struct TimelineRequest {
    name: Option<String>,
    /// `new`: *Secondes* (default) or *Temps*.
    time_base: Option<crate::timeline::TimeBase>,
    show: Option<crate::timeline::Show>,
    position: Option<f64>,
    on: Option<bool>,
    /// `null` clears the region; absent leaves it.
    #[serde(default, deserialize_with = "some_value")]
    region: Option<Option<(f64, f64)>>,
}

/// Tells an explicit `null` (Some(None)) from a missing field (None).
fn some_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<(f64, f64)>>, D::Error> {
    Option::<(f64, f64)>::deserialize(d).map(Some)
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

/// `GET /api/project`, `POST /api/project/{new,open,save,save-as}` (T-286).
/// Nothing here can arm, or change calibration or safety settings.
/// `GET /api/external`: the gate, each source's scope, access and
/// refusals, plus the DMX summary (the top bar polls it).
fn external_view(s: &mut Shared) -> serde_json::Value {
    let now = Instant::now();
    let mut v = crate::external::view(s, now);
    v["dmx"] = crate::netin::summary(s, now);
    v
}

/// The external inputs gate (T-408) and the DMX input (T-406):
/// - `GET /api/external`; `POST /api/external/gate` `{ "on": bool }`, only
///   from this Mac's own page (loopback, and an on-screen source, as for
///   arming); `POST /api/external/scopes` `{ "scopes": [...] }`.
/// - `GET /api/dmx/interfaces`, `GET|POST /api/dmx/config` (the DMX part
///   of `netin.json`), `GET /api/dmx/monitor?universe=N`,
///   `POST /api/dmx/learn` `{ "control": id | null }`,
///   `GET /api/dmx/personality?mode=compact|full&format=json|md|xml|gdtf`.
fn external_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, method: Method, path: &str) -> HttpResponse {
    match (method, path) {
        (Method::Get, "/api/external") => json_response(external_view(&mut shared.lock().unwrap())),
        (Method::Post, "/api/external/gate") => {
            let local = request.remote_addr().is_some_and(|a| a.ip().is_loopback());
            let req = match body::<serde_json::Value>(request) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or("ui"));
            if !local || !matches!(source, ArmSource::Ui | ArmSource::Keyboard | ArmSource::Api) {
                return text(403, "les entrées externes ne s'ouvrent que depuis l'interface de ce Mac");
            }
            let mut s = shared.lock().unwrap();
            let want = match (req.get("on").and_then(|v| v.as_bool()), req.get("toggle").and_then(|v| v.as_bool())) {
                (Some(on), _) => on,
                (None, Some(true)) => !s.external.gate.open,
                _ => return text(400, "expected \"on\" or \"toggle\""),
            };
            match crate::external::set_open(&mut s, want, source) {
                Ok(()) => json_response(external_view(&mut s)),
                Err(why) => with_status(json_response(json!({ "open": false, "error": why })), 409),
            }
        }
        (Method::Post, "/api/external/scopes") => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let (result, job) = {
                    let mut s = shared.lock().unwrap();
                    let r = crate::external::update_scopes(&mut s, &req);
                    (r, s.external.save_job())
                };
                match result {
                    Ok(changed) => {
                        if let (false, Some((path, cfg))) = (changed.is_empty(), job) {
                            crate::save_json(&path, &cfg);
                        }
                        json_response(external_view(&mut shared.lock().unwrap()))
                    }
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/dmx/interfaces") => json_response(json!(crate::netin::interfaces())),
        (Method::Get, "/api/dmx/config") => {
            let cfg = shared.lock().unwrap().netin.hub.file();
            json_response(json!({ "artnet": cfg.artnet, "sacn": cfg.sacn, "dmx": cfg.dmx, "loss": cfg.loss }))
        }
        (Method::Post, "/api/dmx/config") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                if patch.as_object().is_some_and(|o| o.contains_key("osc")) {
                    return text(400, "réglage inconnu : osc");
                }
                let (result, hub) = {
                    let mut s = shared.lock().unwrap();
                    (crate::netin::update(&mut s, &patch), Arc::clone(&s.netin.hub))
                };
                match result {
                    Ok(changed) => {
                        if !changed.is_empty() {
                            hub.save();
                        }
                        let cfg = hub.file();
                        json_response(json!({ "artnet": cfg.artnet, "sacn": cfg.sacn, "dmx": cfg.dmx, "loss": cfg.loss, "changed": changed }))
                    }
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/dmx/personality") => personality(request.url()),
        (Method::Get, "/api/dmx/monitor") => {
            let universe = query_u64(request.url(), "universe").unwrap_or(1).min(u16::MAX as u64) as u16;
            json_response(crate::netin::monitor(&shared.lock().unwrap(), universe, Instant::now()))
        }
        (Method::Post, "/api/dmx/learn") => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let control = req.get("control").and_then(|v| v.as_str());
                let mut s = shared.lock().unwrap();
                match crate::netin::patch::start_learn(&mut s, control) {
                    Ok(()) => json_response(json!({ "learn": s.netin.patch.learn.as_ref().map(|l| l.control.clone()) })),
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        _ => text(404, "not found"),
    }
}

/// Our DMX personalities (T-407), from the tables the receiver uses:
/// `json` (default; both modes without `mode`), `md` (the chart; the whole
/// published document without `mode`), `xml` (the GDTF description) or
/// `gdtf` (the fixture file to import on a console, both modes).
fn personality(url: &str) -> HttpResponse {
    use crate::dmx::{gdtf, personality::{self, Personality}};
    let mode = match query_str(url, "mode").filter(|m| !m.is_empty()) {
        Some(m) => match Personality::parse(&m) {
            Some(p) => Some(p),
            None => return text(400, "personnalité inconnue (compact ou full)"),
        },
        None => None,
    };
    match query_str(url, "format").as_deref().unwrap_or("json") {
        "json" => json_response(match mode {
            Some(m) => personality::json(m),
            None => json!({ "version": personality::VERSION, "modes": Personality::ALL.iter().map(|m| personality::json(*m)).collect::<Vec<_>>() }),
        }),
        "md" => with_type(Response::from_string(personality::markdown(mode)), "text/markdown; charset=utf-8"),
        "xml" => with_type(Response::from_string(gdtf::description_xml()), "application/xml; charset=utf-8"),
        "gdtf" => {
            let disposition = format!("attachment; filename=\"{}\"", gdtf::FILE_NAME);
            let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
            with_type(Response::from_data(gdtf::gdtf()), "application/octet-stream").with_header(header)
        }
        _ => text(400, "format inconnu (json, md, xml ou gdtf)"),
    }
}

fn project_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, post: bool, path: &str) -> HttpResponse {
    let mut raw = String::new();
    if post {
        if let Err(e) = request.as_reader().read_to_string(&mut raw) {
            return text(400, &format!("unreadable body: {e}"));
        }
    }
    let action = path.strip_prefix("/api/project").unwrap_or("").trim_start_matches('/');
    match crate::project::route(shared, post, action, &raw) {
        Some(crate::project::Reply::Json(v)) => json_response(v),
        Some(crate::project::Reply::Text(code, t)) => text(code, &t),
        None => text(404, "not found"),
    }
}

/// `POST /api/heartbeat` from a UI page, every 500 ms and on every change
/// of visibility or of the hold key; `gone` when the page closes.
#[derive(Deserialize)]
struct Heartbeat {
    client_id: String,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    hold: bool,
    #[serde(default)]
    gone: bool,
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
struct AlignRequest {
    zone: Option<u8>,
}

/// `GET /api/projection`.
fn projection_view(s: &Shared) -> serde_json::Value {
    json!({ "config": s.projection.get(), "align": s.projection.align, "max_zones": crate::projection::MAX_ZONES })
}

/// `POST /api/grid`: a new size, and/or every page at once (`[]` = the defaults).
#[derive(Deserialize)]
struct GridRequest {
    cols: Option<u8>,
    rows: Option<u8>,
    pages: Option<Vec<crate::grid::CuePage>>,
}

/// `POST /api/grid/page`: « Nouvelle page », « Renommer la page », delete.
#[derive(Deserialize)]
struct GridPageRequest {
    action: String,
    page: Option<usize>,
    name: Option<String>,
}

/// `POST /api/grid/pages` (T-405): `add`, `rename`, `delete`.
#[derive(Deserialize)]
struct GridPagesRequest {
    op: String,
    page: Option<usize>,
    name: Option<String>,
}

/// `POST /api/cues/store`.
#[derive(Deserialize)]
struct StoreRequest {
    page: usize,
    slot: usize,
    name: Option<String>,
    #[serde(default)]
    filter: crate::usercues::StoreFilter,
    #[serde(default)]
    with_master: bool,
}

/// `POST /api/cues/update`: `id` absent = the newest playing user cue.
#[derive(Deserialize)]
struct UpdateRequest {
    id: Option<String>,
    #[serde(default)]
    filter: crate::usercues::StoreFilter,
    #[serde(default)]
    with_master: bool,
}

/// `POST /api/grid/move`: (page, cell) → (page, cell or the first empty one).
#[derive(Deserialize)]
struct GridMoveRequest {
    from: (usize, usize),
    to: (usize, Option<usize>),
}

#[derive(Deserialize)]
struct GridCellRequest {
    page: usize,
    slot: usize,
}

/// The grid as the page draws it: size, pages as shown, current page.
fn grid_json(s: &Shared) -> serde_json::Value {
    let g = &s.grid;
    json!({
        "cols": g.cols,
        "rows": g.rows,
        "default": g.is_default(),
        "rev": g.rev,
        "page": s.cue_page,
        "pages": g.view(),
        "limits": { "cols": [crate::grid::MIN_COLS, crate::grid::MAX_COLS], "rows": [crate::grid::MIN_ROWS, crate::grid::MAX_ROWS] },
    })
}

/// Runs a grid edit; on success, resolves the grid again, rebuilds the
/// grid controls, saves `pages.json` and answers with the new grid. `Some(n)`
/// from the edit shows page n.
fn grid_edit(shared: &Mutex<Shared>, edit: impl FnOnce(&mut crate::grid::CueGrid, &[crate::presets::Preset]) -> anyhow::Result<Option<usize>>) -> HttpResponse {
    let mut guard = shared.lock().unwrap();
    let s = &mut *guard;
    let mut grid = s.grid.clone();
    match edit(&mut grid, &s.presets) {
        Ok(show) => {
            s.grid = grid;
            if let Some(n) = show {
                s.cue_page = n;
            }
            crate::grid::refresh(s);
            s.grid.save();
            json_response(grid_json(s))
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct SlotRequest {
    id: String,
    #[serde(flatten)]
    slot: CueSlot,
}

#[derive(Deserialize)]
struct ProgramRequest {
    id: String,
    program: Option<crate::program::CueProgram>,
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
struct TextRequest {
    text: String,
    #[serde(default = "default_text_size")]
    size: f32,
}

fn default_text_size() -> f32 {
    0.3
}

/// `POST /api/outputs/limits`.
#[derive(Deserialize)]
struct OutputsBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    limits: Option<crate::power::OutputLimits>,
    #[serde(default)]
    projector: Option<crate::power::ProjectorInfo>,
    #[serde(default, alias = "confirm_raise")]
    confirm_loosen: bool,
}

/// `POST /api/outputs/color` (T-414).
#[derive(Deserialize)]
struct ColorBody {
    color: crate::colorcal::ColorCalibration,
    #[serde(default)]
    confirm_loosen: bool,
}

/// The output named by `?output=`: an id, or an index in the list.
/// Absent: the main output. Unknown: 400.
fn color_output(url: &str, s: &Shared) -> Result<String, HttpResponse> {
    let Some(q) = query_str(url, "output") else { return Ok(crate::power::MAIN_OUTPUT.into()) };
    let list = s.outputs.list();
    let found = match q.parse::<usize>() {
        Ok(i) => list.get(i),
        Err(_) => list.iter().find(|o| o.id == q),
    };
    found.map(|o| o.id.clone()).ok_or_else(|| text(400, &format!("Sortie inconnue : « {q} »")))
}

/// `GET /api/outputs/color`: the colour rendering of one output, and the
/// colour delay `laser-dac` already applies (ours adds to it).
fn color_view(s: &Shared, id: &str) -> serde_json::Value {
    let color = s.outputs.get(id).map(|o| o.color).unwrap_or_default();
    json!({
        "output": id,
        "color": color,
        "identity": color.is_identity(),
        "defaults": crate::colorcal::ColorCalibration::default(),
        "max_delay": crate::colorcal::MAX_DELAY,
        "gamma_range": crate::colorcal::GAMMA_RANGE,
        "library_delay_points": crate::output::library_color_delay_points(s.pps),
    })
}

/// `GET /api/outputs/artnet` (T-301): the ShowNET SD card box settings.
fn artnet_view(s: &Shared) -> serde_json::Value {
    use crate::artnet::{Profile, PORT};
    let profiles: Vec<_> = [Profile::Dj, Profile::Pro].iter().map(|p| json!({ "id": p, "label": p.label_fr(), "channels": p.channels() })).collect();
    let cfg = s.shownet_dmx.get();
    json!({ "config": cfg, "id": cfg.id(), "port": PORT, "profiles": profiles })
}

/// `POST /api/outputs/artnet` (T-301): the box's address (empty: off),
/// universe, DMX address, profile and zone. Refused while that output is
/// selected (disconnect first: the values are fixed when it opens).
/// Never connects, never arms.
fn set_artnet(cfg: crate::artnet::ShowNetDmxConfig, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let s = shared.lock().unwrap();
    if s.output_mgr.selected().is_some_and(|id| id.starts_with(crate::artnet::ID_PREFIX)) {
        return text(409, "Déconnectez la sortie ShowNET carte SD avant de changer ses réglages");
    }
    match s.shownet_dmx.set(cfg) {
        Ok(cfg) => {
            log::info!("Sortie ShowNET carte SD (Art-Net) : {}", cfg.target.as_deref().unwrap_or("désactivée"));
            json_response(artnet_view(&s))
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `POST /api/outputs/config` (T-401).
#[derive(Deserialize)]
struct OutputConfigBody {
    #[serde(default)]
    auto_reconnect: Option<bool>,
}

/// `POST /api/outputs/select` `{ "id": "<id>" }` connects, `{ "id": null }`
/// goes back to preview only. Whatever the choice, the gate is disarmed
/// first (« Changement de sortie ») and arming stays refused until the new
/// output is connected; the open itself runs on the output manager's
/// thread, never here. The choice is remembered in `outputs.json`.
fn select_output(req: &serde_json::Value, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let id = match req.get("id") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(id)) if !id.is_empty() => Some(id.clone()),
        _ => return text(400, "expected \"id\": a device id or null"),
    };
    let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or("ui"));
    let mut s = shared.lock().unwrap();
    let mgr = Arc::clone(&s.output_mgr);
    if let Some(id) = &id {
        if !mgr.knows(id) {
            return text(400, &format!("Sortie inconnue : « {id} » (lancez une recherche)"));
        }
    }
    s.gate.disarm(DisarmReason::OutputChanged, source);
    s.gate.set_output_ready(false);
    s.gate.log().record("output_select", Some(source.id()), json!({ "id": id }));
    mgr.select(id, source);
    if let Err(e) = s.outputs.set_connection(mgr.prefs()) {
        log::warn!("{e:#}");
    }
    json_response(mgr.view(Instant::now()))
}

/// `GET /api/outputs/limits`: every output's caps and projector sheet.
fn outputs_view(s: &Shared) -> serde_json::Value {
    json!({
        "active": crate::power::MAIN_OUTPUT,
        "outputs": s.outputs.list(),
        "defaults": { "limits": crate::power::OutputLimits::default(), "projector": crate::power::ProjectorInfo::default() },
        "classes": crate::power::LASER_CLASSES,
        "models": crate::power::MODELS,
        "max_brightness_effective": s.outputs.max_brightness_effective(),
        "load_error": s.outputs.load_error(),
    })
}

#[derive(Deserialize)]
struct LockRequest {
    show_mode: bool,
    #[serde(default)]
    source: Option<String>,
}

/// `GET /api/lock`, also `lock` in `/api/state` and `/api/frame`.
fn lock_view(s: &Shared) -> serde_json::Value {
    json!({ "show_mode": s.show_mode.on })
}

/// `GET /api/audience` (T-255): the mode, every unlock condition, the
/// phrase to type and whether the `audience_zone` interlock is satisfied.
fn audience_view(s: &Shared) -> serde_json::Value {
    let mut v = s.audience.view();
    v["conditions"] = json!(s.audience_conditions());
    v["phrase"] = json!(crate::audience::PHRASE);
    v["protected"] = json!(crate::audience::protected(&s.safety.get()));
    v["enforced"] = json!(s.checklist.enforced());
    v["armed"] = json!(s.gate.is_armed());
    v
}

/// 423 Locked: an edit refused in show mode (T-283).
fn locked() -> HttpResponse {
    text(423, crate::showmode::LOCKED_FR)
}

#[derive(Deserialize)]
struct KeepAwakeRequest {
    enabled: bool,
}

#[derive(Deserialize)]
struct SaveScene {
    name: String,
    duration_secs: f32,
}

/// `POST /api/data/warnings/ack`.
#[derive(Deserialize)]
struct DataAck {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    all: bool,
}

/// Damaged data files (T-402), with their French banner text.
fn data_warnings_view(s: &Shared) -> serde_json::Value {
    let list: Vec<serde_json::Value> = s
        .data_warnings
        .iter()
        .map(|w| {
            let mut v = json!(w);
            v["message"] = json!(w.message_fr());
            v
        })
        .collect();
    json!({ "warnings": list, "arm_blocked": !s.gate.interlock_ok(crate::interlock::DATA_ACK) })
}

fn state(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    json_response(state_json(&mut s))
}

/// `GET /api/state`'s body (also `state.json` in the diagnostic report).
fn state_json(s: &mut Shared) -> serde_json::Value {
    s.sync_data_warnings();
    s.gate.sync_estop(&s.estop);
    s.sync_audience();
    let arm = s.gate.status(&s.estop);
    json!({
        "settings": s.settings,
        "settings_rev": s.settings_rev,
        "calibration": s.calibration,
        "projection": s.projection.get(),
        "safety": s.safety.get(),
        "output_limits": s.outputs.active_limits(),
        "max_brightness_effective": s.outputs.max_brightness_effective(),
        "armed": arm.armed,
        "estop": arm.estop.is_some(),
        "arm": arm,
        "engine_ok": s.health.engine_ok(),
        "presence": s.presence.status(),
        "keep_awake": s.keep_awake,
        "lock": lock_view(s),
        "output": s.output_name,
        "output_kind": s.output_kind,
        "output_state": s.output_mgr.state(),
        "output_status": s.output_status,
        "pps": s.pps,
        "shapes": SHAPE_NAMES,
        "generators": GENERATOR_NAMES,
        "scenes": s.scenes.list(),
        "tempo": s.tempo.state(s.now_s()),
        "playlist": s.playlist.as_ref().map(|p| p.index),
        "evolving": evolving_status(s),
        "timeline": s.timeline.state(&s.timeline_clock()),
        "timeline_audio": song_view(s),
        "audio": s.audio_in.view(s.audio, s.audio_at, Instant::now()),
        "data_warnings": data_warnings_view(s),
        "checklist": { "enforced": s.checklist.enforced(), "complete": s.checklist.is_complete() },
        "audience": s.audience.view(),
        "shownet": shownet_view(s),
        "applog": s.applog.status(),
        "external": { "open": s.external.gate.open },
        "dmx": crate::netin::summary(s, Instant::now()),
    })
}

/// `GET /api/diagnostics/report[?project=1]` (T-403): a zip to send to
/// whoever helps after an incident. A snapshot is taken under the lock,
/// then the logs are read and the zip is built with the lock released.
/// The project (without its figures) only with `project=1`.
fn diagnostic_report(url: &str, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let with_project = query_str(url, "project").is_some_and(|v| v == "1" || v == "true");
    let (app, state, devices, mut settings, data_dir) = {
        let mut s = shared.lock().unwrap();
        let s = &mut *s;
        let state = state_json(s);
        let devices = json!({
            "audio": {
                "capture": s.audio_in.capture_enabled(),
                "inputs": s.audio_in.devices(),
                "config": s.audio_in.config().0,
                "status": state["audio"].clone(),
            },
            "midi": { "enabled": s.midi.enabled, "devices": s.midi.devices, "test": s.midi.sim.is_some() },
            "output": {
                "name": s.output_name,
                "kind": s.output_kind,
                "error": s.output_error,
                "pps": s.pps,
                "link": s.output_mgr.view(Instant::now()),
                "armed": s.gate.is_armed(),
                "engine_ok": s.health.engine_ok(),
            },
        });
        let settings = json!({
            "safety": s.safety.get(),
            "presence": s.presence.settings,
            "calibration": s.calibration,
            "projection": s.projection.get(),
            "outputs": s.outputs.list(),
            "midi_safety": s.midi.store.devices.safety,
        });
        (Arc::clone(&s.applog), state, devices, settings, s.project.data_dir.clone())
    };
    settings["prefs"] = crate::load_json::<serde_json::Value>(&data_dir.join("prefs.json"));
    let project = with_project.then(|| crate::project::diagnostic_snapshot(shared));
    let zip = crate::applog::report(&app, crate::applog::ReportParts { devices, settings, state, project });
    let date = crate::safety_log::today();
    log::info!("Rapport de diagnostic exporté ({} Ko, projet {})", zip.len() / 1024, if with_project { "inclus" } else { "non inclus" });
    let disposition = format!("attachment; filename=\"laser-studio-diagnostic-{date}.zip\"");
    let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
    with_type(Response::from_data(zip), "application/zip").with_header(header)
}

/// The current frame for the preview, as `[x, y, r, g, b]` rows rounded
/// to 3 decimals to keep the payload small at 30 requests a second.
fn frame(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    let s = &mut *s;
    s.gate.sync_estop(&s.estop);
    let arm = s.gate.status(&s.estop);
    let now = s.now_s();
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let points: Vec<[f32; 5]> = s.frame.iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect();
    json_response(json!({
        "points": points,
        "output_lit": s.output_lit,
        "armed": arm.armed,
        "estop": arm.estop.is_some(),
        "arm": arm,
        "engine_ok": s.health.engine_ok(),
        "presence": s.presence.status(),
        "keep_awake": s.keep_awake,
        "lock": lock_view(s),
        "output": s.output_name,
        "output_kind": s.output_kind,
        "output_error": s.output_error,
        "output_state": s.output_mgr.state(),
        "output_status": s.output_status,
        "pps": s.pps,
        "playlist": s.playlist.as_ref().map(|p| p.index),
        "cue_page": s.cue_page,
        "grid_rev": s.grid.rev,
        "active_cue": s.active_cue,
        "settings_rev": s.settings_rev,
        "cues": {
            "active": s.deck.active.iter().map(|a| json!({ "cue": a.cue, "held": a.held, "layer": a.layer, "phase": s.deck.phase(a.id, now) })).collect::<Vec<_>>(),
            "shown": s.deck.visible().iter().map(|a| a.cue.as_str()).collect::<Vec<_>>(),
            // T-158: stopped cues still fading out (*Fin*).
            "ending": s.deck.ending.iter().map(|e| json!({ "cue": e.cue.cue, "layer": e.cue.layer, "phase": s.deck.phase(e.cue.id, now) })).collect::<Vec<_>>(),
            // T-159: cues waiting for their quantised start (« en attente »).
            "pending": s.deck.pending.iter().map(|p| json!({ "cue": p.cue, "at_beat": p.at_beat })).collect::<Vec<_>>(),
            "click_mode": s.deck.click_mode,
            "multi": s.deck.multi,
            "max_active": s.deck.max_active,
            "transition": s.deck.transition,
            "quantize": s.deck.quantize,
            "beat_every": s.deck.beat_every,
        },
        "layers": { "mixer": s.mixer, "mix": s.mix },
        "tempo": s.tempo.state(s.now_s()),
        "timeline": s.timeline.state(&s.timeline_clock()),
        "timeline_audio": song_view(s),
        "live": s.live,
        "lfos": lfo_positions(s),
        "audio_routes": s.routes.meters().collect::<Vec<_>>(),
        "strobe": s.strobe,
        "audio_guard": s.audio_guard,
        "audio_triggers": crate::audio::triggers::status(s),
        "dwell": s.dwell,
        "evolving": evolving_status(s),
        "test_pattern": s.test_pattern,
        "checklist": { "enforced": s.checklist.enforced(), "complete": s.checklist.is_complete() },
        "audience": s.audience.view(),
        "shownet": shownet_view(s),
    }))
}

/// The ShowNET reminders (T-472), in `/api/state` and every frame: whether
/// the output is a ShowNET path, whether its hardware stop is ticked, and
/// the Wi-Fi warning (null when the link is not known to be Wi-Fi).
fn shownet_view(s: &Shared) -> serde_json::Value {
    json!({
        "active": s.checklist.output().is_some(),
        "hw_stop_ok": s.checklist.hw_stop_ok(),
        "wifi": s.wifi.warning(),
    })
}

/// `GET /api/testpattern`: the patterns (id and French name) and the mode.
fn test_pattern_view(s: &Shared) -> serde_json::Value {
    let list: Vec<_> = crate::testpattern::LABELS_FR.iter().map(|(p, label)| json!({ "id": p, "label": label })).collect();
    json!({
        "patterns": list,
        "state": s.test_pattern,
        "default_brightness": crate::testpattern::DEFAULT_BRIGHTNESS,
        "max_brightness_effective": s.outputs.max_brightness_effective(),
    })
}

/// Evolving cues on show (last frame), oldest first: the cue (`null` for
/// the manual look), its layer and where it is in its keys.
fn evolving_status(s: &Shared) -> Vec<serde_json::Value> {
    s.evolving
        .iter()
        .map(|(id, progress)| {
            let cue = s.deck.find(*id);
            let mut v = json!(progress);
            v["cue"] = json!(cue.map(|a| a.cue.as_str()));
            v["layer"] = json!(cue.map_or(1, |a| a.layer));
            v
        })
        .collect()
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

/// `calibration.json`, or the identity. A damaged file is a safety file
/// (T-402): quarantined, the identity used, and arming waits until the
/// operator has acknowledged it.
pub fn load_calibration(path: &Path) -> Calibration {
    crate::datafile::load_or_default(path, &crate::datafile::beside(path), true)
}

fn save_calibration(path: &Path, cal: &Calibration) {
    crate::datafile::save_json(path, cal);
}

/// Stack of the import thread.
const IMPORT_STACK: usize = 64 * 1024 * 1024;

/// `POST /api/figures/import?name=…&simplify=…`: an SVG, PNG or JPEG
/// (body, at most `MAX_FILE_BYTES`) → `{figure, warnings, stats}`. Runs
/// without the lock held; a panic in the importer is caught and reported.
fn import_figure(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::figure_import::{self as fi, Mode, Options};
    use std::io::Read;
    let limit = fi::MAX_FILE_BYTES;
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(limit as u64 + 1).read_to_end(&mut bytes) {
        return text(400, &format!("fichier illisible : {e}"));
    }
    if bytes.len() > limit {
        return text(413, &format!("fichier trop gros : {} Mo au plus", limit / (1024 * 1024)));
    }
    let url = request.url().to_string();
    let q = |k: &str| query_str(&url, k);
    let num = |k: &str| q(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
    let d = Options::default();
    let budget = shared.lock().unwrap().mixer.point_budget;
    let opts = Options {
        simplify: num("simplify").unwrap_or(d.simplify),
        size: num("size").map_or(d.size, |v| v / 100.0),
        budget: num("budget").map_or(budget, |v| v.max(0.0) as usize),
        color: q("color").and_then(|c| parse_hex(&c)).unwrap_or(d.color),
        fills: q("fills").map_or(d.fills, |v| v != "0"),
        mode: q("mode").and_then(|m| Mode::parse(&m)).unwrap_or(d.mode),
        threshold: num("threshold").map(|v| v.clamp(0.0, 255.0) as u8),
        invert: q("invert").is_some_and(|v| v == "1"),
        smooth: num("smooth").map_or(d.smooth, |v| v.clamp(0.0, 5.0) as u8),
        colors: num("colors").map_or(d.colors, |v| v.clamp(1.0, 6.0) as u8),
        resolution: num("resolution").map_or(d.resolution, |v| v.clamp(0.0, 10_000.0) as u32),
        min_size: num("min_size").map_or(d.min_size, |v| v.clamp(0.0, 1_000.0) as u32),
    };
    let name = q("name").unwrap_or_default();
    // Its own thread with a roomy stack (the XML parser recurses), so a
    // file can't take the HTTP worker down; a panic is caught as well.
    let job = std::thread::Builder::new().name("figure-import".into()).stack_size(IMPORT_STACK).spawn({
        let name = name.clone();
        move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fi::import(&bytes, &name, &opts)))
    });
    match job.map(|h| h.join()) {
        Ok(Ok(Ok(Ok(result)))) => json_response(json!(result)),
        Ok(Ok(Ok(Err(e)))) => text(400, &format!("{e:#}")),
        _ => {
            log::warn!("figure import failed internally on {name:?}");
            text(400, "fichier impossible à importer (erreur interne)")
        }
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    Some([u8::from_str_radix(&h[0..2], 16).ok()?, u8::from_str_radix(&h[2..4], 16).ok()?, u8::from_str_radix(&h[4..6], 16).ok()?])
}

/// `key`'s value in the URL's query string, as a number.
fn query_u64(url: &str, key: &str) -> Option<u64> {
    url.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

/// `key`'s value in the URL's query string, percent-decoded (UTF-8).
fn query_str(url: &str, key: &str) -> Option<String> {
    let raw = url.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A scene or the playlist started while armed goes to the safety log.
fn log_scene(s: &Shared, name: &str) {
    if s.gate.is_armed() {
        s.gate.log().record("scene", Some("ui"), json!({ "name": name }));
    }
}

/// `GET /api/safety/log?date=AAAA-MM-JJ` (default today): `{date, days,
/// events, status}`; `GET /api/safety/log.csv?date=…`: the same day as a
/// CSV download (T-259). Waits briefly for the writer so the latest events
/// are in; never holds the lock while reading the file.
fn safety_log_view(url: &str, shared: &Arc<Mutex<Shared>>, csv: bool) -> HttpResponse {
    let log = shared.lock().unwrap().gate.log().clone();
    let Some(dir) = log.dir().map(Path::to_path_buf) else { return text(404, "journal de sécurité désactivé") };
    let date = match query_str(url, "date").filter(|d| !d.is_empty()) {
        Some(d) => match crate::safety_log::parse_day(&d) {
            Some(day) => day.to_string(),
            None => return text(400, "date attendue au format AAAA-MM-JJ"),
        },
        None => crate::safety_log::today().to_string(),
    };
    log.flush(Duration::from_millis(200));
    let events = crate::safety_log::read_day(&dir, &date);
    if csv {
        let disposition = format!("attachment; filename=\"journal-securite-{date}.csv\"");
        let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
        return with_type(Response::from_string(crate::safety_log::to_csv(&events)), "text/csv; charset=utf-8").with_header(header);
    }
    json_response(json!({
        "date": date,
        "today": crate::safety_log::today().to_string(),
        "days": crate::safety_log::days(&dir),
        "events": crate::safety_log::view(&events),
        "status": log.status(),
    }))
}

/// The pre-show checklist (T-258). `operator {name}`, `check {id, on,
/// value?}`, `complete`, `restart`, `config {items, outdoor,
/// audience_scan}` or `config {defaults: true}`. Each answers the
/// checklist's view (400 with the French reason when refused; `complete`
/// answers 409 `{error, missing}`). Completing only lets the operator
/// arm: nothing here ever arms. Restarting or editing the list closes
/// the `checklist` interlock again, which disarms if armed.
fn checklist_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, action: &str) -> HttpResponse {
    let mut raw = String::new();
    if let Err(e) = request.as_reader().read_to_string(&mut raw) {
        return text(400, &format!("unreadable body: {e}"));
    }
    // `complete` and `restart` need no body.
    let v: serde_json::Value = match raw.trim() {
        "" => json!({}),
        raw => match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => return text(400, &format!("invalid JSON: {e}")),
        },
    };
    let mut s = shared.lock().unwrap();
    let s = &mut *s;
    let result: anyhow::Result<()> = match action {
        "operator" => s.checklist.set_operator(v.get("name").and_then(|n| n.as_str()).unwrap_or("")),
        "check" => match (v.get("id").and_then(|i| i.as_str()), v.get("on").and_then(|o| o.as_bool())) {
            (Some(id), Some(on)) => s.checklist.check(id, on, v.get("value").and_then(|x| x.as_str())),
            _ => return text(400, "expected \"id\" and \"on\""),
        },
        "complete" => match s.checklist.complete() {
            Ok(detail) => {
                s.gate.log().record("checklist", Some("ui"), detail);
                Ok(())
            }
            Err(missing) => {
                return with_status(json_response(json!({ "error": "Liste de contrôle incomplète", "missing": missing, "checklist": s.checklist.view() })), 409);
            }
        },
        "restart" => {
            if s.checklist.restart() {
                s.gate.log().record("checklist_reset", Some("ui"), json!({}));
            }
            Ok(())
        }
        "config" => {
            let config = if v.get("defaults").and_then(|d| d.as_bool()).unwrap_or(false) {
                Ok(crate::checklist::ChecklistConfig::default())
            } else {
                serde_json::from_value::<crate::checklist::ChecklistConfig>(v)
            };
            match config {
                Err(e) => return text(400, &format!("invalid JSON: {e}")),
                Ok(config) => {
                    s.gate.sync_estop(&s.estop);
                    let before = s.checklist.config().clone();
                    let r = s.checklist.set_config(config, s.gate.is_armed());
                    if r.is_ok() {
                        if let Some(change) = crate::safety_log::settings_change(&before, s.checklist.config()) {
                            s.gate.log().record("checklist_edit", Some("ui"), change);
                        }
                    }
                    r
                }
            }
        }
        _ => return text(404, "not found"),
    };
    s.sync_checklist();
    match result {
        Ok(()) => json_response(s.checklist.view()),
        Err(e) => text(400, &e.to_string()),
    }
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

/// `POST /api/test/tempo_estimate` (`--test-hooks`): a simulated detector.
#[derive(Deserialize)]
struct SimEstimate {
    bpm: f32,
    confidence: f32,
    state: crate::audio::bpm::DetectState,
    #[serde(default)]
    offset_s: f64,
}

impl SimEstimate {
    fn snapshot(&self, now: f64) -> crate::audio::NativeSnapshot {
        let spb = 60.0 / (self.bpm as f64).clamp(crate::tempo::MIN_BPM, crate::tempo::MAX_BPM);
        let beat_time = self.offset_s + ((now - self.offset_s) / spb).floor() * spb;
        let tempo = crate::audio::bpm::TempoEstimate { bpm: self.bpm, confidence: self.confidence, beat_time, next_beat: beat_time + spb, state: self.state };
        crate::audio::NativeSnapshot {
            features: crate::engine::AudioFeatures::default(),
            rms_db: crate::audio::analysis::FLOOR_DB,
            peak_db: crate::audio::analysis::FLOOR_DB,
            spectral: Default::default(),
            onsets: Default::default(),
            tempo,
            spectrum: [crate::audio::analysis::FLOOR_DB; crate::audio::spectrum::SPECTRUM_BANDS],
            sections: Default::default(),
            t: now,
            at: Instant::now(),
        }
    }
}

/// `POST /api/test/native_audio` (`--test-hooks`): a simulated native input.
/// Every field is optional; what is sent is applied.
#[derive(Deserialize)]
struct SimNative {
    /// Capture state (and the device it names), as the capture thread would set it.
    state: Option<crate::audio::CaptureState>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    message: String,
    /// The device list `GET /api/audio/devices` serves.
    devices: Option<Vec<crate::audio::capture::InputDevice>>,
    /// One analysis snapshot.
    snapshot: Option<SimSnapshot>,
}

#[derive(Deserialize)]
struct SimSnapshot {
    /// An `AudioFeatures` v2 body, read like `POST /api/audio`'s.
    #[serde(default)]
    features: serde_json::Value,
    /// Meter (dBFS); the analysis floor when absent.
    level_db: Option<f32>,
    peak_db: Option<f32>,
    /// The 64-band display spectrum in dBFS (fewer values: the rest at the floor).
    #[serde(default)]
    spectrum_db: Vec<f32>,
    /// The detector's tempo state (the BPM and confidence are the features').
    #[serde(default)]
    tempo_state: crate::audio::bpm::DetectState,
    /// Seconds since the section started.
    #[serde(default)]
    section_since_s: f32,
}

impl SimNative {
    fn apply(self, hub: &crate::audio::AudioHub, now: f64) -> anyhow::Result<()> {
        use crate::audio::analysis::FLOOR_DB;
        use crate::audio::spectrum::SPECTRUM_BANDS;
        let snapshot = match self.snapshot {
            Some(sim) => {
                let body = if sim.features.is_null() { json!({}) } else { sim.features };
                let mut features = crate::audio::browser_features(&body, now)?;
                let level_db = sim.level_db.unwrap_or(FLOOR_DB);
                if body.get("level_db").is_none() {
                    features.level_db = level_db;
                }
                let mut spectrum = [FLOOR_DB; SPECTRUM_BANDS];
                for (d, v) in spectrum.iter_mut().zip(&sim.spectrum_db) {
                    *d = v.clamp(FLOOR_DB, 12.0);
                }
                let spb = 60.0 / f64::from(features.bpm.max(40.0));
                let tempo = crate::audio::bpm::TempoEstimate { bpm: features.bpm, confidence: features.bpm_confidence, beat_time: now, next_beat: now + spb, state: sim.tempo_state };
                let onsets = crate::audio::onsets::Onsets { onset: features.onset, kick: features.kick, snare: features.snare, hat: features.hat, ..Default::default() };
                let sections = crate::audio::sections::SectionState { section: features.section, buildup: features.buildup, drop: features.drop, since_s: sim.section_since_s, ..Default::default() };
                Some(crate::audio::NativeSnapshot {
                    features,
                    rms_db: level_db,
                    peak_db: sim.peak_db.unwrap_or(level_db),
                    spectral: Default::default(),
                    onsets,
                    tempo,
                    spectrum,
                    sections,
                    t: now,
                    at: Instant::now(),
                })
            }
            None => None,
        };
        if let Some(state) = self.state {
            let device = self.device.or_else(|| hub.status().device);
            hub.set_status(crate::audio::CaptureStatus { state, device, message: self.message, sample_rate: 48_000, channels: 1, overruns: 0 });
        }
        if let Some(devices) = self.devices {
            hub.set_devices(devices);
        }
        if let Some(snapshot) = snapshot {
            hub.publish(snapshot);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{DeviceInfo, DeviceList};
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
        /// T-358: a test stuck on the server (or on its join) fails fast.
        _guard: test_support::TimeGuard,
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
            let _guard = test_support::time_guard(&format!("web API test on {addr}"), Duration::from_secs(180));
            Self { addr, shared, running, thread: Some(thread), _guard }
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

    /// T-258: with a (simulated) real output, arming waits for the
    /// checklist; completing it never arms; restarting it disarms.
    #[test]
    fn the_checklist_gates_arming_only_when_enforced() {
        let t = TestServer::start(false);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200, "preview: no checklist interlock");
        t.request("POST", "/api/arm", r#"{"on":false}"#);
        {
            let mut s = t.shared.lock().unwrap();
            s.checklist = crate::checklist::Checklist::in_memory(true);
            s.sync_checklist();
        }
        let (status, body) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(status, 409);
        assert!(body.contains(crate::checklist::LABEL_FR), "{body}");
        let (status, body) = t.request("POST", "/api/checklist/complete", "");
        assert_eq!(status, 409);
        assert!(body.contains("Nom de l'opérateur"), "{body}");
        assert_eq!(t.request("POST", "/api/checklist/operator", r#"{"name":"Michel"}"#).0, 200);
        let view: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/checklist", "").1).unwrap();
        for item in view["items"].as_array().unwrap().iter().filter(|i| i["visible"] == true) {
            let body = json!({ "id": item["id"], "on": true }).to_string();
            assert_eq!(t.request("POST", "/api/checklist/check", &body).0, 200);
        }
        assert_eq!(t.request("POST", "/api/checklist/check", r#"{"id":"nope","on":true}"#).0, 400);
        let (status, body) = t.request("POST", "/api/checklist/complete", "");
        assert_eq!(status, 200, "{body}");
        assert_eq!(arm_status(&t)["armed"], false, "completing never arms");
        // T-255: and the audience is located (a full horizon at 0 %).
        let (status, body) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(status, 409);
        assert!(body.contains("Aucune zone Public"), "{body}");
        let mut safety = json!(t.shared.lock().unwrap().safety.get());
        safety["horizon"]["lines"] = json!(true);
        safety["horizon"]["level"] = json!(0.0);
        assert_eq!(t.request("POST", "/api/safety", &safety.to_string()).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        // Editing the list while armed is refused; restarting it disarms.
        let cfg = json!(t.shared.lock().unwrap().checklist.config()).to_string();
        assert_eq!(t.request("POST", "/api/checklist/config", &cfg).0, 400);
        assert_eq!(t.request("POST", "/api/checklist/restart", "").0, 200);
        let st = arm_status(&t);
        assert_eq!(st["armed"], false);
        assert_eq!(st["last_disarm"]["reason_fr"], format!("Verrou : {}", crate::checklist::LABEL_FR));
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 409);
    }

    /// T-255: `/api/audience`. Above by default; unlocking answers 409
    /// with the missing conditions, then while armed, then without the
    /// phrase; 423 in show mode, 403 from a controller; never arms.
    #[test]
    fn the_audience_route_unlocks_only_through_every_step() {
        let t = TestServer::start(false);
        let unlock = |phrase: &str| t.request("POST", "/api/audience", &json!({ "mode": "scanning", "phrase": phrase }).to_string());
        let view: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/audience", "").1).unwrap();
        assert_eq!(view["mode"], "above");
        assert_eq!(view["conditions"].as_array().unwrap().len(), 5);
        assert_eq!(view["phrase"], crate::audience::PHRASE);
        let (status, body) = unlock(crate::audience::PHRASE);
        assert_eq!(status, 409);
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["missing"].as_array().unwrap().len(), 5, "{body}");
        // Every condition met (fictitious declarations).
        {
            let mut s = t.shared.lock().unwrap();
            let zone = crate::zones::Zone { name: "Public".into(), kind: crate::zones::ZoneKind::Dim, level: 0.1, points: vec![[-1.0, -1.0], [1.0, -1.0], [0.0, 0.0]], audience: true, ..Default::default() };
            let mut cfg = crate::safety::SafetySettings { zones: vec![zone], ..Default::default() };
            cfg.dwell.profile = crate::dwell::DwellProfile::Strict;
            s.safety.set(cfg, true).unwrap();
            let sheet = crate::power::ProjectorInfo { name: "P".into(), power_mw: [10.0; 3], divergence_mrad: 1.0, hw_scan_fail: Some(true), ..Default::default() };
            s.outputs.set(crate::power::MAIN_OUTPUT, None, Some(sheet), false, false).unwrap();
            let mut list = s.checklist.config().clone();
            list.audience_scan = true;
            s.checklist.set_config(list, false).unwrap();
            s.checklist.set_operator("Op").unwrap();
            let ids: Vec<String> = s.checklist.visible_items().map(|i| i.id.clone()).collect();
            for id in ids {
                s.checklist.check(&id, true, Some("1")).unwrap();
            }
            s.checklist.complete().unwrap();
            s.presence.settings.hold_to_run = true;
        }
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (status, body) = unlock(crate::audience::PHRASE);
        assert_eq!((status, serde_json::from_str::<serde_json::Value>(&body).unwrap()["armed"].clone()), (409, json!(true)));
        t.request("POST", "/api/arm", r#"{"on":false}"#);
        let (status, body) = unlock("JE PRENDS");
        assert_eq!(status, 409);
        assert!(body.contains("needs_phrase"), "{body}");
        let source = t.request("POST", "/api/audience", &json!({ "mode": "scanning", "phrase": crate::audience::PHRASE, "source": "midi" }).to_string());
        assert_eq!(source.0, 403);
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(unlock(crate::audience::PHRASE).0, 423);
        t.shared.lock().unwrap().show_mode.on = false;
        let (status, body) = unlock(crate::audience::PHRASE);
        assert_eq!(status, 200, "{body}");
        assert_eq!(arm_status(&t)["armed"], false, "unlocking never arms");
        let state: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert_eq!(state["audience"]["mode"], "scanning");
        // Back above: allowed in show mode and armed, it only tightens.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        t.shared.lock().unwrap().show_mode.on = true;
        let (status, body) = t.request("POST", "/api/audience", r#"{"mode":"above"}"#);
        assert_eq!(status, 200, "{body}");
        assert!(body.contains(r#""mode":"above""#), "{body}");
        assert_eq!(arm_status(&t)["armed"], true, "going back above leaves the arming alone");
        assert_eq!(t.request("POST", "/api/audience", r#"{"mode":"nope"}"#).0, 400);
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

    /// T-404: the pattern API shows and leaves a pattern, never arms, and
    /// refuses unknown names and bad brightness values.
    #[test]
    fn test_pattern_api_selects_and_leaves_without_arming() {
        let t = TestServer::start(false);
        let (status, body) = t.request("GET", "/api/testpattern", "");
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["patterns"].as_array().unwrap().len(), 7);
        assert_eq!((v["state"]["active"].clone(), v["state"]["brightness"].as_f64()), (json!(null), Some(0.2f32 as f64)));
        let (status, body) = t.request("POST", "/api/testpattern", r#"{"pattern":"grid","brightness":0.3}"#);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["state"]["active"], "grid");
        assert!(!t.shared.lock().unwrap().gate.is_armed(), "a pattern never arms");
        assert_eq!(t.request("POST", "/api/testpattern", r#"{"pattern":"ilda"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/testpattern", r#"{"brightness":"x"}"#).0, 400);
        assert_eq!(t.shared.lock().unwrap().test_pattern.active, Some(crate::testpattern::TestPattern::Grid), "a refused body changes nothing");
        // Brightness alone keeps the pattern; it is clamped to 0..1.
        t.request("POST", "/api/testpattern", r#"{"brightness":5}"#);
        assert_eq!(t.shared.lock().unwrap().test_pattern.brightness, 1.0);
        assert_eq!(t.request("POST", "/api/testpattern", r#"{"pattern":null}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().test_pattern.active, None);
    }

    #[test]
    fn figure_import_previews_without_saving_and_refuses_bad_files() {
        let t = TestServer::start(false);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="10" stroke="red"/></svg>"#;
        let (status, body) = t.request("POST", "/api/figures/import?name=Mon%20logo%2B1.svg&budget=300&size=50", svg);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["figure"]["name"], "Mon logo 1");
        assert_eq!(v["stats"]["kind"], "svg");
        assert_eq!(v["stats"]["budget"], 300);
        assert_eq!(v["figure"]["frames"][0]["strokes"][0]["color"], json!([255, 0, 0]));
        let xs: Vec<f64> = v["figure"]["frames"][0]["strokes"][0]["points"].as_array().unwrap().iter().map(|p| p[0].as_f64().unwrap()).collect();
        assert!(xs.iter().all(|x| x.abs() <= 0.5 + 1e-4) && xs.iter().any(|x| (x.abs() - 0.5).abs() < 1e-3));
        // A preview only: nothing in the library.
        assert!(t.shared.lock().unwrap().figures.list().is_empty());
        let (status, body) = t.request("POST", "/api/figures/import", "not an image");
        assert_eq!(status, 400);
        assert!(body.contains("format non reconnu"), "{body}");
        let (status, body) = t.request("POST", "/api/figures/import", "");
        assert_eq!((status, body.as_str()), (400, "fichier vide"));
        let deep = format!("<svg>{}</svg>", "<g>".repeat(50_000));
        let (status, body) = t.request("POST", "/api/figures/import", &deep);
        assert_eq!(status, 400);
        assert!(body.contains("imbriqués"), "{body}");
        // Still serving.
        assert_eq!(t.request("GET", "/api/arm", "").0, 200);
        assert_eq!(parse_hex("#00ff80"), Some([0, 255, 128]));
        assert_eq!(parse_hex("#00ff8"), None);
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

    fn get_bytes(addr: SocketAddr, path: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&raw[..split]).into_owned(), raw[split + 4..].to_vec())
    }

    /// T-403: the diagnostic report is a zip download with the logs and
    /// the snapshots, the project only on request; the tail shows the log.
    #[test]
    fn the_diagnostic_report_downloads_a_zip_and_the_tail_shows_the_log() {
        use crate::applog::{testing::read_zip, AppLog, LogSettings};
        let dir = std::env::temp_dir().join(format!("laser-studio-web-applog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let t = TestServer::start(false);
        let app = Arc::new(AppLog::start(dir.join("logs"), &LogSettings::default(), Vec::new()));
        app.push(log::Level::Info, "laser_studio", "Démarrage de Laser Studio".into());
        t.shared.lock().unwrap().applog = Arc::clone(&app);

        let (head, zip) = get_bytes(t.addr, "/api/diagnostics/report");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("application/zip") && head.contains("filename=\"laser-studio-diagnostic-"), "{head}");
        assert_eq!(&zip[..2], b"PK");
        let entries = read_zip(&zip);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        for name in ["about.json", "devices.json", "settings.json", "state.json"] {
            assert!(names.contains(&name), "{names:?}");
        }
        assert!(names.iter().any(|n| n.starts_with("logs/app-")), "{names:?}");
        assert!(!names.contains(&"project.json"), "not without the box ticked");
        let state: serde_json::Value = serde_json::from_slice(&entries.iter().find(|(n, _)| n == "state.json").unwrap().1).unwrap();
        assert_eq!(state["armed"], false);
        let settings: serde_json::Value = serde_json::from_slice(&entries.iter().find(|(n, _)| n == "settings.json").unwrap().1).unwrap();
        assert!(settings["safety"].is_object() && settings["calibration"].is_object(), "{settings}");

        let (_, zip) = get_bytes(t.addr, "/api/diagnostics/report?project=1");
        let entries = read_zip(&zip);
        let project: serde_json::Value = serde_json::from_slice(&entries.iter().find(|(n, _)| n == "project.json").unwrap().1).unwrap();
        assert!(project.get("figures").is_none() && project["figures_omitted"] == 0, "no media in the project");

        let (code, body) = t.request("GET", "/api/logs/tail?n=5", "");
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let lines = v["lines"].as_array().unwrap();
        assert!(lines.len() <= 5 && lines.iter().any(|l| l.as_str().unwrap().contains("Démarrage")), "{v}");
        let (_, state) = t.request("GET", "/api/state", "");
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!((state["applog"]["enabled"].as_bool(), state["applog"]["write_failed"].as_bool()), (Some(true), Some(false)));
        drop(t);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-239 / T-240: presets replace the routes, trigger rules are checked
    /// and logged, the switch is a control that never arms; in show mode
    /// the rules and presets are locked, the switch is not.
    #[test]
    fn audio_presets_and_triggers_over_the_api() {
        let t = TestServer::start(false);
        let json = |(code, body): (u16, String)| -> (u16, serde_json::Value) { (code, serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)) };
        let (code, list) = json(t.request("GET", "/api/audio/presets", ""));
        assert_eq!(code, 200);
        assert_eq!(list["presets"].as_array().unwrap().len(), 5);
        assert_eq!(t.request("POST", "/api/audio/preset", r#"{"id":"nope"}"#).0, 400);
        let (code, r) = json(t.request("POST", "/api/audio/preset", r#"{"id":"punch"}"#));
        assert_eq!((code, r["routes"].as_array().unwrap().len()), (200, 2));
        let (_, routes) = json(t.request("GET", "/api/audio/routes", ""));
        assert_eq!((routes["preset"].as_str(), routes["mix"].as_f64()), (Some("punch"), Some(0.5)));
        assert_eq!(routes["flicker"], serde_json::json!([false, false]));

        assert_eq!(t.request("POST", "/api/audio/triggers", r#"{"triggers":[{"event":"drop","action":{"launch_cue":"nope"}}]}"#).0, 400);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let body = format!(r#"{{"triggers":[{{"event":"drop","action":{{"launch_cue":"{cue}"}}}}]}}"#);
        let (code, v) = json(t.request("POST", "/api/audio/triggers", &body));
        assert_eq!((code, v["enabled"].as_bool(), v["triggers"].as_array().map(Vec::len)), (200, Some(false), Some(1)));
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"audio.triggers.enabled","value":1}"#).0, 200);
        let (_, v) = json(t.request("GET", "/api/audio/triggers", ""));
        assert_eq!(v["enabled"], true);
        assert!(!t.shared.lock().unwrap().gate.is_armed());

        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/audio/triggers", r#"{"triggers":[]}"#).0, 423);
        assert_eq!(t.request("POST", "/api/audio/preset", r#"{"id":"calm"}"#).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"audio.triggers.enabled","value":0}"#).0, 200, "the switch is playback");
        assert!(!t.shared.lock().unwrap().triggers.enabled());
    }

    /// T-283: in show mode, edits get 423 and change nothing; playing,
    /// arming, disarming and the e-stop work as before. The lock never arms.
    #[test]
    fn show_mode_refuses_edits_and_keeps_playback_and_safety() {
        let t = TestServer::start(false);
        let scene = crate::scenes::Scene { name: "Garde".into(), settings: Settings::default(), duration_secs: 8.0 };
        let dir = crate::datafile::tests::temp_dir("show-mode-web");
        let mut store = crate::scenes::SceneStore::load_or_create(dir.join("scenes.json"));
        store.replace_in_memory(vec![scene]);
        t.shared.lock().unwrap().scenes = store;
        let lock = |on: bool| t.request("POST", "/api/lock", &format!(r#"{{"show_mode":{on}}}"#));
        let (code, body) = lock(true);
        assert_eq!(code, 200);
        assert!(body.contains(r#""show_mode":true"#), "{body}");
        assert!(!arm_status(&t)["armed"].as_bool().unwrap(), "locking never arms");
        let state: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert_eq!(state["lock"]["show_mode"], true);

        for (path, body) in [
            ("/api/scenes/delete", r#"{"name":"Garde"}"#),
            ("/api/scenes/save", r#"{"name":"Autre"}"#),
            ("/api/settings", "{}"),
            ("/api/calibration", r#"{"offset_x":0.5}"#),
            ("/api/projection", r#"{"zones":[{"id":1,"geometry":{"keystone_x":0.3}}]}"#),
            ("/api/projection/align", r#"{"zone":1}"#),
            ("/api/safety", r#"{"strobe_max_hz":3}"#),
            ("/api/outputs/limits", r#"{"limits":{"max_power":0.3}}"#),
            ("/api/outputs/color", r#"{"color":{"white_balance":[0.5,1,1]}}"#),
            ("/api/presence", "{}"),
            ("/api/checklist/config", r#"{"defaults":true}"#),
            ("/api/testpattern", r#"{"pattern":"grid"}"#),
            ("/api/testpattern", r#"{"brightness":0.5}"#),
            ("/api/project/new", "{}"),
            ("/api/project/open", r#"{"path":"x"}"#),
            ("/api/midi/learn", r#"{"target":"master.size"}"#),
            ("/api/cues/slot", r#"{"id":"tunnels-001"}"#),
            ("/api/grid", r#"{"cols":6,"rows":4}"#),
            ("/api/grid/page", r#"{"action":"new"}"#),
            ("/api/grid/move", r#"{"from":[1,0],"to":[0,null]}"#),
            ("/api/grid/clear", r#"{"page":0,"slot":0}"#),
            ("/api/grid/pages", r#"{"op":"add"}"#),
            ("/api/grid/edit", r#"{"op":"delete","from":[0,0]}"#),
            ("/api/cues/store", r#"{"page":0,"slot":0}"#),
            ("/api/cues/update", "{}"),
            ("/api/timeline/edit", "{}"),
        ] {
            let (code, text) = t.request("POST", path, body);
            assert_eq!((code, text.as_str()), (423, crate::showmode::LOCKED_FR), "{path} {body}");
        }
        assert_eq!(t.shared.lock().unwrap().scenes.list().len(), 1, "the scene is still there");
        assert_eq!(t.shared.lock().unwrap().calibration.offset_x, 0.0);
        assert_eq!(t.shared.lock().unwrap().projection.get(), &crate::projection::ProjectionConfig::default());
        assert_eq!(t.shared.lock().unwrap().test_pattern.active, None);

        // Reading is never locked.
        assert_eq!(t.request("GET", "/api/safety", "").0, 200);
        // Playing and safety still work.
        assert_eq!(t.request("POST", "/api/scenes/play", r#"{"name":"Garde"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/cue", r#"{"id":"tunnels-001"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"master.brightness","value":0.2}"#).0, 200);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"tempo.tap"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/testpattern", r#"{"pattern":null}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false,"source":"keyboard"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
        assert_eq!(t.request("POST", "/api/estop/reset", "").0, 200);
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"client_id":"p1"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":"oui"}"#).0, 400);

        // Unlocked: edits work again, and unlocking doesn't arm.
        assert_eq!(lock(false).0, 200);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
        assert_eq!(t.request("POST", "/api/scenes/delete", r#"{"name":"Garde"}"#).0, 200);
        assert!(t.shared.lock().unwrap().scenes.list().is_empty());
        let (code, body) = t.request("GET", "/api/lock", "");
        assert_eq!((code, body.as_str()), (200, r#"{"show_mode":false}"#));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-259: the log API reads back what the gate and the handlers logged.
    #[test]
    fn the_safety_log_api_shows_the_day_in_order_and_exports_csv() {
        let dir = std::env::temp_dir().join(format!("laser-studio-web-safety-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let t = TestServer::start(false);
        t.shared.lock().unwrap().gate.set_log(crate::safety_log::SafetyLog::start(dir.clone()));
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/cue", r#"{"id":"tunnels-001"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false,"source":"keyboard"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert_eq!(t.request("POST", "/api/estop/reset", "").0, 200);
        assert_eq!(t.request("POST", "/api/safety", r#"{"strobe_max_hz":3}"#).0, 200);
        assert_eq!(t.request("POST", "/api/midi/safety", r#"{"allow_arm":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":0.3}}"#).0, 200);
        let (code, body) = t.request("GET", "/api/safety/log", "");
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let kinds: Vec<&str> = v["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["arm", "cue", "disarm", "arm", "estop", "estop_reset", "safety_settings", "midi_safety", "power_caps"]);
        let events = v["events"].as_array().unwrap();
        assert_eq!((events[2]["source"].as_str(), events[2]["source_fr"].as_str()), (Some("keyboard"), Some("clavier")));
        assert_eq!(events[4]["text"], "Arrêt d'urgence (laser désarmé)");
        assert_eq!(events[6]["detail"]["changed"], json!(["strobe_max_hz"]));
        assert_eq!((events[6]["detail"]["before"]["strobe_max_hz"].as_f64(), events[6]["detail"]["after"]["strobe_max_hz"].as_f64()), (Some(4.0), Some(3.0)));
        assert_eq!(events[7]["text"], "Armement MIDI autorisé");
        assert_eq!(events[8]["detail"]["changed"], json!(["limits"]));
        assert_eq!((events[8]["detail"]["before"]["limits"]["max_power"].as_f64(), events[8]["detail"]["after"]["limits"]["max_power"].as_f64()), (Some(0.5), Some(0.3f32 as f64)));
        assert_eq!(events[8]["text"], "Plafonds de puissance modifiés (main) : limits");
        assert_eq!(v["date"], v["today"]);
        assert_eq!(v["days"], json!([v["today"]]));
        assert_eq!(v["status"]["failed"], 0);

        let date = v["date"].as_str().unwrap();
        let (code, csv) = t.request("GET", &format!("/api/safety/log.csv?date={date}"), "");
        assert_eq!(code, 200);
        assert_eq!(csv.lines().count(), 10, "header + 9 events");
        assert!(csv.contains(",estop,clavier,"), "{csv}");
        assert_eq!(t.request("GET", "/api/safety/log?date=..%2F..%2Fetc", "").0, 400);
        let (code, body) = t.request("GET", "/api/safety/log?date=2001-01-01", "");
        assert_eq!(code, 200);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["events"], json!([]));
        let _ = std::fs::remove_dir_all(dir);
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

    /// Presence enforced as in the real studio (no page yet).
    fn enforce_presence(t: &TestServer) {
        let mut s = t.shared.lock().unwrap();
        s.presence = crate::presence::Presence::new(Default::default(), true);
        s.gate.register(crate::interlock::UI_ALIVE, "Aucune interface ouverte", false);
    }

    #[test]
    fn heartbeats_let_a_page_arm_and_gone_disarms_on_the_next_sync() {
        let t = TestServer::start(false);
        enforce_presence(&t);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 409, "no page");
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"client_id":"p1","visible":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 200);
        // sendBeacon posts text/plain: only the body matters.
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"client_id":"p1","gone":true}"#).0, 200);
        t.shared.lock().unwrap().sync_safety(Instant::now());
        let st = arm_status(&t);
        assert_eq!(st["armed"], false);
        assert_eq!(st["last_disarm"]["reason"], "ui_lost");
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"visible":true}"#).0, 400, "client_id is required");
    }

    #[test]
    fn an_arm_request_with_a_client_id_counts_as_a_beat() {
        let t = TestServer::start(false);
        enforce_presence(&t);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard","client_id":"p1"}"#).0, 200);
        assert_eq!(arm_status(&t)["armed"], true);
    }

    #[test]
    fn presence_settings_are_clamped_and_reported() {
        let t = TestServer::start(false);
        let (status, body) = t.request("POST", "/api/presence", r#"{"ui_timeout_ms":60000,"hold_to_run":true,"hold_key":"Space"}"#);
        assert_eq!(status, 200);
        let kept: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(kept["ui_timeout_ms"], 10_000);
        assert_eq!(kept["hold_key"], "ShiftRight");
        let got: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/presence", "").1).unwrap();
        assert_eq!(got["settings"]["hold_to_run"], true);
        assert_eq!(got["status"]["hold_to_run"], true);
        let state: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert!(state["engine_ok"].is_boolean());
        assert_eq!(state["presence"]["hold_key"], "ShiftRight");
        // Changing presence settings never arms.
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn dmx_personality_route_and_fixture_settings() {
        let t = TestServer::start(false);
        let js = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        let both = js(&t.request("GET", "/api/dmx/personality", "").1);
        assert_eq!(both["modes"].as_array().unwrap().len(), 2);
        let compact = js(&t.request("GET", "/api/dmx/personality?mode=compact", "").1);
        assert_eq!((compact["width"].as_u64(), compact["channels"][15]["key"].as_str()), (Some(16), Some("access")));
        let (code, md) = t.request("GET", "/api/dmx/personality?mode=full&format=md", "");
        assert_eq!(code, 200);
        assert!(md.contains("Complet — 32 canaux") && !md.contains("Compact — 16"));
        let (code, xml) = t.request("GET", "/api/dmx/personality?format=xml", "");
        assert!(code == 200 && xml.contains("<GDTF DataVersion=\"1.2\">"));
        assert_eq!(t.request("GET", "/api/dmx/personality?mode=huge", "").0, 400);
        assert_eq!(t.request("GET", "/api/dmx/personality?format=pdf", "").0, 400);
        // Two instances; an overlap is refused with the zone's name.
        let ok = r#"{"dmx":{"fixtures":[{"enabled":true,"zone":1,"mode":"compact","address":1},{"enabled":false,"zone":2,"mode":"full","address":17}]}}"#;
        assert_eq!(t.request("POST", "/api/dmx/config", ok).0, 200);
        let bad = r#"{"dmx":{"fixtures":[{"zone":1,"address":1},{"zone":2,"mode":"full","address":10}]}}"#;
        let (code, msg) = t.request("POST", "/api/dmx/config", bad);
        assert_eq!(code, 400);
        assert_eq!(msg, "Les adresses 10–41 chevauchent l'appareil de la zone « Principale ».");
        let view = js(&t.request("GET", "/api/netin", "").1);
        let list = &view["dmx"]["fixtures"]["list"];
        assert_eq!((list[0]["fixture"]["address"].as_u64(), list[1]["last"].as_u64()), (Some(1), Some(48)));
        assert_eq!(list[0]["signal"], "off", "DMX inputs are off");
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn network_inputs_route_reads_validates_and_never_arms() {
        let t = TestServer::start(false);
        let view: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/netin", "").1).unwrap();
        assert_eq!((view["effective"]["osc"]["enabled"].as_bool(), view["effective"]["artnet"]["enabled"].as_bool()), (Some(false), Some(false)));
        assert_eq!(view["artnet"]["signal"], "off");
        assert_eq!(view["artnet"]["channels"].as_array().unwrap().len(), crate::netin::artnet::CHART.len());
        let (status, body) = t.request("POST", "/api/netin", r#"{"artnet":{"universe":2,"address":20},"loss":{"policy":"hold"}}"#);
        assert_eq!(status, 200, "{body}");
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((view["config"]["artnet"]["universe"].as_u64(), view["artnet"]["channels"][0]["channel"].as_u64()), (Some(2), Some(20)));
        let (status, body) = t.request("POST", "/api/netin", r#"{"osc":{"bind":"console.local"}}"#);
        assert_eq!(status, 400);
        assert!(body.contains("adresse d'écoute invalide"), "{body}");
        assert_eq!(t.request("POST", "/api/netin", "[]").0, 400);
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
    }

    #[test]
    fn keep_awake_setting_and_status_are_reported() {
        let t = TestServer::start(false);
        let state: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert_eq!(state["keep_awake"], json!({ "enabled": true, "held": false, "error": null }));
        let (status, body) = t.request("POST", "/api/keep_awake", r#"{"enabled":false}"#);
        assert_eq!(status, 200);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["enabled"], false);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["keep_awake"]["enabled"], false);
        assert_eq!(t.request("POST", "/api/keep_awake", r#"{}"#).0, 400);
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
    }

    #[test]
    fn the_stall_hook_exists_only_with_test_hooks() {
        let t = TestServer::start(false);
        assert_eq!(t.request("POST", "/api/test/stall?ms=200", "").0, 404);
        assert_eq!(t.shared.lock().unwrap().test_stall_ms, 0);
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/stall?ms=99999", "").0, 200);
        assert_eq!(t.shared.lock().unwrap().test_stall_ms, 2_000, "bounded");
    }

    #[test]
    fn the_tempo_estimate_hook_exists_only_with_test_hooks() {
        let t = TestServer::start(false);
        let est = r#"{"bpm":128,"confidence":0.9,"state":"locked","offset_s":0.25}"#;
        assert_eq!(t.request("POST", "/api/test/tempo_estimate", est).0, 404);
        assert!(t.shared.lock().unwrap().audio_in.snapshot().is_none());
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/tempo_estimate", est).0, 200);
        let snap = t.shared.lock().unwrap().audio_in.snapshot().unwrap();
        assert_eq!((snap.tempo.bpm, snap.tempo.state), (128.0, crate::audio::bpm::DetectState::Locked));
        let n = (snap.tempo.beat_time - 0.25) / (60.0 / 128.0);
        assert!((n - n.round()).abs() < 1e-9 && snap.tempo.beat_time <= snap.t);
    }

    #[test]
    fn the_native_audio_hook_exists_only_with_test_hooks() {
        let t = TestServer::start(false);
        let sim = r#"{"state":"running","device":"Micro test","devices":[{"name":"Micro test","is_default":true}],
            "snapshot":{"features":{"bands":{"sub":0.9,"bass":0.8,"low_mid":0.1,"mid":0.2,"high":0.3},"kick":5,"snare":2,"hat":7,
            "bpm":128,"bpm_confidence":0.8,"section":"buildup","buildup":0.4,"drop":1},
            "level_db":-18,"peak_db":-6,"spectrum_db":[-30,-40],"tempo_state":"locked","section_since_s":3.5}}"#;
        assert_eq!(t.request("POST", "/api/test/native_audio", sim).0, 404);
        assert!(t.shared.lock().unwrap().audio_in.snapshot().is_none());
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/native_audio", sim).0, 200);
        assert_eq!(t.request("POST", "/api/audio/config", r#"{"source":"native"}"#).0, 200);
        // The panel's light endpoint: /api/state.audio alone.
        let a: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/audio/state", "").1).unwrap();
        assert_eq!((a["state"].as_str(), a["capturing"].as_str(), a["active"].as_str()), (Some("running"), Some("Micro test"), Some("native")));
        assert_eq!((a["level_db"].as_f64(), a["peak_db"].as_f64()), (Some(-18.0), Some(-6.0)));
        assert_eq!(a["features"]["level_db"].as_f64(), Some(-18.0));
        assert_eq!((a["counters"]["kick"].as_u64(), a["counters"]["hat"].as_u64(), a["counters"]["drop"].as_u64()), (Some(5), Some(7), Some(1)));
        assert_eq!((a["tempo"]["state"].as_str(), a["tempo"]["bpm"].as_f64()), (Some("locked"), Some(128.0)));
        assert_eq!((a["section"].as_str(), a["sections"]["since_s"].as_f64()), (Some("buildup"), Some(3.5)));
        let devices: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/audio/devices", "").1).unwrap();
        assert_eq!(devices["devices"][0]["name"], "Micro test");
        let sp: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/audio/spectrum", "").1).unwrap();
        assert_eq!((sp["db"][0].as_f64(), sp["db"][1].as_f64(), sp["db"][63].as_f64()), (Some(-30.0), Some(-40.0), Some(-120.0)));
        // Only the state: the snapshot is kept, the device too.
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"state":"permission_denied","message":"refusé"}"#).0, 200);
        let st = t.shared.lock().unwrap().audio_in.status();
        assert_eq!((st.state, st.device.as_deref()), (crate::audio::CaptureState::PermissionDenied, Some("Micro test")));
        // Bad bodies are refused.
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"state":"loud"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"snapshot":{"features":{"section":"chorus"}}}"#).0, 400);
        // Never arms anything.
        assert_eq!(arm_status(&t)["armed"], false);
    }

    #[test]
    fn only_post_estop_takes_the_fast_path() {
        assert!(is_estop(&Method::Post, "/api/estop"));
        assert!(is_estop(&Method::Post, "/api/estop?source=keyboard"));
        assert!(!is_estop(&Method::Get, "/api/estop"));
        assert!(!is_estop(&Method::Post, "/api/estop/reset"));
    }

    /// T-298: only the requests that may decode a song leave the worker
    /// (heartbeats, arm and the rest stay in order on it).
    #[test]
    fn only_decoding_requests_go_aside() {
        assert!(may_decode(&Method::Post, "/api/media/audio?name=a.mp3"));
        assert!(may_decode(&Method::Get, "/api/timeline/waveform?from=0&to=1"));
        assert!(may_decode(&Method::Post, "/api/timeline/audio"));
        assert!(may_decode(&Method::Post, "/api/media/ilda?name=a.ild"));
        assert!(may_decode(&Method::Get, "/api/media/ilda/export?number=1"));
        assert!(may_decode(&Method::Post, "/api/sdbank/export"));
        assert!(!may_decode(&Method::Post, "/api/sdbank"));
        assert!(!may_decode(&Method::Get, "/api/media/ilda"));
        assert!(!may_decode(&Method::Post, "/api/media/ilda/delete"));
        for (m, p) in [(Method::Get, "/api/media/audio"), (Method::Post, "/api/heartbeat"), (Method::Post, "/api/arm"), (Method::Get, "/api/state"), (Method::Post, "/api/timeline/play")] {
            assert!(!may_decode(&m, p), "{p}");
        }
    }

    #[test]
    fn query_numbers() {
        assert_eq!(query_u64("/api/settings?rev=12", "rev"), Some(12));
        assert_eq!(query_u64("/api/settings?x=1&rev=3", "rev"), Some(3));
        assert_eq!(query_u64("/api/settings?revision=3", "rev"), None);
        assert_eq!(query_u64("/api/settings", "rev"), None);
    }

    /// T-303: the DAC list is read-only. Nothing is scanned until asked, a
    /// scan reports what it found, and none of it touches the arm state.
    #[test]
    fn device_list_scans_only_on_request_and_never_arms() {
        let t = TestServer::start(false);
        let fake = DeviceInfo { kind: "helios".into(), id: "helios:left".into(), name: "Helios DAC Left (1:2.3)".into() };
        t.shared.lock().unwrap().devices = Arc::new(DeviceList::fixed(vec![fake]));
        let get = || -> serde_json::Value { serde_json::from_str(&t.request("GET", "/api/devices", "").1).unwrap() };
        let v = get();
        assert_eq!((v["devices"].as_array().map(Vec::len), v["scanned"].as_bool()), (Some(0), Some(false)), "{v}");
        assert_eq!(t.request("POST", "/api/devices/scan", "").0, 200);
        let deadline = Instant::now() + Duration::from_secs(5);
        let v = loop {
            let v = get();
            if v["scanned"] == true {
                break v;
            }
            assert!(Instant::now() < deadline, "scan never finished");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(v["devices"][0], json!({ "kind": "helios", "id": "helios:left", "name": "Helios DAC Left (1:2.3)" }));
        assert_eq!(v["scanning"], false);
        assert_eq!(arm_status(&t)["armed"], false);
        let s = t.shared.lock().unwrap();
        assert_eq!((s.output_name.as_deref(), s.output_kind), (None, None), "still preview only");
    }

    /// A test server whose output manager knows one fake device, scanned.
    /// No manager thread and no engine: a selection stays « connecting ».
    fn with_fake_outputs(t: &TestServer) -> Arc<crate::output_manager::OutputManager> {
        use crate::output_manager::{FakeBackend, OutputManager};
        let dev = DeviceInfo { kind: "etherdream".into(), id: "etherdream:a".into(), name: "Ether Dream a".into() };
        let backend = FakeBackend::new(vec![dev.clone()], None, None, std::env::temp_dir());
        let list = Arc::new(DeviceList::fixed(vec![dev]));
        let mgr = Arc::new(OutputManager::new(Arc::new(backend), Arc::clone(&list), &Default::default(), 30_000));
        let mut s = t.shared.lock().unwrap();
        s.devices = list;
        s.output_mgr = Arc::clone(&mgr);
        mgr
    }

    fn outputs(t: &TestServer) -> serde_json::Value {
        serde_json::from_str(&t.request("GET", "/api/outputs", "").1).unwrap()
    }

    /// T-401: listing, scanning, choosing and disconnecting an output over
    /// the API; every choice disarms with « Changement de sortie ».
    #[test]
    fn output_routes_list_select_and_disarm() {
        let t = TestServer::start(false);
        let mgr = with_fake_outputs(&t);
        let v = outputs(&t);
        assert_eq!((v["devices"].as_array().map(Vec::len), v["state"].as_str(), v["selected"].clone()), (Some(0), Some("available"), json!(null)), "{v}");
        assert_eq!(v["auto_reconnect"], true);
        assert_eq!(t.request("POST", "/api/outputs/scan", "").0, 200);
        let deadline = Instant::now() + Duration::from_secs(5);
        while outputs(&t)["scanned"] != true {
            assert!(Instant::now() < deadline, "scan never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(outputs(&t)["devices"][0], json!({ "id": "etherdream:a", "name": "Ether Dream a", "kind": "etherdream", "state": "available", "error": null }));

        assert_eq!(t.request("POST", "/api/outputs/select", r#"{"id":"helios:left"}"#).0, 400, "unknown id");
        assert_eq!(t.request("POST", "/api/outputs/select", "{}").0, 400, "the id is required");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, body) = t.request("POST", "/api/outputs/select", r#"{"id":"etherdream:a"}"#);
        assert_eq!(code, 200, "{body}");
        let st = arm_status(&t);
        assert_eq!((st["armed"].as_bool(), st["last_disarm"]["reason"].as_str()), (Some(false), Some("output_changed")));
        assert_eq!(st["last_disarm"]["reason_fr"], "Changement de sortie");
        assert_eq!(st["last_disarm"]["source"], "ui");
        let v = outputs(&t);
        assert_eq!((v["selected"].as_str(), v["state"].as_str()), (Some("etherdream:a"), Some("connecting")));
        assert_eq!(v["devices"][0]["state"], "connecting");
        // Until the engine installs it, arming waits.
        let (code, body) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(code, 409);
        assert!(body.contains("Aucune sortie connectée"), "{body}");
        assert_eq!(mgr.prefs().selected.as_deref(), Some("etherdream:a"), "remembered");

        assert_eq!(t.request("POST", "/api/outputs/select", r#"{"id":null}"#).0, 200);
        let v = outputs(&t);
        assert_eq!((v["selected"].clone(), v["state"].as_str()), (json!(null), Some("available")));
        assert_eq!(mgr.prefs().selected, None);

        assert_eq!(t.request("POST", "/api/outputs/config", r#"{"auto_reconnect":false}"#).0, 200);
        assert_eq!(outputs(&t)["auto_reconnect"], false);
        assert!(!mgr.prefs().auto_reconnect);
        // The frame says where the output is.
        let f: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(f["output_state"], "available");
    }

    /// T-301: the ShowNET SD card box is off until an address is typed,
    /// then listed; bad values and broadcasts are refused, and nothing
    /// changes while it is the selected output.
    #[test]
    fn the_artnet_box_is_typed_listed_and_locked_while_selected() {
        let t = TestServer::start(false);
        let v: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/outputs/artnet", "").1).unwrap();
        assert_eq!((v["id"].clone(), v["config"]["target"].clone(), v["port"].as_u64()), (json!(null), json!(null), Some(6454)));
        assert!(outputs(&t)["devices"].as_array().unwrap().iter().all(|d| d["kind"] != "artnet"), "off by default");
        for bad in [r#"{"target":"255.255.255.255"}"#, r#"{"target":"shownet.local"}"#, r#"{"target":"127.0.0.1","dmx_address":0}"#, r#"{"target":"127.0.0.1","universe":40000}"#] {
            assert_eq!(t.request("POST", "/api/outputs/artnet", bad).0, 400, "{bad}");
        }
        let (code, body) = t.request("POST", "/api/outputs/artnet", r#"{"target":"127.0.0.1:7000","universe":2,"dmx_address":20,"profile":"pro"}"#);
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "artnet:127.0.0.1:7000");
        assert_eq!((v["config"]["universe"].as_u64(), v["config"]["profile"].as_str()), (Some(2), Some("pro")));
        let listed = outputs(&t)["devices"].as_array().unwrap().iter().find(|d| d["kind"] == "artnet").cloned().unwrap();
        assert_eq!((listed["id"].as_str(), listed["name"].as_str()), (Some("artnet:127.0.0.1:7000"), Some("ShowNET carte SD (Art-Net 127.0.0.1:7000)")));
        assert_eq!(t.request("POST", "/api/outputs/select", r#"{"id":"artnet:127.0.0.1:7000"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/artnet", r#"{"target":"127.0.0.1:7001"}"#).0, 409);
        assert_eq!(t.request("POST", "/api/outputs/select", r#"{"id":null}"#).0, 200);
        let (code, body) = t.request("POST", "/api/outputs/artnet", r#"{"target":""}"#);
        assert_eq!((code, serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"].clone()), (200, json!(null)), "emptied: off");
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/outputs/artnet", r#"{"target":"127.0.0.1"}"#).0, 423);
    }

    /// T-401: with the `output` interlock open (nothing connected), arming
    /// is refused with « Aucune sortie connectée ».
    #[test]
    fn arming_without_an_output_is_refused() {
        let t = TestServer::start(false);
        t.shared.lock().unwrap().gate.register(crate::interlock::OUTPUT, crate::interlock::OUTPUT_LABEL, false);
        let (code, body) = t.request("POST", "/api/arm", r#"{"on":true,"source":"keyboard"}"#);
        assert_eq!(code, 409);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["blocking"], json!(["Aucune sortie connectée"]));
        assert_eq!(arm_status(&t)["armed"], false);
    }

    /// The unplug hook exists only with `--test-hooks` and a fake backend.
    #[test]
    fn the_output_test_hook_needs_test_hooks() {
        let t = TestServer::start(false);
        with_fake_outputs(&t);
        assert_eq!(t.request("POST", "/api/test/output", r#"{"action":"drop"}"#).0, 404);
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/output", r#"{"action":"drop"}"#).0, 404, "no fake switch");
        let dropped = Arc::new(AtomicBool::new(false));
        {
            let mut s = t.shared.lock().unwrap();
            let backend = crate::output_manager::FakeBackend::new(Vec::new(), None, None, std::env::temp_dir());
            let mgr = crate::output_manager::OutputManager::new(Arc::new(backend), Arc::new(DeviceList::fixed(vec![])), &Default::default(), 30_000);
            s.output_mgr = Arc::new(mgr.with_test_drop(Arc::clone(&dropped)));
        }
        assert_eq!(t.request("POST", "/api/test/output", r#"{"action":"nope"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/test/output", r#"{"action":"drop"}"#).0, 200);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(t.request("POST", "/api/test/output", r#"{"action":"restore"}"#).0, 200);
        assert!(!dropped.load(Ordering::SeqCst));
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
    fn timeline_api_saves_loads_and_drives_a_show_without_arming() {
        let t = TestServer::start(false);
        let show = r#"{"name":"api show","time_base":"beats","tracks":[{"layer":2,"events":[{"start":0,"len":8,"source":{"kind":"cue","id":"x"}}]}]}"#;
        assert_eq!(t.request("POST", "/api/shows", show).0, 200);
        assert_eq!(t.request("POST", "/api/shows", r#"{"name":"../evil"}"#).0, 400);
        let list: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/shows", "").1).unwrap();
        assert_eq!(list[0]["name"], "api show");
        assert_eq!(list[0]["length"], 8.0);

        assert_eq!(t.request("POST", "/api/timeline/play", "").0, 409, "nothing loaded yet");
        assert_eq!(t.request("POST", "/api/timeline/load", r#"{"name":"nope"}"#).0, 404);
        assert_eq!(t.request("POST", "/api/timeline/load", r#"{"name":"api show"}"#).0, 200);
        let (status, body) = t.request("POST", "/api/timeline/play", "");
        assert_eq!(status, 200);
        let st: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((st["name"].as_str(), st["playing"].as_bool(), st["time_base"].as_str()), (Some("api show"), Some(true), Some("beats")));
        let (_, body) = t.request("POST", "/api/timeline/loop", r#"{"on":true,"region":[2,4]}"#);
        let st: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((st["loop"].as_bool(), st["loop_region"].clone()), (Some(true), json!([2.0, 4.0])));
        let (_, body) = t.request("POST", "/api/timeline/loop", r#"{"region":null}"#);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["loop_region"], serde_json::Value::Null);
        assert_eq!(t.request("POST", "/api/timeline/seek", r#"{"position":3}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/seek", "{}").0, 400);
        let state: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert_eq!(state["timeline"]["name"], "api show");
        assert_eq!(state["armed"], false, "the timeline never arms");

        // Échap: the e-stop halts the show's output, and play is refused until reset.
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert_eq!(t.request("POST", "/api/timeline/play", "").0, 409);
        assert_eq!(t.request("POST", "/api/timeline/stop", "").0, 200);
        assert!(!t.shared.lock().unwrap().timeline.is_playing());
    }

    /// T-162: the editor's routes. New show, edits validated and applied
    /// while playing (never arming), save and save-as, names confined.
    #[test]
    fn timeline_editor_routes() {
        let t = TestServer::start(false);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let json = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap();
        assert_eq!(t.request("POST", "/api/timeline/edit", r#"{"show":{}}"#).0, 409, "no show loaded");
        assert_eq!(t.request("POST", "/api/timeline/save", "{}").0, 409);
        assert_eq!(t.request("POST", "/api/timeline/new", r#"{"name":"../x"}"#).0, 400);
        let (status, body) = t.request("POST", "/api/timeline/new", r#"{"name":"Editeur web","time_base":"seconds"}"#);
        assert_eq!(status, 200, "{body}");
        assert_eq!(json(&body)["name"], "Editeur web");
        assert_eq!(t.request("POST", "/api/timeline/new", r#"{"name":"Editeur web"}"#).0, 409, "exists already");
        assert!(t.shared.lock().unwrap().shows.load("Editeur web").is_ok(), "created on disk");

        let (_, body) = t.request("GET", "/api/timeline", "");
        let mut show = json(&body)["show"].clone();
        assert_eq!(show["tracks"].as_array().unwrap().len(), 2);
        show["tracks"][0]["events"] = json!([{ "id": 0, "start": 2.0, "len": 2.0, "source": { "kind": "cue", "id": cue } }]);
        let bad = json!({ "show": { "tracks": [{ "events": [{ "start": 0, "len": 1, "source": { "kind": "cue", "id": "nope" } }] }] } });
        let (status, body) = t.request("POST", "/api/timeline/edit", &bad.to_string());
        assert_eq!((status, body.contains("cue inconnu")), (400, true));
        assert_eq!(t.request("POST", "/api/timeline/play", "").0, 200);
        let (status, body) = t.request("POST", "/api/timeline/edit", &json!({ "show": show }).to_string());
        assert_eq!(status, 200, "{body}");
        let r = json(&body);
        assert_eq!((r["state"]["playing"].as_bool(), r["state"]["modified"].as_bool()), (Some(true), Some(true)));
        assert!(r["show"]["tracks"][0]["events"][0]["id"].as_u64().unwrap() > 0, "ids given by the server");
        assert_eq!(t.shared.lock().unwrap().shows.load("Editeur web").unwrap().end(), 0.0, "not saved yet");

        let (status, body) = t.request("POST", "/api/timeline/save", "{}");
        assert_eq!((status, json(&body)["modified"].as_bool()), (200, Some(false)));
        assert_eq!(t.shared.lock().unwrap().shows.load("Editeur web").unwrap().end(), 4.0);
        assert_eq!(t.request("POST", "/api/timeline/save", r#"{"name":"../../evil"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/timeline/save", r#"{"name":"Editeur copie"}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().shows.load("Editeur copie").unwrap().end(), 4.0);
        let state = json(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["timeline"]["name"], "Editeur copie");
        assert_eq!(state["armed"], false, "editing and saving never arm");
        t.request("POST", "/api/timeline/stop", "");
    }

    /// Like `http`, with a binary body.
    fn http_bytes(addr: SocketAddr, path: &str, body: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let status = raw.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
    }

    /// T-011: an ILDA file generated by our own writer is imported, shows
    /// up as a cue, plays through the look path without arming, exports
    /// back as a valid format 5 file and is deleted.
    #[test]
    fn ilda_files_are_imported_played_exported_and_deleted_without_arming() {
        use crate::ilda::{self, IldaFrame};
        use crate::patterns::Point;
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        let dir = t.shared.lock().unwrap().ilda.dir().to_path_buf();
        let frames: Vec<IldaFrame> = (0..3)
            .map(|i| IldaFrame::new("t", vec![Point::blanked(0.0, 0.0), Point::lit(0.1 * i as f32, 0.5, 0.0, 1.0, 0.0)]))
            .collect();
        let file = ilda::write_format5(&frames).unwrap();

        // Damaged, empty, wrong extension, no name: refused, nothing stored.
        let (status, body) = http_bytes(t.addr, "/api/media/ilda?name=webtest%20abime.ild", b"ILDA\0\0\0\x05abc");
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("ILDA"), "{body}");
        assert_eq!(http_bytes(t.addr, "/api/media/ilda?name=webtest.ild", b"").0, 400);
        assert_eq!(http_bytes(t.addr, "/api/media/ilda?name=webtest.txt", &file).0, 400);
        assert_eq!(http_bytes(t.addr, "/api/media/ilda", &file).0, 400);
        assert!(json(&t.request("GET", "/api/media/ilda", "").1)["files"].as_array().unwrap().is_empty());

        // A path in the name: only the file name is kept, inside media/ilda/.
        let (status, body) = http_bytes(t.addr, "/api/media/ilda?name=..%2F..%2Fwebtest%20logo.ild", &file);
        assert_eq!(status, 200, "{body}");
        let got = json(&body);
        assert_eq!((got["file"].as_str(), got["frames"].as_u64(), got["existing"].as_bool()), (Some("webtest logo.ild"), Some(3), Some(false)));
        assert!(dir.join("webtest logo.ild").is_file());
        let list = json(&t.request("GET", "/api/media/ilda", "").1);
        assert_eq!(list["files"][0]["id"], "ilda:webtest logo.ild");
        let presets = json(&t.request("GET", "/api/presets", "").1);
        assert!(presets["categories"].as_array().unwrap().iter().any(|c| c == "ILDA"));
        let cue = presets["presets"].as_array().unwrap().iter().find(|p| p["id"] == "ilda:webtest logo.ild").cloned().unwrap();
        assert_eq!((cue["name"].as_str(), cue["category"].as_str()), (Some("webtest logo"), Some("ILDA")));

        // Played as a cue: the look path, never armed.
        assert_eq!(t.request("POST", "/api/presets/play", r#"{"id":"ilda:webtest logo.ild"}"#).0, 200);
        let state = json(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["settings"]["content"], json!({ "kind": "ilda", "file": "webtest logo.ild", "fps": 25.0 }));
        assert_eq!(arm_status(&t)["armed"], false);

        // Export: the look on show, named for the SD card, read back by T-001.
        let (head, bytes) = get_bytes(t.addr, "/api/media/ilda/export?number=7&frames=5&fps=25");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("filename=\"007.ild\""), "{head}");
        let back = ilda::read(&bytes).unwrap();
        assert_eq!(back.len(), 5);
        assert!(back.iter().all(|f| f.points.iter().any(|p| p.is_lit())));
        let (_, bytes) = get_bytes(t.addr, "/api/media/ilda/export?number=229&frames=2&fps=10&id=tunnels-001");
        assert_eq!(ilda::read(&bytes).unwrap().len(), 2);
        for bad in ["number=0", "number=230", "number=1.5", "number=x", "frames=0", "frames=99999", "fps=0", "id=nope"] {
            let (head, _) = get_bytes(t.addr, &format!("/api/media/ilda/export?{bad}"));
            assert!(head.starts_with("HTTP/1.1 400") || head.starts_with("HTTP/1.1 404"), "{bad}: {head}");
        }

        // Deleted: gone from the folder, the list and the cue page.
        assert_eq!(t.request("POST", "/api/media/ilda/delete", r#"{"name":"../x.ild"}"#).0, 404);
        assert_eq!(t.request("POST", "/api/media/ilda/delete", r#"{"name":"webtest logo.ild"}"#).0, 200);
        assert!(!dir.join("webtest logo.ild").exists());
        let presets = json(&t.request("GET", "/api/presets", "").1);
        assert!(!presets["presets"].as_array().unwrap().iter().any(|p| p["id"] == "ilda:webtest logo.ild"));
        assert_eq!(arm_status(&t)["armed"], false);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    /// The SD bank (T-302): cues added with automatic numbers, renumbered,
    /// exported to a folder with the safety chain (T-355), locked in show
    /// mode, never arming anything.
    #[test]
    fn the_sd_bank_is_edited_and_exported_to_a_folder_without_arming() {
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        let empty = json(&t.request("GET", "/api/sdbank", "").1);
        assert_eq!(empty["slots"], json!([]));
        assert_eq!(empty["outputs"][0]["id"], "main");
        // Nothing playing: « the current cue » is refused.
        assert_eq!(t.request("POST", "/api/sdbank/add", "{}").0, 400);
        assert_eq!(t.request("POST", "/api/sdbank/add", r#"{"id":"nope"}"#).0, 404);
        assert_eq!(t.request("POST", "/api/presets/play", r#"{"id":"tunnels-001"}"#).0, 200);
        let (status, body) = t.request("POST", "/api/sdbank/add", "{}");
        assert_eq!(status, 200, "{body}");
        assert_eq!(json(&body)["added"], 1);
        let added = json(&t.request("POST", "/api/sdbank/add", r#"{"id":"tunnels-002","seconds":0.5}"#).1);
        assert_eq!(added["added"], 2);
        assert_eq!(added["slots"][0]["cue_id"], "tunnels-001");
        assert!(added["slots"][0]["name"].is_string());
        // Reserved numbers and doubles refused; a hole closed by renumbering.
        for bad in [r#"{"slots":[{"number":0,"cue_id":"a"}]}"#, r#"{"slots":[{"number":230,"cue_id":"a"}]}"#, r#"{"slots":[{"number":4,"cue_id":"a"},{"number":4,"cue_id":"b"}]}"#] {
            assert_eq!(t.request("POST", "/api/sdbank", bad).0, 400, "{bad}");
        }
        let set = r#"{"slots":[{"number":9,"cue_id":"tunnels-002","seconds":0.5},{"number":3,"cue_id":"tunnels-001","seconds":0.3}]}"#;
        assert_eq!(t.request("POST", "/api/sdbank", set).0, 200);
        let renumbered = json(&t.request("POST", "/api/sdbank/renumber", "").1);
        assert_eq!(renumbered["slots"][0]["number"], 1);
        assert_eq!(renumbered["slots"][1]["number"], 2);
        assert_eq!(renumbered["slots"][1]["file"], "002.ild");

        let dir = std::env::temp_dir().join(format!("laser-studio-web-sdbank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(t.request("POST", "/api/sdbank/export", r#"{"dir":"relatif"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/sdbank/export", &json!({ "dir": dir, "output": "ailleurs" }).to_string()).0, 400);
        let (status, body) = t.request("POST", "/api/sdbank/export", &json!({ "dir": dir }).to_string());
        assert_eq!(status, 200, "{body}");
        let report = json(&body);
        assert_eq!(report["files"].as_array().unwrap().len(), 2);
        assert_eq!(report["output"], "main");
        assert_eq!(crate::ilda::read(&std::fs::read(dir.join("001.ild")).unwrap()).unwrap().len(), 15);
        assert!(dir.join("002.ild").exists() && dir.join("banque.json").exists());

        // Show mode: the bank can't be edited or exported, only looked at.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/sdbank/renumber", "").0, 423);
        assert_eq!(t.request("POST", "/api/sdbank/export", &json!({ "dir": dir }).to_string()).0, 423);
        assert_eq!(t.request("GET", "/api/sdbank", "").0, 200);
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(arm_status(&t)["armed"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn songs_are_imported_in_their_folder_and_attached_to_the_loaded_show() {
        use crate::audio::decode::testing::{sine, wav16};
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        let media_dir = t.shared.lock().unwrap().media.dir().to_path_buf();

        // A damaged file: a clear 400, nothing written.
        let (status, body) = http_bytes(t.addr, "/api/media/audio?name=cass%C3%A9%20web.wav", b"RIFF\x10\0\0\0WAVEjunk");
        assert_eq!(status, 400);
        assert!(body.contains("WAV illisible"), "{body}");
        assert!(!media_dir.join("cassé web.wav").exists());
        assert_eq!(http_bytes(t.addr, "/api/media/audio", b"x").0, 400, "no name");
        assert_eq!(http_bytes(t.addr, "/api/media/audio?name=notes.txt", b"x").0, 400);

        // A path in the name: only the file name is kept, inside media/audio/.
        let wav = wav16(8_000, 1, &sine(8_000, 2.0, 100.0, 0.5));
        let (status, body) = http_bytes(t.addr, "/api/media/audio?name=..%2F..%2FChanson%20web.wav", &wav);
        assert_eq!(status, 200, "{body}");
        assert_eq!(json(&body)["file"], "Chanson web.wav");
        assert_eq!(json(&body)["duration_s"], 2.0);
        assert!(media_dir.join("Chanson web.wav").is_file());
        assert!(!media_dir.join("../../Chanson web.wav").exists());
        let list = json(&t.request("GET", "/api/media/audio", "").1);
        assert!(list["files"].as_array().unwrap().iter().any(|f| f["file"] == "Chanson web.wav"));

        let song = r#"{"file":"Chanson web.wav","offset_s":0.9,"gain":2}"#;
        assert_eq!(t.request("POST", "/api/timeline/audio", song).0, 409, "no show loaded");
        let show = r#"{"name":"web song","time_base":"seconds","tracks":[{"events":[{"id":1,"start":0,"len":1,"source":{"kind":"cue","id":"x"}}]}]}"#;
        assert_eq!(t.request("POST", "/api/shows", show).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/load", r#"{"name":"web song"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/audio", r#"{"gain":0.5}"#).0, 400, "no song yet");
        assert_eq!(t.request("POST", "/api/timeline/audio", r#"{"file":"nope.wav"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/timeline/audio", r#"{"file":"../Chanson web.wav"}"#).0, 400);
        let (status, body) = t.request("POST", "/api/timeline/audio", song);
        assert_eq!(status, 200, "{body}");
        let got = json(&body);
        assert_eq!(got["saved"], true);
        let audio = &got["state"]["audio"];
        assert_eq!((audio["offset_s"].as_f64(), audio["gain"].as_f64(), audio["duration_s"].as_f64()), (Some(0.5), Some(1.0), Some(2.0)), "clamped");
        assert_eq!(got["state"]["length"], 2.5, "the show lasts until the song ends");
        assert_eq!(got["audio"]["state"], "disabled", "no playback thread in tests");
        // Saved with the show.
        let saved = t.shared.lock().unwrap().shows.load("web song").unwrap();
        assert_eq!(saved.audio.unwrap().file, "Chanson web.wav");

        // Waveform in show time: the song starts 0.5 s in.
        let wave = json(&t.request("GET", "/api/timeline/waveform?px=5", "").1);
        assert_eq!((wave["from"].as_f64(), wave["to"].as_f64(), wave["px"].as_u64()), (Some(0.0), Some(2.5), Some(5)));
        let max: Vec<f64> = wave["max"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(max[0], 0.0, "before the song");
        assert!(max[1..].iter().all(|m| (m - 0.5).abs() < 0.01), "{max:?}");
        let zoom = json(&t.request("GET", "/api/timeline/waveform?from=1&to=1.5&px=3", "").1);
        assert_eq!(zoom["min"].as_array().unwrap().len(), 3);
        let by_file = json(&t.request("GET", "/api/timeline/waveform?file=Chanson%20web.wav&px=2", "").1);
        assert_eq!((by_file["offset_s"].as_f64(), by_file["to"].as_f64()), (Some(0.0), Some(2.5)));
        assert_eq!(t.request("GET", "/api/timeline/waveform?file=..%2Fsecret.wav", "").0, 404);

        let state = json(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["timeline_audio"]["clock"], "system");
        assert_eq!(state["armed"], false);
        let (_, body) = t.request("POST", "/api/timeline/audio", r#"{"file":null}"#);
        assert!(json(&body)["state"]["audio"].is_null());
        assert_eq!(t.request("GET", "/api/timeline/waveform", "").0, 404, "no song any more");
    }

    #[test]
    fn query_strings_are_percent_decoded() {
        assert_eq!(query_str("/x?name=Mon%20Titre%C3%A9+2.wav&a=1", "name").as_deref(), Some("Mon Titreé 2.wav"));
        assert_eq!(query_str("/x?a=1", "name"), None);
        assert_eq!(query_str("/x?name=%zz", "name"), None);
        assert_eq!(query_str("/x?name=%4", "name"), None);
        assert_eq!(query_str("/x?name=%FF", "name"), None, "not UTF-8");
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
    fn power_caps_lower_at_once_and_rise_only_confirmed_and_disarmed() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let get = |t: &TestServer| js(&t.request("GET", "/api/outputs/limits", "").1);
        let g = get(&t);
        assert_eq!(g["active"], "main");
        assert_eq!(g["outputs"][0]["limits"]["max_power"], 0.5);
        assert_eq!(g["outputs"][0]["projector"]["class"], "4");
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["max_brightness_effective"], 0.5);
        // Armed: lowering is applied, raising refused even confirmed.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, reply) = t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":0.3,"max_color":[1,0.2,1]}}"#);
        assert_eq!(code, 200, "{reply}");
        assert_eq!(t.shared.lock().unwrap().outputs.active_limits().max_color[1], 0.2);
        let (code, reply) = t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":0.6,"max_color":[1,0.2,1]},"confirm_loosen":true}"#);
        assert_eq!(code, 409);
        assert_eq!(js(&reply)["armed"], true);
        // Disarmed: asks, then applies on confirmation.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        let (code, reply) = t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":0.6,"max_color":[1,0.2,1]}}"#);
        assert_eq!(code, 409);
        assert!(reply.contains("30 % → 60 %"), "{reply}");
        assert_eq!(get(&t)["outputs"][0]["limits"]["max_power"].as_f64().unwrap() as f32, 0.3);
        let (code, _) = t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":0.6,"max_color":[1,0.2,1]},"confirm_loosen":true}"#);
        assert_eq!(code, 200);
        assert_eq!(get(&t)["max_brightness_effective"].as_f64().unwrap() as f32, 0.6);
        // The sheet alone: no confirmation; bad values: 400.
        let (code, _) = t.request("POST", "/api/outputs/limits", r#"{"projector":{"name":"Proj","class":"3B","power_mw":[100,50,200]}}"#);
        assert_eq!(code, 200);
        assert_eq!(get(&t)["outputs"][0]["projector"]["name"], "Proj");
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"projector":{"class":"7"}}"#).0, 400);
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":1.5}}"#).0, 400);
        // A look carrying a cap-looking field changes nothing.
        t.request("POST", "/api/settings", r#"{"brightness":1.0,"max_power":1.0}"#);
        assert_eq!(get(&t)["max_brightness_effective"].as_f64().unwrap() as f32, 0.6);
    }

    /// T-414: darker applies at once, brighter needs the confirmation
    /// (armed or not: the caps apply after anyway), bad values are 400.
    #[test]
    fn output_colour_darker_at_once_brighter_only_confirmed() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let g = js(&t.request("GET", "/api/outputs/color", "").1);
        assert_eq!((g["output"].as_str(), g["identity"].as_bool()), (Some("main"), Some(true)));
        assert_eq!(g["color"]["curve"][0]["type"], "gamma");
        assert!(g["library_delay_points"].as_u64().unwrap() > 0, "laser-dac's own delay is shown");
        assert_eq!(js(&t.request("GET", "/api/outputs/color?output=0", "").1)["output"], "main");
        assert_eq!(t.request("GET", "/api/outputs/color?output=3", "").0, 400);
        assert_eq!(t.request("GET", "/api/outputs/color?output=nope", "").0, 400);
        // Darker (gain down, gamma up, a delay): applied.
        let curves = r#""curve":[{"type":"gamma","gamma":1},{"type":"gamma","gamma":2.2},{"type":"gamma","gamma":1}],"color_delay_points":2"#;
        let (code, reply) = t.request("POST", "/api/outputs/color", &format!(r#"{{"color":{{"white_balance":[0.8,1,1],{curves}}}}}"#));
        assert_eq!(code, 200, "{reply}");
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().color_delay_points, 2);
        // Brighter: 409 with the list, unchanged; confirmed: applied.
        let brighter = format!(r#"{{"color":{{"white_balance":[1,1,1],{curves}}}}}"#);
        let (code, reply) = t.request("POST", "/api/outputs/color", &brighter);
        assert_eq!(code, 409, "{reply}");
        assert_eq!(js(&reply)["loosen"][0], "Balance rouge : 80 % → 100 %");
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().white_balance[0], 0.8);
        let confirmed = format!(r#"{{"confirm_loosen":true,"color":{{"white_balance":[1,1,1],{curves}}}}}"#);
        // Armed: brighter refused even confirmed, darker still applies.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, reply) = t.request("POST", "/api/outputs/color", &confirmed);
        assert_eq!(code, 409, "{reply}");
        assert_eq!((js(&reply)["armed"].as_bool(), js(&reply)["error"].as_str()), (Some(true), Some("Désarmez avant d'augmenter les couleurs")));
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().white_balance[0], 0.8);
        let dimmer = format!(r#"{{"color":{{"white_balance":[0.6,1,1],{curves}}}}}"#);
        assert_eq!(t.request("POST", "/api/outputs/color", &dimmer).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().white_balance[0], 0.6);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        // Disarmed and confirmed: applied.
        assert_eq!(t.request("POST", "/api/outputs/color", &confirmed).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().white_balance[0], 1.0);
        // Out of range or malformed: 400, nothing changed.
        for bad in [
            r#"{"color":{"white_balance":[1.5,1,1]}}"#,
            r#"{"color":{"color_delay_points":9}}"#,
            r#"{"color":{"curve":[{"type":"gamma","gamma":0.1},{"type":"gamma","gamma":1},{"type":"gamma","gamma":1}]}}"#,
            r#"{"color":{"curve":[{"type":"spline"},{"type":"gamma","gamma":1},{"type":"gamma","gamma":1}]}}"#,
            r#"{}"#,
        ] {
            assert_eq!(t.request("POST", "/api/outputs/color", bad).0, 400, "{bad}");
        }
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().color_delay_points, 2);
        // The caps and the sheet are untouched by it, and it by them.
        assert_eq!(t.shared.lock().unwrap().outputs.active_limits(), crate::power::OutputLimits::default());
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"projector":{"name":"P"}}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active_color().color_delay_points, 2);
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
        assert_eq!(st["settings"]["horizon"]["y"], 0.25, "T-101's beam_floor_y is the horizon");
        assert_eq!(st["settings"]["strobe_burst_s"], 5.0, "missing fields keep the safe default");
        assert_eq!(st["status"]["active"], false);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["strobe"]["active"], false);

        // A zone is added at once and gets an id; dropping it needs the
        // operator's confirmation.
        let zone = r#"{"strobe_max_hz":3,"horizon":{"y":0.25},"zones":[{"name":"Public","points":[[-1,-1],[1,-1],[1,-0.4],[-1,-0.4]]}]}"#;
        let (code, reply) = t.request("POST", "/api/safety", zone);
        assert_eq!(code, 200, "{reply}");
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["settings"]["zones"][0]["id"], 1);
        let (code, reply) = t.request("POST", "/api/safety", r#"{"strobe_max_hz":3,"horizon":{"y":0.25}}"#);
        assert_eq!(code, 409);
        assert!(reply.contains("Public"), "{reply}");
        assert_eq!(get(&t)["settings"]["zones"].as_array().unwrap().len(), 1, "kept");
        let (code, _) = t.request("POST", "/api/safety", r#"{"confirm_loosen":true}"#);
        assert_eq!(code, 200);
        assert_eq!(get(&t)["settings"], serde_json::to_value(crate::safety::SafetySettings::default()).unwrap());
    }

    #[test]
    fn data_warnings_api_lists_and_acknowledges() {
        let t = TestServer::start(false);
        {
            let mut s = t.shared.lock().unwrap();
            s.data_warnings.push(crate::datafile::DataWarning {
                file: "calibration.json".into(),
                moved_to: Some("quarantine/calibration-20261003-141502.json".into()),
                error: "JSON invalide".into(),
                at: "2026-10-03T14:15:02Z".into(),
                safety: true,
                acknowledged: false,
            });
            s.sync_data_warnings();
        }
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let v = js(&t.request("GET", "/api/data/warnings", "").1);
        assert_eq!(v["arm_blocked"], true);
        assert!(v["warnings"][0]["message"].as_str().unwrap().contains("calibration.json"));
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["data_warnings"]["warnings"][0]["file"], "calibration.json");
        let (code, body) = t.request("POST", "/api/arm", r#"{"on":true,"source":"ui","client_id":"p"}"#);
        assert_eq!(code, 409);
        assert!(body.contains("Fichier de sécurité abîmé"), "{body}");
        assert_eq!(t.request("POST", "/api/data/warnings/ack", "{}").0, 400);
        let v = js(&t.request("POST", "/api/data/warnings/ack", r#"{"file":"calibration.json"}"#).1);
        assert_eq!((v["acknowledged"].as_u64(), v["arm_blocked"].as_bool()), (Some(1), Some(false)));
        assert_eq!(v["warnings"][0]["acknowledged"], true, "still listed");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui","client_id":"p"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false,"source":"ui"}"#).0, 200);
    }

    #[test]
    fn a_damaged_calibration_is_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("calibration", "calibration.json");
        assert_eq!(load_calibration(&path), Calibration::default());
        save_calibration(&path, &Calibration::default());
        assert!(path.exists());
        crate::datafile::tests::check_kept(&dir, "calibration.json", true);
    }

    /// T-264, invariant 6 for the HTTP API: random sequences of requests
    /// to every route that changes state in memory (controls, cues, looks,
    /// live, audio, heartbeats, presence, e-stop and its reset, layers,
    /// caps, safety, playlist, scenes, presets, test patterns, timeline,
    /// warnings, arm requests claiming a controller or the system) never
    /// arm the laser. Routes that write files or decode media are left out
    /// (their tests are elsewhere); the only arming route, `POST /api/arm`
    /// from the UI, keyboard or API, is sent only with sources that must
    /// be refused.
    #[test]
    fn no_api_request_arms_except_an_explicit_arm_request() {
        use crate::safety_invariants::{mutated, seeds, Rng};
        let t = TestServer::start(false);
        let (controls, presets): (Vec<String>, Vec<(String, Settings)>) = {
            let s = t.shared.lock().unwrap();
            (s.controls.list().iter().map(|d| d.id.clone()).collect(), s.presets.iter().map(|p| (p.id.clone(), p.settings.clone())).collect())
        };
        // Fewer requests than the engine properties: each is a real
        // localhost round trip.
        for seed in seeds(6, 25) {
            let mut rng = Rng::new(seed);
            for i in 0..60 {
                let (preset, look) = rng.pick(&presets).clone();
                let (path, body): (&str, String) = match rng.below(22) {
                    0 => ("/api/arm", json!({ "on": true, "source": rng.pick(&["midi", "system"]) }).to_string()),
                    1 => ("/api/arm", json!({ "toggle": true, "source": rng.pick(&["midi", "system"]) }).to_string()),
                    2 => ("/api/arm", json!({ "on": false }).to_string()),
                    3 | 4 => {
                        let value = if rng.chance(0.3) { json!(rng.chance(0.5)) } else { json!(rng.range(-3.0, 3.0)) };
                        ("/api/control", json!({ "id": rng.pick(&controls), "value": value }).to_string())
                    }
                    5 => ("/api/control", json!({ "id": "transport.arm", "norm": 1.0 }).to_string()),
                    6 => ("/api/cue", json!({ "id": preset, "down": rng.chance(0.6), "mode": rng.pick(&["toggle", "flash", "solo", "restart"]) }).to_string()),
                    7 => ("/api/settings", serde_json::to_string(&mutated(&look, &mut rng, 0.15)).unwrap()),
                    8 => ("/api/live", serde_json::to_string(&mutated(&crate::live::LiveModifiers::default(), &mut rng, 0.2)).unwrap()),
                    9 => ("/api/audio", json!({ "level": rng.range(0.0, 6.0), "bass": rng.f(), "beat": i }).to_string()),
                    10 => ("/api/heartbeat", json!({ "client_id": "fuzz", "hold": rng.chance(0.5), "gone": rng.chance(0.1) }).to_string()),
                    11 => ("/api/presence", json!({ "hold_to_run": rng.chance(0.5), "ui_timeout_ms": rng.below(20_000) }).to_string()),
                    12 if rng.chance(0.3) => ("/api/estop", String::new()),
                    13 => ("/api/estop/reset", String::new()),
                    14 => ("/api/layers", serde_json::to_string(&mutated(&crate::layers::Mixer::default(), &mut rng, 0.2)).unwrap()),
                    15 => ("/api/outputs/limits", json!({ "id": "main", "limits": { "max_power": rng.f() }, "confirm_raise": rng.chance(0.5) }).to_string()),
                    16 => ("/api/safety", json!({ "strobe_max_hz": rng.range(0.0, 10.0), "confirm_loosen": rng.chance(0.5) }).to_string()),
                    17 => (*rng.pick(&["/api/playlist/start", "/api/playlist/stop", "/api/timeline/play", "/api/timeline/pause", "/api/timeline/stop", "/api/audio/tempo/new_track"]), "{}".into()),
                    18 => ("/api/presets/play", json!({ "id": preset }).to_string()),
                    19 => ("/api/scenes/play", json!({ "name": "inconnue" }).to_string()),
                    20 => ("/api/testpattern", json!({ "pattern": rng.pick(&[json!("grid"), json!("color_bars"), json!(null)]), "brightness": rng.f() }).to_string()),
                    _ => ("/api/data/warnings/ack", json!({ "all": true }).to_string()),
                };
                let (status, reply) = t.request("POST", path, &body);
                assert!(!t.shared.lock().unwrap().gate.is_armed(), "seed {seed}, request {i}: POST {path} {body} armed the laser ({status} {reply})");
                if path == "/api/arm" && body.contains("\"on\":true") {
                    assert_eq!(status, 409, "seed {seed}: an arm request from a controller or the system is refused");
                }
                if path == "/api/control" && body.contains("transport.arm") {
                    assert_eq!(status, 403, "seed {seed}: the control endpoint never arms");
                }
            }
        }
        // The explicit path still works: the property isn't vacuous.
        t.request("POST", "/api/estop/reset", "");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed());
    }

    #[test]
    fn projection_zones_are_validated_saved_and_never_arm() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let v = js(&t.request("GET", "/api/projection", "").1);
        assert_eq!(v["config"]["zones"].as_array().unwrap().len(), 1);
        assert_eq!(v["config"]["default_zones"], json!([1]));
        assert_eq!(v["align"], serde_json::Value::Null);

        let two = json!({
            "zones": [
                { "id": 1, "name": "Gauche", "geometry": { "size_x": 0.5, "pos_x": -0.5 } },
                { "id": 2, "name": "Droite", "geometry": { "size_x": 9.0, "pos_x": 0.5,
                  "warp": { "enabled": true, "corners": [[-0.8, -0.9], [0.7, -0.6], [0.9, 0.8], [-0.6, 0.5]] } } }
            ],
            "default_zones": [1, 2]
        });
        let (code, body) = t.request("POST", "/api/projection", &two.to_string());
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert_eq!(v["config"]["zones"][1]["geometry"]["size_x"], 2.0, "clamped");
        assert_eq!(v["config"]["default_zones"], json!([1, 2]));
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["projection"]["zones"][0]["name"], "Gauche");

        // A crossed warp, duplicate ids, no zone: refused, nothing changed.
        let crossed = json!({ "zones": [{ "id": 1, "geometry": { "warp": { "enabled": true, "corners": [[-1, -1], [1, 1], [1, -1], [-1, 1]] } } }] });
        for bad in [crossed, json!({ "zones": [{ "id": 2 }, { "id": 2 }] }), json!({ "zones": [] }), json!({ "zones": [{ "id": 9 }] })] {
            let (code, text) = t.request("POST", "/api/projection", &bad.to_string());
            assert_eq!(code, 400, "{bad} → {text}");
        }
        assert_eq!(t.shared.lock().unwrap().projection.get().zones.len(), 2);

        assert_eq!(t.request("POST", "/api/projection/align", r#"{"zone":5}"#).0, 404);
        assert_eq!(js(&t.request("POST", "/api/projection/align", r#"{"zone":2}"#).1)["align"], 2);
        // Removing the aligned zone ends the alignment.
        let one = json!({ "zones": [{ "id": 1 }] });
        assert_eq!(js(&t.request("POST", "/api/projection", &one.to_string()).1)["align"], serde_json::Value::Null);
        assert!(!t.shared.lock().unwrap().gate.is_armed());

        // A cue's zones go through « Propriétés du cue ».
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-001","zones":[2,1,1,12]}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"]["zones"], json!([1, 2]));
    }

    #[test]
    fn cue_programs_are_set_kept_previewed_and_never_arm() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let program = json!({ "play": "ping_pong", "layers": [{ "kind": "wave" }],
            "lanes": [{ "target": "size", "keys": [{ "beat": 0, "value": 0.2 }, { "beat": 16, "value": 1.0 }] }] });
        let set = |id: &str, program: &serde_json::Value| t.request("POST", "/api/cues/program", &json!({ "id": id, "program": program }).to_string());
        assert_eq!(set("tunnels-001", &program).0, 200);
        let slot = &js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"];
        assert_eq!(slot["program"]["play"], "ping_pong");
        // The properties dialog doesn't send it, and keeps it.
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-001","layer":2}"#).0, 200);
        let slot = &js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"];
        assert_eq!((slot["layer"].clone(), slot["program"]["lanes"][0]["target"].clone()), (json!(2), json!("size")));
        // Refused: unknown target, too many layers, unknown cue.
        let (code, text) = set("tunnels-001", &json!({ "lanes": [{ "target": "trace" }] }));
        assert_eq!(code, 400, "{text}");
        assert!(text.contains("trace"));
        assert_eq!(set("tunnels-001", &json!({ "layers": vec![json!({ "kind": "wave" }); 5] })).0, 400);
        assert_eq!(set("nope", &program).0, 404);
        // The cue plays with it.
        assert_eq!(t.request("POST", "/api/cue", r#"{"id":"tunnels-001","down":true}"#).0, 200);
        assert!(t.shared.lock().unwrap().settings.program.is_some());
        // The grid's badge and hover preview.
        let presets = js(&t.request("GET", "/api/presets", "").1);
        let entry = |id: &str| presets["presets"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap().clone();
        assert_eq!((entry("vagues-028")["program"].clone(), entry("vagues-028")["beats"].clone()), (json!(true), json!(8.0)));
        assert_eq!(entry("tunnels-001")["program"], json!(false));
        // (A long, chunked answer: the frames are checked in presets.rs.)
        let (code, body) = t.request("GET", "/api/presets/preview?id=tunnels-001", "");
        assert!(code == 200 && body.contains(r#""step_beats":0.25"#), "{code}");
        assert_eq!(t.request("GET", "/api/presets/preview?id=nope", "").0, 404);
        // `null` removes it.
        assert_eq!(set("tunnels-001", &serde_json::Value::Null).0, 200);
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"]["program"], serde_json::Value::Null);
    }

    /// T-272: the grid's size and pages through the API, and the grid
    /// controls following them.
    #[test]
    fn the_cue_grid_api_resizes_and_edits_pages() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let g = js(&t.request("GET", "/api/grid", "").1);
        assert_eq!((g["cols"].as_u64(), g["rows"].as_u64(), g["default"].as_bool()), (Some(8), Some(5), Some(true)));
        assert_eq!(g["pages"][1]["name"], "Tunnels");
        assert_eq!(g["pages"][1]["cells"].as_array().unwrap().len(), 40);
        assert_eq!(g["pages"][1]["cells"][0], "tunnels-001");

        let g = js(&t.request("POST", "/api/grid", r#"{"cols":6,"rows":4}"#).1);
        assert_eq!(g["pages"][1]["cells"].as_array().unwrap().len(), 24);
        assert_eq!(g["pages"][1]["cells"][0], "tunnels-001");
        assert_eq!(t.request("POST", "/api/grid", r#"{"cols":3}"#).0, 400);
        assert!(t.shared.lock().unwrap().controls.get("grid.2.4.5").is_some(), "Tunnels: 23 cues");
        assert!(t.shared.lock().unwrap().controls.get("grid.2.1.7").is_none(), "6 columns");

        // A new page is shown; a cue dragged onto it plays from its cell.
        let g = js(&t.request("POST", "/api/grid/page", r#"{"action":"new","name":"Final"}"#).1);
        let n = g["page"].as_u64().unwrap() as usize;
        assert_eq!(g["pages"][n]["name"], "Final");
        assert_eq!(g["default"], false);
        assert_eq!(t.request("POST", "/api/grid/move", &format!(r#"{{"from":[1,0],"to":[{n},5]}}"#)).0, 200);
        assert_eq!(t.request("POST", "/api/control", &format!(r#"{{"id":"grid.{}.1.6"}}"#, n + 1)).0, 200);
        assert_eq!(t.shared.lock().unwrap().active_cue.as_deref(), Some("tunnels-001"));
        assert_eq!(t.request("POST", "/api/grid/page", &format!(r#"{{"action":"rename","page":{n},"name":" "}}"#)).0, 400);
        assert_eq!(t.request("POST", "/api/grid/page", r#"{"action":"explode"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/grid/clear", &format!(r#"{{"page":{n},"slot":5}}"#)).0, 200);
        assert!(t.shared.lock().unwrap().controls.get(&format!("grid.{}.1.6", n + 1)).is_none());
        assert_eq!(t.request("POST", "/api/grid/page", &format!(r#"{{"action":"delete","page":{n}}}"#)).0, 200);
        // Back to the default 8×5 pages.
        let g = js(&t.request("POST", "/api/grid", r#"{"cols":8,"rows":5,"pages":[]}"#).1);
        assert_eq!((g["default"].as_bool(), g["pages"][0]["cells"].as_array().unwrap().len()), (Some(true), 40));
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-405: store the look into a cell, update it, edit the grid; the
    /// built-in cues are refused with 409.
    #[test]
    fn user_cues_are_stored_updated_and_edited_through_the_api() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let (code, body) = t.request("POST", "/api/grid/pages", r#"{"op":"add","name":"Mes looks"}"#);
        assert_eq!(code, 200, "{body}");
        let g = js(&body);
        let n = g["page"].as_u64().unwrap() as usize;
        assert_eq!((g["pages"][n]["name"].as_str(), g["pages"][n]["user"].as_bool()), (Some("Mes looks"), Some(true)));
        t.shared.lock().unwrap().settings = Settings { color: [255, 0, 255], scale: 0.3, ..Settings::default() };

        let (code, body) = t.request("POST", "/api/cues/store", &format!(r#"{{"page":{n},"slot":2,"name":"Intro","filter":"all","with_master":true}}"#));
        assert_eq!(code, 200, "{body}");
        let g = js(&body);
        assert_eq!(g["cue"], "u-1");
        assert_eq!(g["pages"][n]["cells"][2], "u-1");
        // Listed with its colour, and its cell properties can be set.
        let presets = js(&t.request("GET", "/api/presets", "").1);
        let p = presets["presets"].as_array().unwrap().iter().find(|p| p["id"] == "u-1").unwrap().clone();
        assert_eq!((p["name"].as_str(), p["user"].as_bool(), p["color"].clone(), p["master"].as_bool()), (Some("Intro"), Some(true), json!([255, 0, 255]), Some(true)));
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"u-1","layer":2}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["u-1"]["layer"], 2);
        // It plays from its cell like any cue.
        assert_eq!(t.request("POST", "/api/control", &format!(r#"{{"id":"grid.{}.1.3"}}"#, n + 1)).0, 200);
        assert_eq!(t.shared.lock().unwrap().active_cue.as_deref(), Some("u-1"));

        // « Mettre à jour »: the playing user cue, through the filter.
        t.shared.lock().unwrap().settings.scale = 0.9;
        t.shared.lock().unwrap().settings.color = [0, 255, 0];
        let (code, body) = t.request("POST", "/api/cues/update", r#"{"filter":"geometry_only"}"#);
        assert_eq!((code, js(&body)["id"].as_str()), (200, Some("u-1")));
        let cue = t.shared.lock().unwrap().user_cues.get("u-1").unwrap().clone();
        assert_eq!((cue.settings.scale, cue.settings.color), (0.9, [255, 0, 255]));
        assert_eq!(t.request("POST", "/api/cues/update", r#"{"id":"tunnels-001"}"#).0, 409);

        // A built-in cue can't be stored onto.
        let (code, text) = t.request("POST", "/api/cues/store", r#"{"page":1,"slot":0}"#);
        assert_eq!((code, text.as_str()), (409, crate::usercues::BUILTIN_REFUSED));
        assert_eq!(t.request("POST", "/api/cues/store", r#"{"page":1,"slot":0,"filter":"sideways"}"#).0, 400);

        // Grid edits: copy to another page, rename, colour, swap, delete.
        let m = js(&t.request("POST", "/api/grid/pages", r#"{"op":"add"}"#).1)["page"].as_u64().unwrap() as usize;
        let g = js(&t.request("POST", "/api/grid/edit", &format!(r#"{{"op":"copy","from":[{n},2],"to":[{m},null]}}"#)).1);
        assert_eq!(g["pages"][m]["cells"][0], "u-2");
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["u-2"]["layer"], 2, "the copy keeps the properties");
        assert_eq!(t.request("POST", "/api/grid/edit", &format!(r#"{{"op":"rename","from":[{m},0],"name":"Copie"}}"#)).0, 200);
        assert_eq!(t.request("POST", "/api/grid/edit", &format!(r#"{{"op":"color","from":[{m},0],"color":[1,2,3]}}"#)).0, 200);
        assert_eq!(t.shared.lock().unwrap().user_cues.get("u-2").unwrap().name, "Copie");
        assert_eq!(t.request("POST", "/api/grid/edit", r#"{"op":"rename","from":[1,0],"name":"X"}"#).0, 409);
        let g = js(&t.request("POST", "/api/grid/edit", r#"{"op":"copy_to_user","from":[1,0]}"#).1);
        let mine = g["page"].as_u64().unwrap() as usize;
        assert!(g["pages"][mine]["user"].as_bool().unwrap());
        assert_eq!(t.request("POST", "/api/grid/edit", &format!(r#"{{"op":"swap","from":[{n},2],"to":[{m},0]}}"#)).0, 200);
        let g = js(&t.request("GET", "/api/grid", "").1);
        assert_eq!((g["pages"][n]["cells"][2].as_str(), g["pages"][m]["cells"][0].as_str()), (Some("u-2"), Some("u-1")));
        assert_eq!(t.request("POST", "/api/grid/edit", &format!(r#"{{"op":"delete","from":[{m},0]}}"#)).0, 200);
        assert!(t.shared.lock().unwrap().user_cues.get("u-1").is_none());
        assert_eq!(t.request("POST", "/api/grid/edit", r#"{"op":"explode","from":[0,0]}"#).0, 400);
        assert_eq!(t.request("POST", "/api/grid/pages", &format!(r#"{{"op":"rename","page":{m},"name":"B"}}"#)).0, 200);
        assert_eq!(t.request("POST", "/api/grid/pages", &format!(r#"{{"op":"delete","page":{m}}}"#)).0, 200);
        assert!(!t.shared.lock().unwrap().gate.is_armed(), "nothing here arms");
    }
}
