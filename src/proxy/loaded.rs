//! What a loaded child is, and how "somebody is reading from it" is known.
//!
//! One entry's worth of state, and the one rule that state depends on. The
//! table that holds these and decides which of them may exist is `slots`; this
//! is only what sits in one of its cells.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::launch::Child;

/// A child this router has running, and what deciding its fate needs.
pub(super) struct Loaded {
    pub(super) child: Arc<Child>,
    /// When it last answered, so the coldest is unloaded first.
    pub(super) last_used: Instant,
}

/// One entry's child, held apart from every other entry's.
///
/// # Invariant
///
/// A reference to a loaded child is obtainable only by locking its slot.
///
/// This is the rule that makes `Arc::strong_count` mean "somebody is reading
/// from this child". While the lock is held no new reference can be taken, so
/// a count of one says the slot's own handle is the only one alive and the
/// child is idle. Hand a reference out anywhere else -- cache one, clone one
/// without the lock -- and the count stops answering that question, at which
/// point eviction can kill a process somebody is still reading from, and the
/// caller sees a stream stop early with no way to tell it from a model that
/// finished.
///
/// The compiler cannot keep this rule, because it is about where clones are
/// made rather than about types. So the type says it, and
/// `a_slot_reports_busy_while_a_reference_it_handed_out_lives` is named for
/// the rule a reader would be breaking.
pub(super) type Slot = Mutex<Option<Loaded>>;

/// Whether anything besides its slot is holding this child.
///
/// Where the slot's invariant is cashed in: the count answers "is somebody
/// reading from this" only because references are handed out under the slot
/// lock and nowhere else. Named and called from one place so that rule has
/// somewhere to live.
///
/// Generic because what is counted is the handle. A `Child` contributes a
/// process and nothing to the count, so the rule can be driven below without
/// spawning one.
pub(super) fn busy<T>(handle: &Arc<T>) -> bool {
    Arc::strong_count(handle) > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot's invariant, driven the way a relay drives it.
    ///
    /// Not a test that `Arc` counts correctly: it does, and that is the
    /// standard library's business. This exists so that a reader who changes
    /// where references are handed out finds a failing test named for the rule
    /// they are about to break -- because the cost of breaking it is a process
    /// killed while somebody is reading from it, which shows up as a stream
    /// that stopped early and nothing else.
    #[test]
    fn a_slot_reports_busy_while_a_reference_it_handed_out_lives() {
        // Stands in for a loaded child, because the count is the whole subject
        // and a real one would spawn a process to say the same thing.
        let slot: Mutex<Option<Arc<&str>>> = Mutex::new(Some(Arc::new("a child")));

        let reading = {
            let held = slot.lock().expect("a fresh mutex is not poisoned");
            let held = held.as_ref().expect("something is loaded");
            assert!(!busy(held), "nothing has been handed out yet");
            // Cloned under the lock, which is the only place the invariant
            // allows a reference to be taken.
            Arc::clone(held)
        };

        {
            let held = slot.lock().expect("a fresh mutex is not poisoned");
            let held = held.as_ref().expect("something is loaded");
            assert!(
                busy(held),
                "a reference is out, so this child must not be unloaded"
            );
        }

        drop(reading);

        let held = slot.lock().expect("a fresh mutex is not poisoned");
        let held = held.as_ref().expect("something is loaded");
        assert!(
            !busy(held),
            "the reader finished, so the child is a candidate again"
        );
    }
}
