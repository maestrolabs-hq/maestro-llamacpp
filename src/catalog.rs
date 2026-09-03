//! The catalog: which models this router can serve, and how each is launched.
//!
//! This module is the whole of slice 1. Its interface is four types and one
//! function: parse text, get a `Catalog` back or a report naming everything
//! wrong with it. How TOML is walked, how an entry inherits the defaults
//! table, and how a location is refused are implementation and stay inside.
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

mod field;
mod path;
mod read;

pub use path::RelativePath;

use std::collections::BTreeMap;
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
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| Report::single(format!("the catalog is not valid TOML: {e}")))?;

        let mut problems = Vec::new();
        let version = read::version(&table, &mut problems);
        let defaults = read::defaults(&table, &mut problems);
        let entries = read::entries(&table, &defaults, &mut problems);

        if problems.is_empty() {
            Ok(Self {
                version: version.unwrap_or_default(),
                entries,
            })
        } else {
            Err(Report { problems })
        }
    }

    /// The entry with this identifier, if the catalog carries one.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}
