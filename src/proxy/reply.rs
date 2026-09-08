//! What the router says in its own voice.
//!
//! Everything a caller can receive from this router that no child produced: a
//! refusal, the model listing, and a preflight answer. Split from `answer`
//! along that seam when the dispatch there grew past the module-size gate:
//! `answer` decides what a connection has earned, and this decides what that
//! looks like on the wire. What a refusal *is* -- its cause, status and code
//! -- is `refusal`'s business; this only writes one.
//!
//! The framing is the same whatever the answer: a declared length, a closed
//! connection, and the origin header that lets a page on this machine read
//! it, because a caller that can rely on none of those has to guess where the
//! reply ended.

use std::fmt::Write as _;
use std::io::Write as _;
use std::net::TcpStream;

use super::Shared;
use super::endpoint::Endpoint;
use super::refusal::{Cause, Refusal};

/// One complete reply the router authored. Only what varies is asked for.
struct Reply<'a> {
    status: u16,
    content_type: Option<&'a str>,
    headers: Vec<(&'static str, String)>,
    body: &'a str,
    /// Whether to send the head alone, as `HEAD` asks. The length declared
    /// is still the body's, so a client can size a buffer from it.
    head_only: bool,
}

impl Reply<'_> {
    fn write(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        let mut text = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        // Writing to a String cannot fail, so this says so once rather than
        // dressing an impossibility up as an error this function returns.
        let infallible = "writing to a String cannot fail";
        if let Some(content_type) = self.content_type {
            write!(text, "Content-Type: {content_type}\r\n").expect(infallible);
        }
        write!(text, "Content-Length: {}\r\n", self.body.len()).expect(infallible);
        for (name, value) in &self.headers {
            write!(text, "{name}: {value}\r\n").expect(infallible);
        }
        text.push_str("Access-Control-Allow-Origin: *\r\n");
        text.push_str("Connection: close\r\n\r\n");
        if !self.head_only {
            text.push_str(self.body);
        }
        stream.write_all(text.as_bytes())?;
        stream.flush()
    }
}

/// The reason phrase a status carries.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Gateway Timeout",
    }
}

/// Refuses a request, before a byte of any child's response was forwarded.
///
/// The headers beyond the framing are the cause's: `Retry-After` when
/// waiting changes the answer, and `Allow` when the method was the problem.
pub(super) fn refuse(stream: &mut TcpStream, refusal: &Refusal) -> std::io::Result<()> {
    let mut headers = Vec::new();
    if let Some(seconds) = refusal.cause().retry_after_seconds() {
        headers.push(("Retry-After", seconds.to_string()));
    }
    if let Cause::MethodNotAllowed(allowed) = refusal.cause() {
        headers.push(("Allow", allowed.to_owned()));
    }
    Reply {
        status: refusal.cause().status(),
        content_type: Some("application/json"),
        headers,
        body: &refusal.envelope(),
        head_only: false,
    }
    .write(stream)
}

/// Every entry the catalog carries, in the shape a client expects.
///
/// Answered from the catalog and nothing else: listing what can be served is
/// not a reason to start serving it, so no child is touched.
pub(super) fn listing(
    stream: &mut TcpStream,
    shared: &Shared,
    head_only: bool,
) -> std::io::Result<()> {
    let data: Vec<serde_json::Value> = shared
        .catalog
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id,
                "object": "model",
                "owned_by": "maestro-llamacpp",
            })
        })
        .collect();
    let body = serde_json::json!({ "object": "list", "data": data }).to_string();
    Reply {
        status: 200,
        content_type: Some("application/json"),
        headers: Vec::new(),
        body: &body,
        head_only,
    }
    .write(stream)
}

/// Answers a preflight: what this endpoint accepts, from any page on this
/// machine.
///
/// Permissive because the router binds loopback only, so the origin that can
/// reach it is already the machine's own. Never a child's business: a
/// preflight asks what is allowed, and the router knows.
pub(super) fn preflight(stream: &mut TcpStream, endpoint: &Endpoint) -> std::io::Result<()> {
    let allowed = endpoint.allowed().to_owned();
    Reply {
        status: 204,
        content_type: None,
        headers: vec![
            ("Allow", allowed.clone()),
            ("Access-Control-Allow-Methods", allowed),
            ("Access-Control-Allow-Headers", "*".to_owned()),
            ("Access-Control-Max-Age", "86400".to_owned()),
        ],
        body: "",
        head_only: true,
    }
    .write(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_a_cause_carries_has_a_reason_phrase_of_its_own() {
        for cause in [
            Cause::MalformedRequest,
            Cause::HeadTooLarge,
            Cause::RequestTimeout,
            Cause::PathNotFound,
            Cause::MethodNotAllowed("GET"),
            Cause::ChunkedBody,
            Cause::LengthRequired,
            Cause::BodyTooLarge,
            Cause::ChildUnavailable,
            Cause::RoomContended,
        ] {
            assert_ne!(
                reason(cause.status()),
                "Gateway Timeout",
                "{cause:?} carries {} and must not fall through to the phrase \
                 kept for 504",
                cause.status()
            );
        }
        assert_eq!(reason(Cause::StartupTimeout.status()), "Gateway Timeout");
    }
}
