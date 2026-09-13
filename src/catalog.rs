//! The catalog: which models this router can serve, and how each is launched.
//!
//! This module began as the whole of slice 1, and its interface is still
//! small: parse text, get a `Catalog` back or a report naming everything
//! wrong with it; or read the same text against a models root, and get back
//! the catalog with every estimate settled and every model file the root
//! carries beside it. How TOML is walked, how an entry inherits the defaults
//! table, how a location is refused, how an estimate is derived and how a
//! file becomes an entry are implementation and stay inside.
//!
//! Two properties are part of the interface rather than the implementation,
//! because a caller cannot use the module correctly without knowing them.
//!
//! A report names every problem, not the first. A catalog with five mistakes
//! is fixed in one pass rather than five, which is the difference between a
//! tool people run and one they work around.
//!
//! Every problem names the entry it came from and the field that caused it.
//! An error reading "invalid catalog" sends the reader back to the file to
//! guess, which is the failure this design exists to avoid.

mod capability;
mod discover;
mod estimate;
mod field;
mod path;
mod read;
mod resolve;

pub use path::RelativePath;
pub use resolve::{EstimateSource, Reading};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Everything wrong with one catalog, gathered in a single pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    problems: Vec<String>,
}

impl Report {
    /// A report carrying one problem, for failures that stop the parse.
    pub(crate) fn single(problem: String) -> Self {
        Self {
            problems: vec![problem],
        }
    }

    /// The problems, in the order they were found.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, problem) in self.problems.iter().enumerate() {
            if n > 0 {
                writeln!(f)?;
            }
            write!(f, "{problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

/// Whether a model is held loaded or loaded when something asks for it.
///
/// A resident model is never evicted, which is what lets a small model answer
/// immediately while larger ones come and go around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Residency {
    /// Loaded at startup and never evicted.
    Resident,
    /// Loaded on first use, and evictable afterwards.
    OnDemand,
}

impl Residency {
    /// The spelling used in a catalog, and the only two accepted.
    pub(crate) const NAMES: [&'static str; 2] = ["resident", "on-demand"];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "resident" => Some(Self::Resident),
            "on-demand" => Some(Self::OnDemand),
            _ => None,
        }
    }
}

/// One model the router can serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Names the model, never the role it happens to serve.
    pub id: String,
    /// The model weights.
    pub path: RelativePath,
    /// The smaller model used for speculative decoding, when there is one.
    pub draft_path: Option<RelativePath>,
    /// The multimodal projector, when the model takes more than text.
    pub projector_path: Option<RelativePath>,
    /// Tokens of context the server is started with.
    pub context_size: u32,
    /// Whether this model is held loaded.
    pub residency: Residency,
    /// What loading this model is expected to cost, in mebibytes.
    pub memory_estimate_mib: u32,
    /// How reasoning output is delimited, when the model produces any.
    pub reasoning_format: Option<String>,
    /// How much reasoning effort to ask for, when the model accepts a level.
    pub reasoning_effort: Option<String>,
    /// How long this model may take to become ready before the router gives up
    /// on it and says so.
    ///
    /// Per entry rather than global because startup time varies by two orders
    /// of magnitude: a small model answers in under a second, a large one on a
    /// cold page cache takes minutes. One value would be either too tight for
    /// the large entries or meaningless for the small ones.
    pub startup_timeout_seconds: u32,
    /// Which build of the server this entry needs, when it needs a particular
    /// one.
    ///
    /// A name, never a path: the catalog describes a set of models without
    /// naming the machine they sit on, and a path to a binary is the most
    /// machine-specific thing there is. The name selects `llama-server-<name>`
    /// on the search path, so an operator points it at their build the way
    /// they point at everything else -- by putting it where the router looks.
    ///
    /// `None` uses the server the router was started with. An entry needing a
    /// patched build -- speculative decoding against a sidecar the stock
    /// server cannot load -- names it, and the rest never think about it.
    pub runtime: Option<String>,
    /// Server settings this router passes through without interpreting.
    pub flags: BTreeMap<String, String>,
}

/// Every model the router can serve, and the settings they share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// The schema version this file was written against.
    pub version: u32,
    /// The entries, ordered by identifier so two reads agree.
    pub entries: Vec<Entry>,
    /// The entries whose estimate was derived from their files.
    ///
    /// Kept beside the entries rather than on them: an entry is what the
    /// router serves, and where a figure came from is a fact about one
    /// reading of it.
    derived: BTreeSet<String>,
    /// The entries that came from the models root rather than the text.
    discovered: BTreeSet<String>,
}

impl Catalog {
    /// What the entries held loaded reserve, in mebibytes.
    ///
    /// A catalog fact rather than a machine one: it is the sum of what the
    /// resident entries say they cost, with no ceiling anywhere near it. What
    /// that sum means for a particular machine is the budget's business.
    ///
    /// Widened to sum, because estimates that each fit in a `u32` need not sum
    /// into one, and an overflow here would report a reservation of almost
    /// nothing.
    #[must_use]
    pub fn resident_reservation_mib(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.residency == Residency::Resident)
            .map(|entry| u64::from(entry.memory_estimate_mib))
            .sum()
    }

    /// Reads a catalog, reporting everything wrong with it.
    ///
    /// # Errors
    ///
    /// Returns a [`Report`] naming every problem found. Text that is not TOML
    /// stops the parse and yields that one problem, because nothing further
    /// can be read; every other failure is collected, so one run of the tool
    /// surfaces one round of mistakes.
    pub fn parse(text: &str) -> Result<Self, Report> {
        let mut drafts = read::drafts(text)?;
        // Text alone cannot derive an estimate; with no root to read the
        // files from, an entry that declares none is refused.
        for id in &drafts.undeclared {
            drafts.problems.push(field::problem(
                &format!("entry '{id}'"),
                "memory_estimate_mib",
                "is required, and no default supplies it",
            ));
        }

        if drafts.problems.is_empty() {
            Ok(Self {
                version: drafts.version.unwrap_or_default(),
                entries: drafts.entries,
                derived: BTreeSet::new(),
                discovered: BTreeSet::new(),
            })
        } else {
            Err(Report {
                problems: drafts.problems,
            })
        }
    }

    /// The entry with this identifier, if the catalog carries one.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Where this entry's estimate came from, if the catalog carries it.
    #[must_use]
    pub fn estimate_source(&self, id: &str) -> Option<EstimateSource> {
        self.entry(id)?;
        Some(if self.derived.contains(id) {
            EstimateSource::Derived
        } else {
            EstimateSource::Declared
        })
    }

    /// Whether this entry was found under the models root rather than
    /// written in the catalog.
    #[must_use]
    pub fn is_discovered(&self, id: &str) -> bool {
        self.discovered.contains(id)
    }
}
