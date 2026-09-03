//! Asking a child whether it is ready to answer.
//!
//! One request, one status line, and nothing else read from the reply. This
//! is deliberately not the beginning of an HTTP client: the slice that proxies
//! needs streamed responses, connection reuse and concurrency, and should
//! choose a library against those requirements rather than inherit one picked
//! for a status code. Keeping the probe behind the launch module's interface
//! is what makes replacing it later a local change.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long one probe may take before it is treated as no answer.
///
/// Both halves are bounded. A connect that hangs and a reply that never
/// arrives are the same thing from here, and neither may outlive the poll it
/// belongs to.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The status code from `GET /health`, or `None` when nothing answered.
///
/// `llama-server` answers 503 while a model is loading and 200 once it will
/// serve, which is the whole contract this slice depends on.
pub(super) fn health(address: SocketAddr) -> Option<u16> {
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;

    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status).ok()?;
    // `HTTP/1.1 200 OK`: the second word, and none of the rest.
    status.split_whitespace().nth(1)?.parse().ok()
}
