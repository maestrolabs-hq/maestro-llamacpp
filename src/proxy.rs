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
//! [`Router::serve`] runs until the process ends. It does not return.
//!
//! [`Router::stop`] ends the children it started. This exists because a child
//! is a separate process and nothing in the operating system ties its lifetime
//! to this one: a router that is never dropped -- which is every router whose
//! `serve` is still running -- leaves its children alive after the process
//! that started them is gone. That was measured, not assumed: forty-five stub
//! servers outlived the test binaries that started them before this method
//! existed. A caller that ends without calling it leaves them behind.
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

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crate::admission::Budget;
use crate::catalog::{Catalog, Entry};
use crate::launch::{Child, Failure, Server};

mod answer;
mod body;
mod endpoint;
mod head;
mod loaded;
mod relay;
mod slots;

use slots::Slots;

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
    slots: Slots,
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
        budget: Budget,
    ) -> Result<Self, Failure> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unavailable(format!(
                "refusing to bind {address}: serving a network is a security \
                 design this repository has not written"
            )));
        }

        let listener = TcpListener::bind(address)
            .map_err(|error| Failure::Unavailable(format!("cannot bind {address}: {error}")))?;

        let slots = Slots::new(&catalog, budget);

        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                catalog,
                root,
                server,
                slots,
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

    /// Stops every child this router started, and forgets them.
    ///
    /// A child still relaying a response is held by that relay and stops when
    /// it finishes, which is the behaviour a caller wants: this ends the
    /// router's claim on its children, not the answer somebody is reading.
    ///
    /// The next request for an entry starts a fresh child, so this is safe to
    /// call on a router that goes on serving.
    pub fn stop(&self) {
        self.shared.slots.clear();
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
                drop(answer::to(&shared, stream));
            });
        }
    }
}

impl Shared {
    /// The child serving this entry, started if there is room for it.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when a child cannot be started, does not become
    /// ready, or is refused for want of room.
    fn child(&self, entry: &Entry) -> Result<Arc<Child>, Failure> {
        self.slots
            .child(&self.catalog, entry, &self.server, &self.root)
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
