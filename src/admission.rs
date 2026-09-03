//! Deciding what may be loaded, and what must be unloaded first.
//!
//! This module touches no process and no socket. It takes a budget, what is
//! loaded now, and what is wanted, and returns a decision; acting on that
//! decision belongs to the caller. That separation is deliberate: the policy
//! is the part of eviction that is hard to get right, and keeping it a pure
//! function means it can be driven exhaustively from four values without a
//! machine, a model, or a clock that has to be waited on.
//!
//! Two rules shape every decision here.
//!
//! A **candidate** is a loaded model the router may unload: on-demand, and
//! with nothing reading from it. A resident model is never a candidate, which
//! is what residency means. A busy model is never a candidate either, because
//! unloading one kills the process answering a request that is still being
//! read -- and the caller sees a stream stop early, which is indistinguishable
//! from a model that finished.
//!
//! **The coldest candidate goes first.** When more than one could be unloaded,
//! the one that answered longest ago is chosen, because it is the one least
//! likely to be asked for again in the next moment.
//!
//! The budget is a ceiling on estimates, never on measurements. Every number
//! reaching this module is what somebody typed into a catalog, so a decision
//! here is only as good as those figures: a model that costs more than its
//! estimate is admitted and then fails to load. The mitigation is that a
//! failed start names its entry, not that the estimate is right.

use std::time::Instant;

use crate::catalog::Residency;

/// What the router may hold models in at once.
pub struct Budget {
    /// The ceiling in mebibytes, or `None` when none was configured.
    ///
    // Read by `admit`, which is `todo!()` until the green commit. `expect`
    // rather than `allow` so this fails to compile once the policy reads it,
    // which is what removes the scaffolding rather than leaving it behind.
    #[expect(dead_code, reason = "read by the policy in the green commit")]
    limit_mib: Option<u32>,
}

/// One model the router has loaded, as admission needs to see it.
pub struct Loaded {
    /// Which entry it is.
    pub id: String,
    /// What it was estimated to cost.
    pub memory_estimate_mib: u32,
    /// Whether it may ever be unloaded.
    pub residency: Residency,
    /// Whether anything is reading from it.
    pub busy: bool,
    /// When it last answered, so the coldest is unloaded first.
    pub last_used: Instant,
}

/// What admission decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// There is room. Start it.
    Fits,
    /// There is room once these are unloaded, coldest first.
    Unload(Vec<String>),
    /// There is not, and this says what is holding the memory.
    Refuse(String),
}

impl Budget {
    /// A budget of the given ceiling, or none at all.
    ///
    /// Separate from reading the environment on purpose: a test states the
    /// budget it means directly rather than setting a process-global variable
    /// that every other test in its binary would race against.
    #[must_use]
    pub fn new(limit_mib: Option<u32>) -> Self {
        Self { limit_mib }
    }

    /// Whether the wanted entry may be loaded, and what must go first.
    ///
    /// Returns [`Decision::Fits`] when there is room already, including when
    /// the entry is loaded, [`Decision::Unload`] naming what to unload coldest
    /// first, or [`Decision::Refuse`] carrying what is holding the memory.
    #[must_use]
    pub fn admit(&self, _loaded: &[Loaded], _wanted_id: &str, _wanted_mib: u32) -> Decision {
        todo!("the policy is written in the green commit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A loaded entry, with the fields a case cares about named at the call.
    fn loaded(id: &str, mib: u32, residency: Residency, busy: bool, age: u64) -> Loaded {
        Loaded {
            id: id.to_owned(),
            memory_estimate_mib: mib,
            residency,
            busy,
            // Subtracted rather than added, so a larger age is older. An
            // Instant cannot be constructed directly, which is why every case
            // states an age in seconds and this turns it into one.
            last_used: Instant::now()
                .checked_sub(Duration::from_secs(age))
                .expect("a process that has run for less than the test ages"),
        }
    }

    fn on_demand(id: &str, mib: u32, age: u64) -> Loaded {
        loaded(id, mib, Residency::OnDemand, false, age)
    }

    #[test]
    fn without_a_limit_anything_fits_and_nothing_is_unloaded() {
        let budget = Budget::new(None);
        let held = [on_demand("a", 900_000, 10), on_demand("b", 900_000, 5)];

        assert_eq!(
            budget.admit(&held, "c", 900_000),
            Decision::Fits,
            "an unset budget means no eviction, however much is loaded"
        );
    }

    #[test]
    fn with_room_to_spare_a_wanted_entry_fits() {
        let budget = Budget::new(Some(10_000));
        let held = [on_demand("a", 2_000, 10)];

        assert_eq!(budget.admit(&held, "b", 3_000), Decision::Fits);
    }

    #[test]
    fn a_wanted_entry_that_does_not_fit_unloads_the_coldest_candidate() {
        let budget = Budget::new(Some(10_000));
        // "a" answered longest ago, so it goes first.
        let held = [on_demand("a", 4_000, 30), on_demand("b", 4_000, 1)];

        assert_eq!(
            budget.admit(&held, "c", 5_000),
            Decision::Unload(vec!["a".to_owned()]),
            "only as many as are needed, coldest first"
        );
    }

    #[test]
    fn only_as_many_candidates_as_are_needed_are_unloaded() {
        let budget = Budget::new(Some(10_000));
        let held = [
            on_demand("a", 3_000, 30),
            on_demand("b", 3_000, 20),
            on_demand("c", 3_000, 10),
        ];

        assert_eq!(
            budget.admit(&held, "d", 4_000),
            Decision::Unload(vec!["a".to_owned(), "b".to_owned()]),
            "two make room for 4000 within 10000; the third stays loaded"
        );
    }

    #[test]
    fn a_resident_entry_is_never_unloaded() {
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("small", 6_000, Residency::Resident, false, 100),
            on_demand("big", 2_000, 1),
        ];

        assert_eq!(
            budget.admit(&held, "wanted", 3_000),
            Decision::Unload(vec!["big".to_owned()]),
            "the resident entry is older and would fit, and is still not a candidate"
        );
    }

    #[test]
    fn a_busy_entry_is_never_unloaded() {
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("busy", 6_000, Residency::OnDemand, true, 100),
            on_demand("idle", 2_000, 1),
        ];

        assert_eq!(
            budget.admit(&held, "wanted", 3_000),
            Decision::Unload(vec!["idle".to_owned()]),
            "the busy entry is coldest and on-demand, and is still not a candidate"
        );
    }

    #[test]
    fn when_every_candidate_is_exhausted_the_decision_refuses_and_says_why() {
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("busy", 8_000, Residency::OnDemand, true, 100),
            loaded("pinned", 1_000, Residency::Resident, false, 50),
        ];

        let Decision::Refuse(message) = budget.admit(&held, "wanted", 5_000) else {
            panic!("nothing can be unloaded, so this cannot be served");
        };
        assert!(
            message.contains("busy") && message.contains("pinned"),
            "the refusal names what is holding the memory: {message}"
        );
    }

    #[test]
    fn an_entry_already_loaded_fits_even_when_the_budget_is_exhausted() {
        let budget = Budget::new(Some(10_000));
        let held = [loaded("wanted", 9_999, Residency::OnDemand, true, 1)];

        assert_eq!(
            budget.admit(&held, "wanted", 9_999),
            Decision::Fits,
            "serving a model that is already loaded costs nothing new"
        );
    }

    #[test]
    fn an_entry_larger_than_the_whole_budget_refuses_immediately() {
        let budget = Budget::new(Some(4_000));
        let held = [on_demand("a", 1_000, 10)];

        let Decision::Refuse(message) = budget.admit(&held, "huge", 8_000) else {
            panic!("an entry larger than the budget can never be served");
        };
        assert!(
            message.contains("8000") && message.contains("4000"),
            "the refusal names the estimate and the budget: {message}"
        );
    }
}
