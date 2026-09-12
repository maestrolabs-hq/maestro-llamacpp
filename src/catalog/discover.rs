//! Every model file under the root that the catalog does not name.
//!
//! The catalog names the models an operator has thought about; the root holds
//! every model they have downloaded. Until this module the difference was a
//! file that could not be invoked without an edit and a restart, and the
//! decision to close that gap is recorded in
//! `docs/adr/0002-discover-models-beside-the-catalog.md`.
//!
//! The rules, all of them about file names, because that is what a file on
//! disk offers before it is opened:
//!
//! - a file is a model when its extension is `gguf`, in any case;
//! - a file the catalog already names, as weights, draft or projector, is the
//!   catalog's and not found again;
//! - a projector, named `mmproj` by every tool that writes one, is not a
//!   model on its own;
//! - a draft for speculative decoding is not either. Nothing inside such a
//!   file says so -- it calls itself a model -- so the name has to, and the
//!   convention is an `mtp` segment such as `mtp-` or `FastMTP`;
//! - of a model split into shards, `-00001-of-0000N` names the entry and the
//!   rest are its weights;
//! - every directory is walked, `.cache` included: a download cache is where
//!   the tool that fetched a model put it, and the shipped catalog already
//!   points into one.
//!
//! An entry is on-demand, takes the defaults table, and gets a derived
//! estimate. Its identifier is the file's stem, lowercased, with every run of
//! anything but letters and digits collapsed to one hyphen. When that is a
//! name the catalog, the proxy or an earlier file already has, the parent
//! directory's name is put in front, and the note says so.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::gguf::Metadata;

use super::estimate::{self, Basis};
use super::read::{DEFAULT_STARTUP_TIMEOUT_SECONDS, Defaults};
use super::{Entry, RelativePath, Residency};

mod name;

use name::{identifier, is_model};

/// How deep the walk goes. What bounds a link that points at an ancestor.
const MAX_DEPTH: usize = 16;

/// The context a discovered entry gets when neither the defaults table nor
/// the file says: the server's own default.
const FALLBACK_CONTEXT: u32 = 4096;

/// Every model file under `root` that `entries` do not name, as entries.
pub(super) fn under(
    root: &Path,
    entries: &[Entry],
    defaults: &Defaults,
    notes: &mut Vec<String>,
) -> Vec<Entry> {
    let referenced: BTreeSet<PathBuf> = entries
        .iter()
        .flat_map(|entry| {
            [
                Some(&entry.path),
                entry.draft_path.as_ref(),
                entry.projector_path.as_ref(),
            ]
        })
        .flatten()
        .map(|location| location.resolve(root))
        .collect();
    let mut taken: BTreeSet<String> = entries.iter().map(|entry| entry.id.clone()).collect();
    taken.extend(name::RESERVED.iter().map(|id| (*id).to_owned()));

    let mut found = Vec::new();
    let mut files = Vec::new();
    walk(root, 0, &mut files);
    for file in files {
        if referenced.contains(&file) {
            continue;
        }
        let Some(location) = relative(root, &file) else {
            notes.push(format!(
                "skipped '{}': its path is not text this catalog can carry",
                file.display()
            ));
            continue;
        };
        let Some(id) = identifier(&file, &mut taken, notes) else {
            continue;
        };
        if let Some(entry) = entry(id, location, root, defaults, notes) {
            found.push(entry);
        }
    }
    found
}

/// Every model file below `directory`, in a stable order.
fn walk(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(listing) = fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = listing.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_model)
        {
            out.push(path);
        }
    }
}

/// The location as a catalog would write it: relative to the root, with
/// the separator every catalog uses.
fn relative(root: &Path, file: &Path) -> Option<RelativePath> {
    let parts: Option<Vec<&str>> = file
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect();
    RelativePath::new(&parts?.join("/")).ok()
}

/// The entry one file becomes, or `None` once the note says why not.
fn entry(
    id: String,
    location: RelativePath,
    root: &Path,
    defaults: &Defaults,
    notes: &mut Vec<String>,
) -> Option<Entry> {
    let trained_for = Metadata::read(&location.resolve(root))
        .ok()
        .and_then(|metadata| metadata.of_model("context_length"))
        .and_then(|context| u32::try_from(context).ok())
        .filter(|context| *context > 0);
    let context_size = defaults
        .context_size
        .or(trained_for)
        .unwrap_or(FALLBACK_CONTEXT)
        .min(trained_for.unwrap_or(u32::MAX));

    let mut entry = Entry {
        id,
        path: location,
        draft_path: None,
        projector_path: None,
        context_size,
        residency: Residency::OnDemand,
        memory_estimate_mib: 0,
        reasoning_format: defaults.reasoning_format.clone(),
        reasoning_effort: defaults.reasoning_effort.clone(),
        startup_timeout_seconds: defaults
            .startup_timeout_seconds
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECONDS),
        // A discovered entry takes the stock server. Which build a model needs
        // is not a thing a file name can say, so it is not a thing discovery
        // may guess at -- a catalog states it or it is not wanted.
        runtime: None,
        flags: defaults.flags.clone(),
    };
    match estimate::derive(&entry, root) {
        Ok(derived) => {
            entry.memory_estimate_mib = derived.mib;
            let basis = match derived.basis {
                Basis::Metadata => "",
                Basis::Size => ", from file size alone",
            };
            notes.push(format!(
                "discovered '{}' at {}: {} MiB estimated at {} tokens of context{basis}",
                entry.id,
                entry.path.as_str(),
                derived.mib,
                entry.context_size
            ));
            Some(entry)
        }
        Err(reason) => {
            notes.push(format!("skipped '{}': {reason}", entry.path.as_str()));
            None
        }
    }
}
