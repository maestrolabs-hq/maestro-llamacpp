//! Idle unloading, driven through the router rather than through the policy.
//!
//! `idle::tests` already proves the policy exhaustively without a clock or a
//! process. These are the same guarantees against a running reaper thread and
//! a real slot, which is a different claim: a rule that holds in a pure
//! function and a rule that holds when a thread is periodically emptying
//! slots are not the same rule until both have been watched.
//!
//! Every window here is two hundred milliseconds, injected through
//! `support::windowed` rather than through `MAESTRO_IDLE_UNLOAD_SECONDS`,
//! which the global constraints forbid: that variable is process-global and
//! would race every other test in this binary. At most two children, and
//! exactly one case here waits for a sweep.

use std::time::Duration;

mod support;
use support::{MODEL, ModelsRoot, get, post, request, settled, status, windowed};

/// The window every case in this file uses.
const WINDOW: Duration = Duration::from_millis(200);

/// The resident's weights, beside the on-demand entry's.
const RESIDENT_MODEL: &str = "cache/qwen/qwen3-4b.gguf";

/// How many paced events the slow-stream case asks for, and how far apart.
/// Their product outlasts `WINDOW`, so a `last_used` stamped at the request's
/// start would already be older than the window by the time it returns.
const SLOW_STREAM_EVENTS: u32 = 6;
const SLOW_STREAM_GAP_MS: u32 = 100;

/// The `gemma3` entry every case in this file starts from, with `extra`
/// appended -- another entry, or flags on this one.
///
/// Factored out once two catalog builders here started from the same header:
/// `resident_and_on_demand` and `on_demand_with_slow_stream` differ only in
/// what they append, mirroring how `support::catalog_text` takes its own
/// `extra`.
fn one_on_demand_entry(extra: &str) -> String {
    format!(
        "version = 1\n\
         \n\
         [defaults]\n\
         context_size = 4096\n\
         residency = \"on-demand\"\n\
         memory_estimate_mib = 512\n\
         startup_timeout_seconds = 30\n\
         \n\
         [models.gemma3]\n\
         path = \"{MODEL}\"\n\
         {extra}"
    )
}

/// One on-demand entry whose child streams slower than `WINDOW`.
fn on_demand_with_slow_stream() -> String {
    one_on_demand_entry(&format!(
        "\n[models.gemma3.flags]\n\
         stream-events = \"{SLOW_STREAM_EVENTS}\"\n\
         stream-gap = \"{SLOW_STREAM_GAP_MS}\"\n"
    ))
}

/// One resident entry and one on-demand entry, each on-demand by default.
fn resident_and_on_demand() -> String {
    one_on_demand_entry(&format!(
        "\n[models.resident]\n\
         path = \"{RESIDENT_MODEL}\"\n\
         residency = \"resident\"\n"
    ))
}

#[test]
fn an_on_demand_entry_idle_past_the_window_is_unloaded() {
    let serving = windowed(
        &resident_and_on_demand(),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        None,
        WINDOW,
    );

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&reply), Some(200), "the entry answers:\n{reply}");
    assert!(serving.loaded().iter().any(|id| id == "gemma3"));

    settled(&serving, "unloaded the idle entry", |s| {
        !s.loaded().iter().any(|id| id == "gemma3")
    });
}

#[test]
fn the_next_request_for_an_unloaded_entry_is_answered_and_it_is_loaded_again() {
    let serving = windowed(
        &resident_and_on_demand(),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        None,
        WINDOW,
    );

    let first = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&first), Some(200), "the entry answers:\n{first}");

    settled(&serving, "unloaded the idle entry", |s| {
        !s.loaded().iter().any(|id| id == "gemma3")
    });

    let second = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&second),
        Some(200),
        "the endpoint never went away, so a second request answers:\n{second}"
    );
    assert!(
        serving.loaded().iter().any(|id| id == "gemma3"),
        "and it is loaded again, which is what separates this from stopping \
         the router: {:?}",
        serving.loaded()
    );
}

#[test]
fn a_resident_outlives_the_window_and_is_still_named() {
    let serving = windowed(
        &resident_and_on_demand(),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        None,
        WINDOW,
    );

    settled(&serving, "loaded its resident", |s| {
        s.loaded().iter().any(|id| id == "resident")
    });

    // Several sweeps' worth of time, so a resident that were mistakenly
    // reapable would already be gone.
    std::thread::sleep(WINDOW * 6);

    assert!(
        serving.loaded().iter().any(|id| id == "resident"),
        "a resident is never a candidate, however long it sits idle: {:?}",
        serving.loaded()
    );
}

#[test]
fn with_no_window_configured_an_entry_idle_far_past_any_window_is_still_named() {
    let serving = windowed(
        &resident_and_on_demand(),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        None,
        Duration::ZERO,
    );

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&reply), Some(200), "the entry answers:\n{reply}");

    // Far past any window this file uses.
    std::thread::sleep(WINDOW * 6);

    assert!(
        serving.loaded().iter().any(|id| id == "gemma3"),
        "no window configured means no reaper, however long anything sits \
         idle: {:?}",
        serving.loaded()
    );
}

#[test]
fn last_used_is_stamped_when_a_relay_ends_rather_than_when_it_started() {
    let serving = windowed(
        &on_demand_with_slow_stream(),
        ModelsRoot::with(&[MODEL]),
        None,
        WINDOW,
    );

    // The relay outlasts the window: six events a hundred milliseconds apart
    // is 600ms of streaming against a 200ms window. A last_used stamped at
    // the request's start would already be well older than the window by the
    // time this returns.
    let reply = request(
        serving.address(),
        &post(
            "/models/gemma3/v1/chat/completions",
            "{\"model\":\"gemma3\",\"stream\":true}",
        ),
    );
    assert_eq!(status(&reply), Some(200), "the stream completes:\n{reply}");

    // A pause short of the window, but past at least one sweep interval, so
    // the reaper has had a chance to act on whatever `last_used` says. A
    // last_used stamped at the request's start is already 600ms + this pause
    // old, far past the 200ms window; one stamped when the relay ended is
    // only this pause old, still within it.
    std::thread::sleep(
        WINDOW
            .checked_sub(Duration::from_millis(50))
            .expect("WINDOW is longer than 50ms"),
    );

    assert!(
        serving.loaded().iter().any(|id| id == "gemma3"),
        "a relay that just finished must not look idle for the whole of its \
         own duration: {:?}",
        serving.loaded()
    );

    settled(
        &serving,
        "unloaded the entry once it had actually gone idle",
        |s| !s.loaded().iter().any(|id| id == "gemma3"),
    );
}
