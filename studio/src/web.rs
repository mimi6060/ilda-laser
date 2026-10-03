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
                json_response(json!(s.presence.settings))
            }
            Err(e) => e,
        },
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
            let beats = |p: &crate::presets::Preset| match &p.settings.content {
                crate::engine::Content::Evolving(e) => Some(e.length_beats),
                _ => None,
            };
            let list: Vec<_> = s.presets.iter().map(|p| json!({ "id": p.id, "name": p.name, "category": p.category, "beats": beats(p) })).collect();
            json_response(json!({ "categories": CATEGORIES, "presets": list }))
        }
        (Method::Get, "/api/presets/thumbs") => {
            let presets: Vec<_> = shared.lock().unwrap().presets.iter().map(|p| (p.id.clone(), p.settings.clone())).collect();
            json_response(json!(cue_thumbnails(&presets)))
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
            json_response(json!({ "routes": r.routes, "mix": r.mix, "sources": sources, "targets": targets, "max": crate::audio::routes::MAX_ROUTES }))
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
        (Method::Get, "/api/timeline/waveform") => waveform(request.url(), shared),
        (Method::Post, "/api/timeline/audio") => match body::<SongPatch>(request) {
            Ok(patch) => attach_song(patch, shared),
            Err(e) => e,
        },
        (Method::Post, p) if p.starts_with("/api/timeline/") => timeline_route(request, shared, &p["/api/timeline/".len()..]),
        (method, p) if p.starts_with("/api/midi") => midi_route(request, shared, method == Method::Post, p),
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
            let key = serde_json::to_string(settings).unwrap_or_default();
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
struct TextRequest {
    text: String,
    #[serde(default = "default_text_size")]
    size: f32,
}

fn default_text_size() -> f32 {
    0.3
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
        "engine_ok": s.health.engine_ok(),
        "presence": s.presence.status(),
        "output": s.output_name,
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
        "engine_ok": s.health.engine_ok(),
        "presence": s.presence.status(),
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
        "timeline": s.timeline.state(&s.timeline_clock()),
        "timeline_audio": song_view(s),
        "live": s.live,
        "lfos": lfo_positions(s),
        "audio_routes": s.routes.meters().collect::<Vec<_>>(),
        "strobe": s.strobe,
        "evolving": evolving_status(s),
    }))
}

/// Evolving cues on show (last frame), oldest first: the cue (`null` for
/// the manual look), its layer and where it is in its keys.
fn evolving_status(s: &Shared) -> Vec<serde_json::Value> {
    s.evolving
        .iter()
        .map(|(id, progress)| {
            let cue = s.deck.active.iter().find(|a| a.id == *id);
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
        let (code, body) = t.request("GET", "/api/safety/log", "");
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let kinds: Vec<&str> = v["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["arm", "cue", "disarm", "arm", "estop", "estop_reset", "safety_settings", "midi_safety"]);
        let events = v["events"].as_array().unwrap();
        assert_eq!((events[2]["source"].as_str(), events[2]["source_fr"].as_str()), (Some("keyboard"), Some("clavier")));
        assert_eq!(events[4]["text"], "Arrêt d'urgence (laser désarmé)");
        assert_eq!(events[6]["detail"]["changed"], json!(["strobe_max_hz"]));
        assert_eq!((events[6]["detail"]["before"]["strobe_max_hz"].as_f64(), events[6]["detail"]["after"]["strobe_max_hz"].as_f64()), (Some(4.0), Some(3.0)));
        assert_eq!(events[7]["text"], "Armement MIDI autorisé");
        assert_eq!(v["date"], v["today"]);
        assert_eq!(v["days"], json!([v["today"]]));
        assert_eq!(v["status"]["failed"], 0);

        let date = v["date"].as_str().unwrap();
        let (code, csv) = t.request("GET", &format!("/api/safety/log.csv?date={date}"), "");
        assert_eq!(code, 200);
        assert_eq!(csv.lines().count(), 9, "header + 8 events");
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
}
