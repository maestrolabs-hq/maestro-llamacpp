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

use std::time::Instant;

use crate::catalog::{Entry, Residency};
use crate::memory::Measurement;

mod budget;
mod room;

pub use budget::Budget;
use room::{Freed, Room};

/// The flags that keep every layer off the device, in the three spellings
/// the server accepts.
///
/// The one flag this router reads rather than passes through, because the
/// device question cannot be asked without it: a model with every layer on
/// the processor holds none of the device's room, and refusing it for want
/// of that room would refuse the resident entry whenever the large model
/// beside it fills the device -- which is the arrangement the shipped catalog
/// was measured in.
const LAYERS_ON_DEVICE: [&str; 3] = ["n-gpu-layers", "ngl", "gpu-layers"];

/// One model the router has loaded, as admission needs to see it.
pub struct Loaded {
    /// Which entry it is.
    pub id: String,
    /// What it costs: its estimate, or what it was measured at if that is
    /// more.
    pub cost_mib: u64,
    /// What unloading it would free on the device: what it was seen to hold
    /// there, or its share of the estimate if nothing could be seen.
    pub device_mib: u64,
    /// Whether it may ever be unloaded.
    pub residency: Residency,
    /// Whether anything is reading from it.
    pub busy: bool,
    /// When it last answered, so the coldest is unloaded first.
    pub last_used: Instant,
}

impl Loaded {
    /// What admission needs to know about one entry that is loaded.
    ///
    /// Built here rather than at the call site because these fields are this
    /// module's own. A caller that names all six is a caller that has to be
    /// edited every time the policy needs one more, and the facts it actually
    /// holds -- whether something is reading, when it last answered, and what
    /// the machine measured -- are the only ones it is asked for.
    ///
    /// The measurement raises the cost and never lowers it. What a model
    /// holds a moment after loading is a floor: the context fills as it is
    /// used, so an estimate above the measurement is the operator saying what
    /// the model grows to, and that is kept.
    #[must_use]
    pub fn of(entry: &Entry, busy: bool, last_used: Instant, measured: &Measurement) -> Self {
        let estimate = u64::from(entry.memory_estimate_mib);
        Self {
            id: entry.id.clone(),
            cost_mib: measured
                .largest_mib()
                .map_or(estimate, |mib| mib.max(estimate)),
            device_mib: measured
                .device_mib
                .unwrap_or_else(|| device_need_mib(entry)),
            residency: entry.residency,
            busy,
            last_used,
        }
    }
}

/// The entry a caller wants started, as admission needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// Which entry it is.
    pub id: String,
    /// What loading it is expected to cost.
    pub cost_mib: u64,
    /// How much of that lands on the device.
    pub device_mib: u64,
}

impl Wanted {
    /// One entry, at its catalog estimate.
    #[must_use]
    pub fn of(entry: &Entry) -> Self {
        Self {
            id: entry.id.clone(),
            cost_mib: u64::from(entry.memory_estimate_mib),
            device_mib: device_need_mib(entry),
        }
    }
}

/// What an entry is expected to hold on the device: its whole estimate,
/// unless its flags keep every layer off it.
///
/// A model kept off the device still holds a small runtime context there,
/// and that is not counted: it is under the margin the derived budget keeps
/// back, and the measurement taken once the model has loaded counts it from
/// then on.
fn device_need_mib(entry: &Entry) -> u64 {
    let off_device = LAYERS_ON_DEVICE.iter().any(|key| {
        entry
            .flags
            .get(*key)
            .is_some_and(|value| value.trim() == "0")
    });
    if off_device {
        0
    } else {
        u64::from(entry.memory_estimate_mib)
    }
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
            Some(short) => Decision::Refuse(room.refusal(short, wanted, &freed, loaded)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::RelativePath;
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
    fn when_every_candidate_is_exhausted_the_decision_refuses_and_says_why() {
        let budget = Budget::new(Some(10_000));
        let held = [
            loaded("busy", 8_000, Residency::OnDemand, true, 100),
            loaded("pinned", 1_000, Residency::Resident, false, 50),
        ];

        let Decision::Refuse(message) = budget.admit(&held, &wanted("wanted", 5_000), None) else {
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
            flags: BTreeMap::new(),
        }
    }
}
