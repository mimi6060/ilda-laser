//! Cue pictures for the console (T-454): our own still of a cue (the grid
//! thumbnail's instant, `presets::thumbnail_points`), drawn as lit
//! segments on black, then encoded as the console asked (PNG, JPEG with
//! the `image` crate, or raw RGB8). Runs on the "citp" thread only.

use super::wire::{FMT_JPEG, FMT_PNG, FMT_RGB8, KEEP_ASPECT};
use crate::engine::Settings;
use crate::patterns::Point;
use image::{ImageEncoder, RgbImage};

/// Smallest side we draw.
pub const MIN_SIDE: u16 = 8;
/// `ThumbnailBufferSize` is a uint16.
pub const MAX_BUFFER: usize = u16::MAX as usize;

/// The picture's size for a request: the size asked, at most `max` a side
/// (and at least `MIN_SIDE`); kept square (our looks are square) when the
/// console asks to keep the aspect ratio.
pub fn fit(width: u16, height: u16, flags: u8, max: u16) -> (u16, u16) {
    let side = |v: u16| if v == 0 { max } else { v.clamp(MIN_SIDE, max.max(MIN_SIDE)) };
    let (w, h) = (side(width), side(height));
    if flags & KEEP_ASPECT != 0 {
        let s = w.min(h);
        (s, s)
    } else {
        (w, h)
    }
}

/// Lit segments of `points` on black, `w × h`. Each segment goes from a
/// point to the next lit one, in the colour of where it arrives (blanked
/// moves stay dark). Colours are lifted so the brightest one is full: a
/// dim look still reads on the console's screen.
pub fn draw(points: &[Point], w: u16, h: u16) -> RgbImage {
    let (w, h) = (w.max(1) as u32, h.max(1) as u32);
    let mut img = RgbImage::new(w, h);
    let peak = points.iter().map(|p| p.r.max(p.g).max(p.b)).fold(0.0f32, f32::max);
    if peak.is_nan() || peak <= 0.0 {
        return img;
    }
    let gain = 1.0 / peak;
    let px = |p: &Point| ((p.x.clamp(-1.0, 1.0) + 1.0) * 0.5 * (w - 1) as f32, (1.0 - (p.y.clamp(-1.0, 1.0) + 1.0) * 0.5) * (h - 1) as f32);
    let pen = (w.min(h) / 96).max(1) as i32;
    let mut put = |x: f32, y: f32, c: [u8; 3]| {
        let (cx, cy) = (x.round() as i32, y.round() as i32);
        for dy in 0..pen {
            for dx in 0..pen {
                let (x, y) = (cx + dx - pen / 2, cy + dy - pen / 2);
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    let p = img.get_pixel_mut(x as u32, y as u32);
                    for (v, &c) in p.0.iter_mut().zip(&c) {
                        *v = (*v).max(c);
                    }
                }
            }
        }
    };
    let col = |v: f32| ((v * gain).clamp(0.0, 1.0) * 255.0).round() as u8;
    for (i, p) in points.iter().enumerate() {
        if !p.is_lit() {
            continue;
        }
        let c = [col(p.r), col(p.g), col(p.b)];
        let (x1, y1) = px(p);
        let (x0, y0) = if i > 0 { px(&points[i - 1]) } else { (x1, y1) };
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            put(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, c);
        }
    }
    img
}

/// `img` in `format`; `bgr`: RGB8 in MSEX 1.0's byte order. None: an
/// unknown format or an encoder error.
pub fn encode(img: &RgbImage, format: [u8; 4], bgr: bool) -> Option<Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    match format {
        FMT_RGB8 => {
            out = img.as_raw().clone();
            if bgr {
                out.chunks_mut(3).for_each(|c| c.swap(0, 2));
            }
        }
        FMT_PNG => image::codecs::png::PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8).ok()?,
        FMT_JPEG => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8).ok()?,
        _ => return None,
    }
    Some(out)
}

pub fn supported(format: [u8; 4]) -> bool {
    matches!(format, FMT_PNG | FMT_JPEG | FMT_RGB8)
}

/// A cue's picture: drawn at the size asked, made smaller until it fits
/// the 16-bit buffer. Returns the size drawn and the bytes.
pub fn render(settings: &Settings, format: [u8; 4], w: u16, h: u16, bgr: bool) -> Option<(u16, u16, Vec<u8>)> {
    let points = crate::presets::thumbnail_points(settings);
    render_points(&points, format, w, h, bgr)
}

pub fn render_points(points: &[Point], format: [u8; 4], mut w: u16, mut h: u16, bgr: bool) -> Option<(u16, u16, Vec<u8>)> {
    if !supported(format) {
        return None;
    }
    loop {
        let bytes = encode(&draw(points, w, h), format, bgr)?;
        if bytes.len() <= MAX_BUFFER {
            return Some((w, h, bytes));
        }
        if w <= MIN_SIDE && h <= MIN_SIDE {
            return None;
        }
        w = (w as u32 * 4 / 5).max(MIN_SIDE as u32) as u16;
        h = (h as u32 * 4 / 5).max(MIN_SIDE as u32) as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_cue_gives_a_lit_picture_of_the_size_asked() {
        let cue = crate::presets::catalog().into_iter().next().unwrap();
        for f in [FMT_PNG, FMT_JPEG, FMT_RGB8] {
            let (w, h, bytes) = render(&cue.settings, f, 128, 128, false).unwrap();
            assert_eq!((w, h), (128, 128));
            let img = match f {
                FMT_RGB8 => RgbImage::from_raw(128, 128, bytes).unwrap(),
                _ => image::load_from_memory(&bytes).unwrap().to_rgb8(),
            };
            assert_eq!(img.dimensions(), (128, 128));
            assert!(img.pixels().filter(|p| p.0.iter().any(|&c| c > 40)).count() > 50, "{}: a dark picture", cue.id);
        }
    }

    #[test]
    fn sizes_are_bounded_and_the_buffer_fits_16_bits() {
        assert_eq!(fit(1000, 50, 0, 128), (128, 50));
        assert_eq!(fit(1000, 50, KEEP_ASPECT, 128), (50, 50));
        assert_eq!(fit(1, 0, 0, 128), (MIN_SIDE, 128));
        // 256 × 256 RGB8 is 196 kB: drawn smaller.
        let pts = vec![Point { x: -1.0, y: -1.0, r: 1.0, g: 1.0, b: 1.0 }, Point { x: 1.0, y: 1.0, r: 1.0, g: 0.0, b: 0.0 }];
        let (w, h, b) = render_points(&pts, FMT_RGB8, 256, 256, false).unwrap();
        assert!(b.len() <= MAX_BUFFER && w < 256 && b.len() == w as usize * h as usize * 3);
        assert!(render_points(&pts, *b"GIF8", 64, 64, false).is_none());
    }

    #[test]
    fn rgb8_in_msex_1_0_is_bgr() {
        let pts = vec![Point { x: 0.0, y: 0.0, r: 1.0, g: 0.0, b: 0.0 }];
        let (_, _, rgb) = render_points(&pts, FMT_RGB8, 8, 8, false).unwrap();
        let (_, _, bgr) = render_points(&pts, FMT_RGB8, 8, 8, true).unwrap();
        let lit = rgb.chunks(3).position(|c| c[0] == 255).unwrap();
        assert_eq!(&bgr[lit * 3..lit * 3 + 3], &[0, 0, 255]);
    }
}
