//! How long an on-demand model may sit unused, and what that expires.
//!
//! A pure function, proven the way `admission` is: from values a test builds
//! directly, with no process, no socket and no clock waited on. Age is stated
//! with `Instant::checked_sub`, exactly as `admission`'s tests state it, so
//! every case here runs in the time a unit test should take.
//!
//! This module decides what has gone idle. It does not touch a slot, print a
//! line, or run on a thread -- that is [`super::proxy::reaper`], modelled on
//! `residents.rs` the way this is modelled on `admission`.

use std::time::{Duration, Instant};

use crate::admission::{Budget, Loaded};
use crate::catalog::Residency;
use crate::launch::Failure;

/// Where the idle window is configured, mirroring `MAESTRO_MEMORY_BUDGET_MIB`.
const VARIABLE: &str = "MAESTRO_IDLE_UNLOAD_SECONDS";

/// What `Router::bind` needs to know about one machine's memory, in the one
/// value that keeps it a fifth argument rather than a sixth.
///
/// The two settings answer different questions -- what may be held at once,
/// and how long unused memory may be held -- and are read from the
/// environment independently. They travel together only because `bind`'s
/// argument count has nowhere left to grow.
pub struct Limits {
    pub(crate) budget: Budget,
    pub(crate) idle_window: IdleWindow,
}

impl Limits {
    /// Carries a budget and an idle window that were decided independently.
    #[must_use]
    pub fn new(budget: Budget, idle_window: IdleWindow) -> Self {
        Self {
            budget,
            idle_window,
        }
    }
}

/// How long a loaded on-demand model may go unused before it is unloaded.
///
/// `None` means never: today's behaviour, and what an unset or zero variable
/// both mean. Zero gets that meaning on purpose -- an operator writing "off"
/// into a variable that is already in a script must not get permanent thrash
/// from a window that expires everything on every sweep.
pub struct IdleWindow(Option<Duration>);

impl IdleWindow {
    /// A window of exactly this duration, or off when it is zero.
    ///
    /// Separate from reading the environment for the reason `Budget::new` is:
    /// a test states the window it means directly, including one shorter than
    /// a second, which `MAESTRO_IDLE_UNLOAD_SECONDS` cannot express at all.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        if window.is_zero() {
            Self(None)
        } else {
            Self(Some(window))
        }
    }

    /// The configured duration, or none when idle unloading is off.
    ///
    /// Read by `proxy::reaper` to derive its sweep interval, and by
    /// `Slots::sweep_idle` to build the cutoff the generalised take path
    /// re-reads under the slot lock.
    pub(crate) fn duration(&self) -> Option<Duration> {
        self.0
    }

    /// The configured window in whole seconds, or none when it is off.
    ///
    /// Read by the command that starts the router, which says at startup
    /// whether anything will ever be unloaded for sitting idle.
    #[must_use]
    pub fn seconds(&self) -> Option<u64> {
        self.0.map(|duration| duration.as_secs())
    }

    /// The identifiers to unload, coldest first.
    ///
    /// On-demand, not busy, and unused for at least the window. Coldest
    /// first, matching `Budget::admit`, so a reader does not have to wonder
    /// whether the two orders differ.
    #[must_use]
    pub fn expired(&self, loaded: &[Loaded], now: Instant) -> Vec<String> {
        let Some(window) = self.0 else {
            return Vec::new();
        };

        let mut candidates: Vec<&Loaded> = loaded
            .iter()
            .filter(|entry| {
                entry.residency == Residency::OnDemand
                    && !entry.busy
                    && now.duration_since(entry.last_used) >= window
            })
            .collect();
        candidates.sort_by_key(|entry| entry.last_used);
        candidates
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect()
    }

    /// The window this machine is configured with, or none.
    ///
    /// An empty or unset variable means no window, as `Budget::configured`
    /// treats a bare `export MAESTRO_MEMORY_BUDGET_MIB=`: reading it as zero
    /// would turn a plausible slip into permanent thrash.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the variable carries something that is not
    /// a whole number of seconds. A window someone set and mistyped must not
    /// silently become no window at all.
    pub fn configured() -> Result<Self, Failure> {
        let Some(value) = std::env::var_os(VARIABLE).filter(|value| !value.is_empty()) else {
            return Ok(Self(None));
        };

        let text = value.to_string_lossy();
        let seconds: u64 = text.trim().parse().map_err(|_| {
            Failure::Unavailable(format!(
                "{VARIABLE} carries '{text}', which is not a whole number of \
                 seconds; unset it for no idle window, or give it a whole \
                 number"
            ))
        })?;
        Ok(Self::new(Duration::from_secs(seconds)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(id: &str, residency: Residency, busy: bool, age: u64) -> Loaded {
        Loaded {
            id: id.to_owned(),
            memory_estimate_mib: 512,
            residency,
            busy,
            last_used: Instant::now()
                .checked_sub(Duration::from_secs(age))
                .expect("a process that has run for less than the test ages"),
        }
    }

    fn on_demand(id: &str, busy: bool, age: u64) -> Loaded {
        loaded(id, Residency::OnDemand, busy, age)
    }

    /// Both protection rules have one shape, mirroring
    /// `admission::tests::with_one_protected`: a protected entry, well past
    /// the window, held beside an on-demand one of the same age that is not
    /// protected. The rules differ only in what makes an entry protected, so
    /// only that field is a parameter.
    fn with_one_protected(protected: Loaded) -> Vec<String> {
        let window = IdleWindow::new(Duration::from_secs(60));
        let held = [protected, on_demand("stale", false, 120)];
        window.expired(&held, Instant::now())
    }

    #[test]
    fn an_on_demand_entry_older_than_the_window_is_named_beside_a_younger_one() {
        let window = IdleWindow::new(Duration::from_secs(60));
        let held = [on_demand("old", false, 120), on_demand("young", false, 10)];

        assert_eq!(
            window.expired(&held, Instant::now()),
            vec!["old".to_owned()],
            "only the one that has been idle longer than the window"
        );
    }

    #[test]
    fn a_resident_older_than_the_window_is_not_named_beside_an_on_demand_one() {
        assert_eq!(
            with_one_protected(loaded("pinned", Residency::Resident, false, 120)),
            vec!["stale".to_owned()],
            "a resident is never a candidate, however long it has been idle"
        );
    }

    #[test]
    fn a_busy_entry_older_than_the_window_is_not_named_beside_an_idle_one() {
        assert_eq!(
            with_one_protected(loaded("reading", Residency::OnDemand, true, 120)),
            vec!["stale".to_owned()],
            "a busy entry is never a candidate, however long since it started"
        );
    }

    #[test]
    fn a_zero_window_names_nothing_whatever_the_ages_are() {
        let window = IdleWindow::new(Duration::ZERO);
        let held = [
            on_demand("just-finished", false, 0),
            on_demand("ancient", false, 1_000_000),
        ];

        assert_eq!(
            window.expired(&held, Instant::now()),
            Vec::<String>::new(),
            "a zero window is off, or it would expire everything on every \
             sweep -- and off means no eviction however long anything has \
             sat idle"
        );
    }

    #[test]
    fn several_expired_entries_come_back_coldest_first() {
        let window = IdleWindow::new(Duration::from_secs(60));
        let held = [
            on_demand("newer", false, 90),
            on_demand("oldest", false, 300),
            on_demand("newest", false, 61),
        ];

        assert_eq!(
            window.expired(&held, Instant::now()),
            vec!["oldest".to_owned(), "newer".to_owned(), "newest".to_owned()],
            "coldest first, matching Budget::admit's order"
        );
    }
}
