//! T-461: the single table of the page's texts (`src/labels.js`).
//!
//! Static checks, no server, no browser:
//! - the table: keys well formed and unique, no empty value;
//! - keys used (markup `data-t*`, script `t('…')`) = keys defined;
//! - the `{parameters}` of a value = the names passed by its `t()` calls;
//! - the safety texts exist and are not empty;
//! - no visible text written in the markup between `<!-- labels:begin`
//!   and `<!-- labels:end -->` (the regions migrated so far; the rest of
//!   the page moves there region by region), except the short, commented
//!   exception list below and the safety fallbacks (`data-t-fallback`),
//!   whose text must equal the table's;
//! - no French text in the ES modules (`beam3d.js`), outside `t()`.
//!
//! Negative cases (`*_finds_*`) prove each scanner fails on a hard-coded
//! text.

use std::collections::{BTreeMap, BTreeSet};

const PAGE: &str = include_str!("../src/index.html");
const LABELS_JS: &str = include_str!("../src/labels.js");
/// The page's ES modules (T-435 adds more): no French text outside t().
const MODULES: &[(&str, &str)] = &[("beam3d.js", include_str!("../src/beam3d.js"))];

/// Safety texts: never missing, never empty (an empty e-stop banner or
/// arm button must not be possible).
const SAFETY_KEYS: &[&str] = &[
    "securite.estop",
    "securite.estop.banniere",
    "securite.estop.reinitialiser",
    "securite.arret",
    "securite.noir",
    "securite.ciel",
    "securite.laser.off",
    "securite.laser.on",
    "securite.laser.arme",
    "securite.laser.arret_materiel",
    "securite.laser.liste",
    "securite.desarme",
    "securite.desarme.raison",
    "securite.arme.depuis",
    "securite.maintien.actif",
    "securite.maintien.demande",
    "securite.public",
];

/// Visible texts allowed in the markup as they are: units, written after
/// a number or alone. Anything without a letter (symbols ▶ ■ ✕ ▾ ÷2, `…`)
/// and the content of `<kbd>` (key names) are allowed too, as are
/// attribute values that are technical ids (`tempo.bpm`: a control id).
const ALLOWED: &[&str] = &["BPM", "ms", "s", "Hz", "%"];

fn is_key(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

/// `'…'` or `"…"` at the start of `s`: (content unescaped, bytes used).
fn js_string(s: &str) -> Option<(String, usize)> {
    let q = s.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let mut out = String::new();
    let mut it = s.char_indices().skip(1);
    while let Some((i, c)) = it.next() {
        if c == '\\' {
            out.push(it.next()?.1);
        } else if c == q {
            return Some((out, i + 1));
        } else {
            out.push(c);
        }
    }
    None
}

/// The table, in file order: `'key': 'value',` lines between
/// `const LABELS = Object.freeze({` and `});`.
fn table(src: &str) -> Result<Vec<(String, String)>, String> {
    let start = src.find("const LABELS = Object.freeze({").ok_or("no LABELS")?;
    let body = &src[start..];
    let end = body.find("\n});").ok_or("LABELS not closed")?;
    let mut out = Vec::new();
    for line in body[..end].lines().skip(1) {
        let l = line.trim();
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        let (key, n) = js_string(l).ok_or_else(|| format!("not a 'key': 'value' line: {l}"))?;
        let rest = l[n..].strip_prefix(": ").ok_or_else(|| format!("no ': ' after the key: {l}"))?;
        let (value, m) = js_string(rest).ok_or_else(|| format!("value not a plain string: {l}"))?;
        if rest[m..].trim() != "," {
            return Err(format!("one entry per line, ending with a comma: {l}"));
        }
        out.push((key, value));
    }
    Ok(out)
}

/// `{name}` placeholders of a value.
fn placeholders(v: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = v;
    while let Some(i) = rest.find('{') {
        rest = &rest[i + 1..];
        if let Some(j) = rest.find('}') {
            let name = &rest[..j];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Every `data-t*="key"` of the markup. A value without a dot is not a
/// label key: the audio triggers' rows (T-240) name their fields with
/// `data-t="event"` and the like, and applyLabels() skips them too.
fn markup_keys(page: &str) -> Vec<String> {
    let mut out = Vec::new();
    for attr in ["data-t=\"", "data-t-title=\"", "data-t-placeholder=\"", "data-t-aria-label=\""] {
        for (i, _) in page.match_indices(attr) {
            let v = &page[i + attr.len()..];
            let v = &v[..v.find('"').unwrap()];
            if v.contains('.') {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// One `t(` call of a script: the keys its first argument names (one
/// literal, or several in a ternary) and the names of its `{ … }`
/// parameters, if any.
#[derive(Debug)]
struct Call {
    keys: Vec<String>,
    single: bool,
    params: Option<BTreeSet<String>>,
}

/// The text from `s` (just after an opening bracket) to its closing one.
fn balanced(s: &str, open: char, close: char) -> &str {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) => {
                if c == q && prev != '\\' {
                    quote = None;
                }
            }
            None if c == '\'' || c == '"' || c == '`' => quote = Some(c),
            None if c == open => depth += 1,
            None if c == close => {
                if depth == 0 {
                    return &s[..i];
                }
                depth -= 1;
            }
            None => {}
        }
        prev = if prev == '\\' && c == '\\' { ' ' } else { c };
    }
    s
}

/// The `t(…)` calls of a script (`t(` not preceded by a name character).
fn calls(src: &str) -> Vec<Call> {
    let mut out = Vec::new();
    for (i, _) in src.match_indices("t(") {
        let before = src[..i].chars().next_back().unwrap_or(' ');
        if before.is_alphanumeric() || before == '_' || before == '$' || before == '.' {
            continue;
        }
        let args = balanced(&src[i + 2..], '(', ')');
        // The first argument ends at the first top-level comma.
        let mut first_end = args.len();
        let mut depth = 0;
        let mut quote: Option<char> = None;
        for (j, c) in args.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == '\'' || c == '"' || c == '`' => quote = Some(c),
                None if "([{".contains(c) => depth += 1,
                None if ")]}".contains(c) => depth -= 1,
                None if c == ',' && depth == 0 => {
                    first_end = j;
                    break;
                }
                None => {}
            }
        }
        let first = args[..first_end].trim();
        let mut keys = Vec::new();
        let mut k = 0;
        while k < first.len() {
            if let Some((s, n)) = js_string(&first[k..]) {
                if is_key(&s) {
                    keys.push(s);
                }
                k += n;
            } else {
                k += first[k..].chars().next().unwrap().len_utf8();
            }
        }
        if keys.is_empty() {
            continue; // not our t() (a local function or variable)
        }
        let single = js_string(first).is_some_and(|(_, n)| n == first.len());
        let params = args.get(first_end + 1..).map(str::trim).filter(|p| p.starts_with('{')).map(|p| {
            let inner = balanced(&p[1..], '{', '}');
            let mut names = BTreeSet::new();
            let mut depth = 0;
            let mut tok = String::new();
            for c in inner.chars().chain(std::iter::once(',')) {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        let name = tok.split(':').next().unwrap().trim().to_string();
                        if !name.is_empty() {
                            names.insert(name);
                        }
                        tok.clear();
                        continue;
                    }
                    _ => {}
                }
                tok.push(c);
            }
            names
        });
        out.push(Call { keys, single, params });
    }
    out
}

/// The page's scripts (inline `<script>` blocks), joined.
fn page_scripts(page: &str) -> String {
    let mut out = String::new();
    let mut rest = page;
    while let Some(i) = rest.find("<script>") {
        let body = &rest[i + 8..];
        let end = body.find("</script>").unwrap();
        out.push_str(&body[..end]);
        out.push('\n');
        rest = &body[end..];
    }
    out
}

fn text_allowed(text: &str) -> bool {
    if !text.chars().any(char::is_alphabetic) || ALLOWED.contains(&text) {
        return true;
    }
    // A number with its unit: « 0 ms », « 100 % ».
    let mut words = text.split(' ');
    match (words.next(), words.next(), words.next()) {
        (Some(n), Some(u), None) => n.parse::<f64>().is_ok() && ALLOWED.contains(&u),
        _ => false,
    }
}

const VOID: &[&str] = &["input", "br", "img", "meta", "link", "hr", "source", "wbr", "col", "area", "base", "track"];

/// Hard-coded visible texts of a markup region (see the module head):
/// one message per finding. `labels` checks the fallbacks' text.
fn hardcoded(region: &str, labels: &BTreeMap<String, String>) -> Vec<String> {
    struct Open {
        tag: String,
        data_t: Option<String>,
        fallback: bool,
        text: String,
    }
    let mut found = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut rest = region;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->").map_or(r.len(), |i| i + 3)..];
            continue;
        }
        if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').unwrap_or(r.len());
            let tag = r[..end].trim().to_ascii_lowercase();
            if let Some(pos) = stack.iter().rposition(|o| o.tag == tag) {
                let o = stack.remove(pos);
                stack.truncate(pos);
                if o.fallback {
                    let want = o.data_t.as_ref().and_then(|k| labels.get(k));
                    let got = o.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if want.map(|w| w.split_whitespace().collect::<Vec<_>>().join(" ")) != Some(got.clone()) {
                        found.push(format!("fallback text « {got} » differs from the table ({:?})", o.data_t));
                    }
                }
            }
            rest = &r[(end + 1).min(r.len())..];
            continue;
        }
        if rest.starts_with('<') && rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            let tag_end = balanced_tag(rest);
            let inner = &rest[1..tag_end];
            let name_end = inner.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(inner.len());
            let tag = inner[..name_end].to_ascii_lowercase();
            let attrs = attributes(&inner[name_end..]);
            if let Some(top) = stack.last() {
                if top.data_t.is_some() {
                    found.push(format!("<{tag}> inside a data-t element (applyLabels replaces its content)"));
                }
            }
            for (name, value) in &attrs {
                if ["title", "placeholder", "aria-label", "alt"].contains(&name.as_str())
                    && !(is_key(value) || !value.chars().any(char::is_alphabetic))
                {
                    found.push(format!("<{tag} {name}=\"{value}\">: use data-t-{name}"));
                }
            }
            let self_closing = inner.trim_end().ends_with('/');
            rest = &rest[tag_end + 1..];
            if tag == "script" || tag == "style" {
                let close = format!("</{tag}>");
                rest = &rest[rest.find(&close).map_or(rest.len(), |i| i + close.len())..];
                continue;
            }
            if !self_closing && !VOID.contains(&tag.as_str()) {
                let get = |n: &str| attrs.iter().find(|(a, _)| a == n).map(|(_, v)| v.clone());
                stack.push(Open {
                    tag,
                    data_t: get("data-t"),
                    fallback: get("data-t-fallback").is_some(),
                    text: String::new(),
                });
            }
            continue;
        }
        let first = rest.chars().next().unwrap().len_utf8();
        let end = rest[first..].find('<').map_or(rest.len(), |i| i + first);
        let text = rest[..end].trim();
        rest = &rest[end..];
        if text.is_empty() {
            continue;
        }
        if let Some(top) = stack.iter_mut().rev().find(|o| o.fallback) {
            top.text.push_str(text);
            top.text.push(' ');
            continue;
        }
        if stack.iter().any(|o| o.tag == "kbd") || text_allowed(text) {
            continue;
        }
        let tag = stack.last().map_or("?", |o| o.tag.as_str());
        found.push(format!("text « {text} » in <{tag}>: use data-t"));
    }
    found
}

/// The index of the `>` closing the tag that starts `s` (quotes skipped).
fn balanced_tag(s: &str) -> usize {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return i,
            None => {}
        }
    }
    s.len() - 1
}

/// `name="value"` / `name='value'` / `name` attributes of a tag's inside.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_whitespace() || c == '=' || c == '/').unwrap_or(rest.len());
        let name = rest[..end].to_ascii_lowercase();
        rest = &rest[end..];
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            let q = r.chars().next().unwrap_or(' ');
            if q == '"' || q == '\'' {
                let close = r[1..].find(q).map_or(r.len(), |i| i + 1);
                value = r[1..close].to_string();
                rest = &r[(close + 1).min(r.len())..];
            } else {
                let e = r.find(char::is_whitespace).unwrap_or(r.len());
                value = r[..e].to_string();
                rest = &r[e..];
            }
        } else if end == 0 {
            rest = &rest[1..];
        }
        if !name.is_empty() {
            out.push((name, value));
        }
        rest = rest.trim_start();
    }
    out
}

/// The migrated regions of the page.
fn regions(page: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = page;
    while let Some(i) = rest.find("<!-- labels:begin") {
        let body = &rest[i..];
        let end = body.find("<!-- labels:end -->").expect("labels:begin without labels:end");
        out.push(&body[..end]);
        rest = &body[end..];
    }
    out
}

/// French texts in a module's string literals (a literal with an accented
/// letter, or two words with a space), outside comments and `t()` calls.
/// GLSL sources (template literals starting with `#version`) are code;
/// `new Error('…')` messages go to the console, never on screen.
fn module_texts(src: &str) -> Vec<String> {
    let mut found = Vec::new();
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '\'' || c == '"' || c == '`' {
            let start = i;
            i += 1;
            let mut s = String::new();
            while i < b.len() && b[i] != c {
                if b[i] == '\\' {
                    i += 1;
                }
                if i < b.len() {
                    s.push(b[i]);
                }
                i += 1;
            }
            i += 1;
            let in_t = {
                let before: String = b[..start].iter().rev().take(2).collect();
                before == "(t" && !b.get(start.wrapping_sub(3)).is_some_and(|p| p.is_alphanumeric() || *p == '_' || *p == '.')
            };
            let accented = s.chars().any(|ch| ch.is_alphabetic() && !ch.is_ascii());
            let words = s.split(' ').filter(|w| w.chars().any(char::is_alphabetic)).count() >= 2;
            let before: String = b[..start].iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
            if !in_t && before != "Error(" && !s.starts_with("#version") && (accented || words) {
                found.push(s);
            }
            continue;
        }
        i += 1;
    }
    found
}

fn labels() -> BTreeMap<String, String> {
    table(LABELS_JS).unwrap().into_iter().collect()
}

#[test]
fn ui_labels_table_is_well_formed() {
    let entries = table(LABELS_JS).unwrap();
    assert!(entries.len() > 100, "{} entries", entries.len());
    let mut seen = BTreeSet::new();
    for (k, v) in &entries {
        assert!(is_key(k), "key « {k} »: lower case, by domain, dot-separated");
        assert!(seen.insert(k.clone()), "key « {k} » twice");
        assert!(!v.trim().is_empty(), "key « {k} » has an empty value");
        assert_eq!(v.trim(), v, "key « {k} »: no leading or trailing space");
    }
}

#[test]
fn ui_labels_used_keys_are_the_defined_keys() {
    let labels = labels();
    let scripts = page_scripts(PAGE);
    let mut used: BTreeSet<String> = markup_keys(PAGE).into_iter().collect();
    for c in calls(&scripts) {
        used.extend(c.keys);
    }
    for (_, m) in MODULES {
        for c in calls(m) {
            used.extend(c.keys);
        }
    }
    let defined: BTreeSet<String> = labels.keys().cloned().collect();
    let missing: Vec<_> = used.difference(&defined).collect();
    let unused: Vec<_> = defined.difference(&used).collect();
    assert!(missing.is_empty(), "keys used but not in labels.js: {missing:?}");
    assert!(unused.is_empty(), "keys in labels.js never used: {unused:?}");
}

#[test]
fn ui_labels_parameters_match_the_calls() {
    let labels = labels();
    let scripts = page_scripts(PAGE);
    let all: Vec<Call> = calls(&scripts).into_iter().chain(MODULES.iter().flat_map(|(_, m)| calls(m))).collect();
    assert!(all.len() >= 10, "{all:?}");
    for c in &all {
        for k in &c.keys {
            let Some(v) = labels.get(k) else { continue };
            let want = placeholders(v);
            match &c.params {
                Some(p) if c.single => assert_eq!(&want, p, "t('{k}', …): parameters of « {v} »"),
                Some(_) => {}
                None => assert!(want.is_empty(), "t('{k}') without the parameters {want:?} of « {v} »"),
            }
        }
    }
    // Markup keys are filled without parameters.
    for k in markup_keys(PAGE) {
        if let Some(v) = labels.get(&k) {
            assert!(placeholders(v).is_empty(), "data-t « {k} » has parameters: « {v} »");
        }
    }
}

#[test]
fn ui_labels_safety_texts_exist_and_are_not_empty() {
    let labels = labels();
    for k in SAFETY_KEYS {
        assert!(labels.get(*k).is_some_and(|v| !v.trim().is_empty()), "safety text « {k} » missing or empty");
    }
    // The e-stop banner shows even without labels.js: its text stays in the markup.
    let banner = &PAGE[PAGE.find("<div id=\"estopBanner\"").unwrap()..];
    let banner = &banner[..banner.find("</div>").unwrap()];
    assert!(banner.contains("data-t-fallback>ARRÊT D'URGENCE — réinitialiser pour réarmer<"), "{banner}");
    // The page runs if labels.js does not load.
    assert!(PAGE.contains("if (typeof t !== 'function')") && PAGE.contains("if (typeof applyLabels === 'function') applyLabels();"));
    assert!(PAGE.find("<script src=\"/labels.js\"></script>").unwrap() < PAGE.find("const $ = id =>").unwrap());
}

#[test]
fn ui_labels_no_hardcoded_text_in_migrated_markup() {
    let labels = labels();
    let regions = regions(PAGE);
    assert!(!regions.is_empty(), "no <!-- labels:begin --> region");
    assert!(regions.iter().any(|r| r.contains("<header>")), "the top bar is migrated");
    for r in regions {
        let found = hardcoded(r, &labels);
        assert!(found.is_empty(), "hard-coded texts:\n{}", found.join("\n"));
    }
}

#[test]
fn ui_labels_no_french_text_in_modules() {
    for (name, src) in MODULES {
        let found = module_texts(src);
        assert!(found.is_empty(), "{name}: texts to move to labels.js: {found:?}");
    }
}

#[test]
fn ui_labels_markup_scanner_finds_hardcoded_text() {
    let mut labels = BTreeMap::new();
    labels.insert("a.b".to_string(), "Texte".to_string());
    let bad = |html: &str| !hardcoded(html, &labels).is_empty();
    assert!(bad("<div><button id=\"x\">Texte en dur</button></div>"));
    assert!(bad("<button title=\"Une info\" data-t=\"a.b\"></button>"));
    assert!(bad("<input placeholder=\"Chercher…\">"));
    assert!(bad("<span aria-label=\"Fermer\"></span>"));
    assert!(bad("<img alt=\"Logo\">"));
    assert!(bad("<button data-t=\"a.b\"><span>x</span></button>"), "children under data-t");
    assert!(bad("<button data-t=\"a.b\" data-t-fallback>Autre</button>"), "fallback differs");
    assert!(!bad("<div><button data-t=\"a.b\" data-t-title=\"a.b\"></button> <kbd>Échap</kbd> ▶ <span>0 ms</span><span>BPM</span></div>"));
    assert!(!bad("<button data-t=\"a.b\" data-t-fallback>Texte</button><input title=\"tempo.bpm\"><!-- un commentaire -->"));
    assert!(!bad("<style>a::before { content: 'x y' }</style><script>const s = 'Un texte';</script>"));
}

#[test]
fn ui_labels_module_scanner_finds_hardcoded_text() {
    assert_eq!(module_texts("const a = 'Bouton en dur';"), vec!["Bouton en dur"]);
    assert_eq!(module_texts("el.title = `Vitesse : ${v}`;"), vec!["Vitesse : ${v}"]);
    assert_eq!(module_texts("x = \"Arrêt\";"), vec!["Arrêt"]);
    assert!(module_texts("// un commentaire 'en dur'\n/* « bloc » */ const k = 'tris'; t('a.b');").is_empty());
    assert!(module_texts("throw new Error('WebGL2 unavailable');").is_empty());
    assert!(module_texts("const s = `#version 300 es\nin vec3 aPos;`; const q = t('securite.arret');").is_empty());
}

#[test]
fn ui_labels_call_scanner_reads_keys_and_parameters() {
    let c = calls("x = t('a.b', { n, nom: f(1, 2) }); y = t(on ? 'c.d' : 'e.f'); z = at('g.h'); w = o.t('i.j');");
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(c[0].keys, vec!["a.b"]);
    assert!(c[0].single);
    assert_eq!(c[0].params.clone().unwrap().into_iter().collect::<Vec<_>>(), vec!["n", "nom"]);
    assert_eq!(c[1].keys, vec!["c.d", "e.f"]);
    assert!(!c[1].single && c[1].params.is_none());
}
