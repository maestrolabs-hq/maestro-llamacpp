//! Copying bytes between the caller's connection and the child's.
//!
//! The heart of the slice, and the shortest module in it. That is the point:
//! everything this file does not do -- parse a response, frame it, buffer it,
//! decide when it is complete -- is a way a stream could stop being one.
//!
//! **Upstream** is the connection to the child, **downstream** the connection
//! to the caller. Every failure below says which side it happened on, because
//! the two mean opposite things: an upstream failure is a model that stopped
//! answering, a downstream one is a caller that walked away.
//!
//! Nothing here is buffered in order to keep a late error available. Once a
//! status line has been forwarded, a proxy cannot retract it and send `502`
//! instead, and a router that held a response back so it could still change
//! its mind would trade the property this slice exists for against a nicer
//! message.

use std::io::{BufReader, Read, Write};
use std::net::TcpStream;

use super::head::Head;
use crate::launch::Child;

/// How much is moved per read.
///
/// Small on purpose. Every read is written and flushed downstream before the
/// next one is attempted, so this is the largest a single event can be delayed
/// by -- not a throughput setting.
const BUFFER: usize = 8 * 1024;

/// Forwards one request and copies the response back as it arrives.
///
/// # Errors
///
/// Returns whatever the sockets returned while the request was being sent.
/// Once the response has begun, failures on either side end the relay rather
/// than propagating: there is no status left to send, and the caller's client
/// library is built to notice a closed connection.
pub(super) fn run(
    head: &Head,
    child: &Child,
    body: Option<&[u8]>,
    reader: &mut BufReader<TcpStream>,
    downstream: &mut TcpStream,
) -> std::io::Result<()> {
    let endpoint = child.endpoint();
    let mut upstream = TcpStream::connect(endpoint)?;
    upstream.write_all(head.rewrite(endpoint).as_bytes())?;
    match body {
        // Already read, because the generic endpoint had to look inside it to
        // learn which model answers. Written from what was read rather than
        // from the socket, which has nothing left on it.
        Some(bytes) => upstream.write_all(bytes)?,
        // Never read, so it is copied straight through. The dedicated
        // endpoint names its model in the path and has no reason to look.
        None => forward_body(head, reader, &mut upstream)?,
    }
    upstream.flush()?;

    copy_response(&mut upstream, downstream);
    // Dropped here whatever happened, which closes it. A closed connection is
    // how `llama-server` is told to stop generating, so a caller that hung up
    // does not leave a model producing an answer nobody reads.
    Ok(())
}

/// Copies exactly the body the caller declared.
///
/// Chunked, in the loop sense: a declared length is trusted for how much to
/// read, never for how much to allocate. A `Content-Length` of four gigabytes
/// is a header, not a reason to reserve four gigabytes.
fn forward_body(
    head: &Head,
    reader: &mut BufReader<TcpStream>,
    upstream: &mut TcpStream,
) -> std::io::Result<()> {
    let mut remaining = head.body_bytes();
    let mut buffer = [0u8; BUFFER];
    while remaining > 0 {
        let want = remaining.min(BUFFER);
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            // The caller declared more than it sent. The child is given what
            // arrived and decides for itself; guessing here would be this
            // router having an opinion about a body it does not read.
            break;
        }
        upstream.write_all(&buffer[..read])?;
        remaining -= read;
    }
    Ok(())
}

/// Copies the response until the child closes, flushing after every read.
///
/// The flush is the whole slice. A buffered writer that flushed when its
/// buffer filled would batch a stream into one delivery, and the reply text
/// would be identical either way -- which is why `tests/streaming.rs` asserts
/// when bytes arrive rather than what they say.
fn copy_response(upstream: &mut TcpStream, downstream: &mut TcpStream) {
    let mut buffer = [0u8; BUFFER];
    loop {
        // End-of-file and a broken upstream are one arm because they are one
        // action. A response that ended and one that stopped differ in what
        // they mean, not in what is left to do: no status can be sent once
        // bytes have been forwarded, so both close the connection and let the
        // caller's client library see a complete or truncated answer for
        // itself.
        let read = match upstream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };

        if downstream.write_all(&buffer[..read]).is_err() || downstream.flush().is_err() {
            // The caller hung up mid-answer. Returning drops the upstream
            // socket, which stops the child generating.
            return;
        }
    }
}
