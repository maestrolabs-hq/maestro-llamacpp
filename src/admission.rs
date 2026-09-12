//! Deciding what may be loaded, and what must be unloaded first.
//!
//! This module touches no process and no socket. It takes a budget, what is
//! loaded now, what is wanted, and what the device has free, and returns a
//! decision; acting on that decision belongs to the caller. That separation
//! is deliberate: the policy is the part of eviction that is hard to get
//! right, and keeping it a pure function means it can be driven exhaustively
//! from a handful of values without a machine, a model, or a clock that has
//! to be waited on. The one place the machine is asked is `Budget`'s own
//! construction, in `budget.rs`, and it is asked before any of this runs.
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
//! Two questions are asked of every start, and both have to say yes. The
//! **ledger** is the budget: a ceiling on what the loaded models cost, where
//! a model costs its catalog estimate until it has been measured and the
//! larger of the two afterwards. The **device** is what the machine reports
//! free right now, which counts everything else running on it; a model that
//! puts its weights on the device has to fit in that room too, whatever the
//! ledger says. Where the machine cannot be asked, the device question is
//! not asked, and the ledger decides alone as it always did.

use crate::catalog::Residency;

mod budget;
mod room;
mod subject;

pub use budget::Budget;
use room::{Freed, Room};
pub use subject::{Loaded, Wanted};

/// What admission decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// There is room. Start it.
    Fits,
    /// There is room once these are unloaded, coldest first.
    Unload(Vec<String>),
    /// There is not yet, and what holds the room is on-demand and busy.
    ///
    /// Separated from [`Decision::Refuse`] because the two are only alike on
    /// the surface. What holds the room here would be a candidate the moment
    /// nothing were reading it, so the room frees itself and waiting is the
    /// difference between a router that queues and one that tells every caller
    /// to write a retry loop.
    Blocked(String),
    /// There is not, and this says what is holding the memory.
    ///
    /// Permanent as far as this request is concerned: larger than the whole
    /// budget, or held by residents that never become candidates. Waiting
    /// changes nothing, so the caller is told at once rather than held.
    Refuse(String),
}

impl Budget {
    /// Whether the wanted entry may be loaded, and what must go first.
    ///
    /// Returns [`Decision::Fits`] when there is room already, including when
    /// the entry is loaded, [`Decision::Unload`] naming what to unload coldest
    /// first, or [`Decision::Refuse`] carrying what is holding the memory.
    ///
    /// `device_free_mib` is what the machine reports free on the device right
    /// now, or `None` when it cannot be asked. It counts everything on the
    /// machine, not only what this router loaded, which is the point: a
    /// desktop that grew since the budget was set is room the ledger still
    /// believes in and the device no longer has.
    ///
    /// The ceiling is inclusive: holding exactly the budget is within it.
    #[must_use]
    pub fn admit(
        &self,
        loaded: &[Loaded],
        wanted: &Wanted,
        device_free_mib: Option<u64>,
    ) -> Decision {
        // Already loaded, so serving it costs nothing new. Checked before
        // anything else, because a request for a model that is running must
        // not be refused by arithmetic about loading it again.
        if loaded.iter().any(|held| held.id == wanted.id) {
            return Decision::Fits;
        }

        // Widened because a sum of estimates can exceed what one of them fits
        // in, and an overflow here would decide that everything fits.
        let limit_mib = self.limit_mib.map(u64::from);
        if let Some(limit) = limit_mib
            && wanted.cost_mib > limit
        {
            return Decision::Refuse(format!(
                "'{}' is estimated at {} MiB, which is more than the whole \
                 budget of {limit} MiB; nothing can be unloaded to make it fit",
                wanted.id, wanted.cost_mib
            ));
        }

        let room = Room {
            limit: limit_mib,
            held: loaded.iter().map(|entry| entry.cost_mib).sum(),
            device_free: device_free_mib,
        };
        let mut freed = Freed::default();
        if room.short(wanted, &freed).is_none() {
            return Decision::Fits;
        }

        // On-demand, idle, and not the entry being asked for. A resident
        // entry is never a candidate, and neither is one being read from.
        let mut candidates: Vec<&Loaded> = loaded
            .iter()
            .filter(|entry| {
                entry.residency == Residency::OnDemand && !entry.busy && entry.id != wanted.id
            })
            .collect();
        candidates.sort_by_key(|entry| entry.last_used);

        let mut unload = Vec::new();
        for candidate in candidates {
            if room.short(wanted, &freed).is_none() {
                break;
            }
            freed.cost += candidate.cost_mib;
            freed.device += candidate.device_mib;
            unload.push(candidate.id.clone());
        }

        match room.short(wanted, &freed) {
            None => Decision::Unload(unload),
            Some(short) => {
                let message = room.refusal(short, wanted, &freed, loaded);

                // Whether anything holding the room would be a candidate if it
                // were idle. That, and only that, is what makes waiting worth
                // doing: a resident never becomes a candidate however long
                // anyone waits, and neither does a budget too small for the
                // entry at any occupancy.
                let frees_itself = loaded.iter().any(|entry| {
                    entry.residency == Residency::OnDemand && entry.busy && entry.id != wanted.id
                });

                if frees_itself {
                    Decision::Blocked(message)
                } else {
                    Decision::Refuse(message)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::catalog::{Entry, RelativePath};
    use crate::memory::Measurement;
    use std::collections::BTreeMap;
    use std::time::Duration;

    /// A loaded entry, with the fields a case cares about named at the call.
    fn loaded(id: &str, mib: u64, residency: Residency, busy: bool, age: u64) -> Loaded {
        Loaded {
            id: id.to_owned(),
            cost_mib: mib,
            device_mib: mib,
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

    fn on_demand(id: &str, mib: u64, age: u64) -> Loaded {
        loaded(id, mib, Residency::OnDemand, false, age)
    }

    /// Both protection rules have one shape: a protected entry is held beside
    /// an idle on-demand one, and something is wanted that needs the room.
    ///
    /// Shared because the rules differ only in what makes an entry protected,
    /// and writing the arrangement twice said that twice without saying why.
    /// The protected entry is deliberately the larger and the colder, so a
    /// policy that ignored the rule would take it and the case would fail.
    fn with_one_protected(protected: Loaded) -> Decision {
        let budget = Budget::new(Some(10_000));
        let held = [protected, on_demand("idle", 2_000, 1)];
        budget.admit(&held, &wanted("wanted", 3_000), None)
    }

    #[test]
    fn without_a_limit_anything_fits_and_nothing_is_unloaded() {
        let budget = Budget::new(None);
        let held = [on_demand("a", 900_000, 10), on_demand("b", 900_000, 5)];

        assert_eq!(
            budget.admit(&held, &wanted("c", 900_000), None),
            Decision::Fits,
            "an unset budget means no eviction, however much is loaded"
        );
    }

    #[test]
    fn with_room_to_spare_a_wanted_entry_fits() {
        let budget = Budget::new(Some(10_000));
        let held = [on_demand("a", 2_000, 10)];

        assert_eq!(
            budget.admit(&held, &wanted("b", 3_000), None),
            Decision::Fits
        );
    }

    #[test]
    fn a_wanted_entry_that_does_not_fit_unloads_the_coldest_candidate() {
        let budget = Budget::new(Some(10_000));
        // "a" answered longest ago, so it goes first.
        let held = [on_demand("a", 4_000, 30), on_demand("b", 4_000, 1)];

        assert_eq!(
            budget.admit(&held, &wanted("c", 5_000), None),
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

        // 9000 held, 5000 wanted, 10000 allowed. Unloading the coldest alone
        // leaves 11000, which is over; unloading two leaves 8000, which is
        // not. The third stays loaded because nothing needs its room.
        assert_eq!(
            budget.admit(&held, &wanted("d", 5_000), None),
            Decision::Unload(vec!["a".to_owned(), "b".to_owned()]),
            "two make room for 5000 within 10000; the third stays loaded"
        );
    }

    #[test]
    fn a_resident_entry_is_never_unloaded() {
        assert_eq!(
            with_one_protected(loaded("pinned", 6_000, Residency::Resident, false, 100)),
            Decision::Unload(vec!["idle".to_owned()]),
            "the resident entry is colder and would free enough on its own, \
             and is still not a candidate"
        );
    }

    #[test]
    fn a_busy_entry_is_never_unloaded() {
        assert_eq!(
            with_one_protected(loaded("reading", 6_000, Residency::OnDemand, true, 100)),
            Decision::Unload(vec!["idle".to_owned()]),
            "the busy entry is coldest and on-demand, and is still not a candidate"
        );
    }

    #[test]
    fn when_the_room_is_held_by_something_busy_the_decision_blocks_and_says_why() {
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("busy", 8_000, Residency::OnDemand, true, 100),
            loaded("pinned", 1_000, Residency::Resident, false, 50),
        ];

        // Nothing can be unloaded *now*, which is why this is not `Unload`.
        // But `busy` is on-demand and would be a candidate the moment its
        // reader finished, so the room frees itself and the caller may
        // usefully wait for it. Telling that apart from a refusal is the whole
        // of `Blocked`.
        let Decision::Blocked(message) = budget.admit(&held, &wanted("wanted", 5_000), None) else {
            panic!("the room is held by something that will release it");
        };
        assert!(
            message.contains("busy") && message.contains("pinned"),
            "the message still names what is holding the memory: {message}"
        );
    }

    #[test]
    fn when_a_resident_holds_the_room_the_decision_refuses_because_waiting_cannot_help() {
        let budget = Budget::new(Some(10_000));
        // No on-demand entry at all, so nothing here will ever stop being a
        // blocker. A caller held at this would be held until its wait expired
        // and then told exactly what it could have been told at once.
        let held = [
            loaded("pinned", 8_000, Residency::Resident, false, 100),
            loaded("also-pinned", 1_000, Residency::Resident, false, 50),
        ];

        let Decision::Refuse(message) = budget.admit(&held, &wanted("wanted", 5_000), None) else {
            panic!("residents never become candidates, so waiting is pointless");
        };
        assert!(
            message.contains("pinned"),
            "the refusal names what is holding the memory: {message}"
        );
    }

    #[test]
    fn a_refusal_a_resident_makes_permanent_says_so_instead_of_naming_the_moment() {
        // The shape that sent a real caller into a retry loop. An on-demand
        // entry is loaded and idle, so it *will* be unloaded and the refusal
        // names it -- which reads exactly like a clash that clears in a
        // moment. It does not: 10_000 less the 1_000 a resident never gives
        // back leaves 9_000, and the entry wants 9_500. Every retry, for the
        // life of the catalog, gets this same answer.
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("steward", 1_000, Residency::Resident, false, 100),
            on_demand("big", 6_000, 50),
        ];

        let Decision::Refuse(message) = budget.admit(&held, &wanted("flagship", 9_500), None)
        else {
            panic!("a resident reservation this entry cannot fit beside is permanent");
        };
        assert!(
            message.contains("steward"),
            "the resident in the way is named, because it is the thing to \
             change: {message}"
        );
        assert!(
            message.contains("9000") || message.contains("9,000"),
            "what is left once the residents have taken their share is the \
             number that explains the refusal: {message}"
        );
        assert!(
            message.contains("never") || message.contains("cannot ever"),
            "and it is said to be permanent, so the reader edits the catalog \
             rather than retrying: {message}"
        );
    }

    #[test]
    fn an_entry_already_loaded_fits_even_when_the_budget_is_exhausted() {
        let budget = Budget::new(Some(10_000));
        let held = [loaded("wanted", 9_999, Residency::OnDemand, true, 1)];

        assert_eq!(
            budget.admit(&held, &wanted("wanted", 9_999), None),
            Decision::Fits,
            "serving a model that is already loaded costs nothing new"
        );
    }

    #[test]
    fn an_entry_larger_than_the_whole_budget_refuses_immediately() {
        let budget = Budget::new(Some(4_000));
        let held = [on_demand("a", 1_000, 10)];

        let Decision::Refuse(message) = budget.admit(&held, &wanted("huge", 8_000), None) else {
            panic!("an entry larger than the budget can never be served");
        };
        assert!(
            message.contains("8000") && message.contains("4000"),
            "the refusal names the estimate and the budget: {message}"
        );
    }

    /// What a case wants, at the given cost on both sides.
    fn wanted(id: &str, mib: u64) -> Wanted {
        Wanted {
            id: id.to_owned(),
            cost_mib: mib,
            device_mib: mib,
        }
    }

    #[test]
    fn a_device_short_of_room_unloads_a_candidate_the_ledger_would_have_kept() {
        // No ceiling, so the ledger says everything fits. The device has 100
        // MiB free and the wanted entry needs 512, so the idle entry goes
        // even though nothing about the budget asked for it to.
        let budget = Budget::new(None);
        let held = [on_demand("idle", 512, 10)];

        assert_eq!(
            budget.admit(&held, &wanted("wanted", 512), Some(100)),
            Decision::Unload(vec!["idle".to_owned()]),
            "the device is asked as well as the ledger, and it said no"
        );
    }

    #[test]
    fn a_device_short_of_room_with_nothing_to_unload_refuses_naming_needed_and_free() {
        let budget = Budget::new(None);

        let Decision::Refuse(message) = budget.admit(&[], &wanted("wanted", 512), Some(100)) else {
            panic!("nothing is loaded, so nothing can be unloaded to make room");
        };
        assert!(
            message.contains("512") && message.contains("100"),
            "the refusal says what was needed and what the device had: {message}"
        );
    }

    #[test]
    fn an_entry_that_puts_nothing_on_the_device_is_not_held_to_the_devices_room() {
        let budget = Budget::new(None);
        let wanted = Wanted {
            id: "processor-bound".to_owned(),
            cost_mib: 4096,
            device_mib: 0,
        };

        assert_eq!(
            budget.admit(&[], &wanted, Some(0)),
            Decision::Fits,
            "a model kept off the device needs none of its room"
        );
    }

    #[test]
    fn what_a_loaded_entry_costs_is_the_larger_of_its_estimate_and_its_measurement() {
        let entry = entry_estimated_at(1024);
        let now = Instant::now();

        let measured = Loaded::of(
            &entry,
            false,
            now,
            &Measurement {
                resident_mib: Some(4600),
                device_mib: Some(725),
            },
        );
        assert_eq!(
            measured.cost_mib, 4600,
            "an under-estimated model is counted at what it turned out to cost"
        );
        assert_eq!(
            measured.device_mib, 725,
            "and what unloading it frees on the device is what it was seen to hold"
        );

        let unmeasured = Loaded::of(&entry, false, now, &Measurement::UNKNOWN);
        assert_eq!(
            unmeasured.cost_mib, 1024,
            "with nothing measured, the estimate stands"
        );

        let over_estimated = Loaded::of(
            &entry,
            false,
            now,
            &Measurement {
                resident_mib: Some(300),
                device_mib: None,
            },
        );
        assert_eq!(
            over_estimated.cost_mib, 1024,
            "a measurement below the estimate does not lower it: what a model \
             holds a moment after loading is a floor, not its peak"
        );
    }

    #[test]
    fn device_need_is_the_estimate_unless_the_flags_keep_every_layer_off_the_device() {
        let mut entry = entry_estimated_at(4096);
        assert_eq!(Wanted::of(&entry).device_mib, 4096);

        for key in ["n-gpu-layers", "ngl", "gpu-layers"] {
            entry.flags = BTreeMap::from([(key.to_owned(), "0".to_owned())]);
            assert_eq!(
                Wanted::of(&entry).device_mib,
                0,
                "'{key} = 0' keeps the weights off the device"
            );
        }

        entry.flags = BTreeMap::from([("n-gpu-layers".to_owned(), "999".to_owned())]);
        assert_eq!(Wanted::of(&entry).device_mib, 4096);
    }

    fn entry_estimated_at(mib: u32) -> Entry {
        Entry {
            id: "entry".to_owned(),
            path: RelativePath::new("somewhere/model.gguf").expect("relative"),
            draft_path: None,
            projector_path: None,
            context_size: 4096,
            residency: Residency::OnDemand,
            memory_estimate_mib: mib,
            reasoning_format: None,
            reasoning_effort: None,
            startup_timeout_seconds: 30,
            // The stock server: these fixtures are about other things.
            runtime: None,
            flags: BTreeMap::new(),
        }
    }
}
