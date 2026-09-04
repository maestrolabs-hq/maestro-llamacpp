//! Serving every model in the catalog, and relaying each one to a child.
//!
//! Two shapes reach the same children. A dedicated endpoint names its model in
//! the path; the generic one takes it from the request body, which is what an
//! OpenAI-compatible client already sends. `GET /v1/models` is answered from
//! the catalog without starting anything.
//!
//! What a caller must know to use this correctly is seven things.
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
//! [`Router::stop`] ends the children it started, and also ends idle
//! unloading for the rest of this router's life. This exists because a child
//! is a separate process and nothing in the operating system ties its lifetime
//! to this one: a router that is never dropped -- which is every router whose
//! `serve` is still running -- leaves its children alive after the process
//! that started them is gone. That was measured, not assumed: forty-five stub
//! servers outlived the test binaries that started them before this method
//! existed. A caller that ends without calling it leaves them behind, and a
//! router ended by a signal never gets the chance to call it at all.
//!
//! [`Router::bind`] takes [`Limits`](crate::idle::Limits): a memory budget and
//! an idle window, configured independently, and together these are what can
//! end a process with no caller asking to stop it. Under a budget, a request
//! for a model that does not fit unloads the coldest idle one to make room; a
//! model something is reading from is never chosen, and when the only room is
//! held by one of those the request is refused instead. An idle window ends a
//! process a different way: a background thread unloads an on-demand model
//! nothing has asked for in longer than the window, with no request involved
//! at all. Without a budget nothing is ever unloaded to make room; without a
//! window nothing is ever unloaded for sitting idle.
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
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use crate::catalog::Catalog;
use crate::idle::{IdleWindow, Limits};
use crate::launch::{Failure, Server};

mod answer;
mod body;
mod endpoint;
mod head;
mod loaded;
mod reaper;
mod relay;
mod residents;
mod shared;
mod slots;

use reaper::Stop;
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
    /// Residents the startup loader could not load, as `id: reason`.
    ///
    /// Recorded rather than only printed, because the loader runs on a thread
    /// of its own: what it writes to the output is not reachable from the
    /// caller that started serving, and "a resident that cannot load says so"
    /// is a claim that has to be assertable to be a guarantee.
    resident_failures: Mutex<Vec<String>>,
    /// How long an on-demand, idle entry may go unused before the reaper
    /// unloads it. `None` means the reaper is never spawned at all.
    idle_window: IdleWindow,
    /// Wakes the reaper the moment [`Router::stop`] is called. See
    /// [`reaper::Stop`] for why a `Weak<Shared>` alone is not enough: the
    /// test harness never drops a `Router`, so nothing would ever end it.
    stop: Stop,
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
        limits: Limits,
    ) -> Result<Self, Failure> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unavailable(format!(
                "refusing to bind {address}: serving a network is a security \
                 design this repository has not written"
            )));
        }

        let listener = TcpListener::bind(address)
            .map_err(|error| Failure::Unavailable(format!("cannot bind {address}: {error}")))?;

        let slots = Slots::new(&catalog, limits.budget);

        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                catalog,
                root,
                server,
                slots,
                resident_failures: Mutex::new(Vec::new()),
                idle_window: limits.idle_window,
                stop: Stop::new(),
            }),
        })
    }

    /// Which entries hold a child, by identifier, in catalog order.
    ///
    /// Names rather than handles, deliberately. Without this, "the resident
    /// was loaded at startup" cannot be observed at all: any request that
    /// would reveal the child is also a request that would have started it.
    /// Why it returns names is in [`Slots::loaded_ids`], and it is the slot
    /// invariant rather than a preference.
    #[must_use]
    pub fn loaded(&self) -> Vec<String> {
        self.shared.slots.loaded_ids(&self.shared.catalog)
    }

    /// Residents the startup loader could not load, as `id: reason`.
    ///
    /// Empty when every resident loaded, and empty before [`Router::serve`]
    /// has run: nothing is attempted until there is something to serve.
    #[must_use]
    pub fn resident_failures(&self) -> Vec<String> {
        self.shared
            .resident_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
    /// Also ends idle unloading for the rest of this router's life: the
    /// reaper thread, if one was ever spawned, exits the moment this signals
    /// it rather than sleeping out its interval, and nothing here spawns a
    /// replacement. A model that goes idle after this point is held until
    /// something else unloads it.
    ///
    /// The next request for an entry starts a fresh child, so this is safe to
    /// call on a router that goes on serving.
    pub fn stop(&self) {
        self.shared.slots.clear();
        self.shared.stop.signal();
    }

    /// Accepts connections until the process ends.
    ///
    /// One thread per connection, because a streamed response occupies its
    /// thread for as long as the answer takes and this router serves one
    /// machine.
    ///
    /// Residents load on a thread of their own and the accept loop starts
    /// immediately, so the router answers while they load. Loading first
    /// would be smaller by a thread and is refused: [`Router::bind`] already
    /// reserved the port, so a caller connects successfully into the kernel's
    /// backlog and then waits with nothing to tell it why. A resident
    /// carrying the default startup budget would make that a five-minute
    /// silence from something that looks like a live router.
    ///
    /// What the thread buys is every answer that needs no child: the model
    /// list, a refusal, a route that does not exist. It does not buy the
    /// first request to another entry, which finds nothing loaded, enters
    /// admission, and waits there for the resident's start to return. The
    /// wait is bounded by that entry's startup budget rather than removed,
    /// so the silence moved from the listener to the first load.
    pub fn serve(&self) {
        let loading = Arc::clone(&self.shared);
        thread::spawn(move || residents::load(&loading));

        // No window, no thread -- a machine that has not asked for this pays
        // nothing for it, not even a sleeping thread.
        if self.shared.idle_window.duration().is_some() {
            let reaping = Arc::downgrade(&self.shared);
            thread::spawn(move || reaper::run(&reaping));
        }

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
