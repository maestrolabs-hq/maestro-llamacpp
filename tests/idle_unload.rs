//! Idle unloading, driven through the router rather than through the policy.
//!
//! `idle::tests` already proves the policy exhaustively without a clock or a
//! process. These are the same guarantees against a running reaper thread and
//! a real slot, which is a different claim: a rule that holds in a pure
//! function and a rule that holds when a thread is periodically emptying
//! slots are not the same rule until both have been watched.
//!
//! Windows are injected through `support::windowed` rather than through
//! `MAESTRO_IDLE_UNLOAD_SECONDS`, which the global constraints forbid: that
//! variable is process-global and would race every other test in this
//! binary. At most two children, and exactly one case here waits for a
//! sweep.
//!
//! `WINDOW` is three seconds rather than the two hundred milliseconds an
//! earlier version of this file used. A round trip through a real child --
//! spawning it, on Windows roughly three times slower than on Linux, plus
//! the request itself -- can itself take longer than two hundred
//! milliseconds on a loaded Windows runner. A window that short raced that
//! latency: two requests sent back to back, with no sleep between them,
//! could already be more than one window apart by the time the second
//! reached the router, so a live reaper reaped the model between them and a
//! test asserting both landed on the same child failed -- correctly,
//! against a window that was never wide enough to hold the platform's own
//! overhead. Three seconds leaves that overhead comfortably inside a single
//! window on every platform this suite runs on.
//!
//! `QUICK_WINDOW` stays short, for the two tests that sleep a multiple of
//! it to prove a *non*-event -- a resident or an unconfigured window never
//! unloading. Multiplying `WINDOW` there instead would turn a fast proof
//! into one lasting several seconds for no gain: neither test's assertion
//! depends on the window being wide enough to survive real network
//! latency, since neither sends a second request to race against.

use std::time::Duration;

mod support;
use support::{MODEL, ModelsRoot, get, post, request, settled, status, windowed};

/// The window the tests that send a second request use, wide enough that a
/// slow child spawn and an HTTP round trip on any platform this suite runs
/// on cannot by themselves push two back-to-back requests a whole window
/// apart.
const WINDOW: Duration = Duration::from_secs(3);

/// The window the two tests that sleep a multiple of it to prove a
/// non-event use, kept short so proving "never unloaded" stays fast rather
/// than scaling with `WINDOW`.
const QUICK_WINDOW: Duration = Duration::from_millis(200);

/// The resident's weights, beside the on-demand entry's.
const RESIDENT_MODEL: &str = "cache/qwen/qwen3-4b.gguf";

/// The address the child answering this request was reached on.
///
/// The stub reflects the `Host` it was given, which the relay rewrote to the
/// child's own address. That is how a test learns a port the router never
/// told anyone about, and it is what makes "this is the same child" or "a
/// different one" observable rather than inferred from timing.
fn child_endpoint(reply: &str) -> String {
    reply
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .expect("the echo carries the address the child was reached on")
        .to_owned()
}

/// How many paced events the slow-stream case asks for, and how far apart.
/// Their product -- five seconds -- clears `WINDOW` with two seconds to
/// spare, so a `last_used` stamped at the request's start would already be
/// older than the window by the time it returns, on every platform this
/// suite runs on.
const SLOW_STREAM_EVENTS: u32 = 10;
const SLOW_STREAM_GAP_MS: u32 = 500;

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

    // Proved by re-requesting rather than by reading loaded() right after the
    // first reply: that read races a live reaper ticking every 100ms, and on
    // a loaded machine the gap between the reply arriving and this line
    // running is not bounded. A second request that reaches the same child
    // is a fact about what actually served it, not an assumption about how
    // fast the test thread runs.
    let again = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&again),
        Some(200),
        "the same entry answers again:\n{again}"
    );
    assert_eq!(
        child_endpoint(&again),
        child_endpoint(&reply),
        "the first request must not have made its own model look idle for \
         the whole of its own duration"
    );

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

    // Proved by re-requesting rather than by reading loaded() right after
    // `second`, for the same reason as the sibling test above: that read
    // races a live reaper, and the margin against a 200ms window is not one
    // a loaded test machine can be trusted to keep. A third request reaching
    // the same child `second` did is what "loaded again" actually means --
    // the router did not stop serving the entry, which is what separates
    // this from stopping the router.
    let third = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&third),
        Some(200),
        "a third request answers:\n{third}"
    );
    assert_eq!(
        child_endpoint(&third),
        child_endpoint(&second),
        "the model reloaded for the second request must not have been \
         unloaded again before this one reached it: {:?}",
        serving.loaded()
    );
}

#[test]
fn a_resident_outlives_the_window_and_is_still_named() {
    let serving = windowed(
        &resident_and_on_demand(),
        ModelsRoot::with(&[MODEL, RESIDENT_MODEL]),
        None,
        QUICK_WINDOW,
    );

    settled(&serving, "loaded its resident", |s| {
        s.loaded().iter().any(|id| id == "resident")
    });

    // Several sweeps' worth of time, so a resident that were mistakenly
    // reapable would already be gone. Against `QUICK_WINDOW` rather than
    // `WINDOW`, so proving a non-event stays fast.
    std::thread::sleep(QUICK_WINDOW * 6);

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

    // Far past any window this file uses, against `QUICK_WINDOW` rather than
    // `WINDOW`, so proving a non-event stays fast.
    std::thread::sleep(QUICK_WINDOW * 6);

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

    // The relay outlasts the window: ten events half a second apart is five
    // seconds of streaming against a three-second window. A last_used
    // stamped at the request's start would already be well older than the
    // window by the time this returns.
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
    // last_used stamped at the request's start is already five seconds plus
    // this pause old, far past the window; one stamped when the relay ended
    // is only this pause old, still within it.
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
