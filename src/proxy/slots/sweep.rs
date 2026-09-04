//! What the reaper acts through: naming what has gone idle, and stamping what
//! has just been used. Moved here when `mod.rs` grew past the module-size
//! gate, along the seam admission's own eviction left behind -- that half
//! kills a process to make room now; this half only ever removes, on a
//! schedule nothing is waiting on.

use std::sync::PoisonError;
use std::time::{Duration, Instant};

use crate::catalog::Catalog;
use crate::idle::IdleWindow;

use super::super::loaded::take_if_idle;
use super::Slots;

impl Slots {
    /// Unloads what has gone idle past `window`, and names what actually
    /// went.
    ///
    /// Built from the same snapshot admission uses, and taken the same way:
    /// under the generalised path from `loaded::take_if_idle`, with the
    /// caller's additional condition re-reading `last_used` against the
    /// cutoff rather than trusting the snapshot's copy of it. An entry that
    /// gained a reader, or answered again, between the snapshot and the take
    /// stays loaded and is not named -- reporting it as unloaded would be
    /// reporting a decision rather than an outcome.
    ///
    /// Takes no admission lock: this only ever removes, so a concurrent
    /// `admit` that snapshotted before this ran simply finds more room than
    /// it counted on, which is conservative rather than wrong.
    pub(in super::super) fn sweep_idle(
        &self,
        catalog: &Catalog,
        window: &IdleWindow,
    ) -> Vec<String> {
        let Some(duration) = window.duration() else {
            return Vec::new();
        };
        let now = Instant::now();

        window
            .expired(&self.held(catalog), now)
            .into_iter()
            .filter(|id| take_if_idle(self.slot(id), |held| stale(now, held.last_used, duration)))
            .collect()
    }

    /// Marks this entry's slot as used just now, without touching whether
    /// anything is running in it.
    ///
    /// Called after a relay finishes, so idleness is judged from when a
    /// request ended rather than when it started -- a relay that outlives the
    /// idle window must not look idle for the whole of its own duration.
    /// Called with the child still held by its caller, so its reference keeps
    /// the slot's `Arc` at a count no sweep can take between the relay ending
    /// and this landing.
    ///
    /// Does nothing if the slot has since been emptied, which is not an error:
    /// nothing is left to stamp.
    pub(in super::super) fn touch(&self, id: &str) {
        if let Some(held) = self
            .slot(id)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            held.last_used = Instant::now();
        }
    }
}

/// Whether `last_used` is old enough, against `now`, to count as idle for
/// `duration`.
///
/// Stated as elapsed time rather than as `last_used <=
/// now.checked_sub(duration)`, which fails open: a `duration` past what
/// `Instant`'s reference point can subtract underflows, and defaulting to
/// `now` there would make every entry look stale at once. `duration_since`
/// saturates instead, matching `idle::IdleWindow::expired`.
fn stale(now: Instant, last_used: Instant, duration: Duration) -> bool {
    now.duration_since(last_used) >= duration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_use_from_this_instant_is_never_stale_however_large_the_duration() {
        let now = Instant::now();

        // The largest `Duration` representable at all, so a cutoff computed
        // as `now.checked_sub(duration)` is guaranteed to underflow and,
        // failing open, default to `now` itself -- which would make this
        // instant's own use look exactly as stale as one from the dawn of
        // time.
        let duration = Duration::MAX;

        assert!(
            !stale(now, now, duration),
            "a use from this instant is not stale against any duration, and \
             especially not one large enough to make the old arithmetic \
             degenerate"
        );
    }
}
