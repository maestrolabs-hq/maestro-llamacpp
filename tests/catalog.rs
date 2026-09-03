//! Schema gate for the model catalog.
//!
//! The catalog is the only input slice 1 has, so its shape is the whole
//! contract: what a model entry must carry, what it may omit, and what it
//! inherits. These tests fix that contract before a parser exists.
//!
//! Two rules earn their own cases. Every validation error names both the
//! entry it came from and the field that caused it, because an error that
//! says only "invalid catalog" sends the reader back to the file to guess.
//! And a path anchored to one machine cannot be constructed at all -- the
//! shared gate scans tracked files, and this proves the type refuses one at
//! run time too.

use maestro_llamacpp::catalog::{Catalog, RelativePath, Residency};
use std::path::Path;

/// The golden catalog, parsed. A fixture rather than an inline string: it is
/// the same shape the shipped catalog uses, and a file can be read by eye.
fn golden() -> Catalog {
    let text = include_str!("fixtures/catalog.toml");
    Catalog::parse(text).expect("the golden fixture must parse")
}

#[test]
fn the_golden_catalog_parses_field_by_field() {
    let catalog = golden();
    assert_eq!(catalog.version, 1);
    assert_eq!(catalog.entries.len(), 4, "four entries, one per model");

    let qwen = catalog.entry("qwen38").expect("qwen38");
    assert_eq!(
        qwen.path.as_str(),
        "llm/qwen/qwen3.8-27b/Qwen3.8-27B-UD-Q6_K.gguf"
    );
    assert_eq!(
        qwen.draft_path.as_ref().map(RelativePath::as_str),
        Some("llm/qwen/qwen3.8-27b/MTP/mtp-Qwen3.8-27B-Q4_0.gguf"),
        "the speculative draft model"
    );
    assert_eq!(
        qwen.projector_path.as_ref().map(RelativePath::as_str),
        Some("llm/qwen/qwen3.8-27b/mmproj-F16.gguf"),
        "the multimodal projector"
    );
    assert_eq!(qwen.context_size, 131_072);
    assert_eq!(qwen.memory_estimate_mib, 24_576);
    assert_eq!(qwen.reasoning_format.as_deref(), Some("deepseek"));
    assert_eq!(
        qwen.reasoning_effort, None,
        "only the semantic entry sets it"
    );
}

#[test]
fn an_entry_inherits_every_default_it_does_not_set() {
    let catalog = golden();

    let gemma = catalog.entry("gemma3").expect("gemma3");
    assert_eq!(gemma.context_size, 32_768, "inherited from defaults");
    assert_eq!(gemma.residency, Residency::OnDemand, "inherited");
    assert_eq!(gemma.memory_estimate_mib, 2_048, "set by the entry");
    assert_eq!(gemma.draft_path, None, "no draft model");
    assert_eq!(gemma.projector_path, None, "no projector");

    let qwen = catalog.entry("qwen38").expect("qwen38");
    assert_eq!(
        qwen.context_size, 131_072,
        "the entry overrides the default"
    );

    assert_eq!(
        qwen.flags.get("jinja").map(String::as_str),
        Some("true"),
        "flags merge rather than replace"
    );
    assert_eq!(
        qwen.flags.get("ctk").map(String::as_str),
        Some("q8_0"),
        "the entry's own flags survive the merge"
    );
}

#[test]
fn residency_is_parsed_and_only_one_entry_is_resident() {
    let catalog = golden();
    assert_eq!(
        catalog.entry("qwen3-06b").expect("qwen3-06b").residency,
        Residency::Resident,
        "the entry the steward depends on"
    );
    assert_eq!(
        catalog
            .entry("qwen38-semantic")
            .expect("qwen38-semantic")
            .reasoning_effort
            .as_deref(),
        Some("low"),
    );
}

/// One case per validation rule. Table-driven so the six read as one list of
/// rules rather than six near-identical functions.
const INVALID: &[(&str, &str, &str, &str)] = &[
    (
        "a required field is missing",
        "version = 1\n[models.alpha]\ncontext_size = 4096\nmemory_estimate_mib = 512\n",
        "alpha",
        "path",
    ),
    (
        "a field nobody recognises",
        "version = 1\n[models.beta]\npath = \"a.gguf\"\ncontext_size = 4096\nmemory_estimate_mib = 512\ncolour = \"red\"\n",
        "beta",
        "colour",
    ),
    (
        "a context size of zero",
        "version = 1\n[models.gamma]\npath = \"a.gguf\"\ncontext_size = 0\nmemory_estimate_mib = 512\n",
        "gamma",
        "context_size",
    ),
    (
        "a residency nobody recognises",
        "version = 1\n[models.delta]\npath = \"a.gguf\"\ncontext_size = 4096\nmemory_estimate_mib = 512\nresidency = \"sometimes\"\n",
        "delta",
        "residency",
    ),
    (
        "a memory estimate of zero",
        "version = 1\n[models.epsilon]\npath = \"a.gguf\"\ncontext_size = 4096\nmemory_estimate_mib = 0\n",
        "epsilon",
        "memory_estimate_mib",
    ),
    (
        "a path anchored to a machine",
        "version = 1\n[models.zeta]\npath = \"/somewhere/a.gguf\"\ncontext_size = 4096\nmemory_estimate_mib = 512\n",
        "zeta",
        "path",
    ),
];

#[test]
fn every_validation_error_names_its_entry_and_its_field() {
    for (case, text, entry, field) in INVALID {
        let report = Catalog::parse(text)
            .err()
            .unwrap_or_else(|| panic!("{case}: the catalog must be refused"))
            .to_string();
        assert!(
            report.contains(entry),
            "{case}: the error must name the entry '{entry}':\n{report}"
        );
        assert!(
            report.contains(field),
            "{case}: the error must name the field '{field}':\n{report}"
        );
    }
}

#[test]
fn every_error_is_reported_not_only_the_first() {
    let text = "version = 1\n\
                [models.alpha]\ncontext_size = 0\nmemory_estimate_mib = 512\n\
                [models.beta]\npath = \"b.gguf\"\ncontext_size = 4096\nmemory_estimate_mib = 0\n";
    let report = Catalog::parse(text)
        .expect_err("both entries are invalid")
        .to_string();
    assert!(report.contains("alpha"), "the first entry:\n{report}");
    assert!(report.contains("beta"), "the second entry too:\n{report}");
}

/// One bad entry can be wrong in several ways at once, and a reader fixing it
/// should see all of them before running the tool again.
#[test]
fn one_entry_reports_all_of_its_own_faults() {
    let text = "version = 1\n\
                [models.alpha]\ncontext_size = 0\ncolour = \"red\"\n";
    let report = Catalog::parse(text)
        .expect_err("the entry is invalid three times over")
        .to_string();
    for expected in ["path", "context_size", "memory_estimate_mib", "colour"] {
        assert!(
            report.contains(expected),
            "'{expected}' must be reported alongside the others:\n{report}"
        );
    }
}

/// The file that ships cannot rot away from the parser that reads it.
#[test]
fn the_shipped_catalog_is_valid() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/catalog.toml");
    let text = std::fs::read_to_string(shipped).expect("catalog.toml ships with this repository");
    let catalog = Catalog::parse(&text).unwrap_or_else(|report| {
        panic!("the shipped catalog must be valid:\n{report}");
    });
    assert!(
        catalog
            .entries
            .iter()
            .any(|entry| entry.residency == Residency::Resident),
        "one entry is held loaded, or the steward has nothing to talk to"
    );
}

#[test]
fn a_machine_anchored_path_cannot_be_represented() {
    for anchored in [
        "/somewhere/models/a.gguf",
        "C:\\models\\a.gguf",
        "\\\\server\\share\\a.gguf",
    ] {
        assert!(
            RelativePath::new(anchored).is_err(),
            "the constructor must refuse '{anchored}'"
        );
    }
}

#[test]
fn a_relative_path_resolves_against_a_models_root() {
    let path = RelativePath::new("llm/qwen/a.gguf").expect("a relative path is accepted");
    assert_eq!(path.as_str(), "llm/qwen/a.gguf");
    assert_eq!(
        path.resolve(Path::new("/somewhere/models")),
        Path::new("/somewhere/models").join("llm/qwen/a.gguf"),
        "resolution is the caller's decision, not the catalog's"
    );
}
