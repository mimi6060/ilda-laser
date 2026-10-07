//! The command line (T-424): one short line drives a control, a cue, the
//! page, the tempo or the stops. The same grammar will serve the cue
//! list steps (T-410), macros (T-425) and OSC `/ls/cmd` (T-416), so it is
//! kept small: no variables, no loops, no conditions, one line = one
//! command.
//!
//! Grammar (keywords ignore case and accents, `,` or `.` as decimal mark):
//! - `<id> <value>`: a continuous control, in per cent for a `Percent`
//!   control (`master.size 80` = 0.8), else in its own unit; clamped to
//!   the registry's range.
//! - `<id> on|off`: a toggle or a momentary control; `<id>` alone: a
//!   trigger; `<id> <n>` (1-based) or `<id> "<label>"`: a choice.
//! - `cue <page>.<cell> go|stop` (1-based, cell = row x columns + column,
//!   the order of `grid.<page>.<row>.<col>`), `cue "<name>" go|stop`.
//! - `page <n>`, `tempo <bpm>`, `tap`, `tout arrêter` (or `stop`), `noir`
//!   (= `transport.blackout`, the latching stop).
//! - `macro "<nom>" go|stop` (T-425): a macro by its name (the label of
//!   its `macro.<id>.go` control).
//!
//! Safety: a line never arms the laser. Two barriers: the parser refuses
//! the arming and reset words, `transport.arm`, every `safety.*` id but
//! `safety.estop` and every control that isn't `external`, before anything
//! runs; and `execute` goes through `controls::apply_from`, which checks
//! the registry's `external` flag again. Stopping is always allowed.
//! Nothing is stored server side (the history is the browser's).

use crate::cues::ClickMode;
use crate::controls::{self, ControlDesc, ControlInput, ControlKind, ControlRegistry, ControlSource, Unit};
use crate::presets::Preset;
use crate::Shared;
use serde::Serialize;

/// The longest line accepted, in characters.
pub const MAX_LINE: usize = 200;

/// Words refused anywhere in a line (and in each part of a dotted id),
/// in any case and with or without accents.
pub const REFUSED_WORDS: &[&str] = &["armer", "réarmer", "allumer", "arm", "réinitialiser"];

const ARM_FR: &str = "Commande refusée : le laser ne s'arme qu'à l'écran ou avec Espace";
const RESET_FR: &str = "Commande refusée : l'arrêt d'urgence ne se réinitialise qu'à l'écran";
const SAFETY_FR: &str = "Commande refusée : les réglages de sécurité ne se changent qu'à l'écran";

#[derive(Clone, Debug, PartialEq)]
pub enum CueTarget {
    /// 1-based page and cell.
    Slot { page: usize, index: usize },
    Name(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CueAction {
    Go,
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Set { id: String, input: ControlInput },
    Cue { target: CueTarget, action: CueAction },
    Page(usize),
    Tempo(f64),
    StopAll,
    /// A macro (T-425) by id: launched (`Go`) or stopped.
    Macro { id: String, action: CueAction },
}

/// What went wrong, naming the word (`token`) and where it starts in the
/// line (`pos`, in characters from 0). `refused`: a safety refusal, not
/// a typo.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandError {
    pub token: String,
    pub pos: usize,
    pub message_fr: String,
    pub refused: bool,
}

impl CommandError {
    fn new(tok: &Tok, message_fr: String) -> Self {
        Self { token: tok.text.clone(), pos: tok.pos, message_fr, refused: false }
    }

    fn refused(tok: &Tok, message_fr: &str) -> Self {
        Self { token: tok.text.clone(), pos: tok.pos, message_fr: message_fr.to_string(), refused: true }
    }

    fn at(token: &str, pos: usize, message_fr: String) -> Self {
        Self { token: token.to_string(), pos, message_fr, refused: false }
    }
}

/// One entry of the palette: a control or a cue, and the line it starts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Action {
    pub label_fr: String,
    /// The control id; none for a cue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The line to run, or its start when `needs_value`.
    pub command: String,
    pub needs_value: bool,
    /// What a value looks like: « 0 à 200 % », « on / off », the choices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint_fr: Option<String>,
}

/// Lower case without accents: what keywords, ids and names are compared on.
pub fn fold(s: &str) -> String {
    folded(s).collect()
}

/// `fold`'s characters one by one, without building the string: a name
/// lookup compares each candidate as it goes (T-486).
fn folded(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' => 'y',
            'ñ' => 'n',
            '’' => '\'',
            c => c,
        })
        .flat_map(|c| {
            let (a, b) = match c {
                'œ' => ('o', Some('e')),
                'æ' => ('a', Some('e')),
                c => (c, None),
            };
            std::iter::once(a).chain(b)
        })
}

#[derive(Clone, Debug)]
struct Tok {
    text: String,
    pos: usize,
    quoted: bool,
}

/// Words split on spaces; `"…"` and `«…»` keep their spaces.
fn tokenize(line: &str) -> Result<Vec<Tok>, CommandError> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let close = match chars[i] {
            '"' => Some('"'),
            '«' => Some('»'),
            '“' => Some('”'),
            _ => None,
        };
        if let Some(close) = close {
            let start = i;
            let end = chars[i + 1..].iter().position(|&c| c == close).map(|n| i + 1 + n);
            let Some(end) = end else {
                let text: String = chars[start..].iter().collect();
                return Err(CommandError::at(&text, start, format!("Guillemet non fermé : « {} »", text.trim())));
            };
            let text: String = chars[i + 1..end].iter().collect();
            toks.push(Tok { text: text.trim().to_string(), pos: start, quoted: true });
            i = end + 1;
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            toks.push(Tok { text: chars[start..i].iter().collect(), pos: start, quoted: false });
        }
    }
    Ok(toks)
}

/// The first word that arms or resets the stop, in a typed (unquoted) word.
fn refused_word(tok: &Tok) -> Option<CommandError> {
    if tok.quoted {
        return None;
    }
    let f = fold(&tok.text);
    let parts: Vec<&str> = f.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
    if parts.contains(&"reinitialiser") || (f.contains("estop") && f.contains("reset")) {
        return Some(CommandError::refused(tok, RESET_FR));
    }
    // Folded once per process, not once per word checked (T-486).
    static REFUSED: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let refused = REFUSED.get_or_init(|| REFUSED_WORDS.iter().map(|w| fold(w)).collect());
    if parts.iter().any(|p| refused.iter().any(|w| w == p)) {
        return Some(CommandError::refused(tok, ARM_FR));
    }
    None
}

/// Why `desc` may not be driven from a line, if it may not.
fn refused_control(tok: &Tok, desc: &ControlDesc) -> Option<CommandError> {
    if desc.id == "transport.arm" {
        return Some(CommandError::refused(tok, ARM_FR));
    }
    if desc.group == "safety" && desc.id != "safety.estop" {
        return Some(CommandError::refused(tok, SAFETY_FR));
    }
    if !desc.external {
        return Some(CommandError::refused(tok, &format!("Commande refusée : « {} » ne se règle qu'à l'écran", desc.id)));
    }
    None
}

/// A number with `.` or `,` as decimal mark and an optional `%`.
fn number(tok: &Tok) -> Option<f64> {
    if tok.quoted {
        return None;
    }
    let t = tok.text.trim_end_matches('%').replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn integer(tok: &Tok) -> Option<usize> {
    if tok.quoted {
        return None;
    }
    tok.text.parse().ok()
}

fn value_expected(after: &Tok, what: &str) -> CommandError {
    let end = after.pos + after.text.chars().count();
    CommandError::at("", end, format!("Valeur attendue après « {} »{what}", after.text))
}

fn extra(tok: &Tok) -> CommandError {
    CommandError::new(tok, format!("Mot en trop : « {} »", tok.text))
}

/// No word left after `toks[used..]`.
fn done(toks: &[Tok], used: usize) -> Result<(), CommandError> {
    toks.get(used).map_or(Ok(()), |t| Err(extra(t)))
}

/// Reads one line. Nothing runs here: the result goes to `execute`.
pub fn parse(line: &str, reg: &ControlRegistry, presets: &[Preset]) -> Result<Command, CommandError> {
    let len = line.chars().count();
    if len > MAX_LINE {
        return Err(CommandError::at("", MAX_LINE, format!("Ligne trop longue : {MAX_LINE} caractères au plus")));
    }
    let toks = tokenize(line)?;
    // The refusals first, whatever the rest of the line says.
    if let Some(e) = toks.iter().find_map(refused_word) {
        return Err(e);
    }
    let Some(first) = toks.first() else {
        return Err(CommandError::at("", 0, "Commande vide".to_string()));
    };
    let word = if first.quoted { String::new() } else { fold(&first.text) };
    match word.as_str() {
        "stop" => done(&toks, 1).map(|_| Command::StopAll),
        "tout" => match toks.get(1) {
            Some(t) if !t.quoted && fold(&t.text) == "arreter" => done(&toks, 2).map(|_| Command::StopAll),
            Some(t) => Err(CommandError::new(t, format!("Mot inconnu : « {} » (« tout arrêter » ?)", t.text))),
            None => Err(value_expected(first, " (« tout arrêter » ?)")),
        },
        "noir" => done(&toks, 1).map(|_| Command::Set { id: "transport.blackout".into(), input: ControlInput::Value(1.0) }),
        "tap" => done(&toks, 1).map(|_| Command::Set { id: "tempo.tap".into(), input: ControlInput::Value(1.0) }),
        "tempo" => {
            let v = toks.get(1).ok_or_else(|| value_expected(first, ""))?;
            let bpm = number(v).ok_or_else(|| CommandError::new(v, format!("Nombre attendu : « {} »", v.text)))?;
            done(&toks, 2)?;
            Ok(Command::Tempo(bpm.clamp(crate::tempo::MIN_BPM, crate::tempo::MAX_BPM)))
        }
        "page" => {
            let v = toks.get(1).ok_or_else(|| value_expected(first, ""))?;
            let n = integer(v).ok_or_else(|| CommandError::new(v, format!("Numéro de page attendu : « {} »", v.text)))?;
            if n == 0 || reg.get(&format!("page.{n}")).is_none() {
                return Err(CommandError::new(v, format!("Page inconnue : « {} »", v.text)));
            }
            done(&toks, 2)?;
            Ok(Command::Page(n))
        }
        "cue" => parse_cue(&toks, presets),
        "macro" => parse_macro(&toks, reg),
        _ => parse_control(&toks, reg),
    }
}

fn parse_cue(toks: &[Tok], presets: &[Preset]) -> Result<Command, CommandError> {
    let first = &toks[0];
    if toks.len() < 3 {
        return Err(match toks.get(1) {
            None => value_expected(first, " (une case 2.14 ou un nom entre guillemets)"),
            Some(t) => value_expected(t, " (go ou stop)"),
        });
    }
    let last = &toks[toks.len() - 1];
    let action = match (last.quoted, fold(&last.text).as_str()) {
        (false, "go") => CueAction::Go,
        (false, "stop") => CueAction::Stop,
        _ => return Err(CommandError::new(last, format!("« go » ou « stop » attendu : « {} »", last.text))),
    };
    let what = &toks[1..toks.len() - 1];
    let slot = (what.len() == 1 && !what[0].quoted)
        .then(|| what[0].text.split_once('.'))
        .flatten()
        .and_then(|(p, c)| Some((p.parse::<usize>().ok()?, c.parse::<usize>().ok()?)));
    let target = match slot {
        Some((page, index)) if page >= 1 && index >= 1 => CueTarget::Slot { page, index },
        Some(_) => return Err(CommandError::new(&what[0], format!("Case inconnue : « {} » (comptées à partir de 1)", what[0].text))),
        None => {
            let name = what.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
            let folded = fold(&name);
            let preset = presets.iter().find(|p| self::folded(&p.name).eq(folded.chars())).ok_or_else(|| CommandError::new(&what[0], format!("Cue inconnu : « {name} »")))?;
            CueTarget::Name(preset.name.clone())
        }
    };
    Ok(Command::Cue { target, action })
}

/// `macro "<nom>" go|stop`: the name is the label of a `macro.<id>.go`
/// control of the registry.
fn parse_macro(toks: &[Tok], reg: &ControlRegistry) -> Result<Command, CommandError> {
    let first = &toks[0];
    if toks.len() < 3 {
        return Err(match toks.get(1) {
            None => value_expected(first, " (un nom de macro entre guillemets)"),
            Some(t) => value_expected(t, " (go ou stop)"),
        });
    }
    let last = &toks[toks.len() - 1];
    let action = match (last.quoted, fold(&last.text).as_str()) {
        (false, "go") => CueAction::Go,
        (false, "stop") => CueAction::Stop,
        _ => return Err(CommandError::new(last, format!("« go » ou « stop » attendu : « {} »", last.text))),
    };
    let what = &toks[1..toks.len() - 1];
    let name = what.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
    let folded = fold(&name);
    let id = reg
        .list()
        .iter()
        .filter(|d| d.group == "macro")
        .find_map(|d| d.id.strip_prefix("macro.").and_then(|r| r.strip_suffix(".go")).filter(|_| fold(&d.label_fr) == folded))
        .ok_or_else(|| CommandError::new(&what[0], format!("Macro inconnue : « {name} »")))?;
    Ok(Command::Macro { id: id.to_string(), action })
}

fn parse_control(toks: &[Tok], reg: &ControlRegistry) -> Result<Command, CommandError> {
    let first = &toks[0];
    let desc = (!first.quoted)
        .then(|| reg.get(&fold(&first.text)))
        .flatten()
        .ok_or_else(|| CommandError::new(first, format!("Mot inconnu : « {} »", first.text)))?;
    if let Some(e) = refused_control(first, desc) {
        return Err(e);
    }
    let id = desc.id.clone();
    let arg = toks.get(1);
    let input = match &desc.kind {
        ControlKind::Trigger => {
            done(toks, 1)?;
            ControlInput::Value(1.0)
        }
        ControlKind::Toggle { .. } | ControlKind::Momentary => {
            let v = arg.ok_or_else(|| value_expected(first, " (on ou off)"))?;
            let on = match (v.quoted, fold(&v.text).as_str()) {
                (false, "on") => true,
                (false, "off") => false,
                _ => return Err(CommandError::new(v, format!("« on » ou « off » attendu : « {} »", v.text))),
            };
            done(toks, 2)?;
            ControlInput::Value(if on { 1.0 } else { 0.0 })
        }
        ControlKind::Continuous { min, max, unit, .. } => {
            let v = arg.ok_or_else(|| value_expected(first, ""))?;
            let n = number(v).ok_or_else(|| CommandError::new(v, format!("Nombre attendu : « {} »", v.text)))?;
            done(toks, 2)?;
            let native = if *unit == Unit::Percent { n / 100.0 } else { n };
            ControlInput::Value((native as f32).clamp(*min, *max))
        }
        ControlKind::Choice { options, .. } => {
            let v = arg.ok_or_else(|| value_expected(first, &format!(" (1 à {})", options.len())))?;
            let index = match integer(v) {
                Some(n) if (1..=options.len()).contains(&n) && toks.len() == 2 => n - 1,
                Some(_) if toks.len() == 2 => return Err(CommandError::new(v, format!("Choix inconnu : « {} » (1 à {})", v.text, options.len()))),
                _ => {
                    let label = toks[1..].iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
                    let folded = fold(&label);
                    options
                        .iter()
                        .position(|o| fold(o) == folded)
                        .ok_or_else(|| CommandError::new(v, format!("Choix inconnu : « {label} »")))?
                }
            };
            ControlInput::Value(index as f32)
        }
    };
    Ok(Command::Set { id, input })
}

/// Runs a parsed line, credited to `source` (« En cours », T-423), checked
/// like a controller (`from_external`: nothing that isn't `external`).
/// Returns the echo shown under the field: « master.size = 80 % ».
pub fn execute(s: &mut Shared, cmd: &Command, source: ControlSource) -> Result<String, CommandError> {
    let apply = |s: &mut Shared, id: &str, input: ControlInput| {
        controls::apply_from(s, id, input, source).map_err(|e| {
            let msg = match e {
                controls::ControlError::Unknown(id) => format!("Mot inconnu : « {id} »"),
                controls::ControlError::Refused(why) => format!("Commande refusée : {why}"),
            };
            CommandError { token: id.to_string(), pos: 0, message_fr: msg, refused: true }
        })
    };
    match cmd {
        Command::Set { id, input } => {
            apply(s, id, *input)?;
            let desc = s.controls.get(id).cloned();
            Ok(match desc {
                Some(d) if matches!(d.id.as_str(), "transport.blackout" | "safety.estop") => "Arrêt d'urgence enclenché (réinitialiser à l'écran)".to_string(),
                Some(d) if d.kind == ControlKind::Trigger => format!("{} : fait", d.label_fr),
                Some(d) => match controls::current(s, &d).and_then(|v| value_fr(&d, &v)) {
                    Some(v) => format!("{} = {v}", d.id),
                    None => format!("{} : fait", d.label_fr),
                },
                None => format!("{id} : fait"),
            })
        }
        Command::Page(n) => {
            apply(s, &format!("page.{n}"), ControlInput::Value(1.0))?;
            let name = s.grid.view().get(n - 1).map(|p| p.name.clone()).unwrap_or_default();
            Ok(format!("Page {n} {name}").trim_end().to_string())
        }
        Command::Tempo(bpm) => {
            apply(s, "tempo.bpm", ControlInput::Value(*bpm as f32))?;
            Ok(format!("Tempo = {} BPM", num(s.tempo.bpm)))
        }
        Command::StopAll => {
            apply(s, "running.stop_all", ControlInput::Value(1.0))?;
            Ok("Tout arrêté".to_string())
        }
        Command::Macro { id, action } => {
            let name = s.macros.get(id).map(|m| m.name.clone()).unwrap_or_else(|| id.clone());
            match action {
                CueAction::Go => {
                    crate::macros::start(s, id, source).map_err(|why| CommandError { token: name.clone(), pos: 6, message_fr: format!("Commande refusée : {why}"), refused: true })?;
                    Ok(format!("macro « {name} » : go"))
                }
                CueAction::Stop => {
                    crate::macros::stop(s, id);
                    Ok(format!("macro « {name} » : stop"))
                }
            }
        }
        Command::Cue { target, action } => {
            let (id, what) = match target {
                CueTarget::Slot { page, index } => {
                    let cols = (s.grid.cols as usize).max(1);
                    let (row, col) = ((index - 1) / cols + 1, (index - 1) % cols + 1);
                    let cell = format!("{page}.{row}.{col}");
                    let id = controls::grid_cell_preset(&s.grid, &cell)
                        .ok_or_else(|| CommandError::at(&format!("{page}.{index}"), 4, format!("Case vide ou absente : « {page}.{index} »")))?;
                    (id, format!("cue {page}.{index}"))
                }
                CueTarget::Name(name) => {
                    let folded = fold(name);
                    let id = s.presets.iter().find(|p| self::folded(&p.name).eq(folded.chars())).map(|p| p.id.clone());
                    let id = id.ok_or_else(|| CommandError::at(name, 4, format!("Cue inconnu : « {name} »")))?;
                    (id, "cue".to_string())
                }
            };
            let name = s.presets.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
            match action {
                CueAction::Go => {
                    // « go » always starts it (a second go restarts, never stops).
                    if !controls::press_cue_as(s, &id, Some(ClickMode::Restart), true, Some("ui"), source) {
                        return Err(CommandError::at(&name, 4, format!("Cue inconnu : « {name} »")));
                    }
                    Ok(format!("{what} « {name} » : go"))
                }
                CueAction::Stop => {
                    controls::stop_cue(s, &id);
                    Ok(format!("{what} « {name} » : stop"))
                }
            }
        }
    }
}

/// Parses then runs one line: the HTTP route, and later OSC and macros.
pub fn run(s: &mut Shared, line: &str, source: ControlSource) -> Result<String, CommandError> {
    let cmd = parse(line, &s.controls, &s.presets)?;
    execute(s, &cmd, source)
}

/// A number for people: two decimals at most, a decimal comma.
fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    let t = format!("{r:.2}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    let t = if t == "-0" { "0" } else { t };
    t.replace('.', ",")
}

/// A control's current value in words, as a line would set it.
pub fn value_fr(desc: &ControlDesc, v: &serde_json::Value) -> Option<String> {
    match &desc.kind {
        ControlKind::Trigger => None,
        ControlKind::Toggle { .. } | ControlKind::Momentary => v.as_bool().map(|b| if b { "on" } else { "off" }.to_string()),
        ControlKind::Choice { options, .. } => {
            let i = v.as_u64()? as usize;
            options.get(i).map(|o| format!("{} ({o})", i + 1))
        }
        ControlKind::Continuous { unit, .. } => {
            let x = v.as_f64()?;
            Some(match unit {
                Unit::Percent => format!("{} %", num(x * 100.0)),
                Unit::Deg => format!("{}°", num(x)),
                Unit::DegPerSec => format!("{}°/s", num(x)),
                Unit::Bpm => format!("{} BPM", num(x)),
                Unit::Hz => format!("{} Hz", num(x)),
                Unit::Ms => format!("{} ms", num(x)),
                Unit::None => num(x),
            })
        }
    }
}

/// What a value looks like for `desc`.
fn hint_fr(desc: &ControlDesc) -> Option<String> {
    match &desc.kind {
        ControlKind::Trigger => None,
        ControlKind::Toggle { .. } | ControlKind::Momentary => Some("on / off".to_string()),
        ControlKind::Choice { options, .. } => Some(options.iter().enumerate().map(|(i, o)| format!("{} {o}", i + 1)).collect::<Vec<_>>().join(" · ")),
        ControlKind::Continuous { min, max, unit, .. } => {
            let (k, suffix) = match unit {
                Unit::Percent => (100.0, " %"),
                Unit::Deg => (1.0, "°"),
                Unit::DegPerSec => (1.0, "°/s"),
                Unit::Bpm => (1.0, " BPM"),
                Unit::Hz => (1.0, " Hz"),
                Unit::Ms => (1.0, " ms"),
                Unit::None => (1.0, ""),
            };
            Some(format!("{} à {}{suffix}", num(*min as f64 * k), num(*max as f64 * k)))
        }
    }
}

/// The palette's list: every control a line may drive (French label and
/// id), then the cues by name. Grid cells are reached by their cue's name.
pub fn actions(reg: &ControlRegistry, presets: &[Preset]) -> Vec<Action> {
    let mut out: Vec<Action> = reg
        .list()
        .iter()
        .filter(|d| d.group != "grid")
        .filter(|d| {
            let tok = Tok { text: d.id.clone(), pos: 0, quoted: false };
            refused_word(&tok).is_none() && refused_control(&tok, d).is_none()
        })
        .map(|d| {
            let needs_value = !matches!(d.kind, ControlKind::Trigger);
            // `page.3` reads better as the keyword.
            let command = match d.id.strip_prefix("page.").filter(|n| n.parse::<usize>().is_ok()) {
                Some(n) => format!("page {n}"),
                None => d.id.clone(),
            };
            Action { label_fr: d.label_fr.clone(), id: Some(d.id.clone()), command, needs_value, hint_fr: hint_fr(d) }
        })
        .collect();
    for p in presets {
        let quoted = if p.name.contains('"') { format!("«{}»", p.name) } else { format!("\"{}\"", p.name) };
        out.push(Action { label_fr: p.name.clone(), id: None, command: format!("cue {quoted} go"), needs_value: false, hint_fr: None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-486: `fold` built from its character iterator: lower case,
    /// accents dropped, ligatures spelled out.
    #[test]
    fn fold_lowers_drops_accents_and_spells_out_ligatures() {
        assert_eq!(fold("Œuvre ÆTHER Été’s Çà NOËL ñ"), "oeuvre aether ete's ca noel n");
        assert!(folded("Cœur").eq("coeur".chars()));
    }
    use crate::test_support;

    fn reg() -> (ControlRegistry, Vec<Preset>) {
        let presets = crate::presets::catalog();
        (ControlRegistry::build(&presets), presets)
    }

    fn ok(line: &str) -> Command {
        let (reg, presets) = reg();
        parse(line, &reg, &presets).unwrap_or_else(|e| panic!("{line}: {e:?}"))
    }

    fn err(line: &str) -> CommandError {
        let (reg, presets) = reg();
        parse(line, &reg, &presets).expect_err(line)
    }

    fn set(id: &str, v: f32) -> Command {
        Command::Set { id: id.into(), input: ControlInput::Value(v) }
    }

    #[test]
    fn every_form_of_the_grammar_parses() {
        assert_eq!(ok("master.size 80"), set("master.size", 0.8));
        assert_eq!(ok("master.rot_z.angle 45"), set("master.rot_z.angle", 45.0));
        assert_eq!(ok("master.strobe.on on"), set("master.strobe.on", 1.0));
        assert_eq!(ok("transport.freeze off"), set("transport.freeze", 0.0));
        assert_eq!(ok("tempo.tap"), set("tempo.tap", 1.0));
        assert_eq!(ok("master.mirror 2"), set("master.mirror", 1.0));
        let label = crate::live::MIRROR_LABELS[1];
        assert_eq!(ok(&format!("master.mirror \"{label}\"")), set("master.mirror", 1.0));
        assert_eq!(ok(&format!("master.mirror {}", fold(label).to_uppercase())), set("master.mirror", 1.0));
        assert_eq!(ok("cue 2.14 go"), Command::Cue { target: CueTarget::Slot { page: 2, index: 14 }, action: CueAction::Go });
        let (_, presets) = reg();
        let name = &presets[3].name;
        assert_eq!(ok(&format!("cue \"{name}\" stop")), Command::Cue { target: CueTarget::Name(name.clone()), action: CueAction::Stop });
        assert_eq!(ok(&format!("CUE «{}» Go", fold(name))), Command::Cue { target: CueTarget::Name(name.clone()), action: CueAction::Go });
        assert_eq!(ok("page 3"), Command::Page(3));
        assert_eq!(ok("tempo 128"), Command::Tempo(128.0));
        assert_eq!(ok("tap"), set("tempo.tap", 1.0));
        assert_eq!(ok("tout arrêter"), Command::StopAll);
        assert_eq!(ok("TOUT ARRETER"), Command::StopAll);
        assert_eq!(ok("stop"), Command::StopAll);
        assert_eq!(ok("Noir"), set("transport.blackout", 1.0));
        assert_eq!(ok("safety.estop"), set("safety.estop", 1.0));
        // Case of the id, and the live.* alias.
        assert_eq!(ok("MASTER.SIZE 50"), set("master.size", 0.5));
        assert_eq!(ok("live.size 50"), set("master.size", 0.5));
    }

    #[test]
    fn decimal_comma_and_clamping() {
        assert_eq!(ok("master.size 80,5"), set("master.size", 0.805));
        assert_eq!(ok("master.size 80.5%"), set("master.size", 0.805));
        // master.size is 0..200 %.
        assert_eq!(ok("master.size 900"), set("master.size", 2.0));
        assert_eq!(ok("master.size -5"), set("master.size", 0.0));
        assert_eq!(ok("master.rot_z.angle 999"), set("master.rot_z.angle", 180.0));
        assert_eq!(ok("tempo 1000"), Command::Tempo(crate::tempo::MAX_BPM));
        assert_eq!(ok("tempo 99,5"), Command::Tempo(99.5));
    }

    #[test]
    fn refused_words_in_every_case_and_accent() {
        for line in ["armer", "ARMER", "Armer", "réarmer", "REARMER", "Réarmer", "rearmer", "allumer", "Allumer", "ALLUMER", "arm", "ARM",
            "réinitialiser", "Reinitialiser", "RÉINITIALISER", "transport.arm on", "transport.ARM on", "laser armer", "master.size 80 arm",
            "estop.reset", "safety.estop.reset", "tout armer"] {
            let e = err(line);
            assert!(e.refused, "{line}: {e:?}");
            assert!(e.message_fr.starts_with("Commande refusée"), "{line}: {e:?}");
        }
        assert_eq!(err("armer").message_fr, ARM_FR);
        let e = err("tempo 120 allumer");
        assert_eq!((e.token.as_str(), e.pos), ("allumer", 10));
        // Not a refused word: a word that only contains one.
        assert!(!err("alarme").refused);
        assert_eq!(ok("master.reset"), set("master.reset", 1.0));
    }

    #[test]
    fn every_registry_control_is_accepted_or_refused() {
        let (reg, presets) = reg();
        for d in reg.list() {
            let sample = match &d.kind {
                ControlKind::Trigger => d.id.clone(),
                ControlKind::Toggle { .. } | ControlKind::Momentary => format!("{} on", d.id),
                ControlKind::Continuous { .. } => format!("{} 1", d.id),
                ControlKind::Choice { .. } => format!("{} 1", d.id),
            };
            let allowed = d.external && (d.group != "safety" || d.id == "safety.estop");
            match parse(&sample, &reg, &presets) {
                Ok(Command::Set { id, .. }) => {
                    assert!(allowed, "{sample} must be refused");
                    assert_eq!(id, d.id);
                }
                Ok(other) => panic!("{sample}: {other:?}"),
                Err(e) => {
                    assert!(!allowed, "{sample}: {e:?}");
                    assert!(e.refused, "{sample}: {e:?}");
                }
            }
        }
        // And the palette offers none of the refused ones.
        let acts = actions(&reg, &presets);
        assert!(acts.iter().all(|a| a.id.as_deref() != Some("transport.arm") && a.id.as_deref() != Some("safety.hold")));
        assert!(acts.iter().any(|a| a.id.as_deref() == Some("safety.estop")));
        assert!(acts.iter().any(|a| a.label_fr == "Taille maître" && a.command == "master.size" && a.needs_value));
        assert!(acts.iter().any(|a| a.label_fr == "Teinte"));
        assert_eq!(acts.iter().filter(|a| a.id.is_none()).count(), presets.len());
        // Every action's line parses (with a value when it needs one).
        for a in &acts {
            let line = if a.needs_value {
                let d = reg.get(a.id.as_ref().unwrap()).unwrap();
                match d.kind {
                    ControlKind::Toggle { .. } | ControlKind::Momentary => format!("{} off", a.command),
                    _ => format!("{} 1", a.command),
                }
            } else {
                a.command.clone()
            };
            parse(&line, &reg, &presets).unwrap_or_else(|e| panic!("{line}: {e:?}"));
        }
    }

    #[test]
    fn errors_name_the_word_and_its_position() {
        let e = err("mastr.size 80");
        assert_eq!((e.token.as_str(), e.pos, e.refused), ("mastr.size", 0, false));
        assert_eq!(e.message_fr, "Mot inconnu : « mastr.size »");
        let e = err("tempo");
        assert_eq!(e.message_fr, "Valeur attendue après « tempo »");
        assert_eq!(e.pos, 5);
        let e = err("master.size  beaucoup");
        assert_eq!((e.token.as_str(), e.pos), ("beaucoup", 13));
        let e = err("tempo.tap 3");
        assert_eq!((e.token.as_str(), e.pos), ("3", 10));
        assert!(e.message_fr.starts_with("Mot en trop"));
        assert!(err("master.strobe.on peut-être").message_fr.contains("« on » ou « off »"));
        assert!(err("cue 2.14 joue").message_fr.contains("« go » ou « stop »"));
        assert!(err("cue \"Pas un cue\" go").message_fr.starts_with("Cue inconnu"));
        assert!(err("cue 0.3 go").message_fr.starts_with("Case inconnue"));
        assert!(err("page 99").message_fr.starts_with("Page inconnue"));
        assert!(err("master.mirror 99").message_fr.starts_with("Choix inconnu"));
        assert!(err("cue \"ouvert go").message_fr.starts_with("Guillemet non fermé"));
        assert_eq!(err("   ").message_fr, "Commande vide");
        assert_eq!(err("tout").message_fr, "Valeur attendue après « tout » (« tout arrêter » ?)");
    }

    #[test]
    fn a_long_line_is_refused() {
        let line = format!("master.size {}", "0".repeat(MAX_LINE));
        let e = err(&line);
        assert_eq!(e.pos, MAX_LINE);
        assert!(e.message_fr.starts_with("Ligne trop longue"));
        // Exactly the limit is fine (counted in characters, not bytes).
        let line = format!("tempo 120{}", " ".repeat(MAX_LINE - 9));
        assert_eq!(line.chars().count(), MAX_LINE);
        assert_eq!(ok(&line), Command::Tempo(120.0));
        let accents = format!("noir{}", " é".repeat(98));
        assert!(accents.chars().count() <= MAX_LINE && accents.len() > MAX_LINE);
        assert!(!err(&accents).message_fr.starts_with("Ligne trop longue"));
    }

    #[test]
    fn running_lines_drives_the_studio_and_never_arms() {
        let mut s = test_support::shared();
        let src = ControlSource::Command;
        assert_eq!(run(&mut s, "master.size 80", src).unwrap(), "master.size = 80 %");
        assert!((s.live.size - 0.8).abs() < 1e-6);
        assert_eq!(run(&mut s, "master.size 900", src).unwrap(), "master.size = 200 %");
        assert_eq!(s.touches["master.size"].source, ControlSource::Command);
        assert_eq!(run(&mut s, "tempo 128", src).unwrap(), "Tempo = 128 BPM");
        assert!((s.tempo.bpm - 128.0).abs() < 1e-9);
        run(&mut s, "page 2", src).unwrap();
        assert_eq!(s.cue_page, 1);
        // cue 2.14: page 2, row 2, column 6 of an 8-column grid.
        let id = controls::grid_cell_preset(&s.grid, "2.2.6").expect("cell 2.14 of the default grid");
        run(&mut s, "cue 2.14 go", src).unwrap();
        assert!(s.deck.active.iter().any(|a| a.cue == id));
        // go again restarts, never stops.
        run(&mut s, "cue 2.14 go", src).unwrap();
        assert!(s.deck.active.iter().any(|a| a.cue == id));
        run(&mut s, "tout arrêter", src).unwrap();
        assert!(!s.deck.active.iter().any(|a| a.cue == id));
        let name = s.presets.iter().find(|p| p.id == id).unwrap().name.clone();
        run(&mut s, &format!("cue \"{name}\" go"), src).unwrap();
        assert!(s.deck.active.iter().any(|a| a.cue == id));
        run(&mut s, &format!("cue \"{name}\" stop"), src).unwrap();
        assert!(!s.deck.active.iter().any(|a| a.cue == id));
        // An empty cell is an error, not a crash.
        assert!(run(&mut s, "cue 1.999 go", src).unwrap_err().message_fr.starts_with("Case vide"));
        // Nothing arms: the refused lines leave the gate alone.
        for line in ["armer", "transport.arm on", "safety.hold on", "allumer"] {
            assert!(run(&mut s, line, src).unwrap_err().refused, "{line}");
            assert!(!s.gate.is_armed());
        }
        // Noir latches the stop at once.
        assert!(!s.estop.is_latched());
        run(&mut s, "noir", src).unwrap();
        assert!(s.estop.is_latched());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn execute_checks_the_external_flag_again() {
        // A hand-built command past the parser still can't arm.
        let mut s = test_support::shared();
        let e = execute(&mut s, &set("transport.arm", 1.0), ControlSource::Command).unwrap_err();
        assert!(e.refused);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn values_in_words() {
        let (reg, _) = reg();
        let d = |id: &str| reg.get(id).unwrap().clone();
        assert_eq!(value_fr(&d("master.size"), &serde_json::json!(0.805)).unwrap(), "80,5 %");
        assert_eq!(value_fr(&d("master.rot_z.angle"), &serde_json::json!(-45.0)).unwrap(), "-45°");
        assert_eq!(value_fr(&d("master.strobe.on"), &serde_json::json!(true)).unwrap(), "on");
        assert_eq!(hint_fr(&d("master.size")).unwrap(), "0 à 200 %");
        assert_eq!(num(-0.001), "0");
    }
}
