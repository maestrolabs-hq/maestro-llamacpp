//! Supervising one server child, through the interface a caller holds.
//!
//! Every test here points the router at `stub-llama-server` rather than the
//! real thing. That is the second adapter at the seam, not a mock: the code
//! under test picks a port, builds a command line, spawns, polls `/health`,
//! detects an exit and kills, and every one of those steps runs for real. The
//! only thing the stub avoids is needing a multi-gigabyte model and a
//! graphics card, neither of which exists in continuous integration.
//!
//! The stub is driven through the entry's own flags table, because that table
//! is passed through to the server untouched. `ready-after` becomes
//! `--ready-after`, so a test asks for a slow start using the same mechanism
//! a catalog uses to ask for anything else.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant};

use maestro_llamacpp::catalog::{Entry, RelativePath, Residency};
use maestro_llamacpp::launch::{Failure, Liveness, Server};

mod support;
use support::{ModelsRoot, health, stub_binary};

/// The one model file every entry below points at.
const MODEL: &str = "cache/gemma/gemma-3-1b.gguf";

fn entry(id: &str) -> Entry {
    Entry {
        id: id.to_owned(),
        path: RelativePath::new(MODEL).expect("relative"),
        draft_path: None,
        projector_path: None,
        context_size: 4096,
        residency: Residency::OnDemand,
        memory_estimate_mib: 512,
        reasoning_format: None,
        reasoning_effort: None,
        startup_timeout_seconds: 30,
        flags: BTreeMap::new(),
    }
}

/// The router, pointed at the stub.
fn server() -> Server {
    Server::located(Some(&stub_binary())).expect("the stub binary is built by cargo test")
}

#[test]
fn a_child_becomes_ready_and_its_endpoint_answers() {
    let root = ModelsRoot::with(&[MODEL]);
    let mut child = server()
        .start(&entry("gemma3"), root.path())
        .expect("the stub becomes ready");

    assert_eq!(
        health(child.endpoint()),
        Some(200),
        "start returns only once the child answers, so the caller never has \
         to wait again"
    );
    assert!(
        child.endpoint().ip().is_loopback(),
        "a child binds loopback only"
    );
    child.stop();
}

#[test]
fn a_child_that_never_becomes_ready_fails_when_its_budget_expires() {
    let root = ModelsRoot::with(&[MODEL]);
    let mut entry = entry("slowpoke");
    entry.startup_timeout_seconds = 2;
    // Far longer than the budget, so the budget is what ends the wait.
    entry
        .flags
        .insert("ready-after".to_owned(), "600000".to_owned());

    let failure = server()
        .start(&entry, root.path())
        .expect_err("the budget must expire");

    assert!(
        matches!(failure, Failure::NotReady(_)),
        "a child that started and did not answer is distinct from one that \
         could not start, because the proxy answers them with different \
         statuses: {failure:?}"
    );
    let failure = failure.to_string();
    assert!(
        failure.contains("slowpoke"),
        "the failure names the entry, so the reader is not sent back to the \
         catalog to guess:\n{failure}"
    );
    assert!(
        failure.contains('2'),
        "and the budget it exhausted:\n{failure}"
    );
}

#[test]
fn a_child_that_cannot_start_is_distinct_from_one_that_is_slow() {
    // A root with no files in it, so the entry names a model that is not there
    // and nothing is ever spawned.
    let root = ModelsRoot::with(&[]);

    let failure = server()
        .start(&entry("gemma3"), root.path())
        .expect_err("the model file is not there");

    assert!(
        matches!(failure, Failure::Unavailable(_)),
        "never starting is not the same as starting slowly, and the proxy \
         maps them to different statuses: {failure:?}"
    );
}

#[test]
fn a_child_that_exits_while_loading_fails_with_its_status_not_the_budget() {
    let root = ModelsRoot::with(&[MODEL]);
    let mut entry = entry("crasher");
    entry.startup_timeout_seconds = 300;
    entry
        .flags
        .insert("ready-after".to_owned(), "600000".to_owned());
    entry
        .flags
        .insert("exit-after".to_owned(), "250".to_owned());
    entry.flags.insert("exit-code".to_owned(), "9".to_owned());

    let started = Instant::now();
    let failure = server()
        .start(&entry, root.path())
        .expect_err("a child that dies never becomes ready")
        .to_string();

    assert!(
        started.elapsed().as_secs() < 30,
        "an exit is noticed immediately rather than waiting out the whole \
         budget, which here is 300 seconds"
    );
    assert!(
        failure.contains("crasher"),
        "the failure names the entry:\n{failure}"
    );
}

#[test]
fn stopping_a_child_terminates_it_and_check_reports_the_exit() {
    let root = ModelsRoot::with(&[MODEL]);
    let mut child = server()
        .start(&entry("gemma3"), root.path())
        .expect("the stub becomes ready");

    assert_eq!(
        child.check(),
        Liveness::Running,
        "alive before it is stopped"
    );

    child.stop();

    assert!(
        matches!(child.check(), Liveness::Exited(_)),
        "and gone afterwards, reaped rather than left as a zombie"
    );
}

#[test]
fn two_children_started_together_get_different_ports() {
    let root = ModelsRoot::with(&[MODEL]);
    let server = server();
    let mut first = server
        .start(&entry("first"), root.path())
        .expect("the first starts");
    let mut second = server
        .start(&entry("second"), root.path())
        .expect("the second starts");

    assert_ne!(
        first.endpoint().port(),
        second.endpoint().port(),
        "each child gets its own port, or the second would have failed to bind"
    );

    first.stop();
    second.stop();
}

/// A child that outlives the value representing it is a leaked server.
///
/// This is not only tidiness. A caller that drops a `Child` on an error path
/// would otherwise leave a process holding a port and whatever memory the
/// model took, with nothing left in the program that knows about it. It is
/// also how a run of these tests left ten stubs behind: a panicking test drops
/// its child, and every one of them kept running.
#[test]
fn a_dropped_child_does_not_outlive_the_router() {
    let root = ModelsRoot::with(&[MODEL]);
    let address = {
        let child = server()
            .start(&entry("dropped"), root.path())
            .expect("the stub becomes ready");
        let address = child.endpoint();
        assert_eq!(
            health(address),
            Some(200),
            "answering before it is dropped, so the assertion below is not vacuous"
        );
        address
    };

    for _ in 0..200 {
        if health(address).is_none() {
            return;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("a dropped child kept serving on {address}");
}

/// Slice 1 validated the shape of a location and deferred its existence to
/// here, which is the first code with a models root to resolve against.
#[test]
fn a_missing_model_file_names_the_entry_and_the_place_it_looked() {
    let root = ModelsRoot::with(&[]);
    let failure = server()
        .start(&entry("absent"), root.path())
        .expect_err("the model file is not there")
        .to_string();

    assert!(
        failure.contains("absent"),
        "the failure names the entry:\n{failure}"
    );
    assert!(
        failure.contains("gemma-3-1b.gguf"),
        "and the location it resolved to, so the reader can look:\n{failure}"
    );
}
