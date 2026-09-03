//! What the router unloads to make room, and what it refuses to touch.
//!
//! Its own target, because a reader who sees this fail should think about the
//! policy rather than about routing. `admission.rs` proves the decision from
//! four values without a process; this proves the router acts on it, which is
//! the half that involves killing something.
//!
//! Every case states its budget directly and its estimates in the catalog
//! text. Nothing here measures memory and nothing here reads the environment:
//! a test that set `MAESTRO_MEMORY_BUDGET_MIB` would race every other test in
//! its binary, and one that measured would assert about the machine it
//! happened to run on.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread::sleep;
use std::time::{Duration, Instant};

mod support;
use support::{MODEL, ModelsRoot, budgeted, get, health, post, request, serving, status};

/// A second model file, so there is something to evict in favour of.
const SECOND_MODEL: &str = "cache/qwen/qwen3-8b.gguf";

/// Two entries, each stating what it is estimated to cost.
///
/// Written as text because that is how a catalog reaches the router, and
/// because the estimate is the number every case here turns on -- stating it
/// beside the entry keeps the arithmetic of each test readable at the top of
/// it rather than hidden in a helper.
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
         [models.qwen38]\n\
         path = \"{SECOND_MODEL}\"\n\
         memory_estimate_mib = {second_mib}\n"
    )
}

/// The address the child answering this request was reached on.
///
/// The stub reflects the `Host` it was given, which the relay rewrote to the
/// child's own address. That is how these tests learn a port the router never
/// told anyone about, and it is what makes "this child stopped" observable
/// rather than inferred.
fn child_endpoint(reply: &str) -> String {
    reply
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .expect("the echo carries the address the child was reached on")
        .to_owned()
}

/// Waits for a child to stop answering, or says how long it did not.
///
/// Polled rather than slept on: killing a process is not instantaneous, and a
/// fixed wait is either flaky on a loaded machine or slow on an idle one.
fn assert_stops_answering(endpoint: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if health(endpoint).is_none() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("the child at {endpoint} was not unloaded to make room");
}

#[test]
fn two_entries_that_both_fit_are_both_loaded_at_once() {
    // 512 and 512 against 4096: neither needs the other's room.
    let serving = budgeted(
        &two_entries(512, 512),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
    );

    let first = request(serving.address(), &get("/models/gemma3/v1/echo"));
    let second = request(serving.address(), &get("/models/qwen38/v1/echo"));
    assert_eq!(status(&first), Some(200), "the first answers:\n{first}");
    assert_eq!(status(&second), Some(200), "the second answers:\n{second}");

    // The first is asked again, and its child must be the same one. A router
    // that unloaded it needlessly would answer this from a new process, which
    // is a different port.
    let again = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        child_endpoint(&first),
        child_endpoint(&again),
        "nothing was unloaded, because nothing needed the room"
    );
}

#[test]
fn an_entry_that_does_not_fit_unloads_the_one_holding_the_room() {
    // 3000 and 3000 against 4096: the second cannot load until the first goes.
    let serving = budgeted(
        &two_entries(3000, 3000),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
    );

    let first = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&first), Some(200), "the first answers:\n{first}");
    let evicted = child_endpoint(&first);
    assert_eq!(
        health(evicted.as_str()),
        Some(200),
        "the first child is running before the second is asked for"
    );

    let second = request(serving.address(), &get("/models/qwen38/v1/echo"));
    assert_eq!(
        status(&second),
        Some(200),
        "the second answers, having made its own room:\n{second}"
    );

    assert_stops_answering(&evicted);
}

#[test]
fn no_budget_at_all_loads_everything_and_unloads_nothing() {
    // Estimates far past anything a machine has, and no budget to weigh them
    // against. `serving` is the no-budget case, which is what every test in
    // slices 1 to 3 runs under.
    let serving = serving(
        &two_entries(900_000, 900_000),
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
    );

    let first = request(serving.address(), &get("/models/gemma3/v1/echo"));
    let second = request(serving.address(), &get("/models/qwen38/v1/echo"));
    assert_eq!(status(&first), Some(200), "the first answers:\n{first}");
    assert_eq!(status(&second), Some(200), "the second answers:\n{second}");

    assert_eq!(
        health(child_endpoint(&first).as_str()),
        Some(200),
        "an unset budget means no eviction, however large the estimates"
    );
}

/// Opens a stream and returns the connection with the reply still arriving.
///
/// Returned rather than read here, because the point of every case that uses
/// this is what happens to the stream while something else is going on.
fn start_stream(address: std::net::SocketAddr, path: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).expect("the router is listening");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read timeout, so a hang fails rather than blocking the suite");
    stream
        .write_all(post(path, "{\"model\":\"gemma3\",\"stream\":true}").as_bytes())
        .expect("write");

    // Waited on rather than assumed: the child has to be started and the
    // first event has to have arrived before this counts as in flight, or the
    // test races the very thing it is asserting about.
    let mut first = [0u8; 512];
    let read = stream.read(&mut first).expect("the stream begins");
    assert!(read > 0, "the stream begins before anything else happens");
    stream
}

#[test]
fn a_child_with_a_stream_in_flight_is_not_unloaded() {
    // 3000 and 3000 against 4096, as the eviction case: the second entry can
    // only load if the first goes -- and the first is busy, so it cannot.
    //
    // Twelve events a hundred milliseconds apart is over a second of stream,
    // which is long enough for the second request to be sent, decided and
    // answered while the first is still arriving -- on a loaded continuous-
    // integration machine as well as an idle one.
    let catalog = format!(
        "{}\n\
         [models.gemma3.flags]\n\
         stream-events = \"12\"\n\
         stream-gap = \"100\"\n",
        two_entries(3000, 3000)
    );
    let serving = budgeted(
        &catalog,
        ModelsRoot::with(&[MODEL, SECOND_MODEL]),
        Some(4096),
    );

    let mut streaming = start_stream(serving.address(), "/v1/chat/completions");

    // Asked for while the stream above is still arriving. Whatever this
    // answers, it must not have been served by killing the process that is
    // mid-sentence.
    let second = request(serving.address(), &get("/models/qwen38/v1/echo"));
    assert_eq!(
        status(&second),
        Some(503),
        "the only candidate is busy, so there is no room to make:\n{second}"
    );
    assert!(
        second.contains("gemma3"),
        "the refusal names what is holding the memory:\n{second}"
    );

    let mut rest = String::new();
    drop(streaming.read_to_string(&mut rest));
    assert!(
        rest.contains("data: {\"n\":11}"),
        "the stream in flight arrived complete. A truncated one means the \
         child was unloaded while somebody was reading it, which a caller \
         cannot tell apart from a model that finished early:\n{rest}"
    );
}
