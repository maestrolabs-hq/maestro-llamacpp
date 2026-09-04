//! The read-only projections of what is loaded, moved here when `slots.rs`
//! ran out of room under the module-size gate. No decision lives here: this
//! only looks at what is loaded, through whatever lens a caller needs.

use crate::admission::Loaded as Held;
use crate::catalog::{Catalog, Entry};

use super::super::loaded::{Loaded, busy};
use super::Slots;

use std::sync::PoisonError;

impl Slots {
    /// The identifiers of the entries holding a child, in catalog order.
    pub(in super::super) fn loaded_ids(&self, catalog: &Catalog) -> Vec<String> {
        self.snapshot(catalog, |entry, _| entry.id.clone())
    }

    /// Every occupied slot, seen through `project`, in catalog order.
    ///
    /// Each slot is locked in turn, so this is a snapshot of several moments
    /// rather than a reading of one. The admission lock holds it still against
    /// other admissions and nothing else: the fast path takes no admission
    /// lock, so an entry read as idle here can have a reader before the
    /// decision reaches its slot. That is why `Slots::unload` reads the
    /// signal again at the moment it acts instead of trusting this.
    ///
    /// `project` is handed a borrow and never an owned handle, which keeps
    /// every caller clear of the slot invariant in [`super::super::loaded`] --
    /// a rule about where an `Arc` is cloned, which warns by name against
    /// listing what is loaded by handing out references.
    pub(super) fn snapshot<T>(
        &self,
        catalog: &Catalog,
        project: impl Fn(&Entry, &Loaded) -> T,
    ) -> Vec<T> {
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
    pub(super) fn held(&self, catalog: &Catalog) -> Vec<Held> {
        self.snapshot(catalog, |entry, held| {
            Held::of(entry, busy(&held.child), held.last_used)
        })
    }
}
