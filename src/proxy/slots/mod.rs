//! Which child is loaded for which entry, and what has to go to make room.
//!
//! One type, holding the part of serving that is about memory rather than
//! about HTTP. A caller asks for the child that serves an entry and gets one,
//! or gets told why not; whether that meant finding a running process,
//! starting one, or ending somebody else's first is this module's business
//! and nothing else's.
//!
//! The policy itself is not here. `admission` decides what may be loaded from
//! four values and no machine at all; this acts on that decision, which is the
//! half that kills processes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crate::admission::Budget;
use crate::catalog::{Catalog, Entry};
use crate::launch::{Child, Failure, Server};
use crate::queue::Wait;

use super::loaded::{Slot, Take, live_child, take_if_idle};

mod room;
mod start;
mod sweep;
mod view;

pub(in crate::proxy) use start::say;

/// Every entry's slot, and the budget they compete for.
pub(super) struct Slots {
    /// One slot per catalog entry, built once and never added to.
    ///
    /// The catalog is fixed for the life of the router, so the set of keys
    /// never changes and the map itself needs no lock -- only its values do.
    /// That is what lets a request for one entry proceed while another entry
    /// is loading, which a single map lock could not do.
    by_id: HashMap<String, Slot>,
    /// Serialises starting children, and nothing else.
    ///
    /// Two loads at once compete for the same memory, so admitting one at a
    /// time is the correct behaviour rather than a limitation: a decision made
    /// while another load is in flight is a decision about a machine state
    /// that no longer holds.
    ///
    /// Taken before any slot lock and never held across a relay, which is the
    /// whole deadlock argument: one lock order, so no cycle.
    admission: Mutex<()>,
    budget: Budget,
    wait: Wait,
}

impl Slots {
    /// One slot per entry the catalog carries, all of them empty.
    pub(super) fn new(catalog: &Catalog, budget: Budget, wait: Wait) -> Self {
        Self {
            by_id: catalog
                .entries
                .iter()
                .map(|entry| (entry.id.clone(), Mutex::new(None)))
                .collect(),
            admission: Mutex::new(()),
            budget,
            wait,
        }
    }

    /// The child serving this entry, started if there is room for it.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when a child cannot be started, does not become
    /// ready, or is refused for want of room.
    pub(super) fn child(
        &self,
        catalog: &Catalog,
        entry: &Entry,
        server: &Server,
        root: &Path,
    ) -> Result<Arc<Child>, Failure> {
        if let Some(child) = self.running(entry) {
            return Ok(child);
        }
        self.admit(catalog, entry, server, root)
    }

    /// Ends every child, and forgets them.
    ///
    /// Every slot is emptied first and the children dropped afterwards, so
    /// no slot's guard is held while a process is being killed and waited
    /// for -- the same rule [`take_if_idle`] keeps, for the same reason.
    /// Which entries are loaded right now, by id.
    ///
    /// Ids rather than handles, deliberately: the slot invariant in
    /// [`super::loaded`] is a rule about where an `Arc` may be cloned, and
    /// handing out references to list what is running is exactly what it
    /// warns against. A caller asking this wants to report, not to serve.
    pub(super) fn loaded(&self, catalog: &Catalog) -> Vec<String> {
        self.snapshot(catalog, |entry, _| entry.id.clone())
    }

    pub(super) fn clear(&self) {
        let taken: Vec<_> = self
            .by_id
            .values()
            .filter_map(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take())
            .collect();
        drop(taken);
    }

    /// The slot for an entry, which exists because the catalog named it.
    ///
    /// # Panics
    ///
    /// If the entry is not in the catalog the slots were built from, which
    /// cannot happen: every caller reached this by looking the entry up in
    /// that same catalog.
    pub(super) fn slot(&self, id: &str) -> &Slot {
        self.by_id.get(id).expect("one slot per catalog entry")
    }

    /// The child already running for this entry, if there is a live one.
    ///
    /// The fast path, and the common one. No admission lock is taken, so a
    /// request for a loaded model waits on nothing but its own slot.
    fn running(&self, entry: &Entry) -> Option<Arc<Child>> {
        live_child(self.slot(&entry.id))
    }

    /// Starts a child for this entry, unloading what has to go first.
    ///
    /// The slow path. The admission lock is taken for the whole decision, so
    /// two requests cannot each read a machine state the other is about to
    /// change, and released before anything is relayed.
    fn admit(
        &self,
        catalog: &Catalog,
        entry: &Entry,
        server: &Server,
        root: &Path,
    ) -> Result<Arc<Child>, Failure> {
        let _admitting = self
            .admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        // Checked again under the admission lock, because another request may
        // have started this very entry while this one waited for the lock.
        if let Some(child) = self.running(entry) {
            return Ok(child);
        }

        // Before the decision, because the decision ends processes and this
        // does not. A stale path, an unmounted root or a half-finished
        // download would otherwise unload the operator's warm model and then
        // answer 502, leaving them with neither. What cannot be prevented here
        // is a start that fails later -- a timeout, or a model that costs more
        // than its estimate -- because those are only knowable by trying.
        Server::model_file(entry, root)?;

        // Room is made here rather than decided here: when what holds it is
        // busy rather than resident, this waits for it. The admission lock is
        // held throughout, which is what makes waiting correct rather than
        // merely patient -- a second request that would compete for the same
        // memory queues behind this one instead of racing it to the same
        // conclusion.
        self.make_room(catalog, entry)?;

        let loaded = self.start(entry, server, root)?;
        // The handed-out handle and the slot's own come into existence
        // together under the lock, which is the slot invariant in `loaded`.
        let mut slot = self
            .slot(&entry.id)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let child = Arc::clone(&loaded.child);
        *slot = Some(loaded);
        Ok(child)
    }

    /// Unloads the named entries, or names the one that stopped it.
    ///
    /// Taking the `Loaded` out drops the router's `Arc`, and a child whose
    /// last reference goes is killed by its own `Drop`. Done before the wanted
    /// child is started, which is the point: the room has to be free before
    /// something is put in it. Each drop happens here, once its slot's guard
    /// has been released, so a kill that hangs holds the admission lock this
    /// runs under -- which it has to, since the room is not free until the
    /// process is gone -- and nothing else.
    ///
    /// Each is taken by [`take_if_idle`](super::loaded::take_if_idle), which
    /// re-reads the busy signal for the reason [`Slots::held`] records.
    ///
    /// # Errors
    ///
    /// Returns the entry that had gained a reader, having unloaded whatever
    /// it reached before that one. Those were idle when they were taken, so
    /// ending them was allowed; what is lost is the work of starting them
    /// again, which is the price of not silently overcommitting. Naming the
    /// blocker is what lets the refusal say which model is holding the room,
    /// rather than only that something is.
    fn unload<'a>(&self, ids: &'a [String]) -> Result<(), &'a str> {
        for id in ids {
            match take_if_idle(self.slot(id), |_| true) {
                Take::Taken(child) => drop(child),
                Take::Busy => return Err(id),
                // Gone already, by a sweep or another admission: the room
                // this wanted is there, which is all this asked for.
                Take::Empty => {}
            }
        }
        Ok(())
    }
}
