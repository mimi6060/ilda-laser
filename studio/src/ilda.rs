//! ILDA Image Data Transfer Format (T-001): our own reader and writer,
//! written from the public ILDA IDTF specification (rev. 011).
//!
//! - Reads formats 0 (3D indexed), 1 (2D indexed), 2 (palette), 4 (3D
//!   true colour) and 5 (2D true colour), mixed in one file. Values are
//!   big-endian. Status bit 6 (blanking) wins over the colour; bit 7 (last
//!   point) is informative only, the header's record count is what counts.
//! - Palettes are per projector: a format 2 section applies to the
//!   indexed frames that follow it for that projector. Without one, the
//!   spec's suggested 64-colour default palette is used (generated here
//!   from its gradients, not copied).
//! - Writes format 5, which is what the ShowNET SD card plays.
//!
//! Files come from the user and may be truncated or hostile: every read
//! goes through bounds-checked slices and returns an error (in French, for
//! the UI), never panics - a panic would trip the studio's global panic
//! hook. Z is dropped on import: the studio draws in 2D.
//!
//! Nothing here touches the engine or an HTTP worker: big files are parsed
//! on their own thread with `spawn_read` (wired to the UI by T-011).

// Not wired to the UI yet (T-011): only the tests call most of this.
#![cfg_attr(not(test), allow(dead_code))]

use crate::patterns::Point;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

/// Largest ILDA file the studio reads (well above any real show file; a
/// cap keeps a hostile file from eating the memory).
pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Recommended largest file for the ShowNET SD card (Laserworld's
/// public custom content guide).
pub const SHOWNET_MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Most frames / points per frame a file can describe (u16 counts).
pub const MAX_FRAMES: usize = 65_535;
pub const MAX_POINTS_PER_FRAME: usize = 65_535;

const HEADER_LEN: usize = 32;
const STATUS_LAST: u8 = 0x80;
const STATUS_BLANK: u8 = 0x40;

/// One image of an ILDA file, in the studio's normalized points
/// (x/y -1..1, y up; r/g/b 0..1; blanked points are black).
#[derive(Clone, Debug, PartialEq)]
pub struct IldaFrame {
    /// Frame name from the header (up to 8 ASCII characters).
    pub name: String,
    /// Company name from the header (up to 8 ASCII characters).
    pub company: String,
    /// Projector number from the header (0-255).
    pub projector: u8,
    pub points: Vec<Point>,
}

impl IldaFrame {
    pub fn new(name: &str, points: Vec<Point>) -> Self {
        Self { name: name.to_string(), company: String::new(), projector: 0, points }
    }
}

/// The spec's suggested 64-colour default palette, built from its
/// gradients: red→yellow (0-16), yellow→green (16-24), green→cyan
/// (24-31), cyan→blue (31-40), blue→magenta (40-48), magenta→white
/// (48-56), white→pink (56-63).
pub fn default_palette() -> Vec<[u8; 3]> {
    let up = |step: u32, k: u32| (step * k).min(255) as u8;
    let down = |step: u32, k: u32| 256u32.saturating_sub(step * k).min(255) as u8;
    let frac = |num: u32, den: u32| ((255 * num) as f32 / den as f32).round() as u8;
    (0u32..64)
        .map(|i| match i {
            0..=16 => [255, up(16, i), 0],
            17..=24 => [down(32, i - 16), 255, 0],
            25..=31 => [0, 255, frac(i - 24, 7)],
            32..=40 => [0, frac(40 - i, 9), 255],
            41..=48 => [up(32, i - 40), 0, 255],
            49..=56 => [255, up(32, i - 48), up(32, i - 48)],
            _ => [255, down(32, i - 56), down(32, i - 56)],
        })
        .collect()
}

/// Bounds-checked reader over the file bytes.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

fn be_u16(b: &[u8], at: usize) -> u16 {
    match b.get(at..at + 2) {
        Some(s) => u16::from_be_bytes([s[0], s[1]]),
        None => 0,
    }
}

fn be_i16(b: &[u8], at: usize) -> i16 {
    be_u16(b, at) as i16
}

/// Header text: up to the first NUL, printable ASCII only, trimmed.
fn header_text(b: &[u8]) -> String {
    b.iter()
        .take_while(|&&c| c != 0)
        .map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '_' })
        .collect::<String>()
        .trim()
        .to_string()
}

fn coord(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(-1.0, 1.0)
}

fn point(x: i16, y: i16, status: u8, rgb: [u8; 3]) -> Point {
    if status & STATUS_BLANK != 0 {
        return Point::lit(coord(x), coord(y), 0.0, 0.0, 0.0);
    }
    let c = |v: u8| v as f32 / 255.0;
    Point::lit(coord(x), coord(y), c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

/// Parse an ILDA file. Errors on a bad signature, an unknown format code,
/// a truncated header or record, or a file without any image. A file that
/// ends cleanly after a section without the closing 0-record header is
/// accepted (common in the wild); bytes after the closing header are
/// ignored.
pub fn read(data: &[u8]) -> Result<Vec<IldaFrame>> {
    if data.is_empty() {
        bail!("Fichier ILDA vide");
    }
    if data.len() as u64 > MAX_READ_BYTES {
        bail!("Fichier ILDA trop gros (max {} Mo)", MAX_READ_BYTES / (1024 * 1024));
    }
    let defaults = default_palette();
    let mut palettes: HashMap<u8, Vec<[u8; 3]>> = HashMap::new();
    let mut frames = Vec::new();
    let mut cur = Cursor { data, pos: 0 };
    let mut section = 0usize;
    while cur.remaining() > 0 {
        section += 1;
        let at = cur.pos;
        let h = cur
            .take(HEADER_LEN)
            .ok_or_else(|| anyhow!("Fichier ILDA tronqué : en-tête {section} incomplet (octet {at})"))?;
        if &h[0..4] != b"ILDA" {
            bail!("Fichier ILDA invalide : signature absente à l'octet {at}");
        }
        let format = h[7];
        let name = header_text(&h[8..16]);
        let company = header_text(&h[16..24]);
        let records = be_u16(h, 24) as usize;
        let projector = h[30];
        if records == 0 {
            break; // end of file
        }
        let size = match format {
            0 => 8,
            1 => 6,
            2 => 3,
            4 => 10,
            5 => 8,
            3 => bail!("Format ILDA 3 non standard (jamais approuvé), non pris en charge"),
            f => bail!("Format ILDA inconnu : {f} (section {section})"),
        };
        let body = cur.take(records * size).ok_or_else(|| {
            anyhow!("Fichier ILDA tronqué : section {section} annonce {records} enregistrements")
        })?;
        let recs = body.chunks_exact(size);
        if format == 2 {
            palettes.insert(projector, recs.map(|r| [r[0], r[1], r[2]]).collect());
            continue;
        }
        let palette = palettes.get(&projector).unwrap_or(&defaults);
        let indexed = |i: u8| {
            palette
                .get(i as usize)
                .or_else(|| defaults.get(i as usize))
                .copied()
                .unwrap_or([255, 255, 255])
        };
        let points = recs
            .map(|r| match format {
                0 => point(be_i16(r, 0), be_i16(r, 2), r[6], indexed(r[7])),
                1 => point(be_i16(r, 0), be_i16(r, 2), r[4], indexed(r[5])),
                4 => point(be_i16(r, 0), be_i16(r, 2), r[6], [r[9], r[8], r[7]]),
                _ => point(be_i16(r, 0), be_i16(r, 2), r[4], [r[7], r[6], r[5]]),
            })
            .collect();
        frames.push(IldaFrame { name, company, projector, points });
    }
    if frames.is_empty() {
        bail!("Fichier ILDA sans aucune image");
    }
    Ok(frames)
}

/// Read an ILDA file from disk, refusing oversized files before loading
/// them.
pub fn read_file(path: &Path) -> Result<Vec<IldaFrame>> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("Impossible de lire {}", path.display()))?
        .len();
    if len > MAX_READ_BYTES {
        bail!("Fichier ILDA trop gros (max {} Mo)", MAX_READ_BYTES / (1024 * 1024));
    }
    let data = std::fs::read(path).with_context(|| format!("Impossible de lire {}", path.display()))?;
    read(&data)
}

/// Parse on a background thread so a big file never blocks an HTTP worker
/// or the engine; the result arrives on the returned channel.
pub fn spawn_read(data: Vec<u8>) -> Result<mpsc::Receiver<Result<Vec<IldaFrame>>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ilda-read".into())
        .spawn(move || {
            let _ = tx.send(read(&data));
        })
        .context("Impossible de lancer la lecture ILDA")?;
    Ok(rx)
}

#[allow(clippy::too_many_arguments)] // one per header field
fn header(out: &mut Vec<u8>, format: u8, name: &str, company: &str, records: u16, number: u16, total: u16, projector: u8) {
    let text = |s: &str| {
        let mut b = [0u8; 8];
        for (d, c) in b.iter_mut().zip(s.chars().filter(|c| c.is_ascii() && !c.is_ascii_control())) {
            *d = c as u8;
        }
        b
    };
    out.extend_from_slice(b"ILDA");
    out.extend_from_slice(&[0, 0, 0, format]);
    out.extend_from_slice(&text(name));
    out.extend_from_slice(&text(company));
    out.extend_from_slice(&records.to_be_bytes());
    out.extend_from_slice(&number.to_be_bytes());
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&[projector, 0]);
}

fn to_i16(v: f32) -> i16 {
    if v.is_finite() {
        (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
    } else {
        0
    }
}

fn to_u8(v: f32) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// Write the frames as an ILDA format 5 file (2D, true colour), closed by
/// the 0-record header. Black points are written blanked. An empty frame
/// becomes one blanked point at the centre (0 records would end the file).
pub fn write_format5(frames: &[IldaFrame]) -> Result<Vec<u8>> {
    if frames.is_empty() {
        bail!("Aucune image à exporter");
    }
    if frames.len() > MAX_FRAMES {
        bail!("Trop d'images pour un fichier ILDA ({} > {MAX_FRAMES})", frames.len());
    }
    let total = frames.len() as u16;
    let points: usize = frames.iter().map(|f| f.points.len().max(1)).sum();
    let mut out = Vec::with_capacity(HEADER_LEN * (frames.len() + 1) + points * 8);
    let blank = [Point::lit(0.0, 0.0, 0.0, 0.0, 0.0)];
    for (n, f) in frames.iter().enumerate() {
        if f.points.len() > MAX_POINTS_PER_FRAME {
            bail!("Image {} : trop de points ({} > {MAX_POINTS_PER_FRAME})", n + 1, f.points.len());
        }
        let pts: &[Point] = if f.points.is_empty() { &blank } else { &f.points };
        header(&mut out, 5, &f.name, &f.company, pts.len() as u16, n as u16, total, f.projector);
        for (i, p) in pts.iter().enumerate() {
            let (r, g, b) = (to_u8(p.r), to_u8(p.g), to_u8(p.b));
            let mut status = 0;
            if r == 0 && g == 0 && b == 0 {
                status |= STATUS_BLANK;
            }
            if i + 1 == pts.len() {
                status |= STATUS_LAST;
            }
            out.extend_from_slice(&to_i16(p.x).to_be_bytes());
            out.extend_from_slice(&to_i16(p.y).to_be_bytes());
            out.extend_from_slice(&[status, b, g, r]);
        }
    }
    header(&mut out, 5, "", "", 0, total, total, 0);
    Ok(out)
}

/// ShowNET SD card file name for pattern number `n` (the DMX value on the
/// pattern channel): `001.ild`-`229.ild`. 000 (DMX blackout, yet moves the
/// scanners) and 230-255 (reserved for beams and test patterns) are
/// refused.
pub fn shownet_sd_name(n: u32) -> Result<String> {
    match n {
        1..=229 => Ok(format!("{n:03}.ild")),
        0 => bail!("Le numéro 000 est réservé (blackout DMX) : choisir 001 à 229"),
        230..=255 => bail!("Les numéros 230 à 255 sont réservés par le ShowNET : choisir 001 à 229"),
        _ => bail!("Numéro hors plage : choisir 001 à 229"),
    }
}

/// Refuse a file too big for the ShowNET SD card.
pub fn check_shownet_size(len: usize) -> Result<()> {
    if len > SHOWNET_MAX_FILE_BYTES {
        bail!(
            "Fichier trop gros pour la carte SD du ShowNET ({:.1} Mo > 8 Mo)",
            len as f64 / (1024.0 * 1024.0)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a section by hand (our own bytes, no third-party file).
    fn section(format: u8, projector: u8, records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        header(&mut out, format, "test", "us", records.len() as u16, 0, 1, projector);
        for r in records {
            out.extend_from_slice(r);
        }
        out
    }

    fn end() -> Vec<u8> {
        let mut out = Vec::new();
        header(&mut out, 0, "", "", 0, 0, 0, 0);
        out
    }

    fn xy(x: i16, y: i16) -> Vec<u8> {
        let mut v = x.to_be_bytes().to_vec();
        v.extend_from_slice(&y.to_be_bytes());
        v
    }

    fn sample_frames() -> Vec<IldaFrame> {
        let square = vec![
            Point::lit(-0.5, -0.5, 1.0, 0.0, 0.0),
            Point::lit(0.5, -0.5, 0.0, 1.0, 0.0),
            Point::lit(0.5, 0.5, 0.0, 0.0, 1.0),
            Point::lit(-0.5, 0.5, 0.2, 0.4, 0.6),
            Point::lit(-0.5, -0.5, 0.0, 0.0, 0.0),
        ];
        let line = vec![Point::lit(-1.0, 0.0, 1.0, 1.0, 1.0), Point::lit(1.0, 0.0, 1.0, 1.0, 1.0)];
        let mut b = IldaFrame::new("line", line);
        b.company = "studio".into();
        b.projector = 3;
        vec![IldaFrame::new("square", square), b]
    }

    #[test]
    fn round_trip_is_faithful_to_quantisation() {
        let frames = sample_frames();
        let bytes = write_format5(&frames).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), frames.len());
        for (a, b) in frames.iter().zip(&back) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.company, b.company);
            assert_eq!(a.projector, b.projector);
            assert_eq!(a.points.len(), b.points.len());
            for (p, q) in a.points.iter().zip(&b.points) {
                assert!((p.x - q.x).abs() <= 1.0 / 32767.0, "{p:?} {q:?}");
                assert!((p.y - q.y).abs() <= 1.0 / 32767.0, "{p:?} {q:?}");
                for (c, d) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
                    assert!((c - d).abs() <= 0.5 / 255.0 + 1e-6, "{p:?} {q:?}");
                }
            }
        }
        // Second write is byte-identical: quantisation is stable.
        assert_eq!(write_format5(&back).unwrap(), bytes);
    }

    #[test]
    fn writer_sets_status_bits_and_byte_order() {
        let frames = vec![IldaFrame::new(
            "f",
            vec![Point::lit(1.0, -1.0, 1.0, 0.5, 0.0), Point::lit(0.0, 0.0, 0.0, 0.0, 0.0)],
        )];
        let b = write_format5(&frames).unwrap();
        assert_eq!(&b[0..4], b"ILDA");
        assert_eq!(b[7], 5);
        assert_eq!(be_u16(&b, 24), 2);
        let p0 = &b[32..40];
        assert_eq!(be_i16(p0, 0), 32767);
        assert_eq!(be_i16(p0, 2), -32767);
        assert_eq!(p0[4], 0); // lit, not last
        assert_eq!(&p0[5..8], &[0, 128, 255]); // B, G, R
        let p1 = &b[40..48];
        assert_eq!(p1[4], STATUS_BLANK | STATUS_LAST);
        // Closing header, 0 records.
        assert_eq!(b.len(), 48 + 32);
        assert_eq!(be_u16(&b, 48 + 24), 0);
    }

    #[test]
    fn empty_frame_is_written_as_one_blanked_point() {
        let frames = vec![IldaFrame::new("a", vec![]), IldaFrame::new("b", vec![Point::lit(0.1, 0.1, 1.0, 1.0, 1.0)])];
        let back = read(&write_format5(&frames).unwrap()).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].points, vec![Point::lit(0.0, 0.0, 0.0, 0.0, 0.0)]);
    }

    #[test]
    fn writer_refuses_oversized_frames_and_nothing() {
        assert!(write_format5(&[]).is_err());
        let big = IldaFrame::new("big", vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0); MAX_POINTS_PER_FRAME + 1]);
        assert!(write_format5(&[big]).is_err());
    }

    #[test]
    fn writer_sanitises_names_and_non_finite_values() {
        let mut f = IldaFrame::new("très\nlong nom", vec![Point::lit(f32::NAN, 5.0, f32::INFINITY, -1.0, 2.0)]);
        f.company = "\u{1F600}x".into();
        let back = read(&write_format5(&[f]).unwrap()).unwrap();
        assert_eq!(back[0].name, "trslong");
        assert_eq!(back[0].company, "x");
        let p = back[0].points[0];
        assert_eq!((p.x, p.y), (0.0, 1.0));
        assert_eq!((p.r, p.g, p.b), (0.0, 0.0, 1.0));
    }

    #[test]
    fn format4_and_5_colours_are_per_point_rgb() {
        // Regression of the `ilda` crate bug: B, G, R order, per point.
        let mut r4 = xy(0, 0);
        r4.extend_from_slice(&0i16.to_be_bytes()); // z
        r4.extend_from_slice(&[0, 10, 20, 30]); // status, B, G, R
        let mut r4b = xy(100, -100);
        r4b.extend_from_slice(&500i16.to_be_bytes());
        r4b.extend_from_slice(&[STATUS_LAST, 255, 0, 0]);
        let mut r5 = xy(-32768, 32767);
        r5.extend_from_slice(&[STATUS_LAST, 1, 2, 3]);
        let mut bytes = section(4, 0, &[r4, r4b]);
        bytes.extend(section(5, 0, &[r5]));
        bytes.extend(end());
        let f = read(&bytes).unwrap();
        let c = |p: &Point| ((p.r * 255.0).round() as u8, (p.g * 255.0).round() as u8, (p.b * 255.0).round() as u8);
        assert_eq!(c(&f[0].points[0]), (30, 20, 10));
        assert_eq!(c(&f[0].points[1]), (0, 0, 255));
        assert_eq!(c(&f[1].points[0]), (3, 2, 1));
        assert_eq!((f[1].points[0].x, f[1].points[0].y), (-1.0, 1.0));
    }

    #[test]
    fn blanking_bit_wins_over_colour() {
        let mut r = xy(0, 0);
        r.extend_from_slice(&[STATUS_BLANK, 255, 255, 255]);
        let mut i = xy(0, 0);
        i.extend_from_slice(&[STATUS_BLANK | STATUS_LAST, 56]);
        let mut bytes = section(5, 0, &[r]);
        bytes.extend(section(1, 0, &[i]));
        let f = read(&bytes).unwrap();
        for fr in &f {
            let p = fr.points[0];
            assert_eq!((p.r, p.g, p.b), (0.0, 0.0, 0.0));
        }
    }

    #[test]
    fn default_palette_has_the_spec_gradients() {
        let p = default_palette();
        assert_eq!(p.len(), 64);
        assert_eq!(p[0], [255, 0, 0]);
        assert_eq!(p[16], [255, 255, 0]);
        assert_eq!(p[24], [0, 255, 0]);
        assert_eq!(p[31], [0, 255, 255]);
        assert_eq!(p[40], [0, 0, 255]);
        assert_eq!(p[48], [255, 0, 255]);
        assert_eq!(p[56], [255, 255, 255]);
        assert_eq!(p[63], [255, 32, 32]);
    }

    #[test]
    fn indexed_frames_use_default_then_file_palette_per_projector() {
        let idx = |i: u8| {
            let mut v = xy(0, 0);
            v.extend_from_slice(&0i16.to_be_bytes());
            v.extend_from_slice(&[STATUS_LAST, i]);
            v
        };
        let mut bytes = section(0, 0, &[idx(24)]); // default: green
        bytes.extend(section(2, 0, &[vec![10, 20, 30], vec![40, 50, 60]]));
        bytes.extend(section(0, 0, &[idx(1)])); // file palette
        bytes.extend(section(0, 0, &[idx(40)])); // beyond it: default
        bytes.extend(section(0, 7, &[idx(1)])); // other projector: default
        bytes.extend(end());
        let f = read(&bytes).unwrap();
        assert_eq!(f.len(), 4, "the palette is not an image");
        let rgb = |fr: &IldaFrame| {
            let p = fr.points[0];
            [(p.r * 255.0).round() as u8, (p.g * 255.0).round() as u8, (p.b * 255.0).round() as u8]
        };
        assert_eq!(rgb(&f[0]), [0, 255, 0]);
        assert_eq!(rgb(&f[1]), [40, 50, 60]);
        assert_eq!(rgb(&f[2]), [0, 0, 255]);
        assert_eq!(rgb(&f[3]), default_palette()[1]);
        assert_eq!(f[3].projector, 7);
    }

    #[test]
    fn index_beyond_every_palette_is_white() {
        let mut v = xy(0, 0);
        v.extend_from_slice(&[STATUS_LAST, 200]);
        let f = read(&section(1, 0, &[v])).unwrap();
        let p = f[0].points[0];
        assert_eq!((p.r, p.g, p.b), (1.0, 1.0, 1.0));
    }

    #[test]
    fn missing_end_header_is_accepted_and_trailing_bytes_ignored() {
        let one = write_format5(&sample_frames()[..1]).unwrap();
        let without_end = &one[..one.len() - HEADER_LEN];
        assert_eq!(read(without_end).unwrap().len(), 1);
        let mut with_junk = one.clone();
        with_junk.extend_from_slice(b"garbage after the end");
        assert_eq!(read(&with_junk).unwrap().len(), 1);
    }

    #[test]
    fn truncated_files_are_errors() {
        let bytes = write_format5(&sample_frames()).unwrap();
        let body_end = bytes.len() - HEADER_LEN;
        for cut in 1..body_end {
            // Cutting exactly at a section boundary looks like a file
            // without its end header; every other cut must fail.
            let first_section = HEADER_LEN + 5 * 8;
            if cut == first_section {
                continue;
            }
            assert!(read(&bytes[..cut]).is_err(), "cut at {cut} accepted");
        }
    }

    #[test]
    fn bad_inputs_are_errors() {
        assert!(read(&[]).is_err());
        assert!(read(b"not an ilda file at all, just some text here!!").is_err());
        assert!(read(&end()).is_err(), "no image");
        let mut f3 = section(3, 0, &[vec![1, 2, 3]]);
        f3.extend(end());
        assert!(read(&f3).is_err());
        let mut f9 = section(9, 0, &[vec![1, 2, 3]]);
        f9.extend(end());
        assert!(read(&f9).unwrap_err().to_string().contains("inconnu"));
    }

    #[test]
    fn garbage_never_panics() {
        // Deterministic xorshift: random bytes, random mutations of a valid
        // file, and valid headers followed by noise.
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let valid = write_format5(&sample_frames()).unwrap();
        for _ in 0..3000 {
            let len = (rnd() % 300) as usize;
            let noise: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
            let _ = read(&noise);

            let mut m = valid.clone();
            for _ in 0..(1 + rnd() % 8) {
                let i = (rnd() as usize) % m.len();
                m[i] = rnd() as u8;
            }
            let cut = (rnd() as usize) % (m.len() + 1);
            let _ = read(&m[..cut]);

            let mut h = Vec::new();
            header(&mut h, (rnd() % 6) as u8, "x", "y", rnd() as u16, 0, 1, rnd() as u8);
            h.extend(noise);
            let _ = read(&h);
        }
    }

    #[test]
    fn spawn_read_parses_off_thread() {
        let bytes = write_format5(&sample_frames()).unwrap();
        let rx = spawn_read(bytes).unwrap();
        assert_eq!(rx.recv().unwrap().unwrap().len(), 2);
        let rx = spawn_read(vec![1, 2, 3]).unwrap();
        assert!(rx.recv().unwrap().is_err());
    }

    #[test]
    fn read_file_loads_and_refuses_missing() {
        let dir = std::env::temp_dir().join(format!("ilda-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ours.ild");
        std::fs::write(&path, write_format5(&sample_frames()).unwrap()).unwrap();
        assert_eq!(read_file(&path).unwrap().len(), 2);
        assert!(read_file(&dir.join("missing.ild")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn big_file_parses_quickly() {
        // ~8 MB: the ShowNET limit, well within one parse.
        let pts: Vec<Point> = (0..60_000)
            .map(|i| {
                let a = i as f32 * 0.001;
                Point::lit(a.cos(), a.sin(), 1.0, 0.5, 0.0)
            })
            .collect();
        let frames: Vec<IldaFrame> = (0..17).map(|n| IldaFrame::new(&format!("f{n}"), pts.clone())).collect();
        let bytes = write_format5(&frames).unwrap();
        assert!(check_shownet_size(bytes.len()).is_ok());
        let t = std::time::Instant::now();
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), 17);
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn shownet_names_and_size() {
        assert_eq!(shownet_sd_name(1).unwrap(), "001.ild");
        assert_eq!(shownet_sd_name(42).unwrap(), "042.ild");
        assert_eq!(shownet_sd_name(229).unwrap(), "229.ild");
        for n in [0, 230, 245, 255, 256, 1000] {
            assert!(shownet_sd_name(n).is_err(), "{n} accepted");
        }
        assert!(check_shownet_size(SHOWNET_MAX_FILE_BYTES).is_ok());
        assert!(check_shownet_size(SHOWNET_MAX_FILE_BYTES + 1).is_err());
    }
}
