//! Waiting for room rather than refusing it.
//!
//! Its own target, beside `eviction.rs` rather than inside it: those cases
//! assert what a *full* router refuses, and every one of them would be held
//! for the length of a wait before asserting the same thing. Here the wait is
//! the subject.
//!
//! The distinction under test is between two refusals that look alike. A model
//! larger than the whole budget will never fit and is refused at once, however
//! long anybody waits. A model whose room is held by something still answering
//! will fit the moment that answer ends, and waiting for it is the difference
//! between a router that queues and one that makes every caller write a retry
//! loop.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

mod support;
use support::{MODEL, ModelsRoot, post, queued, request, status};

/// A second model file, so there is something to want the room for.
const SECOND_MODEL: &str = "cache/qwen/qwen3-8b.gguf";

/// Two entries that cannot both be held under the budget each test states.
///
/// The first is paced: twelve events a hundred milliseconds apart is over a
/// second of stream, which is long enough for a second request to be sent,
/// decided and answered while the first is still arriving -- on a loaded
/// continuous-integration machine as well as an idle one. The pacing costs
/// nothing in the case that never asks for a stream.
fn two_entries(first_mib: u32, second_mib: u32) -> String {
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
         memory_estimate_mib = {first_mib}\n\
         \n\
         [models.gemma3.flags]\n\
         stream-events = \"12\"\n\
         stream-gap = \"100\"\n\
         \n\
         [models.qwen38]\n\
         path = \"{SECOND_MODEL}\"\n\
         memory_estimate_mib = {second_mib}\n"
    )
}

/// The body that asks the stub for a paced stream.
const STREAM: &str = "{\"model\":\"gemma3\",\"stream\":true}";

/// Opens a streamed request and returns once the first bytes have arrived.
///
/// Waited on rather than assumed: the child has to be started and the reply
/// has to have begun before the model counts as busy, or the test races the
/// thing it is asserting about.
fn start_stream(address: SocketAddr, path: &str, body: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).expect("the router is listening");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read timeout, so a hang fails rather than blocking the suite");
    stream
        .write_all(post(path, body).as_bytes())
        .expect("write");

    let mut first = [0u8; 512];
    let read = stream.read(&mut first).expect("the router answers");
    assert!(read > 0, "the stream began before anything else happened");
    stream
}

#[test]
fn a_request_waits_for_room_a_busy_model_is_holding() {
    // 3000 and 3000 against 4096: whichever is loaded, the other needs its
    // room. The first is made busy, so there is nothing to evict until its
    // answer finishes.
    let serving = queued(
        &two_entries(3000, 3000),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
        Duration::from_secs(30),
    );

    let streaming = start_stream(serving.address(), "/v1/chat/completions", STREAM);

    // Asked for while the first is still answering. Before the wait existed
    // this was a 503 the instant it arrived.
    let began = Instant::now();
    let second = request(
        serving.address(),
        &post("/models/qwen38/v1/echo", "{\"say\":\"hello\"}"),
    );
    let waited = began.elapsed();

    assert_eq!(
        status(&second),
        Some(200),
        "the room freed up when the first answer finished, and the second \
         request was still there to take it:\n{second}"
    );
    assert!(
        waited >= Duration::from_millis(200),
        "it was answered in {waited:?}, which is too fast to have waited for \
         a stream that takes over a second: the room cannot have been held"
    );

    drop(streaming);
}

#[test]
fn a_wait_of_zero_refuses_exactly_as_it_did_before() {
    let serving = queued(
        &two_entries(3000, 3000),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
        Duration::ZERO,
    );

    let streaming = start_stream(serving.address(), "/v1/chat/completions", STREAM);

    let began = Instant::now();
    let second = request(
        serving.address(),
        &post("/models/qwen38/v1/echo", "{\"say\":\"hello\"}"),
    );
    let waited = began.elapsed();

    assert_eq!(
        status(&second),
        Some(503),
        "an operator who set 0 said 'do not wait', and gets the refusal this \
         router gave before waiting existed:\n{second}"
    );
    assert!(
        waited < Duration::from_secs(1),
        "refused in {waited:?}, which is long enough to have waited: zero has \
         to mean zero or the setting says nothing"
    );
    assert!(
        second.contains("gemma3"),
        "the refusal names what is holding the memory:\n{second}"
    );

    drop(streaming);
}

#[test]
fn a_model_larger_than_the_budget_is_refused_without_waiting() {
    // 5000 against 4096: no eviction makes this fit, so waiting for one would
    // be waiting for something that cannot happen.
    let serving = queued(
        &two_entries(1000, 5000),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
        Duration::from_secs(30),
    );

    let began = Instant::now();
    let reply = request(
        serving.address(),
        &post("/models/qwen38/v1/echo", "{\"say\":\"hello\"}"),
    );
    let waited = began.elapsed();

    assert_eq!(
        status(&reply),
        Some(503),
        "nothing can be unloaded to make it fit:\n{reply}"
    );
    assert!(
        waited < Duration::from_secs(1),
        "refused in {waited:?}: a permanent refusal must not be held for the \
         wait, because no amount of waiting changes the answer"
    );
}
