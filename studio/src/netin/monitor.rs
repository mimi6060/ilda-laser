//! The OSC monitor (T-416): the last `MONITOR_LEN` messages in and out,
//! in memory only (never written to disk), each line bounded in size so a
//! sender with 8 KB addresses can't make it grow.

use super::osc::OscArg;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MONITOR_LEN: usize = 200;
/// Characters kept of an address or of the arguments.
const MAX_TEXT: usize = 120;

pub const APPLIED: &str = "appliqué";
pub const REFUSED: &str = "refusé";
pub const UNKNOWN: &str = "inconnu";
pub const SENT: &str = "envoyé";
/// The sending queue was full: dropped, sent again at the next tick.
pub const DROPPED: &str = "abandonné";

#[derive(Clone, Debug, Serialize)]
pub struct OscLogEntry {
    /// Wall clock, ms since 1970 (the UI shows the local time).
    pub at_ms: u64,
    pub incoming: bool,
    /// The client's IP (in) or `ip:port` (out).
    pub peer: String,
    pub address: String,
    pub args: String,
    pub result: &'static str,
    /// Why, when refused or unknown (French).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Default)]
pub struct Monitor {
    entries: VecDeque<OscLogEntry>,
}

fn bounded(s: &str) -> String {
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// `0.25 1 "abc"`: the arguments as the monitor shows them.
pub fn args_text(args: &[OscArg]) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|a| match a {
            OscArg::Int(i) => i.to_string(),
            OscArg::Float(f) => format!("{f}"),
            OscArg::Double(d) => format!("{d}"),
            OscArg::Long(l) => l.to_string(),
            OscArg::Str(s) => format!("{s:?}"),
            OscArg::Bool(b) => b.to_string(),
            OscArg::Other => "·".into(),
        })
        .collect();
    parts.join(" ")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Monitor {
    pub fn push(&mut self, incoming: bool, peer: String, address: &str, args: &str, result: &'static str, detail: Option<String>) {
        if self.entries.len() >= MONITOR_LEN {
            self.entries.pop_front();
        }
        let detail = detail.map(|d| bounded(&d));
        self.entries.push_back(OscLogEntry { at_ms: now_ms(), incoming, peer, address: bounded(address), args: bounded(args), result, detail });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Newest first; `incoming`: only that direction.
    pub fn list(&self, incoming: Option<bool>) -> Vec<OscLogEntry> {
        self.entries.iter().rev().filter(|e| incoming.is_none_or(|i| e.incoming == i)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_is_bounded_and_lines_are_short() {
        let mut m = Monitor::default();
        for i in 0..(MONITOR_LEN + 50) {
            m.push(i % 2 == 0, "127.0.0.1".into(), &format!("/ls/m/{i}"), "1", APPLIED, None);
        }
        assert_eq!(m.len(), MONITOR_LEN);
        let all = m.list(None);
        assert_eq!(all[0].address, format!("/ls/m/{}", MONITOR_LEN + 49), "newest first");
        assert_eq!(m.list(Some(true)).len() + m.list(Some(false)).len(), MONITOR_LEN);
        assert!(m.list(Some(false)).iter().all(|e| !e.incoming));
        let long = "/".to_string() + &"x".repeat(8000);
        m.push(true, "127.0.0.1".into(), &long, &"y".repeat(5000), REFUSED, Some("z".repeat(900)));
        let e = &m.list(None)[0];
        assert!(e.address.chars().count() <= MAX_TEXT + 1 && e.args.chars().count() <= MAX_TEXT + 1);
        assert!(e.detail.as_ref().unwrap().chars().count() <= MAX_TEXT + 1);
        m.clear();
        assert!(m.is_empty());
    }

    #[test]
    fn results_and_args_read_well() {
        assert_eq!(args_text(&[OscArg::Float(0.25), OscArg::Int(3), OscArg::Str("a".into()), OscArg::Bool(true)]), "0.25 3 \"a\" true");
        let mut m = Monitor::default();
        for r in [APPLIED, REFUSED, UNKNOWN] {
            m.push(true, "127.0.0.1".into(), "/ls/x", "", r, None);
        }
        let results: Vec<&str> = m.list(Some(true)).iter().map(|e| e.result).collect();
        assert_eq!(results, [UNKNOWN, REFUSED, APPLIED]);
        let j = serde_json::to_value(&m.list(None)[0]).unwrap();
        assert_eq!((j["result"].as_str(), j["incoming"].as_bool()), (Some("inconnu"), Some(true)));
    }
}
