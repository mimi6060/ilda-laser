//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.
//!
//! One thread receives requests; `POST /api/estop` is handled right there,
//! without the studio lock, ahead of everything queued. All other requests go, in order,
//! to a single worker thread, so a slow handler never delays a stop. The
//! requests that may decode a song (import, waveform, attach: seconds, up
//! to the decoding time limit) get their own thread, so they never hold up
//! the heartbeats queued behind them (T-298: a slow or hanging decode must
//! not disarm the laser as « Interface perdue »).
//!
//! The pushed state (`GET /api/events`, Server-Sent Events) and the binary
//! frames (`GET /api/frame.bin`) are served on their own threads too, a
//! bounded number of them (push.rs, T-411): a stream never occupies the
//! worker, so it never delays a heartbeat or a stop. A stream whose page
//! stops reading blocks only its own thread (no lock held while writing)
//! until the page goes away; there are at most `MAX_STREAMS` of them.

use crate::controls;
use crate::cues::{ClickMode, CueSlot};
use crate::engine::{Calibration, Settings};
use crate::generators::GENERATOR_NAMES;
use crate::interlock::{ArmSource, DisarmReason, EStop};
use crate::patterns::SHAPE_NAMES;
use crate::presets::CATEGORIES;
use crate::push::{self, Hub, StateWait};
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

/// What a route of the main UI does (T-428), for the table below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteClass {
    /// Reads state; changes nothing.
    Read,
    /// Plays, stops or adjusts live (allowed in show mode).
    Play,
    /// Edits content or settings.
    Edit,
    /// Safety, outputs, calibration, the external gate, the remote access.
    Safety,
    /// Arms, resets the e-stop, or counts as the operator's presence.
    Arm,
    /// `--test-hooks` only.
    Test,
}

/// Every route `dispatch` (and the fast paths before it) serves: method
/// (`*`: any), path (ending in `*`: a prefix), class. The tablet remote
/// (remote.rs) answers 403 to all of them and serves only its own
/// whitelist; `every_route_is_in_the_table` fails when a route is added
/// to `dispatch` without a line here.
pub const ROUTES: &[(&str, &str, RouteClass)] = &[
    ("GET", "/", RouteClass::Read),
    ("GET", "/beam3d.js", RouteClass::Read),
    ("POST", "/api/estop", RouteClass::Safety),
    ("GET", "/api/events", RouteClass::Read),
    ("GET", "/api/frame.bin", RouteClass::Read),
    ("GET", "/api/history", RouteClass::Read),
    ("POST", "/api/undo", RouteClass::Edit),
    ("POST", "/api/redo", RouteClass::Edit),
    ("GET", "/api/state", RouteClass::Read),
    ("GET", "/api/diagnostics/report", RouteClass::Read),
    ("GET", "/api/logs/tail", RouteClass::Read),
    ("GET", "/api/frame", RouteClass::Read),
    ("GET", "/api/preview-frame", RouteClass::Read),
    ("POST", "/api/prepared", RouteClass::Play),
    ("POST", "/api/prepared/adjust", RouteClass::Play),
    ("POST", "/api/prepared/send", RouteClass::Play),
    ("POST", "/api/prepared/clear", RouteClass::Play),
    ("GET", "/api/notices", RouteClass::Read),
    ("GET", "/api/data/warnings", RouteClass::Read),
    ("POST", "/api/data/warnings/ack", RouteClass::Safety),
    ("POST", "/api/settings", RouteClass::Edit),
    ("POST", "/api/audio", RouteClass::Play),
    ("GET", "/api/audio/state", RouteClass::Read),
    ("GET", "/api/audio/spectrum", RouteClass::Read),
    ("GET", "/api/audio/devices", RouteClass::Read),
    ("GET", "/api/devices", RouteClass::Read),
    ("POST", "/api/devices/scan", RouteClass::Safety),
    ("GET", "/api/outputs", RouteClass::Read),
    ("POST", "/api/outputs/scan", RouteClass::Safety),
    ("POST", "/api/outputs/select", RouteClass::Safety),
    ("POST", "/api/outputs/config", RouteClass::Safety),
    ("GET", "/api/outputs/artnet", RouteClass::Read),
    ("POST", "/api/outputs/artnet", RouteClass::Safety),
    ("POST", "/api/test/output", RouteClass::Test),
    ("GET", "/api/audio/config", RouteClass::Read),
    ("POST", "/api/audio/config", RouteClass::Edit),
    ("POST", "/api/audio/tempo/new_track", RouteClass::Play),
    ("POST", "/api/heartbeat", RouteClass::Arm),
    ("GET", "/api/presence", RouteClass::Read),
    ("POST", "/api/presence", RouteClass::Safety),
    ("GET", "/api/lock", RouteClass::Read),
    ("POST", "/api/lock", RouteClass::Safety),
    ("GET", "/api/onboarding", RouteClass::Read),
    ("POST", "/api/onboarding", RouteClass::Play),
    ("GET", "/api/help", RouteClass::Read),
    ("POST", "/api/keep_awake", RouteClass::Safety),
    ("POST", "/api/test/wifi", RouteClass::Test),
    ("GET", "/api/test/menu", RouteClass::Test),
    ("POST", "/api/test/menu", RouteClass::Test),
    ("POST", "/api/test/stall", RouteClass::Test),
    ("POST", "/api/test/tempo_estimate", RouteClass::Test),
    ("POST", "/api/test/timecode", RouteClass::Test),
    ("POST", "/api/test/native_audio", RouteClass::Test),
    ("GET", "/api/arm", RouteClass::Read),
    ("POST", "/api/arm", RouteClass::Arm),
    ("POST", "/api/estop/reset", RouteClass::Arm),
    ("GET", "/api/safety", RouteClass::Read),
    ("POST", "/api/safety", RouteClass::Safety),
    ("GET", "/api/safety/profiles", RouteClass::Read),
    ("POST", "/api/safety/profiles", RouteClass::Safety),
    ("POST", "/api/safety/profiles/load", RouteClass::Safety),
    ("GET", "/api/audience", RouteClass::Read),
    ("POST", "/api/audience", RouteClass::Safety),
    ("GET", "/api/outputs/limits", RouteClass::Read),
    ("POST", "/api/outputs/limits", RouteClass::Safety),
    ("GET", "/api/safety/exposure", RouteClass::Read),
    ("POST", "/api/safety/exposure", RouteClass::Safety),
    ("GET", "/api/outputs/color", RouteClass::Read),
    ("POST", "/api/outputs/color", RouteClass::Safety),
    ("GET", "/api/outputs/scan_limits", RouteClass::Read),
    ("POST", "/api/outputs/scan_limits", RouteClass::Safety),
    ("GET", "/api/testpattern", RouteClass::Read),
    ("POST", "/api/testpattern", RouteClass::Safety),
    ("GET", "/api/checklist", RouteClass::Read),
    ("GET", "/api/safety/log", RouteClass::Read),
    ("GET", "/api/safety/log.csv", RouteClass::Read),
    ("POST", "/api/calibration", RouteClass::Safety),
    ("GET", "/api/venue", RouteClass::Read),
    ("POST", "/api/venue", RouteClass::Safety),
    ("POST", "/api/venue/preset", RouteClass::Safety),
    ("GET", "/api/frames", RouteClass::Read),
    ("GET", "/api/projection", RouteClass::Read),
    ("GET", "/api/groups", RouteClass::Read),
    ("POST", "/api/projection", RouteClass::Safety),
    ("POST", "/api/projection/align", RouteClass::Safety),
    ("POST", "/api/scenes/save", RouteClass::Edit),
    ("POST", "/api/scenes/delete", RouteClass::Edit),
    ("POST", "/api/scenes/play", RouteClass::Play),
    ("POST", "/api/playlist/start", RouteClass::Play),
    ("GET", "/api/presets", RouteClass::Read),
    ("GET", "/api/presets/thumbs", RouteClass::Read),
    ("GET", "/api/presets/loops", RouteClass::Read),
    ("GET", "/api/presets/preview", RouteClass::Read),
    ("POST", "/api/presets/play", RouteClass::Play),
    ("POST", "/api/cue", RouteClass::Play),
    ("GET", "/api/cues", RouteClass::Read),
    ("GET", "/api/cues/tags", RouteClass::Read),
    ("POST", "/api/cues/slot", RouteClass::Edit),
    ("POST", "/api/cues/program", RouteClass::Edit),
    ("POST", "/api/cues/modifiers", RouteClass::Edit),
    ("GET", "/api/cues/program", RouteClass::Read),
    ("POST", "/api/cues/program/preview", RouteClass::Edit),
    ("POST", "/api/cues/program/store", RouteClass::Edit),
    ("GET", "/api/grid", RouteClass::Read),
    ("POST", "/api/grid", RouteClass::Edit),
    ("POST", "/api/grid/page", RouteClass::Edit),
    ("POST", "/api/grid/pages", RouteClass::Edit),
    ("POST", "/api/grid/edit", RouteClass::Edit),
    ("POST", "/api/cues/store", RouteClass::Edit),
    ("POST", "/api/cues/update", RouteClass::Edit),
    ("POST", "/api/grid/move", RouteClass::Edit),
    ("POST", "/api/grid/clear", RouteClass::Edit),
    ("GET", "/api/layers", RouteClass::Read),
    ("POST", "/api/layers", RouteClass::Edit),
    ("GET", "/api/live", RouteClass::Read),
    ("POST", "/api/live", RouteClass::Play),
    ("GET", "/api/festival", RouteClass::Read),
    ("GET", "/api/latency", RouteClass::Read),
    ("POST", "/api/latency", RouteClass::Safety),
    ("POST", "/api/festival/palette", RouteClass::Edit),
    ("POST", "/api/festival", RouteClass::Edit),
    ("GET", "/api/palettes", RouteClass::Read),
    ("POST", "/api/palettes", RouteClass::Edit),
    ("GET", "/api/scan-presets", RouteClass::Read),
    ("POST", "/api/scan-presets", RouteClass::Edit),
    ("GET", "/api/speed-groups", RouteClass::Read),
    ("POST", "/api/speed-groups", RouteClass::Play),
    ("GET", "/api/lfos", RouteClass::Read),
    ("POST", "/api/lfos", RouteClass::Edit),
    ("GET", "/api/executors", RouteClass::Read),
    ("POST", "/api/executors", RouteClass::Edit),
    ("GET", "/api/desks", RouteClass::Read),
    ("POST", "/api/desks", RouteClass::Edit),
    ("GET", "/api/fx", RouteClass::Read),
    ("POST", "/api/fx", RouteClass::Edit),
    ("GET", "/api/audio/routes", RouteClass::Read),
    ("POST", "/api/audio/routes", RouteClass::Edit),
    ("GET", "/api/audio/presets", RouteClass::Read),
    ("POST", "/api/audio/preset", RouteClass::Edit),
    ("GET", "/api/audio/triggers", RouteClass::Read),
    ("POST", "/api/audio/triggers", RouteClass::Edit),
    ("GET", "/api/controls", RouteClass::Read),
    ("GET", "/api/control-values", RouteClass::Read),
    ("POST", "/api/control", RouteClass::Play),
    ("POST", "/api/command", RouteClass::Play),
    ("GET", "/api/command/actions", RouteClass::Read),
    ("GET", "/api/running", RouteClass::Read),
    ("POST", "/api/running/stop", RouteClass::Play),
    ("GET", "/api/cuelists", RouteClass::Read),
    ("POST", "/api/cuelists", RouteClass::Edit),
    ("POST", "/api/cuelists/delete", RouteClass::Edit),
    ("POST", "/api/cuelist/load", RouteClass::Play),
    ("POST", "/api/playlist/stop", RouteClass::Play),
    ("GET", "/api/shows", RouteClass::Read),
    ("POST", "/api/shows", RouteClass::Edit),
    ("GET", "/api/figures", RouteClass::Read),
    ("POST", "/api/figures", RouteClass::Edit),
    ("POST", "/api/figures/load", RouteClass::Edit),
    ("POST", "/api/figures/delete", RouteClass::Edit),
    ("POST", "/api/figures/import", RouteClass::Edit),
    ("POST", "/api/figures/text", RouteClass::Edit),
    ("GET", "/api/fonts", RouteClass::Read),
    ("GET", "/api/timeline", RouteClass::Read),
    ("GET", "/api/timeline/markers.csv", RouteClass::Read),
    ("POST", "/api/timeline/markers/parse", RouteClass::Edit),
    ("GET", "/api/media/audio", RouteClass::Read),
    ("POST", "/api/media/audio", RouteClass::Edit),
    ("GET", "/api/media/ilda", RouteClass::Read),
    ("POST", "/api/media/ilda", RouteClass::Edit),
    ("POST", "/api/media/ilda/delete", RouteClass::Edit),
    ("GET", "/api/media/ilda/export", RouteClass::Read),
    ("GET", "/api/ilda/record", RouteClass::Read),
    ("POST", "/api/ilda/record", RouteClass::Edit),
    ("POST", "/api/ilda/record/cancel", RouteClass::Play),
    ("GET", "/api/ilda/record/file", RouteClass::Read),
    ("GET", "/api/sdbank", RouteClass::Read),
    ("POST", "/api/sdbank", RouteClass::Edit),
    ("POST", "/api/sdbank/add", RouteClass::Edit),
    ("POST", "/api/sdbank/renumber", RouteClass::Edit),
    ("POST", "/api/sdbank/export", RouteClass::Edit),
    ("GET", "/api/timeline/waveform", RouteClass::Read),
    ("GET", "/api/timeline/examples", RouteClass::Read),
    ("POST", "/api/timeline/audio", RouteClass::Edit),
    ("GET", "/api/timeline/autoshow", RouteClass::Read),
    ("POST", "/api/timeline/autoshow", RouteClass::Edit),
    ("POST", "/api/timeline/autoshow/cancel", RouteClass::Edit),
    ("GET", "/api/timeline/templates", RouteClass::Read),
    ("POST", "/api/timeline/templates", RouteClass::Edit),
    ("POST", "/api/timeline/templates/delete", RouteClass::Edit),
    ("POST", "/api/timeline/templates/render", RouteClass::Edit),
    ("GET", "/api/netin", RouteClass::Read),
    ("POST", "/api/netin", RouteClass::Safety),
    ("GET", "/api/backup", RouteClass::Read),
    ("POST", "/api/backup", RouteClass::Edit),
    ("POST", "/api/backup/now", RouteClass::Play),
    ("GET", "/api/help/*", RouteClass::Read),
    ("POST", "/api/checklist/*", RouteClass::Safety),
    ("POST", "/api/cuelist/*", RouteClass::Play),
    ("POST", "/api/timeline/*", RouteClass::Edit),
    ("*", "/api/midi*", RouteClass::Edit),
    ("*", "/api/external*", RouteClass::Safety),
    ("*", "/api/dmx/*", RouteClass::Safety),
    ("*", "/api/timecode", RouteClass::Safety),
    ("*", "/api/timecode/arm", RouteClass::Safety),
    ("*", "/api/project", RouteClass::Edit),
    ("*", "/api/project/*", RouteClass::Edit),
    ("GET", "/api/remote", RouteClass::Read),
    ("POST", "/api/remote", RouteClass::Safety),
    ("POST", "/api/remote/kick", RouteClass::Safety),
];

/// Whether `method path` is a route of the main UI (`ROUTES`).
pub fn is_known_route(method: &str, path: &str) -> bool {
    ROUTES.iter().any(|&(m, p, _)| (m == "*" || m == method) && p.strip_suffix('*').map_or(path == p, |prefix| path.starts_with(prefix)))
}

pub fn run(addr: &str, shared: Arc<Mutex<Shared>>, estop: Arc<EStop>, calibration_path: PathBuf, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let server = Server::http(addr).map_err(|e| anyhow::anyhow!("failed to start web server on {addr}: {e}"))?;
    serve(server, shared, estop, calibration_path, running);
    Ok(())
}

pub(crate) fn serve(server: Server, shared: Arc<Mutex<Shared>>, estop: Arc<EStop>, calibration_path: PathBuf, running: Arc<AtomicBool>) {
    let hub = Arc::clone(&shared.lock().unwrap().push);
    let frames = Arc::new(FramePool::default());
    // The pushed state, built only while a stream is open (T-411).
    let builder = std::thread::Builder::new().name("push-state".into()).spawn({
        let (shared, hub, running) = (Arc::clone(&shared), Arc::clone(&hub), Arc::clone(&running));
        move || build_states(&shared, &hub, &running)
    });
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
            emergency_stop(request, &estop, &shared, &hub);
        } else if let Some(push) = push_route(request.method(), request.url()) {
            // Lock-free here: the hub only.
            match push {
                Push::Events => stream_aside(request, &hub),
                Push::Frame => frame_aside(request, &hub, &frames),
            }
        } else if let Err(e) = queue.send(request) {
            log::warn!("HTTP worker gone: {e}");
        }
    }
    drop(queue);
    // Streams, frame waits and the state builder return at once.
    hub.close();
    frames.close();
    worker.join().ok();
    if let Ok(builder) = builder {
        builder.join().ok();
    }
}

enum Push {
    Events,
    Frame,
}

/// The pushed routes (T-411), served beside the worker.
fn push_route(method: &Method, url: &str) -> Option<Push> {
    if *method != Method::Get {
        return None;
    }
    match url.split('?').next() {
        Some("/api/events") => Some(Push::Events),
        Some("/api/frame.bin") => Some(Push::Frame),
        _ => None,
    }
}

/// While a state stream is open: the page's state (`/api/frame` without
/// the points), at most 30 times a second, published when it changed.
fn build_states(shared: &Mutex<Shared>, hub: &Hub, running: &AtomicBool) {
    while running.load(Ordering::SeqCst) && !hub.is_closed() {
        if !hub.wait_for_streams(Duration::from_millis(200)) {
            continue;
        }
        let started = Instant::now();
        let (mut state, anchor) = {
            let Ok(mut s) = shared.lock() else { return };
            (live_view(&mut s), s.tempo.beat_at(0.0))
        };
        let moving = split_moving(&mut state, anchor);
        // Serialized only when it changed, with what moves put back.
        hub.publish_state(state, |fingerprint| {
            let mut full = fingerprint.clone();
            put_back(&mut full, moving);
            full.to_string()
        });
        std::thread::sleep(push::STATE_INTERVAL.saturating_sub(started.elapsed()));
    }
}

/// What moves by itself in the page's state: the beat (the page counts it
/// on from `bpm`) and the static beam guard's figures the page doesn't
/// show (`extent` only matters while it limits).
const MOVING: [(&str, &str); 6] = [("tempo", "beat"), ("tempo", "bar"), ("tempo", "beat_in_bar"), ("tempo", "phase"), ("dwell", "max_dose"), ("dwell", "extent")];

/// Takes what moves by itself out of `state`, which is left as its
/// fingerprint: a change in it is a real change, not time passing. The
/// beat is replaced by `anchor` (the beat at the epoch), which only moves
/// when the clock is nudged, synced or retimed. Returns what was taken.
fn split_moving(state: &mut serde_json::Value, anchor: f64) -> Vec<(&'static str, &'static str, serde_json::Value)> {
    let limiting = state["dwell"]["extent_factor"].as_f64().is_some_and(|f| f < 1.0);
    let mut moved = Vec::new();
    for (obj, key) in MOVING {
        if (obj, key) == ("dwell", "extent") && limiting {
            continue;
        }
        if let Some(v) = state.get_mut(obj).and_then(|o| o.as_object_mut()).and_then(|o| o.remove(key)) {
            moved.push((obj, key, v));
        }
    }
    if let Some(t) = state.get_mut("tempo").and_then(|t| t.as_object_mut()) {
        t.insert("anchor".into(), json!((anchor * 1e6).round() / 1e6));
    }
    // How long ago each audio trigger fired (T-240) grows by itself: the
    // page counts it on; a new firing changes `fired`.
    if let Some(rules) = state.pointer_mut("/audio_triggers/rules").filter(|r| r.is_array()) {
        let original = rules.clone();
        for r in rules.as_array_mut().into_iter().flatten() {
            if let Some(r) = r.as_object_mut() {
                r.remove("ago_s");
            }
        }
        moved.push(("audio_triggers", "rules", original));
    }
    moved
}

fn put_back(state: &mut serde_json::Value, moved: Vec<(&'static str, &'static str, serde_json::Value)>) {
    for (obj, key, v) in moved {
        if let Some(o) = state.get_mut(obj).and_then(|o| o.as_object_mut()) {
            o.insert(key.into(), v);
        }
    }
}

/// `GET /api/events` (T-411): `text/event-stream` on its own thread. First
/// the current state, then each new one (`id` = its version), and a
/// comment after `KEEPALIVE` without change. At most `MAX_STREAMS` (503).
/// A page that stops reading blocks this thread in a write until it goes
/// away (tiny_http gives no send timeout on its connections); the write
/// holds no lock: the event is a copy taken from the hub beforehand.
fn stream_aside(request: Request, hub: &Arc<Hub>) {
    let Some(slot) = hub.open_stream() else {
        let _ = request.respond(text(503, "trop de flux d'état ouverts"));
        return;
    };
    let hub = Arc::clone(hub);
    let spawned = std::thread::Builder::new().name("http-events".into()).stack_size(PUSH_STACK).spawn(move || {
        let _slot = slot;
        let mut writer = request.into_writer();
        if let Err(e) = write_stream(&mut writer, &hub) {
            log::debug!("state stream closed: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("no thread for a state stream: {e}");
    }
}

/// Stack of a stream or frame thread: they only copy bytes.
const PUSH_STACK: usize = 128 * 1024;

fn write_stream(w: &mut impl std::io::Write, hub: &Hub) -> std::io::Result<()> {
    // Written by hand: the body has no length, it ends with the connection.
    w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nretry: 1000\n\n")?;
    w.flush()?;
    let mut version = 0;
    loop {
        match hub.wait_state(version, push::KEEPALIVE) {
            StateWait::State(v, json) => {
                // A slow reader skips versions: it always gets the newest.
                write!(w, "id: {v}\nevent: state\ndata: {json}\n\n")?;
                version = v;
            }
            StateWait::Timeout => w.write_all(b": maintien\n\n")?,
            StateWait::Closed => return Ok(()),
        }
        w.flush()?;
    }
}

/// `GET /api/frame.bin[?after=<seq>]` (T-411): waits at most `FRAME_WAIT`
/// for a frame other than `after`, then sends the newest, binary
/// (push.rs), with the state version in `X-State-Version`. On a frame
/// thread (`FramePool`); at most `MAX_FRAME_WAITERS` in progress (503).
fn frame_aside(request: Request, hub: &Arc<Hub>, pool: &Arc<FramePool>) {
    let Some(slot) = hub.frame_request() else {
        let _ = request.respond(text(503, "trop de demandes d'image en attente"));
        return;
    };
    pool.submit(request, slot, hub);
}

fn send_frame(request: Request, slot: push::OwnedSlot, hub: &Hub) {
    let after = query_u64(request.url(), "after").map(|v| v as u32);
    let response = match hub.wait_frame(after, push::FRAME_WAIT) {
        Some(f) => with_type(Response::from_data(f.bytes.to_vec()), "application/octet-stream")
            .with_header(header("Cache-Control", "no-store"))
            .with_header(header("X-State-Version", hub.state_version())),
        None => text(503, "aucune image du moteur"),
    };
    if let Err(e) = request.respond(response) {
        log::debug!("failed to send a frame: {e}");
    }
    // Held until sent: a client that doesn't read keeps its slot.
    drop(slot);
}

/// The threads serving `/api/frame.bin`. Each page asks about 30 times a
/// second: a thread stays `FRAME_THREAD_IDLE` for the next request instead
/// of one thread per request. A new one starts whenever none is free, so a
/// client that doesn't read never holds up the others.
#[derive(Default)]
struct FramePool {
    jobs: Mutex<FrameJobs>,
    ready: std::sync::Condvar,
}

#[derive(Default)]
struct FrameJobs {
    queue: std::collections::VecDeque<(Request, push::OwnedSlot)>,
    idle: usize,
    closed: bool,
}

const FRAME_THREAD_IDLE: Duration = Duration::from_secs(5);

impl FramePool {
    fn submit(self: &Arc<Self>, request: Request, slot: push::OwnedSlot, hub: &Arc<Hub>) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.queue.push_back((request, slot));
        if jobs.idle >= jobs.queue.len() {
            drop(jobs);
            self.ready.notify_one();
            return;
        }
        drop(jobs);
        let (pool, hub) = (Arc::clone(self), Arc::clone(hub));
        let spawned = std::thread::Builder::new().name("http-frame".into()).stack_size(PUSH_STACK).spawn(move || pool.work(&hub));
        if let Err(e) = spawned {
            // The request waits for the next thread (or is answered 500 at shutdown).
            log::warn!("no thread for a frame request: {e}");
        }
    }

    fn work(&self, hub: &Hub) {
        loop {
            let job = {
                let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(job) = jobs.queue.pop_front() {
                        break Some(job);
                    }
                    if jobs.closed {
                        break None;
                    }
                    jobs.idle += 1;
                    let (guard, waited) = self.ready.wait_timeout(jobs, FRAME_THREAD_IDLE).unwrap_or_else(|e| e.into_inner());
                    jobs = guard;
                    jobs.idle -= 1;
                    if waited.timed_out() && jobs.queue.is_empty() {
                        break None;
                    }
                }
            };
            let Some((request, slot)) = job else { return };
            send_frame(request, slot, hub);
        }
    }

    /// The server stops: idle threads end, queued requests are dropped (500).
    fn close(&self) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.closed = true;
        jobs.queue.clear();
        drop(jobs);
        self.ready.notify_all();
    }
}

fn header(name: &str, value: impl std::fmt::Display) -> Header {
    Header::from_bytes(name.as_bytes(), value.to_string().as_bytes()).expect("ASCII header")
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
fn emergency_stop(request: Request, estop: &EStop, shared: &Mutex<Shared>, hub: &Hub) {
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
    // Binary frames after this one were finished after the latch, so they
    // show the stop (T-411). One more than the last published: the engine
    // may be publishing a frame it finished just before.
    let response = ok().with_header(header("X-Frame-Seq", hub.seq().wrapping_add(1)));
    if let Err(e) = request.respond(response) {
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
    // An edit (T-285): the sections it may change, before and after, make
    // one history entry if it succeeded and changed something.
    if let Some(sections) = (method == Method::Post).then(|| crate::history::sections_for(&path)).flatten() {
        let _edit = crate::history::edit_lock();
        let before = crate::history::snapshot(&shared.lock().unwrap(), sections);
        let response = dispatch(request, shared, calibration_path, method, &path);
        if (200..300).contains(&response.status_code().0) {
            crate::history::record(&mut shared.lock().unwrap(), &path, before);
        }
        return response;
    }
    dispatch(request, shared, calibration_path, method, &path)
}

fn dispatch(request: &mut Request, shared: &Arc<Mutex<Shared>>, calibration_path: &Path, method: Method, path: &str) -> HttpResponse {
    let path = path.to_string();
    match (method, path.as_str()) {
        (Method::Get, "/api/history") => json_response(shared.lock().unwrap().history.view()),
        // Undo / redo (T-285): `{}` one step, `{"to": id}` down to that
        // entry (the history panel). Refused in show mode (an edit).
        (Method::Post, "/api/undo") | (Method::Post, "/api/redo") => {
            let to = match body::<serde_json::Value>(request) {
                Ok(v) => v.get("to").and_then(serde_json::Value::as_u64),
                Err(_) => None,
            };
            match crate::history::step(shared, path == "/api/redo", to) {
                Ok(v) => json_response(v),
                Err((code, msg)) => text(code, &msg),
            }
        }
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
        // « Préparer » (T-274): the prepared cue's own frame, rendered here
        // on its own animator; never the output's (`/api/frame`).
        (Method::Get, "/api/preview-frame") => {
            let mut s = shared.lock().unwrap();
            let round = |v: f32| (v * 1000.0).round() / 1000.0;
            let points: Vec<[f32; 5]> = crate::prepared::render(&mut s).iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect();
            json_response(json!({ "prepared": crate::prepared::view(&s), "points": points }))
        }
        (Method::Post, "/api/prepared") => match body::<IdRequest>(request) {
            Ok(req) => prepared_reply(shared, |s| crate::prepared::prepare(s, &req.id)),
            Err(e) => e,
        },
        (Method::Post, "/api/prepared/adjust") => match body::<crate::prepared::Adjust>(request) {
            Ok(req) => prepared_reply(shared, |s| crate::prepared::adjust(s, &req)),
            Err(e) => e,
        },
        (Method::Post, "/api/prepared/send") => prepared_reply(shared, |s| crate::prepared::send(s).map(|_| ())),
        (Method::Post, "/api/prepared/clear") => prepared_reply(shared, |s| {
            crate::prepared::clear(s);
            Ok(())
        }),
        // Notification centre (T-432): read only, `since` = the last id seen.
        (Method::Get, "/api/notices") => {
            let since = query_u64(request.url(), "since").unwrap_or(0);
            let mut s = shared.lock().unwrap();
            s.sync_notices();
            json_response(json!({ "notices": s.notices.since(since), "last": s.notices.last() }))
        }
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
        // First launch (T-437): whether the guided tour was seen. A view
        // preference: never arms, never touches an output.
        (Method::Get, "/api/onboarding") => json_response(onboarding_view(&shared.lock().unwrap())),
        (Method::Post, "/api/onboarding") => match body::<OnboardingRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match s.onboarding.set_tour_done(req.tour_done) {
                    Ok(()) => json_response(onboarding_view(&s)),
                    Err(e) => text(500, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        // Built-in help (T-438): pages compiled into the binary, offline.
        (Method::Get, "/api/help") => {
            let full = query_u64(request.url(), "full") == Some(1);
            json_response(json!(crate::help::PAGES.iter().map(|p| help_view(p, full)).collect::<Vec<_>>()))
        }
        (Method::Get, p) if p.starts_with("/api/help/") => match crate::help::page(&p["/api/help/".len()..]) {
            Some(page) => json_response(help_view(page, true)),
            None => text(404, "page d'aide inconnue"),
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
        (Method::Get, "/api/test/menu") => {
            // `--test-hooks` with `--app` (T-413): the menu-bar item's lines.
            let actions = {
                let s = shared.lock().unwrap();
                if !s.test_hooks {
                    return text(404, "not found");
                }
                s.menu.clone()
            };
            match actions {
                Some(a) => json_response(crate::menubar::current_model(shared, &a.estop).to_json()),
                None => text(409, "pas en mode application (--app)"),
            }
        }
        (Method::Post, "/api/test/menu") => {
            // `--test-hooks` with `--app` (T-413): an entry of the
            // menu-bar item, `?action=open|estop|quit`, as if clicked.
            let actions = {
                let s = shared.lock().unwrap();
                if !s.test_hooks {
                    return text(404, "not found");
                }
                s.menu.clone()
            };
            let Some(actions) = actions else { return text(409, "pas en mode application (--app)") };
            match query_str(request.url(), "action").as_deref().and_then(crate::menubar::parse_action) {
                Some(action) => {
                    actions.perform(action, shared);
                    ok()
                }
                None => text(400, "action : open, estop ou quit"),
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
        (Method::Post, "/api/test/timecode") => {
            // `--test-hooks` only (e2e of T-409): ArtTimeCode packets built
            // by our encoder, `count` consecutive frames from `frames`,
            // handed over as the Art-Net listener would (no socket).
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<SimTimecode>(request) {
                Ok(sim) => {
                    let s = shared.lock().unwrap();
                    let mut accepted = 0;
                    for k in 0..sim.count.clamp(1, 100) {
                        let tc = crate::timecode::Tc::from_frames(sim.frames + k as i64, sim.rate);
                        match crate::timecode::inject_artnet(&s, &crate::timecode::artnet::encode(tc, sim.rate)) {
                            Ok(true) => accepted += 1,
                            Ok(false) => {}
                            Err(e) => return text(400, &e),
                        }
                    }
                    json_response(json!({ "accepted": accepted }))
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
                let response = if want {
                    match s.request_arm(source) {
                        Ok(()) => json_response(json!({ "armed": s.gate.is_armed() })),
                        Err(blocking) => with_status(json_response(json!({ "armed": false, "blocking": blocking })), 409),
                    }
                } else {
                    // Disarming is always accepted, whatever else the body says.
                    s.gate.disarm(DisarmReason::User, source);
                    json_response(json!({ "armed": false }))
                };
                // The last frame published before this change (T-411): a
                // binary frame up to this one may still show the old state.
                response.with_header(header("X-Frame-Seq", s.push.seq()))
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
        // Safety profiles per venue (T-260). Locked in show mode (423,
        // not in `showmode::allowed`); never recorded by undo.
        (Method::Get, "/api/safety/profiles") => json_response(crate::profiles::view(&shared.lock().unwrap())),
        // `{action: "save", name, venue?, notes?, audience_scan_allowed?,
        // confirm_loosen?}` (« Enregistrer sous… »: the settings in force,
        // which becomes the active profile), `{action: "verify", name}`,
        // `{action: "delete", name}`. « Aperçu sûr » is refused (403).
        (Method::Post, "/api/safety/profiles") => match body::<ProfileRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
                if !crate::profiles::operator_source(src) {
                    return text(403, &crate::profiles::LoadError::Source.message_fr());
                }
                if crate::profiles::is_builtin(&req.name) {
                    return text(403, &format!("« {} » est intégré : il ne peut être ni modifié ni supprimé", crate::profiles::SAFE_PREVIEW));
                }
                let done = match req.action.as_str() {
                    "save" => {
                        let save = crate::profiles::SaveRequest { name: req.name.clone(), venue: req.venue, notes: req.notes, audience_scan_allowed: req.audience_scan_allowed };
                        crate::profiles::save(&mut s, save, req.confirm_loosen, src).map(|_| ())
                    }
                    "verify" => crate::profiles::verify(&mut s, &req.name, crate::safety_log::today()).map(|_| ()).map_err(|e| crate::profiles::LoadError::Damaged(e.to_string())),
                    "delete" => crate::profiles::delete(&mut s, &req.name).map_err(|e| crate::profiles::LoadError::Damaged(e.to_string())),
                    _ => return text(400, "expected \"action\": \"save\", \"verify\" or \"delete\""),
                };
                match done {
                    Ok(()) => json_response(crate::profiles::view(&s)),
                    Err(e) => profile_error(e),
                }
            }
            Err(e) => e,
        },
        // `{name, confirm_loosen?, source?}`: disarms (« Changement de
        // profil »), restarts the checklist. Looser than what is in force:
        // 409 `{error, loosen}` without `confirm_loosen` (sent only by the
        // operator's confirmation), nothing changed. Unknown (404) or
        // damaged (400): disarmed, settings in force kept. Not from the
        // operator (MIDI, OSC, DMX...): 403.
        (Method::Post, "/api/safety/profiles/load") => match body::<ProfileLoadRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
                match crate::profiles::load(&mut s, &req.name, req.confirm_loosen, src) {
                    Ok(_) => json_response(crate::profiles::view(&s)),
                    Err(e) => profile_error(e),
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
        // Exposure estimator (T-257): MPE and NOHD from the projector sheet,
        // the caps and the audience distance. Informative only: never
        // changes a cap, the output or the arming. `?output=` picks the
        // output (main by default).
        (Method::Get, "/api/safety/exposure") => {
            let s = shared.lock().unwrap();
            match color_output(request.url(), &s) {
                Ok(id) => json_response(exposure_view(&s, &id)),
                Err(e) => e,
            }
        }
        // `{output?, exposure: {distance_m, attenuation_pct}}`: saved with
        // the output. An edit, locked in show mode.
        (Method::Post, "/api/safety/exposure") => match body::<ExposureBody>(request) {
            Ok(b) => {
                let mut s = shared.lock().unwrap();
                let id = b.output.unwrap_or_else(|| crate::power::MAIN_OUTPUT.into());
                match s.outputs.set_exposure(&id, b.exposure) {
                    Ok(_) => json_response(exposure_view(&s, &id)),
                    Err(e) => text(400, &e.to_string()),
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
        (Method::Get, "/api/outputs/scan_limits") => {
            let s = shared.lock().unwrap();
            match color_output(request.url(), &s) {
                Ok(id) => json_response(scan_limits_view(&s, &id)),
                Err(e) => e,
            }
        }
        // Scan speed range and points per frame of one output (T-171),
        // `?output=<id or index>` (default the main output). Body:
        // `{scan}` (a missing field of `scan` is its default). Out of
        // range is 400; a faster `pps_max` is 409 `{error, loosen, armed:
        // true}` while the laser is armed. Refused in show mode (not on
        // the allow-list).
        (Method::Post, "/api/outputs/scan_limits") => {
            let id = match color_output(request.url(), &shared.lock().unwrap()) {
                Ok(id) => id,
                Err(e) => return e,
            };
            match body::<ScanLimitsBody>(request) {
                Ok(b) => {
                    let mut s = shared.lock().unwrap();
                    let s = &mut *s;
                    s.gate.sync_estop(&s.estop);
                    let armed = s.gate.is_armed();
                    let before = s.outputs.get(&id).map(|o| o.scan);
                    match s.outputs.set_scan(&id, b.scan, armed) {
                        Ok(out) => {
                            if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out.scan)) {
                                change["output"] = json!(id);
                                s.gate.log().record("output_scan", Some("ui"), change);
                            }
                            json_response(scan_limits_view(s, &id))
                        }
                        Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                        Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) => with_status(
                            json_response(json!({ "error": "Désarmez le laser pour relever la vitesse max", "loosen": list, "armed": true })),
                            409,
                        ),
                    }
                }
                Err(e) => e,
            }
        }
        (Method::Get, "/api/testpattern") =>json_response(test_pattern_view(&shared.lock().unwrap())),
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
        (Method::Get, "/api/venue") => json_response(venue_view(&shared.lock().unwrap())),
        (Method::Post, "/api/venue") => match body::<crate::venue::Venue>(request) {
            Ok(venue) => set_venue(venue, shared),
            Err(e) => e,
        },
        // A room preset: the room is resized, the projectors stay.
        (Method::Post, "/api/venue/preset") => match body::<NameRequest>(request) {
            Ok(req) => {
                let mut venue = shared.lock().unwrap().venue.get().clone();
                if !venue.apply_preset(&req.name) {
                    return text(400, &format!("Préréglage inconnu : « {} »", req.name));
                }
                set_venue(venue, shared)
            }
            Err(e) => e,
        },
        (Method::Get, "/api/frames") => frames(shared),
        (Method::Get, "/api/projection") => json_response(projection_view(&shared.lock().unwrap())),
        // Zone groups and their masters (T-420): levels move through
        // `/api/control` (`group.<n>.master`), never through here.
        (Method::Get, "/api/groups") => json_response(groups_view(&shared.lock().unwrap())),
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
                    // The « Festival » page's moment of the track (T-110).
                    if let Some(tag) = p.tag {
                        v["tag"] = json!(tag);
                    }
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
        // Animated grid thumbnails (T-273), for the cues of the page on screen.
        (Method::Get, "/api/presets/loops") => {
            let Some(ids) = query_str(request.url(), "ids") else { return text(400, "expected ?ids=<cue>,<cue>…") };
            let looks: Vec<_> = {
                let s = shared.lock().unwrap();
                ids.split(',')
                    .filter(|id| !id.is_empty())
                    .take(MAX_LOOP_IDS)
                    .filter_map(|id| s.presets.iter().find(|p| p.id == id).map(|p| (p.id.clone(), s.deck.with_program(id, p.settings.clone()))))
                    .collect()
            };
            json_response(cue_loops(looks))
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
        // The grid's distinct tags (T-430), for the filter bar and the
        // properties menu's suggestions.
        (Method::Get, "/api/cues/tags") => {
            let tags = shared.lock().unwrap().deck.tags();
            json_response(json!(tags.into_iter().map(|(tag, count)| json!({ "tag": tag, "count": count })).collect::<Vec<_>>()))
        }
        (Method::Post, "/api/cues/slot") => match body::<SlotRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                if !s.presets.iter().any(|p| p.id == req.id) {
                    return text(404, "no such preset");
                }
                // The program and the stored modifiers (T-144) are set on
                // their own routes: the properties dialog never sends them,
                // and keeps them.
                if req.slot.speed_group.is_some_and(|g| crate::speed::check_group(g).is_err()) {
                    return text(400, "groupe de vitesse inconnu (0 à 4)");
                }
                let CueSlot { program, modifiers, .. } = s.deck.slot(&req.id);
                s.deck.set_slot(&req.id, CueSlot { program, modifiers, ..req.slot });
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
        // « Enregistrer dans le cue » (T-144): the cue level's working
        // copy (with the master composed in when `with_master`) into the
        // cell; `clear` empties the cell's. An edit: in the history,
        // refused in show mode.
        (Method::Post, "/api/cues/modifiers") => match body::<CueModifiersRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                if req.clear {
                    if !s.presets.iter().any(|p| p.id == req.id) {
                        return text(404, "cue inconnu");
                    }
                    let slot = s.deck.slot(&req.id);
                    s.deck.set_slot(&req.id, CueSlot { modifiers: Default::default(), ..slot });
                    s.deck.save();
                    return ok();
                }
                match crate::levels::store(&mut s, &req.id, req.with_master) {
                    Ok(()) => ok(),
                    Err((code, why)) => text(code, &why),
                }
            }
            Err(e) => e,
        },
        // CRÉATION › Cue évolutif (T-164): what the editor opens on - the
        // cue's program (the cell's, else its look's), its main content
        // (also how a catalogue cue becomes an inner layer), the length
        // without a program, and what curves and LFOs may move.
        (Method::Get, "/api/cues/program") => {
            let Some(id) = query_str(request.url(), "id") else { return text(400, "expected ?id=<cue>") };
            let s = shared.lock().unwrap();
            let Some(p) = s.presets.iter().find(|p| p.id == id) else { return text(404, "no such preset") };
            let own = s.deck.program(&id).is_some();
            let program = s.deck.program(&id).or(p.settings.program.as_ref());
            let targets: Vec<_> = crate::program::TARGETS.iter().map(|&(id, min, max, default)| json!({ "id": id, "min": min, "max": max, "default": default })).collect();
            json_response(json!({
                "id": p.id, "name": p.name, "category": p.category, "user": s.user_cues.get(&p.id).is_some(),
                "program": program, "own": own,
                "content": p.settings.content,
                "content_beats": crate::program::CueProgram::default().length(&p.settings.content),
                "targets": targets,
                "spin_targets": ["rot_x.speed", "rot_y.speed", "rot_z.speed"],
                "max": { "layers": crate::program::MAX_LAYERS, "lanes": crate::program::MAX_LANES, "keys": crate::program::MAX_KEYS, "lfos": crate::program::MAX_LFOS },
            }))
        }
        // The editor's live preview (T-164): cue `id` played with an unsaved
        // program at the current tempo. Display only, like the hover
        // preview: nothing is stored, nothing reaches the output.
        (Method::Post, "/api/cues/program/preview") => match body::<ProgramRequest>(request) {
            Ok(req) => {
                if let Some(Err(e)) = req.program.as_ref().map(crate::program::CueProgram::validate) {
                    return text(400, &e.to_string());
                }
                let (mut look, bpm, beats_per_bar) = {
                    let s = shared.lock().unwrap();
                    let Some(p) = s.presets.iter().find(|p| p.id == req.id) else { return text(404, "no such preset") };
                    (p.settings.clone(), s.tempo.bpm, s.tempo.beats_per_bar)
                };
                look.program = req.program;
                json_response(json!(crate::presets::preview_with(&look, bpm, beats_per_bar, crate::presets::EDITOR_PREVIEW_FRAMES, 0.125)))
            }
            Err(e) => e,
        },
        // « Enregistrer dans une case » (T-164): a new user cue of cue `id`'s
        // look with the program, in the first free cell of a user page.
        (Method::Post, "/api/cues/program/store") => match body::<ProgramRequest>(request) {
            Ok(req) => {
                let Some(program) = req.program else { return text(400, "programme vide") };
                if let Err(e) = program.validate() {
                    return text(400, &e.to_string());
                }
                let mut s = shared.lock().unwrap();
                match crate::usercues::store_program(&mut s, &req.id, program) {
                    Ok((id, page, slot)) => {
                        let mut g = grid_json(&s);
                        g["cue"] = json!(id);
                        g["cell"] = json!([page, slot]);
                        json_response(g)
                    }
                    Err((code, msg)) => text(code, &msg),
                }
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
                let s = &mut *s;
                // The masters this whole object changes are the page's (T-423).
                let masters = |s: &Shared| -> Vec<(String, Option<serde_json::Value>)> {
                    s.controls.list().iter().filter(|d| d.group == "master").map(|d| (d.id.clone(), controls::current(s, d))).collect()
                };
                let before = masters(s);
                s.live = live;
                s.live_dirty = true;
                let at_s = s.now_s();
                for ((id, was), (_, now)) in before.into_iter().zip(masters(s)) {
                    if was != now {
                        s.touches.insert(id, controls::Touch { source: controls::ControlSource::Ui, at_s });
                    }
                }
                ok()
            }
            Err(e) => e,
        },
        (Method::Get, "/api/festival") => json_response(festival_view(&shared.lock().unwrap())),
        // Latency compensation (T-246): the *Décalage de sortie*, the
        // analysis delay and the output's indicative latency.
        (Method::Get, "/api/latency") => json_response(crate::latency::view(&shared.lock().unwrap())),
        // `{ "output_offset_ms": -100..100 }`: clamped, a non-number refused.
        // Only what the effects draw moves; never arming nor a frame's timing.
        (Method::Post, "/api/latency") => match body::<LatencySet>(request) {
            Ok(set) => {
                let mut s = shared.lock().unwrap();
                match crate::latency::set_offset(&mut s, set.output_offset_ms) {
                    Ok(_) => json_response(crate::latency::view(&s)),
                    Err(e) => text(400, e),
                }
            }
            Err(e) => e,
        },
        // A festival palette (T-130): `{ "palette": 0..7 }`, or `null` for
        // the looks' own colours. Lands on the configured boundary.
        (Method::Post, "/api/festival/palette") => match body::<FestivalPick>(request) {
            Ok(pick) => {
                let mut s = shared.lock().unwrap();
                let t = s.now_s();
                let (beat, bpb) = (s.tempo.effect_beat_at(t), s.tempo.beats_per_bar);
                match s.festival.request(pick.palette, beat, bpb) {
                    Ok(()) => json_response(festival_view(&s)),
                    Err(e) => text(400, &e.to_string()),
                }
            }
            Err(e) => e,
        },
        // Quantisation and perceived balance: `{ "quantize": "bar", "balance": { … } }`.
        (Method::Post, "/api/festival") => match body::<FestivalConfig>(request) {
            Ok(cfg) => {
                let mut s = shared.lock().unwrap();
                s.festival.configure(cfg.quantize, cfg.balance);
                json_response(festival_view(&s))
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
        // Scan presets (T-415). POST takes the user list (a bare array) or
        // `{ "user": [...], "default": "<id>" }` ("" = « Auto »).
        (Method::Get, "/api/scan-presets") => json_response(scan_presets_view(&shared.lock().unwrap())),
        (Method::Post, "/api/scan-presets") => match body::<ScanPresetsRequest>(request) {
            Ok(req) => {
                let (user, default) = match req {
                    ScanPresetsRequest::List(list) => (Some(list), None),
                    ScanPresetsRequest::Edit { user, default } => (user, default),
                };
                let mut s = shared.lock().unwrap();
                if let Some(list) = user {
                    if let Err(e) = s.scan_presets.set(list) {
                        return text(400, &e.to_string());
                    }
                }
                if let Some(id) = default {
                    let id = id.trim().to_string();
                    if !id.is_empty() && !s.scan_presets.is_known(&id) {
                        return text(400, &format!("préréglage de balayage inconnu : « {id} »"));
                    }
                    s.mixer.scan_default = (!id.is_empty()).then_some(id);
                    s.mixer_dirty = true;
                }
                json_response(scan_presets_view(&s))
            }
            Err(e) => e,
        },
        // Speed groups A..D and the *Cadence* (T-419). POST: any of
        // `groups` (4 modes, `null` = unchanged), `group` + `mode` (one),
        // `rate`, `change_on_bar`; checked (NaN, unknown multiples and
        // groups refused), a new multiple waits for its bar when asked.
        (Method::Get, "/api/speed-groups") => {
            let s = shared.lock().unwrap();
            json_response(s.speed.view(s.tempo.bpm))
        }
        (Method::Post, "/api/speed-groups") => match body::<SpeedGroupsRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match apply_speed_groups(&mut s, req) {
                    Ok(()) => {
                        s.speed_dirty = true;
                        json_response(s.speed.view(s.tempo.bpm))
                    }
                    Err(e) => text(400, e),
                }
            }
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
        // The FX grid (T-146): the cells, what plays, the targets a cell's
        // modulators may use, our starter effects. Playing goes through
        // `/api/control` (`fx.*`); POST replaces the cells (an edit:
        // recorded in the history, locked in show mode).
        // The executor bank (T-418): GET the pages and what the hands hold;
        // POST replaces the pages (an edit: in the history, locked in show
        // mode). Playing goes through `/api/control` (`exec.*`).
        (Method::Get, "/api/executors") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "pages": s.executors.bank.pages, "state": crate::executors::view(&s), "per_page": crate::executors::EXEC_PER_PAGE, "max_pages": crate::executors::EXEC_MAX_PAGES }))
        }
        (Method::Post, "/api/executors") => match body::<crate::executors::ExecutorBank>(request) {
            Ok(bank) => {
                let s = &mut *shared.lock().unwrap();
                match bank.validate(|id| s.presets.iter().any(|p| p.id == id)) {
                    Ok(()) => {
                        s.executors.set_bank(bank);
                        s.executors.save();
                        json_response(json!({ "pages": s.executors.bank.pages, "state": crate::executors::view(s) }))
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        // The custom desk (T-427): GET the tabs; POST replaces them all (an
        // edit: in the history, locked in show mode; 400 on an overlap, a
        // bound outside the grid, or a binding to arming or safety).
        // Playing goes through `/api/control` and `/api/cue`.
        (Method::Get, "/api/desks") => json_response(desks_view(&shared.lock().unwrap())),
        (Method::Post, "/api/desks") => match body::<DesksRequest>(request) {
            Ok(req) => {
                let tabs = match req {
                    DesksRequest::Wrapped { tabs } | DesksRequest::Bare(tabs) => tabs,
                };
                let s = &mut *shared.lock().unwrap();
                match crate::desk::validate(&tabs, &s.controls) {
                    Ok(()) => {
                        s.desks.set_tabs(tabs);
                        s.desks.save();
                        json_response(desks_view(s))
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/fx") => {
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
            json_response(json!({
                "cells": s.fx.grid().cells(),
                "state": s.fx.view(),
                "targets": targets,
                "starter": crate::fx::starter_grid().cells(),
                "max_modulators": crate::fx::MAX_CELL_MODULATORS,
            }))
        }
        (Method::Post, "/api/fx") => match body::<FxCellsBody>(request) {
            Ok(b) => {
                let s = &mut *shared.lock().unwrap();
                match s.fx.set_cells(b.cells, &s.controls) {
                    Ok(()) => ok(),
                    Err(e) => text(400, &format!("{e:#}")),
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
                // « En cours » credits the change to the page (T-423).
                match controls::apply_from(&mut shared.lock().unwrap(), &req.id, input, controls::ControlSource::Ui) {
                    Ok(()) => ok(),
                    Err(controls::ControlError::Unknown(id)) => text(404, &format!("contrôle inconnu : {id}")),
                    Err(controls::ControlError::Refused(why)) => text(403, why),
                }
            }
            Err(e) => e,
        },
        // The command line (T-424): one line, parsed then run like a
        // controller (nothing that arms). Nothing is stored here.
        (Method::Post, "/api/command") => match body::<CommandRequest>(request) {
            Ok(req) => match crate::command::run(&mut shared.lock().unwrap(), &req.line, controls::ControlSource::Command) {
                Ok(echo) => json_response(json!({ "ok": true, "echo": echo })),
                Err(e) => with_status(json_response(json!(e)), 400),
            },
            Err(e) => e,
        },
        // The palette's list, with each control's value now.
        (Method::Get, "/api/command/actions") => {
            let s = shared.lock().unwrap();
            let actions: Vec<serde_json::Value> = crate::command::actions(&s.controls, &s.presets)
                .into_iter()
                .map(|a| {
                    let value = a.id.as_deref().and_then(|id| s.controls.get(id)).and_then(|d| controls::current(&s, d).and_then(|v| crate::command::value_fr(d, &v)));
                    let mut v = json!(a);
                    if let Some(value) = value {
                        v["value_fr"] = json!(value);
                    }
                    v
                })
                .collect();
            json_response(json!({ "actions": actions }))
        }
        // « En cours » (T-423): what plays and who drove it; the stop buttons.
        (Method::Get, "/api/running") => json_response(crate::running::snapshot(&shared.lock().unwrap())),
        (Method::Post, "/api/running/stop") => match body::<crate::running::StopRequest>(request) {
            Ok(req) => match crate::running::stop(&mut shared.lock().unwrap(), &req) {
                Ok(()) => ok(),
                Err(why) => text(400, &why),
            },
            Err(e) => e,
        },
        // Cue lists (T-410): editing (locked in show mode) and the transport.
        (Method::Get, "/api/cuelists") => json_response(crate::cuelist::view(&shared.lock().unwrap())),
        (Method::Post, "/api/cuelists") => match body::<crate::cuelist::CueList>(request) {
            Ok(list) => {
                let mut s = shared.lock().unwrap();
                match crate::cuelist::save(&mut s, list) {
                    Ok(()) => json_response(crate::cuelist::view(&s)),
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/cuelists/delete") => match body::<NameRequest>(request) {
            Ok(req) => match crate::cuelist::delete(&mut shared.lock().unwrap(), &req.name) {
                Ok(true) => ok(),
                Ok(false) => text(404, "liste de cues inconnue"),
                Err(e) => text(500, &format!("{e:#}")),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/cuelist/load") => match body::<NameRequest>(request) {
            Ok(req) => match crate::cuelist::load(&mut shared.lock().unwrap(), &req.name) {
                Ok(()) => ok(),
                Err(why) => text(404, why),
            },
            Err(e) => e,
        },
        (Method::Post, p) if p.starts_with("/api/cuelist/") => cuelist_transport(request, shared, &p["/api/cuelist/".len()..]),
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
        // The editor's text tool: a laser font as strokes. No state.
        (Method::Post, "/api/figures/text") => match body::<TextRequest>(request) {
            Ok(req) => {
                let text: String = req.text.chars().take(crate::fonts::MAX_TEXT_CHARS).collect();
                let size = if req.size.is_finite() { req.size.clamp(0.02, 2.0) } else { 0.3 };
                let font = crate::fonts::info(req.font.as_deref().unwrap_or("segments")).id;
                let strokes = if font == "segments" {
                    // As before T-439 (not cut at the frame's edges).
                    crate::fonts::text_strokes(&text.to_uppercase(), size)
                } else {
                    crate::fonts::layout(&text, &crate::fonts::TextStyle { font: font.into(), ..Default::default() }, size)
                };
                json_response(json!({ "strokes": strokes, "font": font }))
            }
            Err(e) => e,
        },
        // The laser fonts (T-439), « Segments » first.
        (Method::Get, "/api/fonts") => json_response(json!(crate::fonts::fonts())),
        (Method::Get, "/api/timeline") => {
            let s = shared.lock().unwrap();
            json_response(json!({ "state": s.timeline.state(&s.timeline_clock()), "show": s.timeline.show, "audio": song_view(&s) }))
        }
        // T-443: the loaded show's markers as CSV, and a CSV read without
        // touching the show (the page applies it through `edit`).
        (Method::Get, "/api/timeline/markers.csv") => export_markers(&shared.lock().unwrap()),
        (Method::Post, "/api/timeline/markers/parse") => parse_markers(request, shared),
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
        // Recording what plays, or a timeline, as an ILDA animation (T-442):
        // the work runs on its own thread (record.rs); the file lands in the
        // library. Starting is a file write: show mode locks it; cancelling
        // is not.
        (Method::Get, "/api/ilda/record") => {
            let s = shared.lock().unwrap();
            // `?show=<name>`: its length and whether it reacts to music
            // (an export renders the music neutral; the dialog says so).
            let show = query_str(request.url(), "show").and_then(|name| {
                let name = name.trim().to_string();
                s.timeline.show.as_ref().filter(|sh| sh.name == name).cloned().or_else(|| s.shows.load(&name).ok())
            });
            let show = show.map(|sh| json!({
                "name": sh.name,
                "seconds": crate::record::show_seconds(&sh, s.tempo.bpm),
                "audio": crate::record::uses_audio(&sh, &s.presets),
            }));
            json_response(json!({
                "show": show,
                "status": s.recorder.status(),
                "min_seconds": crate::record::MIN_SECONDS,
                "max_seconds": crate::record::MAX_SECONDS,
                "min_fps": crate::record::MIN_FPS,
                "max_fps": crate::record::MAX_FPS,
                "default_fps": crate::record::DEFAULT_FPS,
                "shownet_max_bytes": crate::ilda::SHOWNET_MAX_FILE_BYTES,
            }))
        }
        (Method::Post, "/api/ilda/record") => match body::<crate::record::RecordRequest>(request) {
            Ok(req) => match crate::record::start(shared, &req) {
                Ok(status) => json_response(json!(status)),
                Err((code, msg)) => text(code, &msg),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/ilda/record/cancel") => {
            let cancelled = shared.lock().unwrap().recorder.cancel();
            json_response(json!({ "cancelled": cancelled }))
        }
        (Method::Get, "/api/ilda/record/file") => download_ilda(request.url(), shared),
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
        // Built-in example shows (T-124 to T-129): opened with
        // `POST /api/timeline/load { "example": id }`.
        (Method::Get, "/api/timeline/examples") => json_response(json!(crate::examples::EXAMPLES)),
        (Method::Post, "/api/timeline/audio") => match body::<SongPatch>(request) {
            Ok(patch) => attach_song(patch, shared),
            Err(e) => e,
        },
        // « Générer un show depuis le morceau » (T-167): the job runs on
        // its own thread (autoshow.rs); these only start, read or cancel it.
        (Method::Get, "/api/timeline/autoshow") => {
            let hub = Arc::clone(&shared.lock().unwrap().autoshow);
            json_response(json!(hub.view()))
        }
        (Method::Post, "/api/timeline/autoshow") => match body::<AutoShowRequest>(request) {
            Ok(req) => match crate::autoshow::start(shared, req.file.trim(), req.seed) {
                Ok(view) => with_type(Response::from_string(json!(view).to_string()).with_status_code(202), "application/json"),
                Err((code, why)) => text(code, &why),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/timeline/autoshow/cancel") => {
            let hub = Arc::clone(&shared.lock().unwrap().autoshow);
            hub.cancel();
            json_response(json!(hub.view()))
        }
        // Modèles de timeline (T-165): list, save the selection, delete,
        // and render (computes the show with the phrase laid on a region;
        // the page sends it through /api/timeline/edit). Edits: locked in
        // show mode. Nothing here arms anything.
        (Method::Get, "/api/timeline/templates") => {
            let s = shared.lock().unwrap();
            let categories: Vec<&str> = crate::presets::CATEGORIES.iter().copied().filter(|c| s.presets.iter().any(|p| p.category == *c)).collect();
            json_response(json!({ "templates": s.templates.all(), "categories": categories }))
        }
        (Method::Post, "/api/timeline/templates") => match body::<TemplateSave>(request) {
            Ok(req) => save_template(req, shared),
            Err(e) => e,
        },
        (Method::Post, "/api/timeline/templates/delete") => match body::<TemplateSave>(request) {
            Ok(req) => match shared.lock().unwrap().templates.delete(&req.name) {
                Ok(()) => ok(),
                Err(e) => text(404, &format!("{e:#}")),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/timeline/templates/render") => match body::<TemplateRender>(request) {
            Ok(req) => render_template(req, shared),
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
        (method, p) if p == "/api/timecode" || p == "/api/timecode/arm" => timecode_route(request, shared, method, p),
        (method, p) if p == "/api/project" || p.starts_with("/api/project/") => project_route(request, shared, method == Method::Post, p),
        // The tablet remote (T-428): turned on, watched and cut from here
        // only (this server listens on 127.0.0.1); the remote's own
        // server (remote.rs) never reaches these routes.
        (Method::Get, "/api/remote") => json_response(crate::remote::view(&shared.lock().unwrap(), Instant::now())),
        (Method::Post, "/api/remote") => {
            let local = request.remote_addr().is_some_and(|a| a.ip().is_loopback());
            let req = match body::<RemoteRequest>(request) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if !local {
                return text(403, "la télécommande ne s'autorise que depuis l'interface de ce Mac");
            }
            // Show mode: cutting the access is allowed, opening it is not.
            if req.enabled && shared.lock().unwrap().show_mode.on {
                return locked();
            }
            match crate::remote::set_enabled(shared, req.enabled, req.port) {
                Ok(()) => json_response(crate::remote::view(&shared.lock().unwrap(), Instant::now())),
                Err(e) => text(409, &e),
            }
        }
        (Method::Post, "/api/remote/kick") => match body::<RemoteKick>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let n = crate::remote::kick(&mut s, req.name.as_deref());
                let mut v = crate::remote::view(&s, Instant::now());
                v["kicked"] = json!(n);
                json_response(v)
            }
            Err(e) => e,
        },
        // The copy to a second location (T-450): settings, and « Copier maintenant ».
        (Method::Get, "/api/backup") => json_response(shared.lock().unwrap().project.backup.view()),
        (Method::Post, "/api/backup") => match body::<crate::backup::SettingsRequest>(request) {
            Ok(req) => {
                let hub = std::sync::Arc::clone(&shared.lock().unwrap().project.backup);
                match crate::backup::set_settings(&hub, req) {
                    Ok(_) => json_response(hub.view()),
                    Err((code, t)) => text(code, &t),
                }
            }
            Err(r) => r,
        },
        (Method::Post, "/api/backup/now") => {
            let hub = std::sync::Arc::clone(&shared.lock().unwrap().project.backup);
            if hub.settings().active_dir().is_none() {
                text(409, "aucun dossier de copie : activez la copie de sauvegarde dans RÉGLAGES")
            } else {
                // Serialised here (lock released); written by the copy thread.
                let p = crate::project::current_project(shared);
                match crate::project::project_bytes(&p) {
                    Ok(bytes) => {
                        hub.queue(crate::backup::Job::project(&p.name, bytes));
                        json_response(hub.view())
                    }
                    Err(e) => text(500, &format!("{e:#}")),
                }
            }
        }
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
            let key = look_key(settings);
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

/// What a cue picture is drawn from: the look, and for an ILDA cue its
/// file's load stamp (drawn again once the file is (re)loaded).
fn look_key(settings: &Settings) -> String {
    let mut key = serde_json::to_string(settings).unwrap_or_default();
    if let crate::engine::Content::Ilda { file, .. } = &settings.content {
        key += &crate::ilda_library::clip(file).map_or(0, |c| c.stamp).to_string();
    }
    key
}

/// Most cues asked for in one `/api/presets/loops` (a 12 × 8 page).
const MAX_LOOP_IDS: usize = 96;

/// Animated grid thumbnails (T-273): the loops drawn, by cue id with the
/// look they were drawn from, and the ids waiting for the drawing thread.
#[derive(Default)]
struct Loops {
    done: std::collections::HashMap<String, (String, Arc<crate::presets::Preview>)>,
    queued: std::collections::HashSet<String>,
}
static LOOPS: Mutex<Option<Loops>> = Mutex::new(None);
/// The drawing thread's queue, started on the first request.
static LOOP_JOBS: std::sync::OnceLock<std::sync::mpsc::Sender<(String, String, Settings)>> = std::sync::OnceLock::new();

/// One drawing thread for every loop: never the engine's nor the HTTP
/// worker's time. A look that fails to draw gets an empty loop (the UI
/// keeps its still picture) rather than staying queued forever.
fn draw_loops(jobs: std::sync::mpsc::Receiver<(String, String, Settings)>) {
    for (id, key, settings) in jobs {
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::presets::thumb_loop(&settings)))
            .unwrap_or_else(|_| crate::presets::Preview { step_beats: crate::presets::LOOP_BEATS, frames: Vec::new() });
        let mut loops = LOOPS.lock().unwrap();
        let loops = loops.get_or_insert_with(Default::default);
        loops.queued.remove(&id);
        loops.done.insert(id, (key, Arc::new(drawn)));
    }
}

/// `GET /api/presets/loops?ids=a,b`: the loops already drawn, at once;
/// the others are queued for the drawing thread and listed as `pending`,
/// for the page to ask again. Only a cache lookup on the HTTP worker.
fn cue_loops(looks: Vec<(String, Settings)>) -> serde_json::Value {
    let jobs = LOOP_JOBS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("thumb-loops".into()).spawn(move || draw_loops(rx)) {
            log::warn!("no thread for the animated thumbnails: {e}");
        }
        tx
    });
    let mut out = serde_json::Map::new();
    let mut pending = Vec::new();
    let mut loops = LOOPS.lock().unwrap();
    let loops = loops.get_or_insert_with(Default::default);
    for (id, settings) in looks {
        let key = look_key(&settings);
        match loops.done.get(&id) {
            Some((k, drawn)) if *k == key => {
                out.insert(id, json!(**drawn));
            }
            _ => {
                if loops.queued.insert(id.clone()) && jobs.send((id.clone(), key, settings)).is_err() {
                    loops.queued.remove(&id);
                    continue; // no drawing thread: the still picture stays
                }
                pending.push(id);
            }
        }
    }
    json!({ "loops": out, "pending": pending })
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

/// `GET /api/timeline/markers.csv` (T-443): `#,Name,Start,Color`, times in
/// seconds whatever the show's unit, downloaded as `<show>-marqueurs.csv`.
fn export_markers(s: &Shared) -> HttpResponse {
    use crate::timeline::TimeBase;
    let Some(show) = s.timeline.show.as_ref() else { return text(409, "aucun show chargé") };
    let csv = crate::timeline_csv::export(&show.markers, |at| if show.time_base == TimeBase::Beats { show.s_at_beat(at) } else { at });
    // The header is ASCII: accented letters of the name become `_` there,
    // and the full name goes in `filename*`.
    let file = format!("{}-marqueurs.csv", if show.name.is_empty() { "show" } else { show.name.as_str() });
    let ascii: String = file.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    let encoded: String = file.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
    with_type(Response::from_string(csv), "text/csv; charset=utf-8").with_header(header)
}

/// `POST /api/timeline/markers/parse`, the CSV file as the body (1 MB at
/// most): `{ markers, skipped, skipped_count }` in the loaded show's unit.
/// Changes nothing: no show edit, no transport, never arms. The body is
/// read without the lock.
fn parse_markers(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::timeline_csv::MAX_CSV_BYTES;
    use std::io::Read;
    let too_big = || text(413, &format!("fichier trop gros ({} Mo au plus)", MAX_CSV_BYTES >> 20));
    if request.body_length().is_some_and(|n| n > MAX_CSV_BYTES) {
        return too_big();
    }
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_CSV_BYTES as u64 + 1).read_to_end(&mut bytes) {
        return text(400, &format!("unreadable body: {e}"));
    }
    if bytes.len() > MAX_CSV_BYTES {
        return too_big();
    }
    // Only what the conversion needs: the time base and the tempo map.
    let tempo = {
        let s = shared.lock().unwrap();
        let Some(show) = s.timeline.show.as_ref() else { return text(409, "aucun show chargé") };
        crate::timeline::Show { time_base: show.time_base, tempo_map: show.tempo_map.clone(), ..Default::default() }
    };
    let decoded = match crate::timeline_csv::decode(&bytes) {
        Ok(t) => t,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    let beats = tempo.time_base == crate::timeline::TimeBase::Beats;
    let parsed = crate::timeline_csv::parse(&decoded, |sec| if beats { tempo.beat_at_s(sec) } else { sec });
    json_response(json!(parsed))
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

/// `GET /api/ilda/record/file?name=<file>.ild`: a file of the ILDA library
/// (a recording, T-442) as a download. The name is checked like a library
/// name (no folder, no `..`), and only a regular file is sent.
fn download_ilda(url: &str, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let Some(name) = query_str(url, "name") else { return text(400, "expected ?name=<fichier>.ild") };
    if !crate::ilda_library::valid_file_name(&name) {
        return text(400, "nom de fichier invalide");
    }
    let dir = shared.lock().unwrap().ilda.dir().to_path_buf();
    let path = dir.join(&name);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_file() => {}
        _ => return text(404, "fichier introuvable"),
    }
    match std::fs::read(&path) {
        Ok(bytes) => {
            // The header must stay ASCII: a name with accents goes out plain.
            let ascii = if name.is_ascii() { name.clone() } else { "enregistrement.ild".into() };
            let disposition = format!("attachment; filename=\"{ascii}\"");
            let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
            with_type(Response::from_data(bytes), "application/octet-stream").with_header(header)
        }
        Err(e) => text(500, &format!("lecture impossible : {e}")),
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
        "outputs": s.outputs.list().iter().map(|o| {
            // Points per frame baked into the files (T-476), shown in the export dialogs.
            let fl = o.scan.frame_limits();
            json!({ "id": o.id, "name": o.projector.name, "max_power": o.limits.max_power, "min_points": fl.min, "max_points": fl.max })
        }).collect::<Vec<_>>(),
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

/// Envelope targets of the loaded show that no envelope may drive (an
/// unknown id, or a control outside the LFO allow-list, T-163): ignored at
/// playback, logged here, and returned to the editor in French.
fn envelope_warnings(s: &Shared) -> Vec<String> {
    let Some(show) = s.timeline.show.as_ref() else { return Vec::new() };
    show.unknown_envelope_targets(|id| crate::lfo::modulatable(&s.controls, id).is_some())
        .into_iter()
        .map(|id| {
            let msg = format!("enveloppe ignorée : « {} » n'est pas un paramètre modulable", id.chars().take(40).collect::<String>());
            log::warn!("{msg}");
            msg
        })
        .collect()
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
            // A built-in example: a copy in memory, nothing written.
            if let Some(id) = req.example {
                match crate::examples::build(&id, &s.presets) {
                    Ok(show) => s.timeline.load_example(show),
                    Err(e) => return text(404, &format!("{e:#}")),
                }
                return json_response(json!(s.timeline.state(&c)));
            }
            let show = match (req.show, req.name) {
                (Some(show), _) => show,
                (None, Some(name)) => match s.shows.load(&name) {
                    Ok(show) => show,
                    Err(e) => return text(404, &format!("{e:#}")),
                },
                (None, None) => return text(400, "expected \"name\", \"show\" or \"example\""),
            };
            s.timeline.load(show);
            envelope_warnings(&s);
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
            // Verrou de piste (T-445): compared as the player will keep it
            // (sorted, numbered), so a resent copy of the server's show passes.
            let mut kept = show.clone();
            kept.sanitize();
            if let Err(e) = cur.check_locks(&kept) {
                return text(400, &format!("{e:#}"));
            }
            s.timeline.edit(show);
            let warnings = envelope_warnings(&s);
            return json_response(json!({ "state": s.timeline.state(&c), "show": s.timeline.show, "warnings": warnings }));
        }
        "save" => {
            let st = &mut *s;
            let example = st.timeline.is_example();
            let Some(show) = st.timeline.show.as_mut() else { return text(409, "aucun show chargé") };
            let name = req.name.map(|n| n.trim().to_string());
            // An example's copy gets a name of its own, never one in use:
            // saving it can't overwrite a show by surprise.
            if example {
                let Some(name) = name.as_ref().filter(|n| !n.is_empty()) else { return text(400, "exemple : donnez un nom à votre copie") };
                if st.shows.list().iter().any(|i| i.name == *name) {
                    return text(409, &format!("un show s'appelle déjà « {name} »"));
                }
            }
            if let Some(name) = name.filter(|n| *n != show.name) {
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
#[serde(default)]
struct TemplateSave {
    name: String,
    description: String,
    /// Ids of the selected events.
    events: Vec<u64>,
    /// The page's copy of the show; default: the loaded show.
    show: Option<crate::timeline::Show>,
    /// Replace a user template of the same name.
    overwrite: bool,
}

/// `POST /api/timeline/templates`: *Enregistrer la sélection comme modèle*.
fn save_template(req: TemplateSave, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let s = shared.lock().unwrap();
    let Some(mut show) = req.show.or_else(|| s.timeline.show.clone()) else { return text(409, "aucun show chargé") };
    show.sanitize();
    let (t, warnings) = match crate::templates::from_selection(&show, &req.events, &req.name, &req.description) {
        Ok(r) => r,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    if !req.overwrite && s.templates.exists(&t.name) {
        return text(409, &format!("un modèle s'appelle déjà « {} »", t.name));
    }
    if let Err(e) = s.templates.save(&t) {
        return text(400, &format!("{e:#}"));
    }
    json_response(json!({ "template": t, "warnings": warnings }))
}

#[derive(Deserialize)]
struct TemplateRender {
    name: String,
    #[serde(flatten)]
    options: crate::templates::ApplyOptions,
    show: Option<crate::timeline::Show>,
}

/// `POST /api/timeline/templates/render`: the show with the template laid
/// on the region, and how many events it adds and removes. Nothing is
/// changed here: the page sends that show through `/api/timeline/edit`.
fn render_template(req: TemplateRender, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let s = shared.lock().unwrap();
    let t = match s.templates.find(&req.name) {
        Ok(t) => t,
        Err(e) => return text(404, &format!("{e:#}")),
    };
    let Some(mut show) = req.show.or_else(|| s.timeline.show.clone()) else { return text(409, "aucun show chargé") };
    show.sanitize();
    match crate::templates::render(&show, &t, &req.options, &s.presets) {
        Ok(a) => json_response(json!({ "show": a.show, "added": a.added, "removed": a.removed })),
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `POST /api/timeline/autoshow`: a song of the library, and a seed
/// (*Autre proposition*; default: drawn from the file).
#[derive(Deserialize)]
struct AutoShowRequest {
    file: String,
    seed: Option<u64>,
}

#[derive(Deserialize, Default)]
struct TimelineRequest {
    name: Option<String>,
    /// `load`: a built-in example's id (examples.rs).
    example: Option<String>,
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
    let reply = crate::project::route(shared, post, action, &raw);
    // Another project in memory (T-285): nothing of the previous one can be undone.
    if post && matches!(action, "new" | "open" | "recover" | "version-open") && matches!(reply, Some(crate::project::Reply::Json(_))) {
        shared.lock().unwrap().history.clear();
    }
    match reply {
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

/// `GET /api/groups`: each zone group with its master and control id.
fn groups_view(s: &Shared) -> serde_json::Value {
    let groups: Vec<_> = s
        .projection
        .get()
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| json!({ "n": i + 1, "name": g.name, "zones": g.zones, "master": g.master, "control": format!("group.{}.master", i + 1) }))
        .collect();
    json!({ "groups": groups, "max_groups": crate::projection::GROUP_MASTER_CONTROLS })
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
struct SpeedGroupsRequest {
    #[serde(default)]
    groups: Option<Vec<Option<crate::speed::SpeedMode>>>,
    #[serde(default)]
    group: Option<u8>,
    #[serde(default)]
    mode: Option<crate::speed::SpeedMode>,
    #[serde(default)]
    rate: Option<f32>,
    #[serde(default)]
    change_on_bar: Option<bool>,
    /// A layer (1..=4) put in `layer_group` (0 = the master clock, 1..=4).
    #[serde(default)]
    layer: Option<u8>,
    #[serde(default)]
    layer_group: Option<u8>,
}

/// `POST /api/speed-groups` (T-419): all checked before anything changes.
fn apply_speed_groups(s: &mut Shared, req: SpeedGroupsRequest) -> Result<(), &'static str> {
    let mut changes: Vec<(u8, crate::speed::SpeedMode)> = Vec::new();
    if let Some(groups) = &req.groups {
        if groups.len() > crate::speed::SPEED_GROUPS {
            return Err("quatre groupes de vitesse au plus");
        }
        for (i, m) in groups.iter().enumerate() {
            if let Some(m) = m {
                changes.push((i as u8 + 1, m.checked()?));
            }
        }
    }
    match (req.group, req.mode) {
        (Some(g), Some(m)) if (1..=crate::speed::SPEED_GROUPS as u8).contains(&g) => changes.push((g, m.checked()?)),
        (None, None) => {}
        _ => return Err("groupe de vitesse inconnu (1 à 4) ou mode manquant"),
    }
    if req.rate.is_some_and(|r| !r.is_finite()) {
        return Err("cadence invalide");
    }
    let layer = match (req.layer, req.layer_group) {
        (Some(n), Some(g)) if (1..=crate::layers::LAYER_COUNT as u8).contains(&n) => Some((n, crate::speed::check_group(g)?)),
        (None, None) => None,
        _ => return Err("calque inconnu (1 à 4) ou groupe manquant"),
    };
    if let Some((n, g)) = layer {
        s.mixer.layer_mut(n).speed_group = g;
        s.mixer_dirty = true;
    }
    if let Some(on) = req.change_on_bar {
        s.speed.change_on_bar = on;
    }
    if let Some(r) = req.rate {
        s.speed.set_rate(r)?;
    }
    let (beat, t, bpb) = s.speed_clock();
    for (g, m) in changes {
        s.speed.set_mode(g, m, beat, t, bpb)?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct SlotRequest {
    id: String,
    #[serde(flatten)]
    slot: CueSlot,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ScanPresetsRequest {
    List(Vec<crate::scan::ScanPreset>),
    Edit { user: Option<Vec<crate::scan::ScanPreset>>, default: Option<String> },
}

/// The scan presets (T-415): built-in and user ones, the output's default,
/// each preset as drawn once bounded by the output (`effective`, with
/// `clamped` when that changed something), and what refers to a preset
/// that no longer exists (it draws with the output's default).
fn scan_presets_view(s: &Shared) -> serde_json::Value {
    use crate::scan::effective;
    let limits = crate::scan::output_limits(s);
    let effective: serde_json::Map<String, serde_json::Value> = s
        .scan_presets
        .all()
        .iter()
        .map(|p| {
            let e = effective(p, &limits);
            let clamped = e.scan_pct != p.scan_pct || e.params.max_points != p.params.max_points;
            (p.id.clone(), json!({ "scan_pct": e.scan_pct, "max_points": e.params.max_points, "clamped": clamped }))
        })
        .collect();
    let unknown = |id: &Option<String>| id.as_deref().is_some_and(|id| !id.is_empty() && !s.scan_presets.is_known(id));
    let mut dangling: Vec<serde_json::Value> =
        s.deck.slots.iter().filter(|(_, slot)| unknown(&slot.scan)).map(|(cue, slot)| json!({ "kind": "cue", "id": cue, "scan": slot.scan })).collect();
    dangling.extend(s.figures.list().iter().filter(|f| unknown(&f.scan)).map(|f| json!({ "kind": "figure", "id": f.name, "scan": f.scan })));
    json!({
        "builtin": crate::scan::builtin_scan_presets(),
        "user": s.scan_presets.user(),
        "default": s.mixer.scan_default.as_deref().filter(|id| s.scan_presets.is_known(id)),
        "limits": limits,
        "effective": effective,
        "dangling": dangling,
    })
}

#[derive(Deserialize)]
struct CueModifiersRequest {
    id: String,
    #[serde(default)]
    with_master: bool,
    #[serde(default)]
    clear: bool,
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
struct CommandRequest {
    line: String,
}

/// `POST /api/desks`: `{"tabs": [...]}` or the bare list.
#[derive(Deserialize)]
#[serde(untagged)]
enum DesksRequest {
    Wrapped { tabs: Vec<crate::desk::DeskTab> },
    Bare(Vec<crate::desk::DeskTab>),
}

fn desks_view(s: &Shared) -> serde_json::Value {
    json!({ "tabs": s.desks.tabs, "rev": s.desks.rev, "max_tabs": crate::desk::MAX_TABS, "max_items": crate::desk::MAX_ITEMS })
}

/// `POST /api/remote` (T-428).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteRequest {
    enabled: bool,
    port: Option<u16>,
}

/// `POST /api/remote/kick`: one remote by name, or all.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteKick {
    name: Option<String>,
}

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
    /// A `fonts::fonts()` id; absent or unknown = « segments ».
    #[serde(default)]
    font: Option<String>,
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

/// `POST /api/safety/exposure` (T-257).
#[derive(Deserialize)]
struct ExposureBody {
    #[serde(default)]
    output: Option<String>,
    exposure: crate::mpe::ExposureSettings,
}

/// `GET /api/safety/exposure`: the estimate for one output.
fn exposure_view(s: &Shared, id: &str) -> serde_json::Value {
    let cfg = s.outputs.config_for(id);
    let mut v = json!(crate::mpe::report(&cfg.projector, &cfg.limits, &cfg.exposure));
    v["output"] = json!(id);
    v["projector_name"] = json!(cfg.projector.name);
    v["max_power"] = json!(cfg.limits.max_power);
    v
}

/// `POST /api/outputs/color` (T-414).
#[derive(Deserialize)]
struct ColorBody {
    color: crate::colorcal::ColorCalibration,
    #[serde(default)]
    confirm_loosen: bool,
}

/// `POST /api/outputs/scan_limits` (T-171).
#[derive(Deserialize)]
struct ScanLimitsBody {
    scan: crate::power::OutputScan,
}

/// `GET /api/outputs/scan_limits`: one output's scan limits, the rate it
/// runs at, its last frame's points and estimated frames/s, the frame rate at which the UI warns, the speed range as the
/// scan presets see it, and what does not fit (the projector sheet's
/// `max_pps` included: warned, never clamped silently).
fn scan_limits_view(s: &Shared, id: &str) -> serde_json::Value {
    let cfg = s.outputs.config_for(id);
    let points = if id == crate::power::MAIN_OUTPUT { s.frame.len() } else { s.projector_frames.iter().find(|f| f.id == id).map_or(0, |f| f.points.len()) };
    json!({
        "output": id,
        "points": points,
        "fps": crate::power::fps(s.pps, points),
        "scan": cfg.scan,
        "defaults": crate::power::OutputScan::default(),
        "pps": s.pps,
        "alert_fps": cfg.scan.alert_fps(),
        "limits": cfg.scan.scan_limits(s.pps),
        "warnings": cfg.scan.warnings(s.pps, &cfg.projector),
    })
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
        // An output drives one projector only (T-277).
        let venue = s.venue.get();
        if let Some(p) = venue.extras().find(|p| p.output.as_deref() == Some(id.as_str())) {
            return text(409, &format!("La sortie « {id} » est celle du projecteur « {} » (RÉGLAGES › Lieu et projecteurs)", p.name));
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

/// `GET /api/venue` (T-277): the venue, the presets, and each projector's
/// output as the engine has it.
fn venue_view(s: &Shared) -> serde_json::Value {
    let venue = s.venue.get();
    let links = s.output_mgr.projector_links();
    let projectors: Vec<serde_json::Value> = venue
        .projectors
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i == 0 {
                return json!({ "id": p.id, "main": true, "output": s.output_mgr.selected(), "output_name": s.output_name, "output_kind": s.output_kind, "state": s.output_mgr.state() });
            }
            let link = links.iter().find(|l| l.projector == p.id);
            let installed = s.projector_outputs.get(&p.id);
            json!({
                "id": p.id,
                "main": false,
                "output": p.output,
                "output_name": installed.map(|o| o.name.clone()),
                "output_kind": installed.map(|o| o.kind),
                "state": link.map(|l| l.state),
                "error": link.and_then(|l| l.error.clone()),
            })
        })
        .collect();
    json!({
        "venue": venue,
        "presets": crate::venue::PRESETS.iter().map(|(id, label, w, d, h, ceiling)| json!({ "id": id, "label_fr": label, "width_m": w, "depth_m": d, "height_m": h, "ceiling": ceiling })).collect::<Vec<_>>(),
        "max_projectors": crate::venue::MAX_PROJECTORS,
        "links": projectors,
        "armed": s.gate.is_armed(),
    })
}

/// `POST /api/venue` (T-277). Geometry changes apply armed or not (the
/// venue only feeds the 3D view); changing which output a projector
/// drives (adding or removing one that drives an output) is 409 while
/// armed. An output already driven by the main projector is 409 too.
fn set_venue(venue: crate::venue::Venue, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    match apply_venue(&mut s, venue) {
        Ok(()) => json_response(venue_view(&s)),
        Err(VenueRefusal::Armed) => with_status(json_response(json!({ "error": VENUE_ARMED_FR, "armed": true })), 409),
        Err(VenueRefusal::Other(code, msg)) => text(code, &msg),
    }
}

const VENUE_ARMED_FR: &str = "Désarmez le laser pour changer les sorties des projecteurs";

/// Why `apply_venue` refused a venue.
pub(crate) enum VenueRefusal {
    /// A routing change while armed (409).
    Armed,
    Other(u16, String),
}

impl VenueRefusal {
    pub(crate) fn status_and_text(self) -> (u16, String) {
        match self {
            VenueRefusal::Armed => (409, VENUE_ARMED_FR.to_string()),
            VenueRefusal::Other(code, msg) => (code, msg),
        }
    }
}

/// `set_venue` under the lock: the same checks for a manual change and
/// an undo (history.rs, T-285).
pub(crate) fn apply_venue(s: &mut Shared, venue: crate::venue::Venue) -> Result<(), VenueRefusal> {
    venue.validate().map_err(|e| VenueRefusal::Other(400, e.to_string()))?;
    let next = venue.sanitized();
    s.gate.sync_estop(&s.estop);
    if let Some(main) = s.output_mgr.selected() {
        if let Some(p) = next.extras().find(|p| p.output.as_deref() == Some(main.as_str())) {
            return Err(VenueRefusal::Other(409, format!("La sortie « {main} » est déjà celle du projecteur principal : choisissez-en une autre pour « {} »", p.name)));
        }
    }
    let routing = s.venue.get().routing_changes(&next);
    if routing && s.gate.is_armed() {
        return Err(VenueRefusal::Armed);
    }
    s.venue.set(next).map_err(|e| VenueRefusal::Other(400, e.to_string()))?;
    if routing {
        let outputs: Vec<serde_json::Value> = s.venue.get().extras().map(|p| json!({ "projector": p.id, "output": p.output })).collect();
        s.gate.log().record("venue_outputs", Some("ui"), json!({ "projectors": outputs }));
    }
    s.connect_projectors(ArmSource::Ui);
    s.sync_checklist();
    Ok(())
}

/// `GET /api/frames` (T-277): one entry per projector of the venue, its
/// frame after its whole safety chain (what the preview shows for the
/// main one), the lit points its output was sent and its placement for
/// the 3D view.
fn frames(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let s = shared.lock().unwrap();
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let pts = |frame: &[crate::patterns::Point]| frame.iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect::<Vec<_>>();
    let estop = s.estop.is_latched();
    let frames: Vec<serde_json::Value> = s
        .venue
        .get()
        .projectors
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (points, lit, output_name) = if i == 0 {
                (pts(&s.frame), s.output_lit, s.output_name.clone())
            } else {
                let f = s.projector_frames.iter().find(|f| f.id == p.id);
                (f.map(|f| pts(&f.points)).unwrap_or_default(), f.map_or(0, |f| f.lit), s.projector_outputs.get(&p.id).map(|o| o.name.clone()))
            };
            json!({
                "id": p.id,
                "name": p.name,
                "main": i == 0,
                "output": if i == 0 { s.output_mgr.selected() } else { p.output.clone() },
                "output_name": output_name,
                "lit": lit,
                "pos_m": p.pos_m,
                "yaw_deg": p.yaw_deg,
                "pitch_deg": p.pitch_deg,
                "roll_deg": p.roll_deg,
                "scan_deg": p.scan_deg,
                "brightness": p.brightness,
                "mirror_x": p.mirror_x,
                "points": points,
            })
        })
        .collect();
    json_response(json!({ "armed": s.gate.is_armed() && !estop, "estop": estop, "frames": frames }))
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
#[derive(Deserialize)]
struct ProfileRequest {
    action: String,
    name: String,
    venue: Option<String>,
    notes: Option<String>,
    audience_scan_allowed: Option<bool>,
    #[serde(default)]
    confirm_loosen: bool,
    source: Option<String>,
}

#[derive(Deserialize)]
struct ProfileLoadRequest {
    name: String,
    #[serde(default)]
    confirm_loosen: bool,
    source: Option<String>,
}

/// The reply to a refused profile action (T-260).
fn profile_error(e: crate::profiles::LoadError) -> HttpResponse {
    use crate::profiles::LoadError;
    let msg = e.message_fr();
    match e {
        LoadError::Source => text(403, &msg),
        LoadError::Unknown(_) => text(404, &msg),
        LoadError::Damaged(m) => text(400, &m),
        LoadError::Loosens(list) => with_status(json_response(json!({ "error": msg, "loosen": list })), 409),
        LoadError::Failed(_) => text(500, &msg),
    }
}

fn locked() -> HttpResponse {
    text(423, crate::showmode::LOCKED_FR)
}

#[derive(Deserialize)]
struct KeepAwakeRequest {
    enabled: bool,
}

#[derive(Deserialize)]
struct OnboardingRequest {
    tour_done: bool,
}

fn onboarding_view(s: &Shared) -> serde_json::Value {
    json!({ "tour_done": s.onboarding.state.tour_done, "tour_version": crate::onboarding::TOUR_VERSION })
}

/// A help page for the UI; `full` adds its body (the list is searchable
/// offline once fetched with `?full=1`).
fn help_view(page: &crate::help::HelpPage, full: bool) -> serde_json::Value {
    let mut v = json!({ "slug": page.slug, "title_fr": page.title_fr, "group_fr": page.group_fr });
    if full {
        v["body_html"] = json!(page.body_html);
    }
    v
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

/// Incoming timecode (T-168):
/// - `GET /api/timecode`: settings, TC-IN, status, readout, MIDI ports
///   and audio input.
/// - `POST /api/timecode` `{ "source": "mtc", "offset_s": 3600, … }`:
///   the settings given (an edit, locked in show mode).
/// - `POST /api/timecode/arm` `{ "on": bool }` or `{ "toggle": true }`:
///   TC-IN, from this Mac's interface only (like the external gate).
///   Never arms the laser.
fn timecode_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, method: Method, path: &str) -> HttpResponse {
    match (method, path) {
        (Method::Get, "/api/timecode") => json_response(crate::timecode::view(&mut shared.lock().unwrap())),
        (Method::Post, "/api/timecode") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let (result, job) = {
                    let mut s = shared.lock().unwrap();
                    let r = crate::timecode::update(&mut s, &patch);
                    (r, s.timecode.save_job())
                };
                match result {
                    Ok(changed) => {
                        if let (true, Some((path, cfg))) = (changed, job) {
                            crate::save_json(&path, &cfg);
                        }
                        json_response(crate::timecode::view(&mut shared.lock().unwrap()))
                    }
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/timecode/arm") => {
            let local = request.remote_addr().is_some_and(|a| a.ip().is_loopback());
            let req = match body::<serde_json::Value>(request) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or("ui"));
            if !local || !matches!(source, ArmSource::Ui | ArmSource::Keyboard | ArmSource::Api) {
                return text(403, "TC-IN ne s'arme que depuis l'interface de ce Mac");
            }
            let mut s = shared.lock().unwrap();
            let want = match (req.get("on").and_then(|v| v.as_bool()), req.get("toggle").and_then(|v| v.as_bool())) {
                (Some(on), _) => on,
                (None, Some(true)) => !s.timecode.armed,
                _ => return text(400, "expected \"on\" or \"toggle\""),
            };
            match crate::timecode::set_armed(&mut s, want, source) {
                Ok(()) => json_response(crate::timecode::view(&mut s)),
                Err(why) => with_status(json_response(json!({ "armed": false, "error": why })), 409),
            }
        }
        _ => text(404, "not found"),
    }
}

#[derive(serde::Deserialize)]
struct LatencySet {
    output_offset_ms: f64,
}

#[derive(serde::Deserialize)]
struct FestivalPick {
    palette: Option<usize>,
}

#[derive(serde::Deserialize)]
struct FestivalConfig {
    quantize: crate::festival::PaletteQuantize,
    #[serde(default)]
    balance: crate::festival::Balance,
}

/// The festival palettes (T-130): the list, the one on show, a change
/// waiting for its boundary, the quantisation and the perceived balance.
fn festival_view(s: &Shared) -> serde_json::Value {
    let f = &s.festival;
    json!({
        "palettes": crate::festival::PALETTES,
        "active": f.active,
        "pending": f.pending,
        "quantize": f.quantize,
        "balance": f.balance,
    })
}

/// `GET /api/state`'s body (also `state.json` in the diagnostic report).
fn state_json(s: &mut Shared) -> serde_json::Value {
    s.sync_data_warnings();
    s.sync_notices();
    s.gate.sync_estop(&s.estop);
    s.sync_audience();
    let arm = s.gate.status(&s.estop);
    let mut v = json!({
        "settings": s.settings,
        "settings_rev": s.settings_rev,
        "festival": festival_view(s),
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
        "first_run": s.onboarding.first_run(),
        "lock": lock_view(s),
        "output": s.output_name,
        "output_kind": s.output_kind,
        "output_state": s.output_mgr.state(),
        "output_status": s.output_status,
        "pps": s.pps,
        "shapes": SHAPE_NAMES,
        "generators": GENERATOR_NAMES,
        "scenes": s.scenes.list(),
        "tempo": tempo_view(s),
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
        "timecode": crate::timecode::summary(s),
        "dmx": crate::netin::summary(s, Instant::now()),
        "project_modified": s.project.last_modified,
        "notices_last": s.notices.last(),
        "history": s.history.summary(),
    });
    // T-450: free space and the copy to a second location.
    v["storage"] = json!(s.project.backup.status());
    // T-260: the active safety profile, for the status bar.
    v["safety_profile"] = crate::profiles::summary(s);
    v
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

/// The timeline's state with the timecode's under `timecode` (T-409).
fn timeline_with_timecode(s: &Shared) -> serde_json::Value {
    let mut tl = json!(s.timeline.state(&s.timeline_clock()));
    tl["timecode"] = crate::timecode::summary(s);
    tl
}

/// The current frame for the preview, as `[x, y, r, g, b]` rows rounded
/// to 3 decimals to keep the payload small at 30 requests a second, with
/// the page's state (`live_view`). `X-State-Version`: the pushed state's
/// version (T-411), so a page polling as a fallback knows when the
/// stream has caught up.
/// A « Préparer » action (T-274), answered with the slot's view.
fn prepared_reply(shared: &Arc<Mutex<Shared>>, f: impl FnOnce(&mut Shared) -> Result<(), crate::prepared::PrepareError>) -> HttpResponse {
    let mut s = shared.lock().unwrap();
    match f(&mut s) {
        Ok(()) => json_response(crate::prepared::view(&s)),
        Err(e) => text(e.status(), e.message_fr()),
    }
}

fn frame(shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let (mut view, points, version, seq, beats) = {
        let mut s = shared.lock().unwrap();
        let round = |v: f32| (v * 1000.0).round() / 1000.0;
        let points: Vec<[f32; 5]> = s.frame.iter().map(|p| [round(p.x), round(p.y), round(p.r), round(p.g), round(p.b)]).collect();
        let seq = s.push.seq();
        // The master beat and each group's (A..D), at the same instant.
        let (beat, t, _) = s.speed_clock();
        let beats = s.speed.beats(beat, t);
        (live_view(&mut s), points, s.push.state_version(), seq, beats)
    };
    view["points"] = json!(points);
    // The engine frame these points and `output_lit` belong to: the engine
    // sets both and publishes this sequence under one lock. Compared with
    // `X-Frame-Seq` from `POST /api/estop`, it tells a frame finished after
    // the stop from one finished just before (T-475). Only here, not in the
    // pushed state, which would otherwise change every frame.
    view["frame_seq"] = json!(seq);
    // Each speed group's beat now (T-419): only here, like `frame_seq`
    // (it moves every frame; the pushed state keeps the groups' modes).
    if let Some(groups) = view.pointer_mut("/tempo/groups").and_then(|g| g.as_array_mut()) {
        for (g, beat) in groups.iter_mut().zip(&beats[1..]) {
            g["beat"] = json!((beat * 1e4).round() / 1e4);
        }
    }
    view["tempo"]["effect_beat"] = json!((beats[0] * 1e4).round() / 1e4);
    json_response(view).with_header(header("X-State-Version", version))
}

/// The tempo clock's state, with the speed groups (T-419) as `groups`
/// (A..D, without their beats), `rate` (the *Cadence*) and `change_on_bar`.
fn tempo_view(s: &Shared) -> serde_json::Value {
    let mut v = json!(s.tempo.state(s.now_s()));
    let speed = s.speed.view(s.tempo.bpm);
    for key in ["groups", "rate", "change_on_bar"] {
        v[key] = speed[key].clone();
    }
    v
}

/// What the page shows besides the points: `/api/frame` without them,
/// and the pushed state (`/api/events`, T-411).
fn live_view(s: &mut Shared) -> serde_json::Value {
    s.gate.sync_estop(&s.estop);
    s.sync_notices();
    let arm = s.gate.status(&s.estop);
    let now = s.now_s();
    let mut v = json!({
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
        "festival": festival_view(s),
        "cues": {
            "active": s.deck.active.iter().map(|a| json!({ "cue": a.cue, "held": a.held, "layer": a.layer, "phase": s.deck.phase(a.id, now), "level": a.exec_level, "speed_group": s.look_speed_group(a.layer, a.id) })).collect::<Vec<_>>(),
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
        // Group masters (T-420) ride with the layers: the LIVE row shows them.
        "layers": { "mixer": s.mixer, "mix": s.mix, "groups": groups_view(s)["groups"] },
        "tempo": tempo_view(s),
        // T-409: the timecode state also under `timeline.timecode`.
        "timeline": timeline_with_timecode(s),
        "timeline_audio": song_view(s),
        "timecode": crate::timecode::summary(s),
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
        // The loaded cue list (T-410); null when none.
        "cuelist": s.cue_list.view(),
        // T-433: unsaved project changes (as last seen, every 500 ms).
        "project_modified": s.project.last_modified,
        // T-432: the newest notice's id; the page reads /api/notices when it moves.
        "notices_last": s.notices.last(),
    });
    // T-285: undo / redo counts and labels; `rev` moves on every change.
    v["history"] = s.history.summary();
    // T-260: the active safety profile, for the status bar.
    v["safety_profile"] = crate::profiles::summary(s);
    // T-418: the executor page shown, its faders, and the pages with
    // something active.
    v["executors"] = crate::executors::view(s);
    // T-427: the desk's layout revision, its bound controls' values and
    // its missing bindings.
    v["desk"] = crate::desk::view(s);
    // T-146: what plays on the FX grid, each line's level now.
    v["fx"] = s.fx.view();
    // T-274: what is prepared (no points: those are `/api/preview-frame`).
    v["prepared"] = crate::prepared::view(s);
    // T-144: the cue level of every cue on show (the layers' are in
    // `layers.mixer`).
    v["cue_mods"] = crate::levels::view(s);
    v["levels_on"] = json!({
        "layers": s.mixer.layers.iter().map(|l| !crate::levels::is_neutral(&l.modifiers)).collect::<Vec<_>>(),
        "cues": s.cue_mods.values().any(|m| !crate::levels::is_neutral(m)),
    });
    v
}

/// `POST /api/cuelist/{go,back,pause,top,goto}` (T-410), body optional:
/// `{ "source": "keyboard" }` (« En cours »), `{ "step": n }` (0-based)
/// for `goto`. Playback only: allowed in show mode, never arms; Go waits
/// for an e-stop reset.
fn cuelist_transport(request: &mut Request, shared: &Mutex<Shared>, action: &str) -> HttpResponse {
    let mut raw = String::new();
    if let Err(e) = request.as_reader().read_to_string(&mut raw) {
        return text(400, &format!("unreadable body: {e}"));
    }
    let req: serde_json::Value = if raw.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return text(400, &format!("invalid JSON: {e}")),
        }
    };
    let source = match req.get("source").and_then(|v| v.as_str()) {
        Some("keyboard") => controls::ControlSource::Keyboard,
        _ => controls::ControlSource::Ui,
    };
    let mut s = shared.lock().unwrap();
    let r = match action {
        "go" => crate::cuelist::go(&mut s, source),
        "back" => crate::cuelist::back(&mut s, source),
        "pause" => crate::cuelist::pause(&mut s, source),
        "top" => crate::cuelist::top(&mut s, source),
        "goto" => match req.get("step").and_then(|v| v.as_u64()) {
            Some(step) => crate::cuelist::goto(&mut s, step as usize, source),
            None => Err("pas manquant"),
        },
        _ => return text(404, "not found"),
    };
    match r {
        Ok(()) => json_response(s.cue_list.view()),
        Err(why) => text(409, why),
    }
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
    // The beat the engine's LFOs read (T-246).
    let beat = s.tempo.effect_beat_at(t);
    s.lfos.list().iter().map(|m| json!({ "phase": m.position(t, beat).1, "value": m.wave_at(t, beat) })).collect()
}

/// `POST /api/fx`: the grid's cells, line by line.
#[derive(serde::Deserialize)]
struct FxCellsBody {
    cells: crate::fx::Cells,
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

/// `POST /api/test/timecode` (`--test-hooks`): Art-Net timecode frames.
#[derive(Deserialize)]
struct SimTimecode {
    /// Frames since 00:00:00:00 at `rate`.
    frames: i64,
    rate: crate::timecode::Rate,
    #[serde(default = "one")]
    count: u32,
}

fn one() -> u32 {
    1
}

/// `POST /api/test/tempo_estimate` (`--test-hooks`): a simulated detector.
#[derive(Deserialize)]
struct SimEstimate {
    bpm: f32,
    confidence: f32,
    state: crate::audio::bpm::DetectState,
    #[serde(default)]
    offset_s: f64,
    /// A simulated downbeat detection (T-235): which beat of the grid
    /// (`n` mod 4, `n` counted from `offset_s`) is the "1", and its confidence.
    #[serde(default)]
    downbeat_beat: Option<u8>,
    #[serde(default)]
    downbeat_confidence: f32,
}

impl SimEstimate {
    fn snapshot(&self, now: f64) -> crate::audio::NativeSnapshot {
        let spb = 60.0 / (self.bpm as f64).clamp(crate::tempo::MIN_BPM, crate::tempo::MAX_BPM);
        let beat_time = self.offset_s + ((now - self.offset_s) / spb).floor() * spb;
        let tempo = crate::audio::bpm::TempoEstimate { bpm: self.bpm, confidence: self.confidence, beat_time, next_beat: beat_time + spb, state: self.state };
        let downbeat = match self.downbeat_beat {
            Some(one) => {
                let n = ((now - self.offset_s) / spb).floor() as i64;
                let last_one = n - (n - one as i64).rem_euclid(4);
                crate::audio::downbeat::DownbeatDetection { downbeat_t: self.offset_s + last_one as f64 * spb, confidence: self.downbeat_confidence, bpm: self.bpm, bars: 8, ..Default::default() }
            }
            None => Default::default(),
        };
        crate::audio::NativeSnapshot {
            features: crate::engine::AudioFeatures::default(),
            rms_db: crate::audio::analysis::FLOOR_DB,
            peak_db: crate::audio::analysis::FLOOR_DB,
            spectral: Default::default(),
            onsets: Default::default(),
            tempo,
            spectrum: [crate::audio::analysis::FLOOR_DB; crate::audio::spectrum::SPECTRUM_BANDS],
            sections: Default::default(),
            downbeat,
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
                    downbeat: Default::default(),
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
        // Bytes first: a chunk may end in the middle of a character.
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map_or((raw.len(), raw.len()), |i| (i, i + 4));
        let head = String::from_utf8_lossy(&raw[..split.0]).into_owned();
        let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let body = &raw[split.1..];
        // A long answer (over the server's 32 kB threshold) comes chunked.
        let chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
        let body = if chunked { dechunk(body) } else { body.to_vec() };
        (status, String::from_utf8(body).unwrap())
    }

    /// The body of a chunked HTTP answer, put back together.
    fn dechunk(raw: &[u8]) -> Vec<u8> {
        let (mut rest, mut out) = (raw, Vec::new());
        while let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") {
            let n = usize::from_str_radix(String::from_utf8_lossy(&rest[..eol]).trim(), 16).unwrap_or(0);
            let tail = &rest[eol + 2..];
            if n == 0 || tail.len() < n {
                break;
            }
            out.extend_from_slice(&tail[..n]);
            rest = tail[n..].strip_prefix(b"\r\n").unwrap_or(&tail[n..]);
        }
        out
    }

    fn arm_status(t: &TestServer) -> serde_json::Value {
        serde_json::from_str(&t.request("GET", "/api/arm", "").1).unwrap()
    }

    /// T-277: the venue over the API. A projector that drives an output
    /// blocks arming until the engine has installed it; while armed, its
    /// output can't change but its placement can; `/api/frames` has one
    /// entry per projector; a preset keeps the projectors.
    #[test]
    fn the_venue_route_routes_outputs_only_while_disarmed() {
        let t = TestServer::start(false);
        let v: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/venue", "").1).unwrap();
        assert_eq!(v["venue"]["projectors"].as_array().unwrap().len(), 1);
        assert_eq!(v["presets"].as_array().unwrap().len(), 3);
        let mut venue = v["venue"].clone();
        venue["projectors"].as_array_mut().unwrap().push(json!({ "id": "p2", "name": "Jardin", "output": "rec", "mirror_x": true }));
        let (status, body) = t.request("POST", "/api/venue", &venue.to_string());
        assert_eq!(status, 200, "{body}");
        let (status, body) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(status, 409);
        assert!(body.contains("Sortie de projecteur non connectée : Jardin"), "{body}");
        let frames: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frames", "").1).unwrap();
        assert_eq!(frames["frames"].as_array().unwrap().len(), 2);
        assert_eq!(frames["frames"][1]["mirror_x"], true);
        // The engine installs p2's output: arming is allowed.
        {
            let mut s = t.shared.lock().unwrap();
            s.projector_outputs.insert("p2".into(), crate::venue::Installed { output: "rec".into(), name: "Rec".into(), kind: "test" });
            s.sync_projectors();
        }
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        venue["projectors"][1]["output"] = json!("autre");
        let (status, body) = t.request("POST", "/api/venue", &venue.to_string());
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("Désarmez"), "{body}");
        venue["projectors"][1]["output"] = json!("rec");
        venue["projectors"][1]["pos_m"] = json!([4.0, 3.0, 11.0]);
        assert_eq!(t.request("POST", "/api/venue", &venue.to_string()).0, 200, "placement changes while armed");
        assert!(arm_status(&t)["armed"].as_bool().unwrap(), "and never disarm");
        let (status, body) = t.request("POST", "/api/venue/preset", r#"{"name":"festival"}"#);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((v["venue"]["room"]["width_m"].as_f64(), v["venue"]["projectors"].as_array().unwrap().len()), (Some(40.0), 2));
        assert_eq!(t.request("POST", "/api/venue/preset", r#"{"name":"stade"}"#).0, 400);
        // Two projectors on one output: refused.
        venue["projectors"].as_array_mut().unwrap().push(json!({ "id": "p3", "output": "rec" }));
        assert_eq!(t.request("POST", "/api/venue", &venue.to_string()).0, 400);
    }

    /// T-419: the speed groups over the API: read, changed (bad values
    /// refused, nothing half applied), allowed in show mode (played live),
    /// in `/api/frame` as `tempo.groups` with their beats, and a cell's
    /// group checked.
    #[test]
    fn speed_groups_over_the_api() {
        let t = TestServer::start(false);
        let g = js(&t.request("GET", "/api/speed-groups", "").1);
        assert_eq!(g["groups"].as_array().unwrap().len(), 4);
        assert_eq!((g["rate"].as_f64(), g["change_on_bar"].as_bool()), (Some(1.0), Some(true)));
        let body = json!({ "change_on_bar": false, "groups": [{ "type": "multiple", "mult": 0.5 }, null, { "type": "free_bpm", "bpm": 98 }], "rate": 2.0 });
        let (code, reply) = t.request("POST", "/api/speed-groups", &body.to_string());
        assert_eq!(code, 200, "{reply}");
        let g = js(&reply);
        assert_eq!((g["groups"][0]["label"].as_str(), g["groups"][2]["free"].as_bool(), g["rate"].as_f64()), (Some("×½"), Some(true), Some(2.0)));
        for bad in [
            json!({ "group": 1, "mode": { "type": "multiple", "mult": 3 } }),
            json!({ "group": 5, "mode": { "type": "multiple", "mult": 1 } }),
            json!({ "group": 1, "mode": { "type": "free_bpm", "bpm": -3 } }),
            json!({ "group": 1 }),
            json!({ "rate": 0.7, "groups": [{ "type": "multiple", "mult": 7 }] }),
        ] {
            assert_eq!(t.request("POST", "/api/speed-groups", &bad.to_string()).0, 400, "{bad}");
        }
        assert_eq!(t.shared.lock().unwrap().speed.rate(), 2.0, "a refused request changes nothing");
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/speed-groups", r#"{"rate":1.5}"#).0, 200);
        let f = js(&t.request("GET", "/api/frame", "").1);
        let groups = f["tempo"]["groups"].as_array().unwrap();
        assert_eq!(groups[0]["mult"].as_f64(), Some(0.5));
        let (master, a) = (f["tempo"]["effect_beat"].as_f64().unwrap(), groups[0]["beat"].as_f64().unwrap());
        // A carries on from where it was when set, at half the master's pace.
        let expected = t.shared.lock().unwrap().speed.beat(1, master, 0.0);
        assert!((a - expected).abs() < 1e-3 && a <= master, "A at ×½: {a} for {master}");
        assert_eq!(f["tempo"]["rate"].as_f64(), Some(1.5));
        t.shared.lock().unwrap().show_mode.on = false;
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        assert_eq!(t.request("POST", "/api/cues/slot", &json!({ "id": cue, "speed_group": 9 }).to_string()).0, 400);
        assert_eq!(t.request("POST", "/api/cues/slot", &json!({ "id": cue, "speed_group": 1 }).to_string()).0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).speed_group, Some(1));
        assert_eq!(t.request("POST", "/api/speed-groups", r#"{"layer":2,"layer_group":3}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().mixer.layer(2).speed_group, 3);
        for bad in [r#"{"layer":5,"layer_group":1}"#, r#"{"layer":2,"layer_group":5}"#, r#"{"layer":2}"#] {
            assert_eq!(t.request("POST", "/api/speed-groups", bad).0, 400, "{bad}");
        }
        assert_eq!(f["armed"], false);
    }

    /// T-146: the FX grid over the API: read, edited (recorded in the
    /// history, refused in show mode), played through `/api/control`
    /// (allowed in show mode), its state in `/api/frame`.
    #[test]
    fn the_fx_grid_over_the_api() {
        let t = TestServer::start(false);
        let g = js(&t.request("GET", "/api/fx", "").1);
        assert_eq!(g["cells"].as_array().unwrap().len(), crate::fx::LINES);
        assert_eq!(g["cells"][0][0]["name"], "Respiration");
        assert!(g["targets"].as_array().unwrap().iter().all(|t| t["id"] != "transport.arm"));
        let mut cells = g["cells"].clone();
        cells[3][0] = json!({ "name": "Mon effet", "modifiers": { "size": 0.5 } });
        assert_eq!(t.request("POST", "/api/fx", &json!({ "cells": cells }).to_string()).0, 200);
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Effet « Mon effet » ajouté");
        let mut bad = cells.clone();
        bad[3][1] = json!({ "name": "x", "modulators": [{ "target": "safety.estop" }] });
        assert_eq!(t.request("POST", "/api/fx", &json!({ "cells": bad }).to_string()).0, 400);
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/fx", &json!({ "cells": cells }).to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"fx.4.1","value":1}"#).0, 200);
        let f = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(f["fx"]["lines"][3]["active"], 0);
        assert_eq!(f["armed"], false);
    }

    /// T-418: the executor bank over the API: read, assigned (recorded in
    /// the history and undone, refused in show mode, bad assignments
    /// refused), played through `/api/control` (allowed in show mode), its
    /// state and the cue level in `/api/frame`.
    #[test]
    fn the_executor_bank_over_the_api() {
        let t = TestServer::start(false);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let g = js(&t.request("GET", "/api/executors", "").1);
        assert_eq!(g["pages"].as_array().unwrap().len(), 1);
        assert_eq!(g["pages"][0]["handles"].as_array().unwrap().len(), 8);
        let mut pages = g["pages"].clone();
        pages[0]["handles"][0] = json!({ "target": { "type": "cue", "id": cue }, "fader": "fade_in", "buttons": ["flash", "black_held"] });
        let (code, body) = t.request("POST", "/api/executors", &json!({ "pages": pages }).to_string());
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Exécuteur 1.1 affecté");
        for bad in [
            json!({ "target": { "type": "cue", "id": "nope" } }),
            json!({ "target": { "type": "layer", "n": 1 }, "fader": "fade_in" }),
            json!({ "target": { "type": "cue_list" }, "buttons": ["flash", "none"] }),
            json!({ "target": { "type": "macro", "id": "m" } }),
        ] {
            let mut p = pages.clone();
            p[0]["handles"][1] = bad.clone();
            assert_eq!(t.request("POST", "/api/executors", &json!({ "pages": p }).to_string()).0, 400, "{bad}");
        }
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/executors", &json!({ "pages": pages }).to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"exec.1.1.fader","value":0.5}"#).0, 200);
        let f = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(f["executors"]["levels"][0], 0.5);
        assert_eq!(f["executors"]["active_pages"], json!([true]));
        assert_eq!(f["cues"]["active"][0]["cue"], cue.as_str());
        assert_eq!(f["cues"]["active"][0]["level"], 0.5);
        assert_eq!(f["armed"], false);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"exec.1.1.fader","value":0}"#).0, 200);
        assert!(t.shared.lock().unwrap().deck.active.is_empty());
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.shared.lock().unwrap().executors.bank, crate::executors::ExecutorBank::default());
    }

    /// T-427: the desk over the API: saved (recorded in the history,
    /// refused in show mode), arming and safety bindings refused, an XY
    /// pad's controls played through `/api/control` (allowed in show mode)
    /// and read back in `/api/frame`.
    #[test]
    fn the_desk_over_the_api() {
        let t = TestServer::start(false);
        assert_eq!(js(&t.request("GET", "/api/desks", "").1)["tabs"], json!([]));
        let xy = json!({ "id": 1, "kind": "xy_pad", "x": 0, "y": 0, "w": 4, "h": 4,
            "binding": { "type": "control", "id": "master.pos_x" }, "binding_y": { "type": "control", "id": "master.pos_y" } });
        let stop = json!({ "id": 2, "kind": "button", "x": 4, "y": 0, "w": 2, "h": 1, "binding": { "type": "control", "id": "safety.estop" } });
        let tabs = json!([{ "name": "Intro", "items": [xy, stop] }]);
        let (code, body) = t.request("POST", "/api/desks", &json!({ "tabs": tabs }).to_string());
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["tabs"][0]["items"][0]["binding_y"]["id"], "master.pos_y");
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Onglet de pupitre « Intro » ajouté");
        for id in ["transport.arm", "safety.hold"] {
            let bad = json!([{ "items": [{ "id": 1, "kind": "button", "binding": { "type": "control", "id": id } }] }]);
            assert_eq!(t.request("POST", "/api/desks", &bad.to_string()).0, 400, "{id}");
        }
        let overlap = json!([{ "items": [{ "id": 1, "w": 2 }, { "id": 2, "x": 1 }] }]);
        assert_eq!(t.request("POST", "/api/desks", &overlap.to_string()).0, 400);
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/desks", &tabs.to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"master.pos_x","norm":0.75}"#).0, 200);
        let f = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(f["desk"]["values"]["master.pos_x"], 0.5);
        assert_eq!(f["live"]["pos_x"], 0.5);
        assert_eq!(f["armed"], false);
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert!(t.shared.lock().unwrap().desks.tabs.is_empty());
    }

    /// T-285: edits are recorded, cue presses are not; undo restores a
    /// deleted scene; show mode refuses undo; a venue undo that would
    /// change the projectors' outputs is refused while armed, never disarms.
    #[test]
    fn undo_redo_and_the_history_over_the_api() {
        let t = TestServer::start(false);
        let undo_count = |t: &TestServer| js(&t.request("GET", "/api/state", "").1)["history"]["undo"].as_u64().unwrap();
        let has_scene = |t: &TestServer, name: &str| t.shared.lock().unwrap().scenes.get(name).is_some();
        assert_eq!(t.request("POST", "/api/scenes/save", r#"{"name":"Annuler test","duration_secs":4}"#).0, 200);
        assert_eq!(t.request("POST", "/api/scenes/delete", r#"{"name":"Annuler test"}"#).0, 200);
        assert!(!has_scene(&t, "Annuler test"));
        let h = js(&t.request("GET", "/api/history", "").1);
        assert_eq!(h["undo"][0]["label"], "Scène « Annuler test » supprimée");
        assert_eq!(h["undo"][1]["label"], "Scène « Annuler test » enregistrée");
        let n = undo_count(&t);
        // Playing is not editing.
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        assert_eq!(t.request("POST", "/api/cue", &format!(r#"{{"id":"{cue}"}}"#)).0, 200);
        assert_eq!(t.request("POST", "/api/live", r#"{"brightness":0.5}"#).0 / 100, 2);
        assert_eq!(undo_count(&t), n, "cue presses and live masters are never in the history");
        // Undo brings the scene back; redo deletes it again.
        let (code, body) = t.request("POST", "/api/undo", "{}");
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["done"][0], "Scène « Annuler test » supprimée");
        assert!(has_scene(&t, "Annuler test"));
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["history"]["redo"], 1);
        assert_eq!(t.request("POST", "/api/redo", "").0, 200);
        assert!(!has_scene(&t, "Annuler test"));
        // Show mode: undo is an edit.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 423);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        assert!(!has_scene(&t, "Annuler test"));
        // A grid page added, then undone.
        let pages = |t: &TestServer| js(&t.request("GET", "/api/grid", "").1)["pages"].as_array().unwrap().len();
        let before = pages(&t);
        assert_eq!(t.request("POST", "/api/grid/page", r#"{"action":"new","name":"Final"}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Page « Final » ajoutée");
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(pages(&t), before);
        // A projector driving an output: its undo would change the outputs.
        let mut venue = js(&t.request("GET", "/api/venue", "").1)["venue"].clone();
        venue["projectors"].as_array_mut().unwrap().push(json!({ "id": "p2", "name": "Jardin", "output": "rec" }));
        assert_eq!(t.request("POST", "/api/venue", &venue.to_string()).0, 200);
        {
            let mut s = t.shared.lock().unwrap();
            s.projector_outputs.insert("p2".into(), crate::venue::Installed { output: "rec".into(), name: "Rec".into(), kind: "test" });
            s.sync_projectors();
        }
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, body) = t.request("POST", "/api/undo", "{}");
        assert_eq!(code, 409, "{body}");
        assert!(body.contains("Désarmez"), "{body}");
        assert!(arm_status(&t)["armed"].as_bool().unwrap(), "a refused undo never disarms");
        assert_eq!(t.shared.lock().unwrap().venue.get().projectors.len(), 2);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.shared.lock().unwrap().venue.get().projectors.len(), 1);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap(), "and never arms");
        // `to`: down to an entry.
        let h = js(&t.request("GET", "/api/history", "").1);
        let last = h["undo"].as_array().unwrap().last().unwrap()["id"].as_u64().unwrap();
        assert_eq!(t.request("POST", "/api/undo", &format!(r#"{{"to":{last}}}"#)).0, 200);
        assert_eq!(undo_count(&t), 0);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 409, "nothing left");
    }

    /// T-144: cue and layer modifiers over the API: level controls are
    /// live moves (allowed in show mode, never in the history), « Enregistrer
    /// dans le cue » is an edit (history, undo, refused in show mode), the
    /// properties dialog keeps the stored ones, and a restart plays them.
    #[test]
    fn cue_and_layer_modifiers_over_the_api() {
        let t = TestServer::start(false);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let control = |t: &TestServer, id: &str, v: f32| t.request("POST", "/api/control", &format!(r#"{{"id":"{id}","value":{v}}}"#)).0;
        assert_eq!(control(&t, &format!("cue.{cue}.size"), 0.5), 403, "not playing");
        assert_eq!(t.request("POST", "/api/cues/modifiers", &format!(r#"{{"id":"{cue}"}}"#)).0, 409);
        assert_eq!(t.request("POST", "/api/cue", &format!(r#"{{"id":"{cue}"}}"#)).0, 200);
        assert_eq!(control(&t, &format!("cue.{cue}.size"), 0.5), 200);
        assert_eq!(control(&t, "layer.2.pos_x", 0.25), 200);
        assert_eq!(control(&t, "layer.9.pos_x", 0.25), 404);
        let state = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(state["cue_mods"][&cue]["size"], 0.5);
        assert_eq!(state["layers"]["mixer"]["layers"][1]["modifiers"]["pos_x"], 0.25);
        assert_eq!(state["history"]["undo"], 0, "live moves are not edits");
        // Stored, labelled, undone, redone.
        assert_eq!(t.request("POST", "/api/cues/modifiers", &format!(r#"{{"id":"{cue}"}}"#)).0, 200);
        let name = t.shared.lock().unwrap().presets[0].name.clone();
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], format!("Modificateurs du cue « {name} » enregistrés"));
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).modifiers.size, 0.5);
        // The properties dialog never sends them, and keeps them.
        assert_eq!(t.request("POST", "/api/cues/slot", &format!(r#"{{"id":"{cue}","layer":2}}"#)).0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).modifiers.size, 0.5);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).modifiers.size, 1.0);
        assert_eq!(t.request("POST", "/api/redo", "{}").0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).modifiers.size, 0.5);
        // Show mode: moves allowed, storing refused.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(control(&t, &format!("cue.{cue}.size"), 0.8), 200);
        assert_eq!(control(&t, "smoothing.on", 1.0), 200);
        assert_eq!(t.request("POST", "/api/cues/modifiers", &format!(r#"{{"id":"{cue}"}}"#)).0, 423);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        assert!(t.shared.lock().unwrap().mixer.smoothing.enabled);
        // Cleared.
        assert_eq!(t.request("POST", "/api/cues/modifiers", &format!(r#"{{"id":"{cue}","clear":true}}"#)).0, 200);
        assert!(crate::levels::is_neutral(&t.shared.lock().unwrap().deck.slot(&cue).modifiers));
        assert!(!arm_status(&t)["armed"].as_bool().unwrap(), "nothing here arms");
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
        assert_eq!(view["conditions"].as_array().unwrap().len(), 6);
        assert_eq!(view["phrase"], crate::audience::PHRASE);
        let (status, body) = unlock(crate::audience::PHRASE);
        assert_eq!(status, 409);
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["missing"].as_array().unwrap().len(), 6, "{body}");
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
        // The venue's profile allows it (T-260): a loosening, confirmed.
        let allow = json!({ "action": "save", "name": "Club", "audience_scan_allowed": true });
        assert_eq!(t.request("POST", "/api/safety/profiles", &allow.to_string()).0, 409);
        let mut confirmed = allow.clone();
        confirmed["confirm_loosen"] = json!(true);
        assert_eq!(t.request("POST", "/api/safety/profiles", &confirmed.to_string()).0, 200);
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

    /// T-274: a prepared cue changes `/api/preview-frame`, never
    /// `/api/frame`; « Envoyer » plays it; all of it in show mode too.
    #[test]
    fn prepare_previews_apart_and_send_plays_the_cue() {
        let t = TestServer::start(false);
        let js = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap();
        let id = t.shared.lock().unwrap().presets[0].id.clone();
        let before = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(js(&t.request("GET", "/api/preview-frame", "").1)["points"], json!([]));
        assert_eq!(t.request("POST", "/api/prepared/send", "").0, 409);
        assert_eq!(t.request("POST", "/api/prepared", r#"{"id":"pas-un-cue"}"#).0, 404);
        t.shared.lock().unwrap().show_mode.on = true;
        let (status, body) = t.request("POST", "/api/prepared", &json!({ "id": id }).to_string());
        assert_eq!(status, 200, "{body}");
        assert_eq!(js(&body)["cue"]["id"], json!(id));
        let (status, body) = t.request("POST", "/api/prepared/adjust", r#"{"scale":0.4,"color":[0,255,0]}"#);
        assert_eq!(status, 200, "{body}");
        assert_eq!(js(&body)["cue"]["color"], json!([0, 255, 0]));
        let preview = js(&t.request("GET", "/api/preview-frame", "").1);
        assert!(!preview["points"].as_array().unwrap().is_empty(), "the prepared view draws");
        let after = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(after["points"], before["points"], "the output frame is untouched");
        assert_eq!(after["prepared"]["cue"]["id"], json!(id), "the page sees what is prepared");
        assert!(after["cues"]["active"].as_array().unwrap().is_empty(), "nothing plays");
        assert_eq!(t.request("POST", "/api/prepared/send", "").0, 200);
        let sent = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(sent["cues"]["active"][0]["cue"], json!(id));
        assert_eq!(sent["prepared"]["cue"], json!(null));
        assert_eq!(sent["armed"], json!(false), "« Envoyer » never arms");
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

    /// T-428: `ROUTES` lists every route `dispatch` serves (scanned from
    /// this file), and nothing that isn't one; the remote refuses them all.
    #[test]
    fn every_route_is_in_the_table() {
        let src = include_str!("web.rs");
        let start = src.find("fn dispatch(").unwrap();
        let body = &src[start..start + src[start..].find("\n}\n").unwrap()];
        let quoted = |s: &str| s.strip_prefix('"').and_then(|r| r.split('"').next()).map(str::to_string);
        let mut found: Vec<(String, String)> = Vec::new();
        for (i, _) in body.match_indices("(Method::") {
            let rest = &body[i + "(Method::".len()..];
            let (method, rest) = rest.split_once(", ").unwrap();
            if let Some(path) = quoted(rest) {
                found.push((method.to_uppercase(), path));
            }
        }
        for line in body.lines().filter(|l| l.trim_start().starts_with('(') && l.contains(" if ")) {
            let method = if line.trim_start().starts_with("(method,") { "*".to_string() } else { line.trim_start()[9..].split(',').next().unwrap().to_uppercase() };
            for (i, _) in line.match_indices("p.starts_with(") {
                found.push((method.clone(), format!("{}*", quoted(&line[i + 14..]).unwrap())));
            }
            for (i, _) in line.match_indices("p == ") {
                found.push((method.clone(), quoted(&line[i + 5..]).unwrap()));
            }
        }
        assert!(found.len() > 150, "the scan found {}", found.len());
        for (method, path) in &found {
            assert!(ROUTES.iter().any(|&(m, p, _)| m == method && p == path), "{method} {path} missing from web::ROUTES");
        }
        for &(m, p, _) in ROUTES {
            let fast = ["/", "/beam3d.js", "/api/estop", "/api/events", "/api/frame.bin"].contains(&p);
            assert!(fast || found.iter().any(|(fm, fp)| fm == m && fp == p), "{m} {p} in ROUTES but not served");
        }
        for (m, p) in [("POST", "/api/arm"), ("POST", "/api/estop/reset"), ("POST", "/api/heartbeat")] {
            assert!(ROUTES.contains(&(m, p, RouteClass::Arm)), "{p}");
        }
        assert!(is_known_route("POST", "/api/timeline/play") && is_known_route("GET", "/api/midi/devices") && !is_known_route("GET", "/nope"));
    }

    /// T-428: the remote access over the main API: off by default, on
    /// and off from here, sessions listed and kicked, the main server
    /// untouched (still 127.0.0.1), and nothing armed.
    #[test]
    fn the_remote_access_over_the_main_api() {
        let t = TestServer::start(false);
        assert!(t.addr.ip().is_loopback());
        let v = js(&t.request("GET", "/api/remote", "").1);
        assert_eq!((v["enabled"].clone(), v["port"].clone(), v["code"].clone()), (json!(false), json!(crate::remote::DEFAULT_PORT), json!("")));
        assert_eq!(t.request("POST", "/api/remote", r#"{"enabled":true,"port":80}"#).0, 409);
        assert_eq!(t.request("POST", "/api/remote", r#"{"enabled":"yes"}"#).0, 400);
        let (st, body) = t.request("POST", "/api/remote", r#"{"enabled":true,"port":0}"#);
        assert_eq!(st, 200, "{body}");
        let v = js(&body);
        assert_eq!(v["enabled"], true);
        let code = v["code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 6);
        let port = v["bound_port"].as_u64().unwrap() as u16;
        assert_eq!(v["urls"][0], format!("http://127.0.0.1:{port}"));
        // Pair a remote on its own port, see it listed, kick it.
        let raddr = SocketAddr::from(([127, 0, 0, 1], port));
        assert_eq!(http(raddr, "POST", "/pair", &json!({ "code": code, "name": "iPad" }).to_string()).0, 200);
        assert_eq!(http(raddr, "POST", "/api/heartbeat", "{}").0, 403, "no heartbeat from a remote");
        assert_eq!(http(raddr, "POST", "/api/arm", r#"{"on":true}"#).0, 403);
        let v = js(&t.request("GET", "/api/remote", "").1);
        assert_eq!(v["sessions"][0]["name"], "iPad");
        let v = js(&t.request("POST", "/api/remote/kick", r#"{"name":"iPad"}"#).1);
        assert_eq!((v["kicked"].clone(), v["sessions"].as_array().unwrap().len()), (json!(1), 0));
        // Show mode: cutting is allowed, opening is not.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/remote", r#"{"enabled":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/remote", r#"{"enabled":true}"#).0, 423);
        assert!(TcpStream::connect_timeout(&raddr, Duration::from_millis(500)).is_err(), "port closed");
        assert_eq!(js(&t.request("GET", "/api/remote", "").1)["enabled"], false);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-432/T-433: the notices since an id, their last id and the cached
    /// « project modified » flag in the page's state; the page itself is
    /// left with a single `fetch(` (the one in `api()`).
    #[test]
    fn notices_and_the_modified_flag_reach_the_page() {
        let t = TestServer::start(false);
        let frame = |t: &TestServer| serde_json::from_str::<serde_json::Value>(&t.request("GET", "/api/frame", "").1).unwrap();
        let f = frame(&t);
        assert_eq!((f["project_modified"].clone(), f["notices_last"].clone()), (json!(false), json!(0)));
        let first = {
            let mut s = t.shared.lock().unwrap();
            s.project.last_modified = true;
            let a = s.notices.push(crate::notices::Level::Warning, "Projet", "un".into(), None);
            s.notices.push(crate::notices::Level::Error, "Audio", "deux".into(), Some("essai"));
            a
        };
        let f = frame(&t);
        assert_eq!((f["project_modified"].clone(), f["notices_last"].clone()), (json!(true), json!(2)));
        let (code, body) = t.request("GET", &format!("/api/notices?since={first}"), "");
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["last"], 2);
        assert_eq!(v["notices"].as_array().unwrap().len(), 1);
        assert_eq!((v["notices"][0]["source"].clone(), v["notices"][0]["level"].clone(), v["notices"][0]["key"].clone()), (json!("Audio"), json!("error"), json!("essai")));
        // Read only.
        assert_ne!(t.request("POST", "/api/notices", "{}").0, 200);
        assert_eq!(t.request("GET", "/api/project/quit-check", "").0, 200);
    }

    /// T-432: every request of the page goes through `api()`: the page
    /// holds a single `fetch(`, inside it (the 3D module has none).
    #[test]
    fn the_page_fetches_only_through_api() {
        // The heartbeat worker's source runs apart, without the page: its
        // own fetch is the only other one.
        let start = INDEX_HTML.find("const HB_WORKER = `").expect("heartbeat worker");
        let end = start + INDEX_HTML[start..].find("`;").expect("end of the worker");
        let page = format!("{}{}", &INDEX_HTML[..start], &INDEX_HTML[end..]);
        let body = page.split("async function api(").nth(1).expect("api() in the page");
        let api_end = body.find("\n}\n").expect("end of api()");
        assert_eq!(page.matches("fetch(").count(), 1, "one fetch( in the page");
        assert!(body[..api_end].contains("fetch("), "and it is in api()");
        assert!(!BEAM3D_JS.contains("fetch("));
    }

    /// T-431: every slider of the page (static, or in a template) has a
    /// default to go back to: the registry's (`data-control`) or its own
    /// `data-default`.
    #[test]
    fn every_slider_of_the_page_has_a_default() {
        let mut n = 0;
        for tag in INDEX_HTML.split('<').filter(|t| t.starts_with("input ") && t.contains(r#"type="range""#)) {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            assert!(tag.contains("data-control=") || tag.contains("data-default="), "no default: <{tag}>");
            n += 1;
        }
        assert!(n > 80, "{n} sliders found");
    }

    /// The page's static markup: without its scripts, styles and comments
    /// (a `<button` inside a script is a template, checked in e2e by axe).
    fn page_markup() -> String {
        let mut out = String::new();
        let mut rest = INDEX_HTML;
        loop {
            let next = ["<script", "<style", "<!--"].iter().filter_map(|t| rest.find(t).map(|i| (i, *t))).min();
            let Some((i, tag)) = next else { break };
            out.push_str(&rest[..i]);
            let end = match tag { "<script" => "</script>", "<style" => "</style>", _ => "-->" };
            let j = rest[i..].find(end).expect("closed") + i + end.len();
            rest = &rest[j..];
        }
        out.push_str(rest);
        out
    }

    /// Visible text of a piece of markup: tags (and whole SVGs) left out.
    fn markup_text(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find('<') {
            out.push_str(&rest[..i]);
            let after = &rest[i..];
            let end = if after.starts_with("<svg") { after.find("</svg>").map(|j| j + 6) } else { after.find('>').map(|j| j + 1) };
            rest = &after[end.unwrap_or(after.len())..];
        }
        out.push_str(rest);
        out
    }

    /// T-436: a button whose text is only a symbol (« ◀ », « ÷2 », « ? »)
    /// says what it does to a screen reader: it has an `aria-label`. A text
    /// is readable when it holds a word of two letters or digits at least.
    #[test]
    fn every_symbol_button_has_an_aria_label() {
        let page = page_markup();
        let mut n = 0;
        for part in page.split("<button").skip(1) {
            let open = &part[..part.find('>').expect("button tag")];
            let body = &part[open.len() + 1..part.find("</button>").expect("closed button")];
            let text = markup_text(body);
            let readable = text.split(|c: char| !c.is_alphanumeric()).any(|w| w.chars().count() >= 2);
            assert!(readable || open.contains("aria-label"), "<button{open}> {:?} needs an aria-label", text.trim());
            n += 1;
        }
        assert!(n > 300, "{n} buttons found");
        for (sel, label) in [("tempo.half\"", "Tempo divisé par deux"), ("tempo.double\"", "Tempo doublé"), ("tempo.nudge_up\"", "Avancer la phase"), ("tempo.nudge_down\"", "Retarder la phase")] {
            assert!(page.contains(&format!("data-ctl=\"{sel} aria-label=\"{label}\"")), "{label}");
        }
    }

    /// T-436: every form control of the markup has a name: a `<label for>`,
    /// a wrapping `<label>`, `aria-label` / `aria-labelledby`, or the
    /// `<label>` right before it (the page names it with that label's id in
    /// `aria-labelledby` at start, « accessibilité (T-436) » in the page).
    #[test]
    fn every_input_has_a_label() {
        let page = page_markup();
        let fors: std::collections::HashSet<&str> = page.split("<label").skip(1)
            .filter_map(|l| { let open = &l[..l.find('>')?]; let i = open.find("for=\"")? + 5; Some(&open[i..i + open[i..].find('"')?]) })
            .collect();
        let mut n = 0;
        for tag in ["<input", "<select", "<textarea"] {
            let mut from = 0;
            while let Some(i) = page[from..].find(tag).map(|i| i + from) {
                from = i + tag.len();
                let open = &page[i..i + page[i..].find('>').expect("tag")];
                if !open[tag.len()..].starts_with([' ', '>']) || open.contains("type=\"hidden\"") { continue; }
                n += 1;
                if open.contains("aria-label") { continue; }
                let id = open.find(" id=\"").map(|j| &open[j + 5..j + 5 + open[j + 5..].find('"').unwrap()]);
                if id.is_some_and(|id| fors.contains(id)) { continue; }
                let before = &page[..i];
                let wrapped = before.rfind("<label").is_some_and(|o| before.rfind("</label>").is_none_or(|c| c < o));
                let sibling = before.trim_end().ends_with("</label>") && id.is_some();
                assert!(wrapped || sibling, "{open}> has no label");
            }
        }
        assert!(n > 200, "{n} controls found");
    }

    /// T-429: the global search finds the RÉGLAGES sections by their
    /// `data-search` names, and opens from the single shortcut table.
    #[test]
    fn settings_sections_are_searchable() {
        for name in ["Sécurité", "Calibration", "Contrôleur MIDI", "Sorties et projecteurs", "Lieu et projecteurs", "Entrées externes", "Copie de sauvegarde", "Police du texte"] {
            assert!(INDEX_HTML.contains(&format!("data-search=\"{name}\"")), "{name}");
        }
        let settings = INDEX_HTML.split("id=\"settingsPanel\"").nth(1).expect("RÉGLAGES").split("</main>").next().unwrap();
        assert!(settings.split("<details").skip(1).all(|d| d.split('>').next().unwrap().contains("data-search=")), "every RÉGLAGES section is indexed");
        assert!(INDEX_HTML.contains("{ id: 'search', group: 'Recherche', keys: ['k', 'f'], meta: true"));
    }

    /// T-282: the night mode dims the neutral and accent tokens only; the
    /// state colours (armed / danger, warning, playing) and the undimmed
    /// safety tokens are never redefined, and touch mode leaves the laser
    /// button and ARRÊT out of the long press.
    #[test]
    fn night_mode_never_dims_the_safety_colours() {
        let start = INDEX_HTML.find("---------- Mode nuit (T-282)").expect("night mode CSS");
        let end = start + INDEX_HTML[start..].find("---------- Mode tactile (T-282)").expect("touch mode CSS");
        let night = &INDEX_HTML[start..end];
        assert!(night.contains("--text:"), "night mode redefines the text token");
        for token in ["--danger:", "--warn:", "--ok:", "--text-full:", "--accent-full:"] {
            assert!(!night.contains(token), "night mode must not redefine {token}");
        }
        let skip = INDEX_HTML.lines().find(|l| l.starts_with("const LONG_PRESS_SKIP")).expect("long press skip list");
        for id in ["#armBtn", "#estopBtn", "#blackoutBtn", "#estopReset"] {
            assert!(skip.contains(id), "{id} is out of the long press");
        }
    }

    /// T-246: the latency API reads and sets the *Décalage de sortie*,
    /// clamped, refuses a non-number, never arms and never moves the clock.
    #[test]
    fn latency_api_sets_the_offset_clamped_without_arming() {
        let t = TestServer::start(false);
        let (status, body) = t.request("GET", "/api/latency", "");
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((v["config"]["output_offset_ms"].as_f64(), v["min_ms"].as_f64(), v["max_ms"].as_f64()), (Some(0.0), Some(-100.0), Some(100.0)));
        assert!(v["config"]["analysis_delay_ms"].as_f64().unwrap() > 0.0);
        assert!(v["output"]["ms"].as_f64().unwrap() > 0.0);
        let bpm = t.shared.lock().unwrap().tempo.bpm;
        let (status, body) = t.request("POST", "/api/latency", r#"{"output_offset_ms":40}"#);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["config"]["output_offset_ms"].as_f64(), Some(40.0));
        assert_eq!(t.request("POST", "/api/latency", r#"{"output_offset_ms":1e6}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().tempo.output_offset_ms(), 100.0);
        assert_eq!(t.request("POST", "/api/latency", r#"{"output_offset_ms":"x"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/latency", r#"{}"#).0, 400);
        let s = t.shared.lock().unwrap();
        assert_eq!(s.tempo.output_offset_ms(), 100.0, "a refused value changes nothing");
        assert_eq!(s.tempo.bpm, bpm);
        assert!(!s.gate.is_armed(), "the offset never arms");
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
    fn fonts_are_listed_and_the_text_tool_uses_them() {
        let t = TestServer::start(false);
        let (status, body) = t.request("GET", "/api/fonts", "");
        assert_eq!(status, 200);
        let list: serde_json::Value = serde_json::from_str(&body).unwrap();
        let ids: Vec<&str> = list.as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["segments", "hershey-simple", "hershey-double", "relief"]);
        assert_eq!(list[3]["name_fr"], "Relief");
        assert_eq!(list[3]["accents"], true);
        let strokes = |body: &str| -> serde_json::Value {
            let (status, out) = t.request("POST", "/api/figures/text", body);
            assert_eq!(status, 200, "{out}");
            serde_json::from_str(&out).unwrap()
        };
        // Without a font: the segment font, as before.
        let seg = strokes(r#"{"text":"hi","size":0.3}"#);
        assert_eq!(seg["font"], "segments");
        let flat = |v: &serde_json::Value| -> Vec<f64> { v.as_array().unwrap().iter().flat_map(|s| s.as_array().unwrap().iter().flat_map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])).collect() };
        let old = crate::fonts::text_strokes("HI", 0.3);
        let old: Vec<f64> = old.iter().flatten().flat_map(|p| [p[0] as f64, p[1] as f64]).collect();
        let got = flat(&seg["strokes"]);
        assert_eq!(got.len(), old.len());
        assert!(got.iter().zip(&old).all(|(a, b)| (a - b).abs() < 1e-6));
        let relief = strokes(r#"{"text":"Été","size":0.3,"font":"relief"}"#);
        assert_eq!(relief["font"], "relief");
        let pts = flat(&relief["strokes"]);
        assert!(pts.len() > 20 && pts.iter().all(|v| v.abs() <= 1.0));
        assert_eq!(strokes(r#"{"text":"A","font":"nope"}"#)["font"], "segments");
        // 128 characters at most.
        let long = strokes(&json!({ "text": "I".repeat(300), "size": 0.01 }).to_string());
        assert_eq!(long["strokes"].as_array().unwrap().len(), crate::fonts::MAX_TEXT_CHARS);
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
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut body = raw[split + 4..].to_vec();
        // A long answer (over the server's 32 kB threshold) comes chunked.
        if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
            let (mut out, mut rest) = (Vec::new(), &body[..]);
            loop {
                let eol = rest.windows(2).position(|w| w == b"\r\n").unwrap();
                let n = usize::from_str_radix(String::from_utf8_lossy(&rest[..eol]).trim(), 16).unwrap();
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&rest[eol + 2..eol + 2 + n]);
                rest = &rest[eol + 2 + n + 2..];
            }
            body = out;
        }
        (head, body)
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
            ("/api/cues/modifiers", r#"{"id":"tunnels-001"}"#),
            ("/api/grid", r#"{"cols":6,"rows":4}"#),
            ("/api/grid/page", r#"{"action":"new"}"#),
            ("/api/grid/move", r#"{"from":[1,0],"to":[0,null]}"#),
            ("/api/grid/clear", r#"{"page":0,"slot":0}"#),
            ("/api/grid/pages", r#"{"op":"add"}"#),
            ("/api/grid/edit", r#"{"op":"delete","from":[0,0]}"#),
            ("/api/cues/store", r#"{"page":0,"slot":0}"#),
            ("/api/cues/update", "{}"),
            ("/api/cues/program", r#"{"id":"tunnels-001","program":{}}"#),
            ("/api/cues/program/store", r#"{"id":"tunnels-001","program":{}}"#),
            ("/api/timeline/edit", "{}"),
            ("/api/timeline/autoshow", r#"{"file":"x.wav"}"#),
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
        // « En cours » (T-423): reading and stopping are playback.
        let (code, body) = t.request("GET", "/api/running", "");
        let running: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(code, 200);
        assert_eq!(running["cues"][0]["source_label"], "Interface", "{body}");
        assert_eq!(running["masters"][0]["id"], "master.brightness", "{body}");
        assert_eq!(running["masters"][0]["source_label"], "Interface", "{body}");
        assert_eq!(t.request("POST", "/api/running/stop", r#"{"kind":"all"}"#).0, 200);
        assert!(t.shared.lock().unwrap().deck.active.is_empty());
        assert_eq!(t.request("POST", "/api/running/stop", r#"{"kind":"cue","instance":999}"#).0, 400);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
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

    /// T-437: the tour is offered on a fresh data folder, recorded once
    /// seen, and never arms (it is a view, allowed in show mode too).
    #[test]
    fn onboarding_is_reported_and_recorded_without_arming() {
        let t = TestServer::start(false);
        let js = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["first_run"], true);
        assert_eq!(js(&t.request("GET", "/api/onboarding", "").1)["tour_done"], false);
        let (status, body) = t.request("POST", "/api/onboarding", r#"{"tour_done":true}"#);
        assert_eq!((status, js(&body)["tour_done"].clone()), (200, json!(true)));
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["first_run"], false);
        assert_eq!(t.request("POST", "/api/onboarding", "{}").0, 400);
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/onboarding", r#"{"tour_done":false}"#).0, 200, "not an edit: allowed in show mode");
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["first_run"], true);
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
    }

    /// T-438: the help pages come from the binary; unknown or malformed
    /// slugs get 404.
    #[test]
    fn help_pages_are_served_from_the_binary() {
        let t = TestServer::start(false);
        let js = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        let list = js(&t.request("GET", "/api/help", "").1);
        assert_eq!(list.as_array().unwrap().len(), crate::help::PAGES.len());
        assert!(list[0]["slug"].is_string() && list[0]["title_fr"].is_string());
        assert!(list[0].get("body_html").is_none(), "the list is light");
        let full = js(&t.request("GET", "/api/help?full=1", "").1);
        assert!(full.as_array().unwrap().iter().all(|p| p["body_html"].is_string()));
        let (status, body) = t.request("GET", "/api/help/securite-laser", "");
        assert_eq!(status, 200);
        assert_eq!(js(&body)["title_fr"], "Sécurité laser : armer, arrêter, réinitialiser");
        for bad in ["/api/help/inconnue", "/api/help/..%2Fweb", "/api/help/Live-direct", "/api/help/"] {
            assert_eq!(t.request("GET", bad, "").0, 404, "{bad}");
        }
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

    /// T-409: Art-Net timecode injection, `--test-hooks` only, taken only
    /// while the source is Art-Net and TC-IN armed.
    #[test]
    fn the_timecode_hook_exists_only_with_test_hooks() {
        let t = TestServer::start(false);
        let sim = r#"{"frames":250,"rate":"25","count":6}"#;
        assert_eq!(t.request("POST", "/api/test/timecode", sim).0, 404);
        t.shared.lock().unwrap().test_hooks = true;
        let (status, body) = t.request("POST", "/api/test/timecode", sim);
        assert_eq!((status, serde_json::from_str::<serde_json::Value>(&body).unwrap()["accepted"].as_u64()), (200, Some(0)), "not wanted yet");
        assert_eq!(t.request("POST", "/api/timecode", r#"{"source":"artnet"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timecode/arm", r#"{"on":true}"#).0, 200);
        crate::timecode::sync(&mut t.shared.lock().unwrap());
        t.shared.lock().unwrap().netin.hub.set_timecode(true, None);
        let (status, body) = t.request("POST", "/api/test/timecode", sim);
        assert_eq!((status, serde_json::from_str::<serde_json::Value>(&body).unwrap()["accepted"].as_u64()), (200, Some(6)));
        assert_eq!(t.request("POST", "/api/test/timecode", r#"{"frames":1,"rate":"50"}"#).0, 400);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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
    fn timecode_routes_set_tc_in_and_never_arm() {
        let t = TestServer::start(false);
        let get = |path| serde_json::from_str::<serde_json::Value>(&t.request("GET", path, "").1).unwrap();
        let tc = get("/api/timecode");
        assert_eq!((tc["armed"].as_bool(), tc["status"].as_str(), tc["config"]["source"].as_str()), (Some(false), Some("off"), Some("off")));
        assert_eq!(t.request("POST", "/api/timecode/arm", r#"{"on":true}"#).0, 409, "no source yet");
        assert_eq!(t.request("POST", "/api/timecode", r#"{"source":"ltc","ltc_channel":2,"offset_s":3600}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timecode", r#"{"after_roll_s":-1}"#).0, 400);
        assert_eq!(t.request("POST", "/api/timecode", r#"{"bogus":1}"#).0, 400);
        assert_eq!(t.request("POST", "/api/timecode/arm", r#"{"on":true,"source":"midi"}"#).0, 403, "from the UI only");
        let (status, body) = t.request("POST", "/api/timecode/arm", r#"{"toggle":true}"#);
        assert_eq!(status, 200);
        let tc: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((tc["armed"].as_bool(), tc["config"]["ltc_channel"].as_u64()), (Some(true), Some(2)));
        let state = get("/api/state");
        assert_eq!((state["timecode"]["armed"].as_bool(), state["armed"].as_bool()), (Some(true), Some(false)));
        assert_eq!(get("/api/frame")["timecode"]["source"], "ltc");
        // Show mode: TC-IN stays a live switch, the settings are locked.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/timecode", r#"{"offset_s":0}"#).0, 423);
        assert_eq!(t.request("POST", "/api/timecode/arm", r#"{"on":false}"#).0, 200);
        assert!(!t.shared.lock().unwrap().timecode.armed);
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
        assert_eq!(r["warnings"], json!([]));
        assert_eq!(t.shared.lock().unwrap().shows.load("Editeur web").unwrap().end(), 0.0, "not saved yet");

        // Envelopes (T-163): a bus event needs no cue; an unknown target is
        // accepted with a warning (ignored at playback), never armed.
        let mut with_env = show.clone();
        with_env["tracks"][0]["events"][0]["envelopes"] = json!([{ "target": "look.size", "keys": [{ "at": 0, "value": 0.5, "ease": "smooth" }] }]);
        with_env["tracks"][1]["kind"] = json!("bus");
        with_env["tracks"][1]["events"] = json!([{ "start": 0, "len": 1, "envelopes": [{ "target": "transport.arm", "mode": "relative", "keys": [{ "at": 0, "value": 1 }] }] }]);
        let (status, body) = t.request("POST", "/api/timeline/edit", &json!({ "show": with_env }).to_string());
        assert_eq!(status, 200, "{body}");
        let r = json(&body);
        assert_eq!(r["warnings"].as_array().unwrap().len(), 1);
        assert!(r["warnings"][0].as_str().unwrap().contains("transport.arm"));
        assert_eq!(r["show"]["tracks"][0]["events"][0]["envelopes"][0]["keys"][0]["ease"], "smooth");
        assert!(!t.shared.lock().unwrap().gate.is_armed());
        assert_eq!(t.request("POST", "/api/timeline/edit", &json!({ "show": show }).to_string()).0, 200);

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

    /// T-445: `POST /api/timeline/edit` refuses a change to a track locked
    /// before and after it, even sent by hand; locking alone, unlocking and
    /// the free tracks pass; the lock is saved with the show.
    #[test]
    fn timeline_locked_tracks_are_guarded() {
        let t = TestServer::start(false);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let json = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap();
        assert_eq!(t.request("POST", "/api/timeline/new", r#"{"name":"Verrou","time_base":"seconds"}"#).0, 200);
        let mut show = json(&t.request("GET", "/api/timeline", "").1)["show"].clone();
        assert_eq!(show["tracks"][0]["locked"], false);
        show["tracks"][0]["events"] = json!([{ "id": 1, "start": 2.0, "len": 2.0, "source": { "kind": "cue", "id": cue } }]);
        let edit = |s: &serde_json::Value| t.request("POST", "/api/timeline/edit", &json!({ "show": s }).to_string());
        let (status, body) = edit(&show);
        assert_eq!(status, 200, "{body}");
        let mut show = json(&body)["show"].clone();

        show["tracks"][0]["locked"] = json!(true);
        let (status, body) = edit(&show);
        assert_eq!(status, 200, "locking alone: {body}");
        let locked = json(&body)["show"].clone();
        assert_eq!(locked["tracks"][0]["locked"], true);
        assert_eq!(edit(&locked).0, 200, "the server's copy sent back unchanged");

        let mut moved = locked.clone();
        moved["tracks"][0]["events"][0]["start"] = json!(2.004);
        let (status, body) = edit(&moved);
        assert_eq!((status, body.contains("piste verrouillée")), (400, true), "{body}");
        let mut gone = locked.clone();
        gone["tracks"].as_array_mut().unwrap().remove(0);
        assert_eq!(edit(&gone).0, 400, "a locked track can't be deleted");
        let mut free = locked.clone();
        free["tracks"][1]["events"] = json!([{ "start": 0.0, "len": 1.0, "source": { "kind": "cue", "id": cue } }]);
        free["tracks"][0]["mute"] = json!(true);
        assert_eq!(edit(&free).0, 200, "the free track and Muet");
        assert_eq!(t.shared.lock().unwrap().timeline.show.as_ref().unwrap().tracks[0].events[0].start, 2.0);

        assert_eq!(t.request("POST", "/api/timeline/save", "{}").0, 200);
        assert!(t.shared.lock().unwrap().shows.load("Verrou").unwrap().tracks[0].locked, "saved with the show");
        let mut unlocked = json(&t.request("GET", "/api/timeline", "").1)["show"].clone();
        unlocked["tracks"][0]["locked"] = json!(false);
        unlocked["tracks"][0]["events"][0]["start"] = json!(6.0);
        assert_eq!(edit(&unlocked).0, 200, "unlocked in the same edit");
        assert_eq!(t.shared.lock().unwrap().timeline.show.as_ref().unwrap().tracks[0].events[0].start, 6.0);
    }

    /// T-124 to T-129: the built-in examples open as an unsaved copy
    /// (nothing written), play without arming, and save only under a name
    /// of their own. Opening one is a load, allowed in show mode.
    #[test]
    fn timeline_examples_open_as_a_copy() {
        let t = TestServer::start(false);
        let json = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap();
        let list = json(&t.request("GET", "/api/timeline/examples", "").1);
        assert_eq!(list.as_array().unwrap().len(), 6);
        assert_eq!((list[3]["id"].as_str(), list[3]["bpm"].as_f64()), (Some("attaque-hardstyle"), Some(150.0)));
        assert_eq!(t.request("POST", "/api/timeline/load", r#"{"example":"nope"}"#).0, 404);
        let (status, body) = t.request("POST", "/api/timeline/load", r#"{"example":"bloc-techno"}"#);
        assert_eq!(status, 200, "{body}");
        let st = json(&body);
        assert_eq!((st["name"].as_str(), st["example"].as_bool(), st["modified"].as_bool()), (Some("Bloc techno"), Some(true), Some(false)));
        assert_eq!(st["time_base"], "beats");
        assert!(t.shared.lock().unwrap().shows.load("Bloc techno").is_err(), "nothing written");
        let (_, body) = t.request("GET", "/api/timeline", "");
        assert_eq!(json(&body)["show"]["tempo_map"][0]["bpm"], 130.0);
        assert_eq!(t.request("POST", "/api/timeline/play", "").0, 200);
        assert_eq!(json(&t.request("GET", "/api/state", "").1)["armed"], false, "an example never arms");

        let (status, body) = t.request("POST", "/api/timeline/save", "{}");
        assert_eq!((status, body.contains("nom")), (400, true), "{body}");
        t.shared.lock().unwrap().shows.save(&crate::timeline::Show { name: "Exemple pris".into(), ..Default::default() }).unwrap();
        assert_eq!(t.request("POST", "/api/timeline/save", r#"{"name":"Exemple pris"}"#).0, 409, "never overwrites");
        let (status, body) = t.request("POST", "/api/timeline/save", r#"{"name":"Mon bloc techno"}"#);
        assert_eq!((status, json(&body)["example"].as_bool()), (200, Some(false)), "{body}");
        assert_eq!(t.shared.lock().unwrap().shows.load("Mon bloc techno").unwrap().end(), 256.0);
        assert_eq!(t.request("POST", "/api/timeline/save", "{}").0, 200, "a saved copy is a show like any other");

        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/load", r#"{"example":"entree-dj"}"#).0, 200, "opening an example plays, it doesn't edit");
        assert_eq!(t.request("POST", "/api/timeline/save", r#"{"name":"Entree copie"}"#).0, 423);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        t.request("POST", "/api/timeline/stop", "");
    }

    /// T-165: a selection saved as a template, rendered on another region
    /// (nothing changes until the edit route), locked tracks refused,
    /// show mode refuses the edits, never arms.
    #[test]
    fn timeline_templates_over_the_api() {
        let t = TestServer::start(false);
        let json = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap();
        let dir = std::env::temp_dir().join(format!("laser-studio-test-tpl-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cue = {
            let mut s = t.shared.lock().unwrap();
            s.templates = crate::templates::TemplateStore::new(dir.join("templates"));
            s.presets[0].id.clone()
        };
        assert_eq!(t.request("POST", "/api/timeline/templates/render", r#"{"name":"X","from":0,"to":8}"#).0, 404);
        let mut show = crate::timeline::Show::new_empty("Modeles", crate::timeline::TimeBase::Beats);
        show.tracks[0].events.push(crate::timeline::Event { id: 1, start: 0.0, len: 4.0, source: crate::timeline::EventSource::Cue { id: cue.clone() }, ..Default::default() });
        show.tracks[1].events.push(crate::timeline::Event { id: 2, start: 4.0, len: 4.0, source: crate::timeline::EventSource::Cue { id: cue.clone() }, ..Default::default() });
        assert_eq!(t.request("POST", "/api/timeline/load", &json!({ "show": show }).to_string()).0, 200);

        let (status, body) = t.request("POST", "/api/timeline/templates", r#"{"name":"Deux temps","events":[1,2]}"#);
        assert_eq!(status, 200, "{body}");
        assert_eq!(json(&body)["template"]["slots"], json!(["A"]));
        assert_eq!(t.request("POST", "/api/timeline/templates", r#"{"name":"Deux temps","events":[1]}"#).0, 409);
        assert_eq!(t.request("POST", "/api/timeline/templates", r#"{"name":"Vide","events":[]}"#).0, 400);
        let list = json(&t.request("GET", "/api/timeline/templates", "").1);
        assert_eq!((list["templates"][0]["name"].as_str(), list["templates"][0]["builtin"].as_bool()), (Some("Deux temps"), Some(false)));
        assert!(list["categories"].as_array().unwrap().len() > 3);

        let req = json!({ "name": "Deux temps", "from": 8.0, "to": 16.0, "fit": "crop", "slots": { "A": { "cue": cue } } });
        let (status, body) = t.request("POST", "/api/timeline/templates/render", &req.to_string());
        assert_eq!(status, 200, "{body}");
        let r = json(&body);
        assert_eq!(r["added"], 2);
        assert_eq!((r["show"]["tracks"][0]["events"][1]["start"].as_f64(), r["show"]["tracks"][1]["events"][1]["start"].as_f64()), (Some(8.0), Some(12.0)));
        assert_eq!(t.shared.lock().unwrap().timeline.show.as_ref().unwrap().tracks[0].events.len(), 1, "render changes nothing");
        assert_eq!(t.request("POST", "/api/timeline/edit", &json!({ "show": r["show"] }).to_string()).0, 200);
        assert_eq!(t.shared.lock().unwrap().timeline.show.as_ref().unwrap().tracks[0].events.len(), 2);
        let no_slot = json!({ "name": "Deux temps", "from": 8.0, "to": 16.0 });
        assert_eq!(t.request("POST", "/api/timeline/templates/render", &no_slot.to_string()).0, 400);

        t.shared.lock().unwrap().timeline.show.as_mut().unwrap().tracks[1].locked = true;
        let (status, body) = t.request("POST", "/api/timeline/templates/render", &req.to_string());
        assert_eq!((status, body.contains("verrouillée")), (400, true), "{body}");

        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        for (path, body) in [("/api/timeline/templates", r#"{"name":"Autre","events":[1]}"#), ("/api/timeline/templates/render", "{}"), ("/api/timeline/templates/delete", r#"{"name":"Deux temps"}"#)] {
            assert_eq!(t.request("POST", path, body).0, 423, "{path}");
        }
        assert_eq!(t.request("GET", "/api/timeline/templates", "").0, 200, "the list is a read");
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/templates/delete", r#"{"name":"Deux temps"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/templates/delete", r#"{"name":"Deux temps"}"#).0, 404);
        assert_eq!(json(&t.request("GET", "/api/state", "").1)["armed"], false, "templates never arm");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// T-443: markers go out as CSV and a CSV is read back without changing
    /// the show; 409 without a show, 413 beyond 1 MB, 423 in show mode.
    #[test]
    fn timeline_markers_export_and_parse_without_editing() {
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        assert_eq!(t.request("GET", "/api/timeline/markers.csv", "").0, 409);
        assert_eq!(t.request("POST", "/api/timeline/markers/parse", "Name,Start\nA,1\n").0, 409);
        assert_eq!(t.request("POST", "/api/timeline/new", r#"{"name":"csv web"}"#).0, 200);
        let mut show = json(&t.request("GET", "/api/timeline", "").1)["show"].clone();
        show["markers"] = json!([{ "at": 4.0, "name": "Drop, fort", "color": [255, 0, 0] }, { "at": 1.0, "name": "Intro" }]);
        assert_eq!(t.request("POST", "/api/timeline/edit", &json!({ "show": show }).to_string()).0, 200);

        // The download: headers, then the rows in time order.
        let mut stream = TcpStream::connect(t.addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET /api/timeline/markers.csv HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, csv) = raw.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("text/csv; charset=utf-8"), "{head}");
        assert!(head.contains("filename=\"csv web-marqueurs.csv\""), "{head}");
        assert_eq!(csv, "#,Name,Start,Color\r\nM1,Intro,1.000,\r\nM2,\"Drop, fort\",4.000,FF0000\r\n");

        // Parsing it back changes nothing.
        let before = t.request("GET", "/api/timeline", "").1;
        let (status, body) = t.request("POST", "/api/timeline/markers/parse", &format!("{csv}M3,Mauvais,abc,\r\n"));
        assert_eq!(status, 200, "{body}");
        let parsed = json(&body);
        assert_eq!(parsed["markers"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["markers"][1]["name"], "Drop, fort");
        assert_eq!(parsed["skipped"][0]["line"], 4);
        assert_eq!(parsed["skipped"][0]["reason"], "temps illisible");
        let (status, body) = http_bytes(t.addr, "/api/timeline/markers/parse", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        assert_eq!((status, body.contains("binaire")), (400, true), "{body}");
        let (status, body) = t.request("POST", "/api/timeline/markers/parse", "");
        assert_eq!((status, body.contains("vide")), (400, true), "{body}");
        // Too big: refused from its announced length, before reading.
        let mut stream = TcpStream::connect(t.addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let big = vec![b'1'; crate::timeline_csv::MAX_CSV_BYTES + 1];
        write!(stream, "POST /api/timeline/markers/parse HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", big.len()).unwrap();
        let _ = stream.write_all(&big);
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
        assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"), "{}", String::from_utf8_lossy(&raw[..raw.len().min(80)]));
        let after = t.request("GET", "/api/timeline", "").1;
        assert_eq!(json(&before)["show"], json(&after)["show"], "parse never edits the show");
        assert_eq!(json(&after)["state"]["playing"], json(&before)["state"]["playing"]);
        assert_eq!(json(&t.request("GET", "/api/state", "").1)["armed"], false, "never arms");

        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/timeline/markers/parse", "Name,Start\nA,1\n").0, 423);
        assert_eq!(t.request("GET", "/api/timeline/markers.csv", "").0, 200, "reading is allowed");
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
    }

    /// T-442: recordings over the API. A timeline export runs on its own
    /// thread into the library (twice: the same bytes), downloads by a
    /// checked name; a second start while one runs is 409, invalid
    /// parameters 400, show mode 423 (cancel still works); never armed.
    #[test]
    fn ilda_recordings_write_into_the_library_without_arming() {
        use crate::timeline::{Event, EventSource, Show, TimeBase, Track};
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        let look = crate::engine::Settings { content: crate::engine::Content::Shape { shape: "circle".into() }, scale: 0.5, ..Default::default() };
        let mut show = Show::new_empty("webtest rec", TimeBase::Seconds);
        show.tracks = vec![Track { name: "P".into(), layer: 1, events: vec![Event { id: 1, start: 0.0, len: 2.0, source: EventSource::Look { settings: look }, ..Default::default() }], ..Default::default() }];
        t.shared.lock().unwrap().timeline.load(show);
        let info = json(&t.request("GET", "/api/ilda/record?show=webtest%20rec", "").1);
        assert_eq!((info["show"]["seconds"].as_f64(), info["show"]["audio"].as_bool()), (Some(2.0), Some(false)));
        assert_eq!(info["status"]["state"], "idle");

        for bad in [r#"{"fps":5}"#, r#"{"fps":61}"#, r#"{"seconds":0}"#, r#"{"seconds":601}"#, r#"{"name":"../x"}"#, r#"{"sd_number":0}"#, r#"{"sd_number":230}"#, r#"{"output":"ailleurs"}"#] {
            let (status, body) = t.request("POST", "/api/ilda/record", bad);
            assert_eq!(status, 400, "{bad}: {body}");
        }
        assert_eq!(t.request("POST", "/api/ilda/record", r#"{"source":{"kind":"timeline","show":"inconnu"}}"#).0, 404);
        assert_eq!(t.request("POST", "/api/ilda/record", r#"{"source":{"kind":"timeline","show":"webtest rec","from_s":1.5,"to_s":1.0}}"#).0, 400);

        let wait_done = || {
            for _ in 0..500 {
                let st = json(&t.request("GET", "/api/ilda/record", "").1)["status"].clone();
                if st["state"] != "recording" && st["state"] != "writing" {
                    return st;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the recording never ended");
        };
        let mut files = Vec::new();
        for name in ["webtest a", "webtest b"] {
            let req = json!({ "source": { "kind": "timeline", "show": "webtest rec" }, "fps": 25, "name": name });
            let (status, body) = t.request("POST", "/api/ilda/record", &req.to_string());
            assert_eq!(status, 200, "{body}");
            let st = wait_done();
            assert_eq!(st["state"], "done", "{st}");
            assert_eq!((st["frames"].as_u64(), st["total"].as_u64()), (Some(50), Some(50)));
            let file = st["file"].as_str().unwrap().to_string();
            assert_eq!(file, format!("{name}.ild"));
            let (status, bytes) = http_raw(t.addr, &format!("/api/ilda/record/file?name={}", file.replace(' ', "%20")));
            assert_eq!(status, 200);
            assert_eq!(&bytes[..4], b"ILDA");
            assert_eq!(crate::ilda::read(&bytes).unwrap().len(), 50);
            files.push(bytes);
        }
        assert_eq!(files[0], files[1], "the same timeline twice: the same bytes");
        // In the library, as cues of the « ILDA » page.
        let lib = json(&t.request("GET", "/api/media/ilda", "").1);
        assert_eq!(lib["files"].as_array().unwrap().len(), 2);
        // Download: only library names.
        assert_eq!(t.request("GET", "/api/ilda/record/file?name=..%2Fsecret.ild", "").0, 400);
        assert_eq!(t.request("GET", "/api/ilda/record/file?name=absent.ild", "").0, 404);

        // Live (no engine here: it stays recording): a second start is 409.
        assert_eq!(t.request("POST", "/api/ilda/record", r#"{"seconds":1}"#).0, 200);
        assert_eq!(t.request("POST", "/api/ilda/record", r#"{"seconds":1}"#).0, 409);
        // Show mode: starting is a file write (423), cancelling is not.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/ilda/record", r#"{"seconds":1}"#).0, 423);
        let (status, body) = t.request("POST", "/api/ilda/record/cancel", "");
        assert_eq!((status, json(&body)["cancelled"].as_bool()), (200, Some(true)));
        assert_eq!(json(&t.request("GET", "/api/ilda/record", "").1)["status"]["state"], "cancelled");
        assert!(!arm_status(&t)["armed"].as_bool().unwrap(), "never armed");
        let dir = t.shared.lock().unwrap().ilda.dir().to_path_buf();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Status and raw body bytes of a GET.
    fn http_raw(addr: SocketAddr, path: &str) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
        let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let body = raw[end + 4..].to_vec();
        if !head.contains("transfer-encoding: chunked") {
            return (status, body);
        }
        let (mut rest, mut out) = (&body[..], Vec::new());
        while let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") {
            let n = usize::from_str_radix(std::str::from_utf8(&rest[..eol]).unwrap().trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&rest[eol + 2..eol + 2 + n]);
            rest = &rest[eol + 2 + n + 2..];
        }
        (status, out)
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
        // The points per frame the files get (T-476): the output's own range.
        let fl = crate::power::OutputScan::default().frame_limits();
        assert_eq!(empty["outputs"][0]["min_points"], json!(fl.min));
        assert_eq!(empty["outputs"][0]["max_points"], json!(fl.max));
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
    fn the_exposure_estimate_reads_the_sheet_and_changes_nothing() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        // No power in the sheet: nothing to estimate, said.
        let (code, body) = t.request("GET", "/api/safety/exposure", "");
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert!(v["result"].is_null());
        assert!(v["disclaimer"].as_str().unwrap().contains("ne remplace pas"));
        // 1 W, 5 mm, 1 mrad; caps 100 % so the hand calculation of mpe.rs
        // applies: 10 m → 565.9 mW/cm², ratio 222, NOHD 218.6 m.
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"projector":{"power_mw":[300,400,300],"aperture_mm":5,"divergence_mrad":1}}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"limits":{"max_power":1,"max_color":[1,1,1]},"confirm_loosen":true}"#).0, 200);
        let snapshot = |t: &TestServer| {
            let s = t.shared.lock().unwrap();
            (s.outputs.active_limits(), s.gate.is_armed())
        };
        let before = snapshot(&t);
        let (code, body) = t.request("POST", "/api/safety/exposure", r#"{"exposure":{"distance_m":10,"attenuation_pct":0}}"#);
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        let r = &v["result"];
        assert!((r["irradiance_mw_cm2"].as_f64().unwrap() - 565.884).abs() < 0.1, "{r}");
        assert!((r["nohd_m"].as_f64().unwrap() - 218.64).abs() < 0.1, "{r}");
        assert!(r["ratio"].as_f64().unwrap() > 1.0);
        // Saved with the output, and nothing about the output changed.
        assert_eq!(js(&t.request("GET", "/api/safety/exposure", "").1)["settings"]["distance_m"], 10.0);
        assert_eq!(snapshot(&t), before);
        // Untrusted values refused.
        for bad in [r#"{"exposure":{"distance_m":-1}}"#, r#"{"exposure":{"attenuation_pct":100}}"#, r#"{"exposure":{"distance_m":"x"}}"#] {
            assert_eq!(t.request("POST", "/api/safety/exposure", bad).0, 400, "{bad}");
        }
        assert_eq!(t.request("GET", "/api/safety/exposure?output=nope", "").0, 400);
        // An edit: locked in show mode; reading is not.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/safety/exposure", r#"{"exposure":{"distance_m":5}}"#).0, 423);
        assert_eq!(t.request("GET", "/api/safety/exposure", "").0, 200);
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

    /// T-171: an output's scan limits over HTTP: shown with the rate, the
    /// alert and the warnings; out of range is 400; a faster `pps_max`
    /// is refused while armed; the projector sheet's `max_pps` warns.
    #[test]
    fn output_scan_limits_over_http() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let g = js(&t.request("GET", "/api/outputs/scan_limits", "").1);
        assert_eq!((g["output"].as_str(), g["pps"].as_u64(), g["alert_fps"].as_f64()), (Some("main"), Some(30_000), Some(35.0)));
        assert_eq!(g["scan"]["min_points"], 200);
        assert!(g["warnings"].as_array().unwrap().is_empty());
        assert_eq!(t.request("GET", "/api/outputs/scan_limits?output=nope", "").0, 400);
        let (code, reply) = t.request("POST", "/api/outputs/scan_limits", r#"{"scan":{"pps_max":25000,"min_points":300}}"#);
        assert_eq!(code, 200, "{reply}");
        let r = js(&reply);
        assert_eq!(r["scan"]["pps_min"], 10_000, "a missing field is its default");
        assert_eq!(r["warnings"].as_array().unwrap().len(), 1, "30 kpps above a 25 kpps max: {reply}");
        assert_eq!(t.shared.lock().unwrap().outputs.active().scan.min_points, 300);
        for bad in [r#"{"scan":{"pps_min":50000}}"#, r#"{"scan":{"min_fps":0}}"#, r#"{"scan":{"max_points":5}}"#, r#"{}"#] {
            assert_eq!(t.request("POST", "/api/outputs/scan_limits", bad).0, 400, "{bad}");
        }
        // Armed: faster refused, slower applies.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, reply) = t.request("POST", "/api/outputs/scan_limits", r#"{"scan":{"pps_max":60000}}"#);
        assert_eq!(code, 409, "{reply}");
        assert_eq!(js(&reply)["armed"], true);
        assert_eq!(t.request("POST", "/api/outputs/scan_limits", r#"{"scan":{"pps_max":20000}}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/scan_limits", r#"{"scan":{"pps_max":60000}}"#).0, 200);
        // A slower interface on the sheet: warned, nothing clamped.
        assert_eq!(t.request("POST", "/api/outputs/limits", r#"{"projector":{"max_pps":20000}}"#).0, 200);
        let g = js(&t.request("GET", "/api/outputs/scan_limits", "").1);
        assert_eq!(g["warnings"].as_array().unwrap().len(), 2, "{g}");
        assert_eq!(g["scan"]["pps_max"], 60_000);
        // The caps are untouched.
        assert_eq!(t.shared.lock().unwrap().outputs.active_limits(), crate::power::OutputLimits::default());
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
                let (path, body): (&str, String) = match rng.below(23) {
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
                    21 => {
                        // Speed groups (T-419), mangled now and then.
                        let mode = if rng.chance(0.5) { json!({ "type": "multiple", "mult": rng.pick(&[0.25, 0.5, 2.0, 3.0, -1.0]) }) } else { json!({ "type": "free_bpm", "bpm": rng.range(-50.0, 500.0) }) };
                        ("/api/speed-groups", json!({ "group": rng.below(6), "mode": mode, "rate": rng.range(-1.0, 4.0), "change_on_bar": rng.chance(0.5) }).to_string())
                    }
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
    fn scan_presets_are_listed_edited_assigned_and_never_arm() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let dir = std::env::temp_dir().join(format!("ls-scan-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        t.shared.lock().unwrap().scan_presets = crate::scan::ScanPresetStore::load_or_create(dir.join("scan_presets.json"));
        let v = js(&t.request("GET", "/api/scan-presets", "").1);
        let ids: Vec<&str> = v["builtin"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["beams", "graphics", "raster"]);
        assert_eq!(v["default"], serde_json::Value::Null, "« Auto » until set");
        assert_eq!(v["effective"]["beams"]["clamped"], false);

        // Built-ins can't be overridden; a user preset is clamped and saved.
        let (code, _) = t.request("POST", "/api/scan-presets", r#"[{"id":"beams","name":"X"}]"#);
        assert_eq!(code, 400);
        let (code, body) = t.request("POST", "/api/scan-presets", r#"[{"id":"u-1","name":"Logo lent","scan_pct":999}]"#);
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["user"][0]["scan_pct"], 150.0);
        assert!(dir.join("scan_presets.json").exists());

        // The output's default; an unknown one is refused, "" = Auto.
        assert_eq!(t.request("POST", "/api/scan-presets", r#"{"default":"u-9"}"#).0, 400);
        let v = js(&t.request("POST", "/api/scan-presets", r#"{"default":"raster"}"#).1);
        assert_eq!(v["default"], "raster");
        assert_eq!(t.shared.lock().unwrap().mixer.scan_default.as_deref(), Some("raster"));

        // A cell takes a preset through the existing route; deleting the
        // preset leaves it dangling (drawn with the default), listed.
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-001","scan":"u-1"}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot("tunnels-001").scan.as_deref(), Some("u-1"));
        let v = js(&t.request("POST", "/api/scan-presets", r#"{"user":[],"default":""}"#).1);
        assert_eq!(v["default"], serde_json::Value::Null);
        assert_eq!(v["dangling"][0]["id"], "tunnels-001");
        assert!(!t.shared.lock().unwrap().gate.is_armed());
        let _ = std::fs::remove_dir_all(&dir);
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

        // Zone groups (T-420): their masters move by control only.
        let grouped = json!({ "zones": [{ "id": 1 }, { "id": 2 }], "groups": [{ "name": "Centre", "zones": [1] }, { "name": "Côtés", "zones": [2], "master": 0.7 }] });
        assert_eq!(t.request("POST", "/api/projection", &grouped.to_string()).0, 200);
        let g = js(&t.request("GET", "/api/groups", "").1);
        assert_eq!(g["groups"][0]["master"], 1.0);
        assert_eq!(g["groups"][1]["control"], "group.2.master");
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"group.2.master","value":0.5}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/groups", "").1)["groups"][1]["master"], 0.5);
        // The page's stale copy doesn't move it back.
        assert_eq!(t.request("POST", "/api/projection", &grouped.to_string()).0, 200);
        assert_eq!(js(&t.request("GET", "/api/groups", "").1)["groups"][1]["master"], 0.5);
        let bad = json!({ "zones": [{ "id": 1 }], "groups": [{ "name": "X", "zones": [4] }] });
        assert_eq!(t.request("POST", "/api/projection", &bad.to_string()).0, 400);
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
    fn cue_notes_tags_and_colour_go_through_the_slot_route() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        assert_eq!(js(&t.request("GET", "/api/cues/tags", "").1), json!([]));
        let body = json!({ "id": "tunnels-001", "notes": "seulement avec brouillard", "tags": ["Intro", "intro", "calme"], "color": [255, 0, 64] });
        assert_eq!(t.request("POST", "/api/cues/slot", &body.to_string()).0, 200);
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-002","tags":["INTRO"]}"#).0, 200);
        let slot = &js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"];
        assert_eq!((slot["notes"].as_str(), slot["tags"].clone(), slot["color"].clone()), (Some("seulement avec brouillard"), json!(["Intro", "calme"]), json!([255, 0, 64])));
        assert_eq!(js(&t.request("GET", "/api/cues/tags", "").1), json!([{ "tag": "calme", "count": 1 }, { "tag": "Intro", "count": 2 }]));
        // Undoable like every « Propriétés du cue » edit (T-285).
        assert_eq!(t.request("POST", "/api/undo", "").0, 200);
        assert_eq!(js(&t.request("GET", "/api/cues/tags", "").1), json!([{ "tag": "calme", "count": 1 }, { "tag": "Intro", "count": 1 }]));
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
        let (code, text) = set("tunnels-001", &json!({ "lanes": [{ "target": "mirror" }] }));
        assert_eq!(code, 400, "{text}");
        assert!(text.contains("mirror"));
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

    /// T-273: animated grid thumbnails are drawn aside (pending first, then
    /// served from the cache), small, and never touch what plays.
    #[test]
    fn cue_loops_are_drawn_aside_and_leave_the_show_alone() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        assert_eq!(t.request("POST", "/api/cue", r#"{"id":"tunnels-001","down":true}"#).0, 200);
        let now = |t: &TestServer| {
            let s = t.shared.lock().unwrap();
            (serde_json::to_string(&s.settings).unwrap(), s.deck.active.iter().map(|a| a.cue.clone()).collect::<Vec<_>>())
        };
        let was = now(&t);
        assert_eq!(t.request("GET", "/api/presets/loops", "").0, 400);
        let mut v = serde_json::Value::Null;
        for _ in 0..250 {
            let (code, body) = t.request("GET", "/api/presets/loops?ids=tunnels-002,vagues-028,nope,", "");
            assert_eq!(code, 200, "{body}");
            v = js(&body);
            if v["pending"].as_array().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // An unknown cue is neither drawn nor pending.
        let loops = v["loops"].as_object().unwrap();
        let mut ids: Vec<_> = loops.keys().cloned().collect();
        ids.sort();
        assert_eq!(ids, ["tunnels-002", "vagues-028"]);
        for l in loops.values() {
            let frames = l["frames"].as_array().unwrap();
            assert_eq!(frames.len(), crate::presets::LOOP_FRAMES);
            assert!(frames.iter().all(|f| f.as_str().unwrap().len() / 10 <= 300));
        }
        assert_eq!(was, now(&t), "a thumbnail changed the show");
    }

    /// T-164: the cue editor opens a cue's program, previews unsaved edits
    /// without storing or playing anything, and stores a program into a
    /// new user cue. Never arms.
    #[test]
    fn the_cue_editor_reads_previews_and_stores_programs() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        // A built-in program cue: the look's own; a plain cue: none.
        let (code, body) = t.request("GET", "/api/cues/program?id=vagues-028", "");
        let v = js(&body);
        assert_eq!((code, v["own"].clone(), v["program"]["play"].clone()), (200, json!(false), json!("ping_pong")));
        assert_eq!(v["targets"].as_array().unwrap().len(), crate::program::TARGETS.len());
        let v = js(&t.request("GET", "/api/cues/program?id=tunnels-001", "").1);
        assert_eq!((v["program"].clone(), v["content_beats"].clone(), v["content"]["kind"].clone()), (serde_json::Value::Null, json!(16.0), json!("generator")));
        assert_eq!(t.request("GET", "/api/cues/program?id=nope", "").0, 404);

        // Preview: a size curve changes the pictures; nothing is stored.
        // (A long answer: `http` undoes the chunking.)
        let preview = |program: serde_json::Value| t.request("POST", "/api/cues/program/preview", &json!({ "id": "tunnels-001", "program": program }).to_string());
        let size = |a: f32, b: f32| json!({ "length_beats": 4, "lanes": [{ "target": "size", "keys": [{ "beat": 0, "value": a }, { "beat": 4, "value": b }] }] });
        let (code, small) = preview(size(0.1, 0.1));
        assert_eq!(code, 200);
        let small = js(&small);
        assert_eq!((small["step_beats"].clone(), small["frames"].as_array().unwrap().len()), (json!(0.125), 32));
        let big = js(&preview(size(1.0, 1.0)).1);
        assert_ne!(small["frames"], big["frames"]);
        let (code, text) = preview(json!({ "lanes": [{ "target": "mirror" }] }));
        assert!(code == 400 && text.contains("n'est pas un paramètre du cue"), "{code} {text}");
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"]["program"], serde_json::Value::Null);

        // Saved on the cue, then read back as its own.
        assert_eq!(t.request("POST", "/api/cues/program", &json!({ "id": "tunnels-001", "program": size(0.2, 1.0) }).to_string()).0, 200);
        let v = js(&t.request("GET", "/api/cues/program?id=tunnels-001", "").1);
        assert_eq!((v["own"].clone(), v["program"]["lanes"][0]["keys"][1]["value"].clone()), (json!(true), json!(1.0)));

        // « Enregistrer dans une case »: a user cue with the program in its look.
        let (code, body) = t.request("POST", "/api/cues/program/store", &json!({ "id": "tunnels-001", "program": size(0.5, 0.75) }).to_string());
        assert_eq!(code, 200, "{body}");
        let g = js(&body);
        let id = g["cue"].as_str().unwrap().to_string();
        assert_eq!(g["page"], g["cell"][0]);
        let v = js(&t.request("GET", &format!("/api/cues/program?id={id}"), "").1);
        assert_eq!((v["own"].clone(), v["user"].clone(), v["program"]["lanes"][0]["keys"][0]["value"].clone()), (json!(false), json!(true), json!(0.5)));
        assert_eq!(t.request("POST", "/api/cues/program/store", r#"{"id":"tunnels-001","program":null}"#).0, 400);
        assert_eq!(t.request("POST", "/api/cues/program/store", r#"{"id":"nope","program":{}}"#).0, 404);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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

    fn js(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    /// `GET /api/events` opened, the status line and headers read.
    fn open_events(addr: SocketAddr) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        write!(stream, "GET /api/events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n").unwrap();
        let mut head = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !head.contains("\r\n\r\n") && Instant::now() < deadline {
            let mut buf = [0u8; 4096];
            if let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                head.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
        (stream, head)
    }

    /// The `state` events read from an open stream for `wait`: (version, state).
    fn read_events(stream: &mut TcpStream, carry: &mut String, wait: Duration) -> Vec<(u64, serde_json::Value)> {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            let mut buf = [0u8; 65536];
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => carry.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => {}
            }
        }
        let mut events = Vec::new();
        while let Some(end) = carry.find("\n\n") {
            let block: String = carry.drain(..end + 2).collect();
            let field = |name: &str| block.lines().find_map(|l| l.strip_prefix(name)).map(str::to_string);
            if field("event: ").as_deref() == Some("state") {
                events.push((field("id: ").unwrap().parse().unwrap(), serde_json::from_str(&field("data: ").unwrap()).unwrap()));
            }
        }
        events
    }

    /// T-411: the first event is the whole state; nothing more while
    /// nothing changes; a change made through the API comes at once.
    #[test]
    fn the_state_stream_sends_the_state_then_only_changes() {
        let t = TestServer::start(false);
        let (mut stream, head) = open_events(t.addr);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: text/event-stream"), "{head}");
        let mut carry = head.split_once("\r\n\r\n").map(|(_, rest)| rest.to_string()).unwrap_or_default();
        // Without an engine here, the watchdog soon reports it stalled: a
        // real change, after which nothing moves.
        let first = read_events(&mut stream, &mut carry, Duration::from_millis(600));
        assert!(!first.is_empty() && first.len() <= 2, "{first:?}");
        let (_, state) = &first[0];
        let frame = js(&t.request("GET", "/api/frame", "").1);
        // The points and their frame number (T-475) belong to one engine
        // frame: the pushed state has neither.
        for key in frame.as_object().unwrap().keys().filter(|k| *k != "points" && *k != "frame_seq") {
            assert!(state.get(key).is_some(), "the pushed state has {key}");
        }
        assert!(state.get("points").is_none(), "no points in the state");
        assert!(state.get("frame_seq").is_none(), "no frame number in the state");
        assert_eq!(state["armed"], false);
        let v1 = first.last().unwrap().0;
        assert!(read_events(&mut stream, &mut carry, Duration::from_millis(400)).is_empty(), "no change, no event (the beat runs on)");
        let started = Instant::now();
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"master.size","value":0.5}"#).0, 200);
        let mut got = Vec::new();
        while got.is_empty() && started.elapsed() < Duration::from_secs(2) {
            got = read_events(&mut stream, &mut carry, Duration::from_millis(20));
        }
        assert!(started.elapsed() < Duration::from_millis(500), "the change came after {:?}", started.elapsed());
        let (v2, state) = got.last().unwrap();
        assert!(*v2 > v1);
        assert_eq!(state["live"]["size"], 0.5);
        // The binary frames and the JSON polling routes say which version is current.
        let (head, _) = get_bytes(t.addr, "/api/frame");
        assert!(head.contains(&format!("X-State-Version: {v2}")), "{head}");
    }

    /// T-411: eight streams at most; the ninth is refused and the stop,
    /// the heartbeat and the JSON routes still answer at once.
    #[test]
    fn a_ninth_stream_is_refused_and_streams_never_hold_up_the_worker() {
        let t = TestServer::start(false);
        enforce_presence(&t);
        let streams: Vec<_> = (0..push::MAX_STREAMS).map(|_| open_events(t.addr)).collect();
        assert!(streams.iter().all(|(_, head)| head.starts_with("HTTP/1.1 200")));
        let (status, _) = t.request("GET", "/api/events", "");
        assert_eq!(status, 503);
        // An open stream is not the operator (T-252): only beats are.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 409, "streams but no page beating");
        let started = Instant::now();
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"client_id":"p"}"#).0, 200);
        assert_eq!(t.request("GET", "/api/state", "").0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/estop?source=ui", "").0, 200);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        assert_eq!(arm_status(&t)["armed"], false);
        drop(streams);
    }

    /// T-411: `/api/frame.bin` and `/api/frame` describe the same image,
    /// and the arm answer names the last frame published before it.
    #[test]
    fn the_binary_frame_matches_the_json_frame() {
        let t = TestServer::start(false);
        use crate::patterns::Point;
        let points: Vec<Point> = (0..300).map(|i| Point::lit((i as f32 / 150.0) - 1.0, ((i * 7) % 100) as f32 / 100.0, 1.0, 0.25, 0.0)).collect();
        let hub = Arc::clone(&t.shared.lock().unwrap().push);
        // No engine here: published by hand, as the engine does.
        hub.wait_frame(None, Duration::ZERO);
        {
            let mut s = t.shared.lock().unwrap();
            s.frame = points.clone();
            s.push.publish_frame(&points, push::flags::ENGINE_OK);
        }
        let (head, bytes) = get_bytes(t.addr, "/api/frame.bin?after=0");
        assert!(head.contains("application/octet-stream"), "{head}");
        let (seq, flags, bin) = push::decode_frame(&bytes).unwrap();
        assert_eq!((seq, flags), (1, push::flags::ENGINE_OK));
        let frame = js(&t.request("GET", "/api/frame", "").1);
        assert_eq!(frame["frame_seq"], 1, "the JSON frame names its sequence (T-475)");
        let json_points = frame["points"].as_array().unwrap();
        assert_eq!(bin.len(), json_points.len());
        for (b, j) in bin.iter().zip(json_points) {
            let j: Vec<f32> = j.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
            for (a, c) in [b.x, b.y, b.r, b.g, b.b].iter().zip(&j) {
                assert!((a - c).abs() < 0.003, "{b:?} vs {j:?}");
            }
        }
        // Nothing newer than 1 within the wait: the same frame, about 100 ms later.
        let started = Instant::now();
        let (_, again) = get_bytes(t.addr, "/api/frame.bin?after=1");
        assert_eq!(push::decode_frame(&again).unwrap().0, 1);
        assert!(started.elapsed() >= push::FRAME_WAIT);
        let raw = {
            let mut stream = TcpStream::connect(t.addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(stream, "POST /api/arm HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{{\"on\":true}}").unwrap();
            let mut raw = String::new();
            stream.read_to_string(&mut raw).unwrap();
            raw
        };
        assert!(raw.contains("X-Frame-Seq: 1"), "{raw}");
    }

    /// T-411: a client asking for frames and never reading them holds its
    /// own threads only: another page's frames keep coming at once.
    #[test]
    fn a_frame_reader_that_never_reads_does_not_hold_up_the_others() {
        use crate::patterns::Point;
        let t = TestServer::start(false);
        let hub = Arc::clone(&t.shared.lock().unwrap().push);
        hub.wait_frame(None, Duration::ZERO);
        let stop = Arc::new(AtomicBool::new(false));
        let publisher = std::thread::spawn({
            let (hub, stop) = (Arc::clone(&hub), Arc::clone(&stop));
            move || {
                let points: Vec<Point> = (0..3000).map(|i| Point::lit((i as f32 / 1500.0) - 1.0, 0.5, 1.0, 1.0, 1.0)).collect();
                while !stop.load(Ordering::SeqCst) {
                    hub.publish_frame(&points, 0);
                    std::thread::sleep(Duration::from_millis(16));
                }
            }
        });
        let stuck = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        stuck.set_recv_buffer_size(4096).unwrap();
        stuck.connect(&t.addr.into()).unwrap();
        let mut stuck: TcpStream = stuck.into();
        for _ in 0..8 {
            write!(stuck, "GET /api/frame.bin HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
        let mut seq = None;
        for _ in 0..10 {
            let started = Instant::now();
            let path = seq.map_or("/api/frame.bin".to_string(), |s| format!("/api/frame.bin?after={s}"));
            let (_, bytes) = get_bytes(t.addr, &path);
            let (s, _, points) = push::decode_frame(&bytes).unwrap();
            assert_eq!(points.len(), 3000);
            assert_ne!(Some(s), seq, "a newer frame");
            assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
            seq = Some(s);
        }
        stop.store(true, Ordering::SeqCst);
        publisher.join().unwrap();
        drop(stuck);
    }

    /// T-411: a client that opens the stream and never reads blocks only
    /// its own stream thread: states keep being built and published (no
    /// lock held by the blocked write), another stream keeps receiving
    /// them, and the heartbeat, arming and the stop answer at once.
    #[test]
    fn a_stream_that_is_never_read_blocks_nothing_else() {
        let t = TestServer::start(false);
        let hub = Arc::clone(&t.shared.lock().unwrap().push);
        let stuck = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        stuck.set_recv_buffer_size(4096).unwrap();
        stuck.connect(&t.addr.into()).unwrap();
        let mut stuck: TcpStream = stuck.into();
        write!(stuck, "GET /api/events HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while hub.streams() == 0 {
            assert!(Instant::now() < deadline, "the stream never opened");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Big states until the stuck stream's socket is full and its write blocks.
        let filler = "x".repeat(256 * 1024);
        for n in 0..40u64 {
            let started = Instant::now();
            hub.publish_state(json!(n), |_| format!("{{\"n\":{n},\"f\":\"{filler}\"}}"));
            assert!(started.elapsed() < Duration::from_millis(100), "publishing waited on the stuck stream");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hub.streams(), 1, "the stuck stream keeps its slot until its page goes");
        // The studio lock is free: the routes and the state builder carry on.
        let started = Instant::now();
        assert!(t.shared.try_lock().is_ok(), "no lock held by the blocked write");
        assert_eq!(t.request("POST", "/api/heartbeat", r#"{"client_id":"p"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"source":"ui"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/estop?source=ui", "").0, 200);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        // Another page's stream gets the newest state at once.
        let (mut other, head) = open_events(t.addr);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut carry = head.split_once("\r\n\r\n").map(|(_, rest)| rest.to_string()).unwrap_or_default();
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while got.is_empty() && Instant::now() < deadline {
            got = read_events(&mut other, &mut carry, Duration::from_millis(50));
        }
        assert!(!got.is_empty(), "the other stream got nothing");
        // The page goes: its thread fails its write and frees the slot.
        drop(stuck);
        let deadline = Instant::now() + Duration::from_secs(10);
        while hub.streams() > 1 {
            assert!(Instant::now() < deadline, "the closed stream kept its slot");
            hub.publish_state(json!("bye"), |_| "{}".into());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_fingerprint_ignores_the_running_beat_but_not_a_change() {
        let fingerprint = |mut state: serde_json::Value, anchor: f64| {
            let original = state.clone();
            let moved = split_moving(&mut state, anchor);
            let mut back = state.clone();
            put_back(&mut back, moved);
            if let Some(t) = back.get_mut("tempo").and_then(|t| t.as_object_mut()) {
                t.remove("anchor");
            }
            assert_eq!(back, original, "everything taken out is put back");
            state
        };
        let state = |beat: f64, bpm: f64, dose: f64| json!({ "tempo": { "bpm": bpm, "beat": beat, "bar": 0, "beat_in_bar": 1, "phase": beat.fract() }, "dwell": { "max_dose": dose, "extent": 0.4, "extent_factor": 1.0 } });
        assert_eq!(fingerprint(state(1.25, 120.0, 0.1), -3.0), fingerprint(state(7.5, 120.0, 0.3), -3.0));
        assert_ne!(fingerprint(state(1.25, 120.0, 0.1), -3.0), fingerprint(state(1.25, 121.0, 0.1), -3.0));
        assert_ne!(fingerprint(state(1.25, 120.0, 0.1), -3.0), fingerprint(state(1.25, 120.0, 0.1), -3.5), "a nudge of the clock");
        let small = json!({ "dwell": { "max_dose": 0.1, "extent": 0.01, "extent_factor": 0.5 } });
        let smaller = json!({ "dwell": { "max_dose": 0.1, "extent": 0.02, "extent_factor": 0.5 } });
        assert_ne!(fingerprint(small, 0.0), fingerprint(smaller, 0.0), "the page shows a too-small figure's size");
        let trig = |fired: u64, ago: Option<f64>| json!({ "audio_triggers": { "enabled": true, "rules": [{ "fired": fired, "ago_s": ago, "limited": false }] } });
        assert_eq!(fingerprint(trig(1, Some(0.1)), 0.0), fingerprint(trig(1, Some(4.0)), 0.0), "time since a trigger fired");
        assert_ne!(fingerprint(trig(1, Some(4.0)), 0.0), fingerprint(trig(2, Some(0.0)), 0.0), "a new firing");
    }

    #[test]
    fn command_line_route_sets_refuses_and_works_in_show_mode() {
        let t = TestServer::start(false);
        let cmd = |line: &str| t.request("POST", "/api/command", &json!({ "line": line }).to_string());
        let (code, body) = cmd("master.size 80");
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body), json!({ "ok": true, "echo": "master.size = 80 %" }));
        assert!((js(&t.request("GET", "/api/live", "").1)["size"].as_f64().unwrap() - 0.8).abs() < 1e-6);
        assert_eq!(cmd("master.size 80,5").0, 200);
        assert_eq!(js(&cmd("master.size 900").1)["echo"], "master.size = 200 %");
        // Errors: 400 with the word, its position and the French message.
        let (code, body) = cmd("mastr.size 80");
        assert_eq!(code, 400);
        assert_eq!(js(&body), json!({ "token": "mastr.size", "pos": 0, "message_fr": "Mot inconnu : « mastr.size »", "refused": false }));
        let (code, body) = cmd("armer");
        assert_eq!(code, 400);
        assert_eq!(js(&body)["refused"], true);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
        // Playback: allowed in show mode, and never in the edit history.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(cmd("tempo 128").0, 200);
        assert_eq!(cmd("tout arrêter").0, 200);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["history"]["undo"], 0);
        // The palette's list: values in words, nothing that arms.
        let actions = js(&t.request("GET", "/api/command/actions", "").1);
        let list = actions["actions"].as_array().unwrap();
        let size = list.iter().find(|a| a["id"] == "master.size").unwrap();
        assert_eq!(size["value_fr"], "200 %");
        assert_eq!(size["label_fr"], "Taille maître");
        assert!(list.iter().all(|a| a["id"] != "transport.arm"));
        // Noir latches the stop at once.
        assert_eq!(cmd("noir").0, 200);
        assert!(t.shared.lock().unwrap().estop.is_latched());
    }
}
