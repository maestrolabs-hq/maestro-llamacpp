//! Link gate.
//!
//! The specification, the plan and the README cross-reference each other, and
//! a renamed file is drift no other gate sees: prose passes, the build passes,
//! and the link quietly points at nothing. This resolves every
//! repository-relative Markdown link and asserts its target exists.

use std::fs;
use std::path::Path;

mod common;
use common::{has_extension, repo_root, sources};

/// Link targets this gate does not own. External addresses are the network's
/// problem, and a bare fragment addresses the current document.
fn is_external(target: &str) -> bool {
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with('#')
}

/// Inline Markdown link targets: the `dest` of every `[text](dest)`.
///
/// Hand-written because the repository has no dependencies. It reads the
/// destination up to the first unnested `)`, then keeps the part before any
/// whitespace, which drops the optional `"title"` that may follow.
fn link_targets(text: &str) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut targets = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != ']' || bytes[i + 1] != '(' {
            i += 1;
            continue;
        }
        let mut depth = 1;
        let mut dest = String::new();
        let mut j = i + 2;
        while j < bytes.len() && depth > 0 {
            match bytes[j] {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                dest.push(bytes[j]);
            }
            j += 1;
        }
        if let Some(first) = dest.split_whitespace().next() {
            targets.push(first.to_owned());
        }
        i = j;
    }
    targets
}

/// The path a link resolves to, or `None` when this gate does not own it.
fn resolved(document: &Path, target: &str) -> Option<std::path::PathBuf> {
    if is_external(target) {
        return None;
    }
    let path = target.split('#').next().unwrap_or(target);
    if path.is_empty() {
        return None;
    }
    let base = document.parent()?;
    Some(base.join(path))
}

#[test]
fn no_document_links_to_a_missing_file() {
    let root = repo_root();
    let documents: Vec<_> = sources()
        .into_iter()
        .filter(|p| has_extension(p, &["md"]))
        .collect();
    assert!(!documents.is_empty(), "no document was scanned");

    let mut broken = Vec::new();
    for document in &documents {
        let Ok(text) = fs::read_to_string(document) else {
            continue;
        };
        for target in link_targets(&text) {
            let Some(path) = resolved(document, &target) else {
                continue;
            };
            if !path.exists() {
                let rel = document.strip_prefix(&root).unwrap_or(document);
                broken.push(format!("  {}: {target}", rel.display()));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "Links with no file behind them:\n\n{}\n",
        broken.join("\n")
    );
}
