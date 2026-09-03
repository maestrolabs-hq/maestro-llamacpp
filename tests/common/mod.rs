//! Shared by the gates. Kept here so they do not each carry their own copy of
//! the walk -- which the duplication gate would rightly flag.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root. One crate at the top level, so the manifest directory
/// is the root, and the rules apply to every file below it.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every file in the repository, minus build output and tool state. Callers
/// filter by extension: one walk serves the prose, link and size gates.
pub fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let skip = [
            "target",
            ".git",
            "node_modules",
            ".worktrees",
            ".superpowers",
        ];
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip.contains(&name.as_str()) {
                continue;
            }
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    let mut out = Vec::new();
    walk(&repo_root(), &mut out);
    out
}

/// True when the path carries one of the given extensions.
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e))
}
