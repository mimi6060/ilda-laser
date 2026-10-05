//! Timeline markers to and from CSV (T-443): an audio editor's cue points
//! come in without retyping them, ours go back out.
//!
//! The export is plain: `#,Name,Start,Color`, one marker per row, times in
//! seconds with a decimal point. The reader is tolerant, because every
//! audio editor writes its own flavour: `,`, `;` or tab as separator, a
//! decimal comma, a BOM, `\r\n`, columns found by their header names in any
//! order (others ignored), regions imported at their start, times as
//! `12.5`, `m:ss.mmm` or `h:mm:ss.mmm`. A file is untrusted input: its size
//! is capped by the caller (`MAX_CSV_BYTES`), its rows here, and a bad row
//! is reported with its line number without stopping the others.
//!
//! Parsing never touches the show: the page applies the result through the
//! editor's own path (`POST /api/timeline/edit`), so one Cmd+Z undoes it.

use crate::timeline::{Marker, MAX_LABEL, MAX_MARKERS};
use anyhow::{bail, Result};
use serde::Serialize;

/// Largest file `parse` is given (the route answers 413 beyond).
pub const MAX_CSV_BYTES: usize = 1 << 20;
/// Rows read at most; the rest of the file is reported as one skipped line.
const MAX_ROWS: usize = 20_000;
/// Skipped rows reported one by one; beyond, only counted.
const MAX_SKIPPED: usize = 200;
/// Latest time accepted, in seconds (a day: anything later is a typo).
const MAX_TIME_S: f64 = 86_400.0;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Skipped {
    pub line: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Parsed {
    pub markers: Vec<Marker>,
    /// The first `MAX_SKIPPED` rows left out, with why.
    pub skipped: Vec<Skipped>,
    /// Every row left out, listed or not.
    pub skipped_count: usize,
}

impl Parsed {
    fn skip(&mut self, line: usize, reason: impl Into<String>) {
        self.skipped_count += 1;
        if self.skipped.len() < MAX_SKIPPED {
            self.skipped.push(Skipped { line, reason: reason.into() });
        }
    }
}

/// The file's text: UTF-8 (BOM dropped), or Latin-1 for an older
/// spreadsheet's export. Empty or binary files are refused in French.
pub fn decode(bytes: &[u8]) -> Result<String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        bail!("fichier vide : aucun marqueur à lire");
    }
    // Text never holds NUL or most control bytes; a binary file does.
    let controls = bytes.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')).count();
    if bytes.contains(&0) || controls * 100 > bytes.len() {
        bail!("ce fichier n'est pas un CSV (fichier binaire) : exportez les marqueurs en texte CSV");
    }
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    })
}

/// One CSV record and the line it starts on (1-based).
struct Record {
    line: usize,
    fields: Vec<String>,
}

/// Splits `text` into records: quoted fields may hold the separator,
/// doubled quotes and line breaks. Stops after `limit` records; the
/// second value is the line where reading stopped, if it did.
fn records(text: &str, sep: char, limit: usize) -> (Vec<Record>, Option<usize>) {
    let mut out = Vec::new();
    let (mut fields, mut field) = (Vec::new(), String::new());
    let (mut line, mut start_line) = (1usize, 1usize);
    let (mut quoted, mut was_quoted) = (false, false);
    let mut chars = text.chars().peekable();
    fn finish(fields: &mut Vec<String>, start_line: usize, out: &mut Vec<Record>) {
        let record = std::mem::take(fields);
        // A blank line is no record.
        if !(record.len() == 1 && record[0].trim().is_empty()) {
            out.push(Record { line: start_line, fields: record });
        }
    }
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                '\r' => {}
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.trim().is_empty() && !was_quoted => {
                field.clear();
                quoted = true;
                was_quoted = true;
            }
            '\r' => {}
            '\n' => {
                fields.push(std::mem::take(&mut field));
                was_quoted = false;
                finish(&mut fields, start_line, &mut out);
                line += 1;
                start_line = line;
                if out.len() >= limit {
                    return (out, chars.peek().is_some().then_some(line));
                }
            }
            c if c == sep => {
                fields.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !fields.is_empty() || was_quoted {
        fields.push(field);
        finish(&mut fields, start_line, &mut out);
    }
    (out, None)
}

/// The separator of the first non-blank line: tab, then `;`, then `,`.
fn detect_separator(text: &str) -> char {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let (mut tabs, mut semis, mut commas, mut quoted) = (0, 0, 0, false);
    for c in first.chars() {
        match c {
            '"' => quoted = !quoted,
            '\t' if !quoted => tabs += 1,
            ';' if !quoted => semis += 1,
            ',' if !quoted => commas += 1,
            _ => {}
        }
    }
    if tabs > 0 {
        '\t'
    } else if semis > 0 && semis >= commas {
        ';'
    } else {
        ','
    }
}

/// Lower case, accents dropped, `#`/spaces kept: a header cell to compare.
fn header_key(cell: &str) -> String {
    cell.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' => 'a',
            'ô' => 'o',
            'û' | 'ù' => 'u',
            'î' | 'ï' => 'i',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

/// Where each column is: `(time, name, color)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Columns {
    time: Option<usize>,
    name: Option<usize>,
    color: Option<usize>,
}

/// The header row's columns, if it names a time or a name column.
fn header_columns(fields: &[String]) -> Option<Columns> {
    let mut cols = Columns { time: None, name: None, color: None };
    for (i, cell) in fields.iter().enumerate() {
        let key = header_key(cell);
        let slot = match key.as_str() {
            "start" | "position" | "time" | "temps" | "debut" => &mut cols.time,
            "name" | "nom" | "label" => &mut cols.name,
            "color" | "colour" | "couleur" => &mut cols.color,
            _ => continue,
        };
        slot.get_or_insert(i);
    }
    (cols.time.is_some() || cols.name.is_some()).then_some(cols)
}

/// Digits with at most one decimal point (after `,` → `.` when allowed).
fn decimal(s: &str, comma_decimal: bool) -> Option<f64> {
    let s = if comma_decimal { s.replace(',', ".") } else { s.to_string() };
    let ok = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.matches('.').count() <= 1 && s != ".";
    ok.then(|| s.parse::<f64>().ok()).flatten()
}

/// A time cell in seconds, or why it can't be read.
fn parse_time(cell: &str, comma_decimal: bool) -> Result<f64, &'static str> {
    let s = cell.trim();
    if s.is_empty() {
        return Err("temps manquant");
    }
    if s.starts_with('-') {
        return Err("temps négatif");
    }
    // `3.1.00`: bars.beats.ticks, which needs the audio editor's tempo.
    if s.matches('.').count() >= 2 && s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("temps en mesures : exporter en secondes");
    }
    let seconds = if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() > 3 {
            return Err("temps illisible");
        }
        let (last, whole) = parts.split_last().ok_or("temps illisible")?;
        let mut total = 0.0;
        for p in whole {
            if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) || p.len() > 6 {
                return Err("temps illisible");
            }
            total = total * 60.0 + p.parse::<f64>().map_err(|_| "temps illisible")?;
        }
        let secs = decimal(last, true).ok_or("temps illisible")?;
        if secs >= 60.0 {
            return Err("temps illisible");
        }
        total * 60.0 + secs
    } else {
        decimal(s, comma_decimal).ok_or("temps illisible")?
    };
    if !seconds.is_finite() {
        return Err("temps illisible");
    }
    if seconds > MAX_TIME_S {
        return Err("temps hors limites (24 h au plus)");
    }
    Ok(seconds)
}

/// `RRGGBB` or `#RRGGBB`; anything else is the default marker colour.
fn parse_color(cell: &str) -> [u8; 3] {
    let s = cell.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return [0, 0, 0];
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
    [byte(0), byte(2), byte(4)]
}

/// A cell that a spreadsheet would run as a formula (`=`, `+`, `-`, `@`).
fn formula_like(s: &str) -> bool {
    s.starts_with(['=', '+', '-', '@'])
}

/// A name as stored: one line, no control characters, `MAX_LABEL` long,
/// and without the `'` our export puts before formula-like names.
fn clean_name(cell: &str) -> String {
    let s = cell.trim();
    let s = match s.strip_prefix('\'') {
        Some(rest) if formula_like(rest) => rest,
        _ => s,
    };
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one_line.trim().chars().take(MAX_LABEL).collect()
}

/// Reads markers from a CSV file's text (see the module doc). Times in the
/// file are seconds; `to_units` converts them to the show's unit. Never
/// more than `MAX_MARKERS` markers, sorted by time.
pub fn parse(text: &str, to_units: impl Fn(f64) -> f64) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let sep = detect_separator(text);
    let comma_decimal = sep != ',';
    let (rows, stopped) = records(text, sep, MAX_ROWS);
    let mut out = Parsed::default();
    let mut rows = rows.into_iter().peekable();
    let header = rows.peek().and_then(|r| header_columns(&r.fields));
    let cols = match header {
        Some(cols) => {
            let line = rows.next().map_or(1, |r| r.line);
            if cols.time.is_none() {
                out.skip(line, "colonne de temps introuvable (Start, Position, Time, Temps ou Début)");
                return out;
            }
            cols
        }
        None => Columns { time: Some(0), name: Some(1), color: Some(2) },
    };
    for row in rows {
        let cell = |i: Option<usize>| i.and_then(|i| row.fields.get(i)).map_or("", |s| s.as_str());
        let mut name_col = cols.name;
        let mut color_col = cols.color;
        // Without a header, a second number is a region's end (`start, end,
        // label`, as Audacity writes labels): the name comes after it.
        if header.is_none() && row.fields.len() >= 3 && parse_time(cell(Some(1)), comma_decimal).is_ok() {
            name_col = Some(2);
            color_col = Some(3);
        }
        let seconds = match parse_time(cell(cols.time), comma_decimal) {
            Ok(s) => s,
            Err(reason) => {
                out.skip(row.line, reason);
                continue;
            }
        };
        if out.markers.len() >= MAX_MARKERS {
            out.skip(row.line, format!("plus de {MAX_MARKERS} marqueurs"));
            continue;
        }
        let at = to_units(seconds);
        if !at.is_finite() || at < 0.0 {
            out.skip(row.line, "temps hors du show");
            continue;
        }
        out.markers.push(Marker { at, name: clean_name(cell(name_col)), color: parse_color(cell(color_col)) });
    }
    if let Some(line) = stopped {
        out.skip(line, format!("fichier trop long : lu jusqu'à {MAX_ROWS} lignes"));
    }
    out.markers.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

/// A CSV field, quoted when it holds `,`, `"` or a line break.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// `#,Name,Start,Color`, one row per marker in time order (`M1`, `M2`…);
/// `to_seconds` converts the show's unit to seconds. A name a spreadsheet
/// would run as a formula gets a leading `'` (dropped again by `parse`).
pub fn export(markers: &[Marker], to_seconds: impl Fn(f64) -> f64) -> String {
    let mut sorted: Vec<&Marker> = markers.iter().collect();
    sorted.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut out = String::from("#,Name,Start,Color\r\n");
    for (i, m) in sorted.iter().enumerate() {
        let name = if formula_like(&m.name) { format!("'{}", m.name) } else { m.name.clone() };
        let seconds = to_seconds(m.at);
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        let color = if m.color == [0, 0, 0] { String::new() } else { format!("{:02X}{:02X}{:02X}", m.color[0], m.color[1], m.color[2]) };
        out.push_str(&format!("M{},{},{:.3},{}\r\n", i + 1, field(&name), seconds, color));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Show, TempoPoint, TimeBase};

    fn m(at: f64, name: &str, color: [u8; 3]) -> Marker {
        Marker { at, name: name.into(), color }
    }

    fn same(s: f64) -> f64 {
        s
    }

    #[test]
    fn export_then_parse_gives_the_same_markers() {
        let markers = vec![
            m(12.5, "Refrain, « fort »", [255, 0, 16]),
            m(0.0, "Intro \"calme\"", [0, 0, 0]),
            m(61.2345, "=SUM(A1)", [1, 2, 3]),
            m(90.0, "Ligne\ncoupée", [0, 128, 255]),
        ];
        let csv = export(&markers, same);
        assert!(csv.starts_with("#,Name,Start,Color\r\nM1,\"Intro \"\"calme\"\"\",0.000,\r\n"), "{csv}");
        assert!(csv.contains("M2,\"Refrain, « fort »\",12.500,FF0010\r\n"));
        assert!(csv.contains("M3,'=SUM(A1),61.234,010203") || csv.contains("M3,'=SUM(A1),61.235,010203"), "{csv}");
        let back = parse(&csv, same);
        assert!(back.skipped.is_empty(), "{:?}", back.skipped);
        assert_eq!(back.markers.len(), 4);
        let mut want = markers.clone();
        want.sort_by(|a, b| a.at.total_cmp(&b.at));
        for (a, b) in back.markers.iter().zip(&want) {
            assert!((a.at - b.at).abs() < 0.001, "{} vs {}", a.at, b.at);
            assert_eq!(a.color, b.color);
        }
        assert_eq!(back.markers[0].name, "Intro \"calme\"");
        assert_eq!(back.markers[1].name, "Refrain, « fort »");
        assert_eq!(back.markers[2].name, "=SUM(A1)", "the formula guard is dropped on import");
        assert_eq!(back.markers[3].name, "Ligne coupée", "one line");
    }

    #[test]
    fn separators_decimal_comma_bom_and_crlf() {
        let csv = "\u{feff}Nom;Temps;Couleur\r\nCouplet;12,5;#00FF00\r\nDrop;1:02,250;\r\n";
        let p = parse(csv, same);
        assert!(p.skipped.is_empty(), "{:?}", p.skipped);
        assert_eq!(p.markers, vec![m(12.5, "Couplet", [0, 255, 0]), m(62.25, "Drop", [0, 0, 0])]);
        let tabs = parse("Time\tLabel\n3.5\tA\n4\tB\n", same);
        assert_eq!(tabs.markers.iter().map(|m| m.at).collect::<Vec<_>>(), vec![3.5, 4.0]);
        // With a comma separator a decimal comma can't be told apart.
        let commas = parse("Name,Start\nA,12.5\n", same);
        assert_eq!(commas.markers[0].at, 12.5);
        // Bytes: BOM and Latin-1 decode.
        assert_eq!(decode(b"\xEF\xBB\xBFName,Start\n").unwrap(), "Name,Start\n");
        assert_eq!(decode(b"Nom;Temps\nD\xE9but;1\n").unwrap(), "Nom;Temps\nDébut;1\n");
    }

    #[test]
    fn time_formats() {
        assert_eq!(parse_time("12.5", false), Ok(12.5));
        assert_eq!(parse_time("12,5", true), Ok(12.5));
        assert_eq!(parse_time("1:02.5", false), Ok(62.5));
        assert_eq!(parse_time("1:00:01.250", false), Ok(3601.25));
        assert_eq!(parse_time("3.1.00", false), Err("temps en mesures : exporter en secondes"));
        assert_eq!(parse_time("-1", false), Err("temps négatif"));
        for bad in ["NaN", "inf", "1e3", "abc", "1:75", ":5", "1:2:3:4", "", "."] {
            assert!(parse_time(bad, false).is_err(), "{bad}");
        }
        assert_eq!(parse_time("90000", false), Err("temps hors limites (24 h au plus)"));
    }

    #[test]
    fn columns_in_any_order_regions_and_extra_columns() {
        // A DAW's marker list: regions with an end, a length, a bar
        // column and an unreadable colour (our own sample, written by hand).
        let csv = "#,Name,Start,End,Length,Color\nM1,Intro,0.000,,,\nR1,Couplet 1,8.000,24.000,16.000,FF8800\nM2,Drop,24.500,,,zz\n";
        let p = parse(csv, same);
        assert!(p.skipped.is_empty(), "{:?}", p.skipped);
        assert_eq!(p.markers, vec![m(0.0, "Intro", [0, 0, 0]), m(8.0, "Couplet 1", [255, 136, 0]), m(24.5, "Drop", [0, 0, 0])]);
        let shuffled = parse("COLOUR,label,Position\n00ff00,B,2\n,A,1\n", same);
        assert_eq!(shuffled.markers, vec![m(1.0, "A", [0, 0, 0]), m(2.0, "B", [0, 255, 0])]);
        // No header: time, name, colour; or Audacity's start, end, label.
        let bare = parse("1.5,Intro,FF0000\n", same);
        assert_eq!(bare.markers, vec![m(1.5, "Intro", [255, 0, 0])]);
        let labels = parse("1.000000\t4.000000\tCouplet\n6.5\t6.5\tDrop\n", same);
        assert_eq!(labels.markers, vec![m(1.0, "Couplet", [0, 0, 0]), m(6.5, "Drop", [0, 0, 0])]);
        let no_time = parse("Name,Comment\nA,b\n", same);
        assert!(no_time.markers.is_empty());
        assert_eq!(no_time.skipped[0].line, 1);
    }

    #[test]
    fn quotes_keep_separators_line_breaks_and_line_numbers() {
        let csv = "Name,Start\n\"a, \"\"b\"\"\",1\n\"deux\nlignes\",2\nmauvais,x\n";
        let p = parse(csv, same);
        assert_eq!(p.markers[0].name, "a, \"b\"");
        assert_eq!(p.markers[1].name, "deux lignes");
        assert_eq!(p.skipped, vec![Skipped { line: 5, reason: "temps illisible".into() }]);
    }

    #[test]
    fn bad_rows_are_reported_and_the_others_kept() {
        let csv = "Name,Start\nA,1\nB,abc\nC,-2\n\nD,3.1.00\nE,4\n";
        let p = parse(csv, same);
        assert_eq!(p.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["A", "E"]);
        let lines: Vec<usize> = p.skipped.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![3, 4, 6]);
        assert_eq!(p.skipped[2].reason, "temps en mesures : exporter en secondes");
        assert_eq!(p.skipped_count, 3);
    }

    #[test]
    fn caps_on_markers_names_rows_and_reports() {
        let mut csv = String::from("Name,Start\n");
        for i in 0..MAX_MARKERS + 10 {
            csv.push_str(&format!("{},{i}\n", "é".repeat(100)));
        }
        let p = parse(&csv, same);
        assert_eq!(p.markers.len(), MAX_MARKERS);
        assert!(p.markers.iter().all(|m| m.name.chars().count() == MAX_LABEL));
        assert_eq!(p.skipped.len(), 10);
        assert_eq!(p.skipped[0].reason, format!("plus de {MAX_MARKERS} marqueurs"));
        let junk = "x,y\n".repeat(MAX_ROWS + 5);
        let p = parse(&junk, same);
        assert_eq!(p.skipped.len(), MAX_SKIPPED);
        assert_eq!(p.skipped_count, MAX_ROWS + 1, "rows read, plus one for the rest of the file");
    }

    #[test]
    fn empty_and_binary_files_are_refused_in_french() {
        assert!(decode(b"").unwrap_err().to_string().contains("vide"));
        assert!(decode(b"\xEF\xBB\xBF \r\n").unwrap_err().to_string().contains("vide"));
        assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap_err().to_string().contains("binaire"));
        assert!(parse("", same).markers.is_empty());
    }

    #[test]
    fn a_beats_show_converts_through_its_tempo_map() {
        let show = Show {
            time_base: TimeBase::Beats,
            tempo_map: vec![TempoPoint { at_s: 0.0, bpm: 120.0, ..Default::default() }, TempoPoint { at_s: 10.0, bpm: 60.0, ..Default::default() }],
            ..Default::default()
        };
        // 10 s at 120 BPM = 20 beats, then 1 beat per second.
        let p = parse("Name,Start\nA,5\nB,14\n", |s| show.beat_at_s(s));
        assert_eq!(p.markers.iter().map(|m| m.at).collect::<Vec<_>>(), vec![10.0, 24.0]);
        let csv = export(&p.markers, |b| show.s_at_beat(b));
        assert!(csv.contains("M1,A,5.000,") && csv.contains("M2,B,14.000,"), "{csv}");
    }
}
