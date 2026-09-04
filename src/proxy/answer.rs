//! Answering one connection.
//!
//! Split from the module beside it when that file grew past the module-size
//! gate, along the seam the gate exposed: `proxy` carries the type a caller
//! holds, and this carries what one connection is answered with.
//!
//! Everything here happens before a byte of a child's response has been
//! forwarded, which is what makes a status still possible. Once the relay
//! starts, it does not come back here.

use std::io::{BufReader, Write};
use std::net::TcpStream;

use super::endpoint::Endpoint;
use super::head::Length;
use super::{Shared, body, head, relay};
use crate::catalog::Entry;
use crate::launch::{Child, Failure};
use std::sync::Arc;

/// Answers one connection.
pub(super) fn to(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let lines = match head::read(&mut reader) {
        Ok(lines) => lines,
        Err(failure) => return refuse(&mut stream, 400, &failure.to_string()),
    };
    let request = match head::parse(&lines) {
        Ok(request) => request,
        Err(failure) => return refuse(&mut stream, 404, &failure.to_string()),
    };

    // Framing first, before anything is read, looked up or started. A request
    // this router cannot frame is one it will not serve whatever it names, so
    // deciding that here costs neither a body nor a model load.
    if request.chunked {
        // Named as precisely as the request allows: a dedicated path carries
        // its entry, and a generic one keeps its model in the body this
        // refusal is declining to read.
        let about = match &request.endpoint {
            Endpoint::Dedicated { id, .. } => format!("entry '{id}'"),
            Endpoint::Generic { .. } => "the generic endpoint".to_owned(),
            Endpoint::Listing => "the model listing".to_owned(),
        };
        return refuse(
            &mut stream,
            501,
            &format!(
                "{about}: this router does not implement chunked request \
                 bodies; send a body with a Content-Length"
            ),
        );
    }

    // Framing still, and for the same reason: a length this router cannot read
    // is a request it cannot honour whatever it names. Defaulting it to zero
    // was worse than refusing -- the dedicated endpoint would forward the
    // header as received and leave the child waiting for a body nobody was
    // going to send, and the generic one would refuse the empty result for not
    // being JSON, which names the wrong thing entirely.
    if let Length::Malformed(value) = &request.length {
        return refuse(
            &mut stream,
            400,
            &format!(
                "'Content-Length: {value}' is not a length this router can \
                 read; send a byte count, or no such header at all"
            ),
        );
    }

    // The listing is the router's own answer, so it is settled before
    // anything is read from the body or started on a child's behalf.
    if matches!(request.endpoint, Endpoint::Listing) {
        return list(&mut stream, shared);
    }

    // Which model answers, and the body if reading it was what said so. The
    // dedicated endpoint names its model in the path and never looks, which
    // is why only one of these two arms buffers anything.
    let (wanted, buffered) = match &request.endpoint {
        Endpoint::Dedicated { id, .. } => (id.clone(), None),
        Endpoint::Listing => unreachable!("answered above"),
        Endpoint::Generic { .. } => {
            // This endpoint has nothing else to route on, so a request with no
            // declared body is one it can never answer. Said as the missing
            // header rather than as a parser complaining about an empty slice,
            // which is what a caller sending `GET /v1/models/gemma3` would
            // otherwise be told.
            let Length::Given(declared) = request.length else {
                return refuse(
                    &mut stream,
                    411,
                    "the generic endpoint routes on the 'model' field of the \
                     request body, so it needs one and a Content-Length that \
                     declares it; or address a model directly at \
                     /models/<model>/<path>",
                );
            };

            // The one place this bound is enforced, and the only place it can
            // be: `body::read` allocates what it is told to and has no status
            // left to refuse with. Checked before reading rather than after,
            // because taking the memory and then objecting to it is the bug
            // the bound exists to prevent.
            if declared > body::MAX_BODY_BYTES {
                return refuse(
                    &mut stream,
                    413,
                    &format!(
                        "a request body of {declared} bytes is larger than the \
                         {} this router will read",
                        body::MAX_BODY_BYTES
                    ),
                );
            }
            match body::read(&mut reader, declared) {
                Ok((bytes, model)) => (model, Some(bytes)),
                Err(failure) => return refuse(&mut stream, 400, &failure.to_string()),
            }
        }
    };

    let Some(entry) = shared.catalog.entry(&wanted) else {
        return refuse(
            &mut stream,
            404,
            &format!(
                "no model called '{wanted}'; this catalog carries: {}",
                shared.known()
            ),
        );
    };

    // The two causes are distinguished by launch::Failure's variants rather
    // than by reading its message, so the wording of an error is not a
    // load-bearing interface.
    let child = match shared.child(entry) {
        Ok(child) => child,
        Err(Failure::NotReady(message)) => return refuse(&mut stream, 504, &message),
        Err(Failure::Unavailable(message)) => return refuse(&mut stream, 502, &message),
        // Nothing was attempted and nothing is broken, so this is the one
        // refusal a caller can act on by waiting: 503 rather than 502.
        Err(Failure::Refused(message)) => return refuse(&mut stream, 503, &message),
    };

    relay::run(
        &request,
        &child,
        buffered.as_deref(),
        &mut reader,
        &mut stream,
    )
}

/// Every entry the catalog carries, in the shape a client expects.
///
/// Answered from the catalog and nothing else: listing what can be served is
/// not a reason to start serving it, so no child is touched.
fn list(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
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

    reply(stream, 200, "OK", "application/json", &body)
}

/// The router's own answer, as a complete reply with a declared length.
///
/// Distinct from anything relayed: these are the refusals that happen before a
/// single byte of a child's response has been forwarded, which is what makes a
/// status still possible.
fn refuse(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Content Too Large",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Gateway Timeout",
    };
    reply(stream, status, reason, "text/plain", message)
}

impl Shared {
    /// The child serving this entry, started if there is room for it.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when a child cannot be started, does not become
    /// ready, or is refused for want of room.
    pub(super) fn child(&self, entry: &Entry) -> Result<Arc<Child>, Failure> {
        self.slots
            .child(&self.catalog, entry, &self.server, &self.root)
    }

    /// What the catalog carries, for a refusal that can be acted on.
    pub(super) fn known(&self) -> String {
        self.catalog
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Writes one reply the router authored, rather than one it relayed.
///
/// The framing is the same whatever the answer is: a declared length and a
/// closed connection, because a caller that can rely on neither has to guess
/// where the reply ended. Only the status, the type and the body differ, so
/// only those are asked for -- which is what keeps the two callers above about
/// what they answer rather than about how a reply is shaped.
fn reply(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )?;
    stream.flush()
}
