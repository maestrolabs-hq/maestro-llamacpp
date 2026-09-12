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
fn the_resident_reservation_is_what_the_resident_entries_cost() {
    assert_eq!(
        golden().resident_reservation_mib(),
        1024,
        "one resident at 1024 MiB. An on-demand entry costs nothing here \
         however large it is, because a reservation is what stays held \
         whatever else the router does"
    );
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

    // Startup time varies by two orders of magnitude, so the budget is a per
    // entry field: the small model carries a tight one of its own, the large
    // one inherits the generous default.
    assert_eq!(
        gemma.startup_timeout_seconds, 60,
        "a small model is ready quickly, and the catalog says so"
    );
    assert_eq!(
        qwen.startup_timeout_seconds, 300,
        "inherited from the defaults table"
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

mod fixtures;
use fixtures::{Gguf, Scratch, Value};
use maestro_llamacpp::catalog::EstimateSource;

const MIB: u64 = 1024 * 1024;

/// A model whose estimate can be worked out by hand: four layers, two
/// key-value heads, a head width of 64 (256 embedding over 4 heads, with no
/// key length stated), and 64 MiB of weights.
///
/// At 1024 tokens of f16 cache that is 4 x 1024 x 2 x (64 + 64) x 2 bytes,
/// which is 2 MiB, on top of 64 MiB of weights, 5 percent of those for
/// fragmentation, and 1024 MiB of fixed overhead: 1093.2 MiB, rounded up.
/// A model of many layers, optionally declaring how often one of them is a
/// full-attention layer.
///
/// Sixty-four layers at the one-thousand-and-twenty-four-token context of
/// `one_entry` is 32 MiB of f16 cache when every layer keeps one, which is
/// large enough that a quarter of it cannot be mistaken for rounding.
fn layered(full_attention_interval: Option<u32>) -> Gguf {
    let model = Gguf::model("qwen35", 64, 8192, 256)
        .with("qwen35.attention.head_count", Value::U32(4))
        .with("qwen35.attention.head_count_kv", Value::U32(2));
    match full_attention_interval {
        Some(interval) => model.with("qwen35.full_attention_interval", Value::U32(interval)),
        None => model,
    }
}

/// What one layered model's entry is estimated at.
fn estimated(label: &str, model: &Gguf) -> u32 {
    let scratch = Scratch::new(label);
    model.write(&scratch.path().join("a/model.gguf"), 64 * MIB);
    Catalog::read(&one_entry(""), scratch.path())
        .expect("derivable")
        .catalog
        .entry("alpha")
        .expect("alpha")
        .memory_estimate_mib
}

#[test]
fn a_hybrid_model_caches_only_its_full_attention_layers() {
    // A hybrid keeps a key-value cache on one layer in every
    // `full_attention_interval`; the rest carry a recurrent state whose size
    // does not grow with the context. Counting a cache for all of them is the
    // difference between an estimate and a refusal: measured on this estate,
    // Qwen3.8 27B declares sixty-five layers and an interval of four, and
    // charging all sixty-five put its estimate 14 GiB above what it was then
    // measured to hold.
    let dense = estimated("catalog-dense", &layered(None));
    let hybrid = estimated("catalog-hybrid", &layered(Some(4)));

    assert_eq!(
        dense - hybrid,
        24,
        "one layer in four keeps a cache, so three quarters of the 32 MiB \
         goes: dense {dense} MiB, hybrid {hybrid} MiB"
    );
}

fn small_model() -> Gguf {
    Gguf::model("tiny", 4, 8192, 256)
        .with("tiny.attention.head_count", Value::U32(4))
        .with("tiny.attention.head_count_kv", Value::U32(2))
}

const SMALL_MODEL_MIB: u32 = 1094;

/// A catalog with one entry whose estimate is whatever the test says.
fn one_entry(estimate: &str) -> String {
    format!(
        "version = 1\n\
         [models.alpha]\n\
         path = \"a/model.gguf\"\n\
         context_size = 1024\n\
         {estimate}\n"
    )
}

#[test]
fn an_absent_estimate_is_derived_from_the_files() {
    let scratch = Scratch::new("catalog-derive");
    small_model().write(&scratch.path().join("a/model.gguf"), 64 * MIB);

    let reading = Catalog::read(&one_entry(""), scratch.path()).expect("derivable");

    let alpha = reading.catalog.entry("alpha").expect("alpha");
    assert_eq!(
        alpha.memory_estimate_mib, SMALL_MODEL_MIB,
        "weights, cache for the configured context, fragmentation and \
         overhead, rounded up to the next mebibyte"
    );
    assert_eq!(
        reading.catalog.estimate_source("alpha"),
        Some(EstimateSource::Derived),
        "and the catalog remembers that nobody declared it"
    );
    assert!(
        reading.notes.iter().all(|note| !note.contains("declared")),
        "nothing to warn about when nothing was declared:\n{:?}",
        reading.notes
    );
}

#[test]
fn a_declared_estimate_below_what_the_files_suggest_is_kept_and_named() {
    let scratch = Scratch::new("catalog-under");
    small_model().write(&scratch.path().join("a/model.gguf"), 64 * MIB);

    let reading = Catalog::read(&one_entry("memory_estimate_mib = 512"), scratch.path())
        .expect("a declared estimate is never a problem");

    let alpha = reading.catalog.entry("alpha").expect("alpha");
    assert_eq!(
        alpha.memory_estimate_mib, 512,
        "the operator's figure stands: they may know something the files do not"
    );
    assert_eq!(
        reading.catalog.estimate_source("alpha"),
        Some(EstimateSource::Declared)
    );
    let warning = reading
        .notes
        .iter()
        .find(|note| note.contains("alpha"))
        .unwrap_or_else(|| panic!("one note names the entry: {:?}", reading.notes));
    assert!(
        warning.contains("512") && warning.contains(&SMALL_MODEL_MIB.to_string()),
        "both figures, so the operator can see how far apart they are: {warning}"
    );
}

#[test]
fn a_declared_estimate_at_or_above_the_derived_one_earns_no_note() {
    let scratch = Scratch::new("catalog-over");
    small_model().write(&scratch.path().join("a/model.gguf"), 64 * MIB);

    let reading = Catalog::read(&one_entry("memory_estimate_mib = 2048"), scratch.path())
        .expect("a declared estimate is never a problem");

    assert!(
        reading.notes.iter().all(|note| !note.contains("alpha")),
        "an estimate that errs on the safe side is not worth a line:\n{:?}",
        reading.notes
    );
}

#[test]
fn an_absent_estimate_with_no_file_to_derive_it_from_is_a_problem() {
    let scratch = Scratch::new("catalog-nofile");

    let report = Catalog::read(&one_entry(""), scratch.path())
        .expect_err("nothing to measure and nothing declared")
        .to_string();

    assert!(
        report.contains("alpha") && report.contains("memory_estimate_mib"),
        "the entry and the field, like every other problem:\n{report}"
    );
    assert!(
        report.contains("model.gguf"),
        "and the file it would have measured:\n{report}"
    );
}

#[test]
fn the_cache_type_flags_shrink_the_derived_estimate() {
    let scratch = Scratch::new("catalog-cache-type");
    small_model().write(&scratch.path().join("a/model.gguf"), 64 * MIB);

    let reading = Catalog::read(
        &one_entry("[models.alpha.flags]\nctk = \"q8_0\"\nctv = \"q8_0\""),
        scratch.path(),
    )
    .expect("derivable");

    // An eight-bit cache holds 17 bytes per 16 elements: the 2 MiB of f16
    // cache becomes 1.0625 MiB, and the total rounds to one less.
    assert_eq!(
        reading
            .catalog
            .entry("alpha")
            .expect("alpha")
            .memory_estimate_mib,
        SMALL_MODEL_MIB - 1
    );
}

#[test]
fn a_draft_and_a_projector_are_counted_with_the_weights() {
    let scratch = Scratch::new("catalog-draft");
    small_model().write(&scratch.path().join("a/model.gguf"), 64 * MIB);
    // Two layers, one head of width 64: 2 x 1024 x 1 x 128 x 2 bytes, 512 KiB
    // of cache at the same context, on top of 16 MiB of weights.
    Gguf::model("tiny", 2, 8192, 128)
        .with("tiny.attention.head_count", Value::U32(2))
        .with("tiny.attention.head_count_kv", Value::U32(1))
        .write(&scratch.path().join("a/draft.gguf"), 16 * MIB);
    // A projector carries no layers to cache for; only its bytes count.
    Gguf::v3()
        .with("general.architecture", Value::Text("clip".to_owned()))
        .write(&scratch.path().join("a/mmproj.gguf"), 8 * MIB);

    let reading = Catalog::read(
        &one_entry("draft_path = \"a/draft.gguf\"\nprojector_path = \"a/mmproj.gguf\""),
        scratch.path(),
    )
    .expect("derivable");

    // 88 MiB of weights and 4.4 MiB of fragmentation, 2 MiB plus 512 KiB of
    // cache, and 1024 MiB of overhead: 1118.9 MiB, rounded up.
    assert_eq!(
        reading
            .catalog
            .entry("alpha")
            .expect("alpha")
            .memory_estimate_mib,
        1119
    );
}

#[test]
fn every_shard_of_a_split_model_is_weighed() {
    let scratch = Scratch::new("catalog-shards");
    small_model()
        .with("split.count", Value::U16(3))
        .write(&scratch.path().join("a/big-00001-of-00003.gguf"), 4 * MIB);
    for shard in ["a/big-00002-of-00003.gguf", "a/big-00003-of-00003.gguf"] {
        Gguf::v3().write(&scratch.path().join(shard), 4 * MIB);
    }
    let text =
        "version = 1\n[models.alpha]\npath = \"a/big-00001-of-00003.gguf\"\ncontext_size = 1024\n";

    let reading = Catalog::read(text, scratch.path()).expect("derivable");

    // 12 MiB of weights across three files, 0.6 MiB of fragmentation, 2 MiB
    // of cache, 1024 MiB of overhead: 1038.6 MiB, rounded up.
    assert_eq!(
        reading
            .catalog
            .entry("alpha")
            .expect("alpha")
            .memory_estimate_mib,
        1039
    );
}

#[test]
fn a_file_that_is_not_readable_as_gguf_is_estimated_from_its_size_alone() {
    let scratch = Scratch::new("catalog-fallback");
    let path = scratch.path().join("a/model.gguf");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&path, vec![0u8; 10 << 20]).expect("ten mebibytes of nothing");

    let reading = Catalog::read(&one_entry(""), scratch.path()).expect("still derivable");

    // Size and a quarter, plus the fixed overhead: 1036.5 MiB, rounded up.
    assert_eq!(
        reading
            .catalog
            .entry("alpha")
            .expect("alpha")
            .memory_estimate_mib,
        1037
    );
    assert!(
        reading
            .notes
            .iter()
            .any(|note| note.contains("alpha") && note.contains("size")),
        "an estimate from size alone is worth saying, because it is the \
         rougher of the two:\n{:?}",
        reading.notes
    );
}
