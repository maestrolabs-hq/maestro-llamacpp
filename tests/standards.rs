//! Size limits. A test, not a promise.

use std::fs;
use std::path::Path;

mod common;
use common::{has_extension, repo_root, sources};

/// Python's governance bar uses 200. Rust needs more room for the same
/// content -- closing braces, explicit types, match arms, derive attributes --
/// so this sits a quarter higher.
///
/// It is a dumping-ground tripwire, not a design rule. The design rules are
/// per-function and live in `Cargo.toml`: `too_many_lines`,
/// `cognitive_complexity`, `too_many_arguments`. A module of twenty small
/// clear functions is fine; one of three sprawling ones is not, and this
/// limit would not notice the difference. Those lints would.
const MAX_MODULE_LINES: usize = 250;

/// Lines before the first test module. Rust colocates unit tests with the code
/// they cover; counting them would charge a module for being tested.
///
/// Any cfg naming `test` counts, not just the bare `#[cfg(test)]`. Matching the
/// literal string would charge a module for a `#[cfg(all(test, unix))]` block --
/// the gate doing the exact thing this comment says it must not.
fn code_lines(path: &Path) -> usize {
    let text = fs::read_to_string(path).expect("read");
    text.lines()
        .position(|l| {
            let l = l.trim_start();
            l.starts_with("#[cfg(")
                && l.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|tok| tok == "test")
        })
        .unwrap_or_else(|| text.lines().count())
}

#[test]
fn no_module_becomes_a_dumping_ground() {
    let src = repo_root().join("src");
    let files: Vec<_> = sources()
        .into_iter()
        .filter(|p| p.starts_with(&src) && has_extension(p, &["rs"]))
        .collect();
    assert!(!files.is_empty(), "no module found under {}", src.display());

    let over: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let n = code_lines(f);
            (n > MAX_MODULE_LINES)
                .then(|| format!("  {}: {n} lines (max {MAX_MODULE_LINES})", f.display()))
        })
        .collect();
    assert!(over.is_empty(), "Module too large:\n{}\n", over.join("\n"));
}
