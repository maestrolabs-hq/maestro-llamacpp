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

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::launch::{Failure, Server};

// Everything below is written and unit-tested before the relay that calls it,
// so in this commit the module has no consumer outside its own tests and the
// lib build sees it as dead. The allowance comes off in Task 4, when `serve`
// reads a head for real; it is here rather than in a hook bypass so that the
// temporary state is visible in the source.
#[allow(dead_code)]
mod head;

/// The public listener, and everything a request needs to be answered.
pub struct Router {
    listener: TcpListener,
}

impl Router {
    /// Reserves the public port.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the address is not loopback, or when the
    /// port cannot be bound.
    pub fn bind(
        _address: SocketAddr,
        _catalog: Catalog,
        _root: PathBuf,
        _server: Server,
    ) -> Result<Self, Failure> {
        todo!("Task 4")
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
    pub fn serve(&self) {
        todo!("Task 4")
    }
}
