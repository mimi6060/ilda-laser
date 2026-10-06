use std::io::Result as IoResult;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr};

use crate::connection::Connection;
#[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
use crate::ssl::SslStream;

pub(crate) enum Stream {
    Http(Connection),
    #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
    Https(SslStream),
}

impl Clone for Stream {
    fn clone(&self) -> Self {
        match self {
            Stream::Http(tcp_stream) => Stream::Http(tcp_stream.try_clone().unwrap()),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => Stream::Https(ssl_stream.clone()),
        }
    }
}

impl From<Connection> for Stream {
    fn from(tcp_stream: Connection) -> Self {
        Stream::Http(tcp_stream)
    }
}

impl Stream {
    fn secure(&self) -> bool {
        match self {
            Stream::Http(_) => false,
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(_) => true,
        }
    }

    fn peer_addr(&mut self) -> IoResult<Option<SocketAddr>> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.peer_addr(),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => ssl_stream.peer_addr(),
        }
    }

    /// Laser Studio patch (T-449): the socket's write timeout.
    fn write_timeout(&self) -> Option<std::time::Duration> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.write_timeout(),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(_) => None,
        }
    }

    /// Laser Studio patch (T-449): `clone` without the panic when the
    /// process is out of file descriptors.
    fn try_clone(&self) -> IoResult<Self> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.try_clone().map(Stream::Http),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => Ok(Stream::Https(ssl_stream.clone())),
        }
    }

    fn shutdown(&mut self, how: Shutdown) -> IoResult<()> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.shutdown(how),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => ssl_stream.shutdown(how),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.read(buf),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => ssl_stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.write(buf),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => ssl_stream.write(buf),
        }
    }

    fn flush(&mut self) -> IoResult<()> {
        match self {
            Stream::Http(tcp_stream) => tcp_stream.flush(),
            #[cfg(any(feature = "ssl-openssl", feature = "ssl-rustls"))]
            Stream::Https(ssl_stream) => ssl_stream.flush(),
        }
    }
}

pub struct RefinedTcpStream {
    stream: Stream,
    close_read: bool,
    close_write: bool,
    // Laser Studio patch (T-449): the socket's write timeout, if any.
    write_timeout: Option<std::time::Duration>,
}

impl RefinedTcpStream {
    pub(crate) fn new<S>(stream: S) -> IoResult<(RefinedTcpStream, RefinedTcpStream)>
    where
        S: Into<Stream>,
    {
        let stream: Stream = stream.into();

        let write_timeout = stream.write_timeout();
        let (read, write) = (stream.try_clone()?, stream);

        let read = RefinedTcpStream {
            stream: read,
            close_read: true,
            close_write: false,
            write_timeout,
        };

        let write = RefinedTcpStream {
            stream: write,
            close_read: false,
            close_write: true,
            write_timeout,
        };

        Ok((read, write))
    }

    /// Returns true if this struct wraps around a secure connection.
    #[inline]
    pub(crate) fn secure(&self) -> bool {
        self.stream.secure()
    }

    pub(crate) fn peer_addr(&mut self) -> IoResult<Option<SocketAddr>> {
        self.stream.peer_addr()
    }
}

impl Drop for RefinedTcpStream {
    fn drop(&mut self) {
        if self.close_read {
            self.stream.shutdown(Shutdown::Read).ok();
        }

        if self.close_write {
            self.stream.shutdown(Shutdown::Write).ok();
        }
    }
}

impl Read for RefinedTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.stream.read(buf)
    }
}

impl Write for RefinedTcpStream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        // Laser Studio patch (T-449): a peer that kept a write waiting for
        // the whole write timeout (an error, or a few bytes taken at the
        // end of it) is dropped, so the answers queued behind on this
        // connection fail at once instead of each waiting it out.
        let started = std::time::Instant::now();
        let written = self.stream.write(buf);
        let stalled = match &written {
            Err(e) => matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut),
            // (The system's timer may end the wait a little early.)
            Ok(_) => self.write_timeout.is_some_and(|t| started.elapsed() >= t * 9 / 10),
        };
        if stalled {
            // Write only: macOS refuses `Both` once the read side is shut.
            self.stream.shutdown(Shutdown::Write).ok();
        }
        written
    }

    fn flush(&mut self) -> IoResult<()> {
        self.stream.flush()
    }
}
