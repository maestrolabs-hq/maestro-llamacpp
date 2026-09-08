//! What the machine says about its memory, asked at run time.
//!
//! The budget is a ceiling on estimates, and an estimate is what somebody
//! typed into a catalog. This module is the other source of truth: what the
//! device reports as free before a model is started, and what a child turns
//! out to hold once it has loaded. Neither replaces the catalog -- a figure
//! nobody can read stays an estimate -- but where the machine can be asked,
//! it is asked, and the answer is trusted over the guess.
//!
//! Everything here is fallible and everything degrades the same way: a tool
//! that is missing, hangs, or prints something unreadable makes the figure
//! *unknown*, never zero and never a panic. An unknown figure is what the
//! router already lived with before this module existed.
//!
//! The probe is a value rather than a set of free functions so a test can
//! state the numbers it means. [`Probe::Fixed`] answers with what it was
//! built with; [`Probe::Machine`] runs the platform's tools. The router only
//! ever holds one of these, and nothing that acts on a figure knows which.

mod command;
mod parse;

/// What one machine's device memory looks like, in mebibytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMemory {
    /// What every device the driver reports holds, together.
    pub total_mib: u64,
    /// What is in use right now, by anything on the machine.
    pub used_mib: u64,
}

impl DeviceMemory {
    /// What is left for a model to be started into.
    #[must_use]
    pub fn free_mib(&self) -> u64 {
        self.total_mib.saturating_sub(self.used_mib)
    }
}

/// What one child was found to hold once it had loaded, in mebibytes.
///
/// Either side may be unknown, independently: a machine without a device
/// probe still has a resident set to read, and a platform where the device
/// query prints nothing still reports the device's total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measurement {
    /// The process's resident set, or `None` when it could not be read.
    pub resident_mib: Option<u64>,
    /// What the process holds on the device, or `None` when it could not be
    /// read.
    pub device_mib: Option<u64>,
}

impl Measurement {
    /// Nothing could be read, which is what every child cost before this
    /// module existed.
    pub const UNKNOWN: Self = Self {
        resident_mib: None,
        device_mib: None,
    };

    /// The larger of the two sides, or `None` when neither is known.
    ///
    /// The larger rather than the sum, because weights mapped from a file
    /// count in the resident set *and*, once copied to the device, on the
    /// device -- so a sum would charge one model twice for the same bytes.
    /// The larger side is what the model costs on the memory it mostly lives
    /// in, which is what the budget's one number stands for.
    #[must_use]
    pub fn largest_mib(&self) -> Option<u64> {
        self.resident_mib.into_iter().chain(self.device_mib).max()
    }
}

/// The figures a test states, in place of a machine.
#[derive(Debug, Clone, Default)]
pub struct Fixed {
    /// What the device query answers, or nothing when there is no device.
    pub device: Option<DeviceMemory>,
    /// What the system memory query answers.
    pub system_total_mib: Option<u64>,
    /// What every process measures as, whatever its pid.
    pub measurement: Measurement,
}

/// Where the machine's own figures come from.
#[derive(Debug)]
pub enum Probe {
    /// The numbers a test wants, answered without running anything.
    Fixed(Fixed),
    /// The machine this router runs on, asked each time.
    Machine(Machine),
}

/// The tools this machine turned out to have.
#[derive(Debug)]
pub struct Machine {
    /// `nvidia-smi`, when the machine has one; where, so it is found once.
    nvidia_smi: Option<std::path::PathBuf>,
}

impl Probe {
    /// A probe that knows nothing, which is every test's default: the budget
    /// then behaves exactly as it did before the machine could be asked.
    #[must_use]
    pub fn none() -> Self {
        Self::Fixed(Fixed::default())
    }

    /// Looks for the tools this machine has, once.
    #[must_use]
    pub fn detect() -> Self {
        Self::Machine(Machine {
            nvidia_smi: command::nvidia_smi(),
        })
    }

    /// Total and used device memory, or `None` when there is no device or
    /// nothing can report it.
    #[must_use]
    pub fn device(&self) -> Option<DeviceMemory> {
        match self {
            Self::Fixed(fixed) => fixed.device,
            Self::Machine(machine) => {
                let text = command::query(machine.nvidia_smi.as_deref()?, command::DEVICE_QUERY)?;
                parse::device(&text)
            }
        }
    }

    /// What the machine has in system memory, in mebibytes, or `None` when
    /// nothing can report it.
    #[must_use]
    pub fn system_total_mib(&self) -> Option<u64> {
        match self {
            Self::Fixed(fixed) => fixed.system_total_mib,
            Self::Machine(_) => command::system_total_mib(),
        }
    }

    /// What one running process holds, on each side it can be read on.
    #[must_use]
    pub fn measure(&self, pid: u32) -> Measurement {
        match self {
            Self::Fixed(fixed) => fixed.measurement,
            Self::Machine(machine) => Measurement {
                resident_mib: command::resident_mib(pid),
                device_mib: machine
                    .nvidia_smi
                    .as_deref()
                    .and_then(|tool| command::query(tool, command::PROCESS_QUERY))
                    .and_then(|text| parse::compute_apps(&text, pid)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_probe_answers_with_what_it_was_built_with_for_any_pid() {
        let probe = Probe::Fixed(Fixed {
            device: Some(DeviceMemory {
                total_mib: 4096,
                used_mib: 1024,
            }),
            system_total_mib: Some(16384),
            measurement: Measurement {
                resident_mib: Some(700),
                device_mib: Some(300),
            },
        });

        assert_eq!(probe.device().map(|d| d.free_mib()), Some(3072));
        assert_eq!(probe.system_total_mib(), Some(16384));
        assert_eq!(probe.measure(1).largest_mib(), Some(700));
        assert_eq!(probe.measure(99_999).largest_mib(), Some(700));
    }

    #[test]
    fn a_probe_that_knows_nothing_reports_nothing() {
        let probe = Probe::none();
        assert_eq!(probe.device(), None);
        assert_eq!(probe.system_total_mib(), None);
        assert_eq!(probe.measure(1), Measurement::UNKNOWN);
    }

    #[test]
    fn the_largest_side_is_the_measurement_and_one_unknown_side_does_not_hide_the_other() {
        let both = Measurement {
            resident_mib: Some(4600),
            device_mib: Some(725),
        };
        assert_eq!(both.largest_mib(), Some(4600));

        let device_only = Measurement {
            resident_mib: None,
            device_mib: Some(725),
        };
        assert_eq!(device_only.largest_mib(), Some(725));
        assert_eq!(Measurement::UNKNOWN.largest_mib(), None);
    }

    /// The real probe against this test's own process: the one measurement
    /// that can be taken on any machine the tests run on.
    ///
    /// On the Unix platforms `ps` is part of the base system, so a resident
    /// set is expected. Elsewhere the figure may legitimately be unknown --
    /// what is asserted everywhere is that asking never fails loudly.
    #[test]
    fn the_machine_probe_measures_this_process_or_says_it_cannot() {
        let probe = Probe::detect();
        let measured = probe.measure(std::process::id());

        if cfg!(unix) {
            assert!(
                measured.resident_mib.is_some_and(|mib| mib > 0),
                "a running process has a resident set, and ps reads it: {measured:?}"
            );
        }
        assert!(
            measured.resident_mib.is_none_or(|mib| mib > 0),
            "a resident set is positive or unknown, never zero: {measured:?}"
        );
    }
}
