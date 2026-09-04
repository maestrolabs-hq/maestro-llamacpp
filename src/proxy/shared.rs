//! What `Shared` itself answers, apart from either connection handling in
//! `answer` or the memory bookkeeping in `slots`.
//!
//! Split out when `answer.rs` grew past the module-size gate: these two
//! methods are about `Shared` rather than about answering a connection, and
//! moving them here keeps that gate meaningful rather than merely satisfied.

use std::sync::Arc;

use super::Shared;
use crate::catalog::Entry;
use crate::launch::{Child, Failure};

impl Shared {
    /// The child serving this entry, started if there is room for it.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when a child cannot be started, does not become
    /// ready, or is refused for want of room.
    pub(super) fn child(&self, entry: &Entry) -> Result<Arc<Child>, Failure> {
        self.slots
            .child(&self.catalog, entry, &self.server, &self.root)
    }

    /// What the catalog carries, for a refusal that can be acted on.
    pub(super) fn known(&self) -> String {
        self.catalog
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
