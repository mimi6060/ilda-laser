//! Laser Studio patch (T-489): a client that doesn't read its answers
//! holds only its own connection.
//!
//! Upstream, `Request::respond` wrote the answer to the socket on the
//! calling thread: a client that never read made that thread wait for the
//! write timeout, per answer (the single worker of the studio, so every
//! route but the stop). Here an answer is handed whole to its connection's
//! `Outbox` and written by the connection's own writing thread, in request
//! order; `respond` returns at once.
//!
//! - **Order**: each request takes a number on its connection (`Turn`);
//!   answers are written in that order, whatever order they finish in.
//! - **Streams** (`into_writer`, `upgrade`, a `100 Continue`, the errors a
//!   connection answers itself) write on their own thread when their turn
//!   has come, as before: those threads may wait on the client.
//! - **Memory**: the answers waiting on all the connections of a server
//!   share a budget (`OutputBudget`). An answer on a connection that holds
//!   nothing else always goes (the handler built it already, and it is the
//!   one upstream would have been writing); past the budget, a connection
//!   that piles answers up is dropped (its answers discarded, its socket
//!   shut both ways).
//! - **Threads**: the writing thread starts when an answer is ready and
//!   ends when none is left; a client that doesn't read holds it for the
//!   write timeout, after which the connection is dropped.

use std::collections::BTreeMap;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::net::Shutdown;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::connection::Connection;
use crate::util::RefinedTcpStream;

/// A whole answer, written later on the connection's writing thread.
pub(crate) type Job = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

/// What a request's answer is written to (`Request`'s writer).
pub(crate) trait Sink: Write + Send {
    /// Takes the whole answer (`size`: about the bytes it holds) to write
    /// it later without blocking; gives it back when it must be written
    /// here and now (upstream's behaviour).
    fn defer(&mut self, job: Job, size: usize) -> Result<(), Job> {
        let _ = size;
        Err(job)
    }
}

impl Sink for Box<dyn Sink> {
    fn defer(&mut self, job: Job, size: usize) -> Result<(), Job> {
        (**self).defer(job, size)
    }
}

impl Sink for io::Sink {}

impl<W: Write + Send> Sink for crate::util::SequentialWriter<W> {}

/// The bytes of the answers waiting on all the connections of a server.
pub(crate) struct OutputBudget {
    cap: usize,
    used: AtomicUsize,
}

impl OutputBudget {
    pub(crate) fn new(cap: usize) -> Arc<OutputBudget> {
        Arc::new(OutputBudget { cap, used: AtomicUsize::new(0) })
    }

    fn reserve(&self, n: usize) -> bool {
        let before = self.used.fetch_add(n, Ordering::SeqCst);
        if before + n > self.cap {
            self.used.fetch_sub(n, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn force(&self, n: usize) {
        self.used.fetch_add(n, Ordering::SeqCst);
    }

    fn release(&self, n: usize) {
        self.used.fetch_sub(n, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }
}

/// A connection's answers, written in request order by its own thread.
pub(crate) struct Outbox {
    socket: Mutex<BufWriter<RefinedTcpStream>>,
    state: Mutex<State>,
    turn: Condvar,
    budget: Arc<OutputBudget>,
    /// A clone of the socket, to shut it while a write holds `socket`.
    handle: Option<Arc<Connection>>,
}

struct State {
    /// Numbers handed out to requests.
    issued: u64,
    /// The first answer not yet written.
    next: u64,
    /// Answers finished before their turn, with their counted size.
    ready: BTreeMap<u64, (Job, usize)>,
    /// Bytes counted in the budget for this connection (waiting, or being
    /// written).
    held: usize,
    /// A writing thread is running.
    writing: bool,
    /// The connection is dropped: nothing more is written.
    broken: bool,
}

/// Fixed cost counted for an answer besides its body (its head).
const HEAD: usize = 512;

/// The most requests of one connection read but not yet answered and
/// written (`Outbox::wait_room`). A browser sends one at a time.
pub(crate) const PIPELINE: u64 = 4;

impl Outbox {
    pub(crate) fn new(socket: BufWriter<RefinedTcpStream>, budget: Arc<OutputBudget>, handle: Option<Arc<Connection>>) -> Arc<Outbox> {
        Arc::new(Outbox {
            socket: Mutex::new(socket),
            state: Mutex::new(State { issued: 0, next: 0, ready: BTreeMap::new(), held: 0, writing: false, broken: false }),
            turn: Condvar::new(),
            budget,
            handle,
        })
    }

    /// The writer of the next request read on this connection.
    pub(crate) fn turn(self: &Arc<Self>) -> Turn {
        let mut st = self.lock();
        let seq = st.issued;
        st.issued += 1;
        Turn { outbox: Arc::clone(self), seq, wrote: false, done: false }
    }

    /// Waits, on the connection's reading thread, while `PIPELINE`
    /// requests of this connection are still unanswered or unwritten: a
    /// client that sends requests in a row and doesn't read the answers
    /// gets no more read, so it can't fill the application's queue (every
    /// one of them costs the single worker its handling time). Returns at
    /// once when the connection is dropped.
    pub(crate) fn wait_room(&self) {
        let mut st = self.lock();
        while !st.broken && st.issued - st.next >= PIPELINE {
            st = self.turn.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn socket(&self) -> MutexGuard<'_, BufWriter<RefinedTcpStream>> {
        self.socket.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Answer `seq` is finished: written when its turn comes. Never waits
    /// on the client.
    fn finish(self: &Arc<Self>, seq: u64, job: Job, size: usize) {
        let mut st = self.lock();
        if st.broken {
            return;
        }
        if st.held == 0 {
            self.budget.force(size);
        } else if !self.budget.reserve(size) {
            log::debug!("answers piling up past the output budget: connection dropped");
            self.break_off(&mut st);
            return;
        }
        st.held += size;
        st.ready.insert(seq, (job, size));
        self.kick(st);
    }

    /// Starts the writing thread if the next answer is ready and none runs.
    fn kick(self: &Arc<Self>, mut st: MutexGuard<'_, State>) {
        if st.writing || st.broken || !st.ready.contains_key(&st.next) {
            return;
        }
        st.writing = true;
        drop(st);
        let outbox = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("http-write".into()).stack_size(128 * 1024).spawn(move || outbox.drain());
        if let Err(e) = spawned {
            log::warn!("no thread to write an answer: {e}");
            let mut st = self.lock();
            st.writing = false;
            self.break_off(&mut st);
        }
    }

    /// The writing thread: the ready answers, in order, then a flush.
    fn drain(self: &Arc<Self>) {
        let mut st = self.lock();
        loop {
            if st.broken {
                st.writing = false;
                return;
            }
            let next = st.next;
            let Some((job, size)) = st.ready.remove(&next) else {
                drop(st);
                let flushed = self.socket().flush();
                st = self.lock();
                if let Err(e) = flushed {
                    self.failed(&mut st, &e);
                }
                if !st.broken && st.ready.contains_key(&st.next) {
                    continue;
                }
                st.writing = false;
                return;
            };
            drop(st);
            let written = job(&mut *self.socket());
            st = self.lock();
            st.next += 1;
            st.held -= size;
            self.budget.release(size);
            if let Err(e) = written {
                self.failed(&mut st, &e);
            }
            self.turn.notify_all();
        }
    }

    /// A write failed: the client is gone or doesn't read (the write
    /// timeout has already shut the write side). Nothing more is written.
    fn failed(&self, st: &mut State, e: &io::Error) {
        log::debug!("failed to send an answer: {e}");
        self.break_off(st);
    }

    /// Drops the connection: the waiting answers are discarded, the socket
    /// is shut (its reading thread ends, a stream's next write fails).
    fn break_off(&self, st: &mut State) {
        st.broken = true;
        let discarded: usize = st.ready.values().map(|(_, size)| size).sum();
        st.ready.clear();
        st.held -= discarded;
        self.budget.release(discarded);
        if let Some(handle) = &self.handle {
            // One at a time: macOS refuses `Both` once one side is shut.
            handle.shutdown(Shutdown::Write).ok();
            handle.shutdown(Shutdown::Read).ok();
        }
        self.turn.notify_all();
    }

    /// Waits until answer `seq`'s turn has come (every answer before it
    /// written); `Err` once the connection is dropped.
    fn wait_turn(&self, seq: u64) -> io::Result<()> {
        let mut st = self.lock();
        while !st.broken && st.next != seq {
            st = self.turn.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        if st.broken {
            return Err(io::Error::new(ErrorKind::BrokenPipe, "connection dropped"));
        }
        Ok(())
    }
}

/// A request's place in its connection's answers. Deferred, its answer is
/// written by the connection's writing thread; written to directly, it
/// waits for its turn and writes on the calling thread (a stream).
pub(crate) struct Turn {
    outbox: Arc<Outbox>,
    seq: u64,
    /// Bytes were written directly: the turn is held.
    wrote: bool,
    /// The answer was handed over.
    done: bool,
}

impl Turn {
    fn direct<T>(&mut self, f: impl FnOnce(&mut BufWriter<RefinedTcpStream>) -> io::Result<T>) -> io::Result<T> {
        self.outbox.wait_turn(self.seq)?;
        self.wrote = true;
        let result = f(&mut *self.outbox.socket());
        if let Err(e) = &result {
            if !matches!(e.kind(), ErrorKind::Interrupted) {
                let mut st = self.outbox.lock();
                self.outbox.failed(&mut st, e);
            }
        }
        result
    }
}

impl Write for Turn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.direct(|s| s.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.direct(|s| s.flush())
    }
}

impl Sink for Turn {
    fn defer(&mut self, job: Job, size: usize) -> Result<(), Job> {
        if self.wrote {
            // Part of the answer went out directly (a `100 Continue`):
            // the rest follows on this thread, as upstream.
            return Err(job);
        }
        self.done = true;
        self.outbox.finish(self.seq, job, size + HEAD);
        Ok(())
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if self.wrote {
            // The turn is ours: flush here, then pass it on.
            let flushed = self.outbox.socket().flush();
            let mut st = self.outbox.lock();
            if let Err(e) = flushed {
                self.outbox.failed(&mut st, &e);
            }
            st.next += 1;
            self.outbox.turn.notify_all();
            self.outbox.kick(st);
        } else {
            // Nothing written (a request that failed to read): its turn is
            // passed on at once if it has come, else an empty answer keeps
            // the order. No thread starts for it.
            let mut st = self.outbox.lock();
            if st.broken {
                return;
            }
            if st.next == self.seq && !st.writing {
                st.next += 1;
                self.outbox.turn.notify_all();
                self.outbox.kick(st);
            } else {
                st.ready.insert(self.seq, (Box::new(|_| Ok(())), 0));
            }
        }
    }
}

/// A connection's answers in order: `Outbox`es for the limited servers
/// (`ClientLimits`), upstream's sequential writers otherwise.
pub(crate) enum Sinks {
    Sequential(crate::util::SequentialWriterBuilder<BufWriter<RefinedTcpStream>>),
    Outbox(Arc<Outbox>),
}

impl Sinks {
    /// Before the next request is read: waits while the connection has
    /// `PIPELINE` answers outstanding (an `Outbox`; upstream never waits).
    pub(crate) fn wait_room(&self) {
        if let Sinks::Outbox(o) = self {
            o.wait_room();
        }
    }

    pub(crate) fn next_writer(&mut self) -> Box<dyn Sink> {
        match self {
            Sinks::Sequential(b) => Box::new(b.next().expect("endless")),
            Sinks::Outbox(o) => Box::new(o.turn()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    /// An outbox on a real socket pair (127.0.0.1), its reading half (kept
    /// open) and the client's end.
    fn pair(budget: &Arc<OutputBudget>) -> (Arc<Outbox>, RefinedTcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
        let server = Connection::from(server);
        let handle = Arc::new(server.try_clone().unwrap());
        let (read, write) = RefinedTcpStream::new(server).unwrap();
        (Outbox::new(BufWriter::new(write), Arc::clone(budget), Some(handle)), read, client)
    }

    fn answer(text: &'static str) -> Job {
        Box::new(move |w| w.write_all(text.as_bytes()))
    }

    fn read_exactly(client: &mut TcpStream, n: usize) -> String {
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = vec![0u8; n];
        client.read_exact(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn answers_go_out_in_request_order_whatever_order_they_finish_in() {
        let budget = OutputBudget::new(1 << 20);
        let (outbox, _read, mut client) = pair(&budget);
        let (mut a, mut b, mut c) = (outbox.turn(), outbox.turn(), outbox.turn());
        assert!(c.defer(answer("c"), 1).is_ok());
        assert!(b.defer(answer("b"), 1).is_ok());
        assert!(a.defer(answer("a"), 1).is_ok());
        assert_eq!(read_exactly(&mut client, 3), "abc");
        drop((a, b, c));
        // A stream waits for its turn, then writes on its own thread.
        let (mut d, mut stream) = (outbox.turn(), outbox.turn());
        let writer = std::thread::spawn(move || {
            stream.write_all(b"s").unwrap();
            stream.flush().unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(d.defer(answer("d"), 1).is_ok());
        writer.join().unwrap();
        assert_eq!(read_exactly(&mut client, 2), "ds");
        // A request dropped unanswered keeps the order.
        let (skipped, mut e) = (outbox.turn(), outbox.turn());
        assert!(e.defer(answer("e"), 1).is_ok());
        drop(skipped);
        assert_eq!(read_exactly(&mut client, 1), "e");
        // Past `PIPELINE` answers outstanding, the reading side waits until
        // one is written.
        let held: Vec<Turn> = (0..PIPELINE).map(|_| outbox.turn()).collect();
        let reader = {
            let outbox = Arc::clone(&outbox);
            std::thread::spawn(move || {
                let t = Instant::now();
                outbox.wait_room();
                t.elapsed()
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
        assert!(reader.join().unwrap() >= Duration::from_millis(90));
        let deadline = Instant::now() + Duration::from_secs(2);
        while budget.used() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_client_that_never_reads_holds_no_caller_and_is_dropped_past_the_budget() {
        let budget = OutputBudget::new(256 * 1024);
        let (outbox, _read, client) = pair(&budget);
        let big = vec![b'x'; 4 << 20];
        let t = Instant::now();
        for _ in 0..50 {
            let mut turn = outbox.turn();
            let body = big.clone();
            let _ = turn.defer(Box::new(move |w| w.write_all(&body)), big.len());
        }
        // Handing 50 answers over never waited on the client.
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        // The first one went out (alone on its connection), the rest past
        // the budget dropped the connection and gave their bytes back.
        let deadline = Instant::now() + Duration::from_secs(5);
        while budget.used() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(budget.used(), 0);
        assert!(outbox.lock().broken);
        drop(client);
    }
}
