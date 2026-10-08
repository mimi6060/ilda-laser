//! T-449: robustness and endurance of the HTTP API.
//!
//! Runs the real binary in a subprocess, preview-only apart from the
//! hidden `--test-output` fake output (a text file, never a laser), with
//! `--no-midi`, `--no-audio`, a free 127.0.0.1 port and a temporary data
//! directory. Never `--device`, never port 8080, never the user's data.
//!
//! Every route of `web::ROUTES` (parsed from `src/web.rs`, prefixes
//! expanded with their sub-routes) receives generated requests from a
//! seeded generator: truncated or garbage JSON, invalid UTF-8, wrong types,
//! unknown fields, non-finite and huge numbers, 1 MB strings, deep nesting,
//! giant arrays, path traversal names, the route's own `GET` answer with
//! one value mangled, and a body announced at 64 MB. Throughout:
//!
//! 1. the studio never panics, never answers 5xx to bad input and always
//!    answers (no hang, no dead worker);
//! 2. nothing arms except an explicit `POST /api/arm` the generator sent
//!    (then the harness disarms at once); the fake output's log agrees;
//! 3. after a 4xx answer, nothing changed: `/api/state` (clocks aside),
//!    `/api/project` and the data directory's files;
//! 4. `POST /api/estop` answers fast, whatever else is going on;
//! 5. the engine keeps ticking (`/api/perf` frames go up, `engine_ok`);
//! 6. no file appears outside the data directory (traversal names and
//!    absolute paths point into the test's own sandbox).
//!
//! Knobs (environment):
//! - `FUZZ_SEED` (default 449): replays a failure, printed in its message;
//! - `FUZZ_CASES`: generated cases per route (default 6 in `cargo test`,
//!   200 in `api_fuzz_long`); `api_fuzz_long` takes `FUZZ_CASES=20000`;
//! - `ENDURANCE_SECS` (default 600): length of `api_endurance_long`.
//!
//! On demand:
//! `cargo test -p laser-studio --test api_robustness -- --ignored api_fuzz_long`
//! `ENDURANCE_SECS=3600 cargo test -p laser-studio --test api_robustness -- --ignored api_endurance_long --nocapture`

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WEB_RS: &str = include_str!("../src/web.rs");

// ---------------------------------------------------------------- routes

/// `web::ROUTES`: (method, path, class), read from the source.
fn route_table() -> Vec<(String, String, String)> {
    let start = WEB_RS.find("pub const ROUTES").expect("ROUTES in web.rs");
    let body = &WEB_RS[start..start + WEB_RS[start..].find("\n];").unwrap()];
    body.lines()
        .filter(|l| l.trim_start().starts_with("(\""))
        .map(|l| {
            let q: Vec<&str> = l.split('"').collect();
            let class = l.split("RouteClass::").nth(1).unwrap().trim_end_matches([')', ',', ' ']).to_string();
            (q[1].to_string(), q[3].to_string(), class)
        })
        .collect()
}

/// The sub-routes behind each prefix of `ROUTES` (`…*`). A new prefix
/// without a line here fails `every_route_is_fuzzed`.
const PREFIXES: &[(&str, &[&str])] = &[
    ("/api/help/", &["safety", "nope", "..%2F..%2Fetc", "../../../etc/passwd", "%C3%A9%F0%9F%94%A5"]),
    ("/api/checklist/", &["complete", "operator", "check", "config", "restart", "nope"]),
    ("/api/cuelist/", &["go", "back", "pause", "top", "goto", "nope"]),
    ("/api/timeline/", &["load", "play", "pause", "stop", "seek", "loop", "new", "edit", "save", "nope"]),
    (
        "/api/midi",
        &[
            "", "/device", "/learn", "/learn/cancel", "/learn/confirm", "/leds", "/mapping/delete", "/mapping/forget", "/mapping/update", "/profile", "/profile/export",
            "/profile/import", "/profiles", "/safety", "/virtual", "/virtual/model", "/virtual/unmap", "/inject", "/plug", "/sent", "x",
        ],
    ),
    ("/api/external", &["", "/gate", "/scopes"]),
    ("/api/operators/", &["D%C3%A9faut", "Test%20op", "..%2F..%2Fetc", "_current", "nope/../x"]),
    ("/api/dmx/", &["config", "learn", "interfaces", "personality", "monitor"]),
    (
        "/api/project/",
        &["autosave", "recover", "recover-ignore", "new", "open", "save", "save-as", "version-save", "version-open", "version-delete", "import-partial", "quit-check", "versions"],
    ),
];

/// Routes that reach the network or the Mac (DAC discovery, outputs,
/// Art-Net, network inputs, OSC, the tablet remote, sleep): they only get
/// bodies that can't parse, so the fuzz never configures a socket.
const BROKEN_ONLY: &[&str] = &[
    "/api/outputs/select",
    "/api/outputs/config",
    "/api/outputs/artnet",
    "/api/netin",
    "/api/osc/config",
    "/api/external",
    "/api/dmx/",
    "/api/timecode",
    "/api/remote",
    "/api/keep_awake",
    "/api/test/",
];

/// Never sent: they ignore their body and scan the network for DACs, or
/// are the harness's own (the e-stop), or are endless streams.
const NEVER: &[(&str, &str)] = &[("POST", "/api/devices/scan"), ("POST", "/api/outputs/scan"), ("POST", "/api/estop"), ("GET", "/api/events"), ("GET", "/api/frame.bin")];

/// Routes that decode on their own thread and may answer 503 when busy.
const DECODING: &[&str] = &["/api/media/audio", "/api/timeline/audio", "/api/figures/import", "/api/media/ilda", "/api/sdbank/export", "/api/pack/import"];

/// Every concrete (method, path) the fuzz drives.
fn targets() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (m, p, _) in route_table() {
        let methods: Vec<String> = if m == "*" { vec!["GET".into(), "POST".into()] } else { vec![m.clone()] };
        let paths: Vec<String> = match p.strip_suffix('*') {
            Some(prefix) => {
                let subs = PREFIXES.iter().find(|(pre, _)| *pre == prefix).unwrap_or_else(|| panic!("prefix {p} of web::ROUTES has no sub-routes in PREFIXES")).1;
                subs.iter().map(|s| format!("{prefix}{s}")).collect()
            }
            None => vec![p.clone()],
        };
        for m in &methods {
            for p in &paths {
                if !NEVER.contains(&(m.as_str(), p.as_str())) && !out.contains(&(m.clone(), p.clone())) {
                    out.push((m.clone(), p.clone()));
                }
            }
        }
    }
    out
}

fn broken_only(path: &str) -> bool {
    BROKEN_ONLY.iter().any(|p| path.starts_with(p))
}

// ------------------------------------------------------------------- rng

/// xorshift64*: small, seeded, reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// -------------------------------------------------------------- payloads

/// Keys the routes read, so a generated object often reaches the handler's
/// own checks rather than being refused for a missing field.
const KEYS: &[&str] = &[
    "on", "id", "name", "value", "norm", "path", "file", "index", "page", "pages", "slot", "cue", "layer", "level", "bpm", "ms", "step", "to", "source", "client_id", "mode", "settings",
    "text", "x", "y", "color", "enabled", "group", "groups", "items", "cells", "key", "action", "rate", "seconds", "position", "time", "dir", "folder", "example", "show", "list",
    "cues", "shape", "preset", "zone", "zones", "points", "frames", "code", "pin", "toggle", "visible", "dry_run", "sections", "channel", "note", "master", "speed", "size", "width",
    "height", "region", "label", "tags", "steps", "port", "down", "target", "kind", "type", "mult",
];

/// A number token JSON can't carry as a float, put into the text as is.
const RAW_NUMBERS: &[&str] = &["NaN", "Infinity", "-Infinity", "1e999", "-1e999", "1e-999", "123456789012345678901234567890", "-0", "0x10", "1.", ".5", "+1"];

/// Raw markers the serialised body swaps for `RAW_NUMBERS`.
const RAW: &str = "\u{1}raw\u{1}";

struct Gen<'a> {
    rng: &'a mut Rng,
    sandbox: String,
}

impl Gen<'_> {
    fn evil_string(&mut self) -> String {
        let n = self.rng.below(16);
        match n {
            0 => String::new(),
            1 => "é漢字🔥\u{202e}ﬁ\u{200b}".into(),
            2 => "\u{0}nul".into(),
            3 => "x".repeat(1 << 20),
            4 => "../../escape-rel".into(),
            5 => "../../../../../../../../tmp/escape-deep".into(),
            6 => format!("{}/abs-escape", self.sandbox),
            7 => "..\\..\\escape-win".into(),
            8 => "%2e%2e%2fescape-enc".into(),
            9 => "NaN".into(),
            10 => "-Infinity".into(),
            11 => "a/../../escape-mid".into(),
            12 => ".".into(),
            13 => "con:aux?*<>|".into(),
            14 => "y".repeat(300),
            _ => format!("s{}", self.rng.below(1000)),
        }
    }

    fn evil_number(&mut self) -> Value {
        match self.rng.below(14) {
            0 => json!(0),
            1 => json!(-1),
            2 => json!(i64::MAX),
            3 => json!(i64::MIN),
            4 => json!(u64::MAX),
            5 => json!(1e308),
            6 => json!(-1e308),
            7 => json!(5e-324),
            8 => json!(0.5),
            9 => json!(-0.0),
            10 => json!(1e20),
            11 => json!(RAW),
            12 => json!(self.rng.below(10)),
            _ => json!(self.rng.below(100_000) as f64 / 7.0 - 5000.0),
        }
    }

    fn evil_value(&mut self, depth: usize) -> Value {
        let n = if depth > 3 { self.rng.below(5) } else { self.rng.below(9) };
        match n {
            0 => Value::Null,
            1 => json!(self.rng.chance(0.5)),
            2 | 3 => self.evil_number(),
            4 => json!(self.evil_string()),
            5 => json!([]),
            6 => Value::Array((0..self.rng.below(4)).map(|_| self.evil_value(depth + 1)).collect()),
            7 => json!({}),
            _ => self.evil_object(depth + 1),
        }
    }

    fn evil_object(&mut self, depth: usize) -> Value {
        let mut m = serde_json::Map::new();
        for _ in 0..1 + self.rng.below(4) {
            let key = if self.rng.chance(0.85) { (*self.rng.pick(KEYS)).to_string() } else { self.evil_string().chars().take(64).collect() };
            let v = self.evil_value(depth + 1);
            m.insert(key, v);
        }
        Value::Object(m)
    }

    /// The route's own `GET` answer with one value mangled, a key dropped
    /// or an array blown up.
    fn mangle(&mut self, mut v: Value) -> Value {
        fn spots(v: &Value, at: &mut Vec<Vec<String>>, here: Vec<String>) {
            at.push(here.clone());
            match v {
                Value::Object(m) => m.iter().for_each(|(k, c)| spots(c, at, [here.clone(), vec![k.clone()]].concat())),
                Value::Array(a) => a.iter().enumerate().take(8).for_each(|(i, c)| spots(c, at, [here.clone(), vec![i.to_string()]].concat())),
                _ => {}
            }
        }
        let mut at = Vec::new();
        spots(&v, &mut at, Vec::new());
        let path = self.rng.pick(&at).clone();
        let mut target = &mut v;
        for k in &path {
            target = match target {
                Value::Object(m) => m.get_mut(k).unwrap(),
                Value::Array(a) => &mut a[k.parse::<usize>().unwrap()],
                _ => unreachable!(),
            };
        }
        match self.rng.below(4) {
            0 if target.is_object() => {
                let keys: Vec<String> = target.as_object().unwrap().keys().cloned().collect();
                if !keys.is_empty() {
                    let k = self.rng.pick(&keys).clone();
                    target.as_object_mut().unwrap().remove(&k);
                }
            }
            1 if target.is_array() => {
                let item = target.as_array().unwrap().first().cloned().unwrap_or(json!(0));
                *target = Value::Array(vec![item; 2000 + self.rng.below(20_000)]);
            }
            _ => *target = self.evil_value(2),
        }
        v
    }

    /// One body for `path`: (bytes, description for the failure message).
    fn body(&mut self, path: &str, seed: Option<&Value>) -> (Vec<u8>, &'static str) {
        let broken: [(&[u8], &'static str); 6] = [
            (b"{\"", "truncated"),
            (b"{\"on\":tru", "truncated literal"),
            (b"not json at all", "garbage"),
            (b"\xff\xfe\x00{\"id\":1}", "invalid UTF-8"),
            (b"{\"a\":1}}", "trailing"),
            (b"[[[[[[[[", "open arrays"),
        ];
        if broken_only(path) || self.rng.chance(0.2) {
            let (b, why) = *self.rng.pick(&broken);
            return (b.to_vec(), why);
        }
        let (v, why): (Value, &'static str) = match self.rng.below(12) {
            0 => return (Vec::new(), "empty"),
            1 => (self.evil_number(), "number"),
            2 => (json!(self.evil_string()), "string"),
            3 => (json!(self.rng.chance(0.5)), "bool"),
            4 => (Value::Null, "null"),
            5 => (Value::Array(vec![json!(0); 200_000]), "giant array"),
            6 => return (format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)).into_bytes(), "deep nesting"),
            7 => return (format!("\"{}\"", "z".repeat(1 << 20)).into_bytes(), "1 MB string"),
            8 | 9 if seed.is_some() => (self.mangle(seed.unwrap().clone()), "own GET mangled"),
            _ => (self.evil_object(0), "object"),
        };
        let mut text = v.to_string();
        while let Some(i) = text.find(&format!("\"{RAW}\"").replace('\u{1}', "\\u0001")) {
            let token = *self.rng.pick(RAW_NUMBERS);
            let len = format!("\"{RAW}\"").replace('\u{1}', "\\u0001").len();
            text.replace_range(i..i + len, token);
        }
        (text.into_bytes(), why)
    }

    /// A query string for a GET route.
    fn query(&mut self) -> String {
        let k = *self.rng.pick(&["name", "id", "file", "path", "ms", "page", "full", "since", "n", "kind", "output", "q"]);
        let v = match self.rng.below(6) {
            0 => "..%2F..%2F..%2Fetc%2Fpasswd".to_string(),
            1 => "NaN".into(),
            2 => "-1".into(),
            3 => "99999999999999999999".into(),
            4 => "%F0%9F%94%A5%00".into(),
            _ => "../../escape-get".into(),
        };
        format!("?{k}={v}")
    }
}

// ------------------------------------------------------------------ http

struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// One request on its own connection. `Err` on a connection or read
/// failure (a dead or hung server).
fn http(port: u16, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<Resp, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let head = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len());
    // The server may answer (413) and close before the body is all sent.
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body));
    read_response(&mut stream)
}

fn read_response(stream: &mut TcpStream) -> Result<Resp, String> {
    let mut raw = Vec::new();
    if let Err(e) = stream.read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(format!("read: {e}"));
        }
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| format!("no HTTP head in {} bytes", raw.len()))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("bad status line {head:?}"))?;
    let body = &raw[split + 4..];
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(body) } else { body.to_vec() };
    Ok(Resp { status, body })
}

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

/// A body announced at 64 MB, sent until the server answers: it must
/// answer 413 (or 400) without waiting for the whole body.
fn oversized(port: u16, path: &str) -> Result<(u16, usize), String> {
    const ANNOUNCED: usize = 64 << 20;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
    stream.set_write_timeout(Some(Duration::from_millis(500))).ok();
    write!(stream, "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {ANNOUNCED}\r\n\r\n").map_err(|e| e.to_string())?;
    let chunk = vec![b' '; 1 << 20];
    let mut sent = 0;
    while sent < ANNOUNCED {
        if stream.write_all(&chunk).is_err() {
            break;
        }
        sent += chunk.len();
    }
    read_response(&mut stream).map(|r| (r.status, sent))
}

// ---------------------------------------------------------------- studio

struct Studio {
    child: Child,
    port: u16,
    root: PathBuf,
    data: PathBuf,
}

impl Studio {
    fn start(tag: &str, extra: &[&str]) -> Self {
        Self::start_limited(tag, extra, None)
    }

    /// With at most `fds` open files (`ulimit -n`), when given.
    fn start_limited(tag: &str, extra: &[&str], fds: Option<u32>) -> Self {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_ne!(port, 8080, "never the user's instance");
        let root = std::env::temp_dir().join(format!("laser-studio-t449-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The data directory two levels down, so `../../x` from it, or
        // from the working directory beside it, lands in the sandbox.
        let (data, cwd) = (root.join("a/b/data"), root.join("a/b/cwd"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let bin = env!("CARGO_BIN_EXE_laser-studio");
        let mut command = match fds {
            Some(n) => {
                let mut c = Command::new("/bin/sh");
                c.args(["-c", &format!("ulimit -n {n} && exec \"$0\" \"$@\""), bin]);
                c
            }
            None => Command::new(bin),
        };
        let child = command
            .args(["--no-midi", "--no-audio", "--port", &port.to_string(), "--data-dir"])
            .arg(&data)
            .arg("--test-output")
            .arg(root.join("output.log"))
            .args(["--exit-with-parent", &std::process::id().to_string()])
            .args(extra)
            .current_dir(&cwd)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(root.join("stderr.log")).unwrap())
            .spawn()
            .unwrap();
        let studio = Self { child, port, root, data };
        let deadline = Instant::now() + Duration::from_secs(20);
        while studio.get("/api/state").is_err() {
            assert!(Instant::now() < deadline, "studio did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        studio
    }

    fn get(&self, path: &str) -> Result<Resp, String> {
        http(self.port, "GET", path, b"", Duration::from_secs(10))
    }

    fn post(&self, path: &str, body: &str) -> Result<Resp, String> {
        http(self.port, "POST", path, body.as_bytes(), Duration::from_secs(10))
    }

    /// `Err` with the end of stderr when the process is gone.
    fn alive(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(code)) => Err(format!("studio exited ({code}); stderr tail:\n{}", self.stderr_tail())),
            Err(e) => Err(format!("try_wait: {e}")),
        }
    }

    fn stderr_tail(&self) -> String {
        let text = std::fs::read_to_string(self.root.join("stderr.log")).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(15)..].join("\n")
    }

    /// The fake output's log lines.
    fn output_log(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("output.log")).unwrap_or_default().lines().map(String::from).collect()
    }

    fn armed(&self) -> Result<bool, String> {
        let r = self.get("/api/arm")?;
        r.json()["armed"].as_bool().ok_or_else(|| format!("GET /api/arm: {}", String::from_utf8_lossy(&r.body)))
    }

    fn rss_kb(&self) -> u64 {
        let out = Command::new("ps").args(["-o", "rss=", "-p", &self.child.id().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
    }

    /// The process's memory as macOS counts it (`footprint`, what Activity
    /// Monitor shows): unlike the RSS, without the pages the allocator has
    /// freed but not yet handed back. The RSS where `footprint` is missing.
    fn footprint_kb(&self) -> u64 {
        let out = Command::new("footprint").arg(self.child.id().to_string()).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let parsed = text.split("Footprint:").nth(1).and_then(|rest| {
            let mut words = rest.split_whitespace();
            let n: f64 = words.next()?.parse().ok()?;
            let unit = match words.next()? {
                "KB" => 1.0,
                "MB" => 1024.0,
                "GB" => 1024.0 * 1024.0,
                _ => return None,
            };
            Some((n * unit) as u64)
        });
        parsed.unwrap_or_else(|| self.rss_kb())
    }

    /// Files under the sandbox but outside the data directory (the
    /// harness's own two logs aside).
    fn escaped_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk(&self.root, &mut |p| {
            let own = [self.root.join("output.log"), self.root.join("stderr.log")];
            if !p.starts_with(&self.data) && !own.contains(&p.to_path_buf()) {
                out.push(p.to_path_buf());
            }
        });
        out
    }

    /// The data directory's files (logs aside): path → (size, mtime).
    fn data_files(&self) -> BTreeMap<String, (u64, u128)> {
        let mut out = BTreeMap::new();
        walk(&self.data, &mut |p| {
            let rel = p.strip_prefix(&self.data).unwrap().to_string_lossy().to_string();
            if !rel.starts_with("logs") {
                let m = std::fs::metadata(p).ok();
                let mtime = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                out.insert(rel, (m.map_or(0, |m| m.len()), mtime));
            }
        });
        out
    }

    /// E-stop, timed, then reset by the operator. Err if slower than `limit`.
    fn estop_roundtrip(&self, limit: Duration) -> Result<Duration, String> {
        let t = Instant::now();
        let r = http(self.port, "POST", "/api/estop?source=ui", b"", limit + Duration::from_secs(2))?;
        let took = t.elapsed();
        if r.status != 200 || took > limit {
            return Err(format!("e-stop answered {} in {took:?} (limit {limit:?})", r.status));
        }
        self.post("/api/estop/reset", r#"{"source":"ui"}"#)?;
        Ok(took)
    }

    fn perf_frames(&self) -> Result<u64, String> {
        self.get("/api/perf")?.json()["frames"].as_u64().ok_or_else(|| "no frames in /api/perf".to_string())
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("T-449: sandbox kept for inspection: {}", self.root.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

// ------------------------------------------------------- state compare

/// What a refused request must leave alone.
#[derive(Clone)]
struct Snapshot {
    state: Value,
    project: Value,
    files: BTreeMap<String, (u64, u128)>,
}

/// JSON paths (array indices as `[]`) whose values differ.
fn diff(a: &Value, b: &Value, here: &str, out: &mut BTreeSet<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                diff(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &format!("{here}/{k}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).for_each(|(u, v)| diff(u, v, &format!("{here}[]"), out)),
        _ if a != b => {
            out.insert(here.to_string());
        }
        _ => {}
    }
}

/// Mirrors other threads refresh on their own schedule, so they may catch
/// up with an earlier, accepted request during a refused one:
/// `project_modified` is the autosave thread's cached copy of what
/// `/api/project` computes live (and compares).
const REFRESHED_ELSEWHERE: &[&str] = &["state/project_modified"];

/// Clocks and meters: parts of the state that move on their own. Learnt
/// at start-up (and when a difference turns out to move by itself).
#[derive(Default)]
struct Volatile(BTreeSet<String>);

impl Volatile {
    fn ignores(&self, path: &str) -> bool {
        REFRESHED_ELSEWHERE.contains(&path) || self.0.iter().any(|v| path == v || path.starts_with(&format!("{v}/")) || path.starts_with(&format!("{v}[]")))
    }

    /// Differences between two snapshots, clocks aside.
    fn changes(&self, a: &Snapshot, b: &Snapshot) -> Vec<String> {
        let mut out = BTreeSet::new();
        diff(&a.state, &b.state, "state", &mut out);
        diff(&a.project, &b.project, "project", &mut out);
        let mut changes: Vec<String> = out.into_iter().filter(|p| !self.ignores(p)).collect();
        for k in a.files.keys().chain(b.files.keys()) {
            if a.files.get(k) != b.files.get(k) && !self.ignores(&format!("file/{k}")) {
                changes.push(format!("file/{k}"));
            }
        }
        changes.dedup();
        changes
    }

    fn learn(&mut self, a: &Snapshot, b: &Snapshot) {
        let found = self.changes(a, b);
        self.0.extend(found);
    }
}

fn snapshot(studio: &Studio) -> Result<Snapshot, String> {
    Ok(Snapshot { state: studio.get("/api/state")?.json(), project: studio.get("/api/project")?.json(), files: studio.data_files() })
}

// --------------------------------------------------------------- the fuzz

struct Fuzz {
    studio: Studio,
    rng: Rng,
    seed: u64,
    volatile: Volatile,
    /// `GET` answers of POST routes that have one, as mutation seeds.
    seeds: BTreeMap<String, Value>,
    explicit_arms: usize,
    last_frames: u64,
    worst_estop: Duration,
    /// The most the studio grew (kB) on one 64 MB body.
    oversized_max_growth_kb: u64,
    /// Refused requests during which another thread changed the state.
    async_changes: usize,
    statuses: BTreeMap<u16, usize>,
    sent: usize,
    /// Compare state around every refused request (costly: off for the
    /// concurrent and endurance runs).
    check_unchanged: bool,
}

impl Fuzz {
    fn new(tag: &str, seed: u64, extra: &[&str]) -> Self {
        let studio = Studio::start(tag, extra);
        let mut seeds = BTreeMap::new();
        for (m, p) in targets() {
            if m == "POST" && targets().contains(&("GET".to_string(), p.clone())) {
                if let Ok(r) = studio.get(&p) {
                    if r.status == 200 {
                        seeds.insert(p, r.json());
                    }
                }
            }
        }
        let mut fuzz = Fuzz {
            studio,
            rng: Rng::new(seed),
            seed,
            volatile: Volatile::default(),
            seeds,
            explicit_arms: 0,
            last_frames: 0,
            worst_estop: Duration::ZERO,
            oversized_max_growth_kb: 0,
            async_changes: 0,
            statuses: BTreeMap::new(),
            sent: 0,
            check_unchanged: true,
        };
        fuzz.learn_clocks(8);
        fuzz.last_frames = fuzz.studio.perf_frames().unwrap();
        fuzz
    }

    /// Samples the idle state a few times: what differs is a clock.
    fn learn_clocks(&mut self, samples: usize) {
        let mut prev = snapshot(&self.studio).unwrap();
        for _ in 0..samples {
            std::thread::sleep(Duration::from_millis(120));
            let next = snapshot(&self.studio).unwrap();
            self.volatile.learn(&prev, &next);
            prev = next;
        }
    }

    fn fail(&self, what: &str) -> String {
        format!("{what}\n(replay: FUZZ_SEED={} — case {} — sandbox {})", self.seed, self.sent, self.studio.root.display())
    }

    /// One generated request to (method, path), then the invariants.
    fn case(&mut self, method: &str, path: &str) -> Result<(), String> {
        let seed = self.seeds.get(path).cloned();
        let mut gen = Gen { rng: &mut self.rng, sandbox: self.studio.root.to_string_lossy().to_string() };
        let (url, body, why) = if method == "GET" {
            (format!("{path}{}", gen.query()), Vec::new(), "query")
        } else {
            let (b, why) = gen.body(path, seed.as_ref());
            (path.to_string(), b, why)
        };
        let before = if self.check_unchanged && method == "POST" { Some(snapshot(&self.studio)?) } else { None };
        self.sent += 1;
        let shown = String::from_utf8_lossy(&body[..body.len().min(160)]).to_string();
        let resp = http(self.studio.port, method, &url, &body, Duration::from_secs(30)).map_err(|e| self.fail(&format!("{method} {url} ({why}) {shown:?}: no answer: {e}")))?;
        *self.statuses.entry(resp.status).or_default() += 1;
        self.studio.alive().map_err(|e| self.fail(&format!("{method} {url} ({why}) {shown:?}: {e}")))?;
        let busy = resp.status == 503 && DECODING.contains(&path);
        if resp.status >= 500 && !busy {
            return Err(self.fail(&format!("{method} {url} ({why}) {shown:?}: {} {}", resp.status, String::from_utf8_lossy(&resp.body))));
        }
        self.check_arm(method, path, &body, resp.status)?;
        if let (Some(before), true) = (before, (400..500).contains(&resp.status)) {
            self.check_unchanged(&before, method, &url, &body, why, &shown, &resp)?;
        }
        Ok(())
    }

    /// Armed only right after an accepted explicit arm request, which the
    /// harness undoes; the output log never shows more arms than those.
    fn check_arm(&mut self, method: &str, path: &str, body: &[u8], status: u16) -> Result<(), String> {
        let armed = self.studio.armed()?;
        let explicit = method == "POST" && path == "/api/arm" && (200..300).contains(&status);
        if armed {
            if !explicit {
                return Err(self.fail(&format!("ARMED after {method} {path} {:?} ({status})", String::from_utf8_lossy(&body[..body.len().min(200)]))));
            }
            self.explicit_arms += 1;
            self.studio.post("/api/arm", r#"{"on":false,"source":"ui"}"#)?;
            if self.studio.armed()? {
                return Err(self.fail("could not disarm"));
            }
        }
        let arms = self.studio.output_log().iter().filter(|l| *l == "arm").count();
        if arms > self.explicit_arms {
            return Err(self.fail(&format!("the output was armed {arms} times, {} explicit arm requests (last: {method} {path})", self.explicit_arms)));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn check_unchanged(&mut self, before: &Snapshot, method: &str, url: &str, body: &[u8], why: &str, shown: &str, resp: &Resp) -> Result<(), String> {
        let after = snapshot(&self.studio)?;
        let changed = self.volatile.changes(before, &after);
        if changed.is_empty() {
            return Ok(());
        }
        // Moving by itself (a bar counter moves every few seconds)? Then
        // it's a clock: learn it and go on.
        let mut moving = BTreeSet::new();
        let mut real: Vec<String> = changed.clone();
        let deadline = Instant::now() + Duration::from_millis(4500);
        while !real.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            let later = snapshot(&self.studio)?;
            diff(&after.state, &later.state, "state", &mut moving);
            diff(&after.project, &later.project, "project", &mut moving);
            real.retain(|c| !moving.contains(c));
        }
        self.volatile.0.extend(moving);
        if real.is_empty() {
            return Ok(());
        }
        // A one-off change finished by another thread (a show list being
        // prepared after an earlier, accepted request) or this request?
        // Replay it on a quiet studio: a refused request that changes
        // things does so again (a revision bumped, a value half applied).
        let quiet = snapshot(&self.studio)?;
        let again = http(self.studio.port, method, url, body, Duration::from_secs(30)).map_err(|e| self.fail(&format!("{method} {url} replayed: {e}")))?;
        let replayed = self.volatile.changes(&quiet, &snapshot(&self.studio)?);
        let real: Vec<&String> = real.iter().filter(|c| replayed.contains(c)).collect();
        if real.is_empty() || !(400..500).contains(&again.status) {
            self.async_changes += 1;
            return Ok(());
        }
        Err(self.fail(&format!(
            "{method} {url} ({why}) {shown:?} answered {} {:?} but changed {real:?}",
            resp.status,
            String::from_utf8_lossy(&resp.body[..resp.body.len().min(200)])
        )))
    }

    /// E-stop latency, engine ticking, files: every few cases.
    fn checkpoint(&mut self) -> Result<(), String> {
        self.studio.alive().map_err(|e| self.fail(&e))?;
        let took = self.studio.estop_roundtrip(Duration::from_millis(250)).map_err(|e| self.fail(&e))?;
        self.worst_estop = self.worst_estop.max(took);
        // A frame every ~17 ms: give it a second before calling it stopped.
        let mut frames = self.studio.perf_frames()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while frames <= self.last_frames && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            frames = self.studio.perf_frames()?;
        }
        if frames <= self.last_frames {
            let state = self.studio.get("/api/state").map(|r| r.json()).unwrap_or_default();
            let perf = self.studio.get("/api/perf").map(|r| r.json()).unwrap_or_default();
            let alive = self.studio.alive();
            return Err(self.fail(&format!("the engine stopped ticking ({frames} frames, {} before): engine_ok {}, perf {perf}, alive {alive:?}", self.last_frames, state["engine_ok"])));
        }
        self.last_frames = frames;
        let state = self.studio.get("/api/state")?.json();
        if state["engine_ok"] != json!(true) {
            return Err(self.fail(&format!("engine_ok is {}", state["engine_ok"])));
        }
        let escaped = self.studio.escaped_files();
        if !escaped.is_empty() {
            return Err(self.fail(&format!("files written outside the data directory: {escaped:?}")));
        }
        Ok(())
    }

    /// `cases` generated requests per route, plus one 64 MB body per POST
    /// route.
    fn run(&mut self, cases: usize) -> Result<(), String> {
        let all = targets();
        for (method, path) in &all {
            if method == "POST" && !path.starts_with("/api/test/") {
                // Refused (413, 400…), or ignored by a route that reads no
                // body; never a 5xx, never held in memory.
                let rss = self.studio.rss_kb();
                let (status, sent) = oversized(self.studio.port, path).map_err(|e| self.fail(&format!("64 MB body to {path}: {e}")))?;
                // The file imports read up to their own, larger limits
                // (a 1 GB package, a 512 MB song) by design.
                let grew = if DECODING.contains(&path.as_str()) { 0 } else { self.studio.rss_kb().saturating_sub(rss) };
                if (status >= 500 && !(status == 503 && DECODING.contains(&path.as_str()))) || grew > 40 * 1024 {
                    return Err(self.fail(&format!("64 MB body to {path}: {status} after {} MB sent, memory +{} MB", sent >> 20, grew >> 10)));
                }
                self.oversized_max_growth_kb = self.oversized_max_growth_kb.max(grew);
                self.studio.alive().map_err(|e| self.fail(&e))?;
                self.check_arm("POST", path, b"", status)?;
            }
            for _ in 0..cases {
                self.case(method, path)?;
            }
            self.checkpoint()?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ tests

/// Every route of the table is driven: a prefix route without its
/// sub-routes here fails, and the network routes stay broken-only.
#[test]
fn every_route_is_fuzzed() {
    let table = route_table();
    assert!(table.len() > 200, "parsed {} routes", table.len());
    let t = targets();
    for (m, p, _) in &table {
        let m = if m == "*" { "POST" } else { m.as_str() };
        match p.strip_suffix('*') {
            Some(prefix) => assert!(t.iter().any(|(tm, tp)| tm == m && tp.starts_with(prefix)), "{m} {p}"),
            None => assert!(NEVER.contains(&(m, p.as_str())) || t.contains(&(m.to_string(), p.clone())), "{m} {p}"),
        }
    }
    for (_, p) in &t {
        if ["/api/outputs/select", "/api/netin", "/api/osc/config", "/api/dmx/config", "/api/external/gate"].contains(&p.as_str()) {
            assert!(broken_only(p), "{p}");
        }
    }
    // Only unparseable bodies for those routes.
    let mut rng = Rng::new(1);
    for _ in 0..200 {
        let (b, _) = Gen { rng: &mut rng, sandbox: "/x".into() }.body("/api/outputs/select", None);
        assert!(serde_json::from_slice::<Value>(&b).is_err(), "{}", String::from_utf8_lossy(&b));
    }
}

/// The comparator: equal apart from the clocks it learnt.
#[test]
fn the_state_comparator_ignores_clocks_only() {
    let snap = |state: Value| Snapshot { state, project: json!({ "name": "p" }), files: BTreeMap::from([("a.json".to_string(), (3, 7))]) };
    let a = snap(json!({ "t": 1.0, "beat": { "phase": 0.1 }, "cues": [{ "id": 1, "level": 0.5 }], "armed": false }));
    let b = snap(json!({ "t": 2.0, "beat": { "phase": 0.7 }, "cues": [{ "id": 1, "level": 0.5 }], "armed": false }));
    let mut v = Volatile::default();
    assert_eq!(v.changes(&a, &b), ["state/beat/phase", "state/t"]);
    v.learn(&a, &b);
    assert!(v.changes(&a, &b).is_empty(), "two states equal but for the clocks are equal");
    let c = snap(json!({ "t": 3.0, "beat": { "phase": 0.2 }, "cues": [{ "id": 1, "level": 0.9 }], "armed": false }));
    assert_eq!(v.changes(&a, &c), ["state/cues[]/level"]);
    let mut d = c.clone();
    d.files.insert("a.json".into(), (4, 8));
    d.project = json!({ "name": "q" });
    assert_eq!(v.changes(&c, &d), ["project/name", "file/a.json"]);
}

/// The generator replays: the same seed, the same requests.
#[test]
fn the_generator_is_reproducible() {
    let run = |seed| {
        let mut rng = Rng::new(seed);
        (0..50).map(|_| Gen { rng: &mut rng, sandbox: "/s".into() }.body("/api/settings", Some(&json!({ "a": [1, 2], "b": { "c": "d" } }))).0).collect::<Vec<_>>()
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

/// Every route, `FUZZ_CASES` (default 6) generated requests each, with
/// every invariant of the module doc checked after each one.
#[test]
fn api_fuzz() {
    let seed = env_u64("FUZZ_SEED", 449);
    let mut fuzz = Fuzz::new("fuzz", seed, &[]);
    fuzz.run(env_u64("FUZZ_CASES", 6) as usize).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("T-449 api_fuzz: seed {seed}, {} requests, answers {:?}, worst e-stop {:?}, 64 MB bodies grew memory by {} kB at most, {} refusals overlapped a change made by another thread", fuzz.sent, fuzz.statuses, fuzz.worst_estop, fuzz.oversized_max_growth_kb, fuzz.async_changes);
}

/// The long fuzz: `FUZZ_CASES` per route (default 200; 20000 for a night).
#[test]
#[ignore]
fn api_fuzz_long() {
    let seed = env_u64("FUZZ_SEED", 449);
    let mut fuzz = Fuzz::new("fuzz-long", seed, &[]);
    fuzz.run(env_u64("FUZZ_CASES", 200) as usize).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("T-449 api_fuzz_long: seed {seed}, {} requests, answers {:?}, worst e-stop {:?}, 64 MB bodies grew memory by {} kB at most, {} refusals overlapped a change made by another thread", fuzz.sent, fuzz.statuses, fuzz.worst_estop, fuzz.oversized_max_growth_kb, fuzz.async_changes);
}

/// The harness sees a panic: a `--test-hooks` route that panics on
/// purpose fails the case (the studio's panic hook stops it).
#[test]
fn the_fuzz_detects_a_panic() {
    let mut fuzz = Fuzz::new("panic", 1, &["--test-hooks"]);
    let err = fuzz.case("POST", "/api/test/panic").unwrap_err();
    // A 500 from the unwinding worker, no answer, or the studio gone.
    assert!(err.contains("/api/test/panic") && (err.contains(": 500") || err.contains("studio exited") || err.contains("no answer")), "{err}");
    assert!(err.contains("FUZZ_SEED=1"), "{err}");
    // The panic hook cut the output and stopped the studio.
    let deadline = Instant::now() + Duration::from_secs(10);
    while fuzz.studio.alive().is_ok() {
        assert!(Instant::now() < deadline, "the studio carried on after a panic");
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = fuzz.studio.output_log();
    assert!(!log.iter().any(|l| l == "arm"), "{log:?}");
}

/// Hostile connections (the tiny_http patches, `vendor/tiny_http`): an
/// absurd Content-Length used to abort the studio (a buffer that size,
/// allocated to drain the unread body), an endless header line grew
/// without limit, and a body cut short or an answer never read held the
/// single worker, so every route but the stop, for good.
#[test]
fn hostile_connections_neither_crash_nor_hold_the_studio() {
    let mut studio = Studio::start("hostile", &[]);
    let rss = studio.rss_kb();
    let port = studio.port;
    let connect = || TcpStream::connect(("127.0.0.1", port)).unwrap();
    // Absurd announced lengths, a few bytes sent, then gone.
    for n in ["1000000000000", "4611686018427387904", "18446744073709551615"] {
        let mut c = connect();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(c, "POST /api/about/seen HTTP/1.1\r\nHost: x\r\nContent-Length: {n}\r\n\r\nabc").unwrap();
        let _ = c.read(&mut [0u8; 64]);
        drop(c);
        std::thread::sleep(Duration::from_millis(200));
        studio.alive().unwrap_or_else(|e| panic!("Content-Length {n}: {e}"));
        assert_eq!(studio.get("/api/state").unwrap().status, 200);
    }
    // An endless request line, then endless headers.
    for head in [&b"GET /"[..], &b"GET / HTTP/1.1\r\n"[..]] {
        let mut c = connect();
        c.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(head).unwrap();
        let filler = if head.len() > 5 { b"X-A: b\r\n".repeat(4096) } else { vec![b'a'; 64 * 1024] };
        let mut sent = 0;
        while sent < 16 << 20 && c.write_all(&filler).is_ok() {
            sent += filler.len();
        }
        assert!(sent < 16 << 20, "the server read 16 MB of one request's head");
        studio.alive().unwrap();
    }
    let grew = studio.rss_kb().saturating_sub(rss);
    assert!(grew < 30 * 1024, "memory +{} MB", grew >> 10);
    // A body cut short, the connection kept open.
    let mut cut = connect();
    write!(cut, "POST /api/settings HTTP/1.1\r\nHost: x\r\nContent-Length: 100000\r\n\r\n{{\"settings\":").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    // T-481: the body is read on the connection's own thread; the worker
    // never waits on it.
    the_worker_comes_free(&mut studio, "a body cut short", Duration::from_secs(1));
    // Answers never read: many requests on one connection whose page
    // doesn't read (a tiny receive buffer).
    let stuck = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    stuck.set_recv_buffer_size(4096).unwrap();
    stuck.connect(&std::net::SocketAddr::from(([127, 0, 0, 1], port)).into()).unwrap();
    let mut stuck: TcpStream = stuck.into();
    stuck.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    for _ in 0..200 {
        if stuck.write_all(b"GET /api/state HTTP/1.1\r\nHost: x\r\n\r\n").is_err() {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    // T-489: the answers are written by the connection's own thread; the
    // worker never waits on a client that doesn't read.
    the_worker_comes_free(&mut studio, "answers never read", Duration::from_secs(1));
    drop((cut, stuck));
    assert!(!studio.output_log().iter().any(|l| l == "arm"));
}

/// Out of file descriptors (a client holding hundreds of connections, a
/// Mac's default limit of 256): the server used to stop accepting for
/// good, the e-stop route included. It must come back once they close.
#[test]
fn running_out_of_file_descriptors_never_stops_the_server_for_good() {
    let mut studio = Studio::start_limited("fds", &[], Some(96));
    let mut held = Vec::new();
    for _ in 0..200 {
        match TcpStream::connect(("127.0.0.1", studio.port)) {
            Ok(c) => held.push(c),
            Err(_) => break,
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    studio.alive().unwrap();
    drop(held);
    // Closed connections free their descriptors (at once, or on their
    // read timeout); the stop and the rest answer again.
    let deadline = Instant::now() + Duration::from_secs(15);
    let estop = loop {
        if let Ok(r) = http(studio.port, "POST", "/api/estop?source=ui", b"", Duration::from_secs(2)) {
            break r;
        }
        assert!(Instant::now() < deadline, "the server never accepted again");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(estop.status, 200);
    studio.alive().unwrap();
    assert_eq!(studio.get("/api/state").unwrap().status, 200);
    assert_eq!(studio.post("/api/estop/reset", r#"{"source":"ui"}"#).unwrap().status, 200);
    assert!(!studio.output_log().iter().any(|l| l == "arm"));
}

/// The stop answers at once; the other routes within `limit` (at once
/// since T-481 and T-489: neither a stalled sender nor a stalled reader
/// holds the worker).
fn the_worker_comes_free(studio: &mut Studio, what: &str, limit: Duration) {
    let t = Instant::now();
    assert_eq!(http(studio.port, "POST", "/api/estop?source=ui", b"", Duration::from_secs(2)).unwrap().status, 200);
    assert!(t.elapsed() < Duration::from_millis(250), "{what}: e-stop took {:?}", t.elapsed());
    let t = Instant::now();
    let state = http(studio.port, "GET", "/api/state", b"", Duration::from_secs(60)).unwrap_or_else(|e| panic!("{what}: the worker never came free: {e}"));
    assert_eq!(state.status, 200);
    eprintln!("T-449 hostile: {what}: the worker was held {:?}", t.elapsed());
    assert!(t.elapsed() < limit, "{what}: the worker was held {:?}", t.elapsed());
    assert_eq!(studio.post("/api/estop/reset", r#"{"source":"ui"}"#).unwrap().status, 200);
    studio.alive().unwrap();
}

// ------------------------------------------------------- slow clients (T-481)

/// A request on its own connection, timed (`Err` on no answer or a 5xx).
fn timed(port: u16, method: &str, path: &str, body: &str) -> Result<Duration, String> {
    let t = Instant::now();
    let r = http(port, method, path, body.as_bytes(), Duration::from_secs(10))?;
    if r.status >= 500 {
        return Err(format!("{method} {path}: {}", r.status));
    }
    Ok(t.elapsed())
}

const HEARTBEAT: &str = r#"{"client_id":"t481","visible":true}"#;

/// The slowest of a heartbeat and a state read, `rounds` times, 50 ms apart.
fn worst_heartbeat_and_state(port: u16, rounds: usize) -> Duration {
    let mut worst = Duration::ZERO;
    for _ in 0..rounds {
        worst = worst.max(timed(port, "POST", "/api/heartbeat", HEARTBEAT).unwrap());
        worst = worst.max(timed(port, "GET", "/api/state", "").unwrap());
        std::thread::sleep(Duration::from_millis(50));
    }
    worst
}

/// Writes `bytes` to `stream` one a second, until `stop`.
fn dribble(mut stream: TcpStream, bytes: Vec<u8>, stop: Arc<AtomicBool>) -> TcpStream {
    for b in bytes {
        if stop.load(Ordering::SeqCst) || stream.write_all(&[b]).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    stream
}

/// T-481: a client sending its request one byte a second (its head, or
/// its body after a complete head) holds only its own connection. Before,
/// a slow body was read on the single worker: no heartbeat (a disarm
/// through « Interface perdue »), no state, no UI until it ended.
#[test]
fn a_client_sending_one_byte_a_second_delays_nothing() {
    let mut studio = Studio::start("slow-client", &[]);
    let port = studio.port;
    let quiet = worst_heartbeat_and_state(port, 10);
    let stop = Arc::new(AtomicBool::new(false));
    let connect = || TcpStream::connect(("127.0.0.1", port)).unwrap();
    // Its head, a byte a second.
    let head = (b"POST /api/heartbeat HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}".to_vec(), connect(), Arc::clone(&stop));
    // A complete head, then its body a byte a second (a heartbeat, what
    // the worker serves most, padded past the 1 kB tiny_http itself reads
    // ahead).
    let padded = format!("{HEARTBEAT}{}", " ".repeat(2000));
    let mut body = connect();
    write!(body, "POST /api/heartbeat HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n", padded.len()).unwrap();
    let body = (padded.into_bytes(), body, Arc::clone(&stop));
    // A short body sent to its end: answered once it has all arrived.
    let mut short = connect();
    short.write_all(b"POST /api/heartbeat HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 3\r\n\r\n").unwrap();
    let short_sent = Instant::now();
    let short = (b"{} ".to_vec(), short, Arc::new(AtomicBool::new(false)));
    let threads: Vec<_> = [head, body, short].into_iter().map(|(bytes, s, stop)| std::thread::spawn(move || dribble(s, bytes, stop))).collect();
    std::thread::sleep(Duration::from_millis(300));
    let loaded = worst_heartbeat_and_state(port, 40);
    let estop = studio.estop_roundtrip(Duration::from_millis(50)).unwrap();
    // A stop whose body never comes is served on its head alone.
    let mut slow_stop = connect();
    slow_stop.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let t = Instant::now();
    slow_stop.write_all(b"POST /api/estop?source=ui HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{").unwrap();
    let mut first = [0u8; 12];
    slow_stop.read_exact(&mut first).unwrap();
    let slow_estop = t.elapsed();
    assert_eq!(&first, b"HTTP/1.1 200", "a stop with a body still to come");
    assert!(slow_estop < Duration::from_millis(50), "a stop with a body still to come took {slow_estop:?}");
    assert!(studio.get("/api/arm").unwrap().json()["estop"].is_object(), "the stop latched");
    assert_eq!(studio.post("/api/estop/reset", r#"{"source":"ui"}"#).unwrap().status, 200);
    // A chunked body is read ahead too.
    let mut chunked = connect();
    chunked.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(chunked, "POST /api/heartbeat HTTP/1.1\r\nHost: x\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{HEARTBEAT}\r\n0\r\n\r\n", HEARTBEAT.len()).unwrap();
    assert_eq!(read_response(&mut chunked).unwrap().status, 200, "a chunked heartbeat");
    stop.store(true, Ordering::SeqCst);
    let mut streams: Vec<TcpStream> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    // The short one got its answer, after its last byte.
    let mut short = streams.pop().unwrap();
    short.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let answer = read_response(&mut short).unwrap();
    assert!(answer.status < 500, "the slow but complete request: {}", answer.status);
    assert!(short_sent.elapsed() >= Duration::from_secs(2));
    eprintln!("T-481 slow client: heartbeat and state worst {quiet:?} quiet, {loaded:?} beside clients sending 1 byte/s; e-stop {estop:?}");
    assert!(loaded <= quiet + Duration::from_millis(100), "a client sending 1 byte/s delayed the heartbeat or the state: worst {loaded:?} against {quiet:?} quiet");
    studio.alive().unwrap();
    assert!(!studio.output_log().iter().any(|l| l == "arm"));
}

/// A connection whose client never reads: a receive buffer of 4 kB, then
/// `requests` sent in a row (as far as the server takes them).
fn never_reading(port: u16, requests: &[u8]) -> TcpStream {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    s.set_recv_buffer_size(4096).unwrap();
    s.connect(&std::net::SocketAddr::from(([127, 0, 0, 1], port)).into()).unwrap();
    let mut s: TcpStream = s.into();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(requests);
    s
}

/// T-489: clients that never read their answers (pipelined state reads,
/// the 1.4 MB page, frames) hold only their own connections. Before, each
/// answer was written by the single worker, which waited out the write
/// timeout (5 s): no heartbeat (a disarm through « Interface perdue »), no
/// state, a frozen UI, ~10 s with answers queued behind.
#[test]
fn a_client_that_never_reads_delays_nothing() {
    let mut studio = Studio::start("slow-reader", &[]);
    let port = studio.port;
    let quiet = worst_heartbeat_and_state(port, 10);
    let before = threads(&studio);
    let rss = studio.rss_kb();
    let mut stuck = Vec::new();
    for i in 0..6 {
        let one = match i % 3 {
            0 => &b"GET /api/state HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            1 => &b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            _ => &b"GET /api/frame.bin HTTP/1.1\r\nHost: x\r\n\r\n"[..],
        };
        stuck.push(never_reading(port, &one.repeat(100)));
    }
    std::thread::sleep(Duration::from_millis(300));
    let loaded = worst_heartbeat_and_state(port, 40);
    let estop = studio.estop_roundtrip(Duration::from_millis(50)).unwrap();
    let during = threads(&studio);
    let grew = studio.rss_kb().saturating_sub(rss);
    eprintln!("T-489 slow reader: heartbeat and state worst {quiet:?} quiet, {loaded:?} beside 6 clients that never read; e-stop {estop:?}; threads {before} → {during}; memory +{} MB", grew >> 10);
    assert!(loaded <= quiet + Duration::from_millis(100), "a client that never reads delayed the heartbeat or the state: worst {loaded:?} against {quiet:?} quiet");
    // One writing thread per stuck connection at most, beside the frame
    // threads (at most 16 frame requests waiting, push.rs), and the
    // answers piled up are bounded by the output budget (64 MB).
    assert!(during <= before + 6 + 16 + 4, "threads {before} → {during}");
    assert!(grew < 200 * 1024, "memory +{} MB", grew >> 10);
    // Past the write timeout their connections are dropped; the studio
    // still answers.
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(studio.get("/api/state").unwrap().status, 200);
    drop(stuck);
    studio.alive().unwrap();
    assert!(!studio.output_log().iter().any(|l| l == "arm"));
}

/// The studio's thread count (`ps -M`: one line per thread, and a header).
fn threads(studio: &Studio) -> usize {
    let out = Command::new("ps").args(["-M", "-p", &studio.child.id().to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().count().saturating_sub(1)
}

/// T-481: hundreds of slow clients at once, more than the descriptors a
/// Mac app gets by default (256) allow: the connections served are
/// capped, the one waiting longest for its client pushed out by a new
/// one. The stop always gets in and answers within 50 ms, the heartbeat
/// and the state keep answering, and the threads stay bounded.
#[test]
fn a_flood_of_slow_clients_never_delays_the_stop() {
    let mut studio = Studio::start_limited("slow-flood", &[], Some(256));
    let port = studio.port;
    let quiet = worst_heartbeat_and_state(port, 5);
    let before = threads(&studio);
    let stop = Arc::new(AtomicBool::new(false));
    let mut flood = Vec::new();
    for i in 0..300 {
        let Ok(mut c) = TcpStream::connect(("127.0.0.1", port)) else { continue };
        // Half stall in their head, half in their body.
        let started = if i % 2 == 0 { c.write_all(b"GET /api/st") } else { c.write_all(b"POST /api/settings HTTP/1.1\r\nHost: x\r\nContent-Length: 1000\r\n\r\n{") };
        if started.is_ok() {
            flood.push(c);
        }
    }
    // A byte a second on each (so they never time out); the ones pushed
    // out fail to write and are let go.
    let dribbler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                flood.retain_mut(|c| c.write_all(b"a").is_ok());
                std::thread::sleep(Duration::from_secs(1));
            }
            flood.len()
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    let mut worst_estop = Duration::ZERO;
    for _ in 0..20 {
        let took = studio.estop_roundtrip(Duration::from_millis(50)).unwrap_or_else(|e| panic!("under a flood of slow clients: {e}"));
        worst_estop = worst_estop.max(took);
        std::thread::sleep(Duration::from_millis(100));
    }
    let loaded = worst_heartbeat_and_state(port, 10);
    let during = threads(&studio);
    stop.store(true, Ordering::SeqCst);
    let left = dribbler.join().unwrap();
    eprintln!("T-481 flood: 300 slow clients ({left} still connected at the end), worst e-stop {worst_estop:?}, heartbeat and state worst {loaded:?} ({quiet:?} quiet), threads {before} → {during}");
    assert!(loaded <= quiet + Duration::from_millis(100), "the flood delayed the heartbeat or the state: {loaded:?} against {quiet:?}");
    assert!(during <= before + 48 + 8, "threads {before} → {during}");
    studio.alive().unwrap();
    assert!(!studio.output_log().iter().any(|l| l == "arm"));
}

/// Clients at once: fuzzing threads, slow readers that never read their
/// answer, slow senders that stall mid-body, while the e-stop is timed.
#[test]
fn concurrent_clients_slow_readers_and_slow_senders() {
    let secs = env_u64("CONCURRENT_SECS", 6);
    let mut fuzz = Fuzz::new("concurrent", env_u64("FUZZ_SEED", 449), &[]);
    let report = hammer(&mut fuzz, Duration::from_secs(secs), 6);
    eprintln!("T-449 concurrent: {report}");
}

/// Hours of mixed traffic (`ENDURANCE_SECS`, default 600): an operator
/// (controls, cues, timeline, project saves and opens, heartbeats), fuzz
/// clients, slow clients. Memory after a warm-up must not grow by more
/// than 10 %, the engine keeps ticking, nothing arms.
#[test]
#[ignore]
fn api_endurance_long() {
    let secs = env_u64("ENDURANCE_SECS", 600);
    let mut fuzz = Fuzz::new("endurance", env_u64("FUZZ_SEED", 449), &[]);
    let report = hammer(&mut fuzz, Duration::from_secs(secs), 4);
    eprintln!("T-449 endurance: {report}");
}

/// The concurrent run behind the last two tests. Returns a summary line.
fn hammer(fuzz: &mut Fuzz, length: Duration, clients: usize) -> String {
    fuzz.check_unchanged = false;
    let port = fuzz.studio.port;
    let stop = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<String>>> = Arc::default();
    let sent = Arc::new(AtomicU64::new(0));
    let all = Arc::new(targets());
    let sandbox = fuzz.studio.root.to_string_lossy().to_string();
    let mut threads = Vec::new();
    // Fuzz clients: random routes, random bodies; only liveness here (the
    // main thread checks arming and the rest).
    for c in 0..clients {
        let (stop, failures, sent, all, sandbox) = (Arc::clone(&stop), Arc::clone(&failures), Arc::clone(&sent), Arc::clone(&all), sandbox.clone());
        let seed = fuzz.seed.wrapping_add(c as u64 + 1);
        let seeds = fuzz.seeds.clone();
        threads.push(std::thread::spawn(move || {
            let mut rng = Rng::new(seed);
            while !stop.load(Ordering::SeqCst) {
                let (m, p) = all[rng.below(all.len())].clone();
                if p == "/api/arm" || p.starts_with("/api/test/") || p.starts_with("/api/project") || p == "/api/lock" {
                    continue; // arming is the main thread's; projects are the operator's
                }
                let mut gen = Gen { rng: &mut rng, sandbox: sandbox.clone() };
                let (url, body) = if m == "GET" { (format!("{p}{}", gen.query()), Vec::new()) } else { (p.clone(), gen.body(&p, seeds.get(&p)).0) };
                match http(port, &m, &url, &body, Duration::from_secs(30)) {
                    Ok(r) if r.status >= 500 && !(r.status == 503 && DECODING.contains(&p.as_str())) => {
                        failures.lock().unwrap().push(format!("{m} {url}: {} {}", r.status, String::from_utf8_lossy(&r.body)));
                    }
                    Ok(_) => {}
                    Err(e) => failures.lock().unwrap().push(format!("{m} {url}: no answer: {e}")),
                }
                sent.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    // The operator: heartbeats, controls, cues, the timeline, projects.
    {
        let (stop, failures, sent) = (Arc::clone(&stop), Arc::clone(&failures), Arc::clone(&sent));
        let seed = fuzz.seed;
        threads.push(std::thread::spawn(move || operator(port, seed, &stop, &failures, &sent)));
    }
    // Slow clients, now and then: answers never read, a body never
    // finished. Each holds only its own connection (T-481, T-489). The
    // stop is served beside them.
    {
        let stop = Arc::clone(&stop);
        threads.push(std::thread::spawn(move || {
            let mut held = Vec::new();
            let mut last = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
                if last.elapsed() < Duration::from_secs(20) && !held.is_empty() {
                    continue;
                }
                last = Instant::now();
                held.clear();
                if let Ok(s) = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None) {
                    let _ = s.set_recv_buffer_size(4096);
                    if s.connect(&std::net::SocketAddr::from(([127, 0, 0, 1], port)).into()).is_ok() {
                        let mut s: TcpStream = s.into();
                        let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
                        for _ in 0..50 {
                            let _ = s.write_all(b"GET /api/state HTTP/1.1\r\nHost: x\r\n\r\n");
                        }
                        held.push(s);
                    }
                }
                if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                    let _ = s.write_all(b"POST /api/settings HTTP/1.1\r\nHost: x\r\nContent-Length: 100000\r\n\r\n{\"settings\":");
                    held.push(s);
                }
            }
        }));
    }
    // Memory (`footprint`) every 5 s after the warm-up. Under this load it
    // swings by hundreds of MB (multi-MB fuzz bodies in flight, up to 50
    // undo entries holding mangled content), so what must not grow is its
    // floor: the lowest sample of the second half of the run against the
    // lowest of the first.
    let start = Instant::now();
    let warmup = (length / 5).min(Duration::from_secs(120));
    let mut samples: Vec<(Duration, u64)> = Vec::new();
    let mut peak = 0u64;
    let mut worst_frame = 0f64;
    let mut next_print = Instant::now();
    let mut result: Result<(), String> = Ok(());
    let mut next_sample = Instant::now();
    while start.elapsed() < length {
        std::thread::sleep(Duration::from_millis(200));
        // Nothing arms: the clients never send an arm request.
        match fuzz.studio.armed() {
            Ok(false) => {}
            Ok(true) => result = Err(fuzz.fail("ARMED during the concurrent run")),
            Err(e) => result = Err(fuzz.fail(&format!("GET /api/arm: {e}"))),
        }
        if result.is_ok() {
            result = fuzz.checkpoint();
        }
        if let Some(f) = failures.lock().unwrap().first() {
            result = Err(fuzz.fail(f));
        }
        if result.is_err() {
            break;
        }
        if Instant::now() >= next_sample {
            next_sample += Duration::from_secs(5);
            let memory = fuzz.studio.footprint_kb();
            peak = peak.max(memory);
            if start.elapsed() >= warmup {
                samples.push((start.elapsed(), memory));
            }
            let perf = fuzz.studio.get("/api/perf").map(|r| r.json()).unwrap_or_default();
            worst_frame = worst_frame.max(perf["frame"]["worst_ms"].as_f64().unwrap_or(0.0));
            if length >= Duration::from_secs(60) && Instant::now() >= next_print {
                next_print += Duration::from_secs(30);
                eprintln!(
                    "{:>5} s · mémoire {} Mo · pire image {:.1} ms · {} images en retard · {} requêtes",
                    start.elapsed().as_secs(),
                    memory / 1024,
                    perf["frame"]["worst_ms"].as_f64().unwrap_or(0.0),
                    perf["late_frames"],
                    sent.load(Ordering::Relaxed)
                );
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    for t in threads {
        let _ = t.join();
    }
    result.unwrap_or_else(|e| panic!("{e}"));
    fuzz.checkpoint().unwrap_or_else(|e| panic!("{e}"));
    let end = fuzz.studio.get("/api/state").expect("still answers");
    assert_eq!(end.status, 200);
    let log = fuzz.studio.output_log();
    assert!(!log.iter().any(|l| l == "arm" || l == "lit"), "the output never armed nor lit: {log:?}");
    let late = fuzz.studio.get("/api/perf").map(|r| r.json()["late_frames"].as_u64().unwrap_or(0)).unwrap_or(0);
    let mid = warmup + (length - warmup) / 2;
    let floor = |first: bool| samples.iter().filter(|(t, _)| (*t < mid) == first).map(|&(_, m)| m).min().unwrap_or(0);
    let (floor_a, floor_b) = (floor(true), floor(false));
    let growth = if floor_a > 0 { (floor_b as f64 / floor_a as f64 - 1.0) * 100.0 } else { 0.0 };
    // The floor within 10 % (and 8 MB of allocator slack).
    if length >= Duration::from_secs(300) {
        assert!(floor_b <= floor_a + floor_a / 10 + 8 * 1024, "the memory floor grew {growth:.1} % ({floor_a} → {floor_b} kB)");
    }
    format!(
        "{} s, {} requests, memory floor {} MB → {} MB ({growth:+.1} %; peak {} MB), worst frame {worst_frame:.1} ms, {late} late frames, worst e-stop {:?}, seed {}",
        length.as_secs(),
        sent.load(Ordering::Relaxed),
        floor_a / 1024,
        floor_b / 1024,
        peak / 1024,
        fuzz.worst_estop,
        fuzz.seed
    )
}

/// A scripted operator: heartbeats, controls with their real ids, cues,
/// tap tempo, the timeline (an example loaded, played, sought, looped),
/// project save-as / open / new. Never arms.
fn operator(port: u16, seed: u64, stop: &AtomicBool, failures: &Mutex<Vec<String>>, sent: &AtomicU64) {
    let mut rng = Rng::new(seed ^ 0x0BE7);
    let get = |p: &str| http(port, "GET", p, b"", Duration::from_secs(30)).map(|r| r.json()).unwrap_or_default();
    let controls: Vec<String> = get("/api/controls").as_array().map(|a| a.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    let cues: Vec<String> = get("/api/presets").as_array().map(|a| a.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    let examples: Vec<String> = get("/api/timeline/examples").as_array().map(|a| a.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    let mut step = 0u64;
    while !stop.load(Ordering::SeqCst) {
        step += 1;
        let (path, body) = match rng.below(10) {
            0 => ("/api/heartbeat", json!({ "client_id": "t449", "visible": true })),
            1 | 2 if !controls.is_empty() => {
                let id = rng.pick(&controls).clone();
                // Never the arm control: arming is not the operator script's.
                if id.contains("arm") {
                    continue;
                }
                ("/api/control", json!({ "id": id, "value": rng.below(100) as f64 / 99.0 }))
            }
            3 if !cues.is_empty() => ("/api/cue", json!({ "id": rng.pick(&cues), "mode": "toggle" })),
            4 => ("/api/control", json!({ "id": "tempo.tap" })),
            5 if !examples.is_empty() => ("/api/timeline/load", json!({ "example": rng.pick(&examples) })),
            6 => (*rng.pick(&["/api/timeline/play", "/api/timeline/stop", "/api/timeline/pause"]), json!({})),
            7 => ("/api/timeline/seek", json!({ "position": rng.below(60) as f64 })),
            8 if step.is_multiple_of(5) => ("/api/project/save-as", json!({ "path": format!("endurance-{}", rng.below(3)) })),
            9 if step.is_multiple_of(7) => ("/api/project/open", json!({ "path": format!("endurance-{}", rng.below(3)) })),
            _ => ("/api/heartbeat", json!({ "client_id": "t449", "visible": true })),
        };
        match http(port, "POST", path, body.to_string().as_bytes(), Duration::from_secs(30)) {
            Ok(r) if r.status >= 500 => failures.lock().unwrap().push(format!("operator {path} {body}: {} {}", r.status, String::from_utf8_lossy(&r.body))),
            Ok(_) => {}
            Err(e) => failures.lock().unwrap().push(format!("operator {path}: no answer: {e}")),
        }
        sent.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(20));
    }
}
