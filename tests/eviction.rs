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

/// A third, so one decision can name two entries and still have somewhere to
/// put what it made room for.
const THIRD_MODEL: &str = "cache/phi/phi-4.gguf";

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

/// Opens a stream and returns the connection with the reply still arriving,
/// alongside what arrived first.
///
/// Returned rather than read here, because the point of every case that uses
/// this is what happens to the stream while something else is going on. The
/// first read is waited on rather than assumed: the child has to be started
/// and the reply has to have begun before this counts as in flight, or the
/// caller races the very thing it is asserting about.
///
/// What that first reply *is* differs by case, so it is handed back rather
/// than judged here. One case needs a stream that began; another is content
/// with a refusal, the room having genuinely gone.
fn start_stream(address: std::net::SocketAddr, path: &str, body: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(address).expect("the router is listening");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read timeout, so a hang fails rather than blocking the suite");
    stream
        .write_all(post(path, body).as_bytes())
        .expect("write");

    let mut first = [0u8; 512];
    let read = stream.read(&mut first).expect("the router answers");
    assert!(
        read > 0,
        "something came back before anything else happened"
    );
    (stream, String::from_utf8_lossy(&first[..read]).into_owned())
}

/// The gate on the slot invariant, driven the way a relay drives it.
///
/// The rule the invariant states -- that a reference to a loaded child is
/// obtained only under its slot lock -- is what makes `Arc::strong_count` mean
/// "somebody is reading from this". Nothing in the compiler keeps it, because
/// it is about where clones are made rather than about types, so this is what
/// a reader who breaks it runs into.
///
/// The cost of breaking it is why this is asserted end to end rather than
/// against a stand-in: a slot emptied while somebody is reading leaves a
/// process the router no longer accounts for, and a stream that stops early is
/// indistinguishable from a model that finished.
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

    let (mut streaming, opening) = start_stream(
        serving.address(),
        "/v1/chat/completions",
        "{\"model\":\"gemma3\",\"stream\":true}",
    );
    assert_eq!(
        status(&opening),
        Some(200),
        "the stream is in flight before anything else happens:\n{opening}"
    );

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

    let mut rest = opening;
    drop(streaming.read_to_string(&mut rest));
    assert!(
        rest.contains("data: {\"n\":11}"),
        "the stream in flight arrived complete. A truncated one means the \
         child was unloaded while somebody was reading it, which a caller \
         cannot tell apart from a model that finished early:\n{rest}"
    );
}

#[test]
fn a_request_for_a_model_that_is_not_there_unloads_nothing() {
    // 3000 and 3000 against 4096, as the eviction case -- but the second
    // entry's file is missing from the root, so the start that eviction would
    // be making room for cannot succeed whatever is unloaded.
    let serving = budgeted(
        &two_entries(3000, 3000),
        ModelsRoot::with(&[MODEL]),
        Some(4096),
    );

    let first = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&first), Some(200), "the first answers:\n{first}");
    let warm = child_endpoint(&first);

    let second = request(serving.address(), &get("/models/qwen38/v1/echo"));
    assert_eq!(
        status(&second),
        Some(502),
        "the entry names a file the root does not carry:\n{second}"
    );

    assert_eq!(
        health(warm.as_str()),
        Some(200),
        "and the warm model is untouched. Ending it would have bought nothing: \
         the room was for a start that could never have happened, and the \
         operator would be left with neither model"
    );
}

/// Three entries whose estimates make one request unload the other two.
///
/// Two is the point. Unloading the first ends a process, and ending a process
/// is the slowest thing a decision does -- which is the window a request for
/// the second can arrive in.
///
/// `qwen38` streams slowly because the case below needs a reader that is still
/// reading when the decision reaches its slot.
fn three_entries() -> String {
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
         memory_estimate_mib = 3000\n\
         \n\
         [models.qwen38]\n\
         path = \"{SECOND_MODEL}\"\n\
         memory_estimate_mib = 3000\n\
         \n\
         [models.qwen38.flags]\n\
         stream-events = \"40\"\n\
         stream-gap = \"50\"\n\
         \n\
         [models.phi]\n\
         path = \"{THIRD_MODEL}\"\n\
         memory_estimate_mib = 9000\n"
    )
}

/// Both ways the race can go, each asserted for what it must leave behind.
///
/// The interleaving this drives: a decision reads every slot, finds qwen38
/// idle, and names it for unloading; a request for qwen38 then arrives and
/// takes a reference; the decision, acting on what it read a moment ago,
/// reaches qwen38's slot. Reading the signal under one lock acquisition and
/// acting on it under another is what makes that possible.
///
/// The consequence is silent rather than loud. The reference keeps the process
/// alive, so no stream is truncated -- but the slot is emptied while the
/// process runs, so the router believes it freed memory it did not, goes over
/// its budget, and undercounts that entry until the reader finishes.
///
/// Three outcomes are legitimate and each says something different, so each is
/// asserted rather than only the first:
///
/// - **served**, when the reader had not arrived: both idle entries went, and
///   the room is real;
/// - **refused at unload**, when it arrived inside the window: the colder
///   entry was already taken, and the busy one kept its slot;
/// - **refused by policy**, when it arrived before the snapshot: nothing was
///   taken at all.
///
/// Which one happens depends on the machine, so none of them is required. What
/// is required is that at least one iteration evicted something: an estimate
/// raised past the budget, or a model file dropped from the root, would leave
/// every iteration refusing for a reason this test does not guard, and it
/// would stay green while exercising nothing. The rule itself is proved
/// without a race in `proxy::loaded`; this is the end-to-end half.
#[test]
fn a_child_that_becomes_busy_during_a_decision_is_not_taken_from_its_slot() {
    // Swept rather than timed once: the window is the length of one process
    // kill, and where it falls depends on the machine. The eight staggers are
    // microseconds apart, so on a platform where spawning a process costs a
    // third of a second they exercise one interleaving at eight times the
    // price. The full sweep therefore runs only where that price is affordable
    // -- the weekly heavy tier, which sets this variable -- while every pull
    // request runs a two-point smoke that still guards the per-iteration slot
    // invariant below.
    let full_sweep = std::env::var_os("MAESTRO_EVICTION_FULL_SWEEP").is_some();
    let staggers: &[u64] = if full_sweep {
        &[0, 250, 500, 750, 1_000, 1_500, 2_000, 3_000]
    } else {
        &[0, 1_000]
    };
    let mut outcomes = Vec::new();

    for &micros in staggers {
        let serving = budgeted(
            &three_entries(),
            ModelsRoot::with(&[MODEL, SECOND_MODEL, THIRD_MODEL]),
            Some(9000),
        );
        let address = serving.address();

        // Both loaded and idle: 3000 and 3000 against 9000 leaves room for
        // neither to be disturbed yet. gemma3 first, so it is the colder of the
        // two and the decision below reaches it first.
        let first = request(address, &get("/models/gemma3/v1/echo"));
        let second = request(address, &get("/models/qwen38/v1/echo"));
        assert_eq!(status(&first), Some(200), "gemma3 is loaded:\n{first}");
        assert_eq!(status(&second), Some(200), "qwen38 is loaded:\n{second}");
        let cold = child_endpoint(&first);
        let warm = child_endpoint(&second);

        // The path names the model, so this depends on nothing the body says.
        let reader = std::thread::spawn(move || {
            sleep(Duration::from_micros(micros));
            start_stream(
                address,
                "/models/qwen38/v1/chat/completions",
                "{\"stream\":true}",
            )
            .0
        });

        // 9000 against a budget of 9000 already holding 6000: both of the
        // others have to go.
        let wanted = request(address, &get("/models/phi/v1/echo"));

        // Held across the assertions on purpose. The reference is what keeps
        // the evicted process alive, so dropping it first would let the very
        // thing this asserts about disappear before it is looked at.
        let streaming = reader.join().expect("the reading thread");

        let outcome = match status(&wanted) {
            Some(200) => {
                assert_eq!(
                    health(warm.as_str()),
                    None,
                    "phi was answered, so the room it needed was taken -- but \
                     the child at {warm} is still running, which means its slot \
                     was emptied while somebody was reading from it. The router \
                     is now over its budget by that entry's whole estimate and \
                     does not know it (stagger {micros}us)"
                );
                "served"
            }
            Some(503) => {
                // Whichever refusal this is, the busy child is untouched: still
                // running, and still the one its slot hands out. A slot emptied
                // under a reader would leave the process running unaccounted
                // for, which is the defect, and the next request would start a
                // second child for the same entry.
                assert_eq!(
                    health(warm.as_str()),
                    Some(200),
                    "phi was refused, so nothing took the busy child's room -- \
                     but the child at {warm} has stopped (stagger {micros}us)"
                );
                let again = request(address, &get("/models/qwen38/v1/echo"));
                assert_eq!(
                    child_endpoint(&again),
                    warm,
                    "and its slot still holds it. A different address here \
                     means the slot was emptied under its reader and a second \
                     child was started for one entry (stagger {micros}us)"
                );

                if wanted.contains("reached first") {
                    // The re-check fired: the decision had taken the colder
                    // entry before it reached the busy one.
                    assert!(
                        wanted.contains("qwen38"),
                        "the refusal names the model whose room it could not \
                         take:\n{wanted}"
                    );
                    assert_eq!(
                        health(cold.as_str()),
                        None,
                        "the colder entry was idle when the decision reached \
                         it, so it went (stagger {micros}us)"
                    );
                    "refused at unload"
                } else {
                    // The snapshot already saw it busy, so it was never a
                    // candidate and nothing was taken.
                    assert_eq!(
                        health(cold.as_str()),
                        Some(200),
                        "the decision refused before taking anything, so the \
                         colder entry is untouched (stagger {micros}us)"
                    );
                    "refused by policy"
                }
            }
            other => panic!(
                "phi was neither served nor refused ({other:?}) at stagger \
                 {micros}us:\n{wanted}"
            ),
        };

        outcomes.push((micros, outcome));
        drop(streaming);
    }

    // The coverage guard -- that the sweep made the decision fire at least
    // once -- belongs to the full sweep alone. Two points can legitimately
    // miss the window on a given machine, so asserting it on the smoke would
    // make it flaky for a property the smoke is not there to prove. The slot
    // invariant inside the loop is asserted either way.
    if full_sweep {
        assert!(
            outcomes.iter().any(|(_, kind)| *kind == "served"),
            "no iteration evicted anything, so this no longer exercises the \
             decision it is named for. An estimate raised past the budget, or a \
             model file dropped from the root, refuses every iteration for a \
             reason nothing here guards: {outcomes:?}"
        );
    }
}
