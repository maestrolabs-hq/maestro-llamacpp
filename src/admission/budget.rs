//! Where the budget comes from: the environment, the machine, or a test.
//!
//! Split from the decision beside it along the seam the module-size gate
//! exposed: `admission` decides what fits under a ceiling, and this decides
//! what the ceiling is. Nothing here weighs a model against anything.

use crate::launch::Failure;
use crate::memory::Probe;

/// Where the memory budget is configured. The estate prefix keeps it
/// recognisable beside the other variables a machine carries.
const VARIABLE: &str = "MAESTRO_MEMORY_BUDGET_MIB";

/// What is kept back from the device's total when the budget is derived from
/// it: a tenth, and never less than this.
///
/// A device is never empty when a model loads -- the display, the driver and
/// whatever else the machine runs hold some of it -- and a budget set at the
/// whole total would admit a model into room that is not there. A tenth
/// covers a busy desktop on a large device; the floor covers a small device,
/// where a tenth is not a margin at all.
const MIN_DEVICE_MARGIN_MIB: u64 = 1024;

/// What may be held when the budget is derived from system memory, in
/// percent. Less than the device's share because the operating system and
/// everything else on the machine live in the same memory.
const SYSTEM_SHARE_PERCENT: u64 = 80;

/// What the router may hold models in at once.
#[derive(Debug)]
pub struct Budget {
    /// The ceiling in mebibytes, or `None` when there is none.
    pub(super) limit_mib: Option<u32>,
    /// Where the machine's own figures come from, for the checks that go
    /// beyond the ceiling.
    pub(super) probe: Probe,
    /// Where the ceiling came from, said once at startup.
    source: String,
}

impl Budget {
    /// A budget of the given ceiling, or none at all, on a machine that
    /// reports nothing.
    ///
    /// Separate from reading the environment on purpose: a test states the
    /// budget it means directly rather than setting a process-global variable
    /// that every other test in its binary would race against.
    #[must_use]
    pub fn new(limit_mib: Option<u32>) -> Self {
        Self::with_probe(limit_mib, Probe::none())
    }

    /// A stated ceiling, on a machine whose figures come from `probe`.
    ///
    /// What a test uses to drive the checks that read the machine: the
    /// ceiling is stated, and so is what the device will say.
    #[must_use]
    pub fn with_probe(limit_mib: Option<u32>, probe: Probe) -> Self {
        Self {
            limit_mib,
            probe,
            source: "stated directly".to_owned(),
        }
    }

    /// A budget the machine itself sets, when nobody has.
    ///
    /// The device first, at its total less a margin, because a model that
    /// does not fit on the device is the failure this budget exists to
    /// prevent. System memory when there is no device to ask, at four fifths.
    /// No budget at all when the machine reports nothing, which is what an
    /// unset variable meant before the machine could be asked.
    #[must_use]
    pub fn derived(probe: Probe) -> Self {
        let (limit_mib, source) = if let Some(device) = probe.device() {
            let margin = (device.total_mib / 10).max(MIN_DEVICE_MARGIN_MIB);
            let limit = device.total_mib.saturating_sub(margin);
            (
                (limit > 0).then_some(limit),
                format!(
                    "derived from the device ({} MiB total less a {margin} MiB margin)",
                    device.total_mib
                ),
            )
        } else if let Some(total) = probe.system_total_mib() {
            let limit = total * SYSTEM_SHARE_PERCENT / 100;
            (
                (limit > 0).then_some(limit),
                format!(
                    "derived from system memory ({SYSTEM_SHARE_PERCENT} percent of {total} MiB)"
                ),
            )
        } else {
            (
                None,
                "nothing on this machine says what it holds".to_owned(),
            )
        };
        Self {
            limit_mib: limit_mib.map(|limit| u32::try_from(limit).unwrap_or(u32::MAX)),
            probe,
            source,
        }
    }

    /// The budget this machine is configured with.
    ///
    /// The variable when it carries a number. When it is unset or empty, the
    /// machine sets its own, as [`Budget::derived`] does, and says so on one
    /// line: an operator who set nothing should not have to guess what the
    /// router decided for them. A budget is a fact about one machine's
    /// hardware, which is why it is read here rather than written into a
    /// catalog: the catalog describes a set of models without naming the
    /// machine they sit on.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the variable carries something that is not a
    /// number. A budget someone tried to set and mistyped must not silently
    /// become whatever the machine would have chosen, because the difference
    /// is whether the ceiling is the one the operator meant.
    pub fn configured() -> Result<Self, Failure> {
        // An empty value is treated as unset, as the models root is: a bare
        // `export MAESTRO_MEMORY_BUDGET_MIB=` is a plausible slip, and reading
        // it as a budget of nothing would refuse every model on the machine.
        let Some(value) = std::env::var_os(VARIABLE).filter(|value| !value.is_empty()) else {
            // Reported by the caller, not here. A constructor that wrote to
            // standard output printed before the command had said what it was
            // serving on, which put a diagnostic line above the address -- and
            // anything reading that address off the first line got the
            // diagnostic instead. `source` carries what to say; `startup` says
            // it, in the order the command chooses.
            return Ok(Self::derived(Probe::detect()));
        };

        let text = value.to_string_lossy();
        let limit = text.trim().parse().map_err(|_| {
            Failure::Unavailable(format!(
                "{VARIABLE} carries '{text}', which is not a number of \
                 mebibytes; unset it for a budget the machine sets, or give \
                 it a whole number"
            ))
        })?;
        Ok(Self {
            limit_mib: Some(limit),
            probe: Probe::detect(),
            source: format!("from {VARIABLE}"),
        })
    }

    /// The ceiling, or `None` when no budget applies.
    ///
    /// Read by the command that starts the router, which says at startup
    /// whether anything will ever be evicted.
    #[must_use]
    pub fn limit_mib(&self) -> Option<u32> {
        self.limit_mib
    }

    /// Where the ceiling came from, in words.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where the machine's own figures come from.
    #[must_use]
    pub fn probe(&self) -> &Probe {
        &self.probe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{DeviceMemory, Fixed};

    fn machine(device: Option<DeviceMemory>, system_total_mib: Option<u64>) -> Probe {
        Probe::Fixed(Fixed {
            device,
            system_total_mib,
            ..Fixed::default()
        })
    }

    fn device(total_mib: u64) -> DeviceMemory {
        DeviceMemory {
            total_mib,
            used_mib: 0,
        }
    }

    #[test]
    fn a_device_sets_the_budget_at_its_total_less_a_tenth() {
        // The device the router was written against, beside enough system
        // memory that a policy preferring it would be caught.
        let budget = Budget::derived(machine(Some(device(32607)), Some(48175)));

        assert_eq!(budget.limit_mib(), Some(29347), "32607 less 3260");
        assert!(
            budget.source().contains("device") && budget.source().contains("3260"),
            "the source names the device and the margin: {}",
            budget.source()
        );
    }

    #[test]
    fn a_small_device_keeps_a_gibibyte_back_rather_than_a_tenth() {
        let budget = Budget::derived(machine(Some(device(8192)), None));

        assert_eq!(
            budget.limit_mib(),
            Some(7168),
            "a tenth of 8192 is 819, which is not a margin the display and \
             the driver fit in; the floor applies instead"
        );
    }

    #[test]
    fn without_a_device_system_memory_sets_the_budget_at_four_fifths() {
        let budget = Budget::derived(machine(None, Some(48175)));

        assert_eq!(budget.limit_mib(), Some(38540));
        assert!(
            budget.source().contains("system memory"),
            "the source says it was not the device: {}",
            budget.source()
        );
    }

    #[test]
    fn a_machine_that_reports_nothing_sets_no_budget() {
        let budget = Budget::derived(machine(None, None));

        assert_eq!(
            budget.limit_mib(),
            None,
            "what an unset variable meant before the machine could be asked"
        );
    }
}
