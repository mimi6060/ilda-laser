//! `/api/midi*` routes, as a pure function of `Shared` so they are tested
//! without an HTTP server. `web.rs` only reads the body and forwards.

use super::learn;
use super::mapping::{LedFeedback, MapMode, RelEncoding};
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

/// « Exporter le profil » (T-211): by slug, or the profile a port uses.
#[derive(Deserialize)]
struct ExportRequest {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    port: Option<String>,
}

/// « Importer un profil » (T-211): the JSON of an exported profile.
#[derive(Deserialize)]
struct ImportRequest {
    profile: Value,
    /// Wanted slug (default: from the profile's name). Never overwrites.
    #[serde(default)]
    slug: Option<String>,
    /// Use it on this port at once.
    #[serde(default)]
    port: Option<String>,
}

#[derive(Deserialize)]
struct UpdateRequest {
    port: String,
    index: usize,
    #[serde(default)]
    mode: Option<MapMode>,
    #[serde(default)]
    encoding: Option<RelEncoding>,
    /// Absent = unchanged, `null` = no LED feedback.
    #[serde(default, deserialize_with = "some_led")]
    led: Option<Option<LedFeedback>>,
}

/// Tells an explicit `null` (Some(None)) from a missing field (None).
fn some_led<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<LedFeedback>>, D::Error> {
    Option::<LedFeedback>::deserialize(d).map(Some)
}

/// `--midi-test` only: plugs one more simulated device, a generic
/// controller that answers nothing (T-211).
#[derive(Deserialize)]
struct PlugRequest {
    port: String,
}

/// Largest exported profile accepted back (bytes of JSON).
const IMPORT_MAX: usize = 1 << 20;
/// Simulated devices `--midi-test` accepts.
const SIM_MAX: usize = 8;

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
        (true, "/api/midi/mapping/update") => match parse::<UpdateRequest>(body) {
            Ok(req) => {
                let patch = learn::MappingPatch { mode: req.mode, encoding: req.encoding, led: req.led };
                learn_reply(learn::update(s, &req.port, req.index, patch))
            }
            Err(e) => e,
        },
        (true, "/api/midi/profile/export") => match parse::<ExportRequest>(body) {
            Ok(req) => export(s, req),
            Err(e) => e,
        },
        (true, "/api/midi/profile/import") => match parse::<ImportRequest>(body) {
            Ok(req) => import(s, req),
            Err(e) => e,
        },
        // Test mode only (`--midi-test`): without it these routes don't exist.
        (true, "/api/midi/plug") if s.midi.sim.is_some() => match parse::<PlugRequest>(body) {
            Ok(req) => plug(s, &req.port),
            Err(e) => e,
        },
        (true, "/api/midi/inject") if s.midi.sim.is_some() => match parse::<InjectRequest>(body) {
            Ok(req) => inject(s, req),
            Err(e) => e,
        },
        (false, "/api/midi/sent") if s.midi.sim.is_some() => Reply::Json(sent(s)),
        _ => return None,
    })
}

/// The profile JSON, exactly as saved in `<data-dir>/midi/profiles`.
fn export(s: &Shared, req: ExportRequest) -> Reply {
    let slug = match (req.slug.filter(|x| !x.is_empty()), req.port) {
        (Some(slug), _) => slug,
        (None, Some(port)) => match s.midi.devices.iter().find(|d| d.name == port) {
            Some(d) => d.profile.clone(),
            None => return Reply::Text(404, format!("appareil MIDI inconnu : {port}")),
        },
        (None, None) => return Reply::Text(400, "« slug » ou « port » attendu".into()),
    };
    match s.midi.store.get(&slug) {
        Some(p) => Reply::Json(json!({ "slug": slug, "profile": p })),
        None => Reply::Text(404, format!("profil inconnu : {slug}")),
    }
}

/// Checks the profile like a file loaded at start-up, then saves it under a
/// new slug. Its mappings run through the same engine and T-208 rules as
/// any other: importing can't arm anything.
fn import(s: &mut Shared, req: ImportRequest) -> Reply {
    let json = req.profile.to_string();
    if json.len() > IMPORT_MAX {
        return Reply::Text(413, "profil trop gros".into());
    }
    let profile = match super::profile::Profile::parse(&json) {
        Ok(p) => p,
        Err(e) => return Reply::Text(400, format!("profil illisible : {e}")),
    };
    let port = req.port.filter(|p| !p.is_empty());
    match s.midi.store.import(port.as_deref(), req.slug.as_deref(), profile) {
        Ok(slug) => {
            if let Some(d) = port.and_then(|p| s.midi.devices.iter_mut().find(|d| d.name == p)) {
                d.profile = slug.clone();
            }
            Reply::Json(json!({ "slug": slug }))
        }
        Err(e) => Reply::Text(500, e),
    }
}

fn plug(s: &Shared, port: &str) -> Reply {
    let Some(sim) = &s.midi.sim else { return Reply::Text(404, "not found".into()) };
    if port.is_empty() || port.len() > 64 {
        return Reply::Text(400, "« port » : 1 à 64 caractères".into());
    }
    if !sim.ports().iter().any(|p| p == port) && sim.ports().len() >= SIM_MAX {
        return Reply::Text(409, format!("{SIM_MAX} appareils simulés au plus"));
    }
    sim.plug(port, super::testing::FakeApc::generic());
    ok()
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
    let now = Instant::now();
    for (i, d) in m.devices.iter().enumerate() {
        // Activity light (T-211): how long since its last message.
        v["devices"][i]["activity_ms"] = json!(d.last_at.map(|t| now.saturating_duration_since(t).as_millis() as u64));
    }
    for (key, value) in learn::state(s, now) {
        v[key] = value;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::mapping::MapMode;
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
        assert_eq!(list.as_array().unwrap().len(), 7);
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
    fn export_then_import_gives_the_same_mappings() {
        let mut s = test_support::shared();
        s.midi.device_mut("nanoKONTROL2").profile = "nanokontrol2".into();
        let out = json(route(&mut s, true, "/api/midi/profile/export", r#"{"port":"nanoKONTROL2"}"#));
        assert_eq!(out["slug"], "nanokontrol2");
        let body = json!({ "profile": out["profile"], "port": "nanoKONTROL2" }).to_string();
        let slug = json(route(&mut s, true, "/api/midi/profile/import", &body))["slug"].as_str().unwrap().to_string();
        assert_ne!(slug, "nanokontrol2", "a built-in is never overwritten");
        assert_eq!(s.midi.store.get(&slug), s.midi.store.get("nanokontrol2"));
        assert_eq!(s.midi.devices[0].profile, slug, "used at once");
        assert_eq!(s.midi.store.choose("nanoKONTROL2", crate::midi::Model::Unknown).slug, slug);
        let again = json(route(&mut s, true, "/api/midi/profile/export", &json!({ "slug": slug }).to_string()));
        assert_eq!(again["profile"], out["profile"], "round trip");

        assert_eq!(status(route(&mut s, true, "/api/midi/profile/export", r#"{"slug":"nope"}"#)), 404);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile/export", r#"{"port":"nope"}"#)), 404);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile/export", "{}")), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile/import", r#"{"profile":{"version":2}}"#)), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile/import", r#"{"profile":{"mappings":[{"mode":"wobble"}]}}"#)), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/profile/import", r#"{"profile":[1]}"#)), 400);
        let named = json(route(&mut s, true, "/api/midi/profile/import", r#"{"profile":{"name":"Mon Pad à moi"}}"#));
        assert_eq!(named["slug"], "mon-pad-a-moi");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn mapping_update_route() {
        let mut s = test_support::shared();
        s.midi.enabled = true;
        s.midi.device_mut("Pad").connected = true;
        assert_eq!(status(route(&mut s, true, "/api/midi/learn", r#"{"target":"cue.multi"}"#)), 200);
        let ev = MidiEvent { port: "Pad".into(), msg: MidiMsg::NoteOn { channel: 0, note: 5, velocity: 127 }, at: Instant::now() };
        crate::midi::engine::handle_batch(&mut s, &[ev]);
        let body = r#"{"port":"Pad","index":0,"led":{"off":0,"on":1,"blink":2}}"#;
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", body)), 200);
        let v = json(route(&mut s, false, "/api/midi", ""));
        assert_eq!(v["mappings"][0]["led"], json!({ "off": 0, "on": 1, "blink": 2 }));
        assert_eq!(v["mappings"][0]["kind"], "note");
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", r#"{"port":"Pad","index":0}"#)), 200, "nothing to change");
        assert_eq!(json(route(&mut s, false, "/api/midi", ""))["mappings"][0]["led"]["on"], 1, "absent = unchanged");
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", r#"{"port":"Pad","index":0,"led":null}"#)), 200);
        assert_eq!(json(route(&mut s, false, "/api/midi", ""))["mappings"][0]["led"], Value::Null);
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", r#"{"port":"Pad","index":0,"encoding":"sign_bit"}"#)), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", r#"{"port":"Pad","index":0,"encoding":"wobble"}"#)), 400);
        assert_eq!(status(route(&mut s, true, "/api/midi/mapping/update", r#"{"port":"Pad","index":4}"#)), 404);
    }

    /// Every starter template drives known controls and none can arm.
    #[test]
    fn templates_use_known_controls_and_never_arm() {
        let s = test_support::shared();
        for info in s.midi.store.list().into_iter().filter(|p| p.template) {
            let p = s.midi.store.get(&info.slug).unwrap();
            assert!(!p.mappings.is_empty(), "{}", info.slug);
            for mp in &p.mappings {
                if mp.mode == MapMode::Grid {
                    assert!(mp.args["slot"].as_u64().unwrap() < 40, "{}", info.slug);
                    continue;
                }
                let d = s.controls.get(&mp.target).unwrap_or_else(|| panic!("{}: {}", info.slug, mp.target));
                assert!(d.external && d.id != crate::midi::safety::ARM, "{}: {}", info.slug, mp.target);
            }
            assert!(p.mappings.iter().any(|mp| mp.target == crate::midi::safety::BLACKOUT), "{}: a blackout button", info.slug);
        }
    }

    /// T-211 in `--midi-test`: a generic device plugged over HTTP is
    /// listed, learned, and its mapped note's LED follows the control.
    #[test]
    fn plugged_generic_device_learns_and_gets_its_led() {
        use crate::midi::testing::SimMidi;
        use crate::midi::worker::Worker;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let sim = SimMidi::default();
        let shared = Arc::new(Mutex::new(test_support::shared()));
        {
            let mut s = shared.lock().unwrap();
            s.midi.enabled = true;
            s.midi.sim = Some(sim.clone());
        }
        let call = |post, path: &str, body: &str| route(&mut shared.lock().unwrap(), post, path, body);
        assert_eq!(status(call(true, "/api/midi/plug", r#"{"port":"Pad 2000"}"#)), 200);
        assert_eq!(status(call(true, "/api/midi/plug", r#"{"port":""}"#)), 400);
        let mut w = Worker::new(sim.clone(), Arc::clone(&shared));
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        w.step(t0, None);
        w.step(at(600), None);
        let v = json(call(false, "/api/midi", ""));
        assert_eq!((v["devices"][0]["name"].as_str(), v["devices"][0]["profile"].as_str()), (Some("Pad 2000"), Some("generic")));
        assert_eq!(v["devices"][0]["activity_ms"], Value::Null);

        assert_eq!(status(call(true, "/api/midi/learn", r#"{"target":"cue.multi"}"#)), 200);
        assert_eq!(status(call(true, "/api/midi/inject", r#"{"port":"Pad 2000","bytes":[146,60,100]}"#)), 200);
        w.step(at(700), None);
        let v = json(call(false, "/api/midi", ""));
        assert_eq!(v["mappings"][0]["message"], "Note 60");
        assert!(v["devices"][0]["activity_ms"].as_u64().is_some(), "activity light");
        assert_eq!(status(call(true, "/api/midi/mapping/update", r#"{"port":"Pad 2000","index":0,"led":{"off":0,"on":127}}"#)), 200);
        w.step(at(1000), None);
        let led = || sim.with_device("Pad 2000", |d| d.last_value(0x92, 60)).unwrap();
        assert_eq!(led(), Some(0), "multi off");
        call(true, "/api/midi/inject", r#"{"port":"Pad 2000","bytes":[146,60,100,130,60,0]}"#);
        w.step(at(1100), None);
        assert!(shared.lock().unwrap().deck.multi);
        w.step(at(1200), None);
        assert_eq!(led(), Some(127), "lit by the studio");
        let sent = sim.with_device("Pad 2000", |d| d.received.clone()).unwrap();
        assert!(sent.iter().all(|m| m.first() != Some(&0xF0) || m[..] == crate::midi::detect::DEVICE_INQUIRY), "no APC SysEx");
        assert!(!shared.lock().unwrap().gate.is_armed());
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
