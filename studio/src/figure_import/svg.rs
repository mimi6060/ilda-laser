//! SVG → polylines. Our own small reader on top of `roxmltree` (MIT OR
//! Apache-2.0, a read-only XML parser that never touches the network or
//! the disk): shapes, transforms, colours (attributes, `style`, simple
//! `<style>` class / tag / id rules), `<use>` of the same file. Text,
//! embedded images, gradients (their colour becomes the default one),
//! clips and masks are not drawn.

use super::{laser_color, Options, Path, Pt};
use anyhow::{bail, Result};
use roxmltree::{Document, Node, ParsingOptions};
use std::collections::HashMap;

/// XML nodes accepted (elements, text, attributes' owners...).
const MAX_NODES: u32 = 200_000;
/// XML nesting accepted at all: `roxmltree` parses recursively, so a
/// deeper file is refused before it is parsed.
const MAX_XML_DEPTH: usize = 96;
/// Nesting followed; deeper groups are left out.
const MAX_DEPTH: usize = 64;
const MAX_USE_DEPTH: usize = 8;
const MAX_USES: usize = 5_000;
/// Curve and line segments in the whole drawing, before flattening.
const MAX_SEGMENTS: usize = 200_000;
/// Points after flattening.
const MAX_FLAT_POINTS: usize = 500_000;
const XLINK: &str = "http://www.w3.org/1999/xlink";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color([u8; 3]),
    Current,
    /// A gradient or pattern: drawn in the default colour.
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Style {
    fill: Paint,
    stroke: Paint,
    color: [u8; 3],
    visible: bool,
}

impl Default for Style {
    fn default() -> Self {
        // SVG's initial values: filled black, no stroke.
        Self { fill: Paint::Color([0, 0, 0]), stroke: Paint::None, color: [0, 0, 0], visible: true }
    }
}

/// x' = a x + c y + e, y' = b x + d y + f.
type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn apply(m: &Matrix, p: Pt) -> Pt {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    Line(Pt),
    Quad(Pt, Pt),
    Cubic(Pt, Pt, Pt),
}

#[derive(Clone, Debug)]
struct Sub {
    start: Pt,
    segs: Vec<Seg>,
    closed: bool,
}

impl Sub {
    fn new(start: Pt) -> Self {
        Self { start, segs: Vec::new(), closed: false }
    }
    fn transformed(mut self, m: &Matrix) -> Self {
        self.start = apply(m, self.start);
        for s in &mut self.segs {
            *s = match *s {
                Seg::Line(p) => Seg::Line(apply(m, p)),
                Seg::Quad(c, p) => Seg::Quad(apply(m, c), apply(m, p)),
                Seg::Cubic(c1, c2, p) => Seg::Cubic(apply(m, c1), apply(m, c2), apply(m, p)),
            };
        }
        self
    }
    fn points(&self) -> impl Iterator<Item = Pt> + '_ {
        std::iter::once(self.start).chain(self.segs.iter().flat_map(|s| match *s {
            Seg::Line(p) => vec![p],
            Seg::Quad(c, p) => vec![c, p],
            Seg::Cubic(c1, c2, p) => vec![c1, c2, p],
        }))
    }
}

/// `<style>` rules we understand: one simple selector each.
enum Selector {
    All,
    Tag(String),
    Class(String),
    Id(String),
}

struct Reader<'a, 'input> {
    opts: &'a Options,
    ids: HashMap<&'a str, Node<'a, 'input>>,
    rules: Vec<(Selector, Vec<(String, String)>)>,
    out: Vec<(Sub, [u8; 3])>,
    segments: usize,
    uses: usize,
    text: usize,
    images: usize,
    external: usize,
    too_deep: bool,
    bad_data: usize,
}

pub(super) fn parse(text: &str, opts: &Options, warnings: &mut Vec<String>) -> Result<Vec<Path>> {
    // Internal entities are refused outright: roxmltree already guards
    // against expansion bombs, this keeps even small ones out. External
    // ones are never resolved (no resolver: nothing is fetched).
    if text.contains("<!ENTITY") {
        bail!("SVG refusé : les entités XML (<!ENTITY) ne sont pas prises en charge");
    }
    if xml_depth(text) > MAX_XML_DEPTH {
        bail!("SVG refusé : éléments imbriqués sur plus de {MAX_XML_DEPTH} niveaux");
    }
    let popts = ParsingOptions { allow_dtd: true, nodes_limit: MAX_NODES, ..ParsingOptions::default() };
    let doc = match Document::parse_with_options(text, popts) {
        Ok(d) => d,
        Err(roxmltree::Error::NodesLimitReached) => bail!("SVG trop complexe : plus de {MAX_NODES} éléments"),
        Err(e) => bail!("SVG illisible : {e}"),
    };
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        bail!("ce fichier XML n'est pas un SVG (élément racine « {} »)", root.tag_name().name());
    }
    let mut r = Reader {
        opts,
        ids: doc.descendants().filter_map(|n| n.attribute("id").map(|id| (id, n))).collect(),
        rules: Vec::new(),
        out: Vec::new(),
        segments: 0,
        uses: 0,
        text: 0,
        images: 0,
        external: 0,
        too_deep: false,
        bad_data: 0,
    };
    for n in doc.descendants().filter(|n| n.has_tag_name("style")) {
        let css: String = n.children().filter_map(|c| c.text()).collect();
        r.rules.extend(parse_css(&css));
    }
    r.element(root, Style::default(), &IDENTITY, 0, 0)?;

    if r.text > 0 {
        warnings.push(format!("{} texte(s) ignoré(s) : convertissez le texte en contours (chemins) dans votre logiciel de dessin", r.text));
    }
    if r.images > 0 {
        warnings.push(format!("{} image(s) intégrée(s) ignorée(s) : importez-les séparément comme image", r.images));
    }
    if r.external > 0 {
        warnings.push(format!("{} référence(s) externe(s) ignorée(s) (rien n'est téléchargé)", r.external));
    }
    if r.too_deep {
        warnings.push(format!("groupes imbriqués au-delà de {MAX_DEPTH} niveaux ignorés"));
    }
    if r.bad_data > 0 {
        warnings.push(format!("{} forme(s) aux données invalides ignorée(s) ou tronquée(s)", r.bad_data));
    }
    flatten(r.out)
}

impl<'a, 'input> Reader<'a, 'input> {
    fn element(&mut self, node: Node<'a, 'input>, parent: Style, m: &Matrix, depth: usize, use_depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            self.too_deep = true;
            return Ok(());
        }
        let tag = node.tag_name().name();
        match tag {
            "defs" | "symbol" | "clipPath" | "mask" | "pattern" | "marker" | "linearGradient" | "radialGradient" | "filter" | "metadata"
            | "title" | "desc" | "style" | "script" | "foreignObject" => return Ok(()),
            "text" => {
                self.text += 1;
                return Ok(());
            }
            "image" => {
                self.images += 1;
                return Ok(());
            }
            _ => {}
        }
        let props = self.properties(node);
        let get = |k: &str| props.iter().rev().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
        if get("display") == Some("none") || get("opacity").and_then(number_prefix).is_some_and(|o| o <= 0.0) {
            return Ok(());
        }
        let mut style = parent;
        if let Some(Paint::Color(c)) = get("color").and_then(parse_paint) {
            style.color = c;
        }
        if let Some(p) = get("fill").and_then(parse_paint) {
            style.fill = p;
        }
        if let Some(p) = get("stroke").and_then(parse_paint) {
            style.stroke = p;
        }
        match get("visibility") {
            Some("hidden") | Some("collapse") => style.visible = false,
            Some("visible") => style.visible = true,
            _ => {}
        }
        let mut m = *m;
        if let Some(t) = node.attribute("transform") {
            match parse_transform(t) {
                Some(t) => m = mul(&m, &t),
                None => {
                    self.bad_data += 1;
                    return Ok(());
                }
            }
        }

        match tag {
            "svg" if depth > 0 => {
                // Nested viewport: only its position is followed.
                let (x, y) = (len(node.attribute("x")).unwrap_or(0.0), len(node.attribute("y")).unwrap_or(0.0));
                m = mul(&m, &[1.0, 0.0, 0.0, 1.0, x, y]);
                self.children(node, style, &m, depth, use_depth)
            }
            "svg" | "g" | "a" | "switch" => self.children(node, style, &m, depth, use_depth),
            "use" => self.use_element(node, style, &m, depth, use_depth),
            _ => {
                let subs = match tag {
                    "path" => node.attribute("d").map(|d| self.path_data(d)).unwrap_or_default(),
                    "line" => {
                        let v = ["x1", "y1", "x2", "y2"].map(|k| len(node.attribute(k)).unwrap_or(0.0));
                        let mut s = Sub::new([v[0], v[1]]);
                        s.segs.push(Seg::Line([v[2], v[3]]));
                        vec![s]
                    }
                    "polyline" | "polygon" => self.points_list(node.attribute("points").unwrap_or(""), tag == "polygon"),
                    "rect" => rect(node).into_iter().collect(),
                    "circle" => {
                        let r = len(node.attribute("r")).unwrap_or(0.0);
                        ellipse(len(node.attribute("cx")).unwrap_or(0.0), len(node.attribute("cy")).unwrap_or(0.0), r, r).into_iter().collect()
                    }
                    "ellipse" => {
                        let (rx, ry) = (len(node.attribute("rx")), len(node.attribute("ry")));
                        let (rx, ry) = (rx.or(ry).unwrap_or(0.0), ry.or(rx).unwrap_or(0.0));
                        ellipse(len(node.attribute("cx")).unwrap_or(0.0), len(node.attribute("cy")).unwrap_or(0.0), rx, ry).into_iter().collect()
                    }
                    _ => Vec::new(),
                };
                if subs.is_empty() || !style.visible {
                    return Ok(());
                }
                let Some(color) = self.color_of(&style) else { return Ok(()) };
                for s in subs {
                    self.segments += s.segs.len() + 1;
                    if self.segments > MAX_SEGMENTS {
                        bail!("SVG trop complexe : plus de {MAX_SEGMENTS} segments");
                    }
                    self.out.push((s.transformed(&m), color));
                }
                Ok(())
            }
        }
    }

    fn children(&mut self, node: Node<'a, 'input>, style: Style, m: &Matrix, depth: usize, use_depth: usize) -> Result<()> {
        for c in node.children().filter(|c| c.is_element()) {
            self.element(c, style, m, depth + 1, use_depth)?;
        }
        Ok(())
    }

    /// `<use href="#id">`: only inside this file, a bounded number of times.
    fn use_element(&mut self, node: Node<'a, 'input>, style: Style, m: &Matrix, depth: usize, use_depth: usize) -> Result<()> {
        let href = node.attribute("href").or_else(|| node.attribute((XLINK, "href"))).unwrap_or("");
        let Some(id) = href.strip_prefix('#') else {
            if !href.is_empty() {
                self.external += 1;
            }
            return Ok(());
        };
        let Some(&target) = self.ids.get(id) else { return Ok(()) };
        self.uses += 1;
        if use_depth >= MAX_USE_DEPTH || self.uses > MAX_USES {
            self.too_deep = true;
            return Ok(());
        }
        let (x, y) = (len(node.attribute("x")).unwrap_or(0.0), len(node.attribute("y")).unwrap_or(0.0));
        let m = mul(m, &[1.0, 0.0, 0.0, 1.0, x, y]);
        if target.has_tag_name("symbol") {
            self.children(target, style, &m, depth, use_depth + 1)
        } else {
            self.element(target, style, &m, depth + 1, use_depth + 1)
        }
    }

    /// Presentation attributes, then `<style>` rules (tag, class, id),
    /// then the `style` attribute: the last one found wins.
    fn properties(&self, node: Node) -> Vec<(String, String)> {
        const PROPS: [&str; 6] = ["fill", "stroke", "color", "display", "visibility", "opacity"];
        let mut out: Vec<(String, String)> =
            PROPS.iter().filter_map(|&k| node.attribute(k).map(|v| (k.to_string(), v.trim().to_string()))).collect();
        let tag = node.tag_name().name();
        let classes: Vec<&str> = node.attribute("class").unwrap_or("").split_whitespace().collect();
        let id = node.attribute("id");
        for rank in 0..3 {
            for (sel, decls) in &self.rules {
                let hit = match sel {
                    Selector::All | Selector::Tag(_) if rank != 0 => false,
                    Selector::All => true,
                    Selector::Tag(t) => t == tag,
                    Selector::Class(c) => rank == 1 && classes.contains(&c.as_str()),
                    Selector::Id(i) => rank == 2 && id == Some(i.as_str()),
                };
                if hit {
                    out.extend(decls.iter().cloned());
                }
            }
        }
        if let Some(s) = node.attribute("style") {
            out.extend(declarations(s));
        }
        out
    }

    /// The colour a shape is drawn in: its stroke, else (option) its fill.
    fn color_of(&self, style: &Style) -> Option<[u8; 3]> {
        let pick = |p: Paint| match p {
            Paint::None => None,
            Paint::Color(c) => Some(c),
            Paint::Current => Some(style.color),
            Paint::Other => Some(self.opts.color),
        };
        let c = pick(style.stroke).or_else(|| if self.opts.fills { pick(style.fill) } else { None })?;
        Some(laser_color(c, self.opts.color))
    }

    fn points_list(&mut self, s: &str, closed: bool) -> Vec<Sub> {
        let mut lx = Lexer::new(s);
        let mut pts = Vec::new();
        while let (Some(x), Some(y)) = (lx.number(), lx.number()) {
            pts.push([x, y]);
        }
        lx.skip();
        if !lx.done() {
            self.bad_data += 1;
        }
        let Some(&first) = pts.first() else { return Vec::new() };
        let mut sub = Sub::new(first);
        sub.segs.extend(pts[1..].iter().map(|&p| Seg::Line(p)));
        sub.closed = closed;
        vec![sub]
    }

    /// Path data (`d`): every command, absolute and relative. On an error
    /// the path is kept up to it, as SVG viewers do.
    fn path_data(&mut self, d: &str) -> Vec<Sub> {
        let mut lx = Lexer::new(d);
        let mut subs: Vec<Sub> = Vec::new();
        let mut cur: Pt = [0.0, 0.0];
        let mut start: Pt = [0.0, 0.0];
        let mut last_ctrl: Option<(u8, Pt)> = None;
        let mut cmd: Option<u8> = None;
        let mut open: Option<Sub> = None;
        loop {
            lx.skip();
            if lx.done() {
                break;
            }
            let c = match lx.command() {
                Some(c) => c,
                None => match cmd {
                    // Repeated arguments: after a move, lines.
                    Some(b'M') => b'L',
                    Some(b'm') => b'l',
                    Some(c) if !matches!(c, b'Z' | b'z') => c,
                    _ => {
                        self.bad_data += 1;
                        break;
                    }
                },
            };
            let rel = c.is_ascii_lowercase();
            let base = if rel { cur } else { [0.0, 0.0] };
            let pt = |lx: &mut Lexer| -> Option<Pt> { Some([base[0] + lx.number()?, base[1] + lx.number()?]) };
            let ok = match c.to_ascii_uppercase() {
                b'M' => match pt(&mut lx) {
                    Some(p) => {
                        if let Some(s) = open.take() {
                            subs.push(s);
                        }
                        open = Some(Sub::new(p));
                        cur = p;
                        start = p;
                        true
                    }
                    None => false,
                },
                b'Z' => {
                    if let Some(mut s) = open.take() {
                        s.closed = true;
                        subs.push(s);
                    }
                    cur = start;
                    true
                }
                b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A' => {
                    // A drawing command after Z (or first) starts from the current point.
                    if open.is_none() {
                        open = Some(Sub::new(cur));
                    }
                    let seg: Option<Vec<Seg>> = match c.to_ascii_uppercase() {
                        b'L' => pt(&mut lx).map(|p| vec![Seg::Line(p)]),
                        b'H' => lx.number().map(|x| vec![Seg::Line([base[0] + x, cur[1]])]),
                        b'V' => lx.number().map(|y| vec![Seg::Line([cur[0], base[1] + y])]),
                        b'C' => (|| Some(vec![Seg::Cubic(pt(&mut lx)?, pt(&mut lx)?, pt(&mut lx)?)]))(),
                        b'S' => (|| {
                            let c1 = match last_ctrl {
                                Some((b'C', q)) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                                _ => cur,
                            };
                            Some(vec![Seg::Cubic(c1, pt(&mut lx)?, pt(&mut lx)?)])
                        })(),
                        b'Q' => (|| Some(vec![Seg::Quad(pt(&mut lx)?, pt(&mut lx)?)]))(),
                        b'T' => (|| {
                            let c1 = match last_ctrl {
                                Some((b'Q', q)) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                                _ => cur,
                            };
                            Some(vec![Seg::Quad(c1, pt(&mut lx)?)])
                        })(),
                        _ => (|| {
                            let (rx, ry, rot) = (lx.number()?, lx.number()?, lx.number()?);
                            let (large, sweep) = (lx.flag()?, lx.flag()?);
                            let to = pt(&mut lx)?;
                            Some(arc(cur, rx, ry, rot, large, sweep, to))
                        })(),
                    };
                    match seg {
                        Some(segs) if segs.iter().all(seg_finite) => {
                            last_ctrl = match segs.last() {
                                Some(Seg::Cubic(_, c2, _)) => Some((b'C', *c2)),
                                Some(Seg::Quad(c1, _)) => Some((b'Q', *c1)),
                                _ => None,
                            };
                            if let Some(Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p)) = segs.last() {
                                cur = *p;
                            }
                            if let Some(s) = open.as_mut() {
                                s.segs.extend(segs);
                            }
                            true
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if !ok {
                self.bad_data += 1;
                break;
            }
            if !matches!(c.to_ascii_uppercase(), b'C' | b'S' | b'Q' | b'T') {
                last_ctrl = None;
            }
            cmd = Some(c);
            if self.segments + subs.iter().map(|s| s.segs.len()).sum::<usize>() > MAX_SEGMENTS {
                break;
            }
        }
        if let Some(s) = open.take() {
            subs.push(s);
        }
        subs.retain(|s| !s.segs.is_empty() && s.start.iter().all(|v| v.is_finite()));
        subs
    }
}

/// How deep elements nest, from the raw text (comments, CDATA,
/// declarations and quoted attribute values skipped). Stops counting past
/// the limit.
fn xml_depth(text: &str) -> usize {
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0, 0usize, 0usize);
    let skip_to = |from: usize, end: &[u8]| b[from..].windows(end.len()).position(|w| w == end).map_or(b.len(), |p| from + p + end.len());
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip_to(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_to(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_to(i + 2, b"?>");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_to(i + 2, b">");
        } else if rest.starts_with(b"<!") {
            // DOCTYPE: an internal subset ends with "]>".
            let close = skip_to(i + 2, b">");
            let bracket = b[i..close].contains(&b'[');
            i = if bracket { skip_to(i + 2, b"]>") } else { close };
        } else {
            // A start tag: to its '>' outside quotes.
            let mut j = i + 1;
            let mut quote = 0u8;
            while j < b.len() {
                let c = b[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= b.len() || b[j - 1] != b'/' {
                depth += 1;
                max = max.max(depth);
                if max > MAX_XML_DEPTH {
                    return max;
                }
            }
            i = j + 1;
        }
    }
    max
}

fn seg_finite(s: &Seg) -> bool {
    let ok = |p: &Pt| p[0].is_finite() && p[1].is_finite();
    match s {
        Seg::Line(p) => ok(p),
        Seg::Quad(c, p) => ok(c) && ok(p),
        Seg::Cubic(a, b, p) => ok(a) && ok(b) && ok(p),
    }
}

fn rect(node: Node) -> Option<Sub> {
    let (x, y) = (len(node.attribute("x")).unwrap_or(0.0), len(node.attribute("y")).unwrap_or(0.0));
    let (w, h) = (len(node.attribute("width"))?, len(node.attribute("height"))?);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let (rx, ry) = (len(node.attribute("rx")), len(node.attribute("ry")));
    let rx = rx.or(ry).unwrap_or(0.0).clamp(0.0, w / 2.0);
    let ry = ry.or(Some(rx)).unwrap_or(0.0).clamp(0.0, h / 2.0);
    let mut s = Sub::new([x + rx, y]);
    let corner = |s: &mut Sub, from: Pt, to: Pt| {
        if rx > 0.0 && ry > 0.0 {
            s.segs.extend(arc(from, rx, ry, 0.0, false, true, to));
        }
    };
    s.segs.push(Seg::Line([x + w - rx, y]));
    corner(&mut s, [x + w - rx, y], [x + w, y + ry]);
    s.segs.push(Seg::Line([x + w, y + h - ry]));
    corner(&mut s, [x + w, y + h - ry], [x + w - rx, y + h]);
    s.segs.push(Seg::Line([x + rx, y + h]));
    corner(&mut s, [x + rx, y + h], [x, y + h - ry]);
    s.segs.push(Seg::Line([x, y + ry]));
    corner(&mut s, [x, y + ry], [x + rx, y]);
    s.closed = true;
    Some(s)
}

fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Option<Sub> {
    if !(rx > 0.0 && ry > 0.0) {
        return None;
    }
    let mut s = Sub::new([cx + rx, cy]);
    s.segs.extend(arc([cx + rx, cy], rx, ry, 0.0, false, true, [cx - rx, cy]));
    s.segs.extend(arc([cx - rx, cy], rx, ry, 0.0, false, true, [cx + rx, cy]));
    s.closed = true;
    Some(s)
}

/// An elliptical arc (SVG endpoint form) as cubic Béziers of at most 90°
/// each (SVG 1.1, appendix F.6).
fn arc(from: Pt, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, to: Pt) -> Vec<Seg> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if !(rx.is_finite() && ry.is_finite() && rot_deg.is_finite()) {
        return vec![Seg::Line([f64::NAN, f64::NAN])];
    }
    if rx < 1e-12 || ry < 1e-12 || (from[0] - to[0]).abs() + (from[1] - to[1]).abs() < 1e-12 {
        return vec![Seg::Line(to)];
    }
    let phi = rot_deg.to_radians();
    let (cp, sp) = (phi.cos(), phi.sin());
    let (dx, dy) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cp * cxp - sp * cyp + (from[0] + to[0]) / 2.0;
    let cy = sp * cxp + cp * cyp + (from[1] + to[1]) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_finite() {
            a
        } else {
            0.0
        }
    };
    let theta1 = angle(1.0, 0.0, (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut delta = angle((x1 - cxp) / rx, (y1 - cyp) / ry, (-x1 - cxp) / rx, (-y1 - cyp) / ry);
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let n = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().clamp(1.0, 4.0) as usize;
    let step = delta / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |t: f64| [cx + rx * t.cos() * cp - ry * t.sin() * sp, cy + rx * t.cos() * sp + ry * t.sin() * cp];
    let deriv = |t: f64| [-rx * t.sin() * cp - ry * t.cos() * sp, -rx * t.sin() * sp + ry * t.cos() * cp];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (t1, t2) = (theta1 + step * i as f64, theta1 + step * (i + 1) as f64);
        let (p1, p2) = (at(t1), if i + 1 == n { to } else { at(t2) });
        let (d1, d2) = (deriv(t1), deriv(t2));
        out.push(Seg::Cubic([p1[0] + k * d1[0], p1[1] + k * d1[1]], [p2[0] - k * d2[0], p2[1] - k * d2[1]], p2));
    }
    out
}

/// Curves → polylines, with a tolerance relative to the drawing's size
/// (0.05 % of its diagonal), so the result doesn't depend on its units.
fn flatten(subs: Vec<(Sub, [u8; 3])>) -> Result<Vec<Path>> {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in subs.iter().flat_map(|(s, _)| s.points()) {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
    let tol = if diag.is_finite() && diag > 0.0 { diag * 0.0005 } else { 1.0 };
    let mut total = 0;
    let mut out = Vec::with_capacity(subs.len());
    for (sub, color) in subs {
        let mut pts = vec![sub.start];
        let mut cur = sub.start;
        for seg in &sub.segs {
            match *seg {
                Seg::Line(p) => pts.push(p),
                Seg::Quad(c, p) => {
                    let dd = norm([cur[0] - 2.0 * c[0] + p[0], cur[1] - 2.0 * c[1] + p[1]]);
                    let n = steps(dd / (4.0 * tol));
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let u = 1.0 - t;
                        pts.push([u * u * cur[0] + 2.0 * u * t * c[0] + t * t * p[0], u * u * cur[1] + 2.0 * u * t * c[1] + t * t * p[1]]);
                    }
                }
                Seg::Cubic(c1, c2, p) => {
                    let dd = norm([cur[0] - 2.0 * c1[0] + c2[0], cur[1] - 2.0 * c1[1] + c2[1]])
                        .max(norm([c1[0] - 2.0 * c2[0] + p[0], c1[1] - 2.0 * c2[1] + p[1]]));
                    let n = steps(dd * 0.75 / tol);
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let u = 1.0 - t;
                        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        pts.push([a * cur[0] + b * c1[0] + c * c2[0] + d * p[0], a * cur[1] + b * c1[1] + c * c2[1] + d * p[1]]);
                    }
                }
            }
            let (Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p)) = *seg;
            cur = p;
        }
        total += pts.len();
        if total > MAX_FLAT_POINTS {
            bail!("SVG trop complexe : plus de {MAX_FLAT_POINTS} points une fois les courbes tracées");
        }
        out.push(Path { pts, closed: sub.closed, color });
    }
    Ok(out)
}

fn norm(v: Pt) -> f64 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// Segments for a curve whose error bound is `x / n²`.
fn steps(x: f64) -> usize {
    if x.is_finite() {
        x.sqrt().ceil().clamp(1.0, 256.0) as usize
    } else {
        1
    }
}

/// SVG numbers, commas and whitespace, and single-digit arc flags.
struct Lexer<'s> {
    s: &'s [u8],
    i: usize,
}

impl<'s> Lexer<'s> {
    fn new(s: &'s str) -> Self {
        Self { s: s.as_bytes(), i: 0 }
    }
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }
    fn number(&mut self) -> Option<f64> {
        self.skip();
        let st = self.i;
        let mut j = self.i;
        let at = |j: usize| self.s.get(j).copied().unwrap_or(0);
        if matches!(at(j), b'+' | b'-') {
            j += 1;
        }
        let mut digits = false;
        while at(j).is_ascii_digit() {
            j += 1;
            digits = true;
        }
        if at(j) == b'.' {
            j += 1;
            while at(j).is_ascii_digit() {
                j += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if matches!(at(j), b'e' | b'E') {
            let mut k = j + 1;
            if matches!(at(k), b'+' | b'-') {
                k += 1;
            }
            if at(k).is_ascii_digit() {
                while at(k).is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        let v: f64 = std::str::from_utf8(&self.s[st..j]).ok()?.parse().ok()?;
        self.i = j;
        v.is_finite().then_some(v)
    }
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c == b'0' || c == b'1' {
            self.i += 1;
            Some(c == b'1')
        } else {
            None
        }
    }
}

/// A length attribute: its number, units ignored (a drawing uses one
/// unit throughout, and the figure is fitted anyway); percentages are not
/// supported.
fn len(v: Option<&str>) -> Option<f64> {
    let v = v?.trim();
    if v.ends_with('%') {
        return None;
    }
    number_prefix(v)
}

fn number_prefix(v: &str) -> Option<f64> {
    Lexer::new(v).number()
}

/// `transform="translate(…) rotate(…) …"`, `None` if it can't be read.
fn parse_transform(s: &str) -> Option<Matrix> {
    let mut m = IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let mut lx = Lexer::new(&rest[open + 1..close]);
        let mut a = Vec::new();
        while let Some(v) = lx.number() {
            a.push(v);
            if a.len() > 6 {
                return None;
            }
        }
        let t: Matrix = match (name, a.as_slice()) {
            ("matrix", &[a, b, c, d, e, f]) => [a, b, c, d, e, f],
            ("translate", &[x]) => [1.0, 0.0, 0.0, 1.0, x, 0.0],
            ("translate", &[x, y]) => [1.0, 0.0, 0.0, 1.0, x, y],
            ("scale", &[k]) => [k, 0.0, 0.0, k, 0.0, 0.0],
            ("scale", &[x, y]) => [x, 0.0, 0.0, y, 0.0, 0.0],
            ("rotate", &[r]) | ("rotate", &[r, _, _]) => {
                let (c, s) = (r.to_radians().cos(), r.to_radians().sin());
                let rot = [c, s, -s, c, 0.0, 0.0];
                if let &[_, cx, cy] = a.as_slice() {
                    mul(&mul(&[1.0, 0.0, 0.0, 1.0, cx, cy], &rot), &[1.0, 0.0, 0.0, 1.0, -cx, -cy])
                } else {
                    rot
                }
            }
            ("skewX", &[k]) => [1.0, 0.0, k.to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewY", &[k]) => [1.0, k.to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => return None,
        };
        m = mul(&m, &t);
        rest = rest[close + 1..].trim();
    }
    m.iter().all(|v| v.is_finite()).then_some(m)
}

/// `a: b; c: d` → pairs.
fn declarations(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().trim_end_matches("!important").trim().to_string()))
        .collect()
}

/// `<style>` content: only rules whose selectors are a single class,
/// id, tag or `*`; anything else (combinators, @media…) is skipped.
fn parse_css(css: &str) -> Vec<(Selector, Vec<(String, String)>)> {
    let mut clean = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    clean.push_str(rest);
    let mut out = Vec::new();
    for block in clean.split('}') {
        let Some((sels, body)) = block.split_once('{') else { continue };
        let decls = declarations(body);
        for sel in sels.split(',').map(str::trim) {
            let simple = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "-_".contains(c));
            let sel = if sel == "*" {
                Selector::All
            } else if let Some(c) = sel.strip_prefix('.').filter(|c| simple(c)) {
                Selector::Class(c.into())
            } else if let Some(i) = sel.strip_prefix('#').filter(|i| simple(i)) {
                Selector::Id(i.into())
            } else if simple(sel) {
                Selector::Tag(sel.into())
            } else {
                continue;
            };
            out.push((sel, decls.clone()));
        }
    }
    out
}

/// A paint (`fill`, `stroke`, `color`). `None` for `inherit` and unknown
/// words, so the parent's value stays.
fn parse_paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    match lower.as_str() {
        "none" | "transparent" => return Some(Paint::None),
        "currentcolor" => return Some(Paint::Current),
        "inherit" | "" => return None,
        _ => {}
    }
    if let Some(rest) = lower.strip_prefix("url(") {
        // A fallback colour after the reference is used, else the default.
        let after = rest.split_once(')').map_or("", |(_, a)| a.trim());
        return Some(parse_paint(after).filter(|p| *p != Paint::None || after == "none").unwrap_or(Paint::Other));
    }
    if let Some(h) = lower.strip_prefix('#') {
        let hexv = |s: &str| u8::from_str_radix(s, 16).ok();
        let c = match h.len() {
            3 | 4 => {
                let d: Vec<u8> = h.chars().take(3).map(|c| hexv(&c.to_string()).map(|v| v * 17)).collect::<Option<_>>()?;
                [d[0], d[1], d[2]]
            }
            6 | 8 => [hexv(&h[0..2])?, hexv(&h[2..4])?, hexv(&h[4..6])?],
            _ => return None,
        };
        return Some(Paint::Color(c));
    }
    if let Some(args) = lower.strip_prefix("rgb(").or_else(|| lower.strip_prefix("rgba(")) {
        let args = args.trim_end_matches(')');
        let parts: Vec<&str> = args.split([',', ' ', '/']).filter(|s| !s.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        let ch = |s: &str| -> Option<u8> {
            let (n, pct) = s.strip_suffix('%').map_or((s, false), |n| (n, true));
            let v: f64 = n.parse().ok()?;
            let v = if pct { v * 255.0 / 100.0 } else { v };
            v.is_finite().then(|| v.round().clamp(0.0, 255.0) as u8)
        };
        return Some(Paint::Color([ch(parts[0])?, ch(parts[1])?, ch(parts[2])?]));
    }
    let named = match lower.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "lime" => [0, 255, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "gray" | "grey" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "maroon" => [128, 0, 0],
        "navy" => [0, 0, 128],
        "olive" => [128, 128, 0],
        "teal" => [0, 128, 128],
        "pink" => [255, 192, 203],
        "gold" => [255, 215, 0],
        "violet" => [238, 130, 238],
        "indigo" => [75, 0, 130],
        "brown" => [165, 42, 42],
        _ => return Some(Paint::Other),
    };
    Some(Paint::Color(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(svg: &str) -> Result<Vec<Path>> {
        parse(svg, &Options::default(), &mut Vec::new())
    }
    fn bbox(p: &Path) -> ([f64; 2], [f64; 2]) {
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        for q in &p.pts {
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        (lo, hi)
    }
    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn basic_shapes_with_colours() {
        let paths = run(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <rect x="10" y="10" width="30" height="20" stroke="#ff0000" fill="none"/>
            <circle cx="70" cy="70" r="10" fill="blue"/>
            <ellipse cx="50" cy="50" rx="20" ry="5" style="fill:none;stroke:rgb(0,255,0)"/>
            <line x1="0" y1="0" x2="100" y2="0" stroke="yellow"/>
            <polyline points="0,100 50,90 100,100" stroke="#0ff" fill="none"/>
            <polygon points="10 90 20 80 30 90" fill="white"/>
        </svg>"##)
        .unwrap();
        assert_eq!(paths.len(), 6);
        let (lo, hi) = bbox(&paths[0]);
        assert!(near(lo[0], 10.0) && near(lo[1], 10.0) && near(hi[0], 40.0) && near(hi[1], 30.0));
        assert!(paths[0].closed);
        assert_eq!(paths[0].color, [255, 0, 0]);
        // The circle: flattened, every point on the radius.
        assert_eq!(paths[1].color, [0, 0, 255]);
        assert!(paths[1].pts.len() > 16);
        for q in &paths[1].pts {
            assert!(((q[0] - 70.0).hypot(q[1] - 70.0) - 10.0).abs() < 0.05);
        }
        assert_eq!(paths[2].color, [0, 255, 0]);
        let (lo, hi) = bbox(&paths[2]);
        assert!(near(lo[0], 30.0) && near(hi[0], 70.0) && near(lo[1], 45.0) && near(hi[1], 55.0));
        assert_eq!(paths[3].color, [255, 255, 0]);
        assert!(!paths[3].closed);
        assert_eq!(paths[4].color, [0, 255, 255]);
        assert_eq!(paths[4].pts.len(), 3);
        assert!(paths[5].closed);
    }

    #[test]
    fn path_commands_absolute_relative_curves_and_arcs() {
        let paths = run(r#"<svg xmlns="http://www.w3.org/2000/svg">
            <path d="M10 10 h20 v20 H10 z m40 0 l10-5.5e0 10 5 L50,10 Z" stroke="red"/>
            <path d="M0,0 C0,10 10,10 10,0 S20,-10 20,0 Q25,10 30,0 T40,0" stroke="red" fill="none"/>
            <path d="M100 100 A20 20 0 1 1 100 100.001 M0 50 a10 5 30 0 0 20 0" stroke="red"/>
        </svg>"#)
        .unwrap();
        // Two subpaths in the first path, one in the second, two in the third.
        assert_eq!(paths.len(), 5);
        assert!(paths[0].closed && paths[1].closed);
        assert_eq!(paths[0].pts, vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]]);
        assert_eq!(paths[1].pts[1], [60.0, 4.5]);
        assert_eq!(paths[1].pts[2], [70.0, 9.5]);
        let curve = &paths[2];
        assert_eq!(curve.pts[0], [0.0, 0.0]);
        assert_eq!(*curve.pts.last().unwrap(), [40.0, 0.0]);
        // The cubic peaks at 7.5 (3/4 of its control height), the smooth one mirrors it.
        let (lo, hi) = bbox(curve);
        assert!((hi[1] - 7.5).abs() < 0.1 && (lo[1] + 7.5).abs() < 0.1, "{lo:?} {hi:?}");
        // A near-full circle arc of radius 20.
        let (lo, hi) = bbox(&paths[3]);
        assert!(near(hi[0] - lo[0], 40.0) && near(hi[1] - lo[1], 40.0), "{lo:?} {hi:?}");
        assert!(near(*paths[4].pts.last().unwrap().first().unwrap(), 20.0));
    }

    #[test]
    fn transforms_nest_and_compose() {
        let paths = run(r#"<svg xmlns="http://www.w3.org/2000/svg">
            <g transform="translate(100,0)" stroke="lime">
              <g transform="scale(2)"><line x1="0" y1="0" x2="10" y2="0"/></g>
              <line x1="0" y1="0" x2="10" y2="0" transform="rotate(90)"/>
              <line x1="0" y1="0" x2="10" y2="0" transform="matrix(1 0 0 1 0 5) skewX(45)"/>
              <line x1="10" y1="0" x2="20" y2="0" transform="rotate(180, 10 0)"/>
            </g>
        </svg>"#)
        .unwrap();
        assert_eq!(paths.len(), 4);
        assert_eq!(paths[0].pts, vec![[100.0, 0.0], [120.0, 0.0]]);
        assert!(near(paths[1].pts[1][0], 100.0) && near(paths[1].pts[1][1], 10.0));
        assert!(near(paths[2].pts[1][0], 110.0) && near(paths[2].pts[1][1], 5.0));
        assert!(near(paths[3].pts[1][0], 100.0) && near(paths[3].pts[1][1], 0.0));
        assert!(paths.iter().all(|p| p.color == [0, 255, 0]));
    }

    #[test]
    fn styles_classes_use_and_hidden_things() {
        let mut w = Vec::new();
        let paths = parse(
            r##"<?xml version="1.0"?>
            <!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
            <svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
            <style>/* logo */ .a { fill: #e30613 } #b{stroke:blue;fill:none} path { fill: none; stroke: black }</style>
            <defs><rect id="r" width="10" height="10"/><linearGradient id="g"/></defs>
            <rect class="a" width="5" height="5"/>
            <rect id="b" width="5" height="5"/>
            <path d="M0 0 L5 5"/>
            <use xlink:href="#r" x="50" fill="url(#g)"/>
            <use href="http://example.com/x.svg#a"/>
            <rect width="5" height="5" display="none" stroke="red"/>
            <g style="opacity:0"><rect width="5" height="5" stroke="red"/></g>
            <rect width="5" height="5" visibility="hidden" stroke="red"/>
            <text>Bonjour</text>
            <image href="x.png"/>
            <rect width="5" height="5" fill="none" stroke="none"/>
            </svg>"##,
            &Options::default(),
            &mut w,
        )
        .unwrap();
        assert_eq!(paths.len(), 4, "{paths:?}");
        assert_eq!(paths[0].color, [0xe3, 0x06, 0x13]);
        assert_eq!(paths[1].color, [0, 0, 255]);
        // Black on a laser is invisible: the default colour instead.
        assert_eq!(paths[2].color, [255, 255, 255]);
        // `use` of the defs rect, gradient fill → default colour.
        let (lo, hi) = bbox(&paths[3]);
        assert!(near(lo[0], 50.0) && near(hi[0], 60.0));
        assert_eq!(paths[3].color, [255, 255, 255]);
        let all = w.join(" | ");
        assert!(all.contains("texte") && all.contains("image") && all.contains("externe"), "{all}");
        // Fills off: only strokes are drawn.
        let only = parse(r#"<svg><rect width="5" height="5" fill="red"/><circle r="3" stroke="red"/></svg>"#, &Options { fills: false, ..Options::default() }, &mut w).unwrap();
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn hostile_and_broken_files_are_refused_or_cut_without_panicking() {
        let lol = r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;">]><svg>&lol2;</svg>"#;
        assert!(run(lol).unwrap_err().to_string().contains("entités"));
        let ext = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]><svg>&x;</svg>"#;
        assert!(run(ext).is_err());
        assert!(run("<svg><rect").unwrap_err().to_string().contains("SVG illisible"));
        assert!(run("<html><svg/></html>").unwrap_err().to_string().contains("pas un SVG"));
        // Bad numbers and truncated data: kept up to the error.
        let mut w = Vec::new();
        let paths = parse(
            r#"<svg><path d="M0 0 L10 10 L 1e999 0 L5 5" stroke="red"/><path d="M0 0 C 1 2" stroke="red"/>
            <path d="Q" stroke="red"/><rect width="-5" height="1e400" stroke="red"/>
            <circle r="NaN" stroke="red"/><path d="M0 0 A 1e308 1e308 0 1 1 10 10" stroke="red"/>
            <line x2="10" stroke="red" transform="rotate(oops)"/><polyline points="0 0 1 1 2" stroke="red"/></svg>"#,
            &Options::default(),
            &mut w,
        )
        .unwrap();
        assert!(paths.iter().flat_map(|p| &p.pts).flatten().all(|v| v.is_finite()));
        assert_eq!(paths[0].pts, vec![[0.0, 0.0], [10.0, 10.0]]);
        assert!(w.iter().any(|m| m.contains("invalides")));
        // Deep nesting: refused before parsing (the XML parser recurses),
        // and cut, not followed, a little under that.
        let nest = |n: usize| format!("<svg>{}<rect width='1' height='1' stroke='red'/>{}</svg>", "<g a='>'><!-- <g> -->".repeat(n), "</g>".repeat(n));
        assert!(run(&nest(100_000)).unwrap_err().to_string().contains("imbriqués"));
        assert!(run(&nest(MAX_XML_DEPTH)).unwrap_err().to_string().contains("imbriqués"));
        let mut w = Vec::new();
        assert!(parse(&nest(MAX_XML_DEPTH - 2), &Options::default(), &mut w).unwrap().is_empty());
        assert!(w.iter().any(|m| m.contains("imbriqués")));
        assert_eq!(parse(&nest(10), &Options::default(), &mut w).unwrap().len(), 1);
        assert_eq!(xml_depth("<?xml?><!DOCTYPE svg [<!ELEMENT x ANY>]><svg><![CDATA[<a><b>]]><g/><g><x/></g></svg>"), 2);
        // A `use` loop stays bounded.
        let looped = r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><g id="a"><use xlink:href="#a"/><line x2="1" stroke="red"/></g></svg>"##;
        let paths = run(looped).unwrap();
        assert!(paths.len() <= MAX_USE_DEPTH + 2);
        // Too many segments.
        let huge = format!("<svg><path stroke='red' d='M0 0{}'/></svg>", " l1 1".repeat(MAX_SEGMENTS + 10));
        assert!(run(&huge).unwrap_err().to_string().contains("trop complexe"));
    }

    #[test]
    fn paint_and_transform_parsing() {
        assert_eq!(parse_paint("#abc"), Some(Paint::Color([0xaa, 0xbb, 0xcc])));
        assert_eq!(parse_paint("RGB(100%, 0%, 50%)"), Some(Paint::Color([255, 0, 128])));
        assert_eq!(parse_paint("url(#g) red"), Some(Paint::Color([255, 0, 0])));
        assert_eq!(parse_paint("url(#g)"), Some(Paint::Other));
        assert_eq!(parse_paint("#12"), None);
        assert_eq!(parse_paint("inherit"), None);
        assert!(parse_transform("translate(1,2) scale(3)").is_some());
        assert!(parse_transform("translate(1e999)").is_none());
        assert!(parse_transform("frobnicate(1)").is_none());
        assert!(parse_transform("translate(1").is_none());
    }
}
