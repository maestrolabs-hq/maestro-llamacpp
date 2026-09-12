//! Copy-paste detection, via `similarity-rs` (APTED tree edit distance, so it
//! compares structure rather than text and is not fooled by renaming).
//!
//! Structural similarity is not the same as duplication worth removing, so
//! this gate is an allowlist rather than a threshold: every accepted pair is a
//! written decision, and anything new fails. A pair earns its place by being
//! cheaper to keep than to merge, and the reason has to say why -- the one
//! duplication that did not earn it, three gates walking the same tree, is
//! factored into `tests/common/mod.rs` instead of listed here.

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
        "a_resident_entry_is_never_unloaded <-> a_busy_entry_is_never_unloaded",
        "Two protection rules, one shape: a protected entry is held beside an \
         idle one, and the idle one goes. The shared arrangement was factored \
         into with_one_protected, which is what each test now calls; what \
         remains is a single assertion whose structure cannot differ, because \
         the rules differ only in which field makes an entry protected. \
         Merging them would take that field as a parameter and stop naming \
         residency and busyness as the two separate reasons a model is never \
         a candidate -- and those two reasons are the whole of the policy \
         this module exists to keep.",
    ),
    (
        "loaded <-> resident_failures",
        "Two accessors on Router, each reading one piece of state under \
         whatever lock owns it and handing back an owned copy. That shape is \
         the whole of both, which is why they match, and it is the accessor \
         contract rather than duplication to remove. What they read has \
         nothing in common: one walks every slot in the catalog, the other \
         clones a vector the startup loader appended to. Merging them would \
         mean one method taking a parameter saying which of two unrelated \
         things to read, and neither name would survive it.",
    ),
    (
        "a_resident_older_than_the_window_is_not_named_beside_an_on_demand_one \
         <-> a_busy_entry_older_than_the_window_is_not_named_beside_an_idle_one",
        "The same shape as admission.rs's own accepted pair below, for the same \
         reason and the same policy read through idle.rs instead: both call \
         with_one_protected and assert the identical result, because the two \
         rules -- residency and busyness -- differ only in which field \
         protects an entry. Merging them would stop naming the two reasons a \
         model is never a candidate for idle unloading.",
    ),
    (
        "an_on_demand_entry_older_than_the_window_is_named_beside_a_younger_one \
         <-> a_busy_entry_older_than_the_window_is_not_named_beside_an_idle_one",
        "Both build a two-entry array and assert_eq the expired set, which is \
         the whole shape any case in this file can take -- there is no less \
         to write for either without a fixture that hides which two entries \
         are compared and why. What differs is the property under test: one \
         is the basic age comparison, the other a protection rule.",
    ),
    (
        "an_on_demand_entry_older_than_the_window_is_named_beside_a_younger_one \
         <-> a_resident_older_than_the_window_is_not_named_beside_an_on_demand_one",
        "The same as the pair above, with the other protection rule.",
    ),
    (
        "attempt <-> spawn",
        "attempt is spawn plus the readiness loop, and delegates to it -- the \
         same relationship as optional and required above. What they share is \
         the shape every fallible step in that module has: do one thing, and \
         name the entry when it fails. That is the module's error contract \
         rather than duplication to remove. Collapsing them would put the wait \
         inside the function that builds a process, and keeping those apart is \
         what makes either of them readable.",
    ),
    (
        "windowed <-> probed",
        "Both hand a budget and a window to the shared launch, differing only \
         in which of the two they let the caller state. A merged function \
         would take both, and every idle-unload test would have to say it \
         reads no machine and every probe test that it has no window.",
    ),
    (
        "system_total_mib <-> resident_mib",
        "Two questions the platform answers with different tools, and each \
         function is the table of which tool answers on which platform. The \
         shape is the table; merging them would be one table keyed by \
         question and platform, longer than both and answering two \
         unrelated things -- what the machine holds, and what one process \
         holds.",
    ),
    (
        "u32_at <-> u64_at",
        "Two one-line readers over bytes_at, differing only in the integer \
         type the bytes spell. The shared shape is the whole function. \
         Merging them would need a trait over from_le_bytes that the \
         standard library does not offer, and inlining them would spell the \
         width at every call site where the name says it once.",
    ),
    (
        "as_residency <-> as_runtime",
        "The same pair as `as_positive <-> as_residency`, for the same reason: \
         both read text and phrase one refusal, so they share a shape. What \
         they check does not overlap -- enum membership against which \
         characters may name a binary -- and a merged converter would take \
         the check as a parameter and be longer than both.",
    ),
    (
        "as_text <-> as_runtime",
        "`as_runtime` is `as_text` plus one guard, and calls it. Collapsing \
         them means one function taking a predicate, which is what having two \
         named converters exists to avoid.",
    ),
    (
        "an_entry_naming_a_runtime_is_served_from_that_build \
         <-> an_entry_naming_a_runtime_that_is_not_there_says_so_rather_than_falling_back",
        "Both build one catalog, make one request and assert one status, \
         because that shared shape is the contract. The causes differ and are \
         the point: a runtime that is present must be used, and one that is \
         absent must fail loudly rather than fall back to a server that \
         cannot load the entry. Merging them would take the status as a \
         parameter and stop naming either cause.",
    ),
    (
        "entry <-> line",
        "Two fixtures in `invocation.rs` matched on shape and nothing else: \
         one builds an `Entry` literal, the other maps `of`'s output to \
         strings. They share no line and no idea. The match arrived with the \
         `runtime` field, which is the gate comparing structure rather than \
         meaning, and recording it is cheaper than shaping a fixture around a \
         similarity score.",
    ),
    (
        "budgeted <-> queued",
        "The delegation family again, with the way that states a wait. Each \
         of these names one setting and hands the rest to the shared launch, \
         so the shape they share is the handing-on itself. Merging them is \
         one constructor taking every setting, which is exactly what the \
         tests calling `serving` are spared from naming.",
    ),
    (
        "windowed <-> queued",
        "Both state one setting over a budget -- an idle window, or how long \
         a request waits for room -- and are otherwise the same delegation. \
         A merged function would take both, and every idle-unload test would \
         have to say it does not wait while every queueing test said it has \
         no window.",
    ),
    (
        "queued <-> probed",
        "The same delegation with the way that states a machine. What each \
         names does not overlap at all: how long to wait for room, against \
         what the device and every child report. They are alike only in \
         having one parameter and passing the rest on.",
    ),
    (
        "listing <-> json",
        "`listing` is `json` with the catalog read into a value first, and it \
         calls it -- so what the gate has matched is delegation seen from the \
         outside, both ending in the same sentence: build a value, hand it to \
         the framing. Collapsing them leaves the one caller that already \
         holds a value passing it to a function with nothing left to do. The \
         pair replaced `list <-> catalogue` and `list <-> properties`, which \
         went stale when the second writer in `answer::own` was dropped for \
         this one.",
    ),
    (
        "allowed <-> suffix",
        "Two questions asked of the same three endpoint shapes, each answered \
         per shape with a match: which methods an endpoint accepts, and what \
         the child is asked for. The shared structure is the enum they both \
         read. Merging them would be one method taking which question to \
         answer as a parameter, and the two answers have nothing in common \
         but the variants they are keyed on.",
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
