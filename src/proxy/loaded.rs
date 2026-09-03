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
/// Every path that hands out an `Arc<Child>` clones it while holding this
/// lock and never before. A reference may also be minted before insertion,
/// provided the slot's handle and the handed-out handle come into existence
/// together under the lock, so no reader can observe the slot between them.
///
/// Three paths in `slots` touch a slot's contents, and auditing this rule is
/// auditing them:
///
/// - `Slots::running`, the fast path, clones under the lock;
/// - `Slots::admit`, the slow path, mints the reference with the child and
///   inserts its clone under the lock in the same breath;
/// - `Slots::unload` and `Slots::clear`, behind eviction and `Router::stop`,
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
pub(super) type Slot = Mutex<Option<Loaded>>;

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
