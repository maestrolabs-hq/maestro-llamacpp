//! Answering one request.
//!
//! Split from the binary beside it when that file grew past the module-size
//! gate, and along the seam the gate exposed: the parent decides what the stub
//! was asked to do, and this decides what one connection is answered with.
//!
//! Three parts of a server's contract live here. `/health` carries readiness,
//! a path ending `/v1/chat/completions` carries a paced stream, and `/v1/echo`
//! reflects what arrived -- the last of which no real server serves, and
//! exists so a test can observe what reached the child rather than trusting
//! what the router believes it sent.
//!
//! The echo also reports the alias the stub was started as. Every child is
//! the same binary, so without it a test cannot tell which child answered.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// How large a request head this stub will read before giving up on it.
///
/// Bounded, because a stub that grew a buffer for whatever a bad client sent
/// would be a memory bug in the one place a test cannot see it.
const HEAD_LIMIT: usize = 64 * 1024;

/// How the stub was asked to pace a stream.
pub struct Pacing {
    /// How many events a full stream carries.
    pub events: usize,
    /// How long to wait before each one.
    pub gap: Duration,
    /// After how many events to hang up without finishing, if at all.
    pub die_after: Option<usize>,
    /// Where to record that a write failed, if anywhere.
    ///
    /// A file rather than a log line, because the router spawns children with
    /// their output discarded. It is how a test observes that the relay closed
    /// the upstream connection when its caller went away: without it, "the
    /// model stopped generating" is a claim nothing outside the stub can see.
    pub hangup_marker: Option<PathBuf>,
}

/// Answers one request, and says nothing about the next.
///
/// # Errors
///
/// Returns whatever the socket returned. Every error here is a client that
/// hung up, which the caller drops: nothing in this stub is durable.
pub fn answer(
    mut stream: TcpStream,
    ready: bool,
    pacing: &Pacing,
    alias: &str,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let head = read_head(&mut reader)?;

    // Drained from the same reader that read the head, because the buffer may
    // already hold part of the body. Discarded: nothing here inspects it, and
    // a body left unread would fail the caller's write rather than this stub's.
    let length = content_length(&head);
    if length > 0 {
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
    }

    let request_line = head.first().map_or("", String::as_str);
    let path = request_line.split_whitespace().nth(1).unwrap_or("");

    if request_line.starts_with("GET /health ") {
        return serve_health(&mut stream, ready);
    }
    if path.ends_with("/v1/chat/completions") {
        return serve_stream(&mut stream, pacing);
    }
    if path.ends_with("/v1/echo") {
        let body = format!("{}\nalias: {alias}", head.join("\n"));
        return serve_complete(&mut stream, "200 OK", "text/plain", &body);
    }
    serve_complete(
        &mut stream,
        "404 Not Found",
        "application/json",
        "{\"error\":\"not found\"}",
    )
}

/// The request line and headers, ending at the first blank line.
fn read_head(reader: &mut impl BufRead) -> std::io::Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut read = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        read += line.len();
        if read > HEAD_LIMIT {
            return Err(std::io::Error::other("request head is too large"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed);
    }
    Ok(lines)
}

/// The declared body length, or zero when there is no such header.
fn content_length(head: &[String]) -> usize {
    head.iter()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

/// The readiness contract: 503 while loading, 200 once ready.
fn serve_health(stream: &mut TcpStream, ready: bool) -> std::io::Result<()> {
    let (status, body) = if ready {
        ("200 OK", "{\"status\":\"ok\"}")
    } else {
        ("503 Service Unavailable", "{\"status\":\"loading\"}")
    };
    serve_complete(stream, status, "application/json", body)
}

/// A complete small reply with a declared length.
///
/// Every reply but the stream is one of these: a status, a content type, and a
/// body whose length is known before the first byte goes out.
fn serve_complete(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )?;
    stream.flush()
}

/// A paced stream, flushed after every event.
///
/// The flush is the whole point. A stub that buffered its own output would
/// make the router's streaming test vacuous: the events would arrive together
/// whatever the relay did with them.
fn serve_stream(stream: &mut TcpStream, pacing: &Pacing) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\
         \r\n"
    )?;
    stream.flush()?;

    for index in 0..pacing.events {
        thread::sleep(pacing.gap);
        if let Err(error) = write_event(stream, index) {
            // The far end went away. Recorded where a test can see it, then
            // reported: this is the observable half of the router closing an
            // upstream connection whose caller hung up.
            if let Some(marker) = &pacing.hangup_marker {
                drop(std::fs::write(marker, b"the caller went away"));
            }
            return Err(error);
        }
        // Dropped without finishing the stream, so a caller sees the response
        // truncate rather than end. This is how a mid-stream upstream death is
        // driven.
        if pacing.die_after.is_some_and(|limit| index + 1 >= limit) {
            return Ok(());
        }
    }
    Ok(())
}

/// One event, flushed so it leaves this process when it is produced.
fn write_event(stream: &mut TcpStream, index: usize) -> std::io::Result<()> {
    write!(stream, "data: {{\"n\":{index}}}\n\n")?;
    stream.flush()
}
