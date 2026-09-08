//! The request head: what the router reads of a request, and what it sends on.
//!
//! Pure translation. Lines in, a parsed head out, and a rewritten head back to
//! bytes -- no sockets, no processes, no clock. That is what lets every rule
//! below be asserted directly rather than inferred from a running relay. The
//! one step that does touch a socket, reading the lines within bounds, is
//! `read` beside this and is re-exported from here so a caller asks one
//! module for a head.
//!
//! This is the only part of a request the router understands. Everything after
//! the blank line is copied without being read, which is the decision the
//! whole slice rests on: what the router does not parse, it cannot buffer.

use std::fmt::Write as _;
use std::net::SocketAddr;

use super::endpoint::Endpoint;
use super::refusal::{Cause, Refusal};

mod read;
pub(super) use read::{read, timed_out};

/// What a request said about the length of its body.
///
/// Three states rather than a number, because they are three different
/// requests and a router that folded them into one would answer two of them
/// wrongly. Saying nothing is a request with no body. Saying something
/// unreadable is a request this router cannot honour -- forwarding it would
/// hand the child a declared length with nothing behind it, and defaulting it
/// to zero would refuse the request later for something else entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Length {
    /// No `Content-Length` header at all.
    Absent,
    /// A length this router can act on.
    Given(usize),
    /// A header that will not parse, kept as received so a refusal can quote
    /// back what arrived.
    Malformed(String),
}

/// A parsed request head.
///
/// The endpoint is resolved at parse time because every caller wants it: it
/// says which model answers, and what the child is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Head {
    /// The method, passed through unchanged.
    pub(super) method: String,
    /// Which endpoint the path addressed.
    pub(super) endpoint: Endpoint,
    /// Every header as received, in order.
    pub(super) headers: Vec<(String, String)>,
    /// What the request said about its body's length.
    pub(super) length: Length,
    /// Whether the request announced chunked framing, which this router
    /// refuses rather than guesses at.
    pub(super) chunked: bool,
    /// Whether the caller is holding its body back until it is told to send
    /// it, as `Expect: 100-continue` says.
    ///
    /// The router's expectation to meet rather than the child's: the body is
    /// read here, on the generic endpoint before any child is involved, so
    /// the interim answer has to come from here and the header does not
    /// travel on.
    pub(super) expects_continue: bool,
}

/// Turns the lines of a head into the parts the router routes on.
///
/// # Errors
///
/// Returns a [`Refusal`] when there is no request line, or when the path is
/// not a shape this router serves.
pub(super) fn parse(lines: &[String]) -> Result<Head, Refusal> {
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

    let endpoint = Endpoint::of(path)?;

    let mut headers = Vec::new();
    let mut length = Length::Absent;
    let mut chunked = false;
    let mut expects_continue = false;
    for line in lines.iter().skip(1) {
        // A line with no colon is not a header. Skipped rather than refused:
        // the router is a relay, and inventing a rule the child does not have
        // would refuse requests the child would have answered.
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-length") {
            // Kept rather than defaulted. A length that will not parse is a
            // fact about the request, and the one caller that can still send a
            // status is the one that should decide what to do about it.
            length = value
                .parse()
                .map_or_else(|_| Length::Malformed(value.to_owned()), Length::Given);
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
        if name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue") {
            expects_continue = true;
        }
        headers.push((name.to_owned(), value.to_owned()));
    }

    Ok(Head {
        method,
        endpoint,
        headers,
        length,
        chunked,
        expects_continue,
    })
}

/// A request this router cannot read as one.
fn malformed(reason: &str) -> Refusal {
    Refusal::new(Cause::MalformedRequest, reason)
}

impl Head {
    /// How many bytes of body to copy, which is none unless one was declared.
    ///
    /// A malformed length never reaches here: it is refused where a status is
    /// still possible, which is before anything is forwarded.
    pub(super) fn body_bytes(&self) -> usize {
        match self.length {
            Length::Given(bytes) => bytes,
            Length::Absent | Length::Malformed(_) => 0,
        }
    }

    /// The head to send upstream: the same method, the suffix as the path,
    /// pointed at the child and asked to close when done.
    ///
    /// Every other header is passed through as received. The router is not a
    /// participant in the conversation, only a relay for it.
    pub(super) fn rewrite(&self, upstream: SocketAddr) -> String {
        let mut text = String::new();
        let method = &self.method;
        let suffix = self.endpoint.suffix();
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
            // two of each. Its Expect was met by the router before the body
            // was read, so the child is not asked to meet it again.
            if name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("connection")
                || name.eq_ignore_ascii_case("expect")
            {
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
    fn a_head_carries_the_endpoint_its_path_addressed() {
        let head = head_of("/models/gemma3/v1/chat/completions");

        assert_eq!(
            head.endpoint,
            Endpoint::Dedicated {
                id: "gemma3".to_owned(),
                suffix: "/v1/chat/completions".to_owned(),
            },
            "which shape a path addressed is resolved at parse time"
        );
        assert_eq!(head.method, "POST", "the method is passed through");
    }

    #[test]
    fn a_path_that_is_no_shape_the_router_serves_is_refused() {
        let refusal = parse(&lines(&["POST /health HTTP/1.1"]))
            .expect_err("the router serves two shapes and no others");

        assert!(
            refusal.to_string().contains("/models/"),
            "the refusal says what shapes were expected: {refusal}"
        );
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
            head.length,
            Length::Given(42),
            "a client is entitled to send a lowercase header name"
        );
    }

    #[test]
    fn a_head_without_a_length_says_so_rather_than_declaring_nothing() {
        assert_eq!(
            head_of("/models/gemma3/v1/echo").length,
            Length::Absent,
            "absent and zero are different requests, and only one of them is \
             a caller that forgot a header"
        );
    }

    #[test]
    fn a_length_that_will_not_parse_is_kept_rather_than_defaulted() {
        for value in ["abc", "-1", ""] {
            let head = parse(&lines(&[
                "POST /v1/chat/completions HTTP/1.1",
                &format!("Content-Length: {value}"),
            ]))
            .expect("a head the router can still refuse deliberately");

            assert_eq!(
                head.length,
                Length::Malformed(value.to_owned()),
                "defaulting this to zero would forward a head declaring a body \
                 with nothing behind it, and blame whatever failed next"
            );
        }
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
            !head_of("/models/gemma3/v1/echo").chunked,
            "and absent when it was not announced"
        );
    }

    #[test]
    fn an_expectation_is_read_here_and_does_not_travel_on() {
        let head = parse(&lines(&[
            "POST /models/gemma3/v1/chat/completions HTTP/1.1",
            "Expect: 100-Continue",
        ]))
        .expect("a well-formed head");

        assert!(
            head.expects_continue,
            "the caller is holding its body back, whatever the case of the value"
        );
        assert!(
            !head.rewrite(upstream()).to_lowercase().contains("expect"),
            "the router meets the expectation itself, so the child is not \
             asked to meet it again:\n{}",
            head.rewrite(upstream())
        );
        assert!(
            !head_of("/models/gemma3/v1/echo").expects_continue,
            "and absent when nothing was asked"
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
}
