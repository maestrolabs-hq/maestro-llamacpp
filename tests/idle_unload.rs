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
//! Every assertion here is either eventual -- `settled`, waiting for a
//! state that will arrive -- or a lower bound on when the reaper is allowed
//! to act. Neither kind fails because a machine is slow. An earlier version
//! of this file also asserted upper bounds: that a second request, sent
//! straight after a first, would reach the router inside the window, and
//! that a read of `loaded()` would land inside a fifty-millisecond margin
//! of it. A loaded Windows runner stalled those threads for whole seconds
//! and failed every such assertion, at a two-hundred-millisecond window and
//! again at three seconds. There is no window wide enough for a machine
//! that can pause a thread arbitrarily long, so no assertion here requires
//! this test's own threads to win a race against the reaper.
//!
//! `WINDOW` is three seconds not to outlast the platform's latency but to
//! keep the stamped-at-end proof discriminating: a `last_used` stamped at a
//! request's start would surface as an unload within half a window of the
//! stream ending, and half of three seconds is a margin that sweep timing
//! and sleep overshoot cannot blur past the stream-plus-window floor.
//!
//! `QUICK_WINDOW` stays short, for the two tests that sleep a multiple of
//! it to prove a *non*-event -- a resident or an unconfigured window never
//! unloading. Multiplying `WINDOW` there instead would turn a fast proof
//! into one lasting several seconds for no gain.

use std::time::{Duration, Instant};

mod support;
use support::{MODEL, ModelsRoot, get, post, request, settled, status, windowed};

/// The window the unloading tests use. Its width buys discrimination for
/// the stamped-at-end proof rather than safety margin against the platform:
/// see the module prose.
const WINDOW: Duration = Duration::from_secs(3);

/// The window the two tests that sleep a multiple of it to prove a
/// non-event use, kept short so proving "never unloaded" stays fast rather
/// than scaling with `WINDOW`.
const QUICK_WINDOW: Duration = Duration::from_millis(200);

/// The resident's weights, beside the on-demand entry's.
const RESIDENT_MODEL: &str = "cache/qwen/qwen3-4b.gguf";

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

    // Eventual on purpose. Anything stronger -- reading loaded() before the
    // window is out, or racing a second request against the reaper -- would
    // assert that this thread acts inside the window, which a stalled
    // runner refutes at any width. See the module prose.
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

    // A relayed 200 on the dedicated path is the proof of "loaded again":
    // the router authors only refusals, so this echo had to come from a
    // child, and a child had to be started for it -- settled just watched
    // the previous one go. An earlier shape of this proof asserted which
    // child through a third request, and that raced the reaper: the third
    // request had to arrive inside the window, which a stalled runner
    // refutes at any width.
    let second = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&second),
        Some(200),
        "the endpoint never went away, so a second request answers:\n{second}"
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
    // seconds of streaming against a three-second window, so a last_used
    // stamped at the request's start is already expired when the stream
    // ends.
    let started = Instant::now();
    let reply = request(
        serving.address(),
        &post(
            "/models/gemma3/v1/chat/completions",
            "{\"model\":\"gemma3\",\"stream\":true}",
        ),
    );
    assert_eq!(status(&reply), Some(200), "the stream completes:\n{reply}");

    settled(
        &serving,
        "unloaded the entry once it had actually gone idle",
        |s| !s.loaded().iter().any(|id| id == "gemma3"),
    );

    // The proof is a lower bound, which a slow machine can only widen. The
    // stub sleeps its gap before every event, so the stream takes at least
    // the events' product; a last_used stamped when the relay ended cannot
    // expire until a whole window after that, so the unload settled just
    // watched cannot land before stream plus window. One stamped at the
    // request's start expires during the stream itself, and the first sweep
    // after the relay releases the child -- within half a window -- unloads
    // it, well under this floor.
    let floor =
        Duration::from_millis(u64::from(SLOW_STREAM_EVENTS) * u64::from(SLOW_STREAM_GAP_MS))
            + WINDOW;
    assert!(
        started.elapsed() >= floor,
        "the entry was unloaded {:?} after the streaming request began, \
         before the {floor:?} its stream plus the window account for; \
         last_used must have been stamped before the relay ended",
        started.elapsed()
    );
}
