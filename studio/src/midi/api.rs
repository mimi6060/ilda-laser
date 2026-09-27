//! `/api/midi*` routes, as a pure function of `Shared` so they are tested
//! without an HTTP server. `web.rs` only reads the body and forwards.

use super::profile::GENERIC;
use crate::Shared;
use serde::Deserialize;
use serde_json::{json, Value};

pub enum Reply {
    Json(Value),
    Text(u16, String),
}

fn ok() -> Reply {
    Reply::Text(200, "ok".into())
}

#[derive(Deserialize)]
struct ProfileRequest {
    port: String,
    /// `null` or `""` = automatic choice ("Réinitialiser le profil").
    #[serde(default)]
    profile: Option<String>,
}

/// Partial update of the safety options; missing fields are unchanged.
#[derive(Deserialize)]
struct SafetyRequest {
    #[serde(default)]
    allow_arm: Option<bool>,
    #[serde(default)]
    blackout_on_disconnect: Option<bool>,
}

#[derive(Deserialize)]
struct DeviceRequest {
    port: String,
    enabled: bool,
}

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Reply> {
    serde_json::from_str(body).map_err(|e| Reply::Text(400, format!("invalid JSON: {e}")))
}

/// `None` = not a MIDI route.
pub fn route(s: &mut Shared, post: bool, path: &str, body: &str) -> Option<Reply> {
    Some(match (post, path) {
        (false, "/api/midi") => Reply::Json(state(s)),
        (false, "/api/midi/profiles") => Reply::Json(json!(s.midi.store.list())),
        (true, "/api/midi/profile") => match parse::<ProfileRequest>(body) {
            Ok(req) => {
                let slug = req.profile.as_deref().filter(|p| !p.is_empty());
                match s.midi.store.set_port_profile(&req.port, slug) {
                    Ok(()) => ok(),
                    Err(e) if slug.is_some_and(|p| s.midi.store.get(p).is_none()) => Reply::Text(404, e),
                    Err(e) => Reply::Text(500, e),
                }
            }
            Err(e) => e,
        },
        (true, "/api/midi/device") => match parse::<DeviceRequest>(body) {
            Ok(req) => match s.midi.store.set_port_enabled(&req.port, req.enabled) {
                Ok(()) => {
                    // Shown at once; the worker opens/closes the port within 250 ms.
                    if let Some(d) = s.midi.devices.iter_mut().find(|d| d.name == req.port) {
                        d.enabled = req.enabled;
                    }
                    ok()
                }
                Err(e) => Reply::Text(500, e),
            },
            Err(e) => e,
        },
        (true, "/api/midi/safety") => match parse::<SafetyRequest>(body) {
            Ok(req) => {
                let mut safety = s.midi.store.devices.safety;
                safety.allow_arm = req.allow_arm.unwrap_or(safety.allow_arm);
                safety.blackout_on_disconnect = req.blackout_on_disconnect.unwrap_or(safety.blackout_on_disconnect);
                match s.midi.store.set_safety(safety) {
                    Ok(()) => ok(),
                    Err(e) => Reply::Text(500, e),
                }
            }
            Err(e) => e,
        },
        _ => return None,
    })
}

fn state(s: &Shared) -> Value {
    let m = &s.midi;
    let mut errors = m.store.errors.clone();
    errors.extend(m.error.clone());
    errors.extend(m.map.warnings.iter().cloned());
    json!({
        "enabled": m.enabled,
        "devices": m.devices,
        "last": m.last.as_ref().map(|e| e.to_json()),
        "recent": m.recent.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
        "errors": errors,
        "default_profile": GENERIC,
        "safety": m.store.devices.safety,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{MidiEvent, MidiMsg};
    use crate::test_support;
    use std::time::Instant;

    fn json(reply: Option<Reply>) -> Value {
        match reply {
            Some(Reply::Json(v)) => v,
            Some(Reply::Text(code, t)) => panic!("expected JSON, got {code} {t}"),
            None => panic!("not routed"),
        }
    }

    fn status(reply: Option<Reply>) -> u16 {
        match reply {
            Some(Reply::Text(code, _)) => code,
            Some(Reply::Json(_)) => 200,
            None => 0,
        }
    }

    #[test]
    fn no_midi_state() {
        let mut s = test_support::shared();
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["enabled"], false);
        assert_eq!(v["devices"], json!([]));
        assert_eq!(v["last"], Value::Null);
        assert!(route(&mut s, false, "/api/midix", "").is_none());
    }

    #[test]
    fn state_lists_devices_and_last_message() {
        let mut s = test_support::shared();
        s.midi.enabled = true;
        let d = s.midi.device_mut("APC40 mkII");
        d.connected = true;
        d.model = crate::midi::Model::Apc40Mk2;
        s.midi.record(&MidiEvent { port: "APC40 mkII".into(), msg: MidiMsg::NoteOn { channel: 0, note: 0x51, velocity: 127 }, at: Instant::now() });
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["enabled"], true);
        let dev = &v["devices"][0];
        assert_eq!(dev["name"], "APC40 mkII");
        assert_eq!(dev["model"], "Apc40Mk2");
        for key in ["input", "output", "connected", "profile", "enabled"] {
            assert!(dev.get(key).is_some(), "{key}");
        }
        assert_eq!(v["last"]["msg"]["note"], 0x51);
    }

    #[test]
    fn profiles_and_preferences() {
        let mut s = test_support::shared();
        let list = json(route(&mut s, false, "/api/midi/profiles", ""));
        assert_eq!(list.as_array().unwrap().len(), 3);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile", r#"{"port":"APC40 mkII","profile":"generic"}"#)), 200);
        assert_eq!(s.midi.store.choose("APC40 mkII", crate::midi::Model::Apc40Mk2).slug, "generic");
        assert_eq!(status(route(&mut s, true, "/api/midi/profile", r#"{"port":"APC40 mkII","profile":null}"#)), 200);
        assert_eq!(s.midi.store.choose("APC40 mkII", crate::midi::Model::Apc40Mk2).slug, "apc40-mk2");
        assert_eq!(status(route(&mut s, true, "/api/midi/profile", r#"{"port":"APC40 mkII","profile":"nope"}"#)), 404);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile", "{")), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/device", r#"{"port":"X","enabled":false}"#)), 200);
        assert!(!s.midi.store.port_enabled("X"));
    }

    #[test]
    fn safety_options_default_off_and_partial_updates() {
        let mut s = test_support::shared();
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["safety"], json!({ "allow_arm": false, "blackout_on_disconnect": false }));
        assert_eq!(status(route(&mut s, true, "/api/midi/safety", r#"{"blackout_on_disconnect":true}"#)), 200);
        assert_eq!(status(route(&mut s, true, "/api/midi/safety", r#"{"allow_arm":true}"#)), 200);
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["safety"], json!({ "allow_arm": true, "blackout_on_disconnect": true }));
        assert_eq!(status(route(&mut s, true, "/api/midi/safety", "[")), 400);
        assert!(!s.armed, "changing the option never arms");
    }
}
