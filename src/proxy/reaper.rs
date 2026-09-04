//! The thread that unloads what the idle window has expired.
//!
//! Modelled on `residents.rs`: it reaches children through the same path a
//! request does, and adds no coordination of its own. The one thing that
//! module does not need and this one does is a way to end -- `residents::load`
//! runs once and returns; this is the first thread this router spawns that
//! does not, so it needs a signal to stop it rather than an end it reaches on
//! its own.

use std::sync::{Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use super::Shared;

/// The floor under the derived sweep interval, in the sense decision 5 of the
/// idle-unload plan states it: half the window, floored here rather than at a
/// round second, so the timing guarantee -- at most one and a half windows
/// plus one sweep -- holds for every window the configured variable can
/// express.
///
/// Reachable only from [`IdleWindow::new`](crate::idle::IdleWindow::new): the
/// whole-second `configured` a running router reads from the environment
/// cannot express a window under two seconds, so only a test that builds its
/// own window can drive the interval below this floor.
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
    // The previous tick's deadline rather than a fixed start: each one is
    // computed as the last plus `interval`, so a sweep that takes real time
    // does not push every later tick back by that much. Without this a sweep
    // slow enough to matter would compound across the life of the thread
    // rather than costing what it cost once.
    let mut deadline: Option<Instant> = None;

    loop {
        let Some(strong) = shared.upgrade() else {
            return;
        };

        let Some(duration) = strong.idle_window.duration() else {
            return;
        };
        let interval = (duration / 2).max(MIN_INTERVAL);

        let next = deadline.unwrap_or_else(Instant::now) + interval;
        if strong
            .stop
            .wait(next.saturating_duration_since(Instant::now()))
        {
            return;
        }
        deadline = Some(next);

        for id in strong
            .slots
            .sweep_idle(&strong.catalog, &strong.idle_window)
        {
            println!("{id} unloaded after sitting idle past its configured window");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    /// The mechanism decision 7 depends on: a wait ends the moment `signal`
    /// is called, rather than only once its timeout elapses. Proven with the
    /// primitive alone, with no `Shared`, no catalog and no process --
    /// `run`'s loop is only ever this wait plus an upgrade and a sweep, and
    /// the promptness the plan requires lives entirely here.
    #[test]
    fn a_wait_ends_the_moment_signal_is_called_rather_than_at_its_timeout() {
        let stop = Arc::new(Stop::new());
        let signalling = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signalling.signal();
        });

        let started = Instant::now();
        let signalled = stop.wait(Duration::from_secs(10));

        assert!(signalled, "the wait must report that it was signalled");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a reaper configured with a long window must still end at once \
             when stop is called, rather than sleeping out its interval: \
             waited {:?}",
            started.elapsed()
        );
    }

    /// The other half of the same guarantee: nothing waiting must be told
    /// stop happened when it did not.
    #[test]
    fn a_wait_that_times_out_reports_no_signal() {
        let stop = Stop::new();
        assert!(!stop.wait(Duration::from_millis(20)));
    }

    /// The integrated claim rather than the primitive alone: a real `run`
    /// loop, spawned on its own thread against a real `Shared`, ends the
    /// moment `Stop::signal` is called rather than sleeping out its
    /// configured interval.
    ///
    /// The window is ten minutes, which makes the derived interval far
    /// longer than this test's own patience -- a `run` that only reacted to
    /// its next scheduled tick would make this test hang rather than fail an
    /// assertion. Nothing here starts a process: the catalog's one entry is
    /// never requested, so `Server` is never asked to spawn anything and its
    /// binary path only has to exist, not run.
    #[test]
    fn a_live_reaper_ends_promptly_when_stop_is_signalled_rather_than_at_its_scheduled_sweep() {
        use crate::admission::Budget;
        use crate::catalog::Catalog;
        use crate::idle::IdleWindow;
        use crate::launch::Server;
        use std::path::PathBuf;
        use std::sync::Mutex;

        let catalog = Catalog::parse(
            "version = 1\n\
             \n\
             [defaults]\n\
             context_size = 4096\n\
             residency = \"on-demand\"\n\
             memory_estimate_mib = 512\n\
             startup_timeout_seconds = 30\n\
             \n\
             [models.gemma3]\n\
             path = \"unused\"\n",
        )
        .expect("a usable catalog");
        let server = Server::located(Some(
            &std::env::current_exe().expect("this test binary's own path"),
        ))
        .expect("this test binary's own path is a file");
        let slots = super::super::slots::Slots::new(&catalog, Budget::new(None));

        let shared = Arc::new(Shared {
            catalog,
            root: PathBuf::from("/tmp"),
            server,
            slots,
            resident_failures: Mutex::new(Vec::new()),
            idle_window: IdleWindow::new(Duration::from_secs(600)),
            stop: Stop::new(),
        });

        let weak = Arc::downgrade(&shared);
        let reaping = std::thread::spawn(move || run(&weak));

        // A brief pause, so the reaper has actually entered its wait before
        // this signals it -- a signal that arrived before the wait began
        // would prove nothing about promptness.
        std::thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        shared.stop.signal();
        reaping.join().expect("the reaper thread does not panic");

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a reaper configured with a ten-minute sweep interval must still \
             end within moments of stop being signalled, rather than \
             sleeping out its interval: waited {:?}",
            started.elapsed()
        );
    }
}
