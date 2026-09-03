//! The path type the catalog is built from.
//!
//! Every location in a catalog is relative, and resolution against a models
//! root is a separate step the caller asks for. That split is what keeps a
//! parsed catalog inert: it describes where files sit inside a root without
//! deciding which root, so the same catalog is correct on every machine.

use std::path::{Path, PathBuf};

/// A location inside the models root.
///
/// # Invariant
///
/// A value of this type is always relative. The constructor refuses anything
/// anchored to one machine -- a Unix root, a Windows drive letter, or a UNC
/// share -- so a catalog cannot carry such a path even if someone writes one
/// into the file. This is the run-time half of the estate's path rule; the
/// shared gate scans tracked files, and this refuses the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativePath(String);

impl RelativePath {
    /// Accepts a relative location, refusing anything machine-anchored.
    ///
    /// # Errors
    ///
    /// Returns the reason the value was refused, phrased to be embedded in a
    /// message that has already named the entry and the field.
    pub fn new(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err("must not be empty".to_owned());
        }
        if let Some(anchor) = anchor_of(value) {
            return Err(format!(
                "must be relative to the models root, but '{value}' is anchored to {anchor}"
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// The location as written in the catalog.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Where this location sits under a given models root.
    #[must_use]
    pub fn resolve(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }
}

/// What a value is anchored to, when it is anchored to anything.
///
/// Windows separators are checked on every platform on purpose: a catalog
/// written on one machine is read on another, so a drive letter has to be
/// refused by the Linux build too.
fn anchor_of(value: &str) -> Option<&'static str> {
    let bytes = value.as_bytes();
    if value.starts_with("\\\\") {
        return Some("a network share");
    }
    if value.starts_with('/') || value.starts_with('\\') {
        return Some("a filesystem root");
    }
    match (bytes.first(), bytes.get(1)) {
        (Some(letter), Some(b':')) if letter.is_ascii_alphabetic() => Some("a drive letter"),
        _ => None,
    }
}
