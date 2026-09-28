//! `/api/midi*` routes, as a pure function of `Shared` so they are tested
//! without an HTTP server. `web.rs` only reads the body and forwards.

use super::learn;
use super::profile::GENERIC;
use super::testing::InjectError;
use crate::Shared;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

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
    #[serde(default)]
    enabled: Option<bool>,
    /// « Retour LED » (T-205).
    #[serde(default)]
    leds: Option<bool>,
}

/// `--midi-test` only: bytes "played" on a simulated controller.
#[derive(Deserialize)]
struct InjectRequest {
    /// Default: the first simulated device.
    #[serde(default)]
    port: Option<String>,
    bytes: Vec<u8>,
}

/// MIDI learn (T-203).
#[derive(Deserialize)]
struct LearnRequest {
    target: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    shift: bool,
}

#[derive(Deserialize)]
struct ConfirmRequest {
    replace: bool,
}

#[derive(Deserialize)]
struct DeleteRequest {
    port: String,
    index: usize,
}

#[derive(Deserialize)]
struct ForgetRequest {
    target: String,
}

fn learn_reply(r: Result<(), learn::LearnError>) -> Reply {
    match r {
        Ok(()) => ok(),
        Err((code, msg)) => Reply::Text(code, msg),
    }
}

/// Longest injection accepted at once.
const INJECT_MAX: usize = 1024;

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
            Ok(DeviceRequest { enabled: None, leds: None, .. }) => Reply::Text(400, "« enabled » ou « leds » attendu".into()),
            Ok(req) => {
                let store = &mut s.midi.store;
                let mut saved = req.enabled.map_or(Ok(()), |on| store.set_port_enabled(&req.port, on));
                if let (Ok(()), Some(on)) = (&saved, req.leds) {
                    saved = store.set_port_leds(&req.port, on);
                }
                match saved {
                    Ok(()) => {
                        // Shown at once; the worker opens/closes the port within
                        // 250 ms and follows « Retour LED » at its next LED update.
                        if let Some(d) = s.midi.devices.iter_mut().find(|d| d.name == req.port) {
                            d.enabled = req.enabled.unwrap_or(d.enabled);
                            d.leds = req.leds.unwrap_or(d.leds);
                        }
                        ok()
                    }
                    Err(e) => Reply::Text(500, e),
                }
            }
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
        (true, "/api/midi/learn") => match parse::<LearnRequest>(body) {
            Ok(req) => learn_reply(learn::start(s, &req.target, req.args, req.shift, Instant::now())),
            Err(e) => e,
        },
        (true, "/api/midi/learn/cancel") => {
            learn::cancel(s);
            ok()
        }
        (true, "/api/midi/learn/confirm") => match parse::<ConfirmRequest>(body) {
            Ok(req) => learn_reply(learn::confirm(s, req.replace)),
            Err(e) => e,
        },
        (true, "/api/midi/mapping/delete") => match parse::<DeleteRequest>(body) {
            Ok(req) => learn_reply(learn::delete(s, &req.port, req.index)),
            Err(e) => e,
        },
        (true, "/api/midi/mapping/forget") => match parse::<ForgetRequest>(body) {
            Ok(req) => match learn::forget(s, &req.target) {
                Ok(n) => Reply::Json(json!({ "removed": n })),
                Err((code, msg)) => Reply::Text(code, msg),
            },
            Err(e) => e,
        },
        // Test mode only (`--midi-test`): without it these routes don't exist.
        (true, "/api/midi/inject") if s.midi.sim.is_some() => match parse::<InjectRequest>(body) {
            Ok(req) => inject(s, req),
            Err(e) => e,
        },
        (false, "/api/midi/sent") if s.midi.sim.is_some() => Reply::Json(sent(s)),
        _ => return None,
    })
}

/// Hands the bytes to the simulated device's input, exactly as CoreMIDI
/// would: the worker decodes them and the mapping engine applies them
/// with every T-208 rule. Nothing here touches the laser directly.
fn inject(s: &Shared, req: InjectRequest) -> Reply {
    let Some(sim) = &s.midi.sim else { return Reply::Text(404, "not found".into()) };
    if req.bytes.is_empty() || req.bytes.len() > INJECT_MAX {
        return Reply::Text(400, format!("« bytes » : 1 à {INJECT_MAX} octets"));
    }
    let Some(port) = req.port.or_else(|| sim.ports().into_iter().next()) else {
        return Reply::Text(404, "aucun appareil simulé".into());
    };
    match sim.inject(&port, &req.bytes) {
        Ok(()) => ok(),
        Err(InjectError::NoSuchPort) => Reply::Text(404, format!("appareil simulé inconnu : {port}")),
        Err(InjectError::NotOpen) => Reply::Text(409, format!("{port} n'est pas ouvert (désactivé ?)")),
    }
}

/// What the studio sent to each simulated device, and its pads' LEDs.
fn sent(s: &Shared) -> Value {
    let Some(sim) = &s.midi.sim else { return Value::Null };
    let devices: Vec<Value> = sim
        .ports()
        .iter()
        .filter_map(|port| {
            sim.with_device(port, |apc| {
                json!({ "port": port, "model": apc.model, "mode": apc.mode(), "sent": apc.received, "pads": apc.pads() })
            })
        })
        .collect();
    json!({ "devices": devices })
}

fn state(s: &Shared) -> Value {
    let m = &s.midi;
    let mut errors = m.store.errors.clone();
    errors.extend(m.error.clone());
    errors.extend(m.map.warnings.iter().cloned());
    let mut v = json!({
        "enabled": m.enabled,
        "devices": m.devices,
        "last": m.last.as_ref().map(|e| e.to_json()),
        "recent": m.recent.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
        "errors": errors,
        "default_profile": GENERIC,
        "safety": m.store.devices.safety,
        "test": m.sim.is_some(),
    });
    for (key, value) in learn::state(s, Instant::now()) {
        v[key] = value;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{MidiEvent, MidiMsg};
    use crate::test_support;

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
        assert!(s.midi.store.port_leds("X"), "« Retour LED » on by default");
        assert_eq!(status(route(&mut s, true, "/api/midi/device", r#"{"port":"X","leds":false}"#)), 200);
        assert!(!s.midi.store.port_leds("X") && !s.midi.store.port_enabled("X"), "partial update");
        assert_eq!(status(route(&mut s, true, "/api/midi/device", r#"{"port":"X"}"#)), 400);
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
        assert!(!s.gate.is_armed(), "changing the option never arms");
    }

    #[test]
    fn test_routes_do_not_exist_without_midi_test() {
        let mut s = test_support::shared();
        assert!(route(&mut s, true, "/api/midi/inject", r#"{"bytes":[144,32,127]}"#).is_none(), "404 in web.rs");
        assert!(route(&mut s, false, "/api/midi/sent", "").is_none());
        assert_eq!(json(route(&mut s, false, "/api/midi", ""))["test"], false);
    }

    #[test]
    fn inject_reaches_the_worker_and_sent_shows_the_device() {
        use crate::midi::testing::{FakeApc, SimMidi, TEST_MK2_PORT};
        use crate::midi::worker::Worker;
        use std::sync::{Arc, Mutex};

        let sim = SimMidi::default();
        sim.plug(TEST_MK2_PORT, FakeApc::new(crate::midi::Model::Apc40Mk2));
        let shared = Arc::new(Mutex::new(test_support::shared()));
        shared.lock().unwrap().midi.sim = Some(sim.clone());
        let mut w = Worker::new(sim, Arc::clone(&shared));
        for _ in 0..3 {
            w.step(Instant::now(), None);
        }
        let call = |post, path: &str, body: &str| route(&mut shared.lock().unwrap(), post, path, body);
        assert_eq!(json(call(false, "/api/midi", ""))["test"], true);
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"bytes":[144,32,127]}"#)), 200, "default port");
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"port":"Test APC40 mkII","bytes":[128,32,0]}"#)), 200);
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"port":"APC40 mkII","bytes":[144,32,127]}"#)), 404, "only simulated ports");
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"bytes":[]}"#)), 400);
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"bytes":[300]}"#)), 400);
        assert_eq!(status(call(true, "/api/midi/inject", &format!(r#"{{"bytes":{:?}}}"#, vec![0xFEu8; INJECT_MAX + 1]))), 400);
        w.step(Instant::now(), None);
        let recent: Vec<Value> = shared.lock().unwrap().midi.recent.iter().map(|e| e.to_json()).collect();
        assert_eq!(recent[recent.len() - 2]["msg"]["kind"], "note_on");
        assert_eq!(recent[recent.len() - 1]["msg"]["kind"], "note_off");

        let sent = json(call(false, "/api/midi/sent", ""));
        let dev = &sent["devices"][0];
        assert_eq!(dev["port"], TEST_MK2_PORT);
        assert_eq!(dev["mode"], 0x41, "taken over by the studio");
        assert_eq!(dev["sent"][0], json!(crate::midi::detect::DEVICE_INQUIRY));
        assert_eq!(dev["pads"].as_array().unwrap().len(), 5);
        assert!(!shared.lock().unwrap().gate.is_armed());

        // A disabled port can't be injected into.
        call(true, "/api/midi/device", r#"{"port":"Test APC40 mkII","enabled":false}"#);
        w.step(Instant::now() + std::time::Duration::from_millis(300), None);
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"bytes":[144,32,127]}"#)), 409);
    }

    #[test]
    fn learn_routes() {
        let mut s = test_support::shared();
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", r#"{"target":"master.size"}"#)), 409, "--no-midi");
        s.midi.enabled = true;
        s.midi.device_mut("Pad").connected = true;
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", r#"{"target":"nope"}"#)), 404);
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", r#"{"target":"transport.arm"}"#)), 403);
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", "{")), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", r#"{"target":"master.size"}"#)), 200);
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["learn"]["target"], "master.size");
        assert_eq!(v["mappings"], json!([]));
        assert_eq!(status(route(&mut s, true, "/api/midi/learn/cancel", "")), 200);
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["learn"], Value::Null);
        assert_eq!(v["learn_notice"]["text"], "Apprentissage MIDI annulé");
        assert_eq!(status(route(&mut s, true, "/api/midi/learn/confirm", r#"{"replace":true}"#)), 409);
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/delete", r#"{"port":"Pad","index":0}"#)), 404);
        assert_eq!(json(route(&mut s, true, "/api/midi/mapping/forget", r#"{"target":"master.size"}"#))["removed"], 0);
        assert!(!s.gate.is_armed());
    }
}
