//! English-only gate.
//!
//! Pragmatic heuristic: French text virtually always carries accented
//! characters, so scanning for Latin diacritics catches a regression cheaply
//! without any language detection. It will not catch French written without
//! accents, which is the accepted limit of a cheap check.

use std::fs;

mod common;
use common::{has_extension, repo_root, sources};

/// Latin-1 accented letters (A-grave through y-umlaut, skipping the
/// multiplication and division signs) plus the OE ligatures. Built from code
/// points so this file itself stays accent-free and cannot fail its own test.
const RANGES: &[(u32, u32)] = &[
    (0x00C0, 0x00D6),
    (0x00D8, 0x00F6),
    (0x00F8, 0x00FF),
    (0x0152, 0x0153),
];

const SCANNED: &[&str] = &["rs", "md", "toml", "json", "yaml", "yml", "ts"];

fn is_accented(c: char) -> bool {
    RANGES
        .iter()
        .any(|(lo, hi)| (*lo..=*hi).contains(&(c as u32)))
}

#[test]
fn all_prose_is_english() {
    let root = repo_root();
    let files: Vec<_> = sources()
        .into_iter()
        .filter(|p| has_extension(p, SCANNED))
        .collect();
    assert!(!files.is_empty(), "nothing was scanned");

    let mut found = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.chars().any(is_accented) {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                found.push(format!("  {}:{}: {}", rel.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "Accented characters, which usually means French:\n\n{}\n",
        found.join("\n")
    );
}
