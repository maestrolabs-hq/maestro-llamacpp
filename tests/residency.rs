//! Residency, driven through the router rather than through the policy.
//!
//! `admission` already proves in unit tests that a resident is never an
//! eviction candidate and that a refusal names it. Those are proofs about a
//! pure function. These are the same guarantees against running processes,
//! which is a different claim: a rule that holds in a decision and a rule that
//! holds when something is killing children are not the same rule until both
//! have been watched.
//!
//! Every entry here is backed by the stub, which loads instantly. That is what
//! keeps this cheap enough for continuous integration -- the Windows leg is
//! already near fifteen minutes, and Windows spawns a child roughly three
//! times slower than Linux. What a real load costs belongs to the manual
//! verification, not here.

mod support;
use support::{MODEL, ModelsRoot, budgeted, get, post, request, serving, settled, status};

/// The resident's weights, beside the on-demand entry's.
const RESIDENT_MODEL: &str = "cache/qwen/qwen3-4b.gguf";

/// The resident entry, named for the model it serves rather than the role.
///
/// `CONTEXT.md` holds that an identifier names a model and never a role, and a
/// test catalog that broke that rule would teach the next reader the wrong
/// thing about the real one.
const RESIDENT: &str = "qwen3-4b";

/// One resident entry and one on-demand entry, each stating its cost.
///
/// Written as text because that is how a catalog reaches the router, and
/// stated rather than inherited because every case here turns on the
/// arithmetic: a figure that has to be read out of a defaults table to be
/// known is a figure the next reader of this test will get wrong.
fn resident_and_on_demand(resident_mib: u32, on_demand_mib: u32) -> String {
    format!(
        "version = 1\n\
         \n\
         [defaults]\n\
         context_size = 4096\n\
         residency = \"on-demand\"\n\
         startup_timeout_seconds = 30\n\
         \n\
         [models.gemma3]\n\
         path = \"{MODEL}\"\n\
         memory_estimate_mib = {on_demand_mib}\n\
         \n\
         [models.{RESIDENT}]\n\
         path = \"{RESIDENT_MODEL}\"\n\
         residency = \"resident\"\n\
         memory_estimate_mib = {resident_mib}\n"
    )
}

/// Waits for the resident to be loaded, which is what startup promises.
fn resident_loaded(serving: &support::Serving) {
    settled(serving, "loaded its resident", |s| {
        s.loaded().iter().any(|id| id == RESIDENT)
    });
}

#[test]
fn a_resident_entry_is_loaded_before_any_request_reaches_it() {
    let serving = serving(
        &resident_and_on_demand(512, 512),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
    );

    resident_loaded(&serving);

    // Nothing above sent a request. A resident that were only loaded because
    // something asked for it would not be resident at all, so the on-demand
    // entry staying empty is half of what makes this a residency test rather
    // than a routing one.
    assert!(
        !serving.loaded().iter().any(|id| id == "gemma3"),
        "the on-demand entry must still be unloaded, or nothing here \
         distinguishes residency from ordinary loading: loaded {:?}",
        serving.loaded()
    );
}

#[test]
fn a_resident_that_cannot_load_leaves_the_rest_of_the_catalog_serving() {
    // The resident's file is absent from the root, so its start cannot
    // succeed. Refusing to serve at all in that case would let one bad entry
    // deny service to every other model, which is worse than the state it
    // would be protecting against.
    let serving = serving(
        &resident_and_on_demand(512, 512),
        ModelsRoot::with(&[MODEL]),
    );

    settled(&serving, "reported its resident as failed", |s| {
        !s.resident_failures().is_empty()
    });

    let failures = serving.resident_failures();
    assert!(
        failures.iter().any(|line| line.contains(RESIDENT)),
        "the failure must name the entry, or an operator cannot act on it: \
         {failures:?}"
    );

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&reply),
        Some(200),
        "a resident that cannot load must not take the rest of the catalog \
         down with it:\n{reply}"
    );
}

#[test]
fn a_resident_holds_its_room_against_an_on_demand_entry() {
    // 512 and 512 against 768: the resident fits, and the pair does not. The
    // only thing holding the room is a resident, and a resident is never a
    // candidate -- so this is a refusal rather than a swap.
    let serving = budgeted(
        &resident_and_on_demand(512, 512),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        Some(768),
    );

    resident_loaded(&serving);

    let reply = request(
        serving.address(),
        &post("/v1/chat/completions", "{\"model\":\"gemma3\"}"),
    );
    assert_eq!(
        status(&reply),
        Some(503),
        "the room is held by a resident, which is never unloaded:\n{reply}"
    );
    assert!(
        reply.contains(RESIDENT),
        "the refusal names what is holding the memory, or the operator is \
         told only that something is:\n{reply}"
    );
    assert!(
        serving.loaded().iter().any(|id| id == RESIDENT),
        "and the resident is still loaded afterwards, which is the guarantee: \
         loaded {:?}",
        serving.loaded()
    );
}
