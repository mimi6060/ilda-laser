//! Tracing the safety zones with the beam, to check them in the room
//! (T-447).
//!
//! The operator asks for it from the local UI only (`POST
//! /api/safety/trace`): it is not a control of the registry, so no MIDI,
//! DMX, OSC, timeline, macro or remote can start it. While it runs, the
//! output frame is `zones::trace_frame` (the outlines just outside the
//! zones, green at `zones::TRACE_LEVEL`), in output coordinates: it skips
//! the looks and the calibration and goes through the *whole* safety stage
//! (zones, horizon, strobe limiter, static beam guard, power caps) and the
//! arming gate like any frame. Starting it never arms: disarmed, only the
//! preview shows it.
//!
//! Like a test pattern (T-404), the cues wait in the deck and a playing
//! timeline is paused, then resumes when the trace ends. It ends by itself
//! after `zones::TRACE_SECONDS`, on the emergency stop, when the laser is
//! disarmed while it runs, when the page that started it stops beating,
//! and when the one zone it traces is removed. Starting again takes a new
//! click. Never saved: off at start-up.

use crate::zones::TRACE_SECONDS;
use crate::Shared;
use serde_json::json;
use std::time::{Duration, Instant};

/// The verification mode (memory only).
#[derive(Clone, Debug, Default)]
pub struct ZoneTrace {
    /// When it ends by itself; `None` = not tracing.
    pub until: Option<Instant>,
    /// One zone (its id), or every zone and the horizon.
    pub only: Option<u32>,
    /// The page that started it (its heartbeat id), if it said.
    client: Option<String>,
    /// The laser was armed on the last frame of the trace: a disarm ends it.
    was_armed: bool,
    /// The timeline was playing when the trace took over.
    resume_timeline: bool,
}

impl ZoneTrace {
    pub fn is_on(&self) -> bool {
        self.until.is_some()
    }
}

/// Starts tracing (`only`: one zone, else all and the horizon) for
/// `TRACE_SECONDS` from `now`. Refused under the emergency stop and for an
/// unknown zone. Never arms; a trace already running starts its time over.
pub fn start(s: &mut Shared, now: Instant, only: Option<u32>, client: Option<&str>) -> Result<(), String> {
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réarmez-le d'abord".into());
    }
    if let Some(id) = only {
        if !s.safety.get().zones.iter().any(|z| z.id == id) {
            return Err(format!("zone inconnue : {id}"));
        }
    }
    let was_on = s.zone_trace.is_on();
    s.zone_trace.until = Some(now + Duration::from_secs(TRACE_SECONDS));
    s.zone_trace.only = only;
    s.zone_trace.client = client.map(|c| c.chars().take(64).collect());
    s.zone_trace.was_armed = s.gate.is_armed();
    hold(s);
    if !was_on {
        s.gate.log().record("zone_trace", Some("ui"), json!({ "on": true, "only": only }));
    }
    Ok(())
}

/// Ends the trace (`reason` goes to the safety log). A paused timeline
/// resumes where it was, unless an emergency stop halted it or it was
/// moved meanwhile. Never arms.
pub fn stop(s: &mut Shared, reason: &str) {
    if s.zone_trace.until.take().is_none() {
        return;
    }
    s.zone_trace.client = None;
    s.gate.log().record("zone_trace", Some(if reason == "user" { "ui" } else { "system" }), json!({ "on": false, "reason": reason }));
    if std::mem::take(&mut s.zone_trace.resume_timeline)
        && s.timeline.transport == crate::timeline::Transport::Paused
        && !s.estop.is_latched()
        // A test pattern still shown keeps holding it.
        && s.test_pattern.active.is_none()
    {
        let c = s.timeline_clock();
        s.timeline.play(&c);
    } else if s.test_pattern.active.is_some() {
        // Hand the paused timeline over to the pattern.
        s.test_pattern.resume_timeline |= std::mem::take(&mut s.zone_trace.resume_timeline);
    }
}

/// Every frame, before the output is chosen: ends the trace when its time
/// is up, on the emergency stop, on a disarm while it runs, when its page
/// stopped beating (presence enforced) or its zone is gone.
pub fn sync(s: &mut Shared, now: Instant) {
    let Some(until) = s.zone_trace.until else { return };
    let armed = s.gate.is_armed();
    let reason = if s.estop.is_latched() {
        Some("estop")
    } else if now >= until {
        Some("timeout")
    } else if s.zone_trace.was_armed && !armed {
        Some("disarm")
    } else if s.presence.enforced && s.zone_trace.client.as_deref().is_some_and(|c| !s.presence.is_beating(c)) {
        Some("ui_lost")
    } else if s.zone_trace.only.is_some_and(|id| !s.safety.get().zones.iter().any(|z| z.id == id)) {
        Some("zone")
    } else {
        None
    };
    match reason {
        Some(r) => stop(s, r),
        None => s.zone_trace.was_armed = armed,
    }
}

/// While tracing, a timeline that starts playing is paused at once (it
/// resumes when the trace ends). Returns the zone selection to trace.
/// Called by the engine every frame, when the output is chosen.
pub fn hold(s: &mut Shared) -> Option<Option<u32>> {
    // An e-stop tripped since the top of the frame (a controller, the
    // lock-free route) ends it before anything is drawn.
    if s.estop.is_latched() {
        stop(s, "estop");
    }
    if !s.zone_trace.is_on() {
        return None;
    }
    if s.timeline.is_playing() {
        let c = s.timeline_clock();
        s.timeline.pause(&c);
        s.zone_trace.resume_timeline = true;
    }
    Some(s.zone_trace.only)
}

/// `zone_trace` in `/api/state` and the reply of `/api/safety/trace`.
pub fn view(s: &Shared, now: Instant) -> serde_json::Value {
    let remaining = s.zone_trace.until.map_or(0, |u| u.saturating_duration_since(now).as_secs_f64().ceil() as u32);
    json!({ "on": s.zone_trace.is_on(), "remaining_s": remaining, "only": s.zone_trace.only, "seconds": TRACE_SECONDS })
}
