//! The request head: what the router reads of a request, and what it sends on.
//!
//! Pure translation. Bytes in, a parsed head out, and a rewritten head back to
//! bytes -- no sockets, no processes, no clock. That is what lets every rule
//! below be asserted directly rather than inferred from a running relay.
//!
//! This is the only part of a request the router understands. Everything after
//! the blank line is copied without being read, which is the decision the
//! whole slice rests on: what the router does not parse, it cannot buffer.

use std::io::BufRead;
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
pub(super) fn read(_reader: &mut impl BufRead) -> Result<Vec<String>, Failure> {
    todo!("Task 3")
}

/// Turns the lines of a head into the parts the router routes on.
///
/// # Errors
///
/// Returns a [`Failure`] when there is no request line, when the path does
/// not carry the dedicated-endpoint shape, or when it names an identifier
/// with nothing after it.
pub(super) fn parse(_lines: &[String]) -> Result<Head, Failure> {
    todo!("Task 3")
}

impl Head {
    /// The head to send upstream: the same method, the suffix as the path,
    /// pointed at the child and asked to close when done.
    ///
    /// Every other header is passed through as received. The router is not a
    /// participant in the conversation, only a relay for it.
    pub(super) fn rewrite(&self, _upstream: SocketAddr) -> String {
        todo!("Task 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
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
