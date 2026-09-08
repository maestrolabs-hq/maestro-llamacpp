//! Every model file under the models root is servable.
//!
//! The catalog names the models an operator has thought about. The root
//! holds every model they have downloaded, and the difference between the
//! two was, until this slice, a file that could not be invoked without an
//! edit and a restart. These tests fix what discovery finds, what it leaves
//! alone, and what it calls the entries it makes -- and then prove that one
//! of them answers through the router like any entry written by hand.

use maestro_llamacpp::catalog::{Catalog, EstimateSource, Reading, Residency};

mod fixtures;
use fixtures::{Gguf, Scratch, Value};

mod support;
use support::{get, request, status, stub_binary};

const MIB: u64 = 1024 * 1024;

/// A model whose estimate at 4096 tokens can be worked out by hand: four
/// layers, two key-value heads of width 64, so 8 MiB of f16 cache.
fn model() -> Gguf {
    Gguf::model("tiny", 4, 8192, 256)
        .with("tiny.attention.head_count", Value::U32(4))
        .with("tiny.attention.head_count_kv", Value::U32(2))
}

/// One catalog entry, its draft and its projector, and everything else a
/// root might hold.
const CATALOG: &str = "version = 1\n\
    [defaults]\n\
    context_size = 4096\n\
    startup_timeout_seconds = 30\n\
    [defaults.flags]\n\
    jinja = \"true\"\n\
    [models.alpha]\n\
    path = \"a/Alpha-Model.gguf\"\n\
    draft_path = \"a/draft.gguf\"\n\
    projector_path = \"a/mmproj-alpha.gguf\"\n\
    memory_estimate_mib = 512\n";

fn populated(label: &str) -> Scratch {
    let scratch = Scratch::new(label);
    let at = |relative: &str| scratch.path().join(relative);
    // Named by the catalog, so not discovered however they are called.
    model().write(&at("a/Alpha-Model.gguf"), 4 * MIB);
    model().write(&at("a/draft.gguf"), MIB);
    Gguf::v3().write(&at("a/mmproj-alpha.gguf"), MIB);
    // Found.
    model().write(&at("b/Other-Model.gguf"), 4 * MIB);
    // Left alone: a projector, and a draft that describes itself as a model.
    Gguf::v3().write(&at("b/mmproj-Other.gguf"), MIB);
    model().write(&at("b/Other-Model-FastMTP.gguf"), MIB);
    // One entry for three shards.
    model()
        .with("split.count", Value::U16(3))
        .write(&at("c/Big-00001-of-00003.gguf"), 4 * MIB);
    Gguf::v3().write(&at("c/Big-00002-of-00003.gguf"), 4 * MIB);
    Gguf::v3().write(&at("c/Big-00003-of-00003.gguf"), 4 * MIB);
    // An identifier the catalog already uses, and one the proxy reserves.
    model().write(&at("d/alpha.gguf"), 4 * MIB);
    model().write(&at("e/load.gguf"), 4 * MIB);
    // A shouted extension, trained on less context than the defaults ask.
    Gguf::model("tiny", 4, 2048, 256)
        .with("tiny.attention.head_count", Value::U32(4))
        .with("tiny.attention.head_count_kv", Value::U32(2))
        .write(&at("f/Tiny.GGUF"), 4 * MIB);
    // Two files with the same name in different places.
    model().write(&at("h/x.gguf"), 4 * MIB);
    model().write(&at("i/x.gguf"), 4 * MIB);
    // Not a model at all.
    std::fs::create_dir_all(at("g")).expect("mkdir");
    std::fs::write(at("g/notes.txt"), b"not a model").expect("write");
    scratch
}

fn read(scratch: &Scratch) -> Reading {
    Catalog::read(CATALOG, scratch.path()).expect("a usable catalog")
}

#[test]
fn every_model_file_the_catalog_does_not_name_becomes_an_entry() {
    let scratch = populated("discovery-found");
    let reading = read(&scratch);

    let ids: Vec<&str> = reading
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "alpha",
            "big",
            "d-alpha",
            "e-load",
            "i-x",
            "other-model",
            "tiny",
            "x"
        ],
        "the catalog's own entry, then everything found, ordered by identifier"
    );
    assert_eq!(reading.declared(), 1);
    assert_eq!(reading.discovered(), 7);
    assert!(
        reading.summary().contains("1 entries from the catalog")
            && reading.summary().contains("7 discovered"),
        "{}",
        reading.summary()
    );
    assert!(
        reading.catalog.is_discovered("other-model") && !reading.catalog.is_discovered("alpha")
    );
}

#[test]
fn a_discovered_entry_takes_the_defaults_and_a_derived_estimate() {
    let scratch = populated("discovery-fields");
    let reading = read(&scratch);

    let other = reading.catalog.entry("other-model").expect("found");
    assert_eq!(
        other.path.as_str(),
        "b/Other-Model.gguf",
        "relative, with the catalog's separator"
    );
    assert_eq!(
        other.residency,
        Residency::OnDemand,
        "never held loaded: nobody asked for it"
    );
    assert_eq!(other.context_size, 4096, "from the defaults table");
    assert_eq!(other.startup_timeout_seconds, 30, "from the defaults table");
    assert_eq!(other.flags.get("jinja").map(String::as_str), Some("true"));
    assert_eq!(other.draft_path, None);
    assert_eq!(other.projector_path, None);
    assert_eq!(
        reading.catalog.estimate_source("other-model"),
        Some(EstimateSource::Derived)
    );
    // 4 MiB of weights and a twentieth of that, 8 MiB of cache at 4096
    // tokens, and 1024 MiB of overhead: 1036.2 MiB, rounded up.
    assert_eq!(other.memory_estimate_mib, 1037);
}

#[test]
fn a_split_model_is_one_entry_weighed_across_its_shards() {
    let scratch = populated("discovery-shards");
    let reading = read(&scratch);

    let big = reading
        .catalog
        .entry("big")
        .expect("the first shard names it");
    assert_eq!(big.path.as_str(), "c/Big-00001-of-00003.gguf");
    assert!(
        reading.catalog.entry("big-00002-of-00003").is_none(),
        "the other shards are not entries of their own"
    );
    // Three shards of 4 MiB, and the rest as for the single file: 1044.6
    // MiB, rounded up -- which is only right if every shard was weighed.
    assert_eq!(big.memory_estimate_mib, 1045);
}

#[test]
fn a_name_the_catalog_or_the_proxy_already_uses_is_told_apart_by_its_directory() {
    let scratch = populated("discovery-names");
    let reading = read(&scratch);

    assert_eq!(
        reading
            .catalog
            .entry("d-alpha")
            .expect("renamed")
            .path
            .as_str(),
        "d/alpha.gguf",
        "the catalog's alpha keeps its name; the file takes its directory's"
    );
    assert_eq!(
        reading
            .catalog
            .entry("e-load")
            .expect("renamed")
            .path
            .as_str(),
        "e/load.gguf",
        "load is a path the proxy keeps for itself"
    );
    assert_eq!(
        reading.catalog.entry("x").expect("first").path.as_str(),
        "h/x.gguf"
    );
    assert_eq!(
        reading.catalog.entry("i-x").expect("second").path.as_str(),
        "i/x.gguf"
    );
    assert!(
        reading
            .notes
            .iter()
            .any(|note| note.contains("d-alpha") && note.contains("alpha")),
        "a renaming is said, or the operator looks for a name that is not there:\n{:?}",
        reading.notes
    );
}

#[test]
fn the_context_is_clamped_to_what_the_model_was_trained_for() {
    let scratch = populated("discovery-clamp");
    let reading = read(&scratch);

    assert_eq!(
        reading.catalog.entry("tiny").expect("found").context_size,
        2048,
        "the defaults ask for 4096, and the file says it was trained on 2048"
    );
}

#[test]
fn a_root_with_nothing_to_find_reads_as_the_catalog_alone() {
    let scratch = Scratch::new("discovery-empty");
    model().write(&scratch.path().join("a/Alpha-Model.gguf"), 4 * MIB);

    let reading = read(&scratch);

    assert_eq!(reading.discovered(), 0);
    assert_eq!(reading.catalog.entries.len(), 1);

    let missing = Catalog::read(CATALOG, &scratch.path().join("not-there")).expect("still usable");
    assert_eq!(
        missing.discovered(),
        0,
        "a root that is not there finds nothing, and the catalog's own entries \
         are refused at start rather than here, as they always were"
    );
}

/// The point of all of the above: a file that was only on disk answers.
#[test]
fn a_discovered_entry_is_served_like_any_other() {
    let scratch = populated("discovery-served");
    let reading = read(&scratch);
    let server = maestro_llamacpp::launch::Server::located(Some(&stub_binary()))
        .expect("the stub binary is built by cargo test");
    let limits = maestro_llamacpp::idle::Limits::new(
        maestro_llamacpp::admission::Budget::new(None),
        maestro_llamacpp::idle::IdleWindow::new(std::time::Duration::ZERO),
    );
    let router = std::sync::Arc::new(
        maestro_llamacpp::proxy::Router::bind(
            "127.0.0.1:0".parse().expect("a loopback address"),
            reading.catalog,
            scratch.path().to_path_buf(),
            server,
            limits,
        )
        .expect("an ephemeral loopback port"),
    );
    let address = router.address();
    let serving = std::sync::Arc::clone(&router);
    std::thread::spawn(move || serving.serve());

    let listing = request(address, &get("/v1/models"));
    assert!(
        listing.contains("\"other-model\""),
        "the listing carries what was found:\n{listing}"
    );

    let reply = request(address, &get("/models/other-model/v1/echo"));
    router.stop();
    assert_eq!(status(&reply), Some(200), "a child answered:\n{reply}");
    assert!(
        reply.contains("alias: other-model"),
        "and it is the child started for the discovered entry:\n{reply}"
    );
}
