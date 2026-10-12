//! CITP wire format (T-454): the messages a media server needs to give a
//! lighting console the names and thumbnails of its libraries.
//!
//! Written from the public specification only: « CITP protocol suite
//! specification », revision of 2012-08-20 (MSEX 1.2), published on
//! citp-protocol.org (`download/file.php?id=13`). Sections 2.1 (little
//! endian, 1-byte packing, null-terminated ucs1 / ucs2 strings), 3.1.1 (the
//! CITP header), 4 (PINF) and 10 (MSEX). No other implementation was read.
//!
//! Every decoder is bounds-checked: a short, oversized or inconsistent
//! message is an error, never a panic.

use std::net::Ipv4Addr;

pub const COOKIE: [u8; 4] = *b"CITP";
/// The CITP header: cookie, version, request index, size, parts, type.
pub const HEADER_LEN: usize = 20;
/// PINF/PLoc goes to this group and port (§4).
pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 180);
pub const MULTICAST_PORT: u16 = 4809;
/// Library type 1: « Media (images & video) » (§10.4).
pub const MEDIA: u8 = 1;
/// Longest name we send (characters); longer ones are cut.
pub const MAX_NAME: usize = 64;
/// Longest string we read (bytes or code units).
const MAX_STRING: usize = 1024;

pub const FMT_RGB8: [u8; 4] = *b"RGB8";
pub const FMT_JPEG: [u8; 4] = *b"JPEG";
pub const FMT_PNG: [u8; 4] = *b"PNG ";

/// ELUp flags (§10.4.3).
pub const UP_ELEMENTS_UPDATED: u8 = 0x01;
pub const UP_ELEMENTS_ADDED_REMOVED: u8 = 0x02;
pub const UP_LIBRARIES_ADDED_REMOVED: u8 = 0x08;
pub const UP_ALL_ELEMENTS: u8 = 0x10;
/// GETh / GELT flag: use width and height as a maximum.
pub const KEEP_ASPECT: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    V1_0,
    V1_1,
    V1_2,
}

impl Version {
    pub const ALL: [Version; 3] = [Version::V1_0, Version::V1_1, Version::V1_2];

    pub fn pair(self) -> (u8, u8) {
        match self {
            Version::V1_0 => (1, 0),
            Version::V1_1 => (1, 1),
            Version::V1_2 => (1, 2),
        }
    }

    pub fn from_pair(major: u8, minor: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.pair() == (major, minor))
    }

    /// CInf / SInf: MSB = major, LSB = minor.
    pub fn word(self) -> u16 {
        let (a, b) = self.pair();
        (a as u16) << 8 | b as u16
    }

    pub fn from_word(w: u16) -> Option<Self> {
        Self::from_pair((w >> 8) as u8, w as u8)
    }

    /// Counts are uint16 from 1.2, uint8 before.
    fn wide(self) -> bool {
        self >= Version::V1_2
    }
}

/// MSEXLibraryId: level, then one index per level (§10.4).
pub type LibraryId = [u8; 4];

/// The library id of library number `n` at the first level.
pub fn library_id(n: u8) -> LibraryId {
    [1, n, 0, 0]
}

/// The root, parent of the first-level libraries.
pub const ROOT: LibraryId = [0; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub major: u8,
    pub minor: u8,
    /// RequestIndex in a request, InResponseTo in a response; 0: none.
    pub index: u16,
    pub size: u32,
    pub part_count: u16,
    pub part: u16,
    pub content: [u8; 4],
}

impl Header {
    /// The header at the start of `buf` (cookie checked, size not).
    pub fn parse(buf: &[u8]) -> Result<Header, WireError> {
        if buf.len() < HEADER_LEN {
            return Err(WireError::Short);
        }
        if buf[0..4] != COOKIE {
            return Err(WireError::Cookie);
        }
        Ok(Header {
            major: buf[4],
            minor: buf[5],
            index: u16::from_le_bytes([buf[6], buf[7]]),
            size: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            part_count: u16::from_le_bytes([buf[12], buf[13]]),
            part: u16::from_le_bytes([buf[14], buf[15]]),
            content: [buf[16], buf[17], buf[18], buf[19]],
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    Short,
    Cookie,
    Size,
    Fragmented,
    Version,
    Bad(&'static str),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Short => write!(f, "message tronqué"),
            WireError::Cookie => write!(f, "pas un message CITP"),
            WireError::Size => write!(f, "taille incohérente"),
            WireError::Fragmented => write!(f, "message fragmenté"),
            WireError::Version => write!(f, "version MSEX inconnue"),
            WireError::Bad(why) => write!(f, "{why}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryInfo {
    pub id: LibraryId,
    pub serial: u32,
    pub dmx: (u8, u8),
    pub name: String,
    pub sub_libraries: u16,
    pub elements: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    pub number: u8,
    pub serial: u32,
    pub dmx: (u8, u8),
    pub name: String,
    pub stamp: u64,
    pub width: u16,
    pub height: u16,
    pub length: u32,
    pub fps: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    /// 36 characters (1.2 only).
    pub uuid: String,
    pub product: String,
    pub version: (u8, u8, u8),
    pub versions: Vec<u16>,
    pub library_types: u16,
    pub thumb_formats: Vec<[u8; 4]>,
    pub stream_formats: Vec<[u8; 4]>,
    pub layers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbRequest {
    pub format: [u8; 4],
    pub width: u16,
    pub height: u16,
    pub flags: u8,
    pub library_type: u8,
}

/// The MSEX messages we read or write. `None` in a list: « all ».
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msex {
    CInf { versions: Vec<u16> },
    SInf(ServerInfo),
    Nack { received: [u8; 4] },
    /// Layer status with no layer: we play no video layers.
    LSta,
    Geli { library_type: u8, parent: LibraryId, numbers: Option<Vec<u8>> },
    Elin { library_type: u8, libraries: Vec<LibraryInfo> },
    Gein { library_type: u8, library: LibraryId, elements: Option<Vec<u8>> },
    Mein { library: LibraryId, elements: Vec<MediaInfo> },
    Gelt { req: ThumbRequest, libraries: Option<Vec<LibraryId>> },
    Elth { library_type: u8, library: LibraryId, format: [u8; 4], width: u16, height: u16, data: Vec<u8> },
    Geth { req: ThumbRequest, library: LibraryId, elements: Option<Vec<u8>> },
    Ethn { library_type: u8, library: LibraryId, element: u8, format: [u8; 4], width: u16, height: u16, data: Vec<u8> },
    Elup { library_type: u8, library: LibraryId, flags: u8, elements: [u8; 32], libraries: [u8; 32] },
    /// A well-formed MSEX header with a type we don't handle.
    Other { content: [u8; 4] },
}

impl Msex {
    pub fn cookie(&self) -> [u8; 4] {
        match self {
            Msex::CInf { .. } => *b"CInf",
            Msex::SInf(_) => *b"SInf",
            Msex::Nack { .. } => *b"Nack",
            Msex::LSta => *b"LSta",
            Msex::Geli { .. } => *b"GELI",
            Msex::Elin { .. } => *b"ELIn",
            Msex::Gein { .. } => *b"GEIn",
            Msex::Mein { .. } => *b"MEIn",
            Msex::Gelt { .. } => *b"GELT",
            Msex::Elth { .. } => *b"ELTh",
            Msex::Geth { .. } => *b"GETh",
            Msex::Ethn { .. } => *b"EThn",
            Msex::Elup { .. } => *b"ELUp",
            Msex::Other { content } => *content,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    PNam { name: String },
    PLoc { port: u16, kind: String, name: String, state: String },
    Msex { version: Version, body: Msex },
    /// Another layer (SDMX, FPTC, FSEL, FINF, OMEX…) or PINF type.
    Other { layer: [u8; 4], content: [u8; 4] },
}

// ---- writing ----

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    /// ucs1: ASCII, null-terminated; anything else becomes '?'.
    fn ucs1(&mut self, s: &str) {
        for c in s.chars().take(MAX_STRING - 1) {
            self.0.push(if c.is_ascii() && c != '\0' { c as u8 } else { b'?' });
        }
        self.0.push(0);
    }
    /// ucs2: UTF-16LE, null-terminated.
    fn ucs2(&mut self, s: &str) {
        for u in s.chars().filter(|&c| c != '\0').take(MAX_STRING - 1).collect::<String>().encode_utf16() {
            self.u16(u);
        }
        self.u16(0);
    }
    /// A count: uint16 in 1.2, uint8 before.
    fn count(&mut self, v: Version, n: usize) {
        if v.wide() {
            self.u16(n.min(u16::MAX as usize) as u16);
        } else {
            self.u8(n.min(255) as u8);
        }
    }
    /// A library: its number in 1.0, its id after.
    fn library(&mut self, v: Version, id: LibraryId) {
        if v == Version::V1_0 {
            self.u8(id[1]);
        } else {
            self.bytes(&id);
        }
    }
    fn thumb_req(&mut self, r: &ThumbRequest) {
        self.bytes(&r.format);
        self.u16(r.width);
        self.u16(r.height);
        self.u8(r.flags);
        self.u8(r.library_type);
    }
}

/// Cuts a name to `MAX_NAME` characters.
pub fn short_name(s: &str) -> String {
    s.chars().take(MAX_NAME).collect()
}

fn wrap(layer: [u8; 4], index: u16, body: Vec<u8>) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(HEADER_LEN + body.len()));
    w.bytes(&COOKIE);
    w.u8(1);
    w.u8(0);
    w.u16(index);
    w.u32((HEADER_LEN + body.len()) as u32);
    w.u16(1);
    w.u16(0);
    w.bytes(&layer);
    w.bytes(&body);
    w.0
}

/// The whole message, header included. `index` is the RequestIndex of a
/// request or the InResponseTo of a response (0: none).
pub fn encode(msg: &Message, index: u16) -> Vec<u8> {
    let mut w = Writer::default();
    let layer = match msg {
        Message::PNam { name } => {
            w.bytes(b"PNam");
            w.ucs1(name);
            *b"PINF"
        }
        Message::PLoc { port, kind, name, state } => {
            w.bytes(b"PLoc");
            w.u16(*port);
            w.ucs1(kind);
            w.ucs1(name);
            w.ucs1(state);
            *b"PINF"
        }
        Message::Msex { version, body } => {
            let (a, b) = version.pair();
            w.u8(a);
            w.u8(b);
            w.bytes(&body.cookie());
            msex_body(&mut w, *version, body);
            *b"MSEX"
        }
        Message::Other { layer, content } => {
            w.bytes(content);
            *layer
        }
    };
    wrap(layer, index, w.0)
}

fn opt_list<T>(w: &mut Writer, v: Version, list: &Option<Vec<T>>, mut each: impl FnMut(&mut Writer, &T)) {
    match list {
        None => w.count(v, 0),
        Some(l) => {
            w.count(v, l.len());
            l.iter().for_each(|x| each(w, x));
        }
    }
}

fn msex_body(w: &mut Writer, v: Version, body: &Msex) {
    match body {
        Msex::CInf { versions } => {
            w.u8(versions.len().min(255) as u8);
            versions.iter().take(255).for_each(|x| w.u16(*x));
        }
        Msex::SInf(i) => {
            if v.wide() {
                let mut uuid: Vec<u8> = i.uuid.bytes().take(36).collect();
                uuid.resize(36, b'0');
                w.bytes(&uuid);
                w.ucs2(&i.product);
                w.u8(i.version.0);
                w.u8(i.version.1);
                w.u8(i.version.2);
                w.u8(i.versions.len() as u8);
                i.versions.iter().for_each(|x| w.u16(*x));
                w.u16(i.library_types);
                w.u8(i.thumb_formats.len() as u8);
                i.thumb_formats.iter().for_each(|f| w.bytes(f));
                w.u8(i.stream_formats.len() as u8);
                i.stream_formats.iter().for_each(|f| w.bytes(f));
            } else {
                w.ucs2(&i.product);
                w.u8(i.version.0);
                w.u8(i.version.1);
            }
            w.u8(i.layers.len() as u8);
            i.layers.iter().for_each(|l| w.ucs1(l));
        }
        Msex::Nack { received } => w.bytes(received),
        Msex::LSta => w.u8(0),
        Msex::Geli { library_type, parent, numbers } => {
            w.u8(*library_type);
            if v != Version::V1_0 {
                w.bytes(parent);
            }
            opt_list(w, v, numbers, |w, n| w.u8(*n));
        }
        Msex::Elin { library_type, libraries } => {
            w.u8(*library_type);
            w.count(v, libraries.len());
            for l in libraries {
                w.library(v, l.id);
                if v.wide() {
                    w.u32(l.serial);
                }
                w.u8(l.dmx.0);
                w.u8(l.dmx.1);
                w.ucs2(&l.name);
                if v != Version::V1_0 {
                    w.count(v, l.sub_libraries as usize);
                }
                w.count(v, l.elements as usize);
            }
        }
        Msex::Gein { library_type, library, elements } => {
            w.u8(*library_type);
            w.library(v, *library);
            opt_list(w, v, elements, |w, n| w.u8(*n));
        }
        Msex::Mein { library, elements } => {
            w.library(v, *library);
            w.count(v, elements.len());
            for e in elements {
                w.u8(e.number);
                if v.wide() {
                    w.u32(e.serial);
                }
                w.u8(e.dmx.0);
                w.u8(e.dmx.1);
                w.ucs2(&e.name);
                w.u64(e.stamp);
                w.u16(e.width);
                w.u16(e.height);
                w.u32(e.length);
                w.u8(e.fps);
            }
        }
        Msex::Gelt { req, libraries } => {
            w.thumb_req(req);
            opt_list(w, v, libraries, |w, id| w.library(v, *id));
        }
        Msex::Elth { library_type, library, format, width, height, data } => {
            w.u8(*library_type);
            w.library(v, *library);
            w.bytes(format);
            w.u16(*width);
            w.u16(*height);
            w.u16(data.len().min(u16::MAX as usize) as u16);
            w.bytes(&data[..data.len().min(u16::MAX as usize)]);
        }
        Msex::Geth { req, library, elements } => {
            w.thumb_req(req);
            w.library(v, *library);
            opt_list(w, v, elements, |w, n| w.u8(*n));
        }
        Msex::Ethn { library_type, library, element, format, width, height, data } => {
            w.u8(*library_type);
            w.library(v, *library);
            w.u8(*element);
            w.bytes(format);
            w.u16(*width);
            w.u16(*height);
            w.u16(data.len().min(u16::MAX as usize) as u16);
            w.bytes(&data[..data.len().min(u16::MAX as usize)]);
        }
        Msex::Elup { library_type, library, flags, elements, libraries } => {
            w.u8(*library_type);
            w.library(v, *library);
            w.u8(*flags);
            if v.wide() {
                w.bytes(elements);
                w.bytes(libraries);
            }
        }
        Msex::Other { .. } => {}
    }
}

// ---- reading ----

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or(WireError::Short)?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().map_err(|_| WireError::Short)?))
    }
    fn four(&mut self) -> Result<[u8; 4], WireError> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
    fn ucs1(&mut self) -> Result<String, WireError> {
        let rest = &self.buf[self.at..];
        let end = rest.iter().take(MAX_STRING).position(|&b| b == 0).ok_or(WireError::Bad("chaîne sans fin"))?;
        let s = rest[..end].iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect();
        self.at += end + 1;
        Ok(s)
    }
    fn ucs2(&mut self) -> Result<String, WireError> {
        let mut units = Vec::new();
        loop {
            let u = self.u16()?;
            if u == 0 {
                break;
            }
            if units.len() >= MAX_STRING {
                return Err(WireError::Bad("chaîne trop longue"));
            }
            units.push(u);
        }
        Ok(String::from_utf16_lossy(&units))
    }
    fn count(&mut self, v: Version) -> Result<usize, WireError> {
        Ok(if v.wide() { self.u16()? as usize } else { self.u8()? as usize })
    }
    fn library(&mut self, v: Version) -> Result<LibraryId, WireError> {
        if v == Version::V1_0 {
            Ok(library_id(self.u8()?))
        } else {
            self.four()
        }
    }
    /// A list of `count` items, where 0 means « all ».
    fn opt_list<T>(&mut self, v: Version, mut each: impl FnMut(&mut Self) -> Result<T, WireError>) -> Result<Option<Vec<T>>, WireError> {
        let n = self.count(v)?;
        if n == 0 {
            return Ok(None);
        }
        if n > 256 {
            return Err(WireError::Bad("plus de 256 éléments"));
        }
        (0..n).map(|_| each(self)).collect::<Result<Vec<_>, _>>().map(Some)
    }
    fn list<T>(&mut self, n: usize, mut each: impl FnMut(&mut Self) -> Result<T, WireError>) -> Result<Vec<T>, WireError> {
        if n > 256 {
            return Err(WireError::Bad("plus de 256 éléments"));
        }
        (0..n).map(|_| each(self)).collect()
    }
    fn thumb_req(&mut self) -> Result<ThumbRequest, WireError> {
        Ok(ThumbRequest { format: self.four()?, width: self.u16()?, height: self.u16()?, flags: self.u8()?, library_type: self.u8()? })
    }
    fn buffer(&mut self) -> Result<Vec<u8>, WireError> {
        let n = self.u16()? as usize;
        Ok(self.take(n)?.to_vec())
    }
}

/// One whole message (exactly `MessageSize` bytes) → its header and
/// contents. Trailing bytes after a known body are allowed (CInf's
/// « FutureMessageData »; the spec lets messages grow).
pub fn decode(buf: &[u8]) -> Result<(Header, Message), WireError> {
    let h = Header::parse(buf)?;
    if h.size as usize != buf.len() {
        return Err(WireError::Size);
    }
    if h.part_count != 1 || h.part != 0 {
        return Err(WireError::Fragmented);
    }
    let mut r = Reader { buf, at: HEADER_LEN };
    let msg = match &h.content {
        b"PINF" => match &r.four()? {
            b"PNam" => Message::PNam { name: r.ucs1()? },
            b"PLoc" => Message::PLoc { port: r.u16()?, kind: r.ucs1()?, name: r.ucs1()?, state: r.ucs1()? },
            other => Message::Other { layer: h.content, content: *other },
        },
        b"MSEX" => {
            let (a, b) = (r.u8()?, r.u8()?);
            let content = r.four()?;
            let version = Version::from_pair(a, b).ok_or(WireError::Version)?;
            Message::Msex { version, body: msex(&mut r, version, content)? }
        }
        _ => Message::Other { layer: h.content, content: r.four().unwrap_or([0; 4]) },
    };
    Ok((h, msg))
}

fn msex(r: &mut Reader, v: Version, content: [u8; 4]) -> Result<Msex, WireError> {
    Ok(match &content {
        b"CInf" => {
            let n = r.u8()? as usize;
            Msex::CInf { versions: (0..n).map(|_| r.u16()).collect::<Result<_, _>>()? }
        }
        b"SInf" => {
            if v.wide() {
                let uuid = String::from_utf8_lossy(r.take(36)?).into_owned();
                let product = r.ucs2()?;
                let version = (r.u8()?, r.u8()?, r.u8()?);
                let n = r.u8()? as usize;
                let versions = (0..n).map(|_| r.u16()).collect::<Result<_, _>>()?;
                let library_types = r.u16()?;
                let n = r.u8()? as usize;
                let thumb_formats = (0..n).map(|_| r.four()).collect::<Result<_, _>>()?;
                let n = r.u8()? as usize;
                let stream_formats = (0..n).map(|_| r.four()).collect::<Result<_, _>>()?;
                let n = r.u8()? as usize;
                let layers = (0..n).map(|_| r.ucs1()).collect::<Result<_, _>>()?;
                Msex::SInf(ServerInfo { uuid, product, version, versions, library_types, thumb_formats, stream_formats, layers })
            } else {
                let product = r.ucs2()?;
                let version = (r.u8()?, r.u8()?, 0);
                let n = r.u8()? as usize;
                let layers = (0..n).map(|_| r.ucs1()).collect::<Result<_, _>>()?;
                Msex::SInf(ServerInfo { uuid: String::new(), product, version, versions: Vec::new(), library_types: 0, thumb_formats: Vec::new(), stream_formats: Vec::new(), layers })
            }
        }
        b"Nack" => Msex::Nack { received: r.four()? },
        b"LSta" => {
            if r.u8()? != 0 {
                return Err(WireError::Bad("couches non lues"));
            }
            Msex::LSta
        }
        b"GELI" => {
            let library_type = r.u8()?;
            let parent = if v == Version::V1_0 { ROOT } else { r.four()? };
            Msex::Geli { library_type, parent, numbers: r.opt_list(v, |r| r.u8())? }
        }
        b"ELIn" => {
            let library_type = r.u8()?;
            let n = r.count(v)?;
            let libraries = r.list(n, |r| {
                let id = r.library(v)?;
                let serial = if v.wide() { r.u32()? } else { 0 };
                let dmx = (r.u8()?, r.u8()?);
                let name = r.ucs2()?;
                let sub_libraries = if v == Version::V1_0 { 0 } else { r.count(v)? as u16 };
                let elements = r.count(v)? as u16;
                Ok(LibraryInfo { id, serial, dmx, name, sub_libraries, elements })
            })?;
            Msex::Elin { library_type, libraries }
        }
        b"GEIn" => {
            let library_type = r.u8()?;
            let library = r.library(v)?;
            Msex::Gein { library_type, library, elements: r.opt_list(v, |r| r.u8())? }
        }
        b"MEIn" => {
            let library = r.library(v)?;
            let n = r.count(v)?;
            let elements = r.list(n, |r| {
                let number = r.u8()?;
                let serial = if v.wide() { r.u32()? } else { 0 };
                Ok(MediaInfo {
                    number,
                    serial,
                    dmx: (r.u8()?, r.u8()?),
                    name: r.ucs2()?,
                    stamp: r.u64()?,
                    width: r.u16()?,
                    height: r.u16()?,
                    length: r.u32()?,
                    fps: r.u8()?,
                })
            })?;
            Msex::Mein { library, elements }
        }
        b"GELT" => {
            let req = r.thumb_req()?;
            Msex::Gelt { req, libraries: r.opt_list(v, |r| r.library(v))? }
        }
        b"ELTh" => Msex::Elth { library_type: r.u8()?, library: r.library(v)?, format: r.four()?, width: r.u16()?, height: r.u16()?, data: r.buffer()? },
        b"GETh" => {
            let req = r.thumb_req()?;
            let library = r.library(v)?;
            Msex::Geth { req, library, elements: r.opt_list(v, |r| r.u8())? }
        }
        b"EThn" => Msex::Ethn {
            library_type: r.u8()?,
            library: r.library(v)?,
            element: r.u8()?,
            format: r.four()?,
            width: r.u16()?,
            height: r.u16()?,
            data: r.buffer()?,
        },
        b"ELUp" => {
            let library_type = r.u8()?;
            let library = r.library(v)?;
            let flags = r.u8()?;
            let (mut elements, mut libraries) = ([0; 32], [0; 32]);
            if v.wide() {
                elements.copy_from_slice(r.take(32)?);
                libraries.copy_from_slice(r.take(32)?);
            }
            Msex::Elup { library_type, library, flags, elements, libraries }
        }
        _ => Msex::Other { content },
    })
}

/// Sets bit `i` of an ELUp « AffectedItems » set.
pub fn set_bit(set: &mut [u8; 32], i: u8) {
    set[i as usize / 8] |= 1 << (i % 8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(msg: Message) {
        let bytes = encode(&msg, 7);
        let (h, back) = decode(&bytes).unwrap();
        assert_eq!((h.index, h.size as usize, h.major, h.minor), (7, bytes.len(), 1, 0));
        assert_eq!(back, msg);
    }

    fn info(v: Version) -> ServerInfo {
        let wide = v.wide();
        ServerInfo {
            uuid: if wide { "550e8400-e29b-41d4-a716-446655440000".into() } else { String::new() },
            product: "Laser Studio é".into(),
            version: (0, 1, if wide { 3 } else { 0 }),
            versions: if wide { vec![Version::V1_2.word()] } else { vec![] },
            library_types: if wide { 1 } else { 0 },
            thumb_formats: if wide { vec![FMT_PNG, FMT_RGB8] } else { vec![] },
            stream_formats: if wide { vec![FMT_RGB8] } else { vec![] },
            layers: vec!["ArtNet/0/0/1".into()],
        }
    }

    #[test]
    fn every_message_round_trips_in_every_version() {
        round(Message::PNam { name: "Console".into() });
        round(Message::PLoc { port: 4242, kind: "MediaServer".into(), name: "Laser Studio".into(), state: "Idle".into() });
        for v in Version::ALL {
            // 1.0 has no library ids: only {1, n, 0, 0} survive.
            let lib = library_id(3);
            let req = ThumbRequest { format: FMT_PNG, width: 128, height: 96, flags: KEEP_ASPECT, library_type: MEDIA };
            let wide = v.wide();
            let bodies = vec![
                Msex::CInf { versions: vec![0x0100, 0x0102] },
                Msex::SInf(info(v)),
                Msex::Nack { received: *b"GVSr" },
                Msex::LSta,
                Msex::Geli { library_type: MEDIA, parent: ROOT, numbers: None },
                Msex::Geli { library_type: MEDIA, parent: ROOT, numbers: Some(vec![0, 2]) },
                Msex::Elin {
                    library_type: MEDIA,
                    libraries: vec![LibraryInfo { id: lib, serial: if wide { 99 } else { 0 }, dmx: (4, 4), name: "Tunnels".into(), sub_libraries: 0, elements: 12 }],
                },
                Msex::Gein { library_type: MEDIA, library: lib, elements: None },
                Msex::Gein { library_type: MEDIA, library: lib, elements: Some(vec![1, 5]) },
                Msex::Mein {
                    library: lib,
                    elements: vec![MediaInfo { number: 0, serial: if wide { 5 } else { 0 }, dmx: (1, 1), name: "Spirale ✨".into(), stamp: 0, width: 128, height: 128, length: 0, fps: 60 }],
                },
                Msex::Gelt { req: req.clone(), libraries: Some(vec![lib]) },
                Msex::Elth { library_type: MEDIA, library: lib, format: FMT_PNG, width: 2, height: 2, data: vec![1, 2, 3] },
                Msex::Geth { req: req.clone(), library: lib, elements: Some(vec![0]) },
                Msex::Geth { req, library: lib, elements: None },
                Msex::Ethn { library_type: MEDIA, library: lib, element: 4, format: FMT_JPEG, width: 8, height: 8, data: vec![9; 100] },
                Msex::Elup {
                    library_type: MEDIA,
                    library: lib,
                    flags: UP_ELEMENTS_UPDATED,
                    elements: if wide { [0xA5; 32] } else { [0; 32] },
                    libraries: [0; 32],
                },
                Msex::Other { content: *b"GVSr" },
            ];
            for body in bodies {
                round(Message::Msex { version: v, body });
            }
        }
        round(Message::Other { layer: *b"SDMX", content: *b"ChBk" });
    }

    #[test]
    fn the_header_is_little_endian_with_the_cookie_first() {
        let b = encode(&Message::PNam { name: "A".into() }, 0x0102);
        assert_eq!(&b[0..4], b"CITP");
        assert_eq!(&b[4..8], &[1, 0, 0x02, 0x01]);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len());
        assert_eq!(&b[12..16], &[1, 0, 0, 0]);
        assert_eq!(&b[16..24], b"PINFPNam");
        assert_eq!(&b[24..], b"A\0");
    }

    #[test]
    fn malformed_messages_are_errors_not_panics() {
        assert_eq!(decode(b"CITP").unwrap_err(), WireError::Short);
        let mut b = encode(&Message::PNam { name: "A".into() }, 0);
        b[0] = b'X';
        assert_eq!(decode(&b).unwrap_err(), WireError::Cookie);
        let good = encode(&Message::Msex { version: Version::V1_2, body: Msex::Gein { library_type: 1, library: library_id(0), elements: Some(vec![1, 2, 3]) } }, 0);
        // Every truncation, and a wrong size field: refused.
        for n in 0..good.len() {
            assert!(decode(&good[..n]).is_err(), "cut at {n}");
        }
        let mut fragmented = good.clone();
        fragmented[12] = 2;
        assert_eq!(decode(&fragmented).unwrap_err(), WireError::Fragmented);
        let mut v9 = good.clone();
        v9[21] = 9;
        assert_eq!(decode(&v9).unwrap_err(), WireError::Version);
        // A string with no end, a count past its data.
        let mut pnam = encode(&Message::PNam { name: "AB".into() }, 0);
        let last = pnam.len() - 1;
        pnam[last] = b'C';
        assert!(decode(&pnam).is_err());
        // Random bytes after a valid header never panic.
        let mut seed = 1u32;
        for len in 0..400 {
            let mut b = good[..HEADER_LEN + 6].to_vec();
            for _ in 0..len {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                b.push((seed >> 16) as u8);
            }
            let n = b.len() as u32;
            b[8..12].copy_from_slice(&n.to_le_bytes());
            let _ = decode(&b);
        }
    }

    #[test]
    fn version_words_and_affected_bits() {
        assert_eq!(Version::V1_2.word(), 0x0102);
        assert_eq!(Version::from_word(0x0101), Some(Version::V1_1));
        assert_eq!(Version::from_word(0x0200), None);
        let mut set = [0; 32];
        set_bit(&mut set, 9);
        assert_eq!(set[1] & (1 << 1), 2);
    }
}
