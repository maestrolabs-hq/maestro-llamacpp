//! Copy-paste detection, via `similarity-rs` (APTED tree edit distance, so it
//! compares structure rather than text and is not fooled by renaming).
//!
//! Structural similarity is not the same as duplication worth removing, so
//! this gate is an allowlist rather than a threshold: every accepted pair is a
//! written decision, and anything new fails. The list is empty because the one
//! duplication this repository would otherwise carry -- three gates walking
//! the same tree -- is factored into `tests/common/mod.rs` instead.

use std::collections::BTreeSet;
use std::process::Command;

const THRESHOLD: &str = "0.85";

/// Pairs we have looked at and chosen to keep, with the reason.
const ACCEPTED: &[(&str, &str)] = &[
    (
        "optional <-> required",
        "required is optional plus one guard, and delegates to it. Collapsing \
         them would mean one function taking a boolean saying whether the \
         field is mandatory, which is the shape both of these exist to avoid.",
    ),
    (
        "as_positive <-> as_residency",
        "Both convert one value and phrase one refusal, so they share a shape. \
         What they check does not overlap at all: integer bounds against enum \
         membership. A merged converter would take the check as a parameter \
         and be longer than both.",
    ),
    (
        "as_residency <-> as_location",
        "Both read text and hand it to something that owns the rule -- \
         Residency::parse and RelativePath::new respectively. The shared line \
         is the as_text call; the rule each defers to is the point, and it \
         lives elsewhere in both cases.",
    ),
    (
        "defaults <-> entries",
        "Both fetch a top-level table and build from it, which is where the \
         structural match comes from. They differ in what absence means: a \
         missing defaults table is legitimate and yields the empty set, a \
         missing models table is a problem the catalog is refused for.",
    ),
    (
        "an_entry_whose_child_never_becomes_ready_is_a_gateway_timeout \
         <-> an_entry_whose_child_cannot_start_is_a_bad_gateway",
        "Both assert one refusal: the status it carries, and that its message \
         names the entry. That shared shape is the contract -- every refusal \
         names what it was about -- rather than duplication to remove. \
         Merging them would mean one test taking a catalog and a status as \
         parameters, which reads as data and stops naming the two causes the \
         router must tell apart. Those two causes are exactly what \
         launch::Failure's variants exist to distinguish, so a merged test \
         would leave the distinction with no test of its own.",
    ),
    (
        "start <-> spawn",
        "start is spawn plus the readiness loop, and delegates to it -- the \
         same relationship as optional and required above. What they share is \
         the shape every fallible step in that module has: do one thing, and \
         name the entry when it fails. That is the module's error contract \
         rather than duplication to remove. Collapsing them would put the wait \
         inside the function that builds a process, and keeping those apart is \
         what makes either of them readable.",
    ),
];

/// `path:lines function name <-> path:lines function name` -> `name <-> name`.
/// Line numbers move whenever anything above them moves.
fn pair_name(line: &str) -> Option<String> {
    // `Classes: Entry <-> Entry` names the type two methods share, not a pair.
    if line.trim_start().starts_with("Classes:") {
        return None;
    }
    let (left, right) = line.split_once(" <-> ")?;
    let name = |s: &str| s.split_whitespace().last().map(str::to_owned);
    Some(format!("{} <-> {}", name(left)?, name(right)?))
}

fn detected() -> BTreeSet<String> {
    let out = Command::new("similarity-rs")
        .args(["--threshold", THRESHOLD, "src", "tests"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect(
            "similarity-rs must be installed: cargo binstall similarity-rs. \
             A gate that skips when its tool is missing reports green while \
             looking at nothing.",
        );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(pair_name)
        .collect()
}

#[test]
fn no_duplication_is_unaccounted_for() {
    let found = detected();
    let accepted: BTreeSet<&str> = ACCEPTED.iter().map(|(p, _)| *p).collect();

    let unexplained: Vec<&String> = found
        .iter()
        .filter(|p| !accepted.contains(p.as_str()))
        .collect();

    assert!(
        unexplained.is_empty(),
        "Duplication with no recorded decision:\n\n{}\n\n\
         Either factor out what is shared, or add the pair to ACCEPTED in this \
         file with the reason it should stay.\n",
        unexplained
            .iter()
            .map(|p| format!("  {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// An allowlist nobody prunes becomes a list of excuses for code that no longer
/// exists, and the next real duplicate hides among them.
#[test]
fn no_accepted_pair_has_gone_stale() {
    let found = detected();
    let stale: Vec<&str> = ACCEPTED
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| !found.contains(*p))
        .collect();

    assert!(
        stale.is_empty(),
        "ACCEPTED lists pairs that are no longer duplicated:\n\n{}\n\n\
         Remove them.\n",
        stale
            .iter()
            .map(|p| format!("  {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
