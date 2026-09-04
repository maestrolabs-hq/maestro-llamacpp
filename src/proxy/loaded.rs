//! What a loaded child is, and how "somebody is reading from it" is known.
//!
//! One entry's worth of state, and the one rule that state depends on. The
//! table that holds these and decides which of them may exist is `slots`; this
//! is only what sits in one of its cells.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::launch::{Child, Liveness};

/// A child this router has running, and what deciding its fate needs.
///
/// Generic over what is held, defaulting to the `Child` the router serves
/// from. Nothing in taking a slot is about a process -- it is about whether a
/// reference has been handed out -- so saying that in the type lets the rule
/// below be driven from a unit test rather than from a spawned server, a port
/// and a race.
pub(super) struct Loaded<C = Child> {
    pub(super) child: Arc<C>,
    /// When it last answered, so the coldest is unloaded first.
    pub(super) last_used: Instant,
}

/// One entry's child, held apart from every other entry's.
///
/// # Invariant
///
/// Every path that hands out an `Arc<Child>` clones it while holding this
/// lock and never before. A reference may also be minted before insertion,
/// provided the slot's handle and the handed-out handle come into existence
/// together under the lock, so no reader can observe the slot between them.
///
/// Three paths in `slots` touch a slot's contents, and auditing this rule is
/// auditing them:
///
/// - [`live_child`], the fast path, clones under the lock;
/// - `Slots::admit`, the slow path, mints the reference with the child and
///   inserts its clone under the lock in the same breath;
/// - [`take_if_idle`] and `Slots::clear`, behind eviction and `Router::stop`,
///   take under the lock and hand nothing back.
///
/// This is the rule that makes `Arc::strong_count` mean "somebody is reading
/// from this child": a count of one says the slot's own handle is the only
/// one alive and the child is idle. Hand a reference out anywhere else --
/// cache one, clone one without the lock, return one from a future endpoint
/// that lists what is loaded -- and the count stops answering that question,
/// at which point eviction can empty the slot of a process somebody is still
/// reading from. The router then believes it freed memory it did not.
///
/// The compiler cannot keep this rule, because it is about where clones are
/// made rather than about types. So the type says it, and the gate that fails
/// when it breaks is `a_child_with_a_stream_in_flight_is_not_unloaded` in
/// `tests/eviction.rs`, which drives a real reader against a real decision.
pub(super) type Slot<C = Child> = Mutex<Option<Loaded<C>>>;

/// Whether anything besides its slot is holding this child.
///
/// Where the slot's invariant is cashed in: the count answers "is somebody
/// reading from this" only because references are handed out under the slot
/// lock and nowhere else. Named so that rule has somewhere to live, and called
/// from the two places that act on it -- `Slots::held`, which reads it for a
/// decision, and `Slots::unload`, which reads it again at the moment it stops
/// being reversible.
///
/// Generic because what is counted is the handle rather than what it points
/// at, and saying so keeps a `Child`'s process out of a question that is only
/// about references.
pub(super) fn busy<T>(handle: &Arc<T>) -> bool {
    Arc::strong_count(handle) > 1
}

/// The live child in a slot, if there is one, marked as used just now.
///
/// The fast path: one slot's lock and nothing else, so a request for a model
/// that is already loaded waits on no admission and on no other entry.
///
/// Where the slot invariant is kept rather than described -- the clone below
/// happens under the lock this function holds, which is what lets
/// `Arc::strong_count` mean "somebody is reading from this child" everywhere
/// else.
///
/// Concrete in `Child` where its neighbours are generic, because liveness is
/// the one question here that is about a process rather than about references.
pub(super) fn live_child(slot: &Slot) -> Option<Arc<Child>> {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let held = slot.as_mut()?;

    // Liveness while the lock is held, so a child that exited since it last
    // answered is not handed to a relay that will fail on it.
    //
    // Only when nothing else holds a reference: `try_wait` needs the process
    // mutably, and `Arc::get_mut` succeeds exactly when the slot's handle is
    // the only one. So a child that already has a reader goes unchecked -- the
    // count proves a reader, not a live process -- and a dead one is handed on
    // for the relay's own connection to discover.
    if let Some(child) = Arc::get_mut(&mut held.child)
        && matches!(child.check(), Liveness::Exited(_))
    {
        *slot = None;
        return None;
    }

    held.last_used = Instant::now();
    Some(Arc::clone(&held.child))
}

/// Empties a slot, unless something started reading from what is in it or the
/// caller's own condition refuses it.
///
/// The moment the busy signal stops being reversible, which is why it is read
/// here rather than trusted from the snapshot a decision was made against. A
/// signal read under one lock acquisition and acted on under another is a
/// signal about a moment that has passed: between the two, a request can reach
/// the fast path and take a reference. Emptying the slot then would leave a
/// process running that nothing accounts for, and the router would go over the
/// budget it believes it is keeping.
///
/// The same hazard applies to any other signal a caller wants to act on. The
/// idle reaper narrows on `last_used`, and a snapshot's `last_used` is exactly
/// as stale as its busy signal -- for a policy whose whole question is
/// freshness, that staleness cannot be trusted either.
///
/// `also` is checked in addition to the hard-coded busy check, never instead
/// of it: a caller may only narrow what is takeable, never widen it. Budget
/// eviction supplies `|_| true`, so its behaviour is exactly what it was
/// before this took a second argument.
///
/// Returns `false` and leaves the slot exactly as it was when the child is
/// busy or `also` refuses it. An empty slot is nothing to take and counts as
/// taken.
pub(super) fn take_if_idle<C>(slot: &Slot<C>, also: impl FnOnce(&Loaded<C>) -> bool) -> bool {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.as_ref() {
        // Somebody started reading after the snapshot was taken, or the
        // caller's own condition no longer holds -- either way this room is
        // not the decision's to give away.
        Some(held) if busy(&held.child) || !also(held) => false,
        _ => {
            slot.take();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A slot holding one handle, as a loaded entry does.
    ///
    /// The unit is `()` rather than a `Child`: what a slot decides turns on
    /// how many references exist, never on what they point at, so a process
    /// here would only make the test slower and the failure less clear.
    fn occupied() -> (Slot<()>, Arc<()>) {
        let child = Arc::new(());
        let slot = Mutex::new(Some(Loaded {
            child: Arc::clone(&child),
            last_used: Instant::now(),
        }));
        (slot, child)
    }

    #[test]
    fn a_slot_whose_child_gained_a_reader_is_refused_and_left_as_it_was() {
        // The reader is the second reference, held across the call the way a
        // relay holds one across a response.
        let (slot, reader) = occupied();

        assert!(
            !take_if_idle(&slot, |_| true),
            "a child somebody is reading from is not the decision's to take"
        );
        assert!(
            slot.lock().expect("an unpoisoned slot").is_some(),
            "and the slot still holds it, so the budget still counts it. An              emptied slot here is the defect this guards: the process keeps              running and the router believes it freed the memory"
        );
        drop(reader);
    }

    #[test]
    fn a_slot_nobody_is_reading_from_is_emptied() {
        let (slot, reader) = occupied();
        drop(reader);

        assert!(take_if_idle(&slot, |_| true), "an idle child is taken");
        assert!(
            slot.lock().expect("an unpoisoned slot").is_none(),
            "and the slot is empty, which is what makes the room real"
        );
    }

    #[test]
    fn a_slot_whose_child_was_used_after_the_cutoff_is_refused_and_left_as_it_was() {
        let (slot, reader) = occupied();
        drop(reader);

        let cutoff = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .expect("a process that has run for less than the test ages");

        assert!(
            !take_if_idle(&slot, |held| held.last_used <= cutoff),
            "used after the cutoff, so the caller's additional condition \
             refuses it even though nothing is reading from it"
        );
        assert!(
            slot.lock().expect("an unpoisoned slot").is_some(),
            "and the slot still holds it"
        );
    }
}
