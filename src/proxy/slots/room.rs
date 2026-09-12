//! Making room for a model, and waiting when the room is held.
//!
//! Moved out of `mod.rs` when the waiting arrived and the module-size gate
//! said so. What lives here is one question -- can this entry be started now,
//! and if not, is that permanent? -- and the loop that asks it again.
//!
//! Two refusals that look alike and are not. A model larger than the whole
//! budget will never fit, however long anybody waits, and is refused at once.
//! A model whose room is held by something still answering will fit as soon as
//! that answer ends, and waiting for it is the difference between a router
//! that queues and one that tells every caller to write a retry loop.

use std::time::{Duration, Instant};

use crate::admission::{Decision, Wanted};
use crate::catalog::{Catalog, Entry};
use crate::launch::Failure;

use super::{Slots, say};

/// How often the wait re-asks whether the room has freed up.
///
/// Polled rather than woken, because what it waits for is a relay in another
/// thread dropping the last handle on a child, and nothing signals that today.
/// A quarter second is far below a caller noticing and far above spinning: a
/// sixty-second wait costs two hundred and forty decisions, each of which is
/// arithmetic over a handful of slots.
const SWEEP: Duration = Duration::from_millis(250);

impl Slots {
    /// Makes room for this entry, waiting for it if the wait allows.
    ///
    /// Returns once there is room. The caller holds the admission lock
    /// throughout, which is what makes the wait correct rather than merely
    /// patient: a second request that would compete for the same memory is
    /// held behind it rather than racing it to the same conclusion. The fast
    /// path takes no admission lock, so a request for a model that is already
    /// loaded is not delayed by any of this.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure::Refused`] when the entry cannot fit at all, and a
    /// [`Failure::Contended`] when the wait expired with its room still held.
    /// The difference is what a caller should do next, and the proxy carries
    /// it as a `Retry-After` on the one a retry can fix.
    pub(super) fn make_room(&self, catalog: &Catalog, entry: &Entry) -> Result<(), Failure> {
        let deadline = Instant::now() + self.wait.duration();

        loop {
            // Asked again on every pass, not once before the loop. What this
            // waits for is memory being released, and the device is the only
            // thing that knows when that has actually happened: a child that
            // has exited frees its pages whether or not the ledger has caught
            // up. Reading it once would wait on a figure that cannot change.
            let device_free_mib = self.budget.probe().device().map(|device| device.free_mib());

            let held =
                match self
                    .budget
                    .admit(&self.held(catalog), &Wanted::of(entry), device_free_mib)
                {
                    Decision::Fits => return Ok(()),
                    Decision::Unload(ids) => {
                        say(&format!(
                            "{}: unloading {} to make room",
                            entry.id,
                            ids.join(", ")
                        ));
                        match self.unload(&ids) {
                            Ok(()) => return Ok(()),
                            // Something started reading the model whose room this
                            // wanted, between the decision and the taking. It will
                            // stop; the only question is whether this caller is
                            // still here when it does.
                            //
                            // "reached first" distinguishes this from the case
                            // where the snapshot already saw it busy, which took
                            // nothing. `tests/eviction.rs` tells the two apart by
                            // that phrase, because what they leave behind differs.
                            Err(blocker) => format!(
                                "'{}' needs room held by '{blocker}', which a \
                             request reached first",
                                entry.id
                            ),
                        }
                    }
                    // Something holding the room is on-demand and busy, so it
                    // becomes a candidate the moment its reader is done. Waiting
                    // for that is the whole point of this loop.
                    Decision::Blocked(message) => message,
                    // Larger than the whole budget, or held by residents that
                    // never become candidates. Waiting changes nothing.
                    Decision::Refuse(message) => return Err(Failure::Refused(message)),
                };

            if Instant::now() >= deadline {
                return Err(refused(entry, &held, self.wait.duration()));
            }
            std::thread::sleep(SWEEP);
        }
    }
}

/// The refusal a caller sees when the wait ran out.
///
/// Carries what admission said was holding the room, because "try again"
/// without a subject tells an operator nothing about whether trying again is
/// worth it. Names the wait too: a caller held for a minute should not be left
/// wondering whether it waited at all.
///
/// Contended rather than refused, and the distinction is the whole of what a
/// caller does next. Every path that reaches here was held by something that
/// will finish -- a reader that got there first, or an on-demand entry still
/// busy -- so the answer changes on its own and a retry is the right response.
/// A [`Failure::Refused`] is the other kind: larger than the budget, or held
/// by residents that never become candidates, where waiting changes nothing.
/// The proxy reads that difference and sends `Retry-After` on this one alone.
fn refused(entry: &Entry, held: &str, waited: Duration) -> Failure {
    if waited.is_zero() {
        return Failure::Contended(format!(
            "{held}; this may succeed on a retry, or set \
             MAESTRO_ADMISSION_WAIT_SECONDS to wait for the room"
        ));
    }
    Failure::Contended(format!(
        "{held}; '{}' waited {}s and the room did not free up",
        entry.id,
        waited.as_secs()
    ))
}
