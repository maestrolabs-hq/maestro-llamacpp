//! What an entry actually costs, and how fast it actually runs.
//!
//! A catalog's `memory_estimate_mib` is a claim. Until an entry has been
//! loaded on a machine, that claim is arithmetic over file sizes and a guess
//! at the context cache, and the two ways a guess goes wrong are not
//! symmetric: too low and the card runs out part-way through loading, too high
//! and the entry is refused while actually fitting. Neither failure says which
//! it was.
//!
//! This loads one entry, asks the card what it took, asks the server how fast
//! it generated, and stops it again. One at a time and never two: the number
//! wanted is what a *single* entry costs, and co-residency would attribute one
//! model's pages to another.
//!
//! It reports. It does not rewrite the catalog -- see the plan for why, but
//! briefly: the shipped catalog's comments carry the reasoning for the numbers
//! beside them, and a writer that preserved them is a TOML round-tripper.

mod report;

pub use report::command;

use std::path::Path;
use std::time::{Duration, Instant};

mod rate;

pub use rate::Throughput;

use crate::catalog::Entry;
use crate::launch::{Failure, Server};
use crate::memory::Probe;

/// What one entry turned out to cost and do.
#[derive(Debug, Clone)]
pub struct Measurement {
    /// The entry measured.
    pub id: String,
    /// What the catalog claimed, for the comparison that is the whole point.
    pub declared_mib: u32,
    /// What the card said this entry's process held, when a card could be
    /// read.
    pub measured_mib: Option<u32>,
    /// How long it took to answer its first health check.
    pub load: Duration,
    /// How fast it answered, in the unit it answers in.
    pub throughput: Option<Throughput>,
}

impl Measurement {
    /// The estimate this measurement supports, with room over it.
    ///
    /// A twentieth above what was measured, rounded up to a quarter gibibyte.
    /// The margin is not superstition -- a measurement is one driver, one day
    /// and one context, and an estimate sitting exactly on it would be wrong
    /// the first time any of the three moved -- but it cannot be generous
    /// either, and the reason is specific.
    ///
    /// An estimate is what admission compares against the budget. Too low and
    /// a load runs the card out; too high and the entry is refused while it
    /// would actually have fitted. The second failure is the one a large
    /// margin causes, and it bites hardest exactly where the margin is
    /// largest: on a card a model nearly fills. Measured here, `qwen38` holds
    /// 28,715 MiB of a 32,607 MiB card. A tenth over, rounded to a whole
    /// gibibyte, declares 31,744 -- so a 90% budget would refuse an entry with
    /// nearly four gibibytes to spare.
    ///
    /// A twentieth of a 27B model is still over a gibibyte of headroom, which
    /// is more than the run-to-run variation these readings show.
    #[must_use]
    pub fn recommended_mib(&self) -> Option<u32> {
        const STEP: u32 = 256;
        let measured = self.measured_mib?;
        let with_room = measured + measured / 20;
        Some(with_room.div_ceil(STEP) * STEP)
    }
}

/// Loads one entry, measures it, and stops it.
///
/// # Errors
///
/// Returns a [`Failure`] when the entry cannot be started. A measurement that
/// fails after a successful start is reported as an absent field rather than
/// an error: knowing what it cost is still worth having when the rate could
/// not be read.
pub fn entry(server: &Server, model: &Entry, root: &Path) -> Result<Measurement, Failure> {
    // Detected once and reused for both readings, so the before and after come
    // from the same source rather than two independent probes that might
    // disagree about whether a card is there at all.
    let probe = Probe::detect();

    // Read before the load, for the difference below.
    let before = probe.device().map(|device| device.used_mib);

    let started = Instant::now();
    let child = server.start(model, root)?;
    let load = started.elapsed();

    // Two ways to the same number, preferred in order of how much they can be
    // trusted.
    //
    // Per process is exact and needs no assumptions, so it wins where the
    // platform exposes it. It does not everywhere: WSL2 virtualises the card
    // and reports no compute apps, so `device_mib` is `None` there for a
    // process that is plainly holding twenty gigabytes.
    //
    // The difference across the load is the fallback. It is only sound because
    // this command loads one entry at a time and never two -- under the
    // router, where another child may load concurrently, the same subtraction
    // would quietly attribute one model's pages to another.
    let measured_mib = probe
        .measure(child.pid())
        .device_mib
        .or_else(|| {
            let (before, after) = (before?, probe.device().map(|d| d.used_mib)?);
            after.checked_sub(before).filter(|grown| *grown > 0)
        })
        .and_then(|mib| u32::try_from(mib).ok());
    let throughput = rate::of(child.endpoint(), model);

    // `child` is dropped here, which kills and reaps it. Nothing is left
    // holding the card for the next entry to be measured against.
    Ok(Measurement {
        id: model.id.clone(),
        declared_mib: model.memory_estimate_mib,
        measured_mib,
        load,
        throughput,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holding(mib: u32) -> Measurement {
        Measurement {
            id: "gemma3".to_owned(),
            declared_mib: 0,
            measured_mib: Some(mib),
            load: Duration::ZERO,
            throughput: None,
        }
    }

    #[test]
    fn a_recommendation_clears_the_measurement_but_never_the_card() {
        // The real reading that motivated the margin: `qwen38` holding 28,715
        // MiB of a 32,607 MiB card.
        const CARD: u32 = 32_607;
        let wants = holding(28_715).recommended_mib().expect("a measurement");

        assert!(wants > 28_715, "an estimate must clear what was measured");
        assert!(
            wants <= CARD,
            "and must never exceed the card it was measured on, which would \
             declare an entry unloadable on the machine that just loaded it \
             -- got {wants}"
        );
    }

    #[test]
    fn an_entry_that_nearly_fills_a_card_cannot_also_fit_a_tight_share() {
        // Recorded because it is a property of the machine, not a defect in
        // the margin, and the arithmetic is easy to mistake for one.
        //
        // 28,715 MiB is 88% of a 32,607 MiB card. A 90% share leaves 631 MiB
        // over the measurement -- less than any honest margin. No choice of
        // margin makes this entry fit a 90% budget, which is the argument for
        // a reserve below the card rather than a fraction of it: a fraction
        // that suits a 16 GiB model refuses a 28 GiB one on the same card.
        const CARD: u32 = 32_607;
        let wants = holding(28_715).recommended_mib().expect("a measurement");

        assert!(
            wants > CARD * 9 / 10,
            "if this ever passes, a 90% share has become viable for a model \
             this size and the guidance in README.md should say so"
        );
        assert!(
            wants <= CARD * 95 / 100,
            "a 95% share must still admit it, or the margin really has grown \
             too large -- got {wants}"
        );
    }

    #[test]
    fn a_recommendation_lands_on_a_quarter_gibibyte() {
        for measured in [16_703, 22_690, 26_948, 26_997, 28_856] {
            let wants = holding(measured).recommended_mib().expect("a measurement");
            assert_eq!(wants % 256, 0, "{measured} rounded to {wants}");
            assert!(wants >= measured + measured / 20);
        }
    }

    #[test]
    fn a_measurement_that_did_not_happen_recommends_nothing() {
        // Rather than recommending a margin over zero, which would read as a
        // confident claim that an entry needs almost nothing.
        let unmeasured = Measurement {
            measured_mib: None,
            ..holding(0)
        };
        assert_eq!(unmeasured.recommended_mib(), None);
    }
}
