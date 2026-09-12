//! What a decision is about: the models held, and the one being asked for.
//!
//! Split from the deciding beside it along the seam the module-size gate
//! exposed. `admission` weighs these against a ceiling; nothing here weighs
//! anything. Keeping them apart means the arithmetic of what a model costs --
//! which flags put layers on the device, what a measurement overrides -- can
//! be read without the eviction policy in the way, and the reverse.

use std::time::Instant;

use crate::catalog::{Entry, Residency};
use crate::memory::Measurement;

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

        // An entry pinned to the processor is charged what it puts on the
        // device, not what it holds in system memory.
        //
        // Both figures are real and they are not the same resource. The
        // ceiling these are weighed against is derived from the device when
        // the machine has one, so charging a processor-pinned model its
        // resident set spends device budget on memory it never touches.
        // Measured here: a 4B model at `n-gpu-layers = 0` holds 787 MiB of the
        // device and roughly 7.4 GiB of system memory, and held as a resident
        // it reserved a quarter of the whole budget and refused every 27B
        // entry in the catalog.
        //
        // What bounds its system memory is the device-free check beside this
        // one and the machine itself, not this ledger.
        let off_device = device_need_mib(entry) == 0;
        let measured_cost = if off_device {
            measured.device_mib
        } else {
            measured.largest_mib()
        };

        Self {
            id: entry.id.clone(),
            cost_mib: measured_cost.map_or(estimate, |mib| mib.max(estimate)),
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
pub(super) fn device_need_mib(entry: &Entry) -> u64 {
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
