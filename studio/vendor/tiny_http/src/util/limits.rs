//! Laser Studio patch (T-481): slow clients block only themselves.
//!
//! - A request's body is read on its connection's own thread before the
//!   request is handed to the application (`Prefetch`), so a client that
//!   sends its body slowly holds that thread, never the one that serves
//!   the requests. The bodies held in memory have a total budget.
//! - An unread body is skipped on the connection's own thread too
//!   (`Skipping`), never on the thread that answered the request.
//! - The connections being read are capped (`Readers`): past the cap, a new
//!   connection pushes out the one that has been waiting longest for its
//!   request, so a flood of slow clients can't exhaust threads or file
//!   descriptors, nor lock out a new, fast request.

use std::io::{self, Cursor, ErrorKind, Read};
use std::net::Shutdown;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::connection::Connection;
use crate::Method;

/// What a server allows each client (see `Server::from_listener_with_limits`).
#[derive(Debug, Clone, Copy)]
pub struct ClientLimits {
    /// Read and write timeout on every accepted connection.
    pub timeout: Option<Duration>,
    /// A body up to this size is read whole before its request is handed
    /// over (`Request::body_buffered`). A bigger one is streamed, and an
    /// unread rest of it is skipped by the connection's thread. A chunked
    /// body bigger than this is refused (413).
    pub prefetch: usize,
    /// The most body bytes held in memory at once by the prefetched
    /// requests of all connections; past it, a request is refused (503).
    pub prefetch_budget: usize,
    /// The most connections served at once (see `Readers`).
    pub max_connections: usize,
    /// Requests handed over as soon as their head is read, the body left
    /// unread (a stop that must never wait for a slow body).
    pub head_only: Option<fn(&Method, &str) -> bool>,
}

/// The longest unread rest of a body skipped to keep a connection
/// (T-449's drain limit); past it the connection is closed.
pub(crate) const SKIP_LIMIT: usize = 1 << 20;

/// Body prefetching, shared by all the connections of a server.
pub(crate) struct Prefetch {
    pub(crate) limit: usize,
    budget: usize,
    used: AtomicUsize,
    pub(crate) head_only: Option<fn(&Method, &str) -> bool>,
}

/// Why a body could not be prefetched.
pub(crate) enum PrefetchError {
    Io(io::Error),
    /// Past the per-request limit (a chunked body).
    TooLarge,
    /// Past the memory budget shared by all the connections.
    Busy,
}

impl Prefetch {
    pub(crate) fn new(limits: &ClientLimits) -> Arc<Prefetch> {
        Arc::new(Prefetch { limit: limits.prefetch, budget: limits.prefetch_budget, used: AtomicUsize::new(0), head_only: limits.head_only })
    }

    fn reserve(&self, n: usize) -> bool {
        let before = self.used.fetch_add(n, Ordering::SeqCst);
        if before + n > self.budget {
            self.used.fetch_sub(n, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn release(&self, n: usize) {
        self.used.fetch_sub(n, Ordering::SeqCst);
    }

    /// Reads `source` to its end (at most `max` bytes, more is
    /// `TooLarge`), growing with what actually arrives: a client that
    /// announces much and sends little holds little.
    pub(crate) fn read<R: Read>(self: &Arc<Self>, mut source: R, max: usize) -> Result<BufferedBody, PrefetchError> {
        const CHUNK: usize = 64 * 1024;
        let mut body = BufferedBody { data: Cursor::new(Vec::new()), held: 0, prefetch: Arc::clone(self) };
        loop {
            let len = body.data.get_ref().len();
            // One byte past `max` tells a body that is too big.
            let want = CHUNK.min(max + 1 - len);
            if !self.reserve(want) {
                return Err(PrefetchError::Busy);
            }
            body.held += want;
            let data = body.data.get_mut();
            data.resize(len + want, 0);
            let read = loop {
                match source.read(&mut data[len..]) {
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    other => break other,
                }
            };
            let n = match read {
                Ok(n) => n,
                Err(e) => {
                    data.truncate(len);
                    return Err(PrefetchError::Io(e));
                }
            };
            data.truncate(len + n);
            self.release(want - n);
            body.held -= want - n;
            if n == 0 {
                body.data.get_mut().shrink_to_fit();
                return Ok(body);
            }
            if len + n > max {
                return Err(PrefetchError::TooLarge);
            }
        }
    }
}

/// A body read whole in advance; gives its bytes back to the budget when
/// the request goes.
pub(crate) struct BufferedBody {
    data: Cursor<Vec<u8>>,
    held: usize,
    prefetch: Arc<Prefetch>,
}

impl BufferedBody {
    pub(crate) fn len(&self) -> usize {
        self.data.get_ref().len()
    }
}

impl Read for BufferedBody {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.read(buf)
    }
}

impl Drop for BufferedBody {
    fn drop(&mut self) {
        self.prefetch.release(self.held);
    }
}

/// The connection's stream under its requests' readers: before the next
/// request is read, skips what the previous one left unread of its body
/// (`leftover`, set by `EqualReader` when dropped; `usize::MAX`: the body
/// failed or was too long to skip, and the connection ends).
pub(crate) struct Skipping<R> {
    inner: R,
    leftover: Arc<AtomicUsize>,
}

impl<R: Read> Skipping<R> {
    pub(crate) fn new(inner: R, leftover: Arc<AtomicUsize>) -> Self {
        Skipping { inner, leftover }
    }
}

impl<R: Read> Read for Skipping<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut skip = self.leftover.swap(0, Ordering::SeqCst);
        if skip > SKIP_LIMIT {
            self.leftover.store(usize::MAX, Ordering::SeqCst);
            return Err(io::Error::new(ErrorKind::ConnectionAborted, "unread body too long to skip"));
        }
        let mut scratch = [0u8; 8192];
        while skip > 0 {
            let n = match self.inner.read(&mut scratch[..skip.min(8192)]) {
                Ok(0) => return Ok(0),
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.leftover.store(usize::MAX, Ordering::SeqCst);
                    return Err(e);
                }
            };
            skip -= n;
        }
        self.inner.read(buf)
    }
}

/// The connections of a server, capped. Each one records since when it
/// has been waiting for (part of) a request (`Mark`); a connection whose
/// request is being answered isn't waiting. Past the cap, a new connection
/// shuts the read side of the one waiting longest: its thread ends, and
/// an answer it is still owed is still written.
pub(crate) struct Readers {
    max: usize,
    epoch: Instant,
    next_id: AtomicU64,
    open: Mutex<Vec<(Arc<Mark>, Connection)>>,
}

/// Since when a connection has been waiting for its client, in
/// microseconds from the server's start plus one; 0 while it isn't.
pub(crate) struct Mark {
    id: u64,
    since: AtomicU64,
    epoch: Instant,
}

impl Mark {
    pub(crate) fn waiting(&self) {
        self.since.store(self.epoch.elapsed().as_micros() as u64 + 1, Ordering::SeqCst);
    }

    pub(crate) fn served(&self) {
        self.since.store(0, Ordering::SeqCst);
    }
}

/// A connection's place among the `Readers`, given back when its thread
/// ends.
pub(crate) struct Admitted {
    readers: Arc<Readers>,
    pub(crate) mark: Arc<Mark>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        if let Ok(mut open) = self.readers.open.lock() {
            open.retain(|(m, _)| m.id != self.mark.id);
        }
    }
}

impl Readers {
    pub(crate) fn new(max: usize) -> Arc<Readers> {
        Arc::new(Readers { max: max.max(1), epoch: Instant::now(), next_id: AtomicU64::new(0), open: Mutex::new(Vec::new()) })
    }

    /// Admits a new connection (`handle`: a clone of its socket, to shut
    /// it from here). At the cap, the connection waiting longest for its
    /// client is pushed out; `None` (the new one is dropped) when none is
    /// waiting, all of them being answered.
    pub(crate) fn admit(self: &Arc<Self>, handle: Connection) -> Option<Admitted> {
        let mut open = self.open.lock().ok()?;
        if open.len() >= self.max {
            let oldest = open
                .iter()
                .enumerate()
                .filter_map(|(i, (m, _))| match m.since.load(Ordering::SeqCst) {
                    0 => None,
                    since => Some((since, i)),
                })
                .min()?;
            let (_, stale) = open.swap_remove(oldest.1);
            stale.shutdown(Shutdown::Read).ok();
        }
        let mark = Arc::new(Mark { id: self.next_id.fetch_add(1, Ordering::SeqCst), since: AtomicU64::new(0), epoch: self.epoch });
        mark.waiting();
        open.push((Arc::clone(&mark), handle));
        Some(Admitted { readers: Arc::clone(self), mark })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefetch(limit: usize, budget: usize) -> Arc<Prefetch> {
        Prefetch::new(&ClientLimits { timeout: None, prefetch: limit, prefetch_budget: budget, max_connections: 4, head_only: None })
    }

    #[test]
    fn a_body_is_read_whole_within_its_limits() {
        let p = prefetch(16, 64);
        let mut body = p.read(&b"hello"[..], 16).ok().unwrap();
        assert_eq!(p.used.load(Ordering::SeqCst), 5);
        let mut s = String::new();
        body.read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello");
        drop(body);
        assert_eq!(p.used.load(Ordering::SeqCst), 0);
        assert!(matches!(p.read(&[1u8; 17][..], 16), Err(PrefetchError::TooLarge)));
        assert_eq!(p.used.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_budget_is_shared_and_given_back() {
        let p = prefetch(1 << 20, 100_000);
        let a = p.read(&[0u8; 70_000][..], 70_000).ok().unwrap();
        assert!(matches!(p.read(&[0u8; 70_000][..], 70_000), Err(PrefetchError::Busy)));
        drop(a);
        assert!(p.read(&[0u8; 70_000][..], 70_000).is_ok());
        assert_eq!(p.used.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn skipping_drops_the_unread_rest_or_ends_the_connection() {
        let leftover = Arc::new(AtomicUsize::new(3));
        let mut s = Skipping::new(&b"abcGET"[..], Arc::clone(&leftover));
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert_eq!(out, "GET");
        leftover.store(usize::MAX, Ordering::SeqCst);
        assert!(Skipping::new(&b"x"[..], leftover).read(&mut [0u8; 4]).is_err());
    }
}
