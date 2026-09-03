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

use super::{Shared, head, relay};
use crate::launch::Failure;

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

    let Some(entry) = shared.catalog.entry(&request.id) else {
        let id = &request.id;
        return refuse(
            &mut stream,
            404,
            &format!(
                "no model called '{id}'; this catalog carries: {}",
                shared.known()
            ),
        );
    };

    // Refused before anything is started, so a request this router will not
    // serve does not cost a model load.
    if request.chunked {
        let id = &request.id;
        return refuse(
            &mut stream,
            501,
            &format!(
                "entry '{id}': this router does not implement chunked request \
                 bodies; send a body with a Content-Length"
            ),
        );
    }

    // The two causes are distinguished by launch::Failure's variants rather
    // than by reading its message, so the wording of an error is not a
    // load-bearing interface.
    let child = match shared.child(entry) {
        Ok(child) => child,
        Err(Failure::NotReady(message)) => return refuse(&mut stream, 504, &message),
        Err(Failure::Unavailable(message)) => return refuse(&mut stream, 502, &message),
    };

    relay::run(&request, &child, &mut reader, &mut stream)
}

/// The router's own answer, as a complete reply with a declared length.
///
/// Distinct from anything relayed: these are the four refusals that happen
/// before a single byte of a child's response has been forwarded, which is
/// what makes a status still possible.
fn refuse(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        _ => "Gateway Timeout",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {message}",
        message.len()
    )?;
    stream.flush()
}
