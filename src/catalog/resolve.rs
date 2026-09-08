//! Reading a catalog against the models root it will be served from.
//!
//! [`Catalog::parse`] reads text and nothing else, so a test can hand it a
//! synthetic root and the shipped file can be checked on a machine that holds
//! none of its models. This is the other reading: the same text, plus the
//! root, so an estimate nobody declared is derived from the files and every
//! model file beside the catalog's own becomes an entry too.
//!
//! What an operator should hear about a reading -- an estimate declared below
//! what its files suggest, a figure that rests on size alone, a file that was
//! discovered -- comes back as notes rather than being printed here. The
//! catalog module decides nothing about output; the command that called it
//! does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::field::problem;
use super::read;
use super::{Catalog, Entry, Report, estimate};

/// Where an entry's memory estimate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateSource {
    /// Written in the catalog, by the entry or by the defaults table.
    Declared,
    /// Worked out from the entry's files, because nothing declared it.
    Derived,
}

/// A catalog read against a models root, and what there is to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub catalog: Catalog,
    /// What an operator should hear, one line each, in the order found.
    pub notes: Vec<String>,
    declared: usize,
    root: PathBuf,
}

impl Reading {
    /// How many entries the catalog text itself carries.
    #[must_use]
    pub fn declared(&self) -> usize {
        self.declared
    }

    /// How many entries were found under the root rather than in the text.
    #[must_use]
    pub fn discovered(&self) -> usize {
        self.catalog.entries.len() - self.declared
    }

    /// One line saying where the entries came from.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} entries from the catalog, {} discovered under {}",
            self.declared(),
            self.discovered(),
            self.root.display()
        )
    }
}

/// What settling every entry produces besides the entries.
#[derive(Default)]
struct Ledger {
    notes: Vec<String>,
    problems: Vec<String>,
    derived: BTreeSet<String>,
}

impl Catalog {
    /// Reads a catalog against a models root.
    ///
    /// Every entry without a declared estimate gets one derived from its
    /// files; an entry declaring one below what its files suggest keeps it
    /// and is named in the notes. Then every model file under the root that
    /// no entry names becomes an on-demand entry of its own.
    ///
    /// # Errors
    ///
    /// Returns a [`Report`] naming every problem, as [`Catalog::parse`] does,
    /// plus one for each entry whose estimate could neither be read nor
    /// derived because its model file is not there.
    pub fn read(text: &str, root: &Path) -> Result<Reading, Report> {
        let drafts = read::drafts(text)?;
        let mut ledger = Ledger {
            problems: drafts.problems,
            ..Ledger::default()
        };
        let mut entries: Vec<Entry> = drafts
            .entries
            .into_iter()
            .filter_map(|entry| {
                let declared = !drafts.undeclared.contains(&entry.id);
                settle(entry, declared, root, &mut ledger)
            })
            .collect();
        if !ledger.problems.is_empty() {
            return Err(Report {
                problems: ledger.problems,
            });
        }

        let declared = entries.len();
        entries.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Reading {
            catalog: Self {
                version: drafts.version.unwrap_or_default(),
                entries,
                derived: ledger.derived,
                discovered: BTreeSet::new(),
            },
            notes: ledger.notes,
            declared,
            root: root.to_path_buf(),
        })
    }
}

/// One entry with its estimate settled, or `None` once its problem is on
/// the ledger.
fn settle(mut entry: Entry, declared: bool, root: &Path, ledger: &mut Ledger) -> Option<Entry> {
    let scope = format!("entry '{}'", entry.id);
    let derived = estimate::derive(&entry, root);

    if declared {
        // The operator's figure stands: they may know something the files do
        // not, such as a model held on the processor. A figure below what the
        // files suggest is still worth a line, because it is the one that
        // admits a model the machine cannot hold.
        if let Ok(derived) = derived
            && derived.mib > entry.memory_estimate_mib
        {
            ledger.notes.push(format!(
                "{scope}: memory_estimate_mib is declared as {} MiB, but its files \
                 suggest {} MiB; keeping the declared value",
                entry.memory_estimate_mib, derived.mib
            ));
        }
        return Some(entry);
    }

    match derived {
        Ok(derived) => {
            entry.memory_estimate_mib = derived.mib;
            if derived.basis == estimate::Basis::Size {
                ledger.notes.push(format!(
                    "{scope}: memory estimate of {} MiB derived from file size alone, \
                     because the model file's metadata could not be read",
                    derived.mib
                ));
            }
            ledger.derived.insert(entry.id.clone());
            Some(entry)
        }
        Err(reason) => {
            ledger.problems.push(problem(
                &scope,
                "memory_estimate_mib",
                &format!("is required, because it could not be derived: {reason}"),
            ));
            None
        }
    }
}
