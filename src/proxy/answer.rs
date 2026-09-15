//! Answering one connection.
//!
//! Split from the module beside it when that file grew past the module-size
//! gate, along the seam the gate exposed: `proxy` carries the type a caller
//! holds, and this carries what one connection is answered with. What that
//! answer looks like on the wire is `reply`'s business; this decides which
//! answer a request has earned.
//!
//! Everything here happens before a byte of a child's response has been
//! forwarded, which is what makes a status still possible. Once the relay
//! starts, it does not come back here.

use std::io::BufReader;
use std::net::TcpStream;

mod own;

use super::endpoint::Endpoint;
use super::head::{Head, Length};
use super::refusal::{Cause, Refusal};
use super::{Shared, body, head, relay, reply};

/// Answers one connection.
pub(super) fn to(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let lines = match head::read(&mut reader) {
        Ok(lines) => lines,
        Err(refusal) => return reply::refuse(&mut stream, &refusal),
    };
    let request = match head::parse(&lines) {
        Ok(request) => request,
        Err(refusal) => return reply::refuse(&mut stream, &refusal),
    };

    // A preflight asks what is allowed, which the router knows without
    // asking a child. Answered before framing is checked: a preflight has no
    // body, and one that declared something odd is still a preflight.
    if request.method == "OPTIONS" {
        return reply::preflight(&mut stream, &request.endpoint);
    }

    // Before anything else that could start a child. A method no model can
    // be asked anything with is refused here rather than forwarded, because
    // the only thing forwarding it could achieve is a load that answers 404.
    if !request.endpoint.allows(&request.method) {
        let allowed = request.endpoint.allowed();
        return reply::refuse(
            &mut stream,
            &Refusal::new(
                Cause::MethodNotAllowed(allowed),
                format!(
                    "{} is not a method this endpoint answers; it accepts {allowed}",
                    request.method
                ),
            ),
        );
    }

    // Framing first, before anything is read, looked up or started. A request
    // this router cannot frame is one it will not serve whatever it names, so
    // deciding that here costs neither a body nor a model load.
    if let Err(refusal) = framing(&request) {
        return reply::refuse(&mut stream, &refusal);
    }

    // The router's own answers, settled before anything is read from the body
    // or started on a child's behalf: saying what could be served is never a
    // reason to start serving it.
    //
    // All three honour `HEAD` the same way, because a client sizing a buffer
    // from the declared length is doing so for the same reason whichever of
    // them it asked.
    let head_only = request.method == "HEAD";
    match request.endpoint {
        Endpoint::Listing => return reply::listing(&mut stream, shared, head_only),
        Endpoint::Catalogue => return own::catalogue(&mut stream, shared, head_only),
        Endpoint::Properties => return own::properties(&mut stream, head_only),
        // Residency is the one answer here that changes something. It is
        // still the router's own, settled without a child and without
        // starting one: the only direction it moves is towards less loaded.
        Endpoint::Residency { ref id } => return own::unload(&mut stream, shared, id, head_only),
        Endpoint::Dedicated { .. } | Endpoint::Generic { .. } => {}
    }

    // Which model answers, and the body if reading it was what said so. The
    // dedicated endpoint names its model in the path and never looks, which
    // is why only one of these two arms buffers anything.
    let (wanted, buffered) = match &request.endpoint {
        Endpoint::Dedicated { id, .. } => (id.clone(), None),
        Endpoint::Listing
        | Endpoint::Catalogue
        | Endpoint::Properties
        | Endpoint::Residency { .. } => {
            unreachable!("answered above")
        }
        Endpoint::Generic { .. } => {
            let declared = match declared_body(&request.length) {
                Ok(declared) => declared,
                Err(refusal) => return reply::refuse(&mut stream, &refusal),
            };
            // Told to send now and not before: a body refused for its length
            // is one the caller was spared sending.
            if request.expects_continue {
                reply::proceed(&mut stream)?;
            }
            match body::read(&mut reader, declared) {
                Ok((bytes, model)) => (model, Some(bytes)),
                Err(refusal) => return reply::refuse(&mut stream, &refusal),
            }
        }
    };

    let Some(entry) = shared.catalog.entry(&wanted) else {
        return reply::refuse(
            &mut stream,
            &Refusal::new(
                Cause::ModelNotFound,
                format!(
                    "no model called '{wanted}'; this catalog carries: {}",
                    shared.known()
                ),
            ),
        );
    };

    // The causes are distinguished by launch::Failure's variants rather than
    // by reading its message, so the wording of an error is not a
    // load-bearing interface.
    let child = match shared.child(entry) {
        Ok(child) => child,
        Err(failure) => return reply::refuse(&mut stream, &Refusal::from(failure)),
    };

    // The dedicated endpoint reads the body only now, to hand it to a child
    // that is ready for it, so a caller holding its body back is told to
    // send only now. A load that took minutes has already outlasted the
    // wait `curl` gives this before sending anyway, and that is harmless: the
    // body is on the socket either way, and the interim line is ignored by a
    // client that stopped waiting for it.
    if buffered.is_none() && request.expects_continue && request.body_bytes() > 0 {
        reply::proceed(&mut stream)?;
    }

    // The relay is timed by when it ends rather than when it started, so a
    // response that takes longer than the idle window does not make its own
    // model look idle for the whole of its own duration. `child` is still in
    // scope while this runs, which is load-bearing: its `Arc` keeps the
    // slot's strong count at two or more, so no sweep can empty it between
    // the relay ending and the touch landing.
    let outcome = relay::run(
        &request,
        &child,
        buffered.as_deref(),
        &mut reader,
        &mut stream,
    );
    shared.slots.touch(&entry.id);
    outcome
}

/// Refuses the framing this router does not implement, or cannot read.
///
/// Chunked bodies are refused rather than guessed at. A length that will not
/// parse is refused rather than defaulted to zero, which was worse: the
/// dedicated endpoint would forward the header as received and leave the
/// child waiting for a body nobody was going to send, and the generic one
/// would refuse the empty result for not being JSON, which names the wrong
/// thing entirely.
fn framing(request: &Head) -> Result<(), Refusal> {
    if request.chunked {
        // Named as precisely as the request allows: a dedicated path carries
        // its entry, and a generic one keeps its model in the body this
        // refusal is declining to read.
        let about = match &request.endpoint {
            Endpoint::Dedicated { id, .. } => format!("entry '{id}'"),
            Endpoint::Generic { .. } => "the generic endpoint".to_owned(),
            Endpoint::Listing => "the model listing".to_owned(),
            Endpoint::Catalogue => "the catalogue".to_owned(),
            Endpoint::Properties => "the server's properties".to_owned(),
            Endpoint::Residency { id } => format!("the residency of '{id}'"),
        };
        return Err(Refusal::new(
            Cause::ChunkedBody,
            format!(
                "{about}: this router does not implement chunked request \
                 bodies; send a body with a Content-Length"
            ),
        ));
    }
    if let Length::Malformed(value) = &request.length {
        return Err(Refusal::new(
            Cause::MalformedLength,
            format!(
                "'Content-Length: {value}' is not a length this router can \
                 read; send a byte count, or no such header at all"
            ),
        ));
    }
    Ok(())
}

/// How long a body the generic endpoint is about to read, if it may.
///
/// Only this endpoint reads a body, because only this endpoint has nothing
/// else to route on -- so a request with no declared body is one it can never
/// answer. Said as the missing header rather than as a parser complaining
/// about an empty slice, which is what a caller sending `GET /v1/models/gemma3`
/// would otherwise be told.
fn declared_body(length: &Length) -> Result<usize, Refusal> {
    let Length::Given(declared) = *length else {
        return Err(Refusal::new(
            Cause::LengthRequired,
            "the generic endpoint routes on the 'model' field of the request \
             body, so it needs one and a Content-Length that declares it; or \
             address a model directly at /models/<model>/<path>",
        ));
    };

    // The one place this bound is enforced, and the only place it can be:
    // `body::read` allocates what it is told to and has no status left to
    // refuse with. Checked before reading rather than after, because taking
    // the memory and then objecting to it is the bug the bound exists to
    // prevent.
    if declared > body::MAX_BODY_BYTES {
        return Err(Refusal::new(
            Cause::BodyTooLarge,
            format!(
                "a request body of {declared} bytes is larger than the {} this \
                 router will read",
                body::MAX_BODY_BYTES
            ),
        ));
    }
    Ok(declared)
}
