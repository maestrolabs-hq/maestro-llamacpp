//! Serving one dedicated endpoint per model, and relaying it to a child.
//!
//! What a caller must know to use this correctly is five things.
//!
//! [`Router::bind`] reserves the public port and returns immediately. Nothing
//! is served and no child is started until [`Router::serve`] runs, so a caller
//! can learn its address and arrange whatever it needs before traffic begins.
//!
//! [`Router::address`] reports what the operating system gave. A caller that
//! asked for port zero -- a test, usually -- has no other way to find out.
//!
//! [`Router::serve`] runs until the process ends. It does not return, and
//! there is no stop: the router's lifetime is the process's lifetime, and
//! children die with it because [`crate::launch::Child`] says so.
//!
//! The router binds loopback only, as children do. Serving a network is a
//! security design this repository has not written, and a caller that asks for
//! one is refused rather than accommodated.
//!
//! The router reads a request head and copies everything else. That is the
//! decision this module exists to hold: what it does not parse, it cannot
//! buffer, so a streamed response reaches the caller as it arrives rather than
//! when it ends.
//!
//! **This is not a general-purpose HTTP server.** It reads a bounded head,
//! refuses the framing it does not implement, and serves one machine's own
//! traffic on loopback. Nothing here is hardened for anything else, and
//! extending it as though it were is the mistake this paragraph exists to
//! prevent.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use crate::catalog::{Catalog, Entry};
use crate::launch::{Child, Failure, Server};

mod head;

/// The public listener, and everything a request needs to be answered.
pub struct Router {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// What every connection thread shares.
struct Shared {
    catalog: Catalog,
    root: PathBuf,
    server: Server,
    /// One lock over the whole map. A request holds it only long enough to
    /// find or start a child, never across a relay: holding it while bytes
    /// flowed would serialise every caller behind the slowest stream, which
    /// for a generating model is minutes.
    ///
    /// The honest limit is that starting one child blocks lookups for the
    /// others. With one endpoint that is unobservable; slice 4 has several
    /// entries and needs per-entry state.
    children: Mutex<HashMap<String, Arc<Child>>>,
}

impl Router {
    /// Reserves the public port.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the address is not loopback, or when the
    /// port cannot be bound.
    pub fn bind(
        address: SocketAddr,
        catalog: Catalog,
        root: PathBuf,
        server: Server,
    ) -> Result<Self, Failure> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unavailable(format!(
                "refusing to bind {address}: serving a network is a security \
                 design this repository has not written"
            )));
        }

        let listener = TcpListener::bind(address)
            .map_err(|error| Failure::Unavailable(format!("cannot bind {address}: {error}")))?;

        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                catalog,
                root,
                server,
                children: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Where the router is listening, as the operating system assigned it.
    ///
    /// # Panics
    ///
    /// If the listener has no address, which cannot happen: [`Router::bind`]
    /// returns only once one is bound.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("a bound listener has an address")
    }

    /// Accepts connections until the process ends.
    ///
    /// One thread per connection, because a streamed response occupies its
    /// thread for as long as the answer takes and this router serves one
    /// machine.
    pub fn serve(&self) {
        for stream in self.listener.incoming().flatten() {
            let shared = Arc::clone(&self.shared);
            thread::spawn(move || {
                // A failed answer is a caller that hung up, which is its own
                // business. The next connection is what matters.
                drop(answer(&shared, stream));
            });
        }
    }
}

impl Shared {
    /// The child serving this entry, started if this is the first request.
    ///
    /// The lock is released by returning: the handle is cloned out so the
    /// relay runs without it.
    fn child(&self, entry: &Entry) -> Result<Arc<Child>, Failure> {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(child) = children.get(&entry.id) {
            return Ok(Arc::clone(child));
        }

        let child = Arc::new(self.server.start(entry, &self.root)?);
        children.insert(entry.id.clone(), Arc::clone(&child));
        Ok(child)
    }

    /// What the catalog carries, for a refusal that can be acted on.
    fn known(&self) -> String {
        self.catalog
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Answers one connection.
fn answer(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
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

    let _ = (&request, &child, &mut reader);
    todo!("Task 5: the relay")
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
