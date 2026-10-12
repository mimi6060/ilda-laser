//! The script language (T-458): a small interpreter of our own, written
//! for the sandbox rather than fenced in after the fact.
//!
//! What exists is all there is: numbers, texts, booleans, read-only lists
//! and tables, `let` / `if` / `while` / `for` / `break` / `continue` /
//! `return` (with French aliases), and a closed table of functions
//! (`Builtin`) resolved when the source is read. There is no file, network,
//! clock, environment, process or FFI function to disable: none was ever
//! written, so a script naming one fails with « fonction inconnue » before
//! anything runs. No user function, no recursion, no `eval`, no import.
//!
//! Budgets (`ScriptLimits`): every evaluated node costs one operation; the
//! time and the cancel flag are read every 256 operations; the memory a
//! run creates (texts, lists) is counted cumulatively, never given back,
//! so the peak is bounded; nesting is bounded when parsing, so neither the
//! parser nor the evaluator can overflow the stack.
//!
//! The two doors to the studio are the host's: `etat()` returns the
//! snapshot the host gave (`Host::state`), `commande("…")` only queues a
//! line after the host's refusal check (`Host::refusal`); the engine runs
//! the queue later, through the command line's own checks.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The longest source, in bytes.
pub const MAX_SOURCE: usize = 16 * 1024;
/// The deepest expression tree, and the deepest nesting of blocks,
/// brackets and unary operators while parsing.
pub const MAX_DEPTH: u16 = 64;
const MAX_NESTING: usize = 32;
/// Variables alive at once.
pub const MAX_VARS: usize = 256;
/// `journal` lines kept per run, and their length in characters.
pub const MAX_JOURNAL: usize = 64;
pub const MAX_JOURNAL_LINE: usize = 200;
/// The longest command line (`command::MAX_LINE`).
const MAX_COMMAND: usize = 200;

/// A run's budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptLimits {
    pub max_ms: u32,
    pub max_ops: u64,
    pub max_commands: usize,
    pub max_memory_kb: u32,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self { max_ms: 5, max_ops: 100_000, max_commands: 32, max_memory_kb: 1024 }
    }
}

// ---------- values ----------

#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    List(Rc<Vec<Value>>),
    Map(Rc<BTreeMap<String, Value>>),
}

impl Value {
    fn type_fr(&self) -> &'static str {
        match self {
            Value::Nil => "rien",
            Value::Bool(_) => "un booléen",
            Value::Num(_) => "un nombre",
            Value::Str(_) => "un texte",
            Value::List(_) => "une liste",
            Value::Map(_) => "une table",
        }
    }

    /// From the host's snapshot (JSON, built in the engine thread).
    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Nil,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Num(n.as_f64().filter(|x| x.is_finite()).unwrap_or(0.0)),
            serde_json::Value::String(s) => Value::Str(Rc::from(s.as_str())),
            serde_json::Value::Array(a) => Value::List(Rc::new(a.iter().map(Value::from_json).collect())),
            serde_json::Value::Object(o) => Value::Map(Rc::new(o.iter().map(|(k, v)| (k.clone(), Value::from_json(v))).collect())),
        }
    }
}

/// A number for people: integers without decimals, else four at most.
pub fn num_text(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let t = format!("{x:.4}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" { "0".into() } else { t.to_string() }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Nil => "rien".into(),
        Value::Bool(b) => if *b { "vrai" } else { "faux" }.into(),
        Value::Num(x) => num_text(*x),
        Value::Str(s) => s.to_string(),
        Value::List(l) => format!("[liste de {}]", l.len()),
        Value::Map(m) => format!("[table de {}]", m.len()),
    }
}

// ---------- errors ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax,
    Runtime,
    /// A command the sandbox refuses (arming, reset, safety).
    Refused,
    /// Operations, time or memory ran out.
    Budget,
    /// Stopped from outside (stop, e-stop, disarm).
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptError {
    pub line: Option<u32>,
    pub message_fr: String,
    pub kind: ErrorKind,
}

impl ScriptError {
    fn at(line: u32, kind: ErrorKind, message_fr: impl Into<String>) -> Self {
        Self { line: Some(line), message_fr: message_fr.into(), kind }
    }

    fn syntax(line: u32, message_fr: impl Into<String>) -> Self {
        Self::at(line, ErrorKind::Syntax, message_fr)
    }

    fn runtime(line: u32, message_fr: impl Into<String>) -> Self {
        Self::at(line, ErrorKind::Runtime, message_fr)
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "Ligne {l} : {}", self.message_fr),
            None => f.write_str(&self.message_fr),
        }
    }
}

// ---------- lexer ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kw {
    Let,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    True,
    False,
    And,
    Or,
    Not,
}

fn keyword(word: &str) -> Option<Kw> {
    Some(match word {
        "let" | "soit" => Kw::Let,
        "if" | "si" => Kw::If,
        "else" | "sinon" => Kw::Else,
        "while" | "tantque" => Kw::While,
        "for" | "pour" => Kw::For,
        "in" | "dans" => Kw::In,
        "break" | "sortir" => Kw::Break,
        "continue" | "continuer" => Kw::Continue,
        "return" | "fin" => Kw::Return,
        "true" | "vrai" => Kw::True,
        "false" | "faux" => Kw::False,
        "and" | "et" => Kw::And,
        "or" | "ou" => Kw::Or,
        "not" | "non" => Kw::Not,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Kw(Kw),
    Sym(&'static str),
    End,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: u32,
}

const SYMBOLS: [&str; 24] = ["==", "!=", "<=", ">=", "&&", "||", "..", "(", ")", "{", "}", "[", "]", ",", ".", ";", "=", "<", ">", "+", "-", "*", "/", "%"];

fn lex(src: &str) -> Result<Vec<Token>, ScriptError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' || (c == '/' && chars.get(i + 1) == Some(&'/')) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            // A decimal point only before a digit: `1..3` is a range.
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(|| ScriptError::syntax(line, format!("nombre illisible « {text} »")))?;
            out.push(Token { tok: Tok::Num(n), line });
            continue;
        }
        if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                let Some(&d) = chars.get(i) else { return Err(ScriptError::syntax(line, "texte non fermé")) };
                i += 1;
                match d {
                    '\n' => return Err(ScriptError::syntax(line, "texte non fermé")),
                    '\\' => {
                        let e = chars.get(i).copied().ok_or_else(|| ScriptError::syntax(line, "texte non fermé"))?;
                        i += 1;
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            '\\' | '"' | '\'' => e,
                            _ => return Err(ScriptError::syntax(line, format!("échappement inconnu « \\{e} »"))),
                        });
                    }
                    d if d == c => break,
                    d => s.push(d),
                }
            }
            out.push(Token { tok: Tok::Str(s), line });
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let tok = keyword(&word).map_or(Tok::Ident(word), Tok::Kw);
            out.push(Token { tok, line });
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if let Some(sym) = SYMBOLS.iter().find(|s| s.len() == 2 && **s == two) {
            out.push(Token { tok: Tok::Sym(sym), line });
            i += 2;
            continue;
        }
        if c == '!' {
            out.push(Token { tok: Tok::Kw(Kw::Not), line });
            i += 1;
            continue;
        }
        if let Some(sym) = SYMBOLS.iter().find(|s| s.len() == 1 && s.starts_with(c)) {
            out.push(Token { tok: Tok::Sym(sym), line });
            i += 1;
            continue;
        }
        return Err(ScriptError::syntax(line, format!("caractère inattendu « {c} »")));
    }
    out.push(Token { tok: Tok::End, line });
    Ok(out)
}

// ---------- syntax tree ----------

/// The closed table of functions: the whole surface of the sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Etat,
    Commande,
    Hasard,
    Journal,
    Longueur,
    Entier,
    Arrondi,
    Abs,
    Min,
    Max,
    Texte,
}

impl Builtin {
    /// Every name a script can call (accents and case ignored).
    pub const ALL: [(&'static str, Builtin); 11] = [
        ("etat", Builtin::Etat),
        ("commande", Builtin::Commande),
        ("hasard", Builtin::Hasard),
        ("journal", Builtin::Journal),
        ("longueur", Builtin::Longueur),
        ("entier", Builtin::Entier),
        ("arrondi", Builtin::Arrondi),
        ("abs", Builtin::Abs),
        ("min", Builtin::Min),
        ("max", Builtin::Max),
        ("texte", Builtin::Texte),
    ];

    fn find(name: &str) -> Option<Builtin> {
        let f = crate::command::fold(name);
        Self::ALL.iter().find(|(n, _)| *n == f).map(|(_, b)| *b)
    }

    fn arity(self) -> (usize, usize) {
        match self {
            Builtin::Etat => (0, 0),
            Builtin::Hasard => (0, 1),
            Builtin::Journal => (0, 8),
            Builtin::Min | Builtin::Max => (2, 2),
            _ => (1, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug)]
enum ExprKind {
    Lit(Value),
    Var(String),
    List(Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Builtin, Vec<Expr>),
}

#[derive(Debug)]
struct Expr {
    kind: ExprKind,
    line: u32,
    depth: u16,
}

#[derive(Debug)]
enum Stmt {
    Let(String, Expr, u32),
    Assign(String, Expr, u32),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    ForRange(String, Expr, Expr, Vec<Stmt>, u32),
    ForEach(String, Expr, Vec<Stmt>, u32),
    Break,
    Continue,
    Return,
    Expr(Expr),
}

/// A parsed script.
#[derive(Debug)]
pub struct Program {
    body: Vec<Stmt>,
}

// ---------- parser ----------

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    nesting: usize,
    loops: usize,
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Num(n) => num_text(*n),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Ident(s) => s.clone(),
        Tok::Kw(k) => format!("{k:?}").to_lowercase(),
        Tok::Sym(s) => s.to_string(),
        Tok::End => "fin du script".into(),
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn next(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Tok::Sym(x) if *x == s) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Kw) -> bool {
        if *self.peek() == Tok::Kw(k) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), ScriptError> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(ScriptError::syntax(self.line(), format!("« {s} » attendu, trouvé « {} »", tok_text(self.peek()))))
        }
    }

    fn ident(&mut self) -> Result<String, ScriptError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.next();
                Ok(s)
            }
            t => Err(ScriptError::syntax(self.line(), format!("nom de variable attendu, trouvé « {} »", tok_text(&t)))),
        }
    }

    fn enter(&mut self) -> Result<(), ScriptError> {
        self.nesting += 1;
        if self.nesting > MAX_NESTING {
            return Err(ScriptError::syntax(self.line(), "trop d'imbrications"));
        }
        Ok(())
    }

    fn mk(&self, kind: ExprKind, line: u32) -> Result<Expr, ScriptError> {
        let d = |e: &Expr| e.depth;
        let child = match &kind {
            ExprKind::Lit(_) | ExprKind::Var(_) => 0,
            ExprKind::List(v) | ExprKind::Call(_, v) => v.iter().map(d).max().unwrap_or(0),
            ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => d(a),
            ExprKind::Bin(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) => d(a).max(d(b)),
        };
        if child + 1 > MAX_DEPTH {
            return Err(ScriptError::syntax(line, "expression trop longue ou trop imbriquée"));
        }
        Ok(Expr { kind, line, depth: child + 1 })
    }

    fn program(&mut self) -> Result<Program, ScriptError> {
        let mut body = Vec::new();
        while *self.peek() != Tok::End {
            if self.eat_sym(";") {
                continue;
            }
            body.push(self.stmt()?);
        }
        Ok(Program { body })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, ScriptError> {
        self.enter()?;
        self.expect_sym("{")?;
        let mut body = Vec::new();
        loop {
            if self.eat_sym("}") {
                break;
            }
            if *self.peek() == Tok::End {
                return Err(ScriptError::syntax(self.line(), "« } » attendu avant la fin du script"));
            }
            if self.eat_sym(";") {
                continue;
            }
            body.push(self.stmt()?);
        }
        self.nesting -= 1;
        Ok(body)
    }

    fn stmt(&mut self) -> Result<Stmt, ScriptError> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Kw(Kw::Let) => {
                self.next();
                let name = self.ident()?;
                self.expect_sym("=")?;
                Ok(Stmt::Let(name, self.expr()?, line))
            }
            Tok::Kw(Kw::If) => {
                self.next();
                let mut arms = vec![(self.expr()?, self.block()?)];
                let mut other = None;
                while self.eat_kw(Kw::Else) {
                    if self.eat_kw(Kw::If) {
                        arms.push((self.expr()?, self.block()?));
                    } else {
                        other = Some(self.block()?);
                        break;
                    }
                }
                Ok(Stmt::If(arms, other))
            }
            Tok::Kw(Kw::While) => {
                self.next();
                let cond = self.expr()?;
                self.loops += 1;
                let body = self.block()?;
                self.loops -= 1;
                Ok(Stmt::While(cond, body))
            }
            Tok::Kw(Kw::For) => {
                self.next();
                let name = self.ident()?;
                if !self.eat_kw(Kw::In) {
                    return Err(ScriptError::syntax(self.line(), "« in » (ou « dans ») attendu"));
                }
                let from = self.expr()?;
                let to = if self.eat_sym("..") { Some(self.expr()?) } else { None };
                self.loops += 1;
                let body = self.block()?;
                self.loops -= 1;
                Ok(match to {
                    Some(to) => Stmt::ForRange(name, from, to, body, line),
                    None => Stmt::ForEach(name, from, body, line),
                })
            }
            Tok::Kw(k @ (Kw::Break | Kw::Continue)) => {
                self.next();
                if self.loops == 0 {
                    return Err(ScriptError::syntax(line, format!("« {} » hors d'une boucle", tok_text(&Tok::Kw(k)))));
                }
                Ok(if k == Kw::Break { Stmt::Break } else { Stmt::Continue })
            }
            Tok::Kw(Kw::Return) => {
                self.next();
                Ok(Stmt::Return)
            }
            Tok::Ident(name) if matches!(self.toks.get(self.pos + 1).map(|t| &t.tok), Some(Tok::Sym("="))) => {
                self.next();
                self.next();
                Ok(Stmt::Assign(name, self.expr()?, line))
            }
            _ => Ok(Stmt::Expr(self.expr()?)),
        }
    }

    fn expr(&mut self) -> Result<Expr, ScriptError> {
        self.enter()?;
        let e = self.or();
        self.nesting -= 1;
        e
    }

    fn or(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.and()?;
        loop {
            let line = self.line();
            if self.eat_kw(Kw::Or) || self.eat_sym("||") {
                let b = self.and()?;
                a = self.mk(ExprKind::Or(Box::new(a), Box::new(b)), line)?;
            } else {
                return Ok(a);
            }
        }
    }

    fn and(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.not()?;
        loop {
            let line = self.line();
            if self.eat_kw(Kw::And) || self.eat_sym("&&") {
                let b = self.not()?;
                a = self.mk(ExprKind::And(Box::new(a), Box::new(b)), line)?;
            } else {
                return Ok(a);
            }
        }
    }

    fn not(&mut self) -> Result<Expr, ScriptError> {
        let line = self.line();
        if self.eat_kw(Kw::Not) {
            self.enter()?;
            let a = self.not()?;
            self.nesting -= 1;
            return self.mk(ExprKind::Not(Box::new(a)), line);
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.add()?;
        loop {
            let line = self.line();
            let op = match self.peek() {
                Tok::Sym("==") => BinOp::Eq,
                Tok::Sym("!=") => BinOp::Ne,
                Tok::Sym("<") => BinOp::Lt,
                Tok::Sym("<=") => BinOp::Le,
                Tok::Sym(">") => BinOp::Gt,
                Tok::Sym(">=") => BinOp::Ge,
                _ => return Ok(a),
            };
            self.next();
            let b = self.add()?;
            a = self.mk(ExprKind::Bin(op, Box::new(a), Box::new(b)), line)?;
        }
    }

    fn add(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.mul()?;
        loop {
            let line = self.line();
            let op = match self.peek() {
                Tok::Sym("+") => BinOp::Add,
                Tok::Sym("-") => BinOp::Sub,
                _ => return Ok(a),
            };
            self.next();
            let b = self.mul()?;
            a = self.mk(ExprKind::Bin(op, Box::new(a), Box::new(b)), line)?;
        }
    }

    fn mul(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.unary()?;
        loop {
            let line = self.line();
            let op = match self.peek() {
                Tok::Sym("*") => BinOp::Mul,
                Tok::Sym("/") => BinOp::Div,
                Tok::Sym("%") => BinOp::Rem,
                _ => return Ok(a),
            };
            self.next();
            let b = self.unary()?;
            a = self.mk(ExprKind::Bin(op, Box::new(a), Box::new(b)), line)?;
        }
    }

    fn unary(&mut self) -> Result<Expr, ScriptError> {
        let line = self.line();
        if self.eat_sym("-") {
            self.enter()?;
            let a = self.unary()?;
            self.nesting -= 1;
            return self.mk(ExprKind::Neg(Box::new(a)), line);
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, ScriptError> {
        let mut a = self.primary()?;
        loop {
            let line = self.line();
            if self.eat_sym(".") {
                let name = self.ident()?;
                a = self.mk(ExprKind::Field(Box::new(a), name), line)?;
            } else if self.eat_sym("[") {
                let i = self.expr()?;
                self.expect_sym("]")?;
                a = self.mk(ExprKind::Index(Box::new(a), Box::new(i)), line)?;
            } else if matches!(self.peek(), Tok::Sym("(")) {
                return Err(ScriptError::syntax(line, "seules les fonctions du studio s'appellent"));
            } else {
                return Ok(a);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, ScriptError> {
        let t = self.next();
        let line = t.line;
        match t.tok {
            Tok::Num(n) => self.mk(ExprKind::Lit(Value::Num(n)), line),
            Tok::Str(s) => self.mk(ExprKind::Lit(Value::Str(Rc::from(s.as_str()))), line),
            Tok::Kw(Kw::True) => self.mk(ExprKind::Lit(Value::Bool(true)), line),
            Tok::Kw(Kw::False) => self.mk(ExprKind::Lit(Value::Bool(false)), line),
            Tok::Sym("(") => {
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Sym("[") => {
                self.enter()?;
                let mut items = Vec::new();
                if !self.eat_sym("]") {
                    loop {
                        items.push(self.expr()?);
                        if self.eat_sym("]") {
                            break;
                        }
                        self.expect_sym(",")?;
                    }
                }
                self.nesting -= 1;
                self.mk(ExprKind::List(items), line)
            }
            Tok::Ident(name) => {
                if !self.eat_sym("(") {
                    return self.mk(ExprKind::Var(name), line);
                }
                let f = Builtin::find(&name).ok_or_else(|| ScriptError::syntax(line, format!("fonction inconnue « {name} »")))?;
                self.enter()?;
                let mut args = Vec::new();
                if !self.eat_sym(")") {
                    loop {
                        args.push(self.expr()?);
                        if self.eat_sym(")") {
                            break;
                        }
                        self.expect_sym(",")?;
                    }
                }
                self.nesting -= 1;
                let (lo, hi) = f.arity();
                if args.len() < lo || args.len() > hi {
                    let want = if lo == hi { format!("{lo}") } else { format!("{lo} à {hi}") };
                    return Err(ScriptError::syntax(line, format!("« {name} » prend {want} argument(s), pas {}", args.len())));
                }
                self.mk(ExprKind::Call(f, args), line)
            }
            t => Err(ScriptError::syntax(line, format!("valeur attendue, trouvé « {} »", tok_text(&t)))),
        }
    }
}

/// Reads a script: syntax, unknown functions, nesting. Nothing runs.
pub fn parse(src: &str) -> Result<Program, ScriptError> {
    if src.len() > MAX_SOURCE {
        return Err(ScriptError { line: None, message_fr: format!("script trop long : {} Ko au plus", MAX_SOURCE / 1024), kind: ErrorKind::Syntax });
    }
    let toks = lex(src)?;
    Parser { toks, pos: 0, nesting: 0, loops: 0 }.program()
}

/// The lines given as plain text to `commande("…")`, with their line
/// numbers: the host checks them against the registry when a script is
/// saved or loaded.
pub fn literal_commands(p: &Program) -> Vec<(u32, String)> {
    fn expr(e: &Expr, out: &mut Vec<(u32, String)>) {
        match &e.kind {
            ExprKind::Call(f, args) => {
                if let (Builtin::Commande, [Expr { kind: ExprKind::Lit(Value::Str(s)), .. }]) = (f, args.as_slice()) {
                    out.push((e.line, s.to_string()));
                }
                args.iter().for_each(|a| expr(a, out));
            }
            ExprKind::Lit(_) | ExprKind::Var(_) => {}
            ExprKind::List(v) => v.iter().for_each(|a| expr(a, out)),
            ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => expr(a, out),
            ExprKind::Bin(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Index(a, b) => {
                expr(a, out);
                expr(b, out);
            }
        }
    }
    fn stmts(body: &[Stmt], out: &mut Vec<(u32, String)>) {
        for s in body {
            match s {
                Stmt::Let(_, e, _) | Stmt::Assign(_, e, _) | Stmt::Expr(e) => expr(e, out),
                Stmt::If(arms, other) => {
                    for (c, b) in arms {
                        expr(c, out);
                        stmts(b, out);
                    }
                    if let Some(b) = other {
                        stmts(b, out);
                    }
                }
                Stmt::While(c, b) | Stmt::ForEach(_, c, b, _) => {
                    expr(c, out);
                    stmts(b, out);
                }
                Stmt::ForRange(_, a, z, b, _) => {
                    expr(a, out);
                    expr(z, out);
                    stmts(b, out);
                }
                Stmt::Break | Stmt::Continue | Stmt::Return => {}
            }
        }
    }
    let mut out = Vec::new();
    stmts(&p.body, &mut out);
    out
}

// ---------- the interpreter ----------

/// What the host lends a run.
pub struct Host<'a> {
    /// `etat()`: the snapshot taken when the run was asked for.
    pub state: &'a serde_json::Value,
    /// `hasard()`'s seed.
    pub seed: u64,
    /// Set from outside to stop the run (read every 256 operations).
    pub cancel: Option<&'a AtomicBool>,
    /// Why a command line is refused outright (arming, reset, safety), if it is.
    pub refusal: &'a dyn Fn(&str) -> Option<String>,
}

/// What a run produced, even when it failed (the journal helps).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunOutput {
    /// The queued command lines, with the script line that sent each.
    pub commands: Vec<(u32, String)>,
    pub journal: Vec<String>,
    pub ops: u64,
}

enum Flow {
    Normal,
    Break,
    Continue,
    Return,
}

struct Interp<'a> {
    host: &'a Host<'a>,
    limits: ScriptLimits,
    start: Instant,
    ops: u64,
    mem: usize,
    vars: Vec<(String, Value)>,
    out: RunOutput,
    rng: u64,
    state: Option<Value>,
    journal_cut: bool,
}

fn budget(line: u32, what: &str) -> ScriptError {
    ScriptError::at(line, ErrorKind::Budget, format!("budget dépassé ({what})"))
}

impl Interp<'_> {
    fn tick(&mut self, line: u32) -> Result<(), ScriptError> {
        self.ops += 1;
        if self.ops > self.limits.max_ops {
            return Err(budget(line, &format!("{} opérations", self.limits.max_ops)));
        }
        if self.ops.is_multiple_of(256) {
            if self.start.elapsed().as_secs_f64() * 1000.0 > self.limits.max_ms as f64 {
                return Err(budget(line, &format!("{} ms", self.limits.max_ms)));
            }
            if self.host.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(ScriptError::at(line, ErrorKind::Cancelled, "arrêté"));
            }
        }
        Ok(())
    }

    fn alloc(&mut self, line: u32, bytes: usize) -> Result<(), ScriptError> {
        self.mem = self.mem.saturating_add(bytes);
        if self.mem > self.limits.max_memory_kb as usize * 1024 {
            return Err(budget(line, &format!("mémoire, {} Ko", self.limits.max_memory_kb)));
        }
        Ok(())
    }

    fn new_str(&mut self, line: u32, s: String) -> Result<Value, ScriptError> {
        self.alloc(line, s.len() + 16)?;
        Ok(Value::Str(Rc::from(s)))
    }

    fn lookup(&self, name: &str, line: u32) -> Result<usize, ScriptError> {
        self.vars.iter().rposition(|(n, _)| n == name).ok_or_else(|| ScriptError::runtime(line, format!("variable inconnue « {name} » (la déclarer avec let)")))
    }

    fn declare(&mut self, name: &str, v: Value, line: u32) -> Result<(), ScriptError> {
        if self.vars.len() >= MAX_VARS {
            return Err(ScriptError::runtime(line, format!("{MAX_VARS} variables au plus")));
        }
        self.vars.push((name.to_string(), v));
        Ok(())
    }

    fn block(&mut self, body: &[Stmt]) -> Result<Flow, ScriptError> {
        let mark = self.vars.len();
        let r = self.stmts(body);
        self.vars.truncate(mark);
        r
    }

    fn stmts(&mut self, body: &[Stmt]) -> Result<Flow, ScriptError> {
        for s in body {
            match self.stmt(s)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }

    fn cond(&mut self, e: &Expr) -> Result<bool, ScriptError> {
        match self.eval(e)? {
            Value::Bool(b) => Ok(b),
            v => Err(ScriptError::runtime(e.line, format!("condition : un booléen attendu, pas {}", v.type_fr()))),
        }
    }

    fn stmt(&mut self, s: &Stmt) -> Result<Flow, ScriptError> {
        match s {
            Stmt::Let(name, e, line) => {
                self.tick(*line)?;
                let v = self.eval(e)?;
                self.declare(name, v, *line)?;
            }
            Stmt::Assign(name, e, line) => {
                self.tick(*line)?;
                let v = self.eval(e)?;
                let i = self.lookup(name, *line)?;
                self.vars[i].1 = v;
            }
            Stmt::If(arms, other) => {
                for (c, body) in arms {
                    if self.cond(c)? {
                        return self.block(body);
                    }
                }
                if let Some(body) = other {
                    return self.block(body);
                }
            }
            Stmt::While(c, body) => {
                while self.cond(c)? {
                    self.tick(c.line)?;
                    match self.block(body)? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
            }
            Stmt::ForRange(name, a, z, body, line) => {
                let (a, z) = (self.number(a)?, self.number(z)?);
                let mut i = a.floor();
                while i < z {
                    self.tick(*line)?;
                    let mark = self.vars.len();
                    self.declare(name, Value::Num(i), *line)?;
                    let f = self.block(body);
                    self.vars.truncate(mark);
                    match f? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        Flow::Normal | Flow::Continue => {}
                    }
                    i += 1.0;
                }
            }
            Stmt::ForEach(name, e, body, line) => {
                let items = match self.eval(e)? {
                    Value::List(l) => l,
                    v => return Err(ScriptError::runtime(*line, format!("« for … in » attend une liste ou a..b, pas {}", v.type_fr()))),
                };
                for item in items.iter() {
                    self.tick(*line)?;
                    let mark = self.vars.len();
                    self.declare(name, item.clone(), *line)?;
                    let f = self.block(body);
                    self.vars.truncate(mark);
                    match f? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
            }
            Stmt::Break => return Ok(Flow::Break),
            Stmt::Continue => return Ok(Flow::Continue),
            Stmt::Return => return Ok(Flow::Return),
            Stmt::Expr(e) => {
                self.eval(e)?;
            }
        }
        Ok(Flow::Normal)
    }

    fn number(&mut self, e: &Expr) -> Result<f64, ScriptError> {
        match self.eval(e)? {
            Value::Num(x) => Ok(x),
            v => Err(ScriptError::runtime(e.line, format!("un nombre attendu, pas {}", v.type_fr()))),
        }
    }

    fn finite(line: u32, x: f64) -> Result<Value, ScriptError> {
        if x.is_finite() {
            Ok(Value::Num(x))
        } else {
            Err(ScriptError::runtime(line, "calcul impossible (division par zéro ?)"))
        }
    }

    /// Structural equality, one operation per element compared.
    fn equal(&mut self, a: &Value, b: &Value, line: u32) -> Result<bool, ScriptError> {
        self.tick(line)?;
        Ok(match (a, b) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Num(x), Value::Num(y)) => x == y,
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::List(x), Value::List(y)) => {
                if x.len() != y.len() {
                    return Ok(false);
                }
                for (p, q) in x.iter().zip(y.iter()) {
                    if !self.equal(p, q, line)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::Map(x), Value::Map(y)) => Rc::ptr_eq(x, y),
            _ => false,
        })
    }

    fn eval(&mut self, e: &Expr) -> Result<Value, ScriptError> {
        let line = e.line;
        self.tick(line)?;
        match &e.kind {
            ExprKind::Lit(v) => Ok(v.clone()),
            ExprKind::Var(name) => {
                let i = self.lookup(name, line)?;
                Ok(self.vars[i].1.clone())
            }
            ExprKind::List(items) => {
                self.alloc(line, 16 * items.len() + 16)?;
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?);
                }
                Ok(Value::List(Rc::new(out)))
            }
            ExprKind::Neg(a) => {
                let x = self.number(a)?;
                Ok(Value::Num(-x))
            }
            ExprKind::Not(a) => Ok(Value::Bool(!self.cond(a)?)),
            ExprKind::And(a, b) => Ok(Value::Bool(self.cond(a)? && self.cond(b)?)),
            ExprKind::Or(a, b) => Ok(Value::Bool(self.cond(a)? || self.cond(b)?)),
            ExprKind::Bin(op, a, b) => {
                let (x, y) = (self.eval(a)?, self.eval(b)?);
                self.binary(*op, x, y, line)
            }
            ExprKind::Field(a, name) => match self.eval(a)? {
                Value::Map(m) => m.get(name).cloned().ok_or_else(|| ScriptError::runtime(line, format!("champ inconnu « {name} »"))),
                v => Err(ScriptError::runtime(line, format!("« .{name} » : une table attendue, pas {}", v.type_fr()))),
            },
            ExprKind::Index(a, i) => {
                let target = self.eval(a)?;
                let key = self.eval(i)?;
                match (target, key) {
                    (Value::List(l), Value::Num(n)) => {
                        // 1-based, like `hasard(n)` and the pages.
                        if n.fract() != 0.0 || n < 1.0 || n > l.len() as f64 {
                            return Err(ScriptError::runtime(line, format!("indice {} hors de la liste (1 à {})", num_text(n), l.len())));
                        }
                        Ok(l[n as usize - 1].clone())
                    }
                    (Value::Map(m), Value::Str(k)) => m.get(&*k).cloned().ok_or_else(|| ScriptError::runtime(line, format!("champ inconnu « {k} »"))),
                    (Value::Str(s), Value::Num(n)) => {
                        let count = s.chars().count();
                        if n.fract() != 0.0 || n < 1.0 || n > count as f64 {
                            return Err(ScriptError::runtime(line, format!("indice {} hors du texte (1 à {count})", num_text(n))));
                        }
                        let c = s.chars().nth(n as usize - 1).map(String::from).unwrap_or_default();
                        self.new_str(line, c)
                    }
                    (t, k) => Err(ScriptError::runtime(line, format!("on ne peut pas indexer {} par {}", t.type_fr(), k.type_fr()))),
                }
            }
            ExprKind::Call(f, args) => self.call(*f, args, line),
        }
    }

    fn binary(&mut self, op: BinOp, x: Value, y: Value, line: u32) -> Result<Value, ScriptError> {
        use Value::*;
        match (op, &x, &y) {
            (BinOp::Eq, _, _) => Ok(Bool(self.equal(&x, &y, line)?)),
            (BinOp::Ne, _, _) => Ok(Bool(!self.equal(&x, &y, line)?)),
            (BinOp::Add, Num(a), Num(b)) => Self::finite(line, a + b),
            (BinOp::Sub, Num(a), Num(b)) => Self::finite(line, a - b),
            (BinOp::Mul, Num(a), Num(b)) => Self::finite(line, a * b),
            (BinOp::Div, Num(a), Num(b)) => Self::finite(line, a / b),
            (BinOp::Rem, Num(a), Num(b)) => Self::finite(line, a.rem_euclid(*b)),
            (BinOp::Add, Str(_), _) | (BinOp::Add, _, Str(_)) => {
                let (a, b) = (text_of(&x), text_of(&y));
                self.alloc(line, a.len() + b.len())?;
                self.new_str(line, a + &b)
            }
            (BinOp::Add, List(a), List(b)) => {
                self.alloc(line, 16 * (a.len() + b.len()) + 16)?;
                let mut v = Vec::with_capacity(a.len() + b.len());
                v.extend(a.iter().cloned());
                v.extend(b.iter().cloned());
                Ok(List(Rc::new(v)))
            }
            (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Num(a), Num(b)) => Ok(Bool(cmp(op, a.partial_cmp(b)))),
            (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Str(a), Str(b)) => Ok(Bool(cmp(op, a.partial_cmp(b)))),
            _ => Err(ScriptError::runtime(line, format!("opération impossible entre {} et {}", x.type_fr(), y.type_fr()))),
        }
    }

    fn rand(&mut self) -> f64 {
        // xorshift64*: enough for picking cues, and reproducible.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn call(&mut self, f: Builtin, args: &[Expr], line: u32) -> Result<Value, ScriptError> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.eval(a)?);
        }
        let num = |v: &Value, what: &str| match v {
            Value::Num(x) => Ok(*x),
            v => Err(ScriptError::runtime(line, format!("« {what} » attend un nombre, pas {}", v.type_fr()))),
        };
        match f {
            Builtin::Etat => {
                if self.state.is_none() {
                    self.state = Some(Value::from_json(self.host.state));
                }
                Ok(self.state.clone().unwrap_or(Value::Nil))
            }
            Builtin::Commande => {
                let Value::Str(cmd) = &vals[0] else {
                    return Err(ScriptError::runtime(line, format!("« commande » attend un texte, pas {}", vals[0].type_fr())));
                };
                if cmd.chars().count() > MAX_COMMAND {
                    return Err(ScriptError::runtime(line, format!("commande trop longue : {MAX_COMMAND} caractères au plus")));
                }
                if let Some(why) = (self.host.refusal)(cmd) {
                    return Err(ScriptError::at(line, ErrorKind::Refused, format!("commande refusée « {} » : {why}", cmd.trim())));
                }
                if self.out.commands.len() >= self.limits.max_commands {
                    self.log(format!("Ligne {line} : commande ignorée « {} » ({} au plus par exécution)", cmd.trim(), self.limits.max_commands));
                    return Ok(Value::Bool(false));
                }
                self.alloc(line, cmd.len() + 16)?;
                self.out.commands.push((line, cmd.to_string()));
                Ok(Value::Bool(true))
            }
            Builtin::Hasard => match vals.first() {
                None => Ok(Value::Num(self.rand())),
                Some(v) => {
                    let n = num(v, "hasard")?;
                    if n.fract() != 0.0 || !(1.0..=1e9).contains(&n) {
                        return Err(ScriptError::runtime(line, "« hasard(n) » attend un entier de 1 à 1 000 000 000"));
                    }
                    Ok(Value::Num((self.rand() * n).floor().min(n - 1.0) + 1.0))
                }
            },
            Builtin::Journal => {
                let text: String = vals.iter().map(text_of).collect::<Vec<_>>().join(" ");
                let text: String = text.chars().take(MAX_JOURNAL_LINE).collect();
                self.alloc(line, text.len() + 16)?;
                self.log(text);
                Ok(Value::Nil)
            }
            Builtin::Longueur => match &vals[0] {
                Value::Str(s) => Ok(Value::Num(s.chars().count() as f64)),
                Value::List(l) => Ok(Value::Num(l.len() as f64)),
                Value::Map(m) => Ok(Value::Num(m.len() as f64)),
                v => Err(ScriptError::runtime(line, format!("« longueur » d'{} ?", v.type_fr()))),
            },
            Builtin::Entier => Ok(Value::Num(num(&vals[0], "entier")?.floor())),
            Builtin::Arrondi => Ok(Value::Num(num(&vals[0], "arrondi")?.round())),
            Builtin::Abs => Ok(Value::Num(num(&vals[0], "abs")?.abs())),
            Builtin::Min => Ok(Value::Num(num(&vals[0], "min")?.min(num(&vals[1], "min")?))),
            Builtin::Max => Ok(Value::Num(num(&vals[0], "max")?.max(num(&vals[1], "max")?))),
            Builtin::Texte => {
                let t = text_of(&vals[0]);
                self.new_str(line, t)
            }
        }
    }

    fn log(&mut self, text: String) {
        if self.out.journal.len() < MAX_JOURNAL {
            self.out.journal.push(text);
        } else if !self.journal_cut {
            self.journal_cut = true;
            self.out.journal.push(format!("… journal limité à {MAX_JOURNAL} lignes par exécution"));
        }
    }
}

fn cmp(op: BinOp, o: Option<std::cmp::Ordering>) -> bool {
    use std::cmp::Ordering::*;
    matches!((op, o), (BinOp::Lt, Some(Less)) | (BinOp::Le, Some(Less | Equal)) | (BinOp::Gt, Some(Greater)) | (BinOp::Ge, Some(Greater | Equal)))
}

/// Parses and runs `src` within `limits`. The output carries the queued
/// commands and the journal even when the run failed (the host drops the
/// commands of a failed run).
pub fn run(src: &str, limits: ScriptLimits, host: &Host) -> (Result<(), ScriptError>, RunOutput) {
    let start = Instant::now();
    let program = match parse(src) {
        Ok(p) => p,
        Err(e) => return (Err(e), RunOutput::default()),
    };
    let mut it = Interp { host, limits, start, ops: 0, mem: 0, vars: Vec::new(), out: RunOutput::default(), rng: host.seed | 1, state: None, journal_cut: false };
    let r = it.stmts(&program.body).map(|_| ());
    it.out.ops = it.ops;
    (r, it.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_refusal(_: &str) -> Option<String> {
        None
    }

    fn arm_refused(line: &str) -> Option<String> {
        line.contains("armer").then(|| "le laser ne s'arme qu'à l'écran".to_string())
    }

    fn go(src: &str) -> (Result<(), ScriptError>, RunOutput) {
        go_with(src, ScriptLimits { max_ms: 2_000, ..ScriptLimits::default() })
    }

    fn go_with(src: &str, limits: ScriptLimits) -> (Result<(), ScriptError>, RunOutput) {
        let state = serde_json::json!({ "tempo": { "bpm": 128, "mesure": 7 }, "page": 2, "controles": { "master.size": 0.8 }, "cues": ["Vague", "Tunnel"] });
        let host = Host { state: &state, seed: 42, cancel: None, refusal: &arm_refused };
        run(src, limits, &host)
    }

    fn ok(src: &str) -> RunOutput {
        let (r, out) = go(src);
        r.unwrap_or_else(|e| panic!("{src}: {e}"));
        out
    }

    fn err(src: &str) -> ScriptError {
        go(src).0.expect_err(src)
    }

    #[test]
    fn reads_the_state_and_queues_commands() {
        let out = ok(r#"
            let e = etat()
            if e.tempo.bpm > 120 and e.page == 2 {
                commande("page " + texte(e.page + 1))
            }
            journal("taille", e.controles["master.size"] * 100, e.cues[2])
        "#);
        assert_eq!(out.commands, vec![(4, "page 3".to_string())]);
        assert_eq!(out.journal, vec!["taille 80 Tunnel".to_string()]);
    }

    #[test]
    fn french_keywords_and_loops() {
        let out = ok("soit n = 0\npour i dans 0..10 { si i % 2 == 0 { continuer } n = n + i }\ntantque vrai { sortir }\njournal(n)\nfin\njournal(\"jamais\")");
        assert_eq!(out.journal, vec!["25"]);
        let out = ok("for x in [3, 4] { journal(x * 2) }");
        assert_eq!(out.journal, vec!["6", "8"]);
    }

    #[test]
    fn at_most_the_command_limit_per_run() {
        let out = ok(r#"for i in 0..40 { commande("tap") }"#);
        assert_eq!(out.commands.len(), 32);
        assert!(out.journal[0].contains("commande ignorée"), "{:?}", out.journal);
    }

    #[test]
    fn refused_commands_stop_the_run_with_their_line() {
        let e = err("journal(1)\n\ncommande(\"armer\")");
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(e.to_string(), "Ligne 3 : commande refusée « armer » : le laser ne s'arme qu'à l'écran");
        // Built at run time: still refused.
        assert_eq!(err("let a = \"ar\" + \"mer\"\ncommande(a)").kind, ErrorKind::Refused);
    }

    #[test]
    fn an_infinite_loop_is_stopped_by_its_budget() {
        let t = Instant::now();
        let (r, _) = go_with("while true {}", ScriptLimits::default());
        let e = r.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Budget);
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        // Time first when the operations are plenty.
        let (r, _) = go_with("let x = 0\nwhile true { x = x + 1 }", ScriptLimits { max_ops: u64::MAX, max_ms: 20, ..ScriptLimits::default() });
        let e = r.unwrap_err();
        assert!(e.message_fr.contains("20 ms"), "{e}");
        assert!(t.elapsed().as_millis() < 1_000);
    }

    #[test]
    fn huge_allocations_are_refused() {
        for src in [
            "let s = \"x\"\nwhile true { s = s + s }",
            "let l = [1]\nwhile true { l = l + l }",
            "let l = []\nfor i in 0..1000000 { l = l + [i] }",
        ] {
            let (r, _) = go_with(src, ScriptLimits { max_ops: u64::MAX, max_ms: 5_000, ..ScriptLimits::default() });
            let e = r.unwrap_err();
            assert_eq!(e.kind, ErrorKind::Budget, "{src}");
            assert!(e.message_fr.contains("mémoire"), "{src}: {e}");
        }
    }

    #[test]
    fn system_access_does_not_exist() {
        for (src, name) in [
            ("lire_fichier(\"/etc/passwd\")", "lire_fichier"),
            ("open(\"/tmp/x\")", "open"),
            ("fichier(\"a\")", "fichier"),
            ("reseau(\"192.168.129.51\")", "reseau"),
            ("http(\"http://example.com\")", "http"),
            ("heure()", "heure"),
            ("now()", "now"),
            ("env(\"HOME\")", "env"),
            ("exec(\"ls\")", "exec"),
            ("system(\"rm -rf /\")", "system"),
            ("import(\"os\")", "import"),
            ("eval(\"1\")", "eval"),
            ("print(1)", "print"),
        ] {
            let e = parse(src).expect_err(src);
            assert_eq!(e.message_fr, format!("fonction inconnue « {name} »"), "{src}");
            assert_eq!(e.kind, ErrorKind::Syntax);
        }
        // Not callable through a value either.
        assert!(parse("let f = etat\nf()").is_err());
        assert!(parse("etat().tempo()").is_err());
    }

    #[test]
    fn errors_carry_their_line() {
        assert_eq!(err("let a = 1\nlet b = a +\n").line, Some(3));
        assert_eq!(err("let a = 1\n\njournal(b)").to_string(), "Ligne 3 : variable inconnue « b » (la déclarer avec let)");
        assert_eq!(err("let a = 1 / 0").kind, ErrorKind::Runtime);
        assert_eq!(err("if 1 { }").to_string(), "Ligne 1 : condition : un booléen attendu, pas un nombre");
        assert_eq!(err("let l = [1]\nl[2]").line, Some(2));
        assert!(err("break").message_fr.contains("hors d'une boucle"));
        assert!(err("\"abc").message_fr.contains("non fermé"));
        assert!(err("hasard(1, 2)").message_fr.contains("argument"));
    }

    #[test]
    fn nesting_is_bounded_without_overflowing() {
        let deep = format!("{}1{}", "(".repeat(5_000), ")".repeat(5_000));
        assert!(parse(&deep).is_err());
        let chain = format!("let a = 1{}", " + 1".repeat(2_000));
        assert!(parse(&chain).unwrap_err().message_fr.contains("imbriquée"));
        let blocks = format!("{}{}", "if true { ".repeat(200), "}".repeat(200));
        assert!(parse(&blocks).is_err());
        let neg = format!("let a = {}1", "-".repeat(3_000));
        assert!(parse(&neg).is_err());
        assert!(parse(&"x".repeat(MAX_SOURCE + 1)).is_err());
    }

    #[test]
    fn variables_are_bounded() {
        let src: String = (0..300).map(|i| format!("let v{i} = {i}\n")).collect();
        assert!(err(&src).message_fr.contains("variables au plus"));
    }

    #[test]
    fn random_is_seeded_and_in_range() {
        let a = ok("for i in 0..50 { journal(hasard(6)) }").journal;
        assert_eq!(a, ok("for i in 0..50 { journal(hasard(6)) }").journal);
        assert!(a.iter().all(|x| (1..=6).contains(&x.parse::<i32>().unwrap())));
        assert!(a.iter().any(|x| x == "1") && a.iter().any(|x| x == "6"));
    }

    #[test]
    fn a_cancelled_run_stops() {
        let state = serde_json::json!({});
        let flag = AtomicBool::new(true);
        let host = Host { state: &state, seed: 1, cancel: Some(&flag), refusal: &no_refusal };
        let (r, _) = run("while true {}", ScriptLimits { max_ms: 10_000, max_ops: u64::MAX, ..ScriptLimits::default() }, &host);
        assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
    }

    #[test]
    fn literal_commands_are_listed() {
        let p = parse("commande(\"tap\")\nif true {\n  commande('cue \"A\" go')\n}\ncommande(\"page \" + 2)").unwrap();
        assert_eq!(literal_commands(&p), vec![(1, "tap".into()), (3, "cue \"A\" go".into())]);
    }

    #[test]
    fn the_journal_is_bounded() {
        let out = ok("for i in 0..100 { journal(i) }");
        assert_eq!(out.journal.len(), MAX_JOURNAL + 1);
    }

    #[test]
    fn numbers_print_for_people() {
        assert_eq!(num_text(3.0), "3");
        assert_eq!(num_text(0.1 + 0.2), "0.3");
        assert_eq!(num_text(-2.5), "-2.5");
    }
}
