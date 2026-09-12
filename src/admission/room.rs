//! What there is to fit into, and the words for when there is not.
//!
//! Split from the decision beside it when `admission.rs` grew past the
//! module-size gate, along the seam the two questions left: `admission`
//! chooses candidates coldest first, and this says whether what they would
//! free is enough -- on the ledger, on the device, or on neither.

use crate::catalog::Residency;

use super::{Loaded, Wanted};

/// What there is to fit into, on both sides, in mebibytes.
pub(super) struct Room {
    /// The ledger's ceiling, or none.
    pub(super) limit: Option<u64>,
    /// What the ledger counts as held now.
    pub(super) held: u64,
    /// What the device reports free now, or none when it cannot be asked.
    pub(super) device_free: Option<u64>,
}

/// What unloading the candidates chosen so far would give back, in
/// mebibytes: on the ledger, and on the device.
#[derive(Default)]
pub(super) struct Freed {
    pub(super) cost: u64,
    pub(super) device: u64,
}

/// Which side has no room, when one has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Short {
    /// The budget's ceiling would be passed.
    Ledger,
    /// The device has less free than the model would put on it.
    Device,
}

impl Room {
    /// Which side is still short once `freed` is given back, or `None` when
    /// the wanted entry fits on both.
    ///
    /// The ledger is named first when both are short, because its holders
    /// are what an operator can act on; the device is named only when the
    /// ledger would have had room, because then the answer is about the
    /// machine rather than about the catalog.
    pub(super) fn short(&self, wanted: &Wanted, freed: &Freed) -> Option<Short> {
        let ledger_fits = self
            .limit
            .is_none_or(|limit| self.held.saturating_sub(freed.cost) + wanted.cost_mib <= limit);
        let device_fits = self
            .device_free
            .is_none_or(|free| free + freed.device >= wanted.device_mib);
        match (ledger_fits, device_fits) {
            (false, _) => Some(Short::Ledger),
            (true, false) => Some(Short::Device),
            (true, true) => None,
        }
    }

    /// Why nothing more can be done, naming whichever side said no.
    pub(super) fn refusal(
        &self,
        short: Short,
        wanted: &Wanted,
        freed: &Freed,
        loaded: &[Loaded],
    ) -> String {
        match short {
            Short::Ledger => {
                let limit = self.limit.unwrap_or_default();

                // Whether unloading everything that *can* be unloaded would
                // still leave this entry short. If so the refusal is a
                // property of the catalog, not of the moment, and saying it
                // any other way sends the reader into a retry loop: every
                // attempt gets this same answer, and the entries the message
                // names keep changing, which makes it look transient.
                let reserved: u64 = loaded
                    .iter()
                    .filter(|entry| entry.residency == Residency::Resident)
                    .map(|entry| entry.cost_mib)
                    .sum();
                if wanted.cost_mib + reserved > limit {
                    let residents: Vec<&str> = loaded
                        .iter()
                        .filter(|entry| entry.residency == Residency::Resident)
                        .map(|entry| entry.id.as_str())
                        .collect();
                    return format!(
                        "'{}' needs {} MiB and can never load: of the budget's \
                         {limit} MiB, {reserved} MiB is permanently reserved \
                         by resident entries ({}), leaving {}. Unloading the \
                         rest would not be enough and waiting cannot help -- \
                         make one of those entries on-demand, or lower this \
                         one's estimate or context",
                        wanted.id,
                        wanted.cost_mib,
                        residents.join(", "),
                        limit - reserved,
                    );
                }

                format!(
                    "'{}' needs {} MiB and the budget of {limit} MiB is held by: {}",
                    wanted.id,
                    wanted.cost_mib,
                    holders(loaded)
                )
            }
            Short::Device => {
                let free = self.device_free.unwrap_or_default();
                let beside = if loaded.is_empty() {
                    String::new()
                } else {
                    format!(", beside: {}", holders(loaded))
                };
                format!(
                    "'{}' needs {} MiB on the device, which has {free} MiB free and \
                     would have {} MiB once every idle model was unloaded; the rest \
                     is held by something outside this router{beside}",
                    wanted.id,
                    wanted.device_mib,
                    free + freed.device
                )
            }
        }
    }
}

/// What is holding the memory, and why each one is staying.
///
/// A refusal a caller can act on has to say which entries are in the way and
/// which of those could never move, so the reader knows whether to retry or
/// to change the catalog.
fn holders(loaded: &[Loaded]) -> String {
    loaded
        .iter()
        .map(|entry| {
            let (id, mib) = (&entry.id, entry.cost_mib);
            let why = match (entry.residency, entry.busy) {
                (_, true) => ", busy",
                (Residency::Resident, _) => ", resident",
                (Residency::OnDemand, false) => "",
            };
            format!("{id} ({mib} MiB{why})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}
