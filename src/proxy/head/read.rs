//! Reading a request head off a connection, within bounds.
//!
//! The one part of handling a head that touches a socket, split from the
//! translation beside it so that module can be what its doc says it is: bytes
//! in, a head out, and nothing that blocks. What blocks lives here, and the
//! two ways it can stop -- a bound reached, a deadline reached -- are what
//! this module has to say.

use std::io::{BufRead, Read as _};

use crate::proxy::refusal::{Cause, Refusal};

/// How many bytes of head the router will read before refusing.
///
/// A router that read an unbounded head from a socket is a router with a
/// memory bug waiting for a bad client. The limit is generous for a request
/// line and a dozen headers, and far below anything worth allocating for.
pub(in crate::proxy) const MAX_HEAD_BYTES: usize = 64 * 1024;

/// How many header lines the router will accept.
pub(in crate::proxy) const MAX_HEADERS: usize = 100;

/// Reads the request line and headers, ending at the first blank line.
///
/// # Errors
///
/// Returns a [`Refusal`] when the head exceeds either bound, when the caller
/// stops sending before a blank line arrives, or when the connection ends
/// first.
pub(in crate::proxy) fn read(reader: &mut impl BufRead) -> Result<Vec<String>, Refusal> {
    let mut lines: Vec<String> = Vec::new();
    let mut read = 0usize;
    loop {
        let remaining = MAX_HEAD_BYTES.saturating_sub(read);
        if remaining == 0 {
            return Err(unread(
                Cause::HeadTooLarge,
                &format!("longer than {MAX_HEAD_BYTES} bytes"),
            ));
        }

        // Bounded at the read rather than after it. Checking the length of a
        // line already in memory would be a limit that allocates whatever it
        // was given before deciding it was too much.
        let mut line = String::new();
        let taken = u64::try_from(remaining).unwrap_or(u64::MAX);
        let count = (&mut *reader)
            .take(taken)
            .read_line(&mut line)
            .map_err(|error| {
                let cause = if timed_out(&error) {
                    Cause::RequestTimeout
                } else {
                    Cause::MalformedRequest
                };
                unread(cause, &format!("unreadable: {error}"))
            })?;
        if count == 0 {
            return Err(unread(
                Cause::MalformedRequest,
                "ended before its blank line",
            ));
        }
        read += count;

        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        if trimmed.is_empty() {
            return Ok(lines);
        }
        if lines.len() >= MAX_HEADERS {
            return Err(unread(
                Cause::HeadTooLarge,
                &format!("more than {MAX_HEADERS} lines"),
            ));
        }
        lines.push(trimmed);
    }
}

/// Whether a socket read gave up on a deadline rather than on the peer.
///
/// Two kinds, because the platforms disagree: a read deadline surfaces as
/// `WouldBlock` on the Unix platforms and as `TimedOut` on Windows, and a
/// router that knew only one would report a silent caller as a malformed one
/// on the other.
pub(in crate::proxy) fn timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// A head the router will not read, and why.
fn unread(cause: Cause, reason: &str) -> Refusal {
    Refusal::new(cause, format!("the request head is {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::io::Cursor;

    #[test]
    fn a_head_within_the_bounds_is_read_up_to_the_blank_line() {
        let mut source = Cursor::new(
            b"POST /models/gemma3/v1/chat/completions HTTP/1.1\r\n\
              Host: localhost\r\n\
              \r\n\
              {\"model\":\"gemma3\"}"
                .to_vec(),
        );

        let head = read(&mut source).expect("a head that ends");

        assert_eq!(head.len(), 2, "the request line and one header: {head:?}");
        assert!(
            head[0].starts_with("POST /models/gemma3/"),
            "and nothing of the body: {head:?}"
        );
    }

    #[test]
    fn a_head_larger_than_the_bound_is_refused_rather_than_allocated_for() {
        let padding = "x".repeat(MAX_HEAD_BYTES);
        let mut source = Cursor::new(
            format!("GET /models/gemma3/v1/models HTTP/1.1\r\nX-Big: {padding}\r\n\r\n")
                .into_bytes(),
        );

        let refusal = read(&mut source).expect_err("a bad client cannot make the router allocate");

        assert_eq!(
            refusal.cause(),
            Cause::HeadTooLarge,
            "and is told the head was the problem, not the request"
        );
    }

    #[test]
    fn more_headers_than_the_bound_are_refused() {
        let mut text = String::from("GET /models/gemma3/v1/models HTTP/1.1\r\n");
        for index in 0..=MAX_HEADERS {
            writeln!(text, "X-Count-{index}: \r").expect("writing to a String");
        }
        text.push_str("\r\n");

        read(&mut Cursor::new(text.into_bytes()))
            .expect_err("a head is a small thing, and one that is not is refused");
    }

    #[test]
    fn a_connection_that_ends_before_the_blank_line_is_refused() {
        let mut source = Cursor::new(b"GET /models/gemma3/v1/models HTTP/1.1\r\n".to_vec());

        let refusal = read(&mut source).expect_err("a truncated head is not a head");

        assert_eq!(refusal.cause(), Cause::MalformedRequest);
    }

    #[test]
    fn a_deadline_is_told_apart_from_a_peer_that_went_away() {
        for kind in [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut] {
            assert!(
                timed_out(&std::io::Error::from(kind)),
                "{kind:?} is what a read deadline looks like on one platform \
                 or the other"
            );
        }
        assert!(!timed_out(&std::io::Error::from(
            std::io::ErrorKind::ConnectionReset
        )));
    }
}
