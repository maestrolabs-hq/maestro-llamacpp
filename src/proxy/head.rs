//! The request head: what the router reads of a request, and what it sends on.
//!
//! Pure translation. Bytes in, a parsed head out, and a rewritten head back to
//! bytes -- no sockets, no processes, no clock. That is what lets every rule
//! below be asserted directly rather than inferred from a running relay.
//!
//! This is the only part of a request the router understands. Everything after
//! the blank line is copied without being read, which is the decision the
//! whole slice rests on: what the router does not parse, it cannot buffer.

use std::fmt::Write as _;
use std::io::{BufRead, Read as _};
use std::net::SocketAddr;

use crate::launch::Failure;

/// How many bytes of head the router will read before refusing.
///
/// A router that read an unbounded head from a socket is a router with a
/// memory bug waiting for a bad client. The limit is generous for a request
/// line and a dozen headers, and far below anything worth allocating for.
pub(super) const MAX_HEAD_BYTES: usize = 64 * 1024;

/// How many header lines the router will accept.
pub(super) const MAX_HEADERS: usize = 100;

/// The path shape a dedicated endpoint carries.
const PREFIX: &str = "/models/";

/// A parsed request head.
///
/// The identifier and the suffix are split apart at parse time because every
/// caller wants them separately: one selects the entry, the other becomes the
/// path sent upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Head {
    /// The method, passed through unchanged.
    pub(super) method: String,
    /// The model identifier the path named.
    pub(super) id: String,
    /// What the child is asked for, with the prefix and identifier removed.
    pub(super) suffix: String,
    /// Every header as received, in order.
    pub(super) headers: Vec<(String, String)>,
    /// The declared body length, or zero when there is none.
    pub(super) content_length: usize,
    /// Whether the request announced chunked framing, which this router
    /// refuses rather than guesses at.
    pub(super) chunked: bool,
}

/// Reads the request line and headers, ending at the first blank line.
///
/// # Errors
///
/// Returns a [`Failure`] when the head exceeds either bound, or when the
/// connection ends before a blank line arrives.
pub(super) fn read(reader: &mut impl BufRead) -> Result<Vec<String>, Failure> {
    let mut lines: Vec<String> = Vec::new();
    let mut read = 0usize;
    loop {
        let remaining = MAX_HEAD_BYTES.saturating_sub(read);
        if remaining == 0 {
            return Err(oversized(&format!("longer than {MAX_HEAD_BYTES} bytes")));
        }

        // Bounded at the read rather than after it. Checking the length of a
        // line already in memory would be a limit that allocates whatever it
        // was given before deciding it was too much.
        let mut line = String::new();
        let taken = u64::try_from(remaining).unwrap_or(u64::MAX);
        let count = (&mut *reader)
            .take(taken)
            .read_line(&mut line)
            .map_err(|error| oversized(&format!("unreadable: {error}")))?;
        if count == 0 {
            return Err(oversized("ended before its blank line"));
        }
        read += count;

        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        if trimmed.is_empty() {
            return Ok(lines);
        }
        if lines.len() >= MAX_HEADERS {
            return Err(oversized(&format!("more than {MAX_HEADERS} lines")));
        }
        lines.push(trimmed);
    }
}

/// A head the router will not read, and why.
fn oversized(reason: &str) -> Failure {
    Failure::Unavailable(format!("the request head is {reason}"))
}

/// Turns the lines of a head into the parts the router routes on.
///
/// # Errors
///
/// Returns a [`Failure`] when there is no request line, when the path does
/// not carry the dedicated-endpoint shape, or when it names an identifier
/// with nothing after it.
pub(super) fn parse(lines: &[String]) -> Result<Head, Failure> {
    let mut words = lines
        .first()
        .ok_or_else(|| malformed("a request with no request line"))?
        .split_whitespace();
    let method = words
        .next()
        .ok_or_else(|| malformed("a request line with no method"))?
        .to_owned();
    let path = words
        .next()
        .ok_or_else(|| malformed("a request line with no path"))?;

    let (id, suffix) = split(path)?;

    let mut headers = Vec::new();
    let mut content_length = 0;
    let mut chunked = false;
    for line in lines.iter().skip(1) {
        // A line with no colon is not a header. Skipped rather than refused:
        // the router is a relay, and inventing a rule the child does not have
        // would refuse requests the child would have answered.
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
        headers.push((name.to_owned(), value.to_owned()));
    }

    Ok(Head {
        method,
        id,
        suffix,
        headers,
        content_length,
        chunked,
    })
}

/// The identifier a path names, and what the child is asked for.
fn split(path: &str) -> Result<(String, String), Failure> {
    let shape = format!("the shape is {PREFIX}<model>/<path>");
    let rest = path
        .strip_prefix(PREFIX)
        .ok_or_else(|| malformed(&format!("'{path}' is not a dedicated endpoint: {shape}")))?;

    match rest.split_once('/') {
        Some((id, suffix)) if !id.is_empty() && !suffix.is_empty() => {
            Ok((id.to_owned(), format!("/{suffix}")))
        }
        _ => Err(malformed(&format!(
            "'{path}' names a model with nothing after it: {shape}"
        ))),
    }
}

/// A request this router does not serve, and the shape of one it does.
fn malformed(reason: &str) -> Failure {
    Failure::Unavailable(reason.to_owned())
}

impl Head {
    /// The head to send upstream: the same method, the suffix as the path,
    /// pointed at the child and asked to close when done.
    ///
    /// Every other header is passed through as received. The router is not a
    /// participant in the conversation, only a relay for it.
    pub(super) fn rewrite(&self, upstream: SocketAddr) -> String {
        let mut text = String::new();
        let method = &self.method;
        let suffix = &self.suffix;
        // Writing to a String cannot fail, so this says so once rather than
        // dressing an impossibility up as an error this function returns.
        let infallible = "writing to a String cannot fail";
        write!(text, "{method} {suffix} HTTP/1.1\r\n").expect(infallible);
        write!(text, "Host: {upstream}\r\n").expect(infallible);
        text.push_str("Connection: close\r\n");

        for (name, value) in &self.headers {
            // The caller's Host named the router, and its Connection was about
            // the router's connection. Both have been answered above with the
            // child's, so passing the originals through would send the child
            // two of each.
            if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("connection") {
                continue;
            }
            write!(text, "{name}: {value}\r\n").expect(infallible);
        }

        text.push_str("\r\n");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn lines(head: &[&str]) -> Vec<String> {
        head.iter().map(|line| (*line).to_owned()).collect()
    }

    fn upstream() -> SocketAddr {
        "127.0.0.1:41234".parse().expect("a loopback address")
    }

    fn head_of(path: &str) -> Head {
        parse(&lines(&[
            &format!("POST {path} HTTP/1.1"),
            "Host: 127.0.0.1:8080",
        ]))
        .expect("a well-formed head")
    }

    #[test]
    fn a_path_splits_into_an_identifier_and_a_suffix() {
        let head = head_of("/models/gemma3/v1/chat/completions");

        assert_eq!(head.id, "gemma3", "the segment after the prefix selects");
        assert_eq!(
            head.suffix, "/v1/chat/completions",
            "and everything after it is what the child is asked for"
        );
        assert_eq!(head.method, "POST", "the method is passed through");
    }

    #[test]
    fn a_path_that_does_not_name_models_is_refused() {
        let refusal = parse(&lines(&["POST /v1/chat/completions HTTP/1.1"]))
            .expect_err("only the dedicated shape is served in this slice");

        assert!(
            refusal.to_string().contains("/models/"),
            "the refusal says what shape was expected: {refusal}"
        );
    }

    #[test]
    fn a_path_with_no_suffix_after_the_identifier_is_refused() {
        for path in ["/models/gemma3", "/models/gemma3/"] {
            let refusal = parse(&lines(&[&format!("GET {path} HTTP/1.1")]))
                .expect_err("an identifier with nothing after it asks for nothing");
            assert!(
                refusal.to_string().contains("gemma3"),
                "the refusal names what was asked for: {refusal}"
            );
        }
    }

    #[test]
    fn an_empty_head_is_refused_rather_than_assumed() {
        parse(&[]).expect_err("a head with no request line names nothing");
    }

    #[test]
    fn content_length_is_read_whatever_its_spelling() {
        let head = parse(&lines(&[
            "POST /models/gemma3/v1/chat/completions HTTP/1.1",
            "content-length: 42",
        ]))
        .expect("a well-formed head");

        assert_eq!(
            head.content_length, 42,
            "a client is entitled to send a lowercase header name"
        );
    }

    #[test]
    fn a_head_without_a_length_carries_no_body() {
        assert_eq!(head_of("/models/gemma3/v1/models").content_length, 0);
    }

    #[test]
    fn chunked_transfer_encoding_is_detected() {
        let head = parse(&lines(&[
            "POST /models/gemma3/v1/chat/completions HTTP/1.1",
            "Transfer-Encoding: chunked",
        ]))
        .expect("a well-formed head");

        assert!(
            head.chunked,
            "detected here so the caller can refuse it rather than mangle a body"
        );
        assert!(
            !head_of("/models/gemma3/v1/models").chunked,
            "and absent when it was not announced"
        );
    }

    #[test]
    fn the_rewritten_head_keeps_the_method_and_carries_the_suffix() {
        let rewritten = head_of("/models/gemma3/v1/chat/completions").rewrite(upstream());

        assert!(
            rewritten.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
            "the child is asked for the path without the prefix:\n{rewritten}"
        );
    }

    #[test]
    fn the_rewritten_head_points_at_the_child_and_asks_it_to_close() {
        let rewritten = head_of("/models/gemma3/v1/chat/completions").rewrite(upstream());

        assert!(
            rewritten.contains("Host: 127.0.0.1:41234\r\n"),
            "Host names the child, not the router:\n{rewritten}"
        );
        assert!(
            rewritten.contains("Connection: close\r\n"),
            "so the response ends at end-of-file and needs no chunk parsing:\n{rewritten}"
        );
        assert!(
            rewritten.ends_with("\r\n\r\n"),
            "and the head is terminated:\n{rewritten}"
        );
    }

    #[test]
    fn the_rewritten_head_preserves_every_other_header_as_received() {
        let head = parse(&lines(&[
            "POST /models/gemma3/v1/chat/completions HTTP/1.1",
            "Host: 127.0.0.1:8080",
            "Content-Type: application/json",
            "Content-Length: 17",
            "X-Written-By: the test",
        ]))
        .expect("a well-formed head");
        let rewritten = head.rewrite(upstream());

        assert!(
            rewritten.contains("Content-Type: application/json\r\n"),
            "the router is a relay, not a participant:\n{rewritten}"
        );
        assert!(
            rewritten.contains("X-Written-By: the test\r\n"),
            "including headers it has never heard of:\n{rewritten}"
        );
        assert!(
            rewritten.contains("Content-Length: 17\r\n"),
            "the body it forwards is still the length the caller declared:\n{rewritten}"
        );
    }

    #[test]
    fn the_original_host_and_connection_headers_do_not_survive() {
        let head = parse(&lines(&[
            "POST /models/gemma3/v1/chat/completions HTTP/1.1",
            "Host: 127.0.0.1:8080",
            "connection: keep-alive",
        ]))
        .expect("a well-formed head");
        let rewritten = head.rewrite(upstream());

        assert!(
            !rewritten.contains("127.0.0.1:8080"),
            "the caller's Host named the router, and the child is not it:\n{rewritten}"
        );
        assert!(
            !rewritten.to_lowercase().contains("keep-alive"),
            "the upstream connection closes, whatever the caller asked of the \
             router:\n{rewritten}"
        );
    }

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

        read(&mut source).expect_err("a bad client cannot make the router allocate");
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

        read(&mut source).expect_err("a truncated head is not a head");
    }
}
