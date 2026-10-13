//! The browser UI's HTTP API. Every handler just reads or edits `Shared`;
//! the engine thread picks changes up on its next frame.
//!
//! Bound to 127.0.0.1 only: the UI can turn a laser on, so it isn't
//! exposed to the rest of the network.
//!
//! One thread receives requests; `POST /api/estop` is handled right there,
//! without the studio lock, ahead of everything queued. All other requests go, in order,
//! to a single worker thread, so a slow handler never delays a stop.
//! A request reaches them only once its head and its body (up to
//! `MAX_JSON_BODY`) have arrived, read on the connection's own thread
//! (T-481), so a client that sends slowly holds only itself; the stop is
//! handed over as soon as its head is read, its body never waited for. The
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
/// The page's text table (T-461), a classic script loaded before the
/// modules (the page must still arm and stop without it: labels.spec.ts).
const LABELS_JS: &str = include_str!("labels.js");

const JS: &str = "text/javascript; charset=utf-8";
/// The page's stylesheet and its ES modules (T-435): (path, body, content
/// type), the modules in the order they run (`ui/app.js` imports the last
/// one, each one imports the one before it). Everything is compiled in:
/// the studio works offline and reads no file at run time.
pub(crate) const UI_ASSETS: &[(&str, &str, &str)] = &[
    ("/ui/styles.css", include_str!("ui/styles.css"), "text/css; charset=utf-8"),
    ("/ui/core.js", include_str!("ui/core.js"), JS),
    ("/ui/laser.js", include_str!("ui/laser.js"), JS),
    ("/ui/presence.js", include_str!("ui/presence.js"), JS),
    ("/ui/state.js", include_str!("ui/state.js"), JS),
    ("/ui/view3d.js", include_str!("ui/view3d.js"), JS),
    ("/ui/workspaces.js", include_str!("ui/workspaces.js"), JS),
    ("/ui/music.js", include_str!("ui/music.js"), JS),
    ("/ui/cuelist.js", include_str!("ui/cuelist.js"), JS),
    ("/ui/zones.js", include_str!("ui/zones.js"), JS),
    ("/ui/safety.js", include_str!("ui/safety.js"), JS),
    ("/ui/targets.js", include_str!("ui/targets.js"), JS),
    ("/ui/direct.js", include_str!("ui/direct.js"), JS),
    ("/ui/tempo.js", include_str!("ui/tempo.js"), JS),
    ("/ui/cues.js", include_str!("ui/cues.js"), JS),
    ("/ui/presets.js", include_str!("ui/presets.js"), JS),
    ("/ui/macros.js", include_str!("ui/macros.js"), JS),
    ("/ui/shows.js", include_str!("ui/shows.js"), JS),
    ("/ui/timeline-editor.js", include_str!("ui/timeline-editor.js"), JS),
    ("/ui/timeline-markers.js", include_str!("ui/timeline-markers.js"), JS),
    ("/ui/timeline-canvas.js", include_str!("ui/timeline-canvas.js"), JS),
    ("/ui/layers.js", include_str!("ui/layers.js"), JS),
    ("/ui/desk.js", include_str!("ui/desk.js"), JS),
    ("/ui/modulators.js", include_str!("ui/modulators.js"), JS),
    ("/ui/outputs.js", include_str!("ui/outputs.js"), JS),
    ("/ui/venue.js", include_str!("ui/venue.js"), JS),
    ("/ui/midi.js", include_str!("ui/midi.js"), JS),
    ("/ui/apc40.js", include_str!("ui/apc40.js"), JS),
    ("/ui/project.js", include_str!("ui/project.js"), JS),
    ("/ui/figures.js", include_str!("ui/figures.js"), JS),
    ("/ui/media.js", include_str!("ui/media.js"), JS),
    ("/ui/program.js", include_str!("ui/program.js"), JS),
    ("/ui/help.js", include_str!("ui/help.js"), JS),
    ("/ui/command.js", include_str!("ui/command.js"), JS),
    ("/ui/app.js", include_str!("ui/app.js"), JS),
];

/// Files the UI loads besides the page itself: (body, content type).
/// Everything is compiled in, so the studio works offline.
fn static_asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/" => Some((INDEX_HTML, "text/html; charset=utf-8")),
        "/beam3d.js" => Some((BEAM3D_JS, JS)),
        "/labels.js" => Some((LABELS_JS, JS)),
        _ => UI_ASSETS.iter().find(|a| a.0 == path).map(|a| (a.1, a.2)),
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
    ("GET", "/labels.js", RouteClass::Read),
    ("GET", "/ui/*", RouteClass::Read),
    ("POST", "/api/estop", RouteClass::Safety),
    ("GET", "/api/events", RouteClass::Read),
    ("GET", "/api/frame.bin", RouteClass::Read),
    ("GET", "/api/history", RouteClass::Read),
    ("POST", "/api/undo", RouteClass::Edit),
    ("POST", "/api/redo", RouteClass::Edit),
    ("GET", "/api/state", RouteClass::Read),
    ("GET", "/api/diagnostics/report", RouteClass::Read),
    ("GET", "/api/about", RouteClass::Read),
    ("POST", "/api/about/seen", RouteClass::Play),
    // Vues et préférences d'opérateur (T-426): display only, never the
    // output; allowed in show mode like a camera move.
    ("GET", "/api/operators", RouteClass::Read),
    ("POST", "/api/operators/current", RouteClass::Play),
    ("POST", "/api/operators/delete", RouteClass::Play),
    ("GET", "/api/operators/*", RouteClass::Read),
    ("POST", "/api/operators/*", RouteClass::Play),
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
    ("GET", "/api/audio/diagnostics", RouteClass::Read),
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
    ("GET", "/api/perf", RouteClass::Read),
    ("POST", "/api/presence", RouteClass::Safety),
    ("GET", "/api/lock", RouteClass::Read),
    ("POST", "/api/lock", RouteClass::Safety),
    ("GET", "/api/onboarding", RouteClass::Read),
    ("POST", "/api/onboarding", RouteClass::Play),
    ("GET", "/api/help", RouteClass::Read),
    ("POST", "/api/keep_awake", RouteClass::Safety),
    ("POST", "/api/test/wifi", RouteClass::Test),
    ("POST", "/api/test/power", RouteClass::Test),
    ("GET", "/api/test/menu", RouteClass::Test),
    ("POST", "/api/test/menu", RouteClass::Test),
    ("POST", "/api/test/stall", RouteClass::Test),
    ("POST", "/api/test/panic", RouteClass::Test),
    ("POST", "/api/test/tempo_estimate", RouteClass::Test),
    ("POST", "/api/test/timecode", RouteClass::Test),
    ("POST", "/api/test/psn", RouteClass::Test),
    ("POST", "/api/test/native_audio", RouteClass::Test),
    ("POST", "/api/test/clock", RouteClass::Test),
    ("GET", "/api/arm", RouteClass::Read),
    ("POST", "/api/arm", RouteClass::Arm),
    ("POST", "/api/estop/reset", RouteClass::Arm),
    ("GET", "/api/safety", RouteClass::Read),
    ("POST", "/api/safety", RouteClass::Safety),
    ("GET", "/api/targets", RouteClass::Read),
    ("POST", "/api/targets", RouteClass::Safety),
    ("GET", "/api/psn", RouteClass::Read),
    ("POST", "/api/psn", RouteClass::Safety),
    ("GET", "/api/psn/trackers", RouteClass::Read),
    ("GET", "/api/safety/lock", RouteClass::Read),
    ("POST", "/api/safety/lock", RouteClass::Safety),
    ("POST", "/api/safety/unlock", RouteClass::Safety),
    ("GET", "/api/safety/profiles", RouteClass::Read),
    ("POST", "/api/safety/profiles", RouteClass::Safety),
    ("POST", "/api/safety/profiles/load", RouteClass::Safety),
    ("GET", "/api/sites", RouteClass::Read),
    ("POST", "/api/sites", RouteClass::Safety),
    ("POST", "/api/sites/use-project", RouteClass::Safety),
    ("GET", "/api/pack/export", RouteClass::Read),
    ("POST", "/api/pack/import", RouteClass::Edit),
    ("POST", "/api/safety/sky", RouteClass::Safety),
    ("GET", "/api/safety/outdoor/export", RouteClass::Read),
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
    ("POST", "/api/outputs/fixed_rate", RouteClass::Safety),
    ("GET", "/api/testpattern", RouteClass::Read),
    ("POST", "/api/testpattern", RouteClass::Safety),
    ("POST", "/api/safety/trace", RouteClass::Safety),
    ("GET", "/api/checklist", RouteClass::Read),
    ("GET", "/api/safety/log", RouteClass::Read),
    ("GET", "/api/safety/log.csv", RouteClass::Read),
    ("GET", "/api/safety/sheet", RouteClass::Read),
    ("POST", "/api/safety/sheet", RouteClass::Read),
    ("POST", "/api/calibration", RouteClass::Safety),
    ("GET", "/api/venue", RouteClass::Read),
    ("POST", "/api/venue", RouteClass::Safety),
    ("POST", "/api/venue/preset", RouteClass::Safety),
    ("POST", "/api/venue/camera", RouteClass::Play),
    ("POST", "/api/venue/sim", RouteClass::Play),
    ("GET", "/api/frames", RouteClass::Read),
    ("GET", "/api/projection", RouteClass::Read),
    ("GET", "/api/groups", RouteClass::Read),
    ("GET", "/api/zone-chase", RouteClass::Read),
    ("POST", "/api/projection", RouteClass::Safety),
    ("POST", "/api/projection/align", RouteClass::Safety),
    // Zone alignment from a phone (T-460): main screen only. Opening and
    // applying are safety edits (show mode, safety lock); rejecting and
    // closing only go back to the saved zone (never locked).
    ("POST", "/api/zones/align/open", RouteClass::Safety),
    ("POST", "/api/zones/align/apply", RouteClass::Safety),
    ("POST", "/api/zones/align/reject", RouteClass::Safety),
    ("POST", "/api/zones/align/close", RouteClass::Safety),
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
    ("POST", "/api/cues/parts", RouteClass::Edit),
    ("GET", "/api/cues/program", RouteClass::Read),
    ("POST", "/api/cues/program/preview", RouteClass::Edit),
    ("POST", "/api/cues/program/store", RouteClass::Edit),
    ("POST", "/api/abstracts/preview", RouteClass::Edit),
    ("POST", "/api/abstracts/store", RouteClass::Edit),
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
    ("GET", "/api/pools", RouteClass::Read),
    ("POST", "/api/pools", RouteClass::Edit),
    ("POST", "/api/pools/delete", RouteClass::Edit),
    ("POST", "/api/pools/capture", RouteClass::Edit),
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
    ("GET", "/api/macros", RouteClass::Read),
    ("POST", "/api/macros", RouteClass::Edit),
    ("POST", "/api/macros/run", RouteClass::Play),
    ("POST", "/api/macros/stop", RouteClass::Play),
    ("POST", "/api/macros/go", RouteClass::Play),
    // Scripts (T-458): the list is an edit; running and stopping are
    // playback; checking a source reads only; approving a project's
    // scripts is an edit (never in show mode).
    ("GET", "/api/scripts", RouteClass::Read),
    ("POST", "/api/scripts", RouteClass::Edit),
    ("POST", "/api/scripts/run", RouteClass::Play),
    ("POST", "/api/scripts/stop", RouteClass::Play),
    ("POST", "/api/scripts/check", RouteClass::Read),
    ("POST", "/api/scripts/trust", RouteClass::Edit),
    ("GET", "/api/showlist", RouteClass::Read),
    ("POST", "/api/showlist", RouteClass::Edit),
    ("POST", "/api/showlist/play", RouteClass::Play),
    ("POST", "/api/showlist/stop", RouteClass::Play),
    ("POST", "/api/showlist/next", RouteClass::Play),
    ("POST", "/api/showlist/prev", RouteClass::Play),
    ("POST", "/api/showlist/loop", RouteClass::Play),
    // L'agenda (T-455): an edit (history, locked in show mode). It never
    // arms and has no control: nothing external reaches it.
    ("GET", "/api/agenda", RouteClass::Read),
    ("POST", "/api/agenda", RouteClass::Edit),
    ("GET", "/api/fx", RouteClass::Read),
    ("POST", "/api/fx", RouteClass::Edit),
    ("GET", "/api/audio/routes", RouteClass::Read),
    ("POST", "/api/audio/routes", RouteClass::Edit),
    ("GET", "/api/audio/presets", RouteClass::Read),
    ("POST", "/api/audio/preset", RouteClass::Edit),
    ("GET", "/api/audio/triggers", RouteClass::Read),
    ("POST", "/api/audio/triggers", RouteClass::Edit),
    // Pilote automatique (T-148): its pages and public content are an
    // edit; the switch, rhythm and order are the `vlj.*` controls.
    ("GET", "/api/autopilot", RouteClass::Read),
    ("POST", "/api/autopilot", RouteClass::Edit),
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
    ("GET", "/api/objects", RouteClass::Read),
    ("POST", "/api/objects", RouteClass::Edit),
    ("POST", "/api/objects/import", RouteClass::Edit),
    ("POST", "/api/objects/delete", RouteClass::Edit),
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
    // Enregistrer le jeu en direct (T-169): recording only observes (Play,
    // allowed in show mode); laying the take out is an edit's preview.
    ("GET", "/api/timeline/record", RouteClass::Read),
    ("POST", "/api/timeline/record/start", RouteClass::Play),
    ("POST", "/api/timeline/record/stop", RouteClass::Play),
    ("POST", "/api/timeline/record/discard", RouteClass::Play),
    ("POST", "/api/timeline/record/take", RouteClass::Edit),
    ("GET", "/api/netin", RouteClass::Read),
    ("POST", "/api/netin", RouteClass::Safety),
    ("GET", "/api/osc/addresses", RouteClass::Read),
    ("GET", "/api/osc/monitor", RouteClass::Read),
    ("POST", "/api/osc/monitor/clear", RouteClass::Edit),
    ("GET", "/api/osc/config", RouteClass::Read),
    ("POST", "/api/osc/config", RouteClass::Safety),
    ("GET", "/api/backup", RouteClass::Read),
    ("POST", "/api/backup", RouteClass::Edit),
    ("POST", "/api/backup/now", RouteClass::Play),
    ("GET", "/api/help/*", RouteClass::Read),
    ("POST", "/api/checklist/*", RouteClass::Safety),
    ("POST", "/api/cuelist/*", RouteClass::Play),
    ("POST", "/api/timeline/*", RouteClass::Edit),
    ("GET", "/api/midi/clock", RouteClass::Read),
    ("POST", "/api/midi/clock", RouteClass::Safety),
    ("GET", "/api/link", RouteClass::Read),
    ("POST", "/api/link", RouteClass::Safety),
    ("*", "/api/midi*", RouteClass::Edit),
    ("*", "/api/external*", RouteClass::Safety),
    ("*", "/api/dmx/*", RouteClass::Safety),
    ("*", "/api/timecode", RouteClass::Safety),
    ("*", "/api/timecode/arm", RouteClass::Safety),
    ("*", "/api/timecode/out", RouteClass::Safety),
    // DMX output (T-453): its settings are Safety (outputs, never the
    // remote); the haze button and fan are live play.
    ("GET", "/api/dmx-out", RouteClass::Read),
    ("POST", "/api/dmx-out", RouteClass::Safety),
    ("POST", "/api/dmx-out/haze", RouteClass::Play),
    // CITP (T-454): names and pictures for a console; switching it on
    // opens a socket, so Safety (never from the tablet remote).
    ("GET", "/api/citp", RouteClass::Read),
    ("POST", "/api/citp", RouteClass::Safety),
    ("*", "/api/project", RouteClass::Edit),
    ("*", "/api/project/*", RouteClass::Edit),
    ("GET", "/api/remote", RouteClass::Read),
    ("POST", "/api/remote", RouteClass::Safety),
    ("POST", "/api/remote/kick", RouteClass::Safety),
    // The backup machine (T-456): enabling sync, pairing, taking over and
    // receiving the project are machine settings (local only).
    ("GET", "/api/standby", RouteClass::Read),
    ("POST", "/api/standby", RouteClass::Safety),
    ("POST", "/api/standby/pair", RouteClass::Safety),
    ("POST", "/api/standby/takeover", RouteClass::Safety),
    ("POST", "/api/standby/receive", RouteClass::Safety),
];

/// Whether `method path` is a route of the main UI (`ROUTES`).
pub fn is_known_route(method: &str, path: &str) -> bool {
    route_class(method, path).is_some()
}

/// The class of `method path` in `ROUTES` (the first matching line).
pub fn route_class(method: &str, path: &str) -> Option<RouteClass> {
    ROUTES.iter().find(|&&(m, p, _)| (m == "*" || m == method) && p.strip_suffix('*').map_or(path == p, |prefix| path.starts_with(prefix))).map(|&(_, _, c)| c)
}

/// How long a client may stop sending (a body cut short) or stop reading
/// (an answer it never reads) before its connection is dropped (T-449).
/// Without it, one such request held the single worker, and so every
/// route but the stop, for good. Longer than the 4 s a browser or Node
/// keeps an idle connection, so an idle keep-alive is closed by the
/// client first.
pub(crate) const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The most connections served at once (T-481). Past it, a new one pushes
/// out the connection that has waited longest for its client (a slow or
/// idle one), so a flood of slow clients holds neither all the threads
/// and file descriptors nor the door: a stop on a new connection still
/// gets in. A browser opens at most 6 per page; three descriptors each.
pub(crate) const MAX_CONNECTIONS: usize = 48;

/// The body bytes held at once by all the requests read in advance
/// (T-481): four of the biggest JSON body. Past it, 503.
const PREFETCH_BUDGET: usize = 4 * MAX_JSON_BODY;

/// The answer bytes waiting at once on all the connections (T-489), past
/// the one each connection is writing: a page that pipelines requests and
/// never reads the answers is dropped past it. Four of the biggest JSON
/// body; `/api/state` is far smaller.
const OUTPUT_BUDGET: usize = 4 * MAX_JSON_BODY;

/// A server on `addr` whose connections time out reads and writes after
/// `CLIENT_IO_TIMEOUT` (set on each accepted socket by our tiny_http
/// patch, `vendor/tiny_http`: macOS sockets don't inherit the listener's).
/// Bodies up to `prefetch` bytes are read on the connection's own thread
/// before the request is handed over, at most `budget` bytes at once, and
/// at most `max_connections` are served at once (T-481); the stop is
/// handed over on its head alone. Answers are written by their
/// connection's own thread, never by the one that answers (T-489): a
/// client that doesn't read them holds only its connection; at most
/// `output` bytes of them wait at once.
pub(crate) fn bind_with(addr: impl std::net::ToSocketAddrs, prefetch: usize, budget: usize, max_connections: usize, output: usize) -> anyhow::Result<Server> {
    let listener = std::net::TcpListener::bind(addr)?;
    let limits = tiny_http::ClientLimits {
        timeout: Some(CLIENT_IO_TIMEOUT),
        prefetch,
        prefetch_budget: budget,
        max_connections,
        head_only: Some(is_estop),
        output_budget: output,
    };
    Server::from_listener_with_limits(listener, None, limits).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The UI's server on `addr` (see `bind_with`).
pub(crate) fn bind(addr: impl std::net::ToSocketAddrs) -> anyhow::Result<Server> {
    bind_with(addr, MAX_JSON_BODY, PREFETCH_BUDGET, MAX_CONNECTIONS, OUTPUT_BUDGET)
}

pub fn run(addr: &str, shared: Arc<Mutex<Shared>>, estop: Arc<EStop>, calibration_path: PathBuf, running: Arc<AtomicBool>) -> anyhow::Result<()> {
    let server = bind(addr).map_err(|e| anyhow::anyhow!("failed to start web server on {addr}: {e}"))?;
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
                // T-481: a body that didn't fit in advance (bigger than any
                // JSON body) would be read here, at the client's pace;
                // only the long jobs above, on their own threads, take one.
                if !request.body_buffered() {
                    let _ = request.respond(text(413, &format!("requête trop grosse ({} Mo au plus)", MAX_JSON_BODY >> 20)));
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
/// A page that stops reading blocks this thread in a write until the
/// client timeout (`CLIENT_IO_TIMEOUT`) drops it; the write holds no
/// lock: the event is a copy taken from the hub beforehand. A stream
/// writes directly, on this thread, once the answers before it on its
/// connection are out (T-489).
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
    // Handed to the connection's own thread (T-489): a client that doesn't
    // read holds only that thread, and its unread frames count in the
    // output budget, past which its connection is dropped.
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
            // OBJ import (T-462): up to 8 MB of text parsed and chained.
            | (Method::Post, "/api/objects/import")
            // ILDA import parses up to 64 MB; an export renders and encodes
            // up to `MAX_EXPORT_FRAMES` images (T-011).
            | (Method::Post, "/api/media/ilda")
            | (Method::Get, "/api/media/ilda/export")
            // A whole SD bank: up to 229 cues rendered and written (T-302).
            | (Method::Post, "/api/sdbank/export")
            // A package: media read and zipped, or unzipped, checked,
            // decoded and written (T-289).
            | (Method::Get, "/api/pack/export")
            | (Method::Post, "/api/pack/import")
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
        // Macros end now (T-425); if the lock is busy, the engine's next
        // frame ends them before any step, as it sees the latch.
        s.macros.player.stop_all();
        // Script runs are dropped and their triggers held (T-458).
        crate::script::stop_all(s);
        // The show list stops chaining (T-444); the engine halts its show.
        s.showlist.halt();
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
    // A backup machine in standby (T-456): read only, but for the stops,
    // arming (refused with its reason) and the machine's own settings.
    if method == Method::Post && crate::standby::refused_in_standby(method.as_str(), &path) && shared.lock().unwrap().standby.is_standby() {
        return text(423, crate::standby::STANDBY_BANNER_FR);
    }
    // Safety lock (T-261): a Safety-class change other than a stop is
    // refused while locked; the `HANDLER` routes refuse only loosening.
    // While unlocked, it restarts the relock countdown.
    if method != Method::Get && !crate::safetylock::ALWAYS.contains(&path.as_str()) && route_class(method.as_str(), &path) == Some(RouteClass::Safety) {
        let now = Instant::now();
        let mut s = shared.lock().unwrap();
        if !crate::safetylock::check(&mut s, now) {
            s.safety_lock.touch(now);
        } else if !crate::safetylock::passes_gate(&path) {
            return safety_locked();
        }
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
            // No body: one step. A body that can't be read or parsed is
            // refused (T-449: never an undo the request didn't ask for).
            let to = match read_body(request) {
                Ok(raw) if raw.trim().is_empty() => None,
                Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(v) => v.get("to").and_then(serde_json::Value::as_u64),
                    Err(e) => return text(400, &format!("invalid JSON: {e}")),
                },
                Err(e) => return e,
            };
            match crate::history::step(shared, path == "/api/redo", to) {
                Ok(v) => json_response(v),
                Err((code, msg)) => text(code, &msg),
            }
        }
        (Method::Get, p) if static_asset(p).is_some() => {
            let (body, content_type) = static_asset(p).expect("checked by the guard");
            // T-435: revalidated on every load, so a new build never runs
            // with a module the browser kept from the old one.
            let no_cache = Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..]).expect("ASCII header");
            with_type(Response::from_string(body), content_type).with_header(no_cache)
        }
        (Method::Get, "/api/state") => state(shared),
        (Method::Get, "/api/diagnostics/report") => diagnostic_report(request.url(), shared),
        // À propos (T-452): version, folders, third-party files, release
        // notes. Local only, no update check. The first read on a new data
        // folder records the version (no « Nouveautés » on a first launch).
        (Method::Get, "/api/about") => {
            let (data_dir, projects_dir) = {
                let s = shared.lock().unwrap();
                (s.project.data_dir.clone(), s.project.dir.clone())
            };
            json_response(json!(crate::about::info(&data_dir, &projects_dir)))
        }
        // « Fermer » on the « Nouveautés » window: the notes are not shown again.
        (Method::Post, "/api/about/seen") => {
            let data_dir = shared.lock().unwrap().project.data_dir.clone();
            match crate::about::mark_seen(&data_dir) {
                Ok(()) => json_response(json!({ "last_seen_version": crate::about::VERSION })),
                Err(e) => text(500, &format!("impossible d'enregistrer les préférences : {e:#}")),
            }
        }
        // Vues et préférences d'opérateur (T-426): files under
        // <data-dir>/operators, never Shared.settings, the deck or the output.
        (Method::Get, "/api/operators") => {
            let data_dir = shared.lock().unwrap().project.data_dir.clone();
            json_response(crate::operators::summary(&data_dir))
        }
        (Method::Post, "/api/operators/current") => match body::<OperatorRequest>(request) {
            Ok(r) => {
                let data_dir = shared.lock().unwrap().project.data_dir.clone();
                match crate::operators::set_current(&data_dir, r.name.trim()) {
                    Ok(()) => json_response(json!({ "current": crate::operators::current(&data_dir), "profile": crate::operators::load(&data_dir, r.name.trim()) })),
                    Err((code, msg)) => text(code, &msg),
                }
            }
            Err(resp) => resp,
        },
        (Method::Post, "/api/operators/delete") => match body::<OperatorRequest>(request) {
            Ok(r) => {
                let data_dir = shared.lock().unwrap().project.data_dir.clone();
                match crate::operators::delete(&data_dir, r.name.trim()) {
                    Ok(()) => json_response(crate::operators::summary(&data_dir)),
                    Err((code, msg)) => text(code, &msg),
                }
            }
            Err(resp) => resp,
        },
        (Method::Get, p) if p.starts_with("/api/operators/") => match operator_name(p) {
            Some(name) => {
                let data_dir = shared.lock().unwrap().project.data_dir.clone();
                json_response(json!(crate::operators::load(&data_dir, &name)))
            }
            None => text(400, "nom d'opérateur invalide"),
        },
        (Method::Post, p) if p.starts_with("/api/operators/") => save_operator(request, shared, p),
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
            json_response(json!({ "capture": hub.capture_enabled(), "devices": hub.devices(), "system_audio": hub.system_support() }))
        }
        (Method::Get, "/api/audio/diagnostics") => {
            // T-242: read-only, from what the audio threads published (no
            // CoreAudio call here, nothing recorded).
            let hub = Arc::clone(&shared.lock().unwrap().audio_in);
            json_response(hub.diagnostics(Instant::now()))
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
            // A backup in standby (T-456) connects nothing; going back to
            // preview is always allowed.
            Ok(req) if !req["id"].is_null() && shared.lock().unwrap().output_mgr.hold().is_some() => {
                let reason = shared.lock().unwrap().output_mgr.hold().unwrap_or_default();
                text(409, &reason)
            }
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
        (Method::Get, "/api/perf") => {
            // T-448: the engine's last report, a copy; the pages counted now.
            let s = shared.lock().unwrap();
            let report = crate::perf::PerfReport { clients: s.presence.client_count(), ..s.perf.clone() };
            drop(s);
            let mut v = serde_json::to_value(report).unwrap_or_default();
            v["warn_worst_ms"] = json!(crate::perf::WARN_WORST_MS);
            v["late_ms"] = json!(crate::perf::LATE_MS);
            json_response(v)
        }
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
        (Method::Post, "/api/test/power") => {
            // `--test-hooks` only (e2e of T-495): what the battery and
            // thermal reading finds, `{ "source", "battery_pct", "thermal" }`.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<crate::power_watch::PowerStatus>(request) {
                Ok(status) => {
                    shared.lock().unwrap().power_watch.simulate(status);
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
        (Method::Post, "/api/test/panic") => {
            // `--test-hooks` only (T-449): proves the API robustness harness
            // sees a handler panic. The lock is released first; the panic
            // hook then cuts the output and stops the studio, as for any panic.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            panic!("POST /api/test/panic (--test-hooks)");
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
        (Method::Post, "/api/test/clock") => {
            // `--test-hooks` only (e2e of T-455): the agenda's clock reads
            // `at` (local time) from now on, in zone `tz`.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<SimClock>(request) {
                Ok(sim) => {
                    let s = &mut *shared.lock().unwrap();
                    match crate::agenda::set_test_clock(s, &sim.at, sim.tz.as_deref()) {
                        Ok(()) => json_response(crate::agenda::view(s)),
                        Err(e) => text(400, &e),
                    }
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
        (Method::Post, "/api/test/psn") => {
            // `--test-hooks` only (e2e of T-457): tracker positions as
            // packets built by our encoder, handed to the receiver as its
            // socket would (no network). Ignored while tracking is off.
            if !shared.lock().unwrap().test_hooks {
                return text(404, "not found");
            }
            match body::<SimPsn>(request) {
                Ok(sim) => {
                    let s = shared.lock().unwrap();
                    let list: Vec<(u16, [f32; 3], Option<f32>)> = sim.trackers.iter().take(64).map(|t| (t.id, t.pos, t.validity)).collect();
                    let names: Vec<(u16, &str)> = sim.trackers.iter().take(64).filter_map(|t| t.name.as_deref().map(|n| (t.id, n))).collect();
                    match crate::psn::inject(&s, &list, &names, Instant::now()) {
                        Ok(()) => json_response(json!({ "ok": true })),
                        Err(e) => text(409, &e),
                    }
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
                // A main machine whose backup is unreachable (T-456): 409
                // `{armed, blocking, confirm_backup_lost}` without
                // `"confirm_backup_lost": true`, sent only by the
                // operator's confirmation in the page.
                let confirm = req.get("confirm_backup_lost").and_then(|c| c.as_bool()).unwrap_or(false);
                let backup_lost = if want { crate::standby::arm_check(&s, confirm).err() } else { None };
                let response = if let Some(why) = backup_lost {
                    with_status(json_response(json!({ "armed": false, "blocking": [why], "confirm_backup_lost": crate::standby::BACKUP_LOST_CONFIRM_FR })), 409)
                } else if want {
                    // `request_arm` returns Ok without doing anything when
                    // already armed: only a real disarmed-to-armed change
                    // logs the confirmation.
                    let was_armed = s.gate.is_armed();
                    match s.request_arm(source) {
                        Ok(()) => {
                            if !was_armed {
                                crate::standby::note_armed(&s, source);
                            }
                            json_response(json!({ "armed": s.gate.is_armed() }))
                        }
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
                        // Safety lock (T-261): tighter only, no confirmation.
                        let lock_on = safety_lock_on(&mut s);
                        let before = s.safety.get();
                        match s.safety.set(cfg, confirm && !lock_on) {
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
                            Err(crate::safety::SetError::Loosens(list)) if lock_on => safety_locked_list(&list),
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
        // Mirror targets (T-109, targets.rs): the list, where each one's
        // beam goes and what the safety stage does to it there.
        (Method::Get, "/api/targets") => json_response(targets_view(&shared.lock().unwrap())),
        // `{items, confirm_loosen?}`: the whole list. A new or moved target
        // is a loosening: 409 `{error, loosen}` without `"confirm_loosen":
        // true` (sent only by the operator's confirmation in the UI), 409
        // `{error, loosen, armed: true}` while armed, 423 under the safety
        // lock; one below the horizon is 400. Removing or renaming always
        // runs, except that with tracking links (T-457) any change in the
        // number of fixed targets renumbers the mobile ones: a loosening.
        // Never arms.
        (Method::Post, "/api/targets") => match body::<TargetsRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let lock_on = safety_lock_on(&mut s);
                let armed = s.gate.is_armed();
                let (cfg, cal) = (s.effective_safety(), s.calibration);
                let before = s.targets.get().clone();
                let new = crate::targets::Targets { items: req.items };
                // Mobile targets (T-457) are numbered after the fixed ones.
                let mobile = s.psn.settings().links.len();
                match s.targets.set_tracked(new, &cfg, &cal, armed || lock_on, req.confirm_loosen && !lock_on, mobile) {
                    Ok(stored) => {
                        if let Some(mut change) = crate::safety_log::settings_change(&before, &stored) {
                            change["confirmed_loosen"] = json!(req.confirm_loosen && !before.loosenings_tracked(&stored, mobile).is_empty());
                            s.gate.log().record("mirror_targets", Some("ui"), change);
                        }
                        json_response(targets_view(&s))
                    }
                    Err(crate::targets::SetError::Invalid(msg)) => text(400, &msg),
                    Err(crate::targets::SetError::Armed(list)) if lock_on => safety_locked_list(&list),
                    Err(crate::targets::SetError::Armed(list)) => with_status(
                        json_response(json!({ "error": "Désarmez le laser pour ajouter ou déplacer une cible", "loosen": list, "armed": true })),
                        409,
                    ),
                    Err(crate::targets::SetError::Loosens(list)) => with_status(
                        json_response(json!({ "error": "Ajouter ou déplacer une cible assouplit la sécurité : confirmation requise", "loosen": list })),
                        409,
                    ),
                }
            }
            Err(e) => e,
        },
        // Target tracking (T-457, psn.rs): settings, links, calibrations.
        (Method::Get, "/api/psn") => json_response(crate::psn::view(&shared.lock().unwrap(), Instant::now())),
        (Method::Get, "/api/psn/trackers") => json_response(crate::psn::trackers_view(&shared.lock().unwrap(), Instant::now())),
        // `{settings…, confirm_loosen?}`: the whole settings. Switching on,
        // a new or changed link, a higher limit: 409 `{error, loosen}`
        // without the confirmation, 409 `{…, armed: true}` while armed, 423
        // under the safety lock or in show mode. Switching off and removing
        // always run. Never arms.
        (Method::Post, "/api/psn") => match body::<PsnRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let lock_on = safety_lock_on(&mut s);
                let armed = s.gate.is_armed();
                let before = s.psn.settings().clone();
                match s.psn.set(req.settings, armed || lock_on, req.confirm_loosen && !lock_on) {
                    Ok(stored) => {
                        if let Some(mut change) = crate::safety_log::settings_change(&before, &stored) {
                            change["confirmed_loosen"] = json!(req.confirm_loosen && !before.loosenings(&stored).is_empty());
                            s.gate.log().record("psn_settings", Some("ui"), change);
                        }
                        json_response(crate::psn::view(&s, Instant::now()))
                    }
                    Err(crate::psn::SetError::Invalid(msg)) => text(400, &msg),
                    Err(crate::psn::SetError::Armed(list)) if lock_on => safety_locked_list(&list),
                    Err(crate::psn::SetError::Armed(list)) => with_status(
                        json_response(json!({ "error": "Désarmez le laser pour activer le suivi ou changer une cible mobile", "loosen": list, "armed": true })),
                        409,
                    ),
                    Err(crate::psn::SetError::Loosens(list)) => with_status(
                        json_response(json!({ "error": "Ce changement assouplit la sécurité : confirmation requise", "loosen": list })),
                        409,
                    ),
                }
            }
            Err(e) => e,
        },
        // Safety lock (T-261, safetylock.rs).
        (Method::Get, "/api/safety/lock") => json_response(shared.lock().unwrap().safety_lock.view(Instant::now())),
        (Method::Post, "/api/safety/lock") => {
            let local = request.remote_addr().is_none_or(|a| a.ip().is_loopback());
            match body::<SafetyLockRequest>(request) {
                Ok(req) if local => safety_lock_route(shared, req),
                Ok(_) => text(403, "le verrou de sécurité se règle depuis l'interface de ce Mac"),
                Err(e) => e,
            }
        }
        (Method::Post, "/api/safety/unlock") => {
            let local = request.remote_addr().is_none_or(|a| a.ip().is_loopback());
            match body::<SafetyUnlockRequest>(request) {
                Ok(req) if local => safety_unlock_route(shared, req),
                Ok(_) => text(403, "le verrou de sécurité s'ouvre depuis l'interface de ce Mac"),
                Err(e) => e,
            }
        }
        // « Ciel coupé » (T-262): `{on: bool}` or `{toggle: true}`, with
        // `source`. Cutting is always accepted (show mode included: it only
        // darkens); restoring only from the UI, keyboard or local API
        // (403 otherwise). Never arms nor disarms.
        (Method::Post, "/api/safety/sky") => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let source = ArmSource::parse(req.get("source").and_then(|v| v.as_str()).unwrap_or(""));
                let mut s = shared.lock().unwrap();
                let want = match (req.get("on").and_then(|v| v.as_bool()), req.get("toggle").and_then(|v| v.as_bool())) {
                    (Some(on), _) => on,
                    (None, Some(true)) => !s.sky_off,
                    _ => return text(400, "expected \"on\" or \"toggle\""),
                };
                // Safety lock (T-261): cutting always, restoring never.
                if !want && s.sky_off && safety_lock_on(&mut s) {
                    return safety_locked();
                }
                if s.set_sky_off(want, source) {
                    json_response(outdoor_view(&s))
                } else {
                    text(403, "Rétablir le ciel se fait depuis l'interface seulement")
                }
            }
            Err(e) => e,
        },
        // The declared sectors as on the DGTA form (T-262 / T-263).
        (Method::Get, "/api/safety/outdoor/export") => {
            let csv = shared.lock().unwrap().safety.get().outdoor.export_csv();
            let header = Header::from_bytes(&b"Content-Disposition"[..], &b"attachment; filename=\"secteurs-dgta.csv\""[..]).expect("ASCII header");
            with_type(Response::from_string(csv), "text/csv; charset=utf-8").with_header(header)
        }
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
                // Safety lock (T-261): a tighter profile only.
                let lock_on = safety_lock_on(&mut s);
                match crate::profiles::load(&mut s, &req.name, req.confirm_loosen && !lock_on, src) {
                    Ok(_) => json_response(crate::profiles::view(&s)),
                    Err(crate::profiles::LoadError::Loosens(list)) if lock_on => safety_locked_list(&list),
                    Err(e) => profile_error(e),
                }
            }
            Err(e) => e,
        },
        // Site profiles (T-289): calibration, safety and projection zones of
        // a venue. Locked in show mode (423); never recorded by undo.
        (Method::Get, "/api/sites") => json_response(crate::sites::view(&shared.lock().unwrap())),
        // `{action: "load", name, confirm_loosen?}`: disarms; looser than
        // what is in force: 409 `{error, loosen}` without `confirm_loosen`,
        // nothing changed. `{action: "save", name}` (« Enregistrer sous… »,
        // the settings in force), `{action: "delete", name}`.
        (Method::Post, "/api/sites") => match body::<SiteRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
                if !crate::profiles::operator_source(src) {
                    return text(403, &crate::profiles::LoadError::Source.message_fr());
                }
                let done = match req.action.as_str() {
                    "load" => crate::sites::load(&mut s, &req.name, req.confirm_loosen, src).map(|_| ()),
                    "save" => crate::sites::save(&mut s, &req.name).map(|_| ()).map_err(|e| crate::profiles::LoadError::Damaged(format!("{e:#}"))),
                    "delete" => crate::sites::delete(&mut s, &req.name).map_err(|e| crate::profiles::LoadError::Damaged(format!("{e:#}"))),
                    _ => return text(400, "expected \"action\": \"load\", \"save\" or \"delete\""),
                };
                match done {
                    Ok(()) => json_response(crate::sites::view(&s)),
                    Err(e) => site_error(e),
                }
            }
            Err(e) => e,
        },
        // *Utiliser celui du projet*: the open project's site, `{confirm_loosen?}`.
        (Method::Post, "/api/sites/use-project") => match body::<SiteRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
                match crate::sites::use_project(&mut s, req.confirm_loosen, src) {
                    Ok(_) => json_response(crate::sites::view(&s)),
                    Err(e) => site_error(e),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/pack/export") => export_pack(request.url(), shared),
        (Method::Post, "/api/pack/import") => import_pack(request, shared),
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
                        if safety_lock_on(&mut s) {
                            return safety_locked();
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
                // Safety lock (T-261): lower caps only; the projector sheet
                // (what the exposure and the class come from) stays.
                let lock_on = safety_lock_on(s);
                if lock_on && b.projector.as_ref().is_some_and(|p| before.as_ref().is_none_or(|o| o.projector != *p)) {
                    return safety_locked();
                }
                match s.outputs.set(id, b.limits, b.projector, armed || lock_on, b.confirm_loosen && !lock_on) {
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
                    Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) if lock_on => safety_locked_list(&list),
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
                    // Safety lock (T-261): darker only.
                    let lock_on = safety_lock_on(s);
                    match s.outputs.set_color(&id, b.color, armed || lock_on, b.confirm_loosen && !lock_on) {
                        Ok(out) => {
                            if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out.color)) {
                                change["output"] = json!(id);
                                change["confirmed_loosen"] = json!(b.confirm_loosen);
                                s.gate.log().record("output_color", Some("ui"), change);
                            }
                            json_response(color_view(s, &id))
                        }
                        Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                        Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) if lock_on => safety_locked_list(&list),
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
                Ok(mut b) => {
                    let mut s = shared.lock().unwrap();
                    let s = &mut *s;
                    s.gate.sync_estop(&s.estop);
                    let armed = s.gate.is_armed();
                    let before = s.outputs.get(&id).map(|o| o.scan);
                    // The fixed rate (T-463) has its own route: kept as it is.
                    b.scan.fixed_fps = before.and_then(|b| b.fixed_fps);
                    // Safety lock (T-261): slower only.
                    let lock_on = safety_lock_on(s);
                    match s.outputs.set_scan(&id, b.scan, armed || lock_on) {
                        Ok(out) => {
                            if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out.scan)) {
                                change["output"] = json!(id);
                                s.gate.log().record("output_scan", Some("ui"), change);
                            }
                            json_response(scan_limits_view(s, &id))
                        }
                        Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                        Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) if lock_on => safety_locked_list(&list),
                        Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) => with_status(
                            json_response(json!({ "error": "Désarmez le laser pour relever la vitesse max", "loosen": list, "armed": true })),
                            409,
                        ),
                    }
                }
                Err(e) => e,
            }
        }
        // Fixed frame rate for video capture (T-463) of one output,
        // `?output=<id or index>` (default the main output). Body: `{fps:
        // number | null}` (null = off). Not a number or outside 20..60 is
        // 400. Turning it on or changing it while armed (or under the
        // safety lock) is 409: every frame is resampled at once, a setting
        // for before the show. Off always applies. Not a control: no MIDI,
        // DMX, OSC or timeline reaches it. Refused in show mode.
        (Method::Post, "/api/outputs/fixed_rate") => {
            let id = match color_output(request.url(), &shared.lock().unwrap()) {
                Ok(id) => id,
                Err(e) => return e,
            };
            match body::<FixedRateBody>(request) {
                Ok(b) => {
                    let mut s = shared.lock().unwrap();
                    let s = &mut *s;
                    s.gate.sync_estop(&s.estop);
                    let locked = s.gate.is_armed() || safety_lock_on(s);
                    let before = s.outputs.get(&id).map(|o| o.scan);
                    match s.outputs.set_fixed_fps(&id, b.fps, locked) {
                        Ok(out) => {
                            if let Some(mut change) = before.and_then(|before| crate::safety_log::settings_change(&before, &out.scan)) {
                                change["output"] = json!(id);
                                s.gate.log().record("output_scan", Some("ui"), change);
                            }
                            json_response(scan_limits_view(s, &id))
                        }
                        Err(crate::power::SetError::Invalid(msg)) => text(400, &msg),
                        Err(crate::power::SetError::Raises(list) | crate::power::SetError::Armed(list)) => with_status(
                            json_response(json!({ "error": "Désarmez le laser (et levez le verrouillage de sécurité) pour régler la cadence fixe", "loosen": list, "armed": true })),
                            409,
                        ),
                    }
                }
                Err(e) => e,
            }
        }
        // T-447: `{"on": true, "only": <zone id> | null, "client_id": "…"}`
        // traces the zones' outlines for 60 s, `{"on": false}` stops it.
        // The local UI only: not a control, no remote, no external input.
        // Never arms; refused under the e-stop; in show mode, stopping only.
        (Method::Post, "/api/safety/trace") => {
            let local = request.remote_addr().is_some_and(|a| a.ip().is_loopback());
            let req = match body::<serde_json::Value>(request) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if !local {
                return text(403, "le tracé des zones ne se lance que depuis l'interface de ce Mac");
            }
            let Some(on) = req.get("on").and_then(|v| v.as_bool()) else {
                return text(400, "on: true ou false");
            };
            let only = match req.get("only") {
                None | Some(serde_json::Value::Null) => None,
                Some(v) => match v.as_u64().and_then(|id| u32::try_from(id).ok()) {
                    Some(id) => Some(id),
                    None => return text(400, "only: identifiant de zone ou null"),
                },
            };
            let now = Instant::now();
            let client = req.get("client_id").and_then(|v| v.as_str());
            let mut s = shared.lock().unwrap();
            if let Some(id) = client {
                s.presence.touch(id, now);
            }
            if on {
                if s.show_mode.on {
                    return locked();
                }
                if safety_lock_on(&mut s) {
                    return safety_locked();
                }
                if let Err(e) = crate::zonetrace::start(&mut s, now, only, client) {
                    return text(409, &e);
                }
            } else {
                crate::zonetrace::stop(&mut s, "user");
            }
            json_response(crate::zonetrace::view(&s, now))
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
                // Safety lock (T-261): leaving a pattern or dimming it only.
                if (matches!(pattern, Some(Some(_))) || brightness.is_some_and(|b| b > s.test_pattern.brightness)) && safety_lock_on(&mut s) {
                    return safety_locked();
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
        // The safety sheet (T-263): a printable HTML summary of the
        // settings in force. GET: the settings only (event fields left
        // blank to fill by hand); POST `{event, include_log,
        // include_notes}`: with what the operator typed. Reads only: the
        // body (with its mobile number) is written nowhere.
        (Method::Get, "/api/safety/sheet") => safety_sheet(&crate::safetysheet::SheetRequest::default(), shared),
        (Method::Post, "/api/safety/sheet") => match body::<crate::safetysheet::SheetRequest>(request) {
            Ok(req) => match req.event.validate() {
                Ok(()) => safety_sheet(&req, shared),
                Err(e) => text(400, &e.to_string()),
            },
            Err(_) => text(400, "fiche sécurité : requête illisible"),
        },
        (Method::Post, "/api/calibration") => match body::<Calibration>(request) {
            Ok(cal) => {
                let cal = cal.sanitized();
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
        // The 3D view's viewpoint (T-278): a view, like a camera move. Not
        // an edit (never in the undo history, allowed in show mode), never
        // touches the projectors, the outputs or the arm state.
        (Method::Post, "/api/venue/camera") => match body::<crate::venue::CameraState>(request) {
            Ok(camera) => json_response(json!({ "camera": shared.lock().unwrap().venue.set_camera(camera) })),
            Err(e) => e,
        },
        // The previews' galvo simulation and divergence (T-280): display
        // settings the page reads back, like the camera. Display only:
        // nothing here reaches the engine, the safety chain, an output or
        // an export. A non-finite number is refused, the rest clamped.
        (Method::Post, "/api/venue/sim") => match body::<crate::venue::SimPatch>(request) {
            Ok(patch) => match shared.lock().unwrap().venue.set_sim(&patch) {
                Ok(render) => json_response(json!({ "render": render })),
                Err(e) => text(400, &e.to_string()),
            },
            Err(e) => e,
        },
        (Method::Get, "/api/frames") => frames(shared),
        (Method::Get, "/api/projection") => json_response(projection_view(&shared.lock().unwrap())),
        // Zone groups and their masters (T-420): levels move through
        // `/api/control` (`group.<n>.master`), never through here.
        (Method::Get, "/api/groups") => json_response(groups_view(&shared.lock().unwrap())),
        // Zone chasers (T-170): where each stands now. They are edited with
        // the zones (`POST /api/projection`), played through `/api/control`
        // (`chaser.<n>.enabled`, `chaser.rate`).
        (Method::Get, "/api/zone-chase") => json_response(chase_view(&shared.lock().unwrap())),
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
        // Zone alignment from a phone (T-460): the session is opened, and
        // the draft applied, from this server only (127.0.0.1); the
        // remote's own server only moves the draft.
        (Method::Post, "/api/zones/align/open") => {
            let local = request.remote_addr().is_some_and(|a| a.ip().is_loopback());
            let req = match body::<AlignOpen>(request) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if !local {
                return text(403, "l'alignement ne s'ouvre que depuis l'interface de ce Mac");
            }
            let mut s = shared.lock().unwrap();
            match crate::remote::align_open(&mut s, req.zone, &req.remote, Instant::now()) {
                Ok(()) => json_response(json!({ "align": crate::remote::align_view(&s) })),
                Err((code, t)) => text(code, &t),
            }
        }
        (Method::Post, "/api/zones/align/apply") => {
            if !request.remote_addr().is_some_and(|a| a.ip().is_loopback()) {
                return text(403, "« Appliquer » ne vaut que depuis l'interface de ce Mac");
            }
            let mut s = shared.lock().unwrap();
            match crate::remote::align_apply(&mut s) {
                Ok(()) => json_response(json!({ "align": null, "projection": projection_view(&s) })),
                Err((code, t)) => text(code, &t),
            }
        }
        (Method::Post, "/api/zones/align/reject") | (Method::Post, "/api/zones/align/close") => {
            let mut s = shared.lock().unwrap();
            crate::remote::align_end(&mut s, if path.ends_with("reject") { "rejected" } else { "closed" });
            json_response(json!({ "align": null }))
        }
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
                    // A figure cue's named parts (T-441): the targets of
                    // the Direct panel's « Cible » and the part modulators.
                    if let crate::engine::Content::Figure(f) = &p.settings.content {
                        if !f.parts.is_empty() {
                            v["parts"] = json!(f.parts);
                        }
                    }
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
                // A zone chaser (T-170) may be created later; one past the
                // most there can be is a mistake.
                if req.slot.zone_chaser.is_some_and(|c| c >= crate::zonechase::MAX_CHASERS) {
                    return text(400, "chenillard de zones inconnu");
                }
                let CueSlot { program, modifiers, part_modifiers, refs: before, .. } = s.deck.slot(&req.id);
                let mut slot = req.slot;
                if let Err(e) = crate::pools::check_refs(&s.pools, &before, &mut slot.refs) {
                    return text(400, &e);
                }
                s.deck.set_slot(&req.id, CueSlot { program, modifiers, part_modifiers, ..slot });
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
        // A figure cue's per-part modifiers (T-441): `modifiers` for part
        // `part` of cue `id` (`null` or neutral removes them). Stored in
        // the cell, applied to that part only, before the cue's own
        // modifiers and the whole chain. An edit: in the history, refused
        // in show mode.
        (Method::Post, "/api/cues/parts") => match body::<PartModifiersRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                if !req.id.starts_with(crate::figures::ID_PREFIX) || !s.presets.iter().any(|p| p.id == req.id) {
                    return text(404, "cue de figure inconnu");
                }
                if !(1..=crate::figures::MAX_PARTS).contains(&req.part) {
                    return text(400, &format!("partie 1 à {} attendue", crate::figures::MAX_PARTS));
                }
                let mut slot = s.deck.slot(&req.id);
                match req.modifiers {
                    Some(m) => slot.part_modifiers.insert(req.part, m),
                    None => slot.part_modifiers.remove(&req.part),
                };
                s.deck.set_slot(&req.id, slot);
                s.deck.save();
                json_response(json!(s.deck.slot(&req.id).part_modifiers))
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
        // CRÉATION › Abstraits (T-440): the editor's live preview, drawn by
        // the engine on its own animator within the layer's point budget.
        // Display only: nothing is stored, nothing reaches the output.
        (Method::Post, "/api/abstracts/preview") => match body::<AbstractRequest>(request) {
            Ok(req) => {
                let Some(look) = crate::abstracts::clean_look(&req.look) else { return text(400, "ce look n'est pas un abstrait") };
                let (bpm, beats_per_bar, budget) = {
                    let s = shared.lock().unwrap();
                    (s.tempo.bpm, s.tempo.beats_per_bar, s.mixer.point_budget)
                };
                json_response(json!(crate::abstracts::preview(&look, bpm, beats_per_bar, budget)))
            }
            Err(e) => e,
        },
        // « Enregistrer comme cue… » (T-440): a new user cue of the
        // editor's abstract, in the first free cell of a user page. An
        // edit: in the history, locked in show mode.
        (Method::Post, "/api/abstracts/store") => match body::<AbstractRequest>(request) {
            Ok(req) => {
                let Some(look) = crate::abstracts::clean_look(&req.look) else { return text(400, "ce look n'est pas un abstrait") };
                let mut s = shared.lock().unwrap();
                match crate::usercues::store_look(&mut s, look, req.name.as_deref()) {
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
        // Referenced presets (T-422, pools.rs). Edits only (never in show
        // mode); none of them arms or touches a safety setting.
        (Method::Get, "/api/pools") => json_response(crate::pools::view(&shared.lock().unwrap())),
        (Method::Post, "/api/pools") => match body::<Vec<crate::pools::PoolPreset>>(request) {
            Ok(list) => match crate::pools::set_all(&mut shared.lock().unwrap(), list) {
                Ok(()) => ok(),
                Err((code, v)) => with_status(json_response(v), code),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/pools/delete") => match body::<IdRequest>(request) {
            Ok(req) => match crate::pools::delete(&mut shared.lock().unwrap(), &req.id) {
                Ok(()) => ok(),
                Err((code, v)) => with_status(json_response(v), code),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/pools/capture") => match body::<crate::pools::CaptureRequest>(request) {
            Ok(req) => match crate::pools::capture(&mut shared.lock().unwrap(), &req) {
                Ok(p) => json_response(json!(p)),
                Err((code, v)) => with_status(json_response(v), code),
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
            // Figure parts a modulator may move instead (T-441).
            let parts: Vec<_> = s
                .figures
                .list()
                .iter()
                .filter(|f| !f.parts.is_empty())
                .map(|f| json!({ "cue": crate::figures::cue_id(&f.name), "figure": f.name, "parts": f.parts }))
                .collect();
            json_response(json!({ "lfos": s.lfos.list(), "targets": targets, "parts": parts, "max": crate::lfo::MAX_MODULATORS }))
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
                match bank.validate(|id| s.presets.iter().any(|p| p.id == id), |id| s.macros.get(id).is_some()) {
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
        // Macros (T-425): GET the list and what runs; POST replaces the
        // list (checked line by line: an edit, in the history, locked in
        // show mode). Running, stopping and Go are playback.
        (Method::Get, "/api/macros") => json_response(crate::macros::view(&shared.lock().unwrap())),
        (Method::Post, "/api/macros") => match body::<MacroListRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                match crate::macros::set(s, req.macros) {
                    Ok(()) => json_response(crate::macros::view(s)),
                    Err(e) => with_status(json_response(json!(e)), 400),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/macros/run") => match body::<MacroIdRequest>(request) {
            Ok(MacroIdRequest { id: Some(id), source }) => {
                let s = &mut *shared.lock().unwrap();
                match crate::macros::start(s, &id, controls::ControlSource::from_id(source.as_deref())) {
                    Ok(()) => json_response(crate::macros::view(s)),
                    Err(why) => text(409, why),
                }
            }
            Ok(_) => text(400, "macro manquante"),
            Err(e) => e,
        },
        (Method::Post, "/api/macros/stop") => match body::<MacroIdRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                match req.id {
                    Some(id) => {
                        crate::macros::stop(s, &id);
                    }
                    None => crate::macros::stop_all(s),
                }
                json_response(crate::macros::view(s))
            }
            Err(e) => e,
        },
        // Scripts (T-458): GET the list, the states and consoles; POST
        // replaces the list (checked: refused lines never saved; an edit,
        // in the history, locked in show mode). Run and stop are playback.
        (Method::Get, "/api/scripts") => json_response(crate::script::view(&shared.lock().unwrap())),
        (Method::Post, "/api/scripts") => match body::<ScriptListRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                match crate::script::set(s, req.scripts) {
                    Ok(()) => json_response(crate::script::view(s)),
                    Err(e) => with_status(json_response(json!(e)), 400),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/scripts/run") => match body::<ScriptIdRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                let r = match req.id {
                    Some(id) => crate::script::request_run(s, id, controls::ControlSource::from_id(req.source.as_deref())),
                    // No id: *Reprendre les scripts*.
                    None => crate::script::resume(s),
                };
                match r {
                    Ok(()) => json_response(crate::script::view(s)),
                    Err(why) => text(409, why),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/scripts/stop") => match body::<ScriptIdRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                match req.id {
                    Some(id) => {
                        crate::script::stop(s, id);
                    }
                    None => crate::script::stop_all(s),
                }
                json_response(crate::script::view(s))
            }
            Err(e) => e,
        },
        (Method::Post, "/api/scripts/check") => match body::<ScriptCheckRequest>(request) {
            Ok(req) => {
                let s = &*shared.lock().unwrap();
                match crate::script::check_source(s, &req.source) {
                    None => json_response(json!({ "ok": true })),
                    Some(e) => json_response(json!({ "ok": false, "error": e })),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/scripts/trust") => match body::<ScriptTrustRequest>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                crate::script::approve(s, req.approve);
                json_response(crate::script::view(s))
            }
            Err(e) => e,
        },
        // The show player (T-444): GET the list and its state; POST
        // replaces the list (an edit: history, locked in show mode); the
        // transport and *Boucler la liste* are playback. Never arms.
        (Method::Get, "/api/showlist") => json_response(crate::showlist::view(&shared.lock().unwrap())),
        (Method::Post, "/api/showlist") => match body::<crate::showlist::ShowList>(request) {
            Ok(list) => {
                let s = &mut *shared.lock().unwrap();
                match crate::showlist::set(s, list) {
                    Ok(()) => json_response(crate::showlist::view(s)),
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        // The agenda (T-455): GET the entries, the state and the clock;
        // POST replaces the agenda (an edit: history, locked in show mode).
        (Method::Get, "/api/agenda") => json_response(crate::agenda::view(&shared.lock().unwrap())),
        (Method::Post, "/api/agenda") => match body::<crate::agenda::Agenda>(request) {
            Ok(agenda) => {
                let s = &mut *shared.lock().unwrap();
                match crate::agenda::set(s, agenda) {
                    Ok(()) => json_response(crate::agenda::view(s)),
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/showlist/play") => match body::<ShowListPlay>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                match crate::showlist::play(s, req.index) {
                    Ok(()) => json_response(crate::showlist::view(s)),
                    Err(e) => text(409, &e),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/showlist/stop") => {
            let s = &mut *shared.lock().unwrap();
            crate::showlist::stop(s);
            json_response(crate::showlist::view(s))
        }
        (Method::Post, "/api/showlist/next") | (Method::Post, "/api/showlist/prev") => {
            let s = &mut *shared.lock().unwrap();
            let r = if path == "/api/showlist/next" { crate::showlist::next(s) } else { crate::showlist::prev(s) };
            match r {
                Ok(()) => json_response(crate::showlist::view(s)),
                Err(e) => text(409, &e),
            }
        }
        (Method::Post, "/api/showlist/loop") => match body::<ShowListLoop>(request) {
            Ok(req) => {
                let s = &mut *shared.lock().unwrap();
                crate::showlist::set_loop(s, req.on);
                json_response(crate::showlist::view(s))
            }
            Err(e) => e,
        },
        (Method::Post, "/api/macros/go") => {
            let s = &mut *shared.lock().unwrap();
            let n = crate::macros::go(s);
            json_response(json!({ "advanced": n }))
        }
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
        // Pilote automatique (T-148): the pages and the public content
        // (an edit: history, locked in show mode); playing it is `vlj.*`.
        (Method::Get, "/api/autopilot") => json_response(crate::autopilot::view(&shared.lock().unwrap())),
        (Method::Post, "/api/autopilot") => match body::<crate::autopilot::AutopilotEdit>(request) {
            Ok(edit) => {
                let s = &mut *shared.lock().unwrap();
                match crate::autopilot::set_config(s, edit) {
                    Ok(()) => json_response(crate::autopilot::view(s)),
                    Err(e) => text(400, &e),
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
            Ok(show) => {
                let s = &mut *shared.lock().unwrap();
                match s.shows.save(&show) {
                    Ok(()) => {
                        // The show list's times and next show (T-444).
                        crate::showlist::refresh(s);
                        ok()
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
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
        // 3D objects (T-462): the library, also the « Objets 3D » cue page.
        (Method::Get, "/api/objects") => {
            let s = shared.lock().unwrap();
            let list: Vec<_> = s.objects.list().iter().map(object_summary).collect();
            json_response(json!({
                "objects": list,
                "budget": s.mixer.point_budget,
                "max_bytes": crate::figure_import::obj::MAX_OBJ_BYTES,
                "max_edges": crate::wire::MAX_EDGES,
            }))
        }
        // An object's properties (and its name): never its edges.
        (Method::Post, "/api/objects") => match body::<ObjectProps>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                let Some(mut obj) = s.objects.get(&req.name).cloned() else {
                    return text(404, &format!("objet introuvable : {}", req.name.trim()));
                };
                let old = obj.name.clone();
                if let Some(name) = req.rename.as_deref().map(str::trim).filter(|n| *n != old) {
                    if s.objects.get(name).is_some() {
                        return text(409, &format!("un objet s'appelle déjà « {name} »"));
                    }
                    obj.name = name.to_string();
                }
                obj.color = req.color.unwrap_or(obj.color);
                obj.period_beats = req.period_beats.unwrap_or(obj.period_beats);
                obj.per = req.per.unwrap_or(obj.per);
                obj.axis_ratio = req.axis_ratio.unwrap_or(obj.axis_ratio);
                obj.perspective = req.perspective.unwrap_or(obj.perspective);
                obj.depth_color = req.depth_color.unwrap_or(obj.depth_color);
                match s.objects.save(obj) {
                    Ok(kept) => {
                        if kept.name != old {
                            if let Err(e) = s.objects.remove(&old) {
                                log::warn!("renamed object: the old one stays: {e:#}");
                            }
                        }
                        crate::wire::refresh(&mut s);
                        json_response(object_summary(&kept))
                    }
                    Err(e) => text(400, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        (Method::Post, "/api/objects/delete") => match body::<NameRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match s.objects.remove(&req.name) {
                    Ok(()) => {
                        crate::wire::refresh(&mut s);
                        ok()
                    }
                    Err(e) => text(404, &format!("{e:#}")),
                }
            }
            Err(e) => e,
        },
        // The OBJ file in the body, options in the query; `preview=1`
        // answers without saving.
        (Method::Post, "/api/objects/import") => import_object(request, shared),
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
                Err(e) if crate::templates::is_builtin(&req.name) => text(409, &format!("{e:#}")),
                Err(e) => text(404, &format!("{e:#}")),
            },
            Err(e) => e,
        },
        (Method::Post, "/api/timeline/templates/render") => match body::<TemplateRender>(request) {
            Ok(req) => render_template(req, shared),
            Err(e) => e,
        },
        // Enregistrer le jeu en direct (T-169, take.rs).
        (Method::Get, "/api/timeline/record") => json_response(shared.lock().unwrap().take.view()),
        (Method::Post, "/api/timeline/record/start") => {
            let mut s = shared.lock().unwrap();
            if s.timeline.show.is_none() {
                return text(409, "aucun show chargé");
            }
            if s.estop.is_latched() {
                return text(409, "arrêt d'urgence actif : réinitialiser avant d'enregistrer");
            }
            match s.take.start() {
                Ok(()) => json_response(s.take.view()),
                Err(why) => text(409, why),
            }
        }
        (Method::Post, "/api/timeline/record/stop") => {
            let mut s = shared.lock().unwrap();
            s.take.stop(crate::take::EndReason::User);
            json_response(s.take.view())
        }
        (Method::Post, "/api/timeline/record/discard") => {
            let mut s = shared.lock().unwrap();
            s.take.discard();
            json_response(s.take.view())
        }
        (Method::Post, "/api/timeline/record/take") => match body::<TakeRequest>(request) {
            Ok(req) => take_layout(req, shared),
            Err(e) => e,
        },
        (Method::Post, p) if p.starts_with("/api/timeline/") => timeline_route(request, shared, &p["/api/timeline/".len()..]),
        // MIDI clock (T-207): its own lines in ROUTES (POST is Safety).
        (Method::Get, "/api/midi/clock") => midi_route(request, shared, false, "/api/midi/clock"),
        (Method::Post, "/api/midi/clock") => midi_route(request, shared, true, "/api/midi/clock"),
        // Ableton Link (T-154): `{ "enabled", "quantum" }`; 501 when the
        // build has no Link (the default), 409 while the MIDI clock is
        // followed. Tempo only: nothing here can arm.
        (Method::Get, "/api/link") => json_response(crate::ableton_link::view(&shared.lock().unwrap())),
        (Method::Post, "/api/link") => match body::<crate::ableton_link::LinkRequest>(request) {
            Ok(req) => {
                let mut s = shared.lock().unwrap();
                match crate::ableton_link::update(&mut s, req) {
                    Ok(()) => json_response(crate::ableton_link::view(&s)),
                    Err((code, e)) => text(code, &e),
                }
            }
            Err(e) => e,
        },
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
                    Err(e) => text(netin_refusal(&e), &e),
                }
            }
            Err(e) => e,
        },
        // OSC feedback, addresses and monitor (T-416).
        (Method::Get, "/api/osc/addresses") => json_response(json!(crate::netin::feedback::addresses(&shared.lock().unwrap().controls))),
        (Method::Get, "/api/osc/monitor") => {
            let dir = query_str(request.url(), "dir");
            json_response(crate::netin::osc_monitor(&shared.lock().unwrap(), dir.as_deref()))
        }
        (Method::Post, "/api/osc/monitor/clear") => {
            let mut s = shared.lock().unwrap();
            s.netin.monitor.clear();
            json_response(crate::netin::osc_monitor(&s, None))
        }
        (Method::Get, "/api/osc/config") => json_response(crate::netin::osc_view(&shared.lock().unwrap(), Instant::now())),
        (Method::Post, "/api/osc/config") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let (result, hub) = {
                    let mut s = shared.lock().unwrap();
                    (crate::netin::update(&mut s, &json!({ "feedback": patch })), Arc::clone(&s.netin.hub))
                };
                match result {
                    Ok(changed) => {
                        if !changed.is_empty() {
                            hub.save();
                        }
                        json_response(crate::netin::osc_view(&shared.lock().unwrap(), Instant::now()))
                    }
                    Err(e) => text(400, &e),
                }
            }
            Err(e) => e,
        },
        (method, p) if p.starts_with("/api/external") || p.starts_with("/api/dmx/") => external_route(request, shared, method, p),
        (method, p) if p == "/api/timecode" || p == "/api/timecode/arm" || p == "/api/timecode/out" => timecode_route(request, shared, method, p),
        (Method::Get, "/api/dmx-out") => json_response(crate::dmx_out::view(&shared.lock().unwrap())),
        (Method::Post, p) if p == "/api/dmx-out" || p == "/api/dmx-out/haze" => dmx_out_route(request, shared, p),
        (Method::Get, "/api/citp") => json_response(crate::citp::view(&shared.lock().unwrap())),
        (Method::Post, "/api/citp") => citp_route(request, shared),
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
            // Safety lock (T-261): same, cutting only.
            if req.enabled && safety_lock_on(&mut shared.lock().unwrap()) {
                return safety_locked();
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
        (Method::Get, "/api/standby") => json_response(crate::standby::view(&shared.lock().unwrap(), Instant::now())),
        (Method::Post, "/api/standby") | (Method::Post, "/api/standby/pair") | (Method::Post, "/api/standby/takeover") | (Method::Post, "/api/standby/receive") => standby_route(request, shared, &path),
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
pub(crate) fn look_key(settings: &Settings) -> String {
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
    let (settings, zones, bpm, chain) = {
        let s = shared.lock().unwrap();
        // A cue draws in its cell's projection zones, the look on show in
        // the default ones, as live (T-478).
        let (look, zones) = match query_str(url, "id").filter(|id| !id.is_empty()) {
            Some(id) => match s.presets.iter().find(|p| p.id == id) {
                Some(p) => (p.settings.clone(), s.deck.slot(&p.id).zones),
                None => return text(404, "cue introuvable"),
            },
            None => (s.settings.clone(), Vec::new()),
        };
        let chain = match crate::ilda_library::ExportSafety::of(&s, &output) {
            Ok(c) => c,
            Err(e) => return text(400, &format!("{e:#}")),
        };
        (look, zones, s.tempo.bpm, chain)
    };
    let frames = if frames >= 1.0 && frames.fract() == 0.0 { frames as usize } else { 0 };
    // Rendered and put through the safety chain with the lock released.
    match crate::ilda_library::export_cue(&settings, &zones, frames, fps, bpm, &chain) {
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
            // The show list reads the saved show again (T-444).
            crate::showlist::refresh(st);
        }
        _ => return text(404, "not found"),
    }
    json_response(json!(s.timeline.state(&c)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TakeRequest {
    /// The page's copy of the show; default: the loaded show.
    show: Option<crate::timeline::Show>,
    quantize: crate::take::Quantize,
}

/// `POST /api/timeline/record/take` (T-169): the pending take laid out as
/// new tracks *Prise n* added to the show, returned for the page to apply
/// through `/api/timeline/edit` (one undo step, locks and show mode
/// checked there). Changes nothing here; the take stays pending until
/// *Jeter* (the page drops it once the edit is accepted).
fn take_layout(req: TakeRequest, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let s = shared.lock().unwrap();
    let Some(take) = s.take.take.as_ref() else { return text(409, "aucune prise à garder") };
    let Some(loaded) = s.timeline.show.as_ref() else { return text(409, "aucun show chargé") };
    if loaded.name != take.show || loaded.time_base != take.time_base {
        return text(409, &format!("la prise a été jouée sur « {} » : rouvrez ce show pour la garder", take.show));
    }
    let mut show = req.show.unwrap_or_else(|| loaded.clone());
    show.sanitize();
    let built = match crate::take::build(take, &show, req.quantize, s.tempo.beats_per_bar, |id| s.presets.iter().any(|p| p.id == id)) {
        Ok(b) => b,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    let names: Vec<String> = built.tracks.iter().map(|t| t.name.clone()).collect();
    show.tracks.extend(built.tracks);
    show.sanitize();
    json_response(json!({ "show": show, "added": built.events, "skipped": built.skipped, "tracks": names, "number": built.number }))
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
    if crate::templates::is_builtin(&t.name) {
        return text(409, &format!("« {} » est un modèle intégré : choisissez un autre nom", t.name));
    }
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
        match read_body(request) {
            Ok(r) => raw = r,
            Err(e) => return e,
        }
    }
    match crate::midi::api::route(&mut shared.lock().unwrap(), post, path, &raw) {
        Some(crate::midi::api::Reply::Json(v)) => json_response(v),
        Some(crate::midi::api::Reply::Text(code, t)) => text(code, &t),
        None => text(404, "not found"),
    }
}

/// `GET /api/project`, `POST /api/project/{new,open,save,save-as}` (T-286);
/// templates (T-451) `GET /api/project/templates`,
/// `POST /api/project/{save-template,delete-template}`, all through the
/// `/api/project/*` line of `ROUTES` (Edit: 423 in show mode).
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
///   `GET /api/dmx/personality?mode=compact|full&format=json|md|xml|gdtf`,
///   `GET /api/dmx/mvr` (the venue file, T-500).
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
            // Safety lock (T-261): closing only.
            if want && !s.external.gate.open && safety_lock_on(&mut s) {
                return safety_locked();
            }
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
                    Err(e) => text(netin_refusal(&e), &e),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/dmx/personality") => personality(request.url()),
        (Method::Get, "/api/dmx/mvr") => {
            use crate::dmx::mvr;
            let data = {
                let s = shared.lock().unwrap();
                let selected = s.output_mgr.prefs().selected;
                mvr::mvr(s.venue.get(), s.projection.get(), &s.netin.hub.file().dmx.fixtures, selected.as_deref())
            };
            let disposition = format!("attachment; filename=\"{}\"", mvr::FILE_NAME);
            let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
            with_type(Response::from_data(data), "application/octet-stream").with_header(header)
        }
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
        match read_body(request) {
            Ok(r) => raw = r,
            Err(e) => return e,
        }
    }
    let action = path.strip_prefix("/api/project").unwrap_or("").trim_start_matches('/');
    let reply = crate::project::route(shared, post, action, &raw);
    // Another project in memory (T-285): nothing of the previous one can be
    // undone; a partial import (T-289) replaces sections the history can't.
    let real = !(action == "import-partial" && body_says_dry_run(&raw));
    if post && real && matches!(action, "new" | "open" | "recover" | "version-open" | "import-partial") && matches!(reply, Some(crate::project::Reply::Json(_))) {
        let gone = shared.lock().unwrap().history.take();
        drop(gone);
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

/// `POST /api/zones/align/open` (T-460): a projection zone and a paired
/// remote's name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AlignOpen {
    zone: u8,
    remote: String,
}

/// `GET /api/groups`: each zone group with its master and control id.
fn groups_view(s: &Shared) -> serde_json::Value {
    let groups: Vec<_> = s
        .projection
        .get()
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            // Only the first groups have a master control (T-170: up to 32 groups).
            let control = (i < crate::projection::GROUP_MASTER_CONTROLS).then(|| format!("group.{}.master", i + 1));
            json!({ "n": i + 1, "name": g.name, "zones": g.zones, "master": g.master, "control": control })
        })
        .collect();
    json!({ "groups": groups, "max_groups": crate::projection::MAX_GROUPS, "master_controls": crate::projection::GROUP_MASTER_CONTROLS })
}

/// `GET /api/zone-chase` (T-170): each zone chaser, whether it runs free,
/// its step and active group now (1-based, null when it has no step), the
/// rate, and the choices the page offers.
fn chase_view(s: &Shared) -> serde_json::Value {
    use crate::zonechase::{DEFAULT_RATE, MAX_CHASERS, MAX_STEPS, RATES, RATE_LABELS, STEP_BEATS, STEP_LABELS};
    let cfg = s.projection.get();
    let chasers: Vec<_> = cfg
        .chasers
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let now = s.zone_chase.steps.get(i).copied().flatten();
            json!({
                "n": i + 1,
                "name": c.name,
                "enabled": c.enabled,
                "control": format!("chaser.{}.enabled", i + 1),
                "step": now.map(|(step, _)| step + 1),
                "group": now.map(|(_, g)| g + 1),
            })
        })
        .collect();
    let rate = RATES.iter().position(|r| *r == cfg.chase_rate).unwrap_or(DEFAULT_RATE);
    json!({
        "chasers": chasers,
        "rate": rate,
        "zones": s.zone_chase.free,
        "rates": RATE_LABELS,
        "step_beats": STEP_BEATS,
        "step_labels": STEP_LABELS,
        "max_chasers": MAX_CHASERS,
        "max_steps": MAX_STEPS,
    })
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
struct PartModifiersRequest {
    id: String,
    part: u8,
    #[serde(default)]
    modifiers: Option<crate::live::LiveModifiers>,
}

#[derive(Deserialize)]
struct ProgramRequest {
    id: String,
    program: Option<crate::program::CueProgram>,
}

/// `POST /api/abstracts/preview` and `/store` (T-440): the editor's look.
#[derive(Deserialize)]
struct AbstractRequest {
    look: Settings,
    #[serde(default)]
    name: Option<String>,
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

/// `POST /api/standby[/pair|/takeover|/receive]` (T-456): from this Mac's
/// interface only. Nothing here arms: a takeover leaves the laser disarmed.
fn standby_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, path: &str) -> HttpResponse {
    if !request.remote_addr().is_some_and(|a| a.ip().is_loopback()) {
        return text(403, "la machine de secours ne se règle que depuis l'interface de ce Mac");
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PairBody {
        code: String,
    }
    let done = match path {
        "/api/standby" => match body::<crate::standby::SettingsRequest>(request) {
            Ok(req) => crate::standby::set_settings(shared, req),
            Err(e) => return e,
        },
        "/api/standby/pair" => match body::<PairBody>(request) {
            Ok(req) => crate::standby::pair(shared, &req.code),
            Err(e) => return e,
        },
        "/api/standby/takeover" => crate::standby::takeover(shared),
        "/api/standby/receive" => crate::standby::receive_project(shared).map(|_| ()),
        _ => return text(404, "not found"),
    };
    match done {
        Ok(()) => json_response(crate::standby::view(&shared.lock().unwrap(), Instant::now())),
        Err((code, msg)) => text(code, &msg),
    }
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

/// `POST /api/outputs/fixed_rate` (T-463): `fps` absent or null = off.
#[derive(Deserialize)]
struct FixedRateBody {
    #[serde(default)]
    fps: Option<f32>,
}

/// The fixed rate part of `scan_limits_view` (T-463): the choices, the
/// points of one frame, what the engine measured (the main output's) and
/// the warning when the content does not fit.
fn fixed_rate_view(s: &Shared, id: &str, scan: &crate::power::OutputScan) -> serde_json::Value {
    let points = scan.fixed_fps.map(|fps| crate::fixed_rate::target_points(s.pps, fps));
    let status = if id == crate::power::MAIN_OUTPUT { s.fixed_rate } else { None };
    let warning = status.filter(|st| st.report.over_step).map(|st| {
        format!("Trop de points pour la cadence fixe : réduisez le budget de points à {} ou baissez la cadence.", st.report.output)
    });
    json!({
        "fps": scan.fixed_fps,
        "choices": crate::fixed_rate::FIXED_FPS_CHOICES,
        "range": crate::fixed_rate::FIXED_FPS_RANGE,
        "points": points,
        "status": status,
        "warning": warning,
    })
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
        "fixed": fixed_rate_view(s, id, &cfg.scan),
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
    let mut next = venue.sanitized();
    // The viewpoint (T-278) only changes through `/api/venue/camera`: a
    // venue edit or an undo never moves the camera back.
    next.camera = s.venue.get().camera.clone();
    // Nor the previews' galvo simulation (T-280): `/api/venue/sim` only.
    let cur = s.venue.get().render;
    (next.render.galvo, next.render.divergence_mrad) = (cur.galvo, cur.divergence_mrad);
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
    json!({ "show_mode": s.show_mode.on, "safety": s.safety_lock.view(Instant::now()) })
}

/// `POST /api/safety/lock`. The code never reaches a log.
#[derive(Deserialize)]
struct SafetyLockRequest {
    action: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    relock_min: Option<u32>,
    #[serde(default)]
    source: Option<String>,
}

/// `POST /api/safety/unlock` `{code, source?}`.
#[derive(Deserialize)]
struct SafetyUnlockRequest {
    code: String,
    #[serde(default)]
    source: Option<String>,
}

/// `{action: "lock"}` locks now (from anyone: it only tightens).
/// From the UI, keyboard or local API only, with the lock open and not
/// in show mode: `{action: "set", code, relock_min?}` sets or changes the
/// code (4 to 8 digits) and locks; `{action: "remove"}` removes it;
/// `{action: "relock", relock_min}` changes the delay. Logged (T-259).
fn safety_lock_route(shared: &Arc<Mutex<Shared>>, req: SafetyLockRequest) -> HttpResponse {
    let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
    let now = Instant::now();
    let reply = |s: &Shared| json_response(s.safety_lock.view(Instant::now()));
    if req.action == "lock" {
        let mut s = shared.lock().unwrap();
        let was = crate::safetylock::check(&mut s, now);
        return match s.safety_lock.lock_now() {
            Ok(()) => {
                if !was {
                    log::info!("sécurité verrouillée");
                    s.gate.log().record("safety_lock", Some(src.id()), json!({ "event": "locked" }));
                }
                reply(&s)
            }
            Err(e) => text(409, &format!("{e:#}")),
        };
    }
    if !crate::profiles::operator_source(src) {
        return text(403, "le verrou de sécurité se règle depuis l'interface seulement");
    }
    {
        let mut s = shared.lock().unwrap();
        if s.show_mode.on {
            return locked();
        }
        if crate::safetylock::check(&mut s, now) {
            return safety_locked();
        }
    }
    match req.action.as_str() {
        "set" => {
            let code = req.code.unwrap_or_default();
            if !crate::safetylock::valid_code(&code) {
                return text(400, "Le code doit compter 4 à 8 chiffres");
            }
            // Slow hash, studio lock released.
            let relock = req.relock_min.unwrap_or(crate::safetylock::DEFAULT_RELOCK_MIN);
            let file = match crate::safetylock::new_file(&code, relock) {
                Ok(f) => f,
                Err(e) => return text(500, &format!("{e:#}")),
            };
            let mut s = shared.lock().unwrap();
            let changed = s.safety_lock.is_set();
            match s.safety_lock.set_code(file, Instant::now()) {
                Ok(()) => {
                    log::info!("code de sécurité {} ; sécurité verrouillée", if changed { "changé" } else { "défini" });
                    s.gate.log().record("safety_lock", Some(src.id()), json!({ "event": if changed { "code_changed" } else { "code_set" } }));
                    reply(&s)
                }
                Err(e) => text(409, &format!("{e:#}")),
            }
        }
        "remove" => {
            let mut s = shared.lock().unwrap();
            match s.safety_lock.remove(now) {
                Ok(()) => {
                    log::info!("code de sécurité retiré");
                    s.gate.log().record("safety_lock", Some(src.id()), json!({ "event": "code_removed" }));
                    reply(&s)
                }
                Err(e) => text(409, &format!("{e:#}")),
            }
        }
        "relock" => {
            let mut s = shared.lock().unwrap();
            match s.safety_lock.set_relock_min(req.relock_min.unwrap_or(crate::safetylock::DEFAULT_RELOCK_MIN), now) {
                Ok(()) => reply(&s),
                Err(e) => text(409, &format!("{e:#}")),
            }
        }
        _ => text(400, "expected \"action\": \"lock\", \"set\", \"remove\" or \"relock\""),
    }
}

/// Unlocks with the code, from the UI, keyboard or local API only (MIDI,
/// OSC, DMX and the remote never reach this; a declared source of theirs
/// is 403). Wrong: 403 `{error, attempts_left}`; the fifth wrong code in a
/// row, and every attempt for 60 s after: 429 `{error, wait_s}`. Logged
/// without the code.
fn safety_unlock_route(shared: &Arc<Mutex<Shared>>, req: SafetyUnlockRequest) -> HttpResponse {
    use crate::safetylock::UnlockError;
    let src = ArmSource::parse(req.source.as_deref().unwrap_or("ui"));
    if !crate::profiles::operator_source(src) {
        return text(403, "le verrou de sécurité s'ouvre depuis l'interface seulement");
    }
    let attempt = {
        let mut s = shared.lock().unwrap();
        if !crate::safetylock::check(&mut s, Instant::now()) {
            return json_response(s.safety_lock.view(Instant::now()));
        }
        s.safety_lock.begin_unlock(Instant::now())
    };
    // Slow hash, studio lock released.
    let result = attempt.and_then(|a| {
        let hash = crate::safetylock::hash_code(&req.code, &a);
        shared.lock().unwrap().safety_lock.finish_unlock(&hash, Instant::now())
    });
    let s = shared.lock().unwrap();
    let mut view = s.safety_lock.view(Instant::now());
    match result {
        Ok(()) => {
            log::info!("sécurité déverrouillée");
            s.gate.log().record("safety_lock", Some(src.id()), json!({ "event": "unlocked" }));
            json_response(view)
        }
        Err(e) => {
            view["error"] = json!(e.message_fr());
            let status = match e {
                UnlockError::Wrong(left) => {
                    log::warn!("code de sécurité incorrect");
                    s.gate.log().record("safety_lock", Some(src.id()), json!({ "event": "failed", "attempts_left": left }));
                    view["attempts_left"] = json!(left);
                    if left == 0 {
                        429
                    } else {
                        403
                    }
                }
                UnlockError::Wait(_) => 429,
                UnlockError::NotSet | UnlockError::Damaged => 409,
            };
            with_status(json_response(view), status)
        }
    }
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

/// `POST /api/operators/current`, `/api/operators/delete` (T-426).
#[derive(serde::Deserialize)]
struct OperatorRequest {
    #[serde(default)]
    name: String,
}

/// The operator named by `/api/operators/<nom>` (percent-decoded), if valid.
fn operator_name(path: &str) -> Option<String> {
    let seg = path.strip_prefix("/api/operators/")?;
    crate::operators::decode_segment(seg).filter(|n| crate::operators::valid_name(n))
}

/// `POST /api/operators/<nom>`: replaces that operator's profile (views
/// and interface preferences). The URL's name wins over the body's.
fn save_operator(request: &mut Request, shared: &Arc<Mutex<Shared>>, path: &str) -> HttpResponse {
    use std::io::Read;
    let Some(name) = operator_name(path) else { return text(400, "nom d'opérateur invalide") };
    let mut raw = Vec::new();
    if let Err(e) = request.as_reader().take(crate::operators::MAX_BODY_BYTES as u64 + 1).read_to_end(&mut raw) {
        return text(400, &format!("unreadable body: {e}"));
    }
    if raw.len() > crate::operators::MAX_BODY_BYTES {
        return text(413, "profil d'opérateur trop grand");
    }
    let mut prefs: crate::operators::OperatorPrefs = match serde_json::from_slice(&raw) {
        Ok(p) => p,
        Err(e) => return text(400, &format!("invalid JSON: {e}")),
    };
    prefs.name = name;
    let data_dir = shared.lock().unwrap().project.data_dir.clone();
    match crate::operators::save(&data_dir, &prefs) {
        Ok(()) => json_response(json!({ "saved": prefs.name, "views": prefs.views.len() })),
        Err((code, msg)) => text(code, &msg),
    }
}

/// `POST /api/sites`, `/api/sites/use-project` (T-289).
#[derive(serde::Deserialize)]
struct SiteRequest {
    #[serde(default)]
    action: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    confirm_loosen: bool,
    source: Option<String>,
}

/// The reply to a refused site action: as a safety profile's.
fn site_error(e: crate::profiles::LoadError) -> HttpResponse {
    match e {
        crate::profiles::LoadError::Unknown(m) => text(404, &m),
        crate::profiles::LoadError::Failed(m) => text(500, &format!("Profil de site appliqué en partie : {m}. Laser désarmé.")),
        crate::profiles::LoadError::Loosens(list) => with_status(json_response(json!({ "error": "Ce profil de site assouplit la sécurité : confirmation requise", "loosen": list })), 409),
        other => profile_error(other),
    }
}

fn body_says_dry_run(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw).ok().and_then(|v| v.get("dry_run").and_then(|d| d.as_bool())).unwrap_or(false)
}

/// `GET /api/pack/export[?site=1]` (T-289): the open project as
/// *Enregistrer* would write it, the media it names and, with `site=1`,
/// the site profile in force, as a `.lspack` download. The lock is held
/// only to copy the state; media are read and zipped after.
fn export_pack(url: &str, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let project = crate::project::current_project(shared);
    let (site, ilda_dir, audio_dir) = {
        let s = shared.lock().unwrap();
        let with_site = query_u64(url, "site").unwrap_or(0) == 1;
        let name = s.sites.active().unwrap_or(crate::sites::DEFAULT_SITE).to_string();
        (with_site.then(|| crate::sites::capture(&s, &name)), s.ilda.dir().to_path_buf(), s.media.dir().to_path_buf())
    };
    match crate::lspack::build(&project, site.as_ref(), &ilda_dir, &audio_dir) {
        Ok((bytes, manifest)) => {
            let ascii: String = project.name.chars().map(|c| if c.is_ascii_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
            let disposition = format!("attachment; filename=\"{ascii}.{}\"", crate::lspack::EXTENSION);
            let header = Header::from_bytes(&b"Content-Disposition"[..], disposition.as_bytes()).expect("ASCII file name");
            let missing = Header::from_bytes(&b"X-Pack-Missing"[..], manifest.missing.len().to_string().as_bytes()).expect("ASCII");
            with_type(Response::from_data(bytes), "application/zip").with_header(header).with_header(missing)
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
}

/// `POST /api/pack/import[?name=<projet>]`, the package's bytes as the
/// body (T-289): checked whole (lspack.rs), then its media go into the
/// libraries and its project becomes a new project of the folder (not
/// opened). Runs beside the HTTP worker, lock released. Never arms, never
/// changes a safety setting: an embedded site is offered at opening.
fn import_pack(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::lspack::MAX_PACK_BYTES;
    use std::io::Read;
    let too_big = || text(413, &format!("paquet trop gros ({} Mo au plus)", MAX_PACK_BYTES >> 20));
    if request.body_length().is_some_and(|n| n as u64 > MAX_PACK_BYTES) {
        return too_big();
    }
    let wanted = query_str(request.url(), "name");
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_PACK_BYTES + 1).read_to_end(&mut bytes) {
        return text(400, &format!("unreadable body: {e}"));
    }
    if bytes.len() as u64 > MAX_PACK_BYTES {
        return too_big();
    }
    let unpacked = match crate::lspack::unpack(&bytes) {
        Ok(u) => u,
        Err(e) => return text(400, &format!("{e:#}")),
    };
    drop(bytes);
    let (projects_dir, ilda, audio) = {
        let s = shared.lock().unwrap();
        (s.project.dir.clone(), Arc::clone(&s.ilda), Arc::clone(&s.media))
    };
    let targets = crate::lspack::Targets { projects_dir: &projects_dir, ilda: &ilda, audio: &audio };
    match crate::lspack::install(unpacked, wanted.as_deref(), &targets) {
        Ok(done) => {
            {
                let mut s = shared.lock().unwrap();
                crate::ilda_library::refresh(&mut s);
                log::info!("Paquet importé : projet « {} », {} média(s)", done.project, done.media.len());
                s.gate.log().record("pack_import", Some("ui"), json!({ "project": done.project, "media": done.media.len(), "site": done.site, "armed": s.gate.is_armed() }));
            }
            json_response(json!(done))
        }
        Err(e) => text(400, &format!("{e:#}")),
    }
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

/// 423 Locked: a loosening refused under the safety lock (T-261).
fn safety_locked() -> HttpResponse {
    text(423, crate::safetylock::LOCKED_FR)
}

/// 423 with what the change would have loosened.
fn safety_locked_list(list: &[String]) -> HttpResponse {
    with_status(json_response(json!({ "error": crate::safetylock::LOCKED_FR, "loosen": list, "safety_locked": true })), 423)
}

/// Whether the safety lock is on now (T-261), for the handlers that let
/// tightening through.
fn safety_lock_on(s: &mut Shared) -> bool {
    crate::safetylock::check(s, Instant::now())
}

#[derive(Deserialize)]
struct PsnRequest {
    #[serde(flatten)]
    settings: crate::psn::PsnSettings,
    #[serde(default)]
    confirm_loosen: bool,
}

#[derive(Deserialize)]
struct SimPsnTracker {
    id: u16,
    pos: [f32; 3],
    #[serde(default)]
    validity: Option<f32>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct SimPsn {
    trackers: Vec<SimPsnTracker>,
}

#[derive(Deserialize)]
struct TargetsRequest {
    items: Vec<crate::targets::Target>,
    #[serde(default)]
    confirm_loosen: bool,
}

/// `GET /api/targets` and the answer to a change (T-109): the list, each
/// target's output point and status, and the static-beam guard's cells
/// that dim one of them on the last frame.
fn targets_view(s: &Shared) -> serde_json::Value {
    let cfg = s.effective_safety();
    let g = s.dwell.grid.max(1) as f32;
    let status: Vec<serde_json::Value> = s
        .targets
        .get()
        .items
        .iter()
        .map(|t| {
            let st = crate::targets::status(t, &cfg, &s.calibration);
            let cell = |v: f32| (((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * g) as usize).min(g as usize - 1) as f32;
            let (cx, cy) = (cell(st.out[0]), cell(st.out[1]));
            let guard = s.dwell.cells.iter().find(|c| c[0] == cx && c[1] == cy).map(|c| c[2]);
            json!({ "out": st.out, "level": st.level, "reasons": st.reasons, "guard": guard })
        })
        .collect();
    // The mobile targets (T-457): after the fixed ones in the look's mask.
    let mobile: Vec<serde_json::Value> = s
        .psn
        .settings()
        .links
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let st = s.psn.statuses().get(i);
            json!({
                "name": l.target,
                "bit": s.targets.get().items.len() + i,
                "state": st.map_or("désactivé", |st| st.state.as_str()),
                "lit": st.is_some_and(|st| st.lit),
                "out": st.and_then(|st| st.out),
            })
        })
        .collect();
    json!({
        "items": s.targets.get().items,
        "mobile": mobile,
        "status": status,
        "max": crate::targets::MAX_TARGETS,
        "horizon_y": cfg.horizon.y,
        "dwell_profile": cfg.dwell.profile,
    })
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

/// A refused input setting: 409 for the DMX output's self-drive trap
/// (T-453), 400 otherwise.
fn netin_refusal(e: &str) -> u16 {
    if e == crate::dmx_out::SELF_DRIVE {
        409
    } else {
        400
    }
}

/// `POST /api/citp` `{ "enabled": true, "interface": "…", "server_name":
/// "…", "thumb_max_px": 128 }` (T-454): the fields given; 400 when
/// invalid. Saved to `citp.json` outside the lock.
fn citp_route(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    match body::<serde_json::Value>(request) {
        Ok(patch) => {
            let (result, job) = {
                let mut s = shared.lock().unwrap();
                let r = crate::citp::update(&mut s, &patch);
                (r, s.citp.save_job())
            };
            match result {
                Ok(changed) => {
                    if let (true, Some((path, cfg))) = (changed, job) {
                        crate::save_json(&path, &cfg);
                    }
                    json_response(crate::citp::view(&shared.lock().unwrap()))
                }
                Err(e) => text(400, &e),
            }
        }
        Err(e) => e,
    }
}

/// The DMX output (T-453):
/// - `GET /api/dmx-out` (in `dispatch`): settings, status, values, interfaces.
/// - `POST /api/dmx-out` `{ "enabled": true, "channels": [...], … }`: the
///   settings given (Safety class: never from the tablet remote); 409 on
///   the self-drive trap or the laser's own address.
/// - `POST /api/dmx-out/haze` `{ "on": bool }` and / or `{ "fan": 0..1 }`:
///   the *Brouillard* button (held) and the fan slider.
fn dmx_out_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, path: &str) -> HttpResponse {
    match path {
        "/api/dmx-out" => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let (result, job) = {
                    let mut s = shared.lock().unwrap();
                    let r = crate::dmx_out::update(&mut s, &patch);
                    (r, s.dmx_out.save_job())
                };
                match result {
                    Ok(changed) => {
                        if let (true, Some((path, cfg))) = (changed, job) {
                            crate::save_json(&path, &cfg);
                        }
                        json_response(crate::dmx_out::view(&shared.lock().unwrap()))
                    }
                    Err(e) => text(e.status, &e.message),
                }
            }
            Err(e) => e,
        },
        "/api/dmx-out/haze" => match body::<serde_json::Value>(request) {
            Ok(req) => {
                let (on, fan) = (req.get("on").map(|v| v.as_bool()), req.get("fan").map(|v| v.as_f64()));
                if on.is_none() && fan.is_none() {
                    return text(400, "attendu : \"on\" et / ou \"fan\"");
                }
                let mut s = shared.lock().unwrap();
                if let Some(fan) = fan {
                    let Some(v) = fan else { return text(400, "valeur du ventilateur invalide") };
                    if let Err(e) = crate::dmx_out::set_fan(&mut s, v as f32) {
                        return text(400, e);
                    }
                }
                if let Some(on) = on {
                    let Some(on) = on else { return text(400, "\"on\" : true ou false") };
                    crate::dmx_out::set_haze(&mut s, on);
                }
                json_response(crate::dmx_out::summary(&s))
            }
            Err(e) => e,
        },
        _ => text(405, "méthode non permise"),
    }
}

/// Incoming timecode (T-168):
/// - `GET /api/timecode`: settings, TC-IN, status, readout, MIDI ports
///   and audio input.
/// - `POST /api/timecode` `{ "source": "mtc", "offset_s": 3600, … }`:
///   the settings given (an edit, locked in show mode).
/// - `POST /api/timecode/arm` `{ "on": bool }` or `{ "toggle": true }`:
///   TC-IN, from this Mac's interface only (like the external gate).
///   Never arms the laser.
/// - `GET/POST /api/timecode/out` (T-417): the timecode output's settings
///   and status. Safety class: never from the tablet remote.
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
                    Err(e) => text(e.status, &e.message),
                }
            }
            Err(e) => e,
        },
        (Method::Get, "/api/timecode/out") => json_response(crate::timecode::out::view(&shared.lock().unwrap())),
        (Method::Post, "/api/timecode/out") => match body::<serde_json::Value>(request) {
            Ok(patch) => {
                let (result, job) = {
                    let mut s = shared.lock().unwrap();
                    let r = crate::timecode::out::update(&mut s, &patch);
                    (r, s.timecode.save_job())
                };
                match result {
                    Ok(changed) => {
                        if let (true, Some((path, cfg))) = (changed, job) {
                            crate::save_json(&path, &cfg);
                        }
                        json_response(crate::timecode::out::view(&shared.lock().unwrap()))
                    }
                    Err(e) => text(e.status, &e.message),
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

/// The outdoor mode at a glance (T-262), in `/api/state`, every frame and
/// the « Ciel coupé » reply.
fn outdoor_view(s: &Shared) -> serde_json::Value {
    let mut o = s.safety.get().outdoor;
    o.sky_off = s.sky_off;
    // What is cut on the main projector (output space), for the preview.
    let cut: Vec<Vec<[f32; 2]>> = o.blank_zones().into_iter().map(|z| z.points).collect();
    // The venue's other projectors, each from its own mounting (T-477),
    // for the 3D view; only while something is cut.
    let mut cuts = serde_json::Map::new();
    if o.active() {
        for p in s.venue.get().extras() {
            o.projector = p.id.clone();
            let c: Vec<Vec<[f32; 2]>> = o.blank_zones().into_iter().map(|z| z.points).collect();
            cuts.insert(p.id.clone(), json!(c));
        }
    }
    json!({ "enabled": o.enabled, "sky_off": s.sky_off, "authorized": o.authorized(), "sectors": o.sectors.len(), "cut": cut, "cuts": cuts })
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
    // Battery and thermal state of this Mac (T-495): a snapshot.
    v["power_watch"] = json!(s.power_watch.status());
    // The show player (T-444).
    v["showlist"] = json!(crate::showlist::summary(s));
    // The backup machine (T-456).
    v["standby"] = crate::standby::summary(s, Instant::now());
    // The agenda (T-455): the next slot and the last runs.
    v["agenda"] = crate::agenda::state(s);
    // The DMX output (T-453): values by channel, haze panel, sending.
    v["dmx_out"] = crate::dmx_out::summary(s);
    // CITP (T-454): listening, consoles connected, counters.
    v["citp"] = crate::citp::summary(s);
    // Pilote automatique (T-148): the switch, its pick, why it waits.
    v["autopilot"] = crate::autopilot::status(s);
    // The live take (T-169).
    v["take"] = s.take.view();
    // T-450: free space and the copy to a second location.
    v["storage"] = json!(s.project.backup.status());
    // T-447: the zone trace, its seconds left and its zone.
    v["zone_trace"] = crate::zonetrace::view(s, Instant::now());
    // T-460: the zone alignment proposed from a phone, or null.
    v["align"] = crate::remote::align_view(s);
    v["align_note"] = crate::remote::align_note(s);
    // T-260: the active safety profile, for the status bar.
    v["safety_profile"] = crate::profiles::summary(s);
    // T-262: the outdoor mode and « Ciel coupé ».
    v["outdoor"] = outdoor_view(s);
    // T-463: the main output's fixed frame rate, measured; null when off.
    v["fixed_rate"] = json!(s.fixed_rate);
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
    // The timecode output (T-417): on, sending, the last time sent.
    tl["timecode_out"] = crate::timecode::out::summary(s);
    // The live take (T-169): its button and counters.
    tl["take"] = s.take.view();
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
/// `POST /api/showlist/play`: from item `index` (none: the current one).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ShowListPlay {
    index: Option<usize>,
}

/// `POST /api/showlist/loop`.
#[derive(serde::Deserialize)]
struct ShowListLoop {
    on: bool,
}

/// `POST /api/macros`.
#[derive(serde::Deserialize)]
struct MacroListRequest {
    macros: Vec<crate::macros::Macro>,
}

/// `POST /api/scripts` (T-458).
#[derive(serde::Deserialize)]
struct ScriptListRequest {
    scripts: Vec<crate::script::Script>,
}

/// `POST /api/scripts/run` / `stop` (no id: resume / stop every script).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ScriptIdRequest {
    id: Option<u32>,
    source: Option<String>,
}

/// `POST /api/scripts/check`.
#[derive(serde::Deserialize)]
struct ScriptCheckRequest {
    source: String,
}

/// `POST /api/scripts/trust`.
#[derive(serde::Deserialize)]
struct ScriptTrustRequest {
    approve: bool,
}

/// `POST /api/macros/run` / `stop` (no id: every macro).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct MacroIdRequest {
    id: Option<String>,
    /// Who launched it (« En cours »): `keyboard`, else the page.
    source: Option<String>,
}

fn tempo_view(s: &Shared) -> serde_json::Value {
    let mut v = json!(s.tempo.state(s.now_s()));
    v["midi_clock"] = crate::midi::clock::summary(s);
    v["link"] = crate::ableton_link::summary(s);
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
        "layers": { "mixer": s.mixer, "mix": s.mix, "groups": groups_view(s)["groups"], "chase": chase_view(s) },
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
    // Battery and thermal state of this Mac (T-495): a snapshot.
    v["power_watch"] = json!(s.power_watch.status());
    // The show player (T-444): the header's times, every frame.
    v["showlist"] = json!(crate::showlist::summary(s));
    // The backup machine (T-456): the top bar's pill or banner.
    v["standby"] = crate::standby::summary(s, Instant::now());
    // The agenda (T-455): the top bar's « Agenda actif — prochain 20:30 ».
    v["agenda"] = crate::agenda::summary(s);
    // The DMX output (T-453): values, haze panel, sending.
    v["dmx_out"] = crate::dmx_out::summary(s);
    // CITP (T-454): listening, consoles connected, counters (RÉGLAGES).
    v["citp"] = crate::citp::summary(s);
    // Pilote automatique (T-148): the switch, its pick, why it waits.
    v["autopilot"] = crate::autopilot::status(s);
    // T-448: 0 normal, 1 loaded, 2 late frames (the points indicator).
    v["perf_level"] = json!(s.perf.level());
    // T-463: the main output's fixed frame rate, measured; null when off.
    v["fixed_rate"] = json!(s.fixed_rate);
    // T-285: undo / redo counts and labels; `rev` moves on every change.
    v["history"] = s.history.summary();
    // T-447: the zone trace, its seconds left and its zone.
    v["zone_trace"] = crate::zonetrace::view(s, Instant::now());
    // T-460: the zone alignment proposed from a phone, or null.
    v["align"] = crate::remote::align_view(s);
    v["align_note"] = crate::remote::align_note(s);
    // T-260: the active safety profile, for the status bar.
    v["safety_profile"] = crate::profiles::summary(s);
    // T-262: the outdoor mode and « Ciel coupé ».
    v["outdoor"] = outdoor_view(s);
    // T-418: the executor page shown, its faders, and the pages with
    // something active.
    v["executors"] = crate::executors::view(s);
    // T-427: the desk's layout revision, its bound controls' values and
    // its missing bindings.
    v["desk"] = crate::desk::view(s);
    // Macros (T-425): the list's revision and what runs (step, Go).
    v["macros"] = crate::macros::live_view(s);
    // Scripts (T-458): the list's revision, what runs, the banner.
    v["scripts"] = crate::script::live_view(s);
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
    let raw = match read_body(request) {
        Ok(raw) => raw,
        Err(e) => return e,
    };
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
        "wifi_device": s.wifi.device(),
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
    let raw = read_body(request)?;
    serde_json::from_str(&raw).map_err(|e| text(400, &format!("invalid JSON: {e}")))
}

/// The largest JSON body a route reads (T-449). Well above the biggest
/// legitimate one (a timeline at its event, track and envelope limits);
/// the file imports keep their own limits. Beyond it: 413, and a body
/// announced bigger is refused before a byte of it is read.
pub(crate) const MAX_JSON_BODY: usize = 16 * 1024 * 1024;

/// A request's text body, at most `MAX_JSON_BODY` bytes (T-449: a huge
/// or endless body is never read whole into memory).
fn read_body(request: &mut Request) -> Result<String, HttpResponse> {
    use std::io::Read;
    let too_big = || text(413, &format!("requête trop grosse ({} Mo au plus)", MAX_JSON_BODY >> 20));
    if request.body_length().is_some_and(|n| n > MAX_JSON_BODY) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    request.as_reader().take(MAX_JSON_BODY as u64 + 1).read_to_end(&mut bytes).map_err(|e| text(400, &format!("unreadable body: {e}")))?;
    if bytes.len() > MAX_JSON_BODY {
        return Err(too_big());
    }
    String::from_utf8(bytes).map_err(|e| text(400, &format!("unreadable body: {e}")))
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

/// `POST /api/figures/import?name=…&simplify=…`: an SVG, DXF, PNG or JPEG
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
        by_layer: q("layers").is_some_and(|v| v == "1"),
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

/// An object of the library as the page lists it.
fn object_summary(o: &crate::wire::WireObject) -> serde_json::Value {
    json!({
        "name": o.name,
        "id": crate::wire::cue_id(&o.name),
        "vertices": o.vertices.len(),
        "edges": o.edges.len(),
        "strokes": o.strokes(),
        "est_points": o.estimated_points(),
        "color": o.color,
        "period_beats": o.period_beats,
        "per": o.per,
        "axis_ratio": o.axis_ratio,
        "perspective": o.perspective,
        "depth_color": o.depth_color,
    })
}

/// `POST /api/objects` (T-462): what « Propriétés de l'objet » changes.
#[derive(Deserialize)]
struct ObjectProps {
    name: String,
    #[serde(default)]
    rename: Option<String>,
    #[serde(default)]
    color: Option<[u8; 3]>,
    #[serde(default)]
    period_beats: Option<f32>,
    #[serde(default)]
    per: Option<crate::figures::RateUnit>,
    #[serde(default)]
    axis_ratio: Option<f32>,
    #[serde(default)]
    perspective: Option<f32>,
    #[serde(default)]
    depth_color: Option<bool>,
}

/// `POST /api/objects/import?name=…&coplanar=…&up=y|z&keep=…[&preview=1]`
/// (T-462): an OBJ file (body, at most `MAX_OBJ_BYTES`) → a wireframe
/// object. Parsed on its own thread without the lock held; a panic in the
/// reader is caught and reported. With `preview=1`: `{object, report,
/// budget}`, nothing saved; otherwise saved in the library under `name`
/// (refused if that name is taken), its cue on the « Objets 3D » page.
fn import_object(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    use crate::figure_import::obj::{self, ObjOptions, UpAxis};
    use std::io::Read;
    let limit = obj::MAX_OBJ_BYTES;
    let mut bytes = Vec::new();
    if let Err(e) = request.as_reader().take(limit as u64 + 1).read_to_end(&mut bytes) {
        return text(400, &format!("Fichier OBJ illisible : {e}"));
    }
    if bytes.len() > limit {
        return text(413, &format!("Fichier OBJ trop gros : {} Mo au plus. Exportez l'objet avec moins de détails.", limit / (1024 * 1024)));
    }
    let url = request.url().to_string();
    let q = |k: &str| query_str(&url, k);
    let num = |k: &str| q(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
    let d = ObjOptions::default();
    let opts = ObjOptions {
        merge_coplanar_deg: num("coplanar").map_or(d.merge_coplanar_deg, |v| v.clamp(0.0, obj::MAX_COPLANAR_DEG)),
        up: q("up").and_then(|u| UpAxis::parse(&u)).unwrap_or(d.up),
        keep_longest: num("keep").filter(|v| *v >= 1.0).map(|v| v.min(1e9) as usize),
    };
    let preview = q("preview").is_some_and(|v| v == "1");
    let name = crate::figure_import::default_name(&q("name").unwrap_or_default());
    // Its own thread, so a file can't take the HTTP worker down; a panic
    // is caught as well.
    let job = std::thread::Builder::new()
        .name("object-import".into())
        .spawn(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| obj::parse_obj_bytes(&bytes, &opts))));
    let (mut object, report) = match job.map(|h| h.join()) {
        Ok(Ok(Ok(Ok(result)))) => result,
        Ok(Ok(Ok(Err(e)))) => return text(400, &format!("{e:#}")),
        _ => {
            log::warn!("object import failed internally on {name:?}");
            return text(400, "Fichier OBJ impossible à importer (erreur interne).");
        }
    };
    object.name = name;
    let mut s = shared.lock().unwrap();
    let budget = s.mixer.point_budget;
    if preview {
        return json_response(json!({ "object": object, "report": report, "budget": budget }));
    }
    if s.objects.get(&object.name).is_some() {
        return text(409, &format!("un objet s'appelle déjà « {} » : choisissez un autre nom", object.name));
    }
    match s.objects.save(object) {
        Ok(kept) => {
            crate::wire::refresh(&mut s);
            json_response(json!({ "name": kept.name, "id": crate::wire::cue_id(&kept.name), "report": report, "budget": budget }))
        }
        Err(e) => text(400, &format!("{e:#}")),
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

/// `GET|POST /api/safety/sheet` (T-263): the sheet, with a policy that
/// forbids any script, external resource or form in it. The safety log is
/// read without the studio lock.
fn safety_sheet(req: &crate::safetysheet::SheetRequest, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let (mut data, log) = {
        let s = shared.lock().unwrap();
        (crate::safetysheet::gather(&s, std::time::SystemTime::now()), s.gate.log().clone())
    };
    if req.include_log {
        data.log = Some(match log.dir().map(Path::to_path_buf) {
            Some(dir) => {
                log.flush(Duration::from_millis(200));
                crate::safety_log::read_day(&dir, &crate::safety_log::today().to_string())
            }
            None => Vec::new(),
        });
    }
    let html = crate::safetysheet::render(&data, req);
    let csp = Header::from_bytes(&b"Content-Security-Policy"[..], crate::safetysheet::CSP.as_bytes()).expect("ASCII header");
    let cache = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).expect("ASCII header");
    with_type(Response::from_string(html), "text/html; charset=utf-8").with_header(csp).with_header(cache)
}

/// The pre-show checklist (T-258). `operator {name}`, `check {id, on,
/// value?}`, `complete`, `restart`, `config {items, outdoor,
/// audience_scan}` or `config {defaults: true}`. Each answers the
/// checklist's view (400 with the French reason when refused; `complete`
/// answers 409 `{error, missing}`). Completing only lets the operator
/// arm: nothing here ever arms. Restarting or editing the list closes
/// the `checklist` interlock again, which disarms if armed.
fn checklist_route(request: &mut Request, shared: &Arc<Mutex<Shared>>, action: &str) -> HttpResponse {
    let raw = match read_body(request) {
        Ok(raw) => raw,
        Err(e) => return e,
    };
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

/// `POST /api/test/clock` (`--test-hooks`): the agenda's simulated time.
#[derive(Deserialize)]
struct SimClock {
    at: String,
    #[serde(default)]
    tz: Option<String>,
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
            health: Default::default(),
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
    /// *Son du Mac* (T-241) offered (`true`: a build with the feature on
    /// a recent macOS) or not, for the e2e of the source list.
    system_audio: Option<bool>,
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
    /// The signal's verdict (T-242), as the analysis thread's `HealthMeter`
    /// would give it.
    #[serde(default)]
    health: crate::audio::health::InputHealth,
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
                    health: sim.health,
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
        if let Some(on) = self.system_audio {
            hub.set_system_support(crate::audio::capture::SystemAudioSupport::from_parts(on, Some("15.0")));
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
            let server = bind("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let running = Arc::new(AtomicBool::new(true));
            let thread = std::thread::spawn({
                let (shared, running) = (Arc::clone(&shared), Arc::clone(&running));
                let calibration = crate::test_support::scratch_dir().join("calibration.json");
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
    fn the_camera_viewpoint_is_saved_alone_and_never_an_edit() {
        let t = TestServer::start(false);
        let undo = |t: &TestServer| js(&t.request("GET", "/api/state", "").1)["history"]["undo"].as_u64().unwrap();
        let n = undo(&t);
        t.shared.lock().unwrap().show_mode.on = true;
        let (code, body) = t.request("POST", "/api/venue/camera", r#"{"preset":"front","pos_m":[0,1.7,9],"target_m":[0,2,15],"fov_deg":50}"#);
        assert_eq!(code, 200, "a viewpoint is a view: allowed in show mode");
        assert_eq!(js(&body)["camera"]["preset"], "front");
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(undo(&t), n, "never in the undo history");
        let v = js(&t.request("GET", "/api/venue", "").1);
        assert_eq!(v["venue"]["camera"]["preset"], "front");
        // A venue edit carrying an older camera keeps the saved one.
        let mut venue = v["venue"].clone();
        venue["camera"] = json!({ "preset": "top" });
        venue["room"]["width_m"] = json!(12.0);
        assert_eq!(t.request("POST", "/api/venue", &venue.to_string()).0, 200);
        let v = js(&t.request("GET", "/api/venue", "").1);
        assert_eq!((v["venue"]["camera"]["preset"].as_str(), v["venue"]["room"]["width_m"].as_f64()), (Some("front"), Some(12.0)));
        assert_eq!(t.request("POST", "/api/venue/camera", r#"{"preset":"nulle-part"}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/venue", "").1)["venue"]["camera"]["preset"], "", "unknown preset: the default view");
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-280: the previews' galvo simulation is a view setting: allowed in
    /// show mode, never undone, clamped, a bad number refused, kept by a
    /// venue edit, and the arm state and frame are untouched.
    #[test]
    fn the_galvo_simulation_is_a_view_setting() {
        let t = TestServer::start(false);
        let undo = |t: &TestServer| js(&t.request("GET", "/api/state", "").1)["history"]["undo"].as_u64().unwrap();
        let n = undo(&t);
        let v = js(&t.request("GET", "/api/venue", "").1);
        assert_eq!(v["venue"]["render"]["galvo"]["enabled"], false, "off by default");
        assert!((v["venue"]["render"]["galvo"]["response_ms"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(v["venue"]["render"]["divergence_mrad"].as_f64(), Some(1.0));
        t.shared.lock().unwrap().show_mode.on = true;
        let (code, body) = t.request("POST", "/api/venue/sim", r#"{"enabled":true,"response_ms":40,"divergence_mrad":2.5}"#);
        assert_eq!(code, 200, "a display setting: allowed in show mode");
        let r = js(&body)["render"].clone();
        assert_eq!((r["galvo"]["enabled"].as_bool(), r["galvo"]["response_ms"].as_f64(), r["divergence_mrad"].as_f64()), (Some(true), Some(5.0), Some(2.5)), "clamped");
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(undo(&t), n, "never in the undo history");
        for bad in [r#"{"response_ms":"vite"}"#, r#"{"response_ms":1e999}"#, r#"{"divergence_mrad":null,"x":1}"#, "pas du json"] {
            assert_eq!(t.request("POST", "/api/venue/sim", bad).0, 400, "{bad}");
        }
        assert_eq!(js(&t.request("GET", "/api/venue", "").1)["venue"]["render"]["galvo"]["response_ms"].as_f64(), Some(5.0), "a refused post changes nothing");
        // A venue edit carrying older settings keeps the saved ones.
        let mut venue = js(&t.request("GET", "/api/venue", "").1)["venue"].clone();
        venue["render"]["galvo"] = json!({ "enabled": false, "response_ms": 0.3 });
        venue["room"]["width_m"] = json!(12.0);
        assert_eq!(t.request("POST", "/api/venue", &venue.to_string()).0, 200);
        let v = js(&t.request("GET", "/api/venue", "").1);
        assert_eq!((v["venue"]["render"]["galvo"]["enabled"].as_bool(), v["venue"]["room"]["width_m"].as_f64()), (Some(true), Some(12.0)));
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

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

    /// T-421: spreads ride on the existing objects (no new route): a
    /// cell's, a modulator's (with width and ramps) and an event's, each
    /// sanitised; objects saved before load without them.
    #[test]
    fn spreads_over_the_existing_routes() {
        let t = TestServer::start(false);
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        let spread = json!({ "group": " Côtés ", "param": "delay", "from": -1, "to": 9, "wings": 40, "blocks": 0, "shuffle": 7 });
        assert_eq!(t.request("POST", "/api/cues/slot", &json!({ "id": cue, "zones": [1], "spread": spread }).to_string()).0, 200);
        let slot = t.shared.lock().unwrap().deck.slot(&cue);
        let sp = slot.spread.expect("kept");
        assert_eq!((sp.group.as_str(), sp.from, sp.to, sp.wings, sp.blocks, sp.shuffle), ("Côtés", 0.0, crate::spread::MAX_DELAY_S, 16, 1, Some(7)));
        let cues = js(&t.request("GET", "/api/cues", "").1);
        assert_eq!(cues["slots"][&cue]["spread"]["param"], "delay");
        // No group named: no spread kept.
        assert_eq!(t.request("POST", "/api/cues/slot", &json!({ "id": cue, "spread": { "group": "" } }).to_string()).0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(&cue).spread, None);
        let lfos = json!([{ "target": "master.size", "width": 0.25, "attack": 0.8, "decay": 0.8, "spread": { "group": "Tout", "param": "speed", "to": 360 } }]);
        assert_eq!(t.request("POST", "/api/lfos", &lfos.to_string()).0, 200);
        let back = js(&t.request("GET", "/api/lfos", "").1);
        let m = &back["lfos"][0];
        assert_eq!((m["width"].as_f64(), m["spread"]["param"].as_str(), m["spread"]["to"].as_f64()), (Some(0.25), Some("phase"), Some(360.0)));
        assert!((m["attack"].as_f64().unwrap() - 0.5).abs() < 1e-6, "attack + decay at most 1");
        let old = js(&t.request("POST", "/api/lfos", r#"[{"target":"master.size"}]"#).1.replace("ok", "null"));
        assert!(old.is_null());
        let m = &js(&t.request("GET", "/api/lfos", "").1)["lfos"][0];
        assert_eq!((m["width"].as_f64(), m["attack"].as_f64(), m.get("spread")), (Some(1.0), Some(0.0), None));
        // A timeline event's own spread, kept through a save.
        let show = json!({ "name": "étalé", "tracks": [{ "layer": 1, "events": [{ "id": 1, "start": 0, "len": 4, "source": { "kind": "cue", "id": cue }, "spread": { "group": "Tout", "param": "phase", "to": 1e9 } }] }] });
        let (code, body) = t.request("POST", "/api/shows", &show.to_string());
        assert_eq!(code, 200, "{body}");
        let saved = t.shared.lock().unwrap().shows.load("étalé").expect("saved");
        assert_eq!(saved.tracks[0].events[0].spread.as_ref().map(|s| s.to), Some(3600.0));
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
    /// T-425: macros over the API: saved (in the history and undone,
    /// refused in show mode, refused lines and cycles refused with the
    /// step), run, waited on Go, stopped and shown in « En cours » (all
    /// allowed in show mode), launched from an executor; never arming.
    #[test]
    fn macros_over_the_api() {
        let t = TestServer::start(false);
        let m = |name: &str, steps: serde_json::Value| json!({ "id": "", "name": name, "steps": steps });
        let list = json!({ "macros": [m("Montée", json!([{ "command": "master.size 50", "wait": "go" }, { "command": "master.size 90" }]))] });
        let (code, body) = t.request("POST", "/api/macros", &list.to_string());
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert_eq!(v["macros"][0]["id"], "m1");
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Macro « Montée » créée");
        // A refused line: 400 with the step.
        let bad = json!({ "macros": [m("X", json!([{ "command": "tempo 120" }, { "command": "armer" }]))] });
        let (code, body) = t.request("POST", "/api/macros", &bad.to_string());
        assert_eq!(code, 400);
        let e = js(&body);
        assert_eq!((e["step"].as_u64(), e["refused"].as_bool()), (Some(2), Some(true)), "{e}");
        assert!(e["message_fr"].as_str().unwrap().starts_with("Macro « X », étape 2 : Commande refusée"));
        // A cycle.
        let cycle = json!({ "macros": [
            { "id": "m1", "name": "A", "steps": [{ "command": "macro \"B\" go" }] },
            { "id": "m2", "name": "B", "steps": [{ "command": "macro.m1.go" }] },
        ] });
        let (code, body) = t.request("POST", "/api/macros", &cycle.to_string());
        assert_eq!(code, 400);
        assert!(js(&body)["message_fr"].as_str().unwrap().starts_with("Cycle refusé"), "{body}");
        // Show mode: the list is locked, playing is not.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/macros", &list.to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/macros/run", r#"{"id":"m1"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/macros/run", r#"{"id":"zz"}"#).0, 409);
        {
            let mut s = t.shared.lock().unwrap();
            let now = s.now_s();
            crate::macros::frame(&mut s, now, 0.0);
            assert!((s.live.size - 0.5).abs() < 1e-6);
        }
        let running = js(&t.request("GET", "/api/running", "").1);
        assert_eq!(running["macros"][0]["name"], "Montée");
        assert_eq!(running["macros"][0]["waiting"]["kind"], "go");
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["macros"]["running"][0]["go"], true);
        assert_eq!(js(&t.request("POST", "/api/macros/go", "{}").1)["advanced"], 1);
        {
            let mut s = t.shared.lock().unwrap();
            let now = s.now_s();
            crate::macros::frame(&mut s, now, 0.0);
            assert!((s.live.size - 0.9).abs() < 1e-6);
        }
        assert_eq!(t.request("POST", "/api/macros/run", r#"{"id":"m1"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/running/stop", r#"{"kind":"macro","id":"m1"}"#).0, 200);
        assert!(t.shared.lock().unwrap().macros.player.runs.is_empty());
        assert_eq!(t.request("POST", "/api/macros/run", r#"{"id":"m1"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/macros/stop", "{}").0, 200);
        assert!(t.shared.lock().unwrap().macros.player.runs.is_empty());
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        // An executor's Go launches it.
        let mut pages = js(&t.request("GET", "/api/executors", "").1)["pages"].clone();
        pages[0]["handles"][0] = json!({ "target": { "type": "macro", "id": "m1" }, "fader": "level", "buttons": ["go", "none"] });
        let (code, body) = t.request("POST", "/api/executors", &json!({ "pages": pages }).to_string());
        assert_eq!(code, 200, "{body}");
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"exec.1.1.button","value":1}"#).0, 200);
        assert!(t.shared.lock().unwrap().macros.player.is_running("m1"));
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["executors"]["active"][0], true);
        // The e-stop stops it.
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert!(t.shared.lock().unwrap().macros.player.runs.is_empty());
        assert_eq!(t.request("POST", "/api/macros/run", r#"{"id":"m1"}"#).0, 409);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
        // Undo takes the macro back.
        assert_eq!(t.request("POST", "/api/estop/reset", "").0, 200);
        // The executor first, then the macro.
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        let undo = js(&t.request("GET", "/api/macros", "").1);
        assert!(undo["macros"].as_array().unwrap().is_empty(), "{undo}");
    }

    /// T-444: the show list over the API: saved (history, undone, 423 in
    /// show mode), played, next / previous / stop (allowed in show mode),
    /// its state in `/api/state`; never arming, refused under an e-stop.
    #[test]
    fn the_show_list_over_the_api() {
        let t = TestServer::start(false);
        let dir = std::env::temp_dir().join(format!("laser-studio-web-showlist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        t.shared.lock().unwrap().shows = crate::timeline::ShowStore::new(dir.join("shows"));
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        for name in ["Liste A", "Liste B"] {
            let show = json!({ "name": name, "tracks": [{ "layer": 1, "events": [{ "id": 1, "start": 0.0, "len": 30.0, "source": { "kind": "cue", "id": cue } }] }] });
            let (code, body) = t.request("POST", "/api/shows", &show.to_string());
            assert_eq!(code, 200, "{body}");
        }
        let list = json!({ "items": [{ "show": "Liste A" }, { "show": "Liste B", "follow_system_tempo": false }] });
        let (code, body) = t.request("POST", "/api/showlist", &list.to_string());
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert_eq!((v["items"][0]["id"].as_u64(), v["items"][1]["id"].as_u64()), (Some(1), Some(2)));
        assert_eq!(v["items"][0]["duration_s"], 30.0);
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Liste de shows modifiée");
        assert_eq!(t.request("POST", "/api/showlist", r#"{"items":[{"show":"../x"}]}"#).0, 400);
        // Show mode: the list is locked, playing is not.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/showlist", &list.to_string()).0, 423);
        let (code, body) = t.request("POST", "/api/showlist/play", "{}");
        assert_eq!(code, 200, "{body}");
        let st = js(&t.request("GET", "/api/state", "").1);
        assert_eq!((st["showlist"]["transport"].as_str(), st["showlist"]["index"].as_u64()), (Some("playing"), Some(0)));
        assert_eq!(st["timeline"]["name"], "Liste A");
        assert_eq!(st["armed"], false);
        assert_eq!(t.request("POST", "/api/showlist/next", "{}").0, 200);
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["showlist"]["index"], 1);
        assert_eq!(t.request("POST", "/api/showlist/prev", "{}").0, 200);
        assert_eq!(t.request("POST", "/api/showlist/loop", r#"{"on":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/showlist/stop", "{}").0, 200);
        assert_eq!(js(&t.request("GET", "/api/showlist", "").1)["state"]["transport"], "stopped");
        assert_eq!(t.request("POST", "/api/showlist/play", r#"{"index":9}"#).0, 409);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        // The e-stop: refused until the reset, and nothing restarts by itself.
        assert_eq!(t.request("POST", "/api/showlist/play", r#"{"index":1}"#).0, 200);
        assert_eq!(t.request("POST", "/api/estop?source=keyboard", "").0, 200);
        assert_eq!(js(&t.request("GET", "/api/showlist", "").1)["state"]["transport"], "stopped");
        assert_eq!(t.request("POST", "/api/showlist/play", "{}").0, 409);
        assert_eq!(t.request("POST", "/api/estop/reset", "").0, 200);
        assert_eq!(js(&t.request("GET", "/api/showlist", "").1)["state"]["transport"], "stopped");
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
        // Undo takes the items back (empty); the loop is a transport switch.
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        let undone = js(&t.request("GET", "/api/showlist", "").1);
        assert!(undone["items"].as_array().unwrap().is_empty(), "{undone}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The agenda (T-455): off by default, edited (checked, in the
    /// history, locked in show mode), its state in `/api/state`; the test
    /// clock only with `--test-hooks`; nothing external reaches it.
    #[test]
    fn the_agenda_over_the_api() {
        let t = TestServer::start(false);
        let g = js(&t.request("GET", "/api/agenda", "").1);
        assert_eq!(g["agenda"]["enabled"], false);
        assert!(g["agenda"]["entries"].as_array().unwrap().is_empty());
        let agenda = json!({ "enabled": true, "entries": [{ "name": "Show du soir", "target": { "kind": "show", "name": "Soir" }, "time": "20:30", "days": [false, false, false, false, true, false, false] }] });
        let (code, body) = t.request("POST", "/api/agenda", &agenda.to_string());
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert_eq!(v["agenda"]["entries"][0]["id"], 1);
        assert_eq!(v["state"]["next"]["name"], "Show du soir");
        assert_eq!(v["state"]["next"]["time"], "20:30");
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Agenda activé");
        let st = js(&t.request("GET", "/api/state", "").1);
        assert_eq!((st["agenda"]["enabled"].as_bool(), st["agenda"]["next"]["name"].as_str()), (Some(true), Some("Show du soir")));
        assert!(st["agenda"]["last_runs"].as_array().unwrap().is_empty());
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["agenda"]["enabled"], true);
        assert_eq!(t.request("POST", "/api/agenda", r#"{"entries":[{"name":"X","time":"25:00"}]}"#).0, 400);
        assert_eq!(t.request("POST", "/api/agenda", r#"{"entries":[{"name":"X","target":{"kind":"cue_list"}}]}"#).0, 400);
        // Show mode: locked.
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/agenda", r#"{"enabled":false}"#).0, 423);
        assert_eq!(t.request("POST", "/api/lock", r#"{"show_mode":false}"#).0, 200);
        // Without --test-hooks, no test clock.
        assert_eq!(t.request("POST", "/api/test/clock", r#"{"at":"2026-10-09T20:29:00"}"#).0, 404);
        // Undo: off and empty again.
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        let undone = js(&t.request("GET", "/api/agenda", "").1);
        assert_eq!(undone["agenda"]["enabled"], false);
        assert!(undone["agenda"]["entries"].as_array().unwrap().is_empty(), "{undone}");
        assert!(!arm_status(&t)["armed"].as_bool().unwrap());
    }

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
            json!({ "target": { "type": "macro", "id": "m9" }, "fader": "level", "buttons": ["go", "none"] }),
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
    /// T-449: an undo whose body can't be read or parsed is refused and
    /// undoes nothing (it used to undo a step); a JSON body announced over
    /// `MAX_JSON_BODY` is refused (413) before a byte of it is read, on
    /// every route that reads one.
    #[test]
    fn a_garbled_undo_undoes_nothing_and_oversized_bodies_are_refused() {
        let t = TestServer::start(false);
        let has_scene = |t: &TestServer| t.shared.lock().unwrap().scenes.get("Garder").is_some();
        assert_eq!(t.request("POST", "/api/scenes/save", r#"{"name":"Garder","duration_secs":4}"#).0, 200);
        assert_eq!(t.request("POST", "/api/undo", r#"{"to":"#).0, 400);
        assert_eq!(t.request("POST", "/api/redo", "pas du JSON").0, 400);
        assert!(has_scene(&t), "a refused undo undoes nothing");
        for path in ["/api/undo", "/api/settings", "/api/project/save-as", "/api/midi/profile", "/api/cuelist/go", "/api/checklist/operator"] {
            let mut stream = TcpStream::connect(t.addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(stream, "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{{}}", MAX_JSON_BODY + 1).unwrap();
            let mut head = [0u8; 12];
            stream.read_exact(&mut head).unwrap();
            assert_eq!(&head, b"HTTP/1.1 413", "{path}");
        }
        assert!(has_scene(&t));
        assert_eq!(t.request("POST", "/api/undo", "").0, 200, "an empty body is still one step");
        assert!(!has_scene(&t));
    }

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

    /// T-441: a figure cue's per-part modifiers over the API: stored in
    /// the cell (an edit: history, undo), kept by the properties dialog,
    /// refused on other cues and out-of-range parts; the part modulators
    /// round-trip through `/api/lfos`, an unknown figure accepted.
    #[test]
    fn figure_part_modifiers_over_the_api() {
        let t = TestServer::start(false);
        let mut fig = crate::figures::test_figure("Visage", 1);
        fig.frames[0].strokes[2].part = 1;
        fig.parts = vec![crate::figures::PartDef { id: 1, name: "Yeux".into() }];
        assert_eq!(t.request("POST", "/api/figures", &serde_json::to_string(&fig).unwrap()).0, 200);
        let cue = "figure:Visage";
        let set = |body: &str| t.request("POST", "/api/cues/parts", body);
        assert_eq!(set(r#"{"id":"tunnels-001","part":1,"modifiers":{"brightness":0.5}}"#).0, 404, "not a figure");
        assert_eq!(set(r#"{"id":"figure:Visage","part":9,"modifiers":{"brightness":0.5}}"#).0, 400);
        let (code, body) = set(r#"{"id":"figure:Visage","part":1,"modifiers":{"brightness":0.5,"rot_angle":[0,0,30]}}"#);
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["1"]["brightness"], 0.5);
        assert_eq!(t.shared.lock().unwrap().deck.slot(cue).part_modifiers[&1].rot_angle[2], 30.0);
        let history = js(&t.request("GET", "/api/history", "").1);
        assert!(history["undo"].as_array().unwrap().iter().any(|e| e["label"] == "Parties du cue « Visage » modifiées"), "{history}");
        // The properties dialog never sends them, and keeps them.
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"figure:Visage","layer":2}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().deck.slot(cue).part_modifiers.len(), 1);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert!(t.shared.lock().unwrap().deck.slot(cue).part_modifiers.is_empty());
        // Removed with null.
        assert_eq!(t.request("POST", "/api/redo", "{}").0, 200);
        assert_eq!(set(r#"{"id":"figure:Visage","part":1,"modifiers":null}"#).0, 200);
        assert!(t.shared.lock().unwrap().deck.slot(cue).part_modifiers.is_empty());
        // Part modulators: saved and read back; a figure that doesn't exist is fine.
        // (Into a file of this test's own: the test servers share the default one.)
        let dir = std::env::temp_dir().join(format!("laser-studio-part-lfos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut s = t.shared.lock().unwrap();
            s.lfos = crate::lfo::LfoStore::load_or_create(dir.join("lfos.json"), &s.controls);
        }
        let lfos = r#"[{"target":"master.size","part":{"cue":"figure:Visage","part":1,"param":"brightness"}},{"part":{"cue":"figure:Disparue","part":3,"param":"angle_z"}}]"#;
        assert_eq!(t.request("POST", "/api/lfos", lfos).0, 200);
        let back = js(&t.request("GET", "/api/lfos", "").1);
        assert_eq!(back["lfos"][1]["part"]["param"], "angle_z", "{back}");
        assert_eq!(back["parts"][0]["cue"], "figure:Visage");
        assert_eq!(back["parts"][0]["parts"][0]["name"], "Yeux");
        assert_eq!(t.request("POST", "/api/lfos", r#"[{"part":{"cue":"tunnels-001","part":1,"param":"size"}}]"#).0, 400, "not a figure cue");
        assert_eq!(t.request("POST", "/api/lfos", r#"[{"part":{"cue":"figure:Visage","part":0,"param":"size"}}]"#).0, 400);
        assert!(!arm_status(&t)["armed"].as_bool().unwrap(), "nothing here arms");
        let _ = std::fs::remove_dir_all(dir);
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
            let fast = ["/", "/beam3d.js", "/labels.js", "/ui/*", "/api/estop", "/api/events", "/api/frame.bin"].contains(&p);
            assert!(fast || found.iter().any(|(fm, fp)| fm == m && fp == p), "{m} {p} in ROUTES but not served");
        }
        for (m, p) in [("POST", "/api/arm"), ("POST", "/api/estop/reset"), ("POST", "/api/heartbeat")] {
            assert!(ROUTES.contains(&(m, p, RouteClass::Arm)), "{p}");
        }
        assert!(is_known_route("POST", "/api/timeline/play") && is_known_route("GET", "/api/midi/devices") && !is_known_route("GET", "/nope"));
    }

    /// T-169: the live take over the API: recording is allowed in show
    /// mode (it only observes), keeping it is not; the take is laid out as
    /// new tracks without touching the loaded show; nothing is armed.
    #[test]
    fn the_live_take_over_the_api() {
        use crate::take::{EndReason, Take, TakeEvent};
        use crate::timeline::{Show, TimeBase};
        let t = TestServer::start(false);
        assert_eq!(t.request("POST", "/api/timeline/record/start", "").0, 409, "no show");
        let cue = t.shared.lock().unwrap().presets[0].id.clone();
        t.shared.lock().unwrap().timeline.load(Show::new_empty("Morceau", TimeBase::Seconds));
        assert_eq!(route_class("POST", "/api/timeline/record/start"), Some(RouteClass::Play));
        assert_eq!(route_class("POST", "/api/timeline/record/take"), Some(RouteClass::Edit));
        t.shared.lock().unwrap().show_mode.on = true;
        let (st, body) = t.request("POST", "/api/timeline/record/start", "");
        assert_eq!((st, js(&body)["state"].clone()), (200, json!("waiting")), "{body}");
        assert_eq!(js(&t.request("GET", "/api/state", "").1)["take"]["state"], "waiting");
        assert_eq!(js(&t.request("POST", "/api/timeline/record/stop", "").1)["state"], "idle");
        let take = Take {
            show: "Morceau".into(),
            time_base: TimeBase::Seconds,
            start: 0.0,
            end: 4.0,
            events: vec![TakeEvent { cue: cue.clone(), layer: 2, start: 0.8, end: 3.0 }],
            envelopes: Vec::new(),
            reason: EndReason::User,
        };
        t.shared.lock().unwrap().take.take = Some(take);
        assert_eq!(t.request("POST", "/api/timeline/record/start", "").0, 409, "a take waits");
        assert_eq!(t.request("POST", "/api/timeline/record/take", "{}").0, 423, "keeping it is an edit");
        t.shared.lock().unwrap().show_mode.on = false;
        let (st, body) = t.request("POST", "/api/timeline/record/take", r#"{"quantize":"bar"}"#);
        assert_eq!(st, 200, "{body}");
        let v = js(&body);
        assert_eq!((v["added"].clone(), v["tracks"].clone()), (json!(1), json!(["Prise 1"])));
        let track = &v["show"]["tracks"][2];
        assert_eq!((track["name"].clone(), track["layer"].clone(), track["events"][0]["start"].clone()), (json!("Prise 1"), json!(2), json!(0.0)));
        assert_eq!(t.shared.lock().unwrap().timeline.show.as_ref().unwrap().tracks.len(), 2, "the loaded show is untouched");
        // Another show loaded: the take's positions are not its own.
        t.shared.lock().unwrap().timeline.load(Show::new_empty("Autre", TimeBase::Seconds));
        assert_eq!(t.request("POST", "/api/timeline/record/take", "{}").0, 409);
        assert!(js(&t.request("POST", "/api/timeline/record/discard", "").1)["take"].is_null());
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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

    /// The page's ES modules (T-435), joined: what the former single script
    /// held. For the tests that scan the interface's code.
    fn ui_scripts() -> String {
        UI_ASSETS.iter().filter(|a| a.0.ends_with(".js")).map(|a| a.1).collect::<Vec<_>>().join("\n")
    }

    /// The page's stylesheet (T-435).
    fn ui_styles() -> &'static str {
        UI_ASSETS.iter().find(|a| a.0 == "/ui/styles.css").expect("styles.css").1
    }

    /// The page and its modules, joined: the whole interface's source.
    fn ui_source() -> String {
        format!("{INDEX_HTML}\n{}", ui_scripts())
    }

    /// T-432: every request of the page goes through `api()`: the modules
    /// hold a single `fetch(`, inside it (the 3D module has none). The page
    /// itself has one, in the inline Échap guard that runs before any
    /// module is loaded (T-435) and so cannot use api().
    #[test]
    fn the_page_fetches_only_through_api() {
        // The heartbeat worker's source runs apart, without the page: its
        // own fetch is the only other one.
        let scripts = ui_scripts();
        let start = scripts.find("const HB_WORKER = `").expect("heartbeat worker");
        let end = start + scripts[start..].find("`;").expect("end of the worker");
        let scripts = format!("{}{}", &scripts[..start], &scripts[end..]);
        let body = scripts.split("async function api(").nth(1).expect("api() in the modules");
        let api_end = body.find("\n}\n").expect("end of api()");
        assert_eq!(scripts.matches("fetch(").count(), 1, "one fetch( in the modules");
        assert!(body[..api_end].contains("fetch("), "and it is in api()");
        assert!(!BEAM3D_JS.contains("fetch("));
        assert_eq!(INDEX_HTML.matches("fetch(").count(), 1, "one fetch( in the page");
        let guard = escape_guard();
        assert!(guard.contains("fetch('/api/estop?source=keyboard', { method: 'POST', keepalive: true })"), "and it is the e-stop of the guard");
    }

    /// The inline script of `<head>` that stops the laser on Échap before
    /// the modules are loaded (T-435).
    fn escape_guard() -> &'static str {
        let head = &INDEX_HTML[..INDEX_HTML.find("<body>").expect("body")];
        let start = head.find("<script>").expect("a classic script in <head>");
        &head[start..start + head[start..].find("</script>").expect("closed")]
    }

    /// T-435: the page's stylesheet and modules are served from the binary,
    /// with a JavaScript / CSS type (browsers refuse modules otherwise) and
    /// revalidated on every load; the page loads them, and nothing else;
    /// every file stays within the size the task sets (one job per module).
    #[test]
    fn ui_assets_are_served_from_the_binary_and_fit_their_limits() {
        let t = TestServer::start(false);
        for &(path, body, content_type) in UI_ASSETS {
            assert_eq!(static_asset(path), Some((body, content_type)), "{path}");
            assert!(ROUTES.iter().any(|r| r.0 == "GET" && r.1 == "/ui/*"), "the /ui/* route");
            let expected = if path.ends_with(".css") { "text/css; charset=utf-8" } else { "text/javascript; charset=utf-8" };
            assert_eq!(content_type, expected, "{path}");
            let lines = body.lines().count();
            assert!(lines < 1100 || path.ends_with(".css"), "{path}: {lines} lines; split it (one job per module)");
            // The SVG namespace is a name, never fetched; nothing else may look like an address.
            let addresses = body.replace("'http://www.w3.org/2000/svg'", "");
            assert!(!addresses.contains("http://") && !addresses.contains("https://"), "{path}: no external URL");
            assert!(!body.contains("import(") || path == "/ui/view3d.js", "{path}: a dynamic import is beam3d.js, in view3d.js only");
        }
        // Served with the header (a raw request: the helper drops the head).
        let mut stream = TcpStream::connect(t.addr).unwrap();
        write!(stream, "GET /ui/app.js HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let head = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(head.contains("content-type: text/javascript; charset=utf-8"), "{head}");
        assert!(head.contains("cache-control: no-cache"), "{head}");
        assert_eq!(t.request("GET", "/ui/nothing.js", "").0, 404);

        // The page: markup only, with the stylesheet and the entry module.
        assert!(!INDEX_HTML.contains("<style"), "the styles are in /ui/styles.css");
        assert!(INDEX_HTML.contains("<link rel=\"stylesheet\" href=\"/ui/styles.css\">"));
        assert!(INDEX_HTML.contains("<script type=\"module\" src=\"/ui/app.js\"></script>"));
        let page_lines = INDEX_HTML.lines().count();
        assert!(page_lines < 3600, "index.html: {page_lines} lines (markup only; the task's 600 was for a 4 800-line page)");
        let inline: usize = INDEX_HTML.split("<script>").skip(1).map(|s| s[..s.find("</script>").unwrap()].lines().count()).sum();
        assert!(inline < 120, "{inline} lines of inline script: the Échap guard and the labels fallback only");
        for tag in INDEX_HTML.split("<script").skip(1) {
            let open = &tag[..tag.find('>').unwrap()];
            if let Some(i) = open.find("src=\"") {
                let src = &open[i + 5..i + 5 + open[i + 5..].find('"').unwrap()];
                assert!(static_asset(src).is_some(), "{src} is not served");
            }
        }
    }

    /// T-435: every `import … from '…'` and `import('…')` of the modules
    /// names a file the studio serves itself (no CDN, no path outside
    /// `/ui/` but the 3D view), and the chain keeps the page order: each
    /// module imports the one before it first, app.js the last one.
    #[test]
    fn module_imports_resolve_to_served_assets_in_page_order() {
        let modules: Vec<&str> = UI_ASSETS.iter().map(|a| a.0).filter(|p| p.ends_with(".js")).collect();
        let mut imports_seen = 0;
        for &(path, body, _) in UI_ASSETS.iter().filter(|a| a.0.ends_with(".js")) {
            let mut first: Option<String> = None;
            // An import statement may wrap over several lines, up to its `;`.
            let mut statement = String::new();
            for line in body.lines() {
                if !statement.is_empty() || line.starts_with("import ") {
                    statement.push_str(line);
                    if !line.ends_with(';') {
                        continue;
                    }
                }
                let target = if let Some(stmt) = statement.strip_prefix("import ") {
                    let from = stmt.rfind(" from '").map(|i| &stmt[i + 7..]).or_else(|| stmt.strip_prefix('\'')).expect("import … from '…' or import '…'");
                    Some(from[..from.find('\'').unwrap()].to_string())
                } else {
                    line.find("import('").map(|i| line[i + 8..i + 8 + line[i + 8..].find('\'').unwrap()].to_string())
                };
                let is_static = !statement.is_empty();
                statement.clear();
                let Some(target) = target else { continue };
                let resolved = if let Some(rel) = target.strip_prefix("./") { format!("/ui/{rel}") } else { target.clone() };
                assert!(static_asset(&resolved).is_some(), "{path} imports {target}: not served");
                assert!(!target.contains("//"), "{path} imports {target}");
                imports_seen += 1;
                if first.is_none() && is_static {
                    first = Some(resolved);
                }
            }
            let i = modules.iter().position(|m| *m == path).unwrap();
            let want = if path == "/ui/app.js" { modules[modules.len() - 2] } else if i > 0 { modules[i - 1] } else { continue };
            assert_eq!(first.as_deref(), Some(want), "{path} must import {want} first (page order)");
        }
        assert!(imports_seen > 100, "{imports_seen} imports found");
        // The modules may not be imported by beam3d.js nor labels.js (they stay apart).
        assert!(!BEAM3D_JS.contains("/ui/") && !LABELS_JS.contains("/ui/"));
    }

    /// T-435: Échap stops the laser as soon as the page shows, before any
    /// module is loaded and even if one fails: a classic script in `<head>`
    /// posts the e-stop and shows the banner until laser.js has taken over
    /// (`data-ui-ready`, set at the end of app.js). A module that fails to
    /// load shows « Interface incomplète » (text kept in the markup).
    #[test]
    fn escape_stops_the_laser_before_the_modules_load() {
        let guard = escape_guard();
        assert!(guard.contains("'Escape'") && guard.contains("/api/estop?source=keyboard") && guard.contains("getElementById('estopBanner')"));
        assert!(guard.contains("'uiReady' in document.documentElement.dataset"));
        assert!(!guard.contains("import"), "the guard depends on no module");
        let app = static_asset("/ui/app.js").unwrap().0;
        assert!(app.trim_end().ends_with("document.documentElement.dataset.uiReady = '';"), "app.js hands Échap over last");
        assert!(INDEX_HTML.find("<script>").unwrap() < INDEX_HTML.find("<body>").unwrap(), "the guard is in <head>");
        assert!(INDEX_HTML.find("<body>").unwrap() < INDEX_HTML.find("type=\"module\"").unwrap());
        assert!(guard.contains("getElementById('uiIncomplete')"));
        assert!(INDEX_HTML.contains("<div id=\"uiIncomplete\" role=\"alert\" hidden data-t=\"page.incomplete\" data-t-fallback>Interface incomplète : rechargez la page</div>"));
        assert!(LABELS_JS.contains("'page.incomplete': 'Interface incomplète : rechargez la page',"));
        assert!(ui_styles().contains("#uiIncomplete[hidden] { display: none; }"));
    }

    /// T-431: every slider of the page (static, or in a template) has a
    /// default to go back to: the registry's (`data-control`) or its own
    /// `data-default`.
    #[test]
    fn every_slider_of_the_page_has_a_default() {
        let mut n = 0;
        let source = ui_source();
        for tag in source.split('<').filter(|t| t.starts_with("input ") && t.contains(r#"type="range""#)) {
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
            // A data-t text (T-461) comes from labels.js, never empty (tests/ui_labels.rs).
            let readable = text.split(|c: char| !c.is_alphanumeric()).any(|w| w.chars().count() >= 2) || part[..part.find("</button>").unwrap()].contains("data-t=\"");
            assert!(readable || open.contains("aria-label"), "<button{open}> {:?} needs an aria-label", text.trim());
            n += 1;
        }
        assert!(n > 300, "{n} buttons found");
        // Through the text table (T-461): the attribute names the key, labels.js holds the text.
        for (sel, key, label) in [
            ("tempo.half\"", "tempo.moitie.libelle", "Tempo divisé par deux"),
            ("tempo.double\"", "tempo.double.libelle", "Tempo doublé"),
            ("tempo.nudge_up\"", "tempo.avancer.libelle", "Avancer la phase"),
            ("tempo.nudge_down\"", "tempo.retarder.libelle", "Retarder la phase"),
        ] {
            assert!(page.contains(&format!("data-ctl=\"{sel} data-t-aria-label=\"{key}\"")), "{label}");
            assert!(LABELS_JS.contains(&format!("'{key}': '{label}',")), "{label}");
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
        assert!(ui_scripts().contains("{ id: 'search', group: 'Recherche', keys: ['k', 'f'], meta: true"));
    }

    /// T-282: the night mode dims the neutral and accent tokens only; the
    /// state colours (armed / danger, warning, playing) and the undimmed
    /// safety tokens are never redefined, and touch mode leaves the laser
    /// button and ARRÊT out of the long press.
    #[test]
    fn night_mode_never_dims_the_safety_colours() {
        let css = ui_styles();
        let start = css.find("---------- Mode nuit (T-282)").expect("night mode CSS");
        let end = start + css[start..].find("---------- Mode tactile (T-282)").expect("touch mode CSS");
        let night = &css[start..end];
        assert!(night.contains("--text:"), "night mode redefines the text token");
        for token in ["--danger:", "--warn:", "--ok:", "--text-full:", "--accent-full:"] {
            assert!(!night.contains(token), "night mode must not redefine {token}");
        }
        let scripts = ui_scripts();
        let skip = scripts.lines().find(|l| l.starts_with("const LONG_PRESS_SKIP")).expect("long press skip list");
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

    /// T-447: the zone trace starts and stops from the local UI, never
    /// arms, refuses a bad body and an unknown zone, shows in the state,
    /// and in show mode can only be stopped.
    #[test]
    fn zone_trace_api_starts_and_stops_without_arming() {
        let t = TestServer::start(false);
        let zone = crate::zones::Zone { points: vec![[-0.2, -0.2], [0.2, -0.2], [0.2, 0.2], [-0.2, 0.2]], ..Default::default() };
        let id = {
            let mut s = t.shared.lock().unwrap();
            let saved = s.safety.set(crate::safety::SafetySettings { zones: vec![zone], ..Default::default() }, true).unwrap();
            saved.zones[0].id
        };
        assert_eq!(t.request("POST", "/api/safety/trace", "{}").0, 400);
        assert_eq!(t.request("POST", "/api/safety/trace", r#"{"on":true,"only":"x"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/safety/trace", r#"{"on":true,"only":999}"#).0, 409);
        assert!(!t.shared.lock().unwrap().zone_trace.is_on(), "a refused body changes nothing");
        let (status, body) = t.request("POST", "/api/safety/trace", &format!(r#"{{"on":true,"only":{id},"client_id":"p"}}"#));
        assert_eq!(status, 200, "{body}");
        let v = js(&body);
        assert_eq!((v["on"].clone(), v["only"].clone()), (json!(true), json!(id)));
        assert!(v["remaining_s"].as_u64().unwrap() > 55);
        let state = js(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["zone_trace"]["on"], true, "{}", state["zone_trace"]);
        assert!(!t.shared.lock().unwrap().gate.is_armed(), "the trace never arms");
        // Show mode: stopping only.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/safety/trace", r#"{"on":true}"#).0, 423);
        assert_eq!(t.request("POST", "/api/safety/trace", r#"{"on":false}"#).0, 200);
        assert!(!t.shared.lock().unwrap().zone_trace.is_on());
        t.shared.lock().unwrap().show_mode.on = false;
        // Under the e-stop: refused.
        t.request("POST", "/api/estop", "");
        assert_eq!(t.request("POST", "/api/safety/trace", r#"{"on":true}"#).0, 409);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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
        // A DXF (T-497), layers as parts on request.
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\nContour\n62\n1\n10\n0\n20\n0\n11\n10\n21\n0\n0\nENDSEC\n0\nEOF\n";
        let (status, body) = t.request("POST", "/api/figures/import?name=plan.dxf&layers=1", dxf);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["stats"]["kind"], "dxf");
        assert_eq!(v["figure"]["parts"][0]["name"], "Contour");
        // Still serving.
        assert_eq!(t.request("GET", "/api/arm", "").0, 200);
        assert_eq!(parse_hex("#00ff80"), Some([0, 255, 128]));
        assert_eq!(parse_hex("#00ff8"), None);
    }

    /// T-462: an OBJ file previewed, imported (its cue on the « Objets 3D »
    /// page), edited, undone, refused in show mode and when hostile, deleted.
    #[test]
    fn object_import_previews_saves_undoes_and_refuses_bad_files() {
        let t = TestServer::start(false);
        let cube = "v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
            f 1 2 3\nf 1 3 4\nf 5 8 7\nf 5 7 6\nf 1 5 6\nf 1 6 2\nf 2 6 7\nf 2 7 3\nf 3 7 8\nf 3 8 4\nf 5 1 4\nf 5 4 8\n";
        let (status, body) = t.request("POST", "/api/objects/import?name=Mon%20cube.obj&preview=1", cube);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((v["object"]["name"].as_str(), v["report"]["edges"].as_u64(), v["report"]["faces"].as_u64()), (Some("Mon cube"), Some(12), Some(12)));
        assert_eq!(v["budget"], crate::layers::DEFAULT_POINT_BUDGET);
        let (_, body) = t.request("POST", "/api/objects/import?name=c&preview=1&coplanar=0", cube);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["report"]["edges"], 18);
        assert!(t.shared.lock().unwrap().objects.list().is_empty(), "a preview saves nothing");
        assert!(t.shared.lock().unwrap().history.view()["undo"].as_array().unwrap().is_empty());

        // Imported: in the library, a cue of the « Objets 3D » page, one undo step.
        let (status, body) = t.request("POST", "/api/objects/import?name=Mon%20cube", cube);
        assert_eq!(status, 200, "{body}");
        {
            let s = t.shared.lock().unwrap();
            assert_eq!(s.objects.list().len(), 1);
            let p = s.presets.iter().find(|p| p.id == "object:Mon cube").expect("its cue");
            assert_eq!(p.category, crate::wire::CATEGORY);
            let page = s.grid.view().iter().find(|v| v.name == crate::wire::CATEGORY).expect("its page");
            assert_eq!(page.cells[0].as_deref(), Some("object:Mon cube"));
        }
        assert_eq!(t.request("POST", "/api/objects/import?name=Mon%20cube", cube).0, 409, "the name is taken");
        let (_, list) = t.request("GET", "/api/objects", "");
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!((list["objects"][0]["edges"].as_u64(), list["max_edges"].as_u64()), (Some(12), Some(2000)));
        // Properties, then undone.
        let (status, body) = t.request("POST", "/api/objects", r#"{"name":"Mon cube","color":[255,0,0],"perspective":99,"rename":"Cube rouge"}"#);
        assert_eq!(status, 200, "{body}");
        assert_eq!(t.shared.lock().unwrap().objects.get("Cube rouge").map(|o| (o.color, o.perspective)), Some(([255, 0, 0], crate::wire::MAX_PERSPECTIVE)));
        assert!(t.shared.lock().unwrap().objects.get("Mon cube").is_none());
        assert_eq!(t.request("POST", "/api/undo", "").0, 200);
        assert!(t.shared.lock().unwrap().objects.get("Mon cube").is_some_and(|o| o.color == [0, 255, 0]));
        assert_eq!(t.request("POST", "/api/undo", "").0, 200);
        assert!(t.shared.lock().unwrap().objects.list().is_empty(), "the import undone");
        assert!(!t.shared.lock().unwrap().presets.iter().any(|p| p.id.starts_with(crate::wire::ID_PREFIX)));
        assert_eq!(t.request("POST", "/api/redo", "").0, 200);
        assert_eq!(t.shared.lock().unwrap().objects.list().len(), 1);

        // Show mode: every edit is locked (423), the list still reads.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/objects/import?name=x", cube).0, 423);
        assert_eq!(t.request("POST", "/api/objects/delete", r#"{"name":"Mon cube"}"#).0, 423);
        assert_eq!(t.request("POST", "/api/objects", r#"{"name":"Mon cube"}"#).0, 423);
        assert_eq!(t.request("GET", "/api/objects", "").0, 200);
        t.shared.lock().unwrap().show_mode.on = false;

        // Hostile or empty files: a French message, nothing saved, still serving.
        for (file, msg) in [
            ("", "le fichier est vide"),
            ("# rien\nvt 0 0\n", "aucun sommet"),
            ("v 0 0 0\nv nan 1 1\nl 1 2\n", "non finie"),
            ("\u{0}\u{1}binaire", "pas un fichier texte"),
        ] {
            let (status, body) = t.request("POST", "/api/objects/import?name=bad", file);
            assert_eq!(status, 400, "{file:?}");
            assert!(body.contains(msg), "{body}");
        }
        let huge = "#".repeat(crate::figure_import::obj::MAX_OBJ_BYTES + 10);
        let (status, body) = t.request("POST", "/api/objects/import?name=big", &huge);
        assert_eq!(status, 413);
        assert!(body.contains("8 Mo au plus"), "{body}");
        assert_eq!(t.shared.lock().unwrap().objects.list().len(), 1);
        assert_eq!(t.request("POST", "/api/objects/delete", r#"{"name":"Mon cube"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/objects/delete", r#"{"name":"Mon cube"}"#).0, 404);
        assert!(t.shared.lock().unwrap().objects.list().is_empty());
        assert_eq!(t.request("GET", "/api/arm", "").0, 200);
        assert!(may_decode(&Method::Post, "/api/objects/import?name=a"));
        assert!(!may_decode(&Method::Post, "/api/objects/delete"));
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

    /// T-148: the autopilot's pages are an edit (checked, undone, 423 in
    /// show mode); its switch, rhythm and order are controls that keep
    /// working in show mode and never arm; the skip flag is a cue property.
    #[test]
    fn autopilot_over_the_api() {
        let t = TestServer::start(false);
        let json = |(code, body): (u16, String)| -> (u16, serde_json::Value) { (code, serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)) };
        let (code, v) = json(t.request("GET", "/api/autopilot", ""));
        assert_eq!((code, v["enabled"].as_bool(), v["every_bars"].as_u64()), (200, Some(false), Some(4)));
        assert!(!v["candidates"].as_array().unwrap().is_empty());
        assert_eq!(t.request("POST", "/api/autopilot", r#"{"pages":[999]}"#).0, 400);
        let (code, v) = json(t.request("POST", "/api/autopilot", r#"{"pages":[2,1],"allow_audience":false}"#));
        assert_eq!((code, v["pages"].clone()), (200, json!([1, 2])));
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        assert!(t.shared.lock().unwrap().autopilot.config().pages.is_empty(), "undone");
        assert_eq!(t.request("POST", "/api/redo", "{}").0, 200);
        assert_eq!(t.shared.lock().unwrap().autopilot.config().pages, vec![1, 2]);

        let cue = t.shared.lock().unwrap().grid.view()[0].cells.iter().flatten().next().unwrap().clone();
        assert_eq!(t.request("POST", "/api/cues/slot", &json!({ "id": cue, "vlj_skip": true }).to_string()).0, 200);
        assert!(t.shared.lock().unwrap().deck.slot(&cue).vlj_skip);
        let (_, v) = json(t.request("GET", "/api/autopilot", ""));
        assert!(!v["candidates"].as_array().unwrap().iter().any(|c| c == &json!(cue)));

        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/autopilot", r#"{"pages":[]}"#).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"vlj.enabled","value":1}"#).0, 200, "the switch is playback");
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"vlj.every","value":3}"#).0, 200);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"vlj.order","value":1}"#).0, 200);
        {
            let s = t.shared.lock().unwrap();
            assert!(s.autopilot.enabled() && s.autopilot.config().random);
            assert_eq!(s.autopilot.config().every_bars, 8);
            assert!(!s.gate.is_armed());
        }
        let (_, v) = json(t.request("GET", "/api/control-values", ""));
        assert_eq!((v["values"]["vlj.enabled"].clone(), v["values"]["vlj.every"].clone()), (json!(true), json!(3)));
        assert_eq!(t.request("POST", "/api/estop", "").0, 200);
        // The fast path trips the latch; the next engine frame sees it.
        crate::autopilot::sync_stops(&mut t.shared.lock().unwrap());
        assert!(!t.shared.lock().unwrap().autopilot.enabled());
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"vlj.enabled","value":1}"#).0, 403, "refused while latched");
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
            ("/api/project/new", r#"{"keep":{"midi":true}}"#),
            ("/api/project/save-template", r#"{"name":"x"}"#),
            ("/api/project/delete-template", r#"{"name":"x"}"#),
            ("/api/project/open", r#"{"path":"x"}"#),
            ("/api/midi/learn", r#"{"target":"master.size"}"#),
            ("/api/cues/slot", r#"{"id":"tunnels-001"}"#),
            ("/api/cues/modifiers", r#"{"id":"tunnels-001"}"#),
            ("/api/cues/parts", r#"{"id":"figure:x","part":1}"#),
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
        assert_eq!(t.request("GET", "/api/project/templates", "").0, 200);
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
        assert_eq!(code, 200);
        assert_eq!(js(&body)["show_mode"], false);
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
        // The venue file (T-500): a zip with the enabled instance only.
        let (code, mvr) = http_raw(t.addr, "/api/dmx/mvr");
        assert!(code == 200 && mvr.starts_with(b"PK"));
        let mvr = String::from_utf8_lossy(&mvr);
        assert!(mvr.contains("<Address break=\"0\">1.1</Address>") && mvr.matches("<Fixture ").count() == 1, "{mvr}");
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

    /// T-416: the address list, the feedback settings (off by default,
    /// validated, Safety class) and the monitor.
    #[test]
    fn osc_feedback_routes() {
        let t = TestServer::start(false);
        let list = js(&t.request("GET", "/api/osc/addresses", "").1);
        let list = list.as_array().unwrap();
        let size = list.iter().find(|a| a["control"] == "master.size").unwrap();
        assert_eq!((size["address"].as_str(), size["kind"].as_str(), size["max"].as_f64()), (Some("/ls/master/size"), Some("continuous"), Some(2.0)));
        assert!(list.iter().all(|a| a["control"] != "transport.arm" && a["control"] != "safety.hold"));
        let cfg = js(&t.request("GET", "/api/osc/config", "").1);
        assert_eq!((cfg["config"]["enabled"].as_bool(), cfg["config"]["allow_subscribe"].as_bool()), (Some(false), Some(true)));
        let (code, body) = t.request("POST", "/api/osc/config", r#"{"enabled":true,"targets":["127.0.0.1:9000"]}"#);
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["config"]["targets"][0], "127.0.0.1:9000");
        for bad in [r#"{"targets":["255.255.255.255:9000"]}"#, r#"{"targets":["console.local:9000"]}"#, r#"{"max_rate_hz":500}"#, r#"{"nope":1}"#] {
            assert_eq!(t.request("POST", "/api/osc/config", bad).0, 400, "{bad}");
        }
        assert!(ROUTES.contains(&("POST", "/api/osc/config", RouteClass::Safety)));
        let mon = js(&t.request("GET", "/api/osc/monitor?dir=out", "").1);
        assert_eq!(mon["capacity"], 200);
        assert_eq!(t.request("POST", "/api/osc/monitor/clear", "").0, 200);
        assert_eq!(t.request("POST", "/api/osc/config", r#"{"enabled":false,"targets":[]}"#).0, 200);
        assert_eq!(arm_status(&t)["armed"], false);
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

    /// T-452: « À propos » reports the version and folders; the release
    /// notes show once after an update, never on a first launch, and
    /// closing them is allowed in show mode. Never arms.
    #[test]
    fn about_reports_version_and_notes_once() {
        let t = TestServer::start(false);
        let js = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        let dir = std::env::temp_dir().join(format!("laser-studio-web-about-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        t.shared.lock().unwrap().project.data_dir = dir.clone();
        let (code, body) = t.request("GET", "/api/about", "");
        let v = js(&body);
        assert_eq!(code, 200);
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["data_dir"], json!(dir));
        assert_eq!(v["project_format"], crate::project::PROJECT_FORMAT);
        assert!(v["unseen"].is_null(), "first launch: nothing shown");
        assert!(v["changelog"].as_array().is_some_and(|c| !c.is_empty()));
        std::fs::write(dir.join("prefs.json"), r#"{"about":{"last_seen_version":"0.0.1"}}"#).unwrap();
        assert_eq!(js(&t.request("GET", "/api/about", "").1)["unseen"]["version"], env!("CARGO_PKG_VERSION"));
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/about/seen", "{}").0, 200, "not an edit: allowed in show mode");
        assert!(js(&t.request("GET", "/api/about", "").1)["unseen"].is_null());
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// T-426: operator profiles over the API, in show mode too; the
    /// settings, their revision and the arming never move.
    #[test]
    fn operator_profiles_never_touch_the_output() {
        let t = TestServer::start(false);
        let js = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        let dir = std::env::temp_dir().join(format!("laser-studio-web-operators-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rev0 = {
            let mut s = t.shared.lock().unwrap();
            s.project.data_dir = dir.clone();
            s.show_mode.on = true;
            s.settings_rev
        };
        let v = js(&t.request("GET", "/api/operators", "").1);
        assert_eq!((v["current"].clone(), v["names"].clone()), (json!("Défaut"), json!(["Défaut"])));
        let profile = r#"{"views":[{"name":"Jeu","slot":1,"live_panel":"music","stage_mode":"3d","cue_page":2}],"prefs":{"laserStudio.workspace":"live"}}"#;
        let (code, body) = t.request("POST", "/api/operators/R%C3%A9gie%202", profile);
        assert_eq!(code, 200, "{body}");
        assert_eq!(t.request("POST", "/api/operators/current", r#"{"name":"Régie 2"}"#).0, 200);
        let v = js(&t.request("GET", "/api/operators", "").1);
        assert_eq!((v["current"].clone(), v["names"].clone()), (json!("Régie 2"), json!(["Défaut", "Régie 2"])));
        let p = js(&t.request("GET", "/api/operators/R%C3%A9gie%202", "").1);
        assert_eq!((p["name"].clone(), p["views"][0]["cue_page"].clone(), p["views"][0]["workspace"].clone()), (json!("Régie 2"), json!(2), json!("live")));
        assert!(dir.join("operators").join("Régie 2.json").exists());
        assert_eq!(t.request("GET", "/api/operators/..%2Fprefs", "").0, 400);
        assert_eq!(t.request("POST", "/api/operators/..%2F..%2Fx", "{}").0, 400);
        assert_eq!(t.request("POST", "/api/operators/delete", r#"{"name":"Défaut"}"#).0, 409);
        assert_eq!(t.request("POST", "/api/operators/delete", r#"{"name":"Régie 2"}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/operators", "").1)["current"], "Défaut");
        let s = t.shared.lock().unwrap();
        assert_eq!(s.settings_rev, rev0, "the look never changes");
        assert!(!s.gate.is_armed(), "never arms");
        drop(s);
        std::fs::remove_dir_all(&dir).ok();
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

    /// T-448: `GET /api/perf` returns the engine's report with the pages
    /// connected now and the thresholds the panel colours with.
    #[test]
    fn perf_returns_the_engine_report_and_the_pages_connected() {
        let t = TestServer::start(false);
        let mut monitor = crate::perf::PerfMonitor::new();
        let d = Duration::from_millis;
        monitor.record(d(2), d(1), d(1), d(4), 1234);
        monitor.record(d(30), d(1), d(1), d(32), 1500);
        t.shared.lock().unwrap().perf = monitor.report(0);
        for id in ["p1", "p2"] {
            assert_eq!(t.request("POST", "/api/heartbeat", &format!(r#"{{"client_id":"{id}"}}"#)).0, 200);
        }
        let (status, body) = t.request("GET", "/api/perf", "");
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        for stage in ["render", "safety", "output", "frame"] {
            assert!(v[stage]["mean_ms"].is_number() && v[stage]["worst_ms"].is_number(), "{stage}: {v}");
        }
        assert_eq!(v["frame"]["worst_ms"], 32.0);
        assert_eq!((v["points"].as_u64(), v["late_frames"].as_u64(), v["late_in_window"].as_u64()), (Some(1500), Some(1), Some(1)));
        assert_eq!(v["clients"], 2);
        assert_eq!(v["warn_worst_ms"], 8.0);
        assert!(v["late_ms"].as_f64().unwrap() > 16.6 && v["window_s"].is_number() && v["frames"] == 2);
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

    /// T-453: the DMX output is off by default, its settings are Safety
    /// (never the remote), the self-drive trap is a 409 both ways, the haze
    /// button is live play and the values show under `dmx_out`.
    #[test]
    fn the_dmx_output_routes() {
        let t = TestServer::start(false);
        let (status, body) = t.request("GET", "/api/dmx-out", "");
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((v["config"]["enabled"].as_bool(), v["config"]["universe"].as_u64()), (Some(false), Some(1)));
        let cfg = r#"{"channels":[{"name":"Pompe","address":1},{"name":"Face","address":10,"rest":40}],"haze":{"pump":"Pompe","pump_level":180}}"#;
        assert_eq!(t.request("POST", "/api/dmx-out", cfg).0, 200);
        assert_eq!(t.request("POST", "/api/dmx-out", r#"{"enabled":true}"#).0, 400, "no destination");
        assert_eq!(t.request("POST", "/api/netin", r#"{"artnet":{"enabled":true,"universe":1}}"#).0, 200);
        let (code, msg) = t.request("POST", "/api/dmx-out", r#"{"enabled":true,"interface":"127.0.0.1","target":"127.0.0.1:7999"}"#);
        assert_eq!((code, msg.as_str()), (409, crate::dmx_out::SELF_DRIVE));
        assert_eq!(t.request("POST", "/api/dmx-out", r#"{"enabled":true,"interface":"127.0.0.1","target":"127.0.0.1:7999","universe":2}"#).0, 200);
        assert_eq!(t.request("POST", "/api/netin", r#"{"artnet":{"universe":2}}"#).0, 409);
        assert_eq!(t.request("POST", "/api/dmx/config", r#"{"artnet":{"universe":2}}"#).0, 409);
        assert_eq!(t.request("POST", "/api/dmx-out/haze", r#"{"on":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/dmx-out/haze", r#"{"fan":"x"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/dmx-out/haze", "{}").0, 400);
        {
            let mut s = t.shared.lock().unwrap();
            let c = s.timeline_clock();
            crate::dmx_out::frame(&mut s, &c);
        }
        let st: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/state", "").1).unwrap();
        assert_eq!((st["dmx_out"]["values"]["Pompe"].as_u64(), st["dmx_out"]["values"]["Face"].as_u64()), (Some(180), Some(40)));
        assert_eq!(st["dmx_out"]["haze"]["held"], true);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["dmx_out"]["enabled"], true);
        assert!(ROUTES.contains(&("POST", "/api/dmx-out", RouteClass::Safety)));
        assert!(ROUTES.contains(&("POST", "/api/dmx-out/haze", RouteClass::Play)));
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
    }

    /// T-417: the timecode output is off by default, set over the API
    /// (Safety class), refuses the self-drive trap with a 409 and shows up
    /// under `timeline.timecode_out`.
    #[test]
    fn the_timecode_output_routes() {
        let t = TestServer::start(false);
        {
            let mut s = t.shared.lock().unwrap();
            s.midi.enabled = true;
            s.midi.device_mut("Sortie MTC").output = true;
        }
        let (status, body) = t.request("GET", "/api/timecode/out", "");
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!((v["config"]["enabled"].as_bool(), v["config"]["fps"].as_str()), (Some(false), Some("25")));
        assert_eq!(v["midi_ports"], json!(["Sortie MTC"]));
        assert_eq!(t.request("POST", "/api/timecode/out", r#"{"enabled":true,"mtc_port":"Sortie MTC"}"#).0, 200);
        let frame: serde_json::Value = serde_json::from_str(&t.request("GET", "/api/frame", "").1).unwrap();
        assert_eq!(frame["timeline"]["timecode_out"]["enabled"], true);
        assert_eq!(frame["timeline"]["timecode_out"]["sending"], false);
        let (status, body) = t.request("POST", "/api/timecode", r#"{"source":"mtc","midi_port":"Sortie MTC"}"#);
        assert_eq!((status, body.as_str()), (409, crate::timecode::out::SELF_DRIVE_MIDI));
        assert_eq!(t.request("POST", "/api/timecode", r#"{"out":{"enabled":false}}"#).0, 400);
        assert_eq!(t.request("POST", "/api/timecode/out", r#"{"artnet":{"interface":"127.0.0.1","target":"224.0.0.1"}}"#).0, 400);
        assert_eq!(ROUTES.iter().find(|r| r.0 == "*" && r.1 == "/api/timecode/out").map(|r| r.2), Some(RouteClass::Safety));
        assert_eq!(arm_status(&t)["armed"], false, "never arms");
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

    /// T-241: *Son du Mac* is listed with why it can't be chosen, refused
    /// then, and accepted where it can run; never arms.
    #[test]
    fn son_du_mac_is_offered_only_where_it_can_run() {
        let t = TestServer::start(false);
        let read = |path: &str| -> serde_json::Value { serde_json::from_str(&t.request("GET", path, "").1).unwrap() };
        let sys = &read("/api/audio/devices")["system_audio"];
        assert_eq!(sys["compiled"].as_bool(), Some(cfg!(all(feature = "system-audio", target_os = "macos"))), "{sys}");
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"system_audio":false}"#).0, 200);
        let sys = read("/api/audio/devices")["system_audio"].clone();
        assert_eq!(sys["available"], false);
        assert!(sys["reason"].as_str().unwrap().contains("BlackHole"), "{sys}");
        let (code, body) = t.request("POST", "/api/audio/config", r#"{"source":"system"}"#);
        assert_eq!(code, 400, "{body}");
        assert!(body.contains("Son du Mac indisponible"), "{body}");
        assert_ne!(read("/api/audio/config")["source"], "system", "nothing changed");
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"system_audio":true}"#).0, 200);
        assert_eq!(read("/api/audio/diagnostics")["system_audio"]["available"], true);
        assert_eq!(t.request("POST", "/api/audio/config", r#"{"source":"system"}"#).0, 200);
        assert_eq!(read("/api/audio/state")["source"], "system");
        assert_eq!(arm_status(&t)["armed"], false);
    }

    /// T-242: the input diagnostics are read-only, show the health the
    /// analysis gave, and an input problem never arms the laser.
    #[test]
    fn audio_diagnostics_show_the_health_and_never_arm() {
        let t = TestServer::start(false);
        t.shared.lock().unwrap().test_hooks = true;
        let read = |path: &str| -> serde_json::Value { serde_json::from_str(&t.request("GET", path, "").1).unwrap() };
        // The browser source: nothing to judge.
        let d = read("/api/audio/diagnostics");
        assert!(d["health"].is_null() && d["last_error"].is_null(), "{d}");
        assert_eq!(t.request("POST", "/api/audio/config", r#"{"source":"native"}"#).0, 200);
        let sim = r#"{"state":"running","device":"Micro test","devices":[{"name":"Micro test","is_default":true}],
            "snapshot":{"level_db":-120,"peak_db":-120,"health":"permission_suspected"}}"#;
        assert_eq!(t.request("POST", "/api/test/native_audio", sim).0, 200);
        let d = read("/api/audio/diagnostics");
        assert_eq!((d["health"]["state"].as_str(), d["health"]["label"].as_str(), d["health"]["tone"].as_str()), (Some("permission_suspected"), Some("Autorisation refusée ?"), Some("bad")));
        assert_eq!((d["signal_present"].as_bool(), d["sample_rate"].as_u64(), d["level_db"].as_f64()), (Some(false), Some(48_000), Some(-120.0)));
        assert_eq!(d["devices"][0]["name"], "Micro test");
        assert_eq!(read("/api/audio/state")["health"]["state"], "permission_suspected");
        let sim = r#"{"state":"running","snapshot":{"level_db":-3,"peak_db":0,"health":"clipping"}}"#;
        assert_eq!(t.request("POST", "/api/test/native_audio", sim).0, 200);
        let d = read("/api/audio/diagnostics");
        assert_eq!((d["health"]["label"].as_str(), d["signal_present"].as_bool()), (Some("Entrée trop forte"), Some(true)));
        assert_eq!(t.request("POST", "/api/test/native_audio", r#"{"state":"device_lost","message":"Entrée « Micro test » perdue"}"#).0, 200);
        let d = read("/api/audio/diagnostics");
        assert_eq!((d["health"]["state"].as_str(), d["last_error"]["message"].as_str()), (Some("device_lost"), Some("Entrée « Micro test » perdue")));
        assert!(d["sample_rate"].is_null());
        // Read-only: no POST.
        assert_eq!(t.request("POST", "/api/audio/diagnostics", "{}").0, 404);
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
        let fake = DeviceInfo::new("helios".into(), "helios:left".into(), "Helios DAC Left (1:2.3)".into());
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
        let dev = DeviceInfo::new("etherdream".into(), "etherdream:a".into(), "Ether Dream a".into());
        let backend = FakeBackend::new(vec![dev.clone()], None, None, crate::test_support::scratch_dir());
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
            let backend = crate::output_manager::FakeBackend::new(Vec::new(), None, None, crate::test_support::scratch_dir());
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
        assert!(page.contains("/ui/app.js") && page.contains("/ui/styles.css"), "the page loads its modules and styles from our own server");
        assert!(static_asset("/ui/view3d.js").unwrap().0.contains("import('/beam3d.js')"), "the 3D view comes from our own server");

        // Browsers refuse ES modules served with a non-JavaScript type.
        let (module, module_type) = static_asset("/beam3d.js").unwrap();
        assert!(module_type.starts_with("text/javascript"));
        assert!(module.contains("export class BeamView"));
        assert!(static_asset("/vendor/nothing.js").is_none());

        // T-461: the text table, served by the studio itself, before the main script.
        let (labels, labels_type) = static_asset("/labels.js").unwrap();
        assert!(labels_type.starts_with("text/javascript"));
        assert!(labels.contains("const LABELS = Object.freeze(") && labels.contains("function applyLabels("));
        assert!(page.contains("<script src=\"/labels.js\"></script>"));
    }

    /// T-434: the page is styled by classes (common components and
    /// utilities), never by inline style attributes. The markup outside
    /// the scripts has none; a script template may set one only for a
    /// computed value, each declaration starting with a whitelisted
    /// property fed by `${…}`.
    #[test]
    fn page_has_no_inline_styles_except_computed_ones() {
        const COMPUTED: [&str; 5] = ["height:${", "background:${", "left:${", "width:${", "border-left-color:${"];
        // Split the page into markup (outside <script>…</script>) and scripts.
        let (mut markup, mut scripts) = (String::new(), String::new());
        let mut rest = INDEX_HTML;
        while let Some(open) = rest.find("<script") {
            markup.push_str(&rest[..open]);
            let close = rest[open..].find("</script>").map(|i| open + i).expect("a closed script");
            scripts.push_str(&rest[open..close]);
            scripts.push('\n');
            rest = &rest[close..];
        }
        markup.push_str(rest);
        // The modules (T-435) hold the templates the single script had.
        scripts.push_str(&ui_scripts());
        assert!(!scripts.is_empty() && markup.contains("<header"));

        let static_styles: Vec<&str> = markup.match_indices("style=").map(|(i, _)| &markup[i..(i + 60).min(markup.len())]).collect();
        assert!(static_styles.is_empty(), "inline styles in the markup: {static_styles:#?}");

        let mut computed = 0;
        for (i, _) in scripts.match_indices("style=") {
            let after = &scripts[i + "style=".len()..];
            assert!(after.starts_with('"'), "a template style must be a quoted attribute: {}", &scripts[i..(i + 60).min(scripts.len())]);
            let value = &after[1..after[1..].find('"').map(|e| e + 1).expect("closed attribute")];
            for decl in value.split(';').map(str::trim).filter(|d| !d.is_empty()) {
                assert!(COMPUTED.iter().any(|ok| decl.starts_with(ok)), "not a computed value: style=\"{value}\"");
            }
            computed += 1;
        }
        assert!(computed > 0, "the timeline heads and swatches keep their computed styles");
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
        let listed = list["templates"].as_array().unwrap();
        assert_eq!(listed.iter().filter(|t| t["builtin"] == true).count(), crate::templates::builtin().len(), "the built-in pack first");
        let mine = listed.iter().find(|t| t["name"] == "Deux temps").unwrap();
        assert_eq!(mine["builtin"], false);
        assert!(listed[0]["auto_pages"].is_object() && listed[0]["description"].is_string());
        assert_eq!(t.request("POST", "/api/timeline/templates", r#"{"name":"Drop 16","events":[1],"overwrite":true}"#).0, 409, "a built-in name");
        assert_eq!(t.request("POST", "/api/timeline/templates/delete", r#"{"name":"Drop 16"}"#).0, 409, "a built-in stays");
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
        // T-166: a built-in, slots on their suggested pages, goes through the edit route.
        t.shared.lock().unwrap().timeline.show.as_mut().unwrap().tracks[1].locked = false;
        let montee = listed.iter().find(|t| t["name"] == "Montée 8 mesures").unwrap();
        let slots: serde_json::Map<String, serde_json::Value> = montee["auto_pages"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), json!({ "auto": v }))).collect();
        let req = json!({ "name": "Montée 8 mesures", "from": 32.0, "to": 64.0, "fit": "stretch", "slots": slots });
        let (status, body) = t.request("POST", "/api/timeline/templates/render", &req.to_string());
        assert_eq!(status, 200, "{body}");
        let (status, body) = t.request("POST", "/api/timeline/edit", &json!({ "show": json(&body)["show"] }).to_string());
        assert_eq!(status, 200, "{body}");
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

    /// T-289: site profiles and packages over HTTP: a site saved and
    /// loaded (disarmed), a looser one refused without confirmation, a
    /// package downloaded as a zip, a « zip slip » package refused, all of
    /// it locked in show mode. Nothing arms.
    #[test]
    fn site_profiles_and_packages_over_http() {
        let t = TestServer::start(false);
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).unwrap();
        let v = json(&t.request("GET", "/api/sites", "").1);
        assert!(v["sites"].as_array().unwrap().is_empty() && v["project_site"].is_null());
        assert_eq!(t.request("POST", "/api/sites", r#"{"action":"save","name":"Club"}"#).0, 200);
        assert_eq!(t.request("POST", "/api/sites", r#"{"action":"load","name":"Club","source":"midi"}"#).0, 403);
        let v = json(&t.request("POST", "/api/sites", r#"{"action":"load","name":"Club"}"#).1);
        assert_eq!(v["active"], "Club");
        assert_eq!(t.request("POST", "/api/sites", r#"{"action":"load","name":"Nulle part"}"#).0, 400);
        assert_eq!(t.request("POST", "/api/sites/use-project", "{}").0, 404, "no site in the project");
        // A looser site: 409 with the list, nothing changed.
        {
            let mut s = t.shared.lock().unwrap();
            let mut cfg = s.safety.get();
            cfg.zones = vec![crate::zones::Zone { id: 1, name: "Z1".into(), kind: crate::zones::ZoneKind::Blank, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, -0.5]], ..Default::default() }];
            s.safety.set(cfg, true).unwrap();
        }
        let (code, body) = t.request("POST", "/api/sites", r#"{"action":"load","name":"Club"}"#);
        assert_eq!(code, 409, "{body}");
        assert!(!json(&body)["loosen"].as_array().unwrap().is_empty());
        assert_eq!(t.shared.lock().unwrap().safety.get().zones.len(), 1);
        // The package download.
        let mut stream = TcpStream::connect(t.addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET /api/pack/export?site=1 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
        assert!(head.starts_with("http/1.1 200") && head.contains("application/zip") && head.contains(".lspack"), "{head}");
        assert!(raw[split + 4..].starts_with(b"PK"));
        // A package with `../`: refused, nothing written.
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("../../evil.lsproj", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"{}").unwrap();
        let evil = zip.finish().unwrap().into_inner();
        let (code, body) = http_bytes(t.addr, "/api/pack/import", &evil);
        assert_eq!(code, 400);
        assert!(body.contains("chemin interdit"), "{body}");
        assert_eq!(http_bytes(t.addr, "/api/pack/import", b"not a zip").0, 400);
        // Show mode: every change refused.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(http_bytes(t.addr, "/api/pack/import", &evil).0, 423);
        assert_eq!(t.request("POST", "/api/sites", r#"{"action":"load","name":"Club","confirm_loosen":true}"#).0, 423);
        assert_eq!(t.request("POST", "/api/sites/use-project", "{}").0, 423);
        assert_eq!(t.request("POST", "/api/project/import-partial", r#"{"from":"x","sections":["midi"]}"#).0, 423);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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
    fn safety_sheet_reads_only_and_works_in_show_mode() {
        let t = TestServer::start(false);
        let snapshot = |t: &TestServer| {
            let s = t.shared.lock().unwrap();
            (s.safety.get(), s.outputs.list().to_vec(), s.checklist.view(), s.gate.is_armed())
        };
        let before = snapshot(&t);
        let (head, body) = get_bytes(t.addr, "/api/safety/sheet");
        let html = String::from_utf8(body).unwrap();
        assert!(head.contains("200"), "{head}");
        assert!(head.contains("text/html") && head.contains("Content-Security-Policy: default-src 'none'"), "{head}");
        assert!(html.contains("Fiche sécurité du show") && html.contains(&crate::safetysheet::esc(crate::safetysheet::FOOTER_FR)));
        let phone = "+32 499 12 34 56";
        let req = format!(r#"{{"event":{{"name":"Test","phone":"{phone}","dates":[{{"day":"10/10","start":"21h","end":"23h"}}]}},"include_log":true}}"#);
        let (code, html) = t.request("POST", "/api/safety/sheet", &req);
        assert_eq!(code, 200, "{html}");
        assert!(html.contains(phone) && html.contains("Journal de sécurité du jour"));
        assert_eq!(t.request("POST", "/api/safety/sheet", r#"{"event":{"name":"a\nb"}}"#).0, 400);
        assert_eq!(t.request("POST", "/api/safety/sheet", "pas du json").0, 400);
        // Reading only: allowed in show mode, changes nothing, never arms.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/safety/sheet", &req).0, 200);
        t.shared.lock().unwrap().show_mode.on = false;
        assert_eq!(snapshot(&t), before);
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

    /// T-463: the fixed frame rate through its own route: on, changed and
    /// off disarmed; on or changed refused armed, off always applies;
    /// out of range or not a number refused; the scan limits route keeps
    /// it; the view gives the choices and the points of one frame.
    #[test]
    fn fixed_rate_over_http() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let g = js(&t.request("GET", "/api/outputs/scan_limits", "").1);
        assert!(g["fixed"]["fps"].is_null() && g["fixed"]["points"].is_null(), "{g}");
        assert_eq!(g["fixed"]["choices"], json!([24.0, 25.0, 30.0, 50.0, 60.0]));
        let (code, reply) = t.request("POST", "/api/outputs/fixed_rate", r#"{"fps":25}"#);
        assert_eq!(code, 200, "{reply}");
        assert_eq!((js(&reply)["fixed"]["fps"].as_f64(), js(&reply)["fixed"]["points"].as_u64()), (Some(25.0), Some(1_200)));
        // The engine's measure is in the state (null until a frame ran at the rate).
        assert!(js(&t.request("GET", "/api/state", "").1).get("fixed_rate").is_some());
        for bad in [r#"{"fps":10}"#, r#"{"fps":61}"#, r#"{"fps":"abc"}"#, r#"{"fps":1e40}"#] {
            assert_eq!(t.request("POST", "/api/outputs/fixed_rate", bad).0, 400, "{bad}");
        }
        assert_eq!(t.request("POST", "/api/outputs/fixed_rate?output=nope", r#"{"fps":25}"#).0, 400);
        // The scan limits route leaves it as it is.
        assert_eq!(t.request("POST", "/api/outputs/scan_limits", r#"{"scan":{"min_points":300}}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active().scan.fixed_fps, Some(25.0));
        // Armed: changing it refused, the same value or off applies.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        let (code, reply) = t.request("POST", "/api/outputs/fixed_rate", r#"{"fps":30}"#);
        assert_eq!(code, 409, "{reply}");
        assert_eq!(js(&reply)["armed"], true);
        assert_eq!(t.request("POST", "/api/outputs/fixed_rate", r#"{"fps":25}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/fixed_rate", r#"{"fps":null}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active().scan.fixed_fps, None);
        assert_eq!(t.request("POST", "/api/outputs/fixed_rate", r#"{"fps":25}"#).0, 409);
        assert!(t.shared.lock().unwrap().gate.is_armed(), "a refusal never disarms");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/outputs/fixed_rate", r#"{}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().outputs.active().scan.fixed_fps, None);
        // The caps are untouched.
        assert_eq!(t.shared.lock().unwrap().outputs.active_limits(), crate::power::OutputLimits::default());
    }

    /// T-262: the outdoor mode through `POST /api/safety` (on: at once;
    /// off or wider: confirmed), the arming lock without the DGTA
    /// reference, « Ciel coupé » (cut from anywhere, restored by the
    /// operator only, allowed in show mode) and the sectors' export.
    #[test]
    fn outdoor_mode_over_http() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let mut cfg = js(&t.request("GET", "/api/safety", "").1)["settings"].clone();
        assert_eq!(cfg["outdoor"]["enabled"], false);
        assert_eq!(cfg["outdoor"]["max_from_vertical_deg"], 45.0);
        cfg["outdoor"]["enabled"] = json!(true);
        cfg["outdoor"]["sectors"] = json!([{ "az_min": 350.0, "az_max": 10.0, "el_min": 45.0, "el_max": 80.0 }]);
        let (code, reply) = t.request("POST", "/api/safety", &cfg.to_string());
        assert_eq!(code, 200, "turning it on tightens: {reply}");
        // No reference: arming refused, with the reason.
        let (code, reply) = t.request("POST", "/api/arm", r#"{"on":true}"#);
        assert_eq!(code, 409, "{reply}");
        assert!(reply.contains("DGTA"), "{reply}");
        cfg["outdoor"]["authorization_ref"] = json!("BCAA-2026-42");
        assert_eq!(t.request("POST", "/api/safety", &cfg.to_string()).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        // Wider sector or the mode off: confirmation needed.
        let mut wider = cfg.clone();
        wider["outdoor"]["sectors"][0]["az_max"] = json!(20.0);
        let (code, reply) = t.request("POST", "/api/safety", &wider.to_string());
        assert_eq!(code, 409, "{reply}");
        let mut off = cfg.clone();
        off["outdoor"]["enabled"] = json!(false);
        assert_eq!(t.request("POST", "/api/safety", &off.to_string()).0, 409);
        // Untrusted values refused.
        let mut bad = cfg.clone();
        bad["outdoor"]["sectors"][0]["el_min"] = json!(95.0);
        assert_eq!(t.request("POST", "/api/safety", &bad.to_string()).0, 400);
        // A runtime field in the body is ignored.
        let mut sneaky = cfg.clone();
        sneaky["outdoor"]["sky_off"] = json!(true);
        assert_eq!(t.request("POST", "/api/safety", &sneaky.to_string()).0, 200);
        assert!(!t.shared.lock().unwrap().sky_off);
        // The export: what was entered.
        let (code, csv) = t.request("GET", "/api/safety/outdoor/export", "");
        assert_eq!(code, 200);
        assert_eq!(csv.lines().nth(1), Some("1;350;10;45;80"), "{csv}");
        // « Ciel coupé »: from a controller it cuts, never restores.
        t.shared.lock().unwrap().show_mode.on = true;
        let (code, reply) = t.request("POST", "/api/safety/sky", r#"{"on":true,"source":"midi"}"#);
        assert_eq!(code, 200, "{reply}");
        assert_eq!(js(&reply)["sky_off"], true);
        assert_eq!(t.request("POST", "/api/safety/sky", r#"{"on":false,"source":"osc"}"#).0, 403);
        assert!(t.shared.lock().unwrap().sky_off);
        let (code, reply) = t.request("POST", "/api/safety/sky", r#"{"toggle":true,"source":"keyboard"}"#);
        assert_eq!(code, 200);
        assert_eq!(js(&reply)["sky_off"], false);
        assert!(!t.shared.lock().unwrap().gate.is_armed(), "never arms");
        // Show mode: the settings themselves are locked.
        assert_eq!(t.request("POST", "/api/safety", &cfg.to_string()).0, 423);
        let state = js(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["outdoor"]["enabled"], true);
        assert_eq!(state["outdoor"]["authorized"], true);
    }

    /// T-477: another projector's mounting through `POST /api/safety`.
    /// Declaring or changing one is a loosening (409, then confirmed);
    /// removing one only cuts more; a value that is not a number or out of
    /// range is refused; show mode locks it. `/api/state` gives each venue
    /// projector's cut for the 3D view (the whole field without a mounting).
    #[test]
    fn outdoor_mountings_per_projector_over_http() {
        let t = TestServer::start(false);
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        {
            let mut s = t.shared.lock().unwrap();
            let mut venue = s.venue.get().clone();
            venue.projectors.push(crate::venue::Projector { id: "p2".into(), name: "Jardin".into(), ..Default::default() });
            s.venue.set(venue).unwrap();
        }
        let mut cfg = js(&t.request("GET", "/api/safety", "").1)["settings"].clone();
        cfg["outdoor"]["enabled"] = json!(true);
        cfg["outdoor"]["authorization_ref"] = json!("DGTA-1");
        cfg["outdoor"]["sectors"] = json!([{ "az_min": 0.0, "az_max": 360.0, "el_min": -90.0, "el_max": 30.0 }]);
        assert_eq!(t.request("POST", "/api/safety", &cfg.to_string()).0, 200);
        // No mounting for p2: its whole field is cut.
        let state = js(&t.request("GET", "/api/state", "").1);
        let p2 = state["outdoor"]["cuts"]["p2"].as_array().unwrap().clone();
        assert_eq!(p2.len(), 1, "{p2:?}");
        // Declared: light gets out, so it waits for the confirmation.
        let mut with = cfg.clone();
        with["outdoor"]["mountings"] = json!({ "p2": { "scan_angle_deg": 40.0, "tilt_deg": -30.0, "azimuth_deg": 90.0, "height_m": 3.0 } });
        let (code, reply) = t.request("POST", "/api/safety", &with.to_string());
        assert_eq!(code, 409, "{reply}");
        assert!(reply.contains("« p2 » déclaré"), "{reply}");
        let mut confirmed = with.clone();
        confirmed["confirm_loosen"] = json!(true);
        assert_eq!(t.request("POST", "/api/safety", &confirmed.to_string()).0, 200);
        assert_eq!(t.shared.lock().unwrap().safety.get().outdoor.mountings["p2"].tilt_deg, -30.0);
        // Pointing below the horizon and within the sector: nothing cut on p2.
        let state = js(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["outdoor"]["cuts"]["p2"], json!([]), "{}", state["outdoor"]);
        // Moved: confirmed again.
        let mut moved = with.clone();
        moved["outdoor"]["mountings"]["p2"]["tilt_deg"] = json!(10.0);
        assert_eq!(t.request("POST", "/api/safety", &moved.to_string()).0, 409);
        // Not a number, out of range or missing values: refused.
        for bad in [json!("NaN"), json!(200.0), json!(null)] {
            let mut b = with.clone();
            b["outdoor"]["mountings"]["p2"]["tilt_deg"] = bad.clone();
            let code = t.request("POST", "/api/safety", &b.to_string()).0;
            assert_eq!(code, 400, "{bad}");
        }
        assert_eq!(t.shared.lock().unwrap().safety.get().outdoor.mountings["p2"].tilt_deg, -30.0, "unchanged");
        // Removed: p2 fully cut again, no confirmation needed.
        assert_eq!(t.request("POST", "/api/safety", &cfg.to_string()).0, 200);
        assert!(t.shared.lock().unwrap().safety.get().outdoor.mountings.is_empty());
        // Show mode: locked.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/safety", &confirmed.to_string()).0, 423);
        assert!(t.shared.lock().unwrap().safety.get().outdoor.mountings.is_empty());
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

    #[test]
    fn arming_with_the_backup_unreachable_needs_the_confirmation_over_http() {
        let t = TestServer::start(false);
        {
            let mut s = t.shared.lock().unwrap();
            s.standby.settings.role = crate::standby::Role::Primary;
            s.standby.state.lease_lost_at = Some(Instant::now());
        }
        for body in [r#"{"on":true}"#, r#"{"toggle":true,"source":"keyboard"}"#, r#"{"on":true,"confirm_backup_lost":false}"#] {
            let (status, reply) = t.request("POST", "/api/arm", body);
            assert_eq!(status, 409, "{body}: {reply}");
            let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(v["armed"], false);
            assert_eq!(v["blocking"][0], crate::standby::BACKUP_LOST_ARM_FR);
            assert_eq!(v["confirm_backup_lost"], crate::standby::BACKUP_LOST_CONFIRM_FR);
            assert!(!t.shared.lock().unwrap().gate.is_armed());
        }
        // Disarming never asks.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"confirm_backup_lost":true}"#).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed());
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
        // The backup polls again: no confirmation.
        t.shared.lock().unwrap().standby.state.lease_lost_at = None;
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
    }

    /// The confirmation is logged only once the gate has armed: a
    /// confirmed request that an interlock (here the e-stop) still refuses
    /// leaves no « confirmed » line, and a request while already armed
    /// adds none.
    #[test]
    fn a_confirmed_arm_with_the_backup_unreachable_is_logged_only_when_it_arms() {
        let t = TestServer::start(false);
        let (log, sink) = crate::safety_log::testing::memory_log();
        {
            let mut s = t.shared.lock().unwrap();
            s.gate.set_log(log);
            s.standby.settings.role = crate::standby::Role::Primary;
            s.standby.state.lease_lost_at = Some(Instant::now());
            s.estop.trip(ArmSource::Ui);
        }
        let flush = || t.shared.lock().unwrap().gate.log().flush(std::time::Duration::from_secs(2));
        let confirmed = |sink: &crate::safety_log::testing::MemorySink| sink.kinds().iter().filter(|k| *k == "standby_arm_backup_lost").count();
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"confirm_backup_lost":true}"#).0, 409);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
        flush();
        assert_eq!(confirmed(&sink), 0, "refused by the e-stop: nothing confirmed");
        {
            let mut s = t.shared.lock().unwrap();
            let s = &mut *s;
            s.gate.reset_estop(&s.estop);
        }
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true,"confirm_backup_lost":true}"#).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed());
        flush();
        assert_eq!(confirmed(&sink), 1);
        // Already armed: a second request (unconfirmed, nothing to
        // confirm) changes nothing and logs nothing more.
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":true}"#).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed());
        flush();
        assert_eq!(confirmed(&sink), 1, "a request while armed is not a new confirmation");
        assert_eq!(t.request("POST", "/api/arm", r#"{"on":false}"#).0, 200);
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

    /// T-170: zone chasers are edited with the zones (an edit: in the
    /// history, 423 in show mode), played through the controls (allowed in
    /// show mode), tied to a cue in « Propriétés du cue »; nothing arms.
    #[test]
    fn zone_chasers_over_the_api() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let v = js(&t.request("GET", "/api/zone-chase", "").1);
        assert_eq!(v["chasers"], json!([]));
        assert_eq!((v["rate"].as_u64(), v["max_chasers"].as_u64()), (Some(2), Some(16)));
        let cfg = json!({
            "zones": [{ "id": 1 }, { "id": 2 }, { "id": 3 }],
            "default_zones": [1, 2, 3],
            "groups": [{ "name": "Gauche", "zones": [1] }, { "name": "Centre", "zones": [2] }, { "name": "Droite", "zones": [3] }],
            "chasers": [{ "name": "Balayage", "steps": [0, 1, 2], "step_beats": 1, "mode": "bounce" }]
        });
        let (code, body) = t.request("POST", "/api/projection", &cfg.to_string());
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Zones de projection modifiées");
        let v = js(&t.request("GET", "/api/zone-chase", "").1);
        assert_eq!(v["chasers"][0]["name"], "Balayage");
        assert_eq!(v["chasers"][0]["control"], "chaser.1.enabled");
        assert_eq!(v["chasers"][0]["enabled"], false);
        // A step on a group that doesn't exist: refused.
        let mut bad = cfg.clone();
        bad["chasers"][0]["steps"] = json!([0, 3]);
        assert_eq!(t.request("POST", "/api/projection", &bad.to_string()).0, 400);
        // Only the chasers changed: the history says so.
        let mut faster = cfg.clone();
        faster["chasers"][0]["step_beats"] = json!(0.5);
        assert_eq!(t.request("POST", "/api/projection", &faster.to_string()).0, 200);
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Chenillards de zones modifiés");
        // Played live, then the edit is undone: the chaser keeps running.
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"chaser.1.enabled","value":1}"#).0, 200);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"chaser.rate","value":3}"#).0, 200);
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 200);
        let p = t.shared.lock().unwrap().projection.get().clone();
        assert_eq!(p.chasers[0].step_beats, 1.0, "undone");
        assert!(p.chasers[0].enabled, "a playing value: kept");
        assert_eq!(p.chase_rate, 2.0);
        assert_eq!(js(&t.request("GET", "/api/frame", "").1)["layers"]["chase"]["chasers"][0]["enabled"], true);
        // A cue tied to chaser 1; one past the most there can be is refused.
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-001","zone_chaser":0}"#).0, 200);
        assert_eq!(js(&t.request("GET", "/api/cues", "").1)["slots"]["tunnels-001"]["zone_chaser"], 0);
        assert_eq!(t.request("POST", "/api/cues/slot", r#"{"id":"tunnels-001","zone_chaser":16}"#).0, 400);
        // Show mode: editing is locked, playing is not.
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/projection", &faster.to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/control", r#"{"id":"chaser.1.enabled","value":0}"#).0, 200);
        assert_eq!(t.shared.lock().unwrap().projection.chaser_enabled(1), Some(false));
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-460: a phone's alignment is opened and applied from the main
    /// screen only; both are refused in show mode and under the safety
    /// lock, rejecting never is; applying makes one undo entry.
    #[test]
    fn phone_alignment_is_confirmed_on_the_main_screen_only() {
        let t = TestServer::start(false);
        let cookie = {
            let mut s = t.shared.lock().unwrap();
            s.remote.state = crate::remote::RemoteState { enabled: true, code: "123456".into(), ..Default::default() };
            let r = crate::remote::handle(&mut s, "POST", "/pair", None, "10.0.0.9", r#"{"code":"123456","name":"Téléphone"}"#.as_bytes(), Instant::now());
            r.set_cookie.unwrap().split(';').next().unwrap().to_string()
        };
        let nudge = |n: usize| {
            let mut s = t.shared.lock().unwrap();
            for _ in 0..n {
                let r = crate::remote::handle(&mut s, "POST", "/api/remote/align/nudge", Some(&cookie), "10.0.0.9", br#"{"corner":2,"dx":-1,"dy":-1}"#, Instant::now());
                assert_eq!(r.status, 200, "{}", r.body);
            }
        };
        let saved = t.shared.lock().unwrap().projection.get().clone();
        assert_eq!(t.request("POST", "/api/zones/align/apply", "{}").0, 409, "no session");
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone","x":1}"#).0, 400);
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":7,"remote":"Téléphone"}"#).0, 404);
        let (code, body) = t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#);
        assert_eq!(code, 200, "{body}");
        nudge(3);
        let st = js(&t.request("GET", "/api/state", "").1);
        assert_eq!(st["align"]["remote"], "Téléphone");
        assert_eq!(st["align"]["changed"], true);
        assert_eq!(t.shared.lock().unwrap().projection.get(), &saved);
        // Rejected: nothing written, the session gone.
        assert_eq!(t.request("POST", "/api/zones/align/reject", "").0, 200);
        assert!(js(&t.request("GET", "/api/state", "").1)["align"].is_null());
        assert_eq!(t.shared.lock().unwrap().projection.get(), &saved);
        // Show mode: no opening, no applying; closing always.
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#).0, 200);
        nudge(2);
        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/zones/align/apply", "").0, 423);
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#).0, 423);
        assert_eq!(t.request("POST", "/api/zones/align/close", "").0, 200);
        t.shared.lock().unwrap().show_mode.on = false;
        // The safety lock: same.
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#).0, 200);
        nudge(2);
        t.shared.lock().unwrap().safety_lock.set_code(crate::safetylock::new_file("5555", 10).unwrap(), Instant::now()).unwrap();
        assert_eq!(t.request("POST", "/api/zones/align/apply", "").0, 423);
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#).0, 423);
        assert_eq!(t.shared.lock().unwrap().projection.get(), &saved);
        assert_eq!(t.request("POST", "/api/zones/align/reject", "").0, 200);
        // No code any more (as after a reset on the Mac itself).
        t.shared.lock().unwrap().safety_lock = crate::safetylock::SafetyLock::in_memory();
        // Applied from the main screen: written, one undo entry.
        assert_eq!(t.request("POST", "/api/zones/align/open", r#"{"zone":1,"remote":"Téléphone"}"#).0, 200);
        nudge(3);
        let (code, body) = t.request("POST", "/api/zones/align/apply", "");
        assert_eq!(code, 200, "{body}");
        let z = t.shared.lock().unwrap().projection.get().zone(1).unwrap().clone();
        assert!(z.geometry.warp.enabled && (z.geometry.warp.corners[2][0] - (1.0 - 3.0 * crate::remote::NUDGE_STEP)).abs() < 1e-5);
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"][0]["label"], "Zones de projection modifiées");
        assert!(!t.shared.lock().unwrap().gate.is_armed());
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

    /// T-440: the abstract editor's preview never touches what plays; « Enregistrer
    /// comme cue… » makes a user cue (in the history, undone, 423 in show
    /// mode) that plays the same abstract.
    #[test]
    fn the_abstract_editor_previews_and_stores_cues() {
        let js = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
        let t = TestServer::start(false);
        let before = js(&t.request("GET", "/api/state", "").1);
        let shape = json!({ "kind": "abstract", "points": 2000,
            "oscs": [{ "target": "x", "ratio": 3, "phase_deg": 90 }, { "target": "y", "ratio": 2 }] });
        let look = json!({ "content": shape, "color": [0, 255, 0], "scale": 0.6, "brightness": 1.0 });
        let (code, body) = t.request("POST", "/api/abstracts/preview", &json!({ "look": look }).to_string());
        assert_eq!(code, 200, "{body}");
        let p = js(&body);
        assert_eq!((p["budget"].clone(), p["requested"].clone(), p["limited"].clone()), (json!(750), json!(2000), json!(true)));
        assert_eq!(p["samples"], json!(749), "one point kept for the closing one");
        assert_eq!(p["frames"].as_array().unwrap().len(), 1, "a still figure: one picture");
        assert!(p["points"].as_u64().unwrap() <= 750);
        let mut drifting = look.clone();
        drifting["content"]["oscs"][0]["drift"] = json!(0.25);
        let p = js(&t.request("POST", "/api/abstracts/preview", &json!({ "look": drifting }).to_string()).1);
        let frames = p["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 32);
        assert_ne!(frames[0], frames[1], "the drift animates it");
        let after = js(&t.request("GET", "/api/state", "").1);
        assert_eq!((after["settings"].clone(), after["settings_rev"].clone()), (before["settings"].clone(), before["settings_rev"].clone()), "the preview leaves the look alone");
        assert_eq!(t.request("POST", "/api/abstracts/preview", r#"{"look":{"content":{"kind":"wave"}}}"#).0, 400);

        // Stored: a user cue holding the abstract, sanitized.
        let mut wild = look.clone();
        wild["content"]["oscs"][1]["ratio"] = json!(99.0);
        let (code, body) = t.request("POST", "/api/abstracts/store", &json!({ "look": wild, "name": "Mon abstrait" }).to_string());
        assert_eq!(code, 200, "{body}");
        let id = js(&body)["cue"].as_str().unwrap().to_string();
        {
            let s = t.shared.lock().unwrap();
            let cue = s.user_cues.get(&id).unwrap();
            assert_eq!(cue.name, "Mon abstrait");
            let crate::engine::Content::Abstract(a) = &cue.settings.content else { panic!("{:?}", cue.settings.content) };
            assert_eq!((a.oscs[0].ratio, a.oscs[1].ratio), (3.0, crate::abstracts::MAX_RATIO));
            assert!(s.presets.iter().any(|p| p.id == id));
        }
        assert_eq!(js(&t.request("GET", "/api/history", "").1)["undo"].as_array().unwrap().len(), 1);
        assert_eq!(t.request("POST", "/api/undo", "").0, 200);
        assert!(t.shared.lock().unwrap().user_cues.get(&id).is_none(), "undone");
        assert_eq!(t.request("POST", "/api/abstracts/store", r#"{"look":{"content":{"kind":"wave"}}}"#).0, 400);

        t.shared.lock().unwrap().show_mode.on = true;
        assert_eq!(t.request("POST", "/api/abstracts/store", &json!({ "look": look }).to_string()).0, 423);
        assert_eq!(t.request("POST", "/api/abstracts/preview", &json!({ "look": look }).to_string()).0, 423);
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
        // Other threads (the server's workers) take the lock for a moment
        // now and then: it must come free soon, not be held by the write.
        let started = Instant::now();
        let free = (0..100).any(|_| t.shared.try_lock().is_ok() || {
            std::thread::sleep(Duration::from_millis(5));
            false
        });
        assert!(free, "no lock held by the blocked write");
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

    /// T-457: tracking settings through the API. Switching on or adding a
    /// link needs the confirmation, is refused armed, under the safety
    /// lock and in show mode; switching off always runs; the test hook
    /// only exists with `--test-hooks`; tracking data never arms and the
    /// mobile target shows in `/api/targets`.
    #[test]
    fn psn_settings_need_a_confirmation_and_tracking_never_arms() {
        let t = TestServer::start(false);
        let post = |b: &serde_json::Value| t.request("POST", "/api/psn", &b.to_string());
        let v = js(&t.request("GET", "/api/psn", "").1);
        assert_eq!(v["settings"]["enabled"], false, "off by default");
        assert_eq!((v["settings"]["group"].as_str(), v["settings"]["port"].as_u64()), (Some("236.10.10.10"), Some(56565)));
        let refs = json!([
            { "tracker": [0.0, 0.0, 0.0], "output": [-0.5, 0.2] }, { "tracker": [4.0, 0.0, 0.0], "output": [0.5, 0.2] },
            { "tracker": [4.0, 2.0, 0.0], "output": [0.5, 0.8] }, { "tracker": [0.0, 2.0, 0.0], "output": [-0.5, 0.8] }
        ]);
        let on = json!({ "enabled": true, "port": 0, "smoothing_ms": 0, "links": [{ "tracker_id": 7, "target": "Chariot", "refs": refs }] });
        let (code, body) = post(&on);
        assert_eq!(code, 409, "{body}");
        assert!(js(&body)["loosen"].as_array().unwrap().len() >= 2);
        let mut confirmed = on.clone();
        confirmed["confirm_loosen"] = json!(true);
        let (code, body) = post(&confirmed);
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["calibration"][0]["ok"], true);
        assert_eq!(post(&json!({ "enabled": true, "group": "10.1.1.1", "confirm_loosen": true })).0, 400);
        // The test hook: 404 without --test-hooks.
        let pos = json!({ "trackers": [{ "id": 7, "pos": [2.0, 1.0, 0.0], "name": "Chariot" }] });
        assert_eq!(t.request("POST", "/api/test/psn", &pos.to_string()).0, 404);
        t.shared.lock().unwrap().test_hooks = true;
        assert_eq!(t.request("POST", "/api/test/psn", &pos.to_string()).0, 200);
        // Gate closed: listed, dark.
        crate::psn::frame(&mut t.shared.lock().unwrap(), Instant::now());
        let v = js(&t.request("GET", "/api/psn/trackers", "").1);
        assert_eq!(v["trackers"][0]["name"], "Chariot");
        assert_eq!(v["links"][0]["state"], "entrées fermées");
        // Gate open: the first sample waits for a confirmation (T-492),
        // then lit, and the mirror targets list it after the fixed ones.
        crate::external::set_open(&mut t.shared.lock().unwrap(), true, crate::interlock::ArmSource::Ui).unwrap();
        assert_eq!(t.request("POST", "/api/test/psn", &pos.to_string()).0, 200);
        crate::psn::frame(&mut t.shared.lock().unwrap(), Instant::now());
        let v = js(&t.request("GET", "/api/psn/trackers", "").1);
        assert_eq!((v["links"][0]["state"].as_str(), v["links"][0]["lit"].as_bool()), (Some("saut"), Some(false)), "{v}");
        assert_eq!(t.request("POST", "/api/test/psn", &pos.to_string()).0, 200);
        crate::psn::frame(&mut t.shared.lock().unwrap(), Instant::now());
        let v = js(&t.request("GET", "/api/psn/trackers", "").1);
        assert_eq!(v["links"][0]["state"], "suivi", "{v}");
        let tv = js(&t.request("GET", "/api/targets", "").1);
        assert_eq!((tv["mobile"][0]["name"].as_str(), tv["mobile"][0]["bit"].as_u64(), tv["mobile"][0]["lit"].as_bool()), (Some("Chariot"), Some(0), Some(true)));
        assert!(!t.shared.lock().unwrap().gate.is_armed());
        // Armed: a new link refused, switching off runs.
        t.shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        let mut two = confirmed.clone();
        two["links"].as_array_mut().unwrap().push(json!({ "tracker_id": 8, "target": "Décor", "refs": refs }));
        let (code, body) = post(&two);
        assert_eq!(code, 409, "{body}");
        assert_eq!(js(&body)["armed"], true);
        assert_eq!(post(&json!({ "enabled": false, "links": confirmed["links"] })).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed(), "never disarms either");
        t.shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        // Show mode: refused.
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": true }).to_string()).0, 200);
        assert_eq!(post(&confirmed).0, 423);
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": false }).to_string()).0, 200);
        // Safety lock: switching on refused, the rest (off) runs.
        assert_eq!(t.request("POST", "/api/safety/lock", &json!({ "action": "set", "code": "2468" }).to_string()).0, 200);
        assert_eq!(post(&confirmed).0, 423);
        assert_eq!(post(&json!({ "enabled": false })).0, 200);
        assert!(!t.shared.lock().unwrap().psn.settings().enabled);
    }

    /// T-457: mobile targets are numbered after the fixed ones, so with a
    /// tracking link, removing a fixed target (even the last one) moves
    /// light to another target: confirmation needed, refused armed and in
    /// show mode. Without links it still always runs.
    #[test]
    fn removing_a_fixed_target_with_tracking_links_is_a_loosening() {
        let t = TestServer::start(false);
        let post = |b: &serde_json::Value| t.request("POST", "/api/targets", &b.to_string());
        let two = json!({ "items": [{ "name": "A", "x": 0.2, "y": 0.5 }, { "name": "B", "x": -0.3, "y": 0.6 }], "confirm_loosen": true });
        assert_eq!(post(&two).0, 200);
        let one = json!({ "items": [{ "name": "A", "x": 0.2, "y": 0.5 }] });
        // No link: removing the last one runs armed.
        t.shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        assert_eq!(post(&one).0, 200);
        t.shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        assert_eq!(post(&two).0, 200);
        let refs = json!([
            { "tracker": [0.0, 0.0, 0.0], "output": [-0.5, 0.2] }, { "tracker": [4.0, 0.0, 0.0], "output": [0.5, 0.2] },
            { "tracker": [4.0, 2.0, 0.0], "output": [0.5, 0.8] }, { "tracker": [0.0, 2.0, 0.0], "output": [-0.5, 0.8] }
        ]);
        let link = json!({ "enabled": true, "port": 0, "links": [{ "tracker_id": 7, "target": "Chariot", "refs": refs }], "confirm_loosen": true });
        assert_eq!(t.request("POST", "/api/psn", &link.to_string()).0, 200);
        // With a link: unconfirmed → 409 with the reason.
        let (code, body) = post(&one);
        assert_eq!(code, 409, "{body}");
        assert!(js(&body)["loosen"][0].as_str().unwrap().contains("mobile"), "{body}");
        // Armed: refused, confirmed or not; nothing changed, still armed.
        t.shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        let mut confirmed = one.clone();
        confirmed["confirm_loosen"] = json!(true);
        let (code, body) = post(&confirmed);
        assert_eq!(code, 409, "{body}");
        assert_eq!(js(&body)["armed"], true);
        assert_eq!(t.shared.lock().unwrap().targets.get().items.len(), 2);
        assert!(t.shared.lock().unwrap().gate.is_armed());
        // Adding one shifts them too.
        let three = json!({ "items": [{ "name": "A", "x": 0.2, "y": 0.5 }, { "name": "B", "x": -0.3, "y": 0.6 }, { "name": "C", "x": 0.0, "y": 0.7 }], "confirm_loosen": true });
        assert_eq!(post(&three).0, 409);
        t.shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        // Show mode: refused.
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": true }).to_string()).0, 200);
        assert_eq!(post(&confirmed).0, 423);
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": false }).to_string()).0, 200);
        // Renaming keeps the count: runs without confirmation.
        assert_eq!(post(&json!({ "items": [{ "name": "A2", "x": 0.2, "y": 0.5 }, { "name": "B", "x": -0.3, "y": 0.6 }] })).0, 200);
        // Disarmed and confirmed: runs.
        assert_eq!(post(&confirmed).0, 200);
        assert_eq!(t.shared.lock().unwrap().targets.get().items.len(), 1);
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-109: mirror targets through the API. A new or moved target needs
    /// the confirmation, is refused armed, under the safety lock and in
    /// show mode; one below the horizon is refused with a message; removing
    /// runs locked; nothing arms; the status says why a target is cut.
    #[test]
    fn mirror_targets_need_a_confirmation_and_never_loosen_locked() {
        let t = TestServer::start(false);
        let post = |b: &serde_json::Value| t.request("POST", "/api/targets", &b.to_string());
        let one = json!({ "items": [{ "name": "Boule", "x": 0.2, "y": 0.5 }] });
        let (code, body) = post(&one);
        assert_eq!(code, 409, "{body}");
        assert!(js(&body)["loosen"][0].as_str().unwrap().contains("ajoutée"));
        assert!(t.shared.lock().unwrap().targets.get().items.is_empty());
        let mut confirmed = one.clone();
        confirmed["confirm_loosen"] = json!(true);
        let (code, body) = post(&confirmed);
        assert_eq!(code, 200, "{body}");
        let v = js(&body);
        assert_eq!(v["items"][0]["name"], "Boule");
        assert_eq!(v["status"][0]["level"], 1.0);
        assert_eq!(v["max"], crate::targets::MAX_TARGETS);
        // Below the horizon: refused with a message, confirmed or not.
        let low = json!({ "items": [{ "name": "Boule", "x": 0.2, "y": 0.5 }, { "name": "Sol", "x": 0.0, "y": -0.4 }], "confirm_loosen": true });
        let (code, body) = post(&low);
        assert_eq!(code, 400);
        assert!(body.contains("sous l'horizon") && body.contains("Sol"), "{body}");
        // Bad values: 400.
        assert_eq!(post(&json!({ "items": [{ "name": "", "x": 0.0, "y": 0.5 }], "confirm_loosen": true })).0, 400);
        assert_eq!(post(&json!({ "items": [{ "name": "A", "x": 2.0, "y": 0.5 }], "confirm_loosen": true })).0, 400);
        // A Blank zone over it: kept, dark, and the status says so.
        let mut cfg = js(&t.request("GET", "/api/safety", "").1)["settings"].clone();
        cfg["zones"] = json!([{ "id": 1, "name": "Régie", "kind": "blank", "points": [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]] }]);
        assert_eq!(t.request("POST", "/api/safety", &cfg.to_string()).0, 200);
        let v = js(&t.request("GET", "/api/targets", "").1);
        assert_eq!(v["status"][0]["level"], 0.0);
        assert!(v["status"][0]["reasons"][0].as_str().unwrap().contains("Régie"), "{v}");
        // Armed: moving refused (409 armed), renaming runs; never disarms nor arms.
        t.shared.lock().unwrap().request_arm(crate::interlock::ArmSource::Ui).unwrap();
        let moved = json!({ "items": [{ "name": "Boule", "x": -0.3, "y": 0.5 }], "confirm_loosen": true });
        let (code, body) = post(&moved);
        assert_eq!(code, 409, "{body}");
        assert_eq!(js(&body)["armed"], true);
        assert_eq!(post(&json!({ "items": [{ "name": "Miroir", "x": 0.2, "y": 0.5 }] })).0, 200);
        assert!(t.shared.lock().unwrap().gate.is_armed());
        t.shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        assert_eq!(post(&moved).0, 200);
        // Show mode: every change refused.
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": true }).to_string()).0, 200);
        assert_eq!(post(&json!({ "items": [] })).0, 423);
        assert_eq!(t.shared.lock().unwrap().targets.get().items.len(), 1);
        assert_eq!(t.request("POST", "/api/lock", &json!({ "show_mode": false }).to_string()).0, 200);
        // Under the safety lock: adding refused (423), removing runs.
        assert_eq!(t.request("POST", "/api/safety/lock", &json!({ "action": "set", "code": "2468" }).to_string()).0, 200);
        let two = json!({ "items": [{ "name": "Miroir", "x": -0.3, "y": 0.5 }, { "name": "Mur", "x": 0.6, "y": 0.7 }], "confirm_loosen": true });
        let (code, body) = post(&two);
        assert_eq!(code, 423, "{body}");
        assert!(js(&body)["loosen"].as_array().is_some_and(|l| l.len() == 1));
        assert_eq!(post(&json!({ "items": [] })).0, 200);
        assert!(t.shared.lock().unwrap().targets.get().items.is_empty());
        assert!(!t.shared.lock().unwrap().gate.is_armed());
    }

    /// T-261: with the safety lock on, every Safety-class route refuses
    /// what loosens (423, nothing changed), while stopping and tightening
    /// keep working: e-stop, disarm, « Ciel coupé », above the audience,
    /// leaving a pattern, stopping the trace, cutting the remote, closing
    /// the external inputs, lower caps, a higher horizon.
    #[test]
    fn the_safety_lock_refuses_loosening_on_every_safety_route() {
        let t = TestServer::start(false);
        let post = |p: &str, b: &serde_json::Value| t.request("POST", p, &b.to_string());
        // Before locking: a tighter horizon, and a looser profile saved.
        let mut cfg = js(&t.request("GET", "/api/safety", "").1)["settings"].clone();
        assert_eq!(post("/api/safety/profiles", &json!({ "action": "save", "name": "Large" })).0, 200);
        cfg["horizon"]["y"] = json!(0.3);
        let (code, body) = post("/api/safety", &cfg);
        assert_eq!(code, 200, "{body}");
        let mut venue = js(&t.request("GET", "/api/venue", "").1)["venue"].clone();
        venue["projectors"][0]["name"] = json!("Cour");
        assert_eq!(post("/api/venue", &venue).0, 200);
        // Lock with a code.
        let (code, body) = post("/api/safety/lock", &json!({ "action": "set", "code": "2468" }));
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["locked"], true);
        assert!(!body.contains("2468"));
        let state = js(&t.request("GET", "/api/state", "").1);
        assert_eq!(state["lock"]["safety"]["locked"], true);
        let settings_before = serde_json::to_value(t.shared.lock().unwrap().safety.get()).unwrap();

        // Every Safety route the gate refuses outright.
        let concrete = |p: &str| match p {
            "/api/checklist/*" => "/api/checklist/items".to_string(),
            "/api/external*" => "/api/external/scopes".to_string(),
            "/api/dmx/*" => "/api/dmx/patch".to_string(),
            p => p.to_string(),
        };
        let mut refused = 0;
        for &(m, p, class) in ROUTES {
            if class != RouteClass::Safety || m == "GET" {
                continue;
            }
            let path = concrete(p);
            if crate::safetylock::passes_gate(&path) {
                continue;
            }
            let (code, body) = t.request(if m == "*" { "POST" } else { m }, &path, "{}");
            assert_eq!((code, body.as_str()), (423, crate::safetylock::LOCKED_FR), "{m} {path}");
            refused += 1;
        }
        assert!(refused >= 20, "{refused}");
        // Every route the lock lets through is a Safety route of the table.
        for p in crate::safetylock::ALWAYS.iter().chain(crate::safetylock::HANDLER) {
            assert_eq!(route_class("POST", p), Some(RouteClass::Safety), "{p}");
        }

        // The routes that check the direction: loosening refused...
        let mut looser = cfg.clone();
        looser["horizon"]["y"] = json!(0.1);
        looser["confirm_loosen"] = json!(true);
        let (code, body) = post("/api/safety", &looser);
        assert_eq!(code, 423, "{body}");
        assert!(js(&body)["loosen"].as_array().is_some_and(|l| !l.is_empty()));
        let (code, body) = post("/api/safety/profiles/load", &json!({ "name": "Large", "confirm_loosen": true }));
        assert_eq!(code, 423, "{body}");
        let high = json!({ "limits": { "max_power": 0.9, "max_color": [1.0, 1.0, 1.0] }, "confirm_loosen": true });
        assert_eq!(post("/api/outputs/limits", &high).0, 423);
        assert_eq!(post("/api/outputs/scan_limits", &json!({ "scan": { "pps_max": 60000 } })).0, 423);
        assert_eq!(post("/api/audience", &json!({ "mode": "scanning", "phrase": crate::audience::PHRASE })).0, 423);
        assert_eq!(post("/api/testpattern", &json!({ "pattern": "grid" })).0, 423);
        assert_eq!(post("/api/safety/trace", &json!({ "on": true })).0, 423);
        assert_eq!(post("/api/remote", &json!({ "enabled": true, "port": 0 })).0, 423);
        assert_eq!(post("/api/external/gate", &json!({ "on": true })).0, 423);
        assert_eq!(serde_json::to_value(t.shared.lock().unwrap().safety.get()).unwrap(), settings_before, "nothing loosened");
        // Undoing the venue change is refused too.
        assert_eq!(t.request("POST", "/api/undo", "{}").0, 423);
        assert_eq!(t.shared.lock().unwrap().venue.get().projectors[0].name, "Cour");
        assert_eq!(js(&t.request("GET", "/api/remote", "").1)["enabled"], false);

        // ...tightening and stopping run.
        let mut tighter = cfg.clone();
        tighter["horizon"]["y"] = json!(0.5);
        let (code, body) = post("/api/safety", &tighter);
        assert_eq!(code, 200, "{body}");
        let low = json!({ "limits": { "max_power": 0.3, "max_color": [1.0, 1.0, 1.0] } });
        assert_eq!(post("/api/outputs/limits", &low).0, 200);
        assert_eq!(post("/api/safety/sky", &json!({ "on": true })).0, 200);
        assert_eq!(post("/api/safety/sky", &json!({ "on": false })).0, 423, "restoring the sky loosens");
        assert!(t.shared.lock().unwrap().sky_off);
        for (p, b) in [
            ("/api/audience", json!({ "mode": "above" })),
            ("/api/testpattern", json!({ "pattern": null })),
            ("/api/safety/trace", json!({ "on": false })),
            ("/api/remote", json!({ "enabled": false })),
            ("/api/external/gate", json!({ "on": false })),
            ("/api/lock", json!({ "show_mode": true })),
            ("/api/lock", json!({ "show_mode": false })),
            ("/api/arm", json!({ "on": true })),
            ("/api/arm", json!({ "on": false })),
            ("/api/estop", json!({})),
            ("/api/estop/reset", json!({})),
        ] {
            let (code, body) = post(p, &b);
            assert_eq!(code, 200, "{p} {b}: {body}");
        }
        assert!(t.shared.lock().unwrap().safety.get().horizon.y > 0.45);
        // Show mode on top: the e-stop still runs.
        assert_eq!(post("/api/lock", &json!({ "show_mode": true })).0, 200);
        assert_eq!(post("/api/estop", &json!({})).0, 200);
        assert!(t.shared.lock().unwrap().estop.is_latched());
    }

    /// T-261: the code unlocks (local UI, keyboard or API only); 5 wrong
    /// codes in a row make every attempt wait (429); the code is never
    /// echoed nor logged; changing or removing it needs the lock open.
    #[test]
    fn the_safety_code_is_rate_limited_and_never_echoed() {
        let t = TestServer::start(false);
        let post = |p: &str, b: &serde_json::Value| t.request("POST", p, &b.to_string());
        assert_eq!(post("/api/safety/lock", &json!({ "action": "set", "code": "12" })).0, 400);
        assert_eq!(post("/api/safety/lock", &json!({ "action": "lock" })).0, 409, "no code yet");
        assert_eq!(post("/api/safety/lock", &json!({ "action": "set", "code": "907312", "source": "midi" })).0, 403);
        assert_eq!(post("/api/safety/lock", &json!({ "action": "set", "code": "907312" })).0, 200);
        // Locked: changing or removing the code is refused.
        assert_eq!(post("/api/safety/lock", &json!({ "action": "remove" })).0, 423);
        assert_eq!(post("/api/safety/lock", &json!({ "action": "set", "code": "1111" })).0, 423);
        // Never from MIDI, OSC, DMX or the remote, even with the code.
        for src in ["midi", "osc", "dmx", "remote", "network"] {
            assert_eq!(post("/api/safety/unlock", &json!({ "code": "907312", "source": src })).0, 403, "{src}");
        }
        let (code, body) = post("/api/safety/unlock", &json!({ "code": "907312" }));
        assert_eq!(code, 200, "{body}");
        assert_eq!(js(&body)["locked"], false);
        assert!(js(&body)["relock_in_s"].as_u64().is_some_and(|s| s > 590));
        assert_eq!(post("/api/safety", &js(&t.request("GET", "/api/safety", "").1)["settings"]).0, 200, "unlocked: edits run");
        assert_eq!(post("/api/safety/lock", &json!({ "action": "lock", "source": "midi" })).0, 200, "locking from anywhere");
        // Five wrong codes: then even the right one waits.
        for left in (1..crate::safetylock::MAX_FAILURES).rev() {
            let (code, body) = post("/api/safety/unlock", &json!({ "code": "000000" }));
            assert_eq!(code, 403, "{body}");
            assert_eq!(js(&body)["attempts_left"], left);
        }
        assert_eq!(post("/api/safety/unlock", &json!({ "code": "000000" })).0, 429);
        let (code, body) = post("/api/safety/unlock", &json!({ "code": "907312" }));
        assert_eq!(code, 429, "{body}");
        assert!(js(&body)["wait_s"].as_u64().is_some_and(|s| s > 50));
        assert_eq!(js(&t.request("GET", "/api/safety/lock", "").1)["locked"], true);
        // Nowhere in the answers, the state or the log.
        for p in ["/api/safety/lock", "/api/state", "/api/lock", "/api/safety/log"] {
            assert!(!t.request("GET", p, "").1.contains("907312"), "{p}");
        }
    }

    /// T-261 with the safety invariants' generator: random safety settings
    /// and power caps posted with `confirm_loosen` while locked never
    /// loosen anything (`SAFETY_CASES` cases).
    #[test]
    fn safety_lock_random_edits_never_loosen() {
        use crate::safety_invariants::{mutated, seeds, Rng};
        use crate::zones::{Horizon, Zone, ZoneKind};
        let t = TestServer::start(false);
        {
            let mut s = t.shared.lock().unwrap();
            let mut cfg = s.safety.get();
            cfg.horizon = Horizon { y: 0.2, lines: true, ramp: 0.1, level: 0.0 };
            let square = |x: f32| vec![[x, -0.9], [x + 0.4, -0.9], [x + 0.4, -0.5], [x, -0.5]];
            cfg.zones = vec![
                Zone { id: 1, name: "Public".into(), kind: ZoneKind::Blank, points: square(-0.6), audience: true, ..Default::default() },
                Zone { id: 2, name: "Bar".into(), kind: ZoneKind::Dim, level: 0.4, points: square(0.2), ..Default::default() },
            ];
            s.safety.set(cfg, true).unwrap();
            s.safety_lock.set_code(crate::safetylock::new_file("5555", 10).unwrap(), Instant::now()).unwrap();
        }
        for seed in seeds(2610, 150) {
            let mut rng = Rng::new(seed);
            let before = t.shared.lock().unwrap().safety.get();
            let mut body = serde_json::to_value(mutated(&before, &mut rng, 0.2)).unwrap();
            body["confirm_loosen"] = json!(true);
            let code = t.request("POST", "/api/safety", &body.to_string()).0;
            let after = t.shared.lock().unwrap().safety.get();
            assert!(before.loosenings(&after).is_empty(), "seed {seed} ({code}): {:?}", before.loosenings(&after));
            let caps = t.shared.lock().unwrap().outputs.active().limits;
            let body = json!({ "limits": mutated(&caps, &mut rng, 0.5), "confirm_loosen": true });
            t.request("POST", "/api/outputs/limits", &body.to_string());
            let now = t.shared.lock().unwrap().outputs.active().limits;
            assert!(now.max_power <= caps.max_power && (0..3).all(|i| now.max_color[i] <= caps.max_color[i]), "seed {seed}: {caps:?} -> {now:?}");
        }
        assert!(t.shared.lock().unwrap().safety_lock.locked(Instant::now()));
    }
}
