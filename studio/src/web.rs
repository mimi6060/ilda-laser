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

/// Requests whose handler may run an audio decoder.
fn may_decode(method: &Method, url: &str) -> bool {
    let path = url.split('?').next().unwrap_or("");
    matches!((method, path), (Method::Post, "/api/media/audio") | (Method::Get, "/api/timeline/waveform") | (Method::Post, "/api/timeline/audio"))
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
        let _ = request.respond(text(503, "trop de décodages audio en cours, réessayez"));
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
        (Method::Post, "/api/audio") => match body::<AudioFeatures>(request) {
            Ok(audio) => {
                let mut s = shared.lock().unwrap();
                s.audio = audio;
                s.audio_at = Instant::now();
                ok()
            }
            Err(e) => e,
        },
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
                s.presence.set_settings(settings);
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
    let c = s.timeline_clock();
    json_response(json!({ "state": s.timeline.state(&c), "audio": song_view(s), "saved": saved }))
}

/// The song player's status, and which clock the timeline follows.
fn song_view(s: &Shared) -> serde_json::Value {
    let mut v = json!(s.song.status());
    v["clock"] = json!(s.song_sync.clock);
    v
}

/// `POST /api/timeline/{load,play,pause,stop,seek,loop}`. Transport only:
/// nothing here can arm the laser.
fn timeline_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, action: &str) -> HttpResponse {
    let req = match action {
        "load" | "seek" | "loop" => match body::<TimelineRequest>(request) {
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
            }
        }
        _ => return text(404, "not found"),
    }
    json_response(json!(s.timeline.state(&c)))
}

#[derive(Deserialize, Default)]
struct TimelineRequest {
    name: Option<String>,
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
        assert_eq!(st["settings"]["beam_floor_y"], 0.25);
        assert_eq!(st["settings"]["strobe_burst_s"], 5.0, "missing fields keep the safe default");
        assert_eq!(st["status"]["active"], false);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["strobe"]["active"], false);
    }
}
