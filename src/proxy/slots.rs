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
use std::time::Instant;

// Renamed: a slot holds a `Loaded`, and so does a decision. Two of them here
// under one name is how a reader loses track of which is which.
use crate::admission::{Budget, Decision, Loaded as Held};
use crate::catalog::{Catalog, Entry};
use crate::launch::{Child, Failure, Server};

use super::loaded::{Loaded, Slot, busy, live_child, take_if_idle};

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
}

impl Slots {
    /// One slot per entry the catalog carries, all of them empty.
    pub(super) fn new(catalog: &Catalog, budget: Budget) -> Self {
        Self {
            by_id: catalog
                .entries
                .iter()
                .map(|entry| (entry.id.clone(), Mutex::new(None)))
                .collect(),
            admission: Mutex::new(()),
            budget,
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

    /// The identifiers of the entries holding a child, in catalog order.
    pub(super) fn loaded_ids(&self, catalog: &Catalog) -> Vec<String> {
        self.snapshot(catalog, |entry, _| entry.id.clone())
    }

    /// Ends every child, and forgets them.
    pub(super) fn clear(&self) {
        for slot in self.by_id.values() {
            slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        }
    }

    /// The slot for an entry, which exists because the catalog named it.
    ///
    /// # Panics
    ///
    /// If the entry is not in the catalog the slots were built from, which
    /// cannot happen: every caller reached this by looking the entry up in
    /// that same catalog.
    fn slot(&self, id: &str) -> &Slot {
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

        match self
            .budget
            .admit(&self.held(catalog), &entry.id, entry.memory_estimate_mib)
        {
            Decision::Fits => {}
            Decision::Unload(ids) => {
                if let Err(blocker) = self.unload(&ids) {
                    return Err(Failure::Refused(format!(
                        "'{}' needs room held by '{blocker}', which a request \
                         reached first; this may succeed on a retry",
                        entry.id
                    )));
                }
            }
            Decision::Refuse(message) => return Err(Failure::Refused(message)),
        }

        let child = Arc::new(server.start(entry, root)?);
        *self
            .slot(&entry.id)
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Loaded {
            child: Arc::clone(&child),
            last_used: Instant::now(),
        });
        Ok(child)
    }

    /// Every occupied slot, seen through `project`, in catalog order.
    ///
    /// Each slot is locked in turn, so this is a snapshot of several moments
    /// rather than a reading of one. The admission lock holds it still against
    /// other admissions and nothing else: the fast path takes no admission
    /// lock, so an entry read as idle here can have a reader before the
    /// decision reaches its slot. That is why [`Slots::unload`] reads the
    /// signal again at the moment it acts instead of trusting this.
    ///
    /// `project` is handed a borrow and never an owned handle, which keeps
    /// every caller clear of the slot invariant in [`super::loaded`] -- a rule
    /// about where an `Arc` is cloned, which warns by name against listing
    /// what is loaded by handing out references.
    fn snapshot<T>(&self, catalog: &Catalog, project: impl Fn(&Entry, &Loaded) -> T) -> Vec<T> {
        catalog
            .entries
            .iter()
            .filter_map(|entry| {
                let slot = self
                    .slot(&entry.id)
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let held = slot.as_ref()?;
                Some(project(entry, held))
            })
            .collect()
    }

    /// What is loaded now, as admission needs to see it.
    fn held(&self, catalog: &Catalog) -> Vec<Held> {
        self.snapshot(catalog, |entry, held| {
            Held::of(entry, busy(&held.child), held.last_used)
        })
    }

    /// Unloads the named entries, or names the one that stopped it.
    ///
    /// Taking the `Loaded` out drops the router's `Arc`, and a child whose
    /// last reference goes is killed by its own `Drop`. Done before the wanted
    /// child is started, which is the point: the room has to be free before
    /// something is put in it.
    ///
    /// Each is taken by [`take_if_idle`](super::loaded::take_if_idle), which
    /// re-reads the busy signal for the reason [`Slots::snapshot`] records.
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
            if !take_if_idle(self.slot(id), |_| true) {
                return Err(id);
            }
        }
        Ok(())
    }
}
