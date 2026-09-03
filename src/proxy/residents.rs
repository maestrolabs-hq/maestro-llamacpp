//! Loading the entries the catalog holds loaded.
//!
//! Startup rather than traffic, which is why it is here and not beside the
//! accept loop: this runs once, before anything has been asked for, and what
//! it decides is which children exist before the first caller arrives.
//!
//! It reaches children through the same path a request does. That is the whole
//! of its concurrency argument -- there is none of its own.

use std::sync::PoisonError;
use std::time::Instant;

use crate::catalog::Residency;

use super::Shared;

/// Loads every resident entry, reporting and recording what failed.
///
/// Through [`Shared::child`], the same path a request takes, rather than
/// around it. A request for a resident that arrives mid-load blocks on the
/// admission lock, and the re-check under that lock finds the child already
/// running -- so the race is handled where it was handled before, and this
/// adds no coordination of its own.
///
/// A resident that cannot load leaves the router serving everything else.
/// Refusing to serve at all would let one missing file deny service to every
/// other model, which is a worse outcome than the one it prevents; the
/// operator learns at startup instead of when the first caller arrives.
///
/// Each outcome is printed because a cold load's cost has no other way to
/// reach the operator, and each failure is recorded as well because this runs
/// on a thread whose output belongs to nobody's call.
pub(super) fn load(shared: &Shared) {
    for entry in shared
        .catalog
        .entries
        .iter()
        .filter(|entry| entry.residency == Residency::Resident)
    {
        let started = Instant::now();
        match shared.child(entry) {
            Ok(_) => println!(
                "resident {} loaded in {:.1} seconds",
                entry.id,
                started.elapsed().as_secs_f64()
            ),
            Err(failure) => {
                let reported = format!("{}: {failure}", entry.id);
                eprintln!("resident {reported}");
                eprintln!("  serving the rest of the catalog without it");
                shared
                    .resident_failures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(reported);
            }
        }
    }
}
