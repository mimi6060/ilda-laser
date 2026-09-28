//! PNG / JPEG → polylines. Decoding by the `image` crate (MIT OR
//! Apache-2.0, PNG and JPEG only, with size and memory limits); the
//! vectorisation is ours: a small working copy, a threshold (Otsu when
//! automatic), marching squares for outlines, Zhang-Suen thinning for
//! centre lines, Sobel for edges, k-means for colours.

use super::{laser_color, Mode, Options, Path, Pt};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

/// Largest picture decoded (either side), and memory the decoder may use.
const MAX_SIDE: u32 = 8192;
const MAX_ALLOC: u64 = 192 * 1024 * 1024;

/// The working copy: RGB and brightness, `w × h`, composited on white.
struct Work {
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
    luma: Vec<f32>,
}

pub(super) fn vectorize(bytes: &[u8], opts: &Options, warnings: &mut Vec<String>) -> Result<(Vec<Path>, Option<u8>)> {
    let work = decode(bytes, opts.resolution)?;
    let min_area = (opts.min_size as usize).pow(2).max(1);
    let (paths, threshold) = match opts.mode {
        Mode::Contours | Mode::Lines => {
            let t = opts.threshold.unwrap_or_else(|| otsu(&work.luma));
            let mut mask: Vec<bool> = work.luma.iter().map(|&l| l < t as f32).collect();
            // What covers most of the border is the background.
            if border_share(&mask, work.w, work.h) > 0.5 {
                mask.iter_mut().for_each(|m| *m = !*m);
            }
            if opts.invert {
                mask.iter_mut().for_each(|m| *m = !*m);
            }
            clean(&mut mask, work.w, work.h, min_area);
            let polys = if opts.mode == Mode::Contours {
                outlines(&mask, work.w, work.h)
            } else {
                thin(&mut mask, work.w, work.h);
                skeleton(&mask, work.w, work.h)
            };
            (finish(polys, opts, opts.color), Some(t))
        }
        Mode::Edges => {
            let mag = sobel(&work.luma, work.w, work.h);
            let t = opts.threshold.unwrap_or_else(|| otsu(&mag));
            let mut mask: Vec<bool> = mag.iter().map(|&m| m > t as f32).collect();
            if opts.invert {
                mask.iter_mut().for_each(|m| *m = !*m);
            }
            clean(&mut mask, work.w, work.h, min_area);
            thin(&mut mask, work.w, work.h);
            (finish(skeleton(&mask, work.w, work.h), opts, opts.color), Some(t))
        }
        Mode::Colors => {
            let (labels, colors, background) = kmeans(&work, opts.colors as usize + 1);
            let mut paths = Vec::new();
            // Biggest colours first.
            let mut order: Vec<usize> = (0..colors.len()).filter(|&c| c != background).collect();
            order.sort_by_key(|&c| std::cmp::Reverse(labels.iter().filter(|&&l| l == c).count()));
            for c in order {
                let mut mask: Vec<bool> = labels.iter().map(|&l| l == c).collect();
                clean(&mut mask, work.w, work.h, min_area);
                paths.extend(finish(outlines(&mask, work.w, work.h), opts, boost(colors[c], opts.color)));
            }
            (paths, None)
        }
    };
    if paths.len() > 5_000 {
        warnings.push(format!("image très détaillée : {} contours ; augmentez « Taches ignorées » ou baissez la résolution", paths.len()));
    }
    Ok((paths, threshold))
}

fn decode(bytes: &[u8], resolution: u32) -> Result<Work> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| anyhow!("image illisible : {e}"))?;
    if !matches!(reader.format(), Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)) {
        bail!("format d'image non pris en charge : PNG ou JPEG seulement");
    }
    let mut reader = reader;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => anyhow!("image trop grande : {MAX_SIDE} × {MAX_SIDE} pixels au plus"),
        e => anyhow!("image illisible ou abîmée : {e}"),
    })?;
    let img = img.into_rgba8();
    let (sw, sh) = (img.width() as usize, img.height() as usize);
    if sw == 0 || sh == 0 {
        bail!("image vide");
    }
    // Area average down to `resolution` on the longest side.
    let k = (sw.max(sh) as f64 / resolution as f64).max(1.0);
    let (w, h) = (((sw as f64 / k).round() as usize).max(1), ((sh as f64 / k).round() as usize).max(1));
    let mut sum = vec![[0f32; 4]; w * h];
    for (x, y, p) in img.enumerate_pixels() {
        let (tx, ty) = (((x as usize) * w / sw).min(w - 1), ((y as usize) * h / sh).min(h - 1));
        let a = p[3] as f32 / 255.0;
        let s = &mut sum[ty * w + tx];
        for c in 0..3 {
            // On white: a transparent background reads as paper.
            s[c] += p[c] as f32 * a + 255.0 * (1.0 - a);
        }
        s[3] += 1.0;
    }
    let rgb: Vec<[f32; 3]> = sum.iter().map(|s| if s[3] > 0.0 { [s[0] / s[3], s[1] / s[3], s[2] / s[3]] } else { [255.0; 3] }).collect();
    let luma = rgb.iter().map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).collect();
    Ok(Work { w, h, rgb, luma })
}

/// Otsu's threshold on values in 0..=255.
fn otsu(values: &[f32]) -> u8 {
    let mut hist = [0u64; 256];
    for &v in values {
        hist[v.clamp(0.0, 255.0) as usize] += 1;
    }
    let total = values.len() as f64;
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
    let mut scores = [0f64; 256];
    let (mut w0, mut sum0) = (0.0, 0.0);
    for (t, &n) in hist.iter().enumerate() {
        w0 += n as f64;
        sum0 += t as f64 * n as f64;
        let w1 = total - w0;
        if w0 > 0.0 && w1 > 0.0 {
            let (m0, m1) = (sum0 / w0, (sum_all - sum0) / w1);
            scores[t] = w0 * w1 * (m0 - m1).powi(2);
        }
    }
    let best = scores.iter().copied().fold(0.0, f64::max);
    if best <= 0.0 {
        return 128;
    }
    // A flat top (two clean tones): its middle, away from both.
    let top: Vec<usize> = (0..256).filter(|&t| scores[t] >= best * (1.0 - 1e-9)).collect();
    // Values below the threshold are "dark": t + 1 keeps bin t in.
    ((top[0] + top[top.len() - 1]) / 2 + 1).min(255) as u8
}

fn border_share(mask: &[bool], w: usize, h: usize) -> f64 {
    let mut on = 0;
    let mut all = 0;
    for x in 0..w {
        for y in [0, h - 1] {
            all += 1;
            on += mask[y * w + x] as usize;
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            all += 1;
            on += mask[y * w + x] as usize;
        }
    }
    on as f64 / all.max(1) as f64
}

/// Removes specks (shapes smaller than `min_area` pixels) and fills
/// holes as small.
fn clean(mask: &mut [bool], w: usize, h: usize, min_area: usize) {
    if min_area <= 1 {
        return;
    }
    remove_small(mask, w, h, min_area, true);
    remove_small(mask, w, h, min_area, false);
}

/// Flips the 4-connected components of `value` smaller than `min_area`
/// (for holes, only those not touching the border).
fn remove_small(mask: &mut [bool], w: usize, h: usize, min_area: usize, value: bool) {
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    for start in 0..mask.len() {
        if seen[start] || mask[start] != value {
            continue;
        }
        comp.clear();
        stack.push(start);
        seen[start] = true;
        let mut border = false;
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % w, i / w);
            border |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let mut visit = |j: usize| {
                if !seen[j] && mask[j] == value {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if comp.len() < min_area && (value || !border) {
            for &i in &comp {
                mask[i] = !value;
            }
        }
    }
}

/// Outlines of the mask (marching squares, on pixel centres, outside the
/// image counts as empty): closed loops, holes included.
fn outlines(mask: &[bool], w: usize, h: usize) -> Vec<(Vec<Pt>, bool)> {
    let at = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    // Edge midpoints in doubled coordinates, two neighbours each.
    let mut adj: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
    let mut link = |a: (i64, i64), b: (i64, i64)| {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    };
    for j in -1..h as i64 {
        for i in -1..w as i64 {
            let case = (at(i, j) as u8) << 3 | (at(i + 1, j) as u8) << 2 | (at(i + 1, j + 1) as u8) << 1 | at(i, j + 1) as u8;
            let t = (2 * i + 1, 2 * j);
            let r = (2 * i + 2, 2 * j + 1);
            let b = (2 * i + 1, 2 * j + 2);
            let l = (2 * i, 2 * j + 1);
            match case {
                1 | 14 => link(l, b),
                2 | 13 => link(b, r),
                3 | 12 => link(l, r),
                4 | 11 => link(t, r),
                6 | 9 => link(t, b),
                7 | 8 => link(t, l),
                // Diagonal pairs are kept apart (4-connected shapes).
                5 => {
                    link(l, b);
                    link(t, r);
                }
                10 => {
                    link(t, l);
                    link(b, r);
                }
                _ => {}
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut loops = Vec::new();
    for &start in adj.keys() {
        if seen.contains(&start) {
            continue;
        }
        let mut pts = Vec::new();
        let (mut prev, mut cur) = (start, start);
        loop {
            seen.insert(cur);
            pts.push([cur.0 as f64 / 2.0, cur.1 as f64 / 2.0]);
            let Some(next) = adj[&cur].iter().copied().find(|&n| n != prev && !seen.contains(&n)) else { break };
            prev = cur;
            cur = next;
        }
        if pts.len() > 2 {
            loops.push((pts, true));
        }
    }
    loops
}

/// Zhang-Suen thinning, in place: strokes become one pixel wide.
fn thin(mask: &mut [bool], w: usize, h: usize) {
    let get = |m: &[bool], x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize];
    let mut remove = Vec::new();
    for _ in 0..500 {
        let mut changed = false;
        for pass in 0..2 {
            remove.clear();
            for y in 0..h as i64 {
                for x in 0..w as i64 {
                    if !get(mask, x, y) {
                        continue;
                    }
                    // P2..P9 clockwise from north.
                    let n = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)].map(|(dx, dy)| get(mask, x + dx, y + dy));
                    let b = n.iter().filter(|&&v| v).count();
                    let a = (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count();
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let cond = if pass == 0 { !(p2 && p4 && p6) && !(p4 && p6 && p8) } else { !(p2 && p4 && p8) && !(p2 && p6 && p8) };
                    if (2..=6).contains(&b) && a == 1 && cond {
                        remove.push(y as usize * w + x as usize);
                    }
                }
            }
            changed |= !remove.is_empty();
            for &i in &remove {
                mask[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
}

/// A thin mask's lines as polylines: from ends and junctions to the next
/// one, then the remaining loops. A diagonal neighbour only counts when no
/// side neighbour already links to it (so stair steps aren't junctions).
fn skeleton(mask: &[bool], w: usize, h: usize) -> Vec<(Vec<Pt>, bool)> {
    let on = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    let neighbours = |i: usize| -> Vec<usize> {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let mut out = Vec::with_capacity(4);
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            if on(x + dx, y + dy) {
                out.push(((y + dy) as usize) * w + (x + dx) as usize);
            }
        }
        for (dx, dy) in [(1, -1), (1, 1), (-1, 1), (-1, -1)] {
            if on(x + dx, y + dy) && !on(x + dx, y) && !on(x, y + dy) {
                out.push(((y + dy) as usize) * w + (x + dx) as usize);
            }
        }
        out
    };
    let pt = |i: usize| [(i % w) as f64, (i / w) as f64];
    let is_node = |i: usize| mask[i] && neighbours(i).len() != 2;
    let mut visited = vec![false; mask.len()];
    let mut direct = std::collections::BTreeSet::new();
    let mut paths = Vec::new();
    for n in (0..mask.len()).filter(|&i| is_node(i)) {
        for q in neighbours(n) {
            if is_node(q) {
                if direct.insert((n.min(q), n.max(q))) {
                    paths.push((vec![pt(n), pt(q)], false));
                }
                continue;
            }
            if visited[q] {
                continue;
            }
            let mut line = vec![pt(n), pt(q)];
            visited[q] = true;
            let (mut prev, mut cur) = (n, q);
            loop {
                let next = neighbours(cur).into_iter().find(|&k| k != prev && (is_node(k) || !visited[k]));
                let Some(k) = next else { break };
                line.push(pt(k));
                if is_node(k) {
                    break;
                }
                visited[k] = true;
                prev = cur;
                cur = k;
            }
            paths.push((line, false));
        }
    }
    // Loops with no end: every pixel has two neighbours.
    for s in 0..mask.len() {
        if !mask[s] || visited[s] || is_node(s) {
            continue;
        }
        let mut line = vec![pt(s)];
        visited[s] = true;
        let (mut prev, mut cur) = (s, s);
        while let Some(k) = neighbours(cur).into_iter().find(|&k| k != prev && !visited[k]) {
            line.push(pt(k));
            visited[k] = true;
            prev = cur;
            cur = k;
        }
        if line.len() > 2 {
            paths.push((line, true));
        }
    }
    paths
}

/// Gradient magnitude (Sobel on a lightly blurred copy), 0..=255.
fn sobel(luma: &[f32], w: usize, h: usize) -> Vec<f32> {
    let get = |v: &[f32], x: i64, y: i64| v[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize];
    let mut blur = vec![0f32; luma.len()];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut s = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    s += get(luma, x + dx, y + dy);
                }
            }
            blur[y as usize * w + x as usize] = s / 9.0;
        }
    }
    let mut out = vec![0f32; luma.len()];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let g = |dx, dy| get(&blur, x + dx, y + dy);
            let gx = g(1, -1) + 2.0 * g(1, 0) + g(1, 1) - g(-1, -1) - 2.0 * g(-1, 0) - g(-1, 1);
            let gy = g(-1, 1) + 2.0 * g(0, 1) + g(1, 1) - g(-1, -1) - 2.0 * g(0, -1) - g(1, -1);
            // 4 × 255 is the largest step; a sharp edge reads ~255.
            out[y as usize * w + x as usize] = ((gx * gx + gy * gy).sqrt() / 4.0).min(255.0);
        }
    }
    out
}

/// k-means on the colours (deterministic: starts from the border's
/// average, then the farthest colours). Returns each pixel's cluster, the
/// clusters' mean colours and the background cluster (most of the border).
fn kmeans(work: &Work, k: usize) -> (Vec<usize>, Vec<[f32; 3]>, usize) {
    let d2 = |a: &[f32; 3], b: &[f32; 3]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
    let step = (work.rgb.len() / 20_000).max(1);
    let samples: Vec<[f32; 3]> = work.rgb.iter().step_by(step).copied().collect();
    let (w, h) = (work.w, work.h);
    let border: Vec<[f32; 3]> =
        (0..work.rgb.len()).filter(|&i| i % w == 0 || i % w == w - 1 || i / w == 0 || i / w == h - 1).map(|i| work.rgb[i]).collect();
    let mean = |v: &[[f32; 3]]| {
        let n = v.len().max(1) as f32;
        let s = v.iter().fold([0f32; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        [s[0] / n, s[1] / n, s[2] / n]
    };
    let mut centres = vec![mean(&border)];
    while centres.len() < k {
        let far = samples.iter().max_by(|a, b| {
            let da = centres.iter().map(|c| d2(a, c)).fold(f32::MAX, f32::min);
            let db = centres.iter().map(|c| d2(b, c)).fold(f32::MAX, f32::min);
            da.total_cmp(&db)
        });
        match far {
            Some(f) if centres.iter().all(|c| d2(f, c) > 1.0) => centres.push(*f),
            _ => break,
        }
    }
    let nearest = |c: &[f32; 3], centres: &[[f32; 3]]| (0..centres.len()).min_by(|&i, &j| d2(c, &centres[i]).total_cmp(&d2(c, &centres[j]))).unwrap_or(0);
    for _ in 0..15 {
        let mut sums = vec![[0f32; 4]; centres.len()];
        for s in &samples {
            let c = nearest(s, &centres);
            for q in 0..3 {
                sums[c][q] += s[q];
            }
            sums[c][3] += 1.0;
        }
        for (c, s) in centres.iter_mut().zip(&sums) {
            if s[3] > 0.0 {
                *c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    let labels: Vec<usize> = work.rgb.iter().map(|c| nearest(c, &centres)).collect();
    let mut on_border = vec![0usize; centres.len()];
    for (i, &l) in labels.iter().enumerate() {
        if i % w == 0 || i % w == w - 1 || i / w == 0 || i / w == h - 1 {
            on_border[l] += 1;
        }
    }
    let background = (0..centres.len()).max_by_key(|&c| on_border[c]).unwrap_or(0);
    (labels, centres, background)
}

/// A colour at full laser brightness (its hue kept); grey and black
/// become `fallback`.
fn boost(c: [f32; 3], fallback: [u8; 3]) -> [u8; 3] {
    let max = c.iter().copied().fold(0.0, f32::max);
    let min = c.iter().copied().fold(255.0, f32::min);
    if max < 1.0 || max - min < 24.0 {
        return laser_color(if max >= 200.0 { [255, 255, 255] } else { fallback }, fallback);
    }
    let k = 255.0 / max;
    c.map(|v| (v * k).round().clamp(0.0, 255.0) as u8)
}

/// Smoothing (moving average, ends kept) and the colour.
fn finish(polys: Vec<(Vec<Pt>, bool)>, opts: &Options, color: [u8; 3]) -> Vec<Path> {
    let min_len = (opts.min_size as f64).max(1.0);
    polys
        .into_iter()
        .filter_map(|(mut pts, closed)| {
            let n = pts.len();
            for _ in 0..opts.smooth {
                if n < 3 {
                    break;
                }
                let old = pts.clone();
                for i in 0..n {
                    if !closed && (i == 0 || i == n - 1) {
                        continue;
                    }
                    let (a, b) = (old[(i + n - 1) % n], old[(i + 1) % n]);
                    pts[i] = [(a[0] + 2.0 * old[i][0] + b[0]) / 4.0, (a[1] + 2.0 * old[i][1] + b[1]) / 4.0];
                }
            }
            let len: f64 = pts.windows(2).map(|w| super::dist(w[0], w[1])).sum();
            (len >= min_len).then_some(Path { pts, closed, color })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    fn png(img: &RgbImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }
    /// White picture with a black disc and a black square.
    fn logo(w: u32) -> RgbImage {
        RgbImage::from_fn(w, w, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / w as f32);
            let disc = (fx - 0.3).hypot(fy - 0.5) < 0.2;
            let square = (0.6..0.9).contains(&fx) && (0.35..0.65).contains(&fy);
            if disc || square {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        })
    }

    #[test]
    fn outlines_of_a_contrasted_picture() {
        let mut w = Vec::new();
        let (paths, t) = vectorize(&png(&logo(200)), &Options::default(), &mut w).unwrap();
        assert_eq!(paths.len(), 2, "{:?}", paths.iter().map(|p| p.pts.len()).collect::<Vec<_>>());
        assert!(paths.iter().all(|p| p.closed && p.color == [255, 255, 255]));
        assert!(t.is_some_and(|t| t > 1 && t < 255));
        // The disc's outline is round: its points sit on the radius.
        let disc = paths.iter().find(|p| p.pts.iter().all(|q| q[0] < 110.0)).expect("disc");
        for q in &disc.pts {
            let r = (q[0] - 60.0).hypot(q[1] - 100.0);
            assert!((r - 40.0).abs() < 2.5, "{r}");
        }
    }

    #[test]
    fn light_on_dark_and_inverted() {
        let mut img = logo(100);
        img.pixels_mut().for_each(|p| p.0 = p.0.map(|v| 255 - v));
        let (paths, _) = vectorize(&png(&img), &Options::default(), &mut Vec::new()).unwrap();
        assert_eq!(paths.len(), 2, "the dark background is recognised as such");
        // Inverted: the white background becomes the shape, with 2 holes.
        let (paths, _) = vectorize(&png(&img), &Options { invert: true, ..Options::default() }, &mut Vec::new()).unwrap();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn centre_lines_and_edges() {
        // A thick black cross on white: its centre lines are 4 arms from a junction.
        let img = RgbImage::from_fn(120, 120, |x, y| {
            if (55..65).contains(&x) && (10..110).contains(&y) || (55..65).contains(&y) && (10..110).contains(&x) {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        });
        let (paths, _) = vectorize(&png(&img), &Options { mode: Mode::Lines, smooth: 0, ..Options::default() }, &mut Vec::new()).unwrap();
        assert!((2..=6).contains(&paths.len()), "{}", paths.len());
        let total: f64 = paths.iter().map(super::super::length).sum();
        assert!(total > 150.0 && total < 230.0, "{total}");
        // Every centre point stays near the axes of the cross.
        assert!(paths.iter().flat_map(|p| &p.pts).all(|q| (q[0] - 59.5).abs() < 6.0 || (q[1] - 59.5).abs() < 6.0));
        let (edges, _) = vectorize(&png(&img), &Options { mode: Mode::Edges, ..Options::default() }, &mut Vec::new()).unwrap();
        assert!(!edges.is_empty());
    }

    #[test]
    fn colours_are_separated_and_boosted() {
        let img = RgbImage::from_fn(90, 90, |x, _| match x / 30 {
            0 => Rgb([128, 0, 0]),
            1 => Rgb([255, 255, 255]),
            _ => Rgb([0, 0, 200]),
        });
        let mut rgba = RgbaImage::new(120, 90);
        for (x, y, p) in rgba.enumerate_pixels_mut() {
            *p = if x < 90 { let c = img.get_pixel(x, y).0; Rgba([c[0], c[1], c[2], 255]) } else { Rgba([0, 0, 0, 0]) };
        }
        let mut out = std::io::Cursor::new(Vec::new());
        rgba.write_to(&mut out, ImageFormat::Png).unwrap();
        let (paths, _) = vectorize(&out.into_inner(), &Options { mode: Mode::Colors, colors: 2, ..Options::default() }, &mut Vec::new()).unwrap();
        let mut colors: Vec<[u8; 3]> = paths.iter().map(|p| p.color).collect();
        colors.dedup();
        colors.sort();
        assert_eq!(colors, vec![[0, 0, 255], [255, 0, 0]]);
    }

    #[test]
    fn jpeg_specks_and_holes() {
        let mut img = logo(160);
        // Specks smaller than min_size² go.
        for (x, y) in [(5, 5), (150, 20), (80, 150)] {
            img.put_pixel(x, y, Rgb([0, 0, 0]));
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Jpeg).unwrap();
        let (paths, _) = vectorize(&out.into_inner(), &Options::default(), &mut Vec::new()).unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn broken_and_oversized_pictures_are_refused() {
        let good = png(&logo(64));
        let cut = &good[..good.len() / 2];
        assert!(vectorize(cut, &Options::default(), &mut Vec::new()).unwrap_err().to_string().contains("illisible"));
        assert!(vectorize(b"\x89PNG\r\n\x1a\n", &Options::default(), &mut Vec::new()).is_err());
        assert!(vectorize(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0], &Options::default(), &mut Vec::new()).is_err());
        // A PNG header claiming 100 000 × 100 000 pixels: refused before allocating.
        let mut huge = good.clone();
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(vectorize(&huge, &Options::default(), &mut Vec::new()).is_err());
        // A blank page: no outline, no panic.
        let blank = png(&RgbImage::from_pixel(50, 50, Rgb([255, 255, 255])));
        assert!(vectorize(&blank, &Options::default(), &mut Vec::new()).unwrap().0.is_empty());
        let tiny = png(&RgbImage::from_pixel(1, 1, Rgb([0, 0, 0])));
        for mode in [Mode::Contours, Mode::Lines, Mode::Edges, Mode::Colors] {
            vectorize(&tiny, &Options { mode, ..Options::default() }, &mut Vec::new()).unwrap();
        }
    }
}
