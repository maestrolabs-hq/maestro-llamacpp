//! The thread that unloads what the idle window has expired.
//!
//! Modelled on `residents.rs`: it reaches children through the same path a
//! request does, and adds no coordination of its own. The one thing that
//! module does not need and this one does is a way to end -- `residents::load`
//! runs once and returns; this is the first thread this router spawns that
//! does not, so it needs a signal to stop it rather than an end it reaches on
//! its own.

use std::sync::{Condvar, Mutex, PoisonError, Weak};
use std::time::Duration;

use super::Shared;

/// The floor under the derived sweep interval, in the sense decision 5 of the
/// idle-unload plan states it: half the window, floored here rather than at a
/// round second, so the timing guarantee -- at most one and a half windows
/// plus one sweep -- holds for every window the configured variable can
/// express.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Wakes the reaper the moment [`super::Router::stop`] is called, rather than
/// leaving it to sleep blind until its next scheduled sweep.
///
/// A `Condvar` beside a `Mutex<bool>`, because a plain sleep cannot be
/// interrupted and a `Weak` alone is never dropped in the harness that runs
/// every test in this repository -- `serve` never returns, so nothing ever
/// drops the `Arc<Shared>` the reaper is watching.
pub(super) struct Stop {
    flag: Mutex<bool>,
    condvar: Condvar,
}

impl Stop {
    pub(super) fn new() -> Self {
        Self {
            flag: Mutex::new(false),
            condvar: Condvar::new(),
        }
    }

    /// Sets the flag and wakes anything waiting on it.
    pub(super) fn signal(&self) {
        *self.flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.condvar.notify_all();
    }

    /// Waits up to `timeout`, returning early the moment [`Stop::signal`] is
    /// called elsewhere. The return says which one happened, so a caller can
    /// tell "stop" from "the deadline arrived".
    pub(super) fn wait(&self, timeout: Duration) -> bool {
        let flag = self.flag.lock().unwrap_or_else(PoisonError::into_inner);
        let (flag, _) = self
            .condvar
            .wait_timeout_while(flag, timeout, |signalled| !*signalled)
            .unwrap_or_else(PoisonError::into_inner);
        *flag
    }
}

/// Sweeps until [`Stop::signal`] fires or the router itself is gone.
///
/// The `Weak` covers the case the signal cannot: a `Router` dropped without
/// `stop` ever being called, where nothing remains to set a flag. Held as a
/// `Weak` rather than an `Arc` for exactly that reason -- an `Arc` here would
/// keep the router alive for as long as this thread runs, which is the leak
/// this exists to avoid.
pub(super) fn run(shared: &Weak<Shared>) {
    loop {
        let Some(strong) = shared.upgrade() else {
            return;
        };

        let Some(duration) = strong.idle_window.duration() else {
            return;
        };
        let interval = (duration / 2).max(MIN_INTERVAL);

        if strong.stop.wait(interval) {
            return;
        }

        for id in strong
            .slots
            .sweep_idle(&strong.catalog, &strong.idle_window)
        {
            println!("{id} unloaded after sitting idle past its configured window");
        }
    }
}
