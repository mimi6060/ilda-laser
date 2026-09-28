//! Audio files for the timeline (T-161): decode the user's own WAV, MP3,
//! AIFF or FLAC into memory, and compute the waveform overview.
//!
//! The format is recognised from the file's first bytes, not its name.
//! Decoders: `hound` (WAV), `claxon` (FLAC), `nanomp3` (MP3, a pure-Rust
//! port of minimp3) and a small AIFF / AIFF-C reader here. Every decoder
//! returns an error on a damaged file; nothing here may panic (a panic on
//! the HTTP worker would take the API down), so the tests throw garbage,
//! truncated and mutated files at it.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Waveform overview resolution: one min/max pair per this many frames.
pub const PEAK_BLOCK: usize = 256;
/// Longest song accepted (memory: 30 min of 48 kHz stereo is ~350 MB).
pub const MAX_SECONDS: u64 = 30 * 60;

/// A decoded song: interleaved 16-bit samples, one or two channels (a
/// file with more keeps its first two).
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Decoded {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn duration_s(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }

    /// Sample `frame` of channel 0 or 1 (a mono file answers both), as
    /// -1..1; silence outside the song.
    #[inline]
    pub fn sample(&self, frame: i64, channel: usize) -> f32 {
        if frame < 0 {
            return 0.0;
        }
        let ch = self.channels as usize;
        let i = frame as usize * ch + channel.min(ch - 1);
        self.samples.get(i).map_or(0.0, |&s| s as f32 / 32768.0)
    }
}

/// Min/max of every block of `block` frames (all channels together), the
/// waveform the timeline draws. Cached on disk by `media.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveformPeaks {
    pub block: usize,
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    pub sample_rate: u32,
    /// Length of the song in frames.
    pub frames: u64,
}

impl WaveformPeaks {
    pub fn compute(d: &Decoded, block: usize) -> Self {
        let block = block.max(1);
        let ch = d.channels.max(1) as usize;
        let (mut min, mut max) = (Vec::new(), Vec::new());
        for chunk in d.samples.chunks(block * ch) {
            let (lo, hi) = chunk.iter().fold((i16::MAX, i16::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
            min.push(lo as f32 / 32768.0);
            max.push(hi as f32 / 32768.0);
        }
        Self { block, min, max, sample_rate: d.sample_rate, frames: d.frames() as u64 }
    }

    pub fn duration_s(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }

    /// The waveform over show seconds `[from, to[` in `px` columns, the song
    /// starting at show time `offset_s`: (min, max) per column, 0 where
    /// there is no song.
    pub fn view(&self, offset_s: f64, from: f64, to: f64, px: usize) -> (Vec<f32>, Vec<f32>) {
        let px = px.max(1);
        let (mut lo, mut hi) = (vec![0.0; px], vec![0.0; px]);
        let n = self.min.len() as i64;
        if n == 0 || to <= from || to.is_nan() || from.is_nan() {
            return (lo, hi);
        }
        let blocks_per_s = self.sample_rate as f64 / self.block as f64;
        let col = (to - from) / px as f64;
        for i in 0..px {
            let a = ((from + i as f64 * col - offset_s) * blocks_per_s).floor() as i64;
            let b = (((from + (i + 1) as f64 * col - offset_s) * blocks_per_s).ceil() as i64).max(a + 1);
            let (a, b) = (a.clamp(0, n), b.clamp(0, n));
            if a < b {
                lo[i] = self.min[a as usize..b as usize].iter().copied().fold(f32::MAX, f32::min);
                hi[i] = self.max[a as usize..b as usize].iter().copied().fold(f32::MIN, f32::max);
            }
        }
        (lo, hi)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Format {
    Wav,
    Aiff,
    Flac,
    Mp3,
}

/// Skips an ID3v2 tag (MP3s, sometimes FLACs): the offset of what follows.
fn after_id3(bytes: &[u8]) -> usize {
    if bytes.len() >= 10 && &bytes[..3] == b"ID3" {
        let size = bytes[6..10].iter().fold(0usize, |acc, &b| (acc << 7) | (b & 0x7f) as usize);
        let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
        (10 + size + footer).min(bytes.len())
    } else {
        0
    }
}

fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return Some(Format::Wav);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"FORM" && (&bytes[8..12] == b"AIFF" || &bytes[8..12] == b"AIFC") {
        return Some(Format::Aiff);
    }
    let rest = &bytes[after_id3(bytes)..];
    if rest.starts_with(b"fLaC") {
        return Some(Format::Flac);
    }
    // An MPEG audio frame sync (11 bits set), or a tag in front of one.
    if (rest.len() >= 2 && rest[0] == 0xff && rest[1] & 0xe0 == 0xe0) || after_id3(bytes) > 0 {
        return Some(Format::Mp3);
    }
    None
}

/// Decode a whole file. The error says what is wrong in French, for the UI.
pub fn decode(bytes: &[u8]) -> Result<Decoded> {
    if bytes.is_empty() {
        bail!("fichier audio vide");
    }
    let d = match sniff(bytes) {
        Some(Format::Wav) => decode_wav(bytes).context("fichier WAV illisible")?,
        Some(Format::Aiff) => decode_aiff(bytes).context("fichier AIFF illisible")?,
        Some(Format::Flac) => decode_flac(bytes).context("fichier FLAC illisible")?,
        Some(Format::Mp3) => decode_mp3(bytes).context("fichier MP3 illisible")?,
        None => bail!("format audio non reconnu (WAV, MP3, AIFF ou FLAC attendu)"),
    };
    if d.frames() == 0 {
        bail!("le fichier ne contient aucun son");
    }
    Ok(d)
}

/// Collects interleaved samples into at most two channels, 16 bits.
struct Collector {
    sample_rate: u32,
    in_channels: usize,
    channels: u16,
    samples: Vec<i16>,
    index: usize,
    max_frames: u64,
}

impl Collector {
    fn new(sample_rate: u32, in_channels: usize) -> Result<Self> {
        if !(1_000..=384_000).contains(&sample_rate) {
            bail!("fréquence d'échantillonnage invalide : {sample_rate} Hz");
        }
        if in_channels == 0 || in_channels > 32 {
            bail!("nombre de canaux invalide : {in_channels}");
        }
        let channels = in_channels.min(2) as u16;
        Ok(Self { sample_rate, in_channels, channels, samples: Vec::new(), index: 0, max_frames: MAX_SECONDS * sample_rate as u64 })
    }

    /// One sample in -1..1 (clamped).
    fn push(&mut self, v: f32) -> Result<()> {
        let v = if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        self.push_i16((v * 32767.0).round() as i16)
    }

    /// Integer sample of `bits` bits (16-bit samples are kept exactly).
    fn push_int(&mut self, v: i32, bits: u32) -> Result<()> {
        let bits = bits.clamp(1, 32);
        let v = if bits >= 16 { v >> (bits - 16) } else { v << (16 - bits) };
        self.push_i16(v.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
    }

    fn push_i16(&mut self, v: i16) -> Result<()> {
        let ch = self.index % self.in_channels;
        self.index += 1;
        if ch < 2 {
            if ch == 0 && self.samples.len() as u64 / self.channels as u64 >= self.max_frames {
                bail!("morceau trop long ({} min au plus)", MAX_SECONDS / 60);
            }
            self.samples.push(v);
        }
        Ok(())
    }

    fn finish(mut self) -> Decoded {
        // A trailing partial frame is dropped.
        let ch = self.channels as usize;
        self.samples.truncate(self.samples.len() / ch * ch);
        Decoded { sample_rate: self.sample_rate, channels: self.channels, samples: self.samples }
    }
}

fn decode_wav(bytes: &[u8]) -> Result<Decoded> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))?;
    let spec = reader.spec();
    let mut out = Collector::new(spec.sample_rate, spec.channels as usize)?;
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                out.push(s?)?;
            }
        }
        hound::SampleFormat::Int => {
            if !(8..=32).contains(&spec.bits_per_sample) {
                bail!("{} bits par échantillon non gérés", spec.bits_per_sample);
            }
            for s in reader.samples::<i32>() {
                out.push_int(s?, spec.bits_per_sample as u32)?;
            }
        }
    }
    Ok(out.finish())
}

fn decode_flac(bytes: &[u8]) -> Result<Decoded> {
    let start = after_id3(bytes);
    let mut reader = claxon::FlacReader::new(std::io::Cursor::new(&bytes[start..]))?;
    let info = reader.streaminfo();
    let mut out = Collector::new(info.sample_rate, info.channels as usize)?;
    for s in reader.samples() {
        out.push_int(s?, info.bits_per_sample)?;
    }
    Ok(out.finish())
}

fn decode_mp3(bytes: &[u8]) -> Result<Decoded> {
    let mut decoder = nanomp3::Decoder::new();
    let mut pcm = vec![0f32; nanomp3::MAX_SAMPLES_PER_FRAME];
    let mut pos = after_id3(bytes);
    let mut out: Option<Collector> = None;
    // minimp3 wants several frames of look-ahead to lock on reliably.
    const WINDOW: usize = 16 * 1024;
    while pos < bytes.len() {
        let window = &bytes[pos..(pos + WINDOW).min(bytes.len())];
        let (consumed, info) = decoder.decode(window, &mut pcm);
        if consumed == 0 {
            break;
        }
        pos += consumed;
        let Some(info) = info else { continue };
        let ch = info.channels.num() as usize;
        let c = match &mut out {
            Some(c) => c,
            None => out.insert(Collector::new(info.sample_rate, ch)?),
        };
        let n = (info.samples_produced * ch).min(pcm.len());
        if ch == c.in_channels {
            for &v in &pcm[..n] {
                c.push(v)?;
            }
        } else {
            // A frame whose channel count differs from the first one's.
            for frame in pcm[..n].chunks(ch) {
                for k in 0..c.in_channels {
                    c.push(frame[k.min(ch - 1)])?;
                }
            }
        }
    }
    match out {
        Some(c) => Ok(c.finish()),
        None => bail!("aucune trame MP3 lisible"),
    }
}

/// The 80-bit IEEE extended float of an AIFF sample rate.
fn extended_to_f64(b: &[u8]) -> f64 {
    let exp = (((b[0] & 0x7f) as i32) << 8) | b[1] as i32;
    let mantissa = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mantissa == 0 {
        return 0.0;
    }
    let v = mantissa as f64 * 2f64.powi(exp - 16383 - 63);
    if b[0] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

/// AIFF and AIFF-C (uncompressed: `NONE`, `twos`, `sowt` little-endian,
/// `fl32`): big-endian PCM in an `SSND` chunk, described by `COMM`.
fn decode_aiff(bytes: &[u8]) -> Result<Decoded> {
    let aifc = &bytes[8..12] == b"AIFC";
    let mut comm: Option<(usize, u32, f64, [u8; 4])> = None;
    let mut ssnd: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_be_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body = &bytes[pos + 8..(pos + 8).saturating_add(size).min(bytes.len())];
        match id {
            b"COMM" => {
                if body.len() < 18 {
                    bail!("bloc COMM trop court");
                }
                let channels = i16::from_be_bytes([body[0], body[1]]).max(0) as usize;
                let bits = i16::from_be_bytes([body[6], body[7]]).max(0) as u32;
                let rate = extended_to_f64(&body[8..18]);
                let kind = if aifc && body.len() >= 22 { [body[18], body[19], body[20], body[21]] } else { *b"NONE" };
                comm = Some((channels, bits, rate, kind));
            }
            b"SSND" => {
                if body.len() < 8 {
                    bail!("bloc SSND trop court");
                }
                let offset = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                ssnd = Some(&body[(8usize.saturating_add(offset)).min(body.len())..]);
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        pos = pos.saturating_add(8).saturating_add(size).saturating_add(size & 1);
    }
    let (channels, bits, rate, kind) = comm.context("bloc COMM absent")?;
    let data = ssnd.context("bloc SSND absent (pas de son)")?;
    if !rate.is_finite() || rate < 1.0 || rate > 1e6 {
        bail!("fréquence d'échantillonnage invalide");
    }
    let mut out = Collector::new(rate.round() as u32, channels)?;
    match &kind {
        b"NONE" | b"twos" | b"sowt" => {
            if !(1..=32).contains(&bits) {
                bail!("{bits} bits par échantillon non gérés");
            }
            let width = bits.div_ceil(8) as usize;
            let little = &kind == b"sowt";
            for s in data.chunks_exact(width) {
                let mut v: i32 = 0;
                for k in 0..width {
                    let byte = if little { s[width - 1 - k] } else { s[k] };
                    v = (v << 8) | byte as i32;
                }
                // Sign-extend from `width` bytes; samples are left-justified.
                let shift = 32 - 8 * width as u32;
                v = (v << shift) >> shift;
                out.push_int(v, 8 * width as u32)?;
            }
        }
        b"fl32" | b"FL32" => {
            for s in data.as_chunks::<4>().0 {
                out.push(f32::from_be_bytes(*s))?;
            }
        }
        other => bail!("compression AIFF-C non gérée : {}", String::from_utf8_lossy(other)),
    }
    Ok(out.finish())
}

/// Test signals and file writers, shared with the other audio tests.
#[cfg(test)]
pub mod testing {
    use std::f32::consts::TAU;

    /// A sine of `amp` at `hz`, `seconds` long, mono f32.
    pub fn sine(rate: u32, seconds: f64, hz: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize).map(|i| amp * (TAU * hz * i as f32 / rate as f32).sin()).collect()
    }

    /// A 16-bit PCM WAV of interleaved samples.
    pub fn wav16(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        for &s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).unwrap();
        }
        w.finalize().unwrap();
        out.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn wav_with(spec: hound::WavSpec, write: impl FnOnce(&mut hound::WavWriter<&mut std::io::Cursor<Vec<u8>>>)) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        write(&mut w);
        w.finalize().unwrap();
        out.into_inner()
    }

    /// An 80-bit extended float (AIFF sample rates are whole numbers).
    fn extended(rate: u32) -> [u8; 10] {
        let mut e = [0u8; 10];
        let bits = 31 - rate.leading_zeros();
        let exp = 16383 + bits as u16;
        e[..2].copy_from_slice(&exp.to_be_bytes());
        e[2..].copy_from_slice(&((rate as u64) << (63 - bits)).to_be_bytes());
        e
    }

    fn aiff(rate: u32, channels: i16, frames: &[i16], aifc_kind: Option<&[u8; 4]>) -> Vec<u8> {
        let mut comm = Vec::new();
        comm.extend(channels.to_be_bytes());
        comm.extend(((frames.len() / channels as usize) as u32).to_be_bytes());
        comm.extend(16i16.to_be_bytes());
        comm.extend(extended(rate));
        if let Some(kind) = aifc_kind {
            comm.extend(kind);
            comm.extend([0u8, 0]); // empty pascal string (padded)
        }
        let little = aifc_kind == Some(b"sowt");
        let mut data = vec![0u8; 8];
        for s in frames {
            data.extend(if little { s.to_le_bytes() } else { s.to_be_bytes() });
        }
        let mut body = Vec::new();
        body.extend(if aifc_kind.is_some() { *b"AIFC" } else { *b"AIFF" });
        for (id, chunk) in [(b"COMM", &comm), (b"SSND", &data)] {
            body.extend(id);
            body.extend((chunk.len() as u32).to_be_bytes());
            body.extend(chunk);
            if chunk.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut file = b"FORM".to_vec();
        file.extend((body.len() as u32).to_be_bytes());
        file.extend(body);
        file
    }

    /// A FLAC file of VERBATIM subframes (valid, just not compressed), so
    /// the FLAC path is tested without an encoder or a third-party file.
    fn flac_verbatim(rate: u32, channels: u8, frames: &[i16]) -> Vec<u8> {
        struct Bits(Vec<u8>, u32);
        impl Bits {
            fn put(&mut self, v: u64, n: u32) {
                for i in (0..n).rev() {
                    if self.1.is_multiple_of(8) {
                        self.0.push(0);
                    }
                    let last = self.0.last_mut().unwrap();
                    *last |= (((v >> i) & 1) as u8) << (7 - self.1 % 8);
                    self.1 += 1;
                }
            }
        }
        fn crc8(data: &[u8]) -> u8 {
            data.iter().fold(0u8, |mut c, &b| {
                c ^= b;
                for _ in 0..8 {
                    c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
                }
                c
            })
        }
        fn crc16(data: &[u8]) -> u16 {
            data.iter().fold(0u16, |mut c, &b| {
                c ^= (b as u16) << 8;
                for _ in 0..8 {
                    c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
                }
                c
            })
        }
        let ch = channels as usize;
        let total = frames.len() / ch;
        const BLOCK: usize = 4096;
        let mut out = b"fLaC".to_vec();
        // STREAMINFO, last metadata block.
        let mut si = Bits(Vec::new(), 0);
        si.put(BLOCK as u64, 16);
        si.put(BLOCK as u64, 16);
        si.put(0, 24);
        si.put(0, 24);
        si.put(rate as u64, 20);
        si.put(channels as u64 - 1, 3);
        si.put(15, 5);
        si.put(total as u64, 36);
        si.put(0, 64);
        si.put(0, 64);
        out.extend([0x80, 0, 0, 34]);
        out.extend(si.0);
        for (n, block) in frames.chunks(BLOCK * ch).enumerate() {
            let len = block.len() / ch;
            let mut f = Bits(Vec::new(), 0);
            f.put(0b11111111111110, 14);
            f.put(0, 1);
            f.put(0, 1); // fixed block size
            f.put(0b0111, 4); // block size: 16-bit value at the end of the header
            f.put(0b0000, 4); // sample rate from STREAMINFO
            f.put(channels as u64 - 1, 4); // independent channels
            f.put(0b100, 3); // 16 bits per sample
            f.put(0, 1);
            f.put(n as u64, 8); // frame number, UTF-8 coded (< 128 here)
            f.put(len as u64 - 1, 16);
            let crc = crc8(&f.0);
            f.put(crc as u64, 8);
            for c in 0..ch {
                f.put(0, 1);
                f.put(0b000001, 6); // VERBATIM
                f.put(0, 1);
                for i in 0..len {
                    f.put(block[i * ch + c] as u16 as u64, 16);
                }
            }
            let pad = (8 - f.1 % 8) % 8;
            f.put(0, pad);
            let crc = crc16(&f.0);
            f.put(crc as u64, 16);
            out.extend(f.0);
        }
        out
    }

    #[test]
    fn a_generated_ten_second_wav_gives_coherent_peaks() {
        let signal = sine(44_100, 10.0, 440.0, 0.5);
        let d = decode(&wav16(44_100, 1, &signal)).unwrap();
        assert_eq!((d.sample_rate, d.channels, d.frames()), (44_100, 1, 441_000));
        assert!((d.duration_s() - 10.0).abs() < 1e-9);
        let p = WaveformPeaks::compute(&d, PEAK_BLOCK);
        assert_eq!(p.block, 256);
        assert_eq!(p.min.len(), 441_000usize.div_ceil(256));
        let max = p.max.iter().copied().fold(f32::MIN, f32::max);
        let min = p.min.iter().copied().fold(f32::MAX, f32::min);
        assert!((max - 0.5).abs() < 0.01, "max {max}");
        assert!((min + 0.5).abs() < 0.01, "min {min}");
        // A 440 Hz sine fills every 256-sample block (≥ 2.5 periods).
        assert!(p.max.iter().all(|&m| m > 0.45), "every block reaches the crest");
        assert!((p.duration_s() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn stereo_peaks_cover_both_channels_and_silence_is_flat() {
        let mut frames = Vec::new();
        for i in 0..1024 {
            frames.push(0.0);
            frames.push(if i < 512 { 0.8 } else { -0.3 });
        }
        let d = decode(&wav16(48_000, 2, &frames)).unwrap();
        let p = WaveformPeaks::compute(&d, 256);
        assert_eq!(p.max.len(), 4);
        assert!((p.max[0] - 0.8).abs() < 1e-3 && p.min[0].abs() < 1e-3);
        assert!((p.min[3] + 0.3).abs() < 1e-3 && p.max[3].abs() < 1e-3);
        let silent = WaveformPeaks::compute(&decode(&wav16(48_000, 1, &[0.0; 2000])).unwrap(), 256);
        assert!(silent.min.iter().chain(&silent.max).all(|&v| v == 0.0));
    }

    #[test]
    fn the_view_follows_the_offset_and_the_zoom() {
        // 2 s at 25 600 Hz = 100 blocks/s: loud first second, quiet second.
        let rate = 25_600;
        let signal: Vec<f32> = (0..2 * rate).map(|i| if i < rate { 0.9 } else { 0.1 }).collect();
        let p = WaveformPeaks::compute(&decode(&wav16(rate as u32, 1, &signal)).unwrap(), 256);
        let (_, hi) = p.view(0.0, 0.0, 2.0, 2);
        assert!((hi[0] - 0.9).abs() < 1e-3 && (hi[1] - 0.1).abs() < 1e-3, "{hi:?}");
        // Song starting 1 s into the show: nothing before, loud after.
        let (lo, hi) = p.view(1.0, 0.0, 3.0, 3);
        assert_eq!((lo[0], hi[0]), (0.0, 0.0));
        assert!((hi[1] - 0.9).abs() < 1e-3 && (hi[2] - 0.1).abs() < 1e-3, "{hi:?}");
        // Zoomed past the end: silence. More columns than blocks: still filled.
        assert_eq!(p.view(0.0, 5.0, 6.0, 4).1, vec![0.0; 4]);
        assert!(p.view(0.0, 0.0, 0.5, 1000).1.iter().all(|&v| (v - 0.9).abs() < 1e-3));
        assert_eq!(p.view(0.0, 1.0, 1.0, 3).1, vec![0.0; 3], "empty range");
    }

    #[test]
    fn wav_bit_depths_and_float_decode() {
        let spec = |bits, fmt| hound::WavSpec { channels: 1, sample_rate: 22_050, bits_per_sample: bits, sample_format: fmt };
        let f = wav_with(spec(32, hound::SampleFormat::Float), |w| [0.5f32, -0.25].iter().for_each(|&s| w.write_sample(s).unwrap()));
        let i24 = wav_with(spec(24, hound::SampleFormat::Int), |w| [4_194_304i32, -2_097_152].iter().for_each(|&s| w.write_sample(s).unwrap()));
        let i8 = wav_with(spec(8, hound::SampleFormat::Int), |w| [64i8, -32].iter().for_each(|&s| w.write_sample(s).unwrap()));
        for bytes in [f, i24, i8] {
            let d = decode(&bytes).unwrap();
            assert_eq!(d.samples.len(), 2);
            assert!((d.sample(0, 0) - 0.5).abs() < 0.01 && (d.sample(1, 0) + 0.25).abs() < 0.01, "{:?}", d.samples);
        }
    }

    #[test]
    fn more_than_two_channels_keep_the_first_two() {
        let frames = [0.1, 0.2, 0.9, 0.9, 0.3, 0.4, 0.9, 0.9];
        let d = decode(&wav16(48_000, 4, &frames)).unwrap();
        assert_eq!(d.channels, 2);
        assert_eq!(d.frames(), 2);
        assert!((d.sample(1, 1) - 0.4).abs() < 1e-3);
        assert_eq!(d.sample(2, 0), 0.0, "past the end: silence");
        assert_eq!(d.sample(-1, 0), 0.0, "before the start: silence");
    }

    #[test]
    fn aiff_and_aifc_decode() {
        let frames: Vec<i16> = vec![16384, -16384, 8192, -8192];
        for kind in [None, Some(b"NONE"), Some(b"sowt")] {
            let d = decode(&aiff(44_100, 2, &frames, kind)).unwrap();
            assert_eq!((d.sample_rate, d.channels), (44_100, 2), "{kind:?}");
            assert_eq!(d.samples, frames, "{kind:?}");
        }
        let odd = aiff(8_000, 1, &[1000, 2000, 3000], None);
        assert_eq!(decode(&odd).unwrap().samples, vec![1000, 2000, 3000]);
    }

    #[test]
    fn flac_decodes() {
        let signal = sine(44_100, 0.25, 220.0, 0.5);
        let mut frames = Vec::new();
        for &s in &signal {
            let v = (s * 32767.0).round() as i16;
            frames.extend([v, -v]);
        }
        let d = decode(&flac_verbatim(44_100, 2, &frames)).unwrap();
        assert_eq!((d.sample_rate, d.channels), (44_100, 2));
        assert_eq!(d.samples, frames);
        let max = WaveformPeaks::compute(&d, 256).max.iter().copied().fold(f32::MIN, f32::max);
        assert!((max - 0.5).abs() < 0.01);
    }

    /// Silent MPEG-1 Layer III frames (128 kb/s, 44.1 kHz, joint stereo):
    /// a header, then zeros (no side info, no main data) decode to silence.
    fn silent_mp3(frames: usize) -> Vec<u8> {
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
        let mut out = b"ID3\x03\x00\x00\x00\x00\x00\x0a".to_vec(); // a 10-byte tag first
        out.extend([0u8; 10]);
        for _ in 0..frames {
            out.extend(&frame);
        }
        out
    }

    #[test]
    fn mp3_frames_decode_after_an_id3_tag() {
        let d = decode(&silent_mp3(100)).unwrap();
        assert_eq!((d.sample_rate, d.channels), (44_100, 2));
        let frames = d.frames();
        assert!((99 * 1152..=100 * 1152).contains(&frames), "{frames} frames");
        assert!(d.samples.iter().all(|&s| s == 0));
    }

    #[test]
    fn damaged_files_give_a_clear_error_and_never_panic() {
        let good = wav16(44_100, 1, &sine(44_100, 0.2, 440.0, 0.5));
        let err = |bytes: &[u8]| format!("{:#}", decode(bytes).unwrap_err());
        assert_eq!(err(b""), "fichier audio vide");
        assert!(err(b"hello, this is not a song").contains("format audio non reconnu"));
        assert!(err(&good[..30]).contains("WAV illisible"), "{}", err(&good[..30]));
        assert!(err(&good[..44]).contains("aucun son") || err(&good[..44]).contains("illisible"));
        assert!(err(b"fLaC\x00\x00").contains("FLAC illisible"));
        assert!(err(&[0xff, 0xfb, 0x90, 0x64, 1, 2, 3]).contains("MP3 illisible"));
        assert!(err(b"FORMxxxxAIFF").contains("AIFF illisible"));
        let mut no_rate = aiff(44_100, 1, &[1, 2], None);
        no_rate[20 + 8..20 + 18].fill(0); // sample rate 0
        assert!(err(&no_rate).contains("AIFF illisible"));

        // Garbage behind every magic, truncations and random byte flips of
        // valid files: an error or a decode, never a panic.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let flac = flac_verbatim(8_000, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let aif = aiff(8_000, 2, &[1, 2, 3, 4], Some(b"sowt"));
        let mp3 = silent_mp3(4);
        for base in [&good, &flac, &aif, &mp3] {
            for cut in (0..base.len().min(600)).step_by(7) {
                let _ = decode(&base[..cut]);
            }
            for _ in 0..150 {
                let mut bytes = base.clone();
                for _ in 0..1 + rand() % 8 {
                    let i = (rand() as usize) % bytes.len().min(512);
                    bytes[i] = rand() as u8;
                }
                let _ = decode(&bytes);
            }
        }
        for magic in [&b"RIFF\0\0\0\0WAVE"[..], b"FORM\0\0\0\0AIFC", b"fLaC", &[0xff, 0xfb], b"ID3\x03\0\0\0\0\0\0"] {
            for _ in 0..100 {
                let mut bytes = magic.to_vec();
                bytes.extend((0..rand() % 3000).map(|_| rand() as u8));
                let _ = decode(&bytes);
            }
        }
    }
}
