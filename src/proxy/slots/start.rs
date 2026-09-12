//! Starting one entry's child, measuring what it holds, and saying both.
//!
//! Split from `mod.rs` along the seam a load leaves: admission decides that
//! a child may be started, and this is the starting -- the only part of
//! serving that takes seconds to minutes, and the only part whose outcome an
//! operator cannot see from any request. So it is said out loud: that a load
//! began and what it was expected to cost, and that it finished and what it
//! turned out to cost.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::catalog::Entry;
use crate::launch::{Failure, Server};
use crate::memory::Measurement;

use super::super::loaded::Loaded;
use super::Slots;

/// Writes one line for the operator.
///
/// Written rather than printed, with the error dropped: a closed standard
/// output is not a reason to stop serving, and `println!` panics on one --
/// which would end whichever thread said the line, and a reaper thread that
/// ends is idle unloading that silently stops.
pub(in crate::proxy) fn say(line: &str) {
    drop(writeln!(std::io::stdout(), "{line}"));
}

impl Slots {
    /// Starts a child for this entry, and measures it once it is ready.
    ///
    /// The measurement is taken the moment the child answers, which is a
    /// floor rather than a peak: the context fills as the model is used.
    /// That is why admission counts the larger of the estimate and this.
    ///
    /// # Errors
    ///
    /// Returns the [`Failure`] the launcher returned, having said so.
    pub(super) fn start(
        &self,
        entry: &Entry,
        server: &Server,
        root: &Path,
    ) -> Result<Loaded, Failure> {
        say(&format!(
            "{}: loading, estimated at {} MiB",
            entry.id, entry.memory_estimate_mib
        ));
        let started = Instant::now();
        let child = match server.start(entry, root) {
            Ok(child) => child,
            Err(failure) => {
                say(&format!("{}: not loaded: {failure}", entry.id));
                return Err(failure);
            }
        };
        let measured = self.budget.probe().measure(child.pid());
        say(&ready(entry, started.elapsed(), &measured));
        Ok(Loaded {
            child: Arc::new(child),
            last_used: Instant::now(),
            measured,
        })
    }
}

/// The line a finished load says: how long it took, what the machine saw it
/// holding on each side, and what the catalog had said -- so an estimate
/// that is wrong is visible the first time the model loads rather than the
/// first time something is refused because of it.
fn ready(entry: &Entry, took: Duration, measured: &Measurement) -> String {
    format!(
        "{}: ready in {:.1} s, measured {} resident and {} on the device \
         (catalog said {} MiB)",
        entry.id,
        took.as_secs_f64(),
        gibibytes(measured.resident_mib),
        gibibytes(measured.device_mib),
        entry.memory_estimate_mib
    )
}

/// A figure in gibibytes to one decimal place, or the admission that there
/// is none.
///
/// Integer arithmetic on purpose: a tenth of a gibibyte is the precision
/// an operator reads at, and going through a float to print it would only
/// add a cast to explain.
fn gibibytes(mib: Option<u64>) -> String {
    match mib {
        Some(mib) => {
            let tenths = mib * 10 / 1024;
            format!("{}.{} GiB", tenths / 10, tenths % 10)
        }
        None => "nothing readable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RelativePath, Residency};
    use std::collections::BTreeMap;

    fn entry() -> Entry {
        Entry {
            id: "qwen3-06b".to_owned(),
            path: RelativePath::new("somewhere/model.gguf").expect("relative"),
            draft_path: None,
            projector_path: None,
            context_size: 4096,
            residency: Residency::OnDemand,
            memory_estimate_mib: 1024,
            reasoning_format: None,
            reasoning_effort: None,
            startup_timeout_seconds: 30,
            // The stock server: these fixtures are about other things.
            runtime: None,
            flags: BTreeMap::new(),
        }
    }

    #[test]
    fn the_ready_line_carries_both_measurements_and_what_the_catalog_said() {
        let line = ready(
            &entry(),
            Duration::from_millis(5_440),
            &Measurement {
                resident_mib: Some(4608),
                device_mib: Some(717),
            },
        );

        assert_eq!(
            line,
            "qwen3-06b: ready in 5.4 s, measured 4.5 GiB resident and 0.7 GiB \
             on the device (catalog said 1024 MiB)"
        );
    }

    #[test]
    fn a_side_that_could_not_be_read_says_so_rather_than_saying_zero() {
        let line = ready(&entry(), Duration::from_secs(1), &Measurement::UNKNOWN);

        assert!(
            line.contains("nothing readable resident and nothing readable on the device"),
            "an unreadable figure is named as such, because a zero here would \
             tell the operator the model is free: {line}"
        );
    }
}
