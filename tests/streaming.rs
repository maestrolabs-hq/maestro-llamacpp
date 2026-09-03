//! The three properties of the relay that are about time rather than content.
//!
//! Separate from `proxy.rs` on purpose. Every other test in this repository
//! asserts what a reply says, and a reply says the same thing whether or not
//! it was buffered on the way. These three are the only gate on the property
//! the slice exists to preserve, so a reader who sees this target fail should
//! think about the relay and the clock rather than about routing.
//!
//! Total elapsed time is deliberately never asserted. A buffering proxy and a
//! streaming one finish at the same moment -- that was measured before this
//! slice was written -- so a duration assertion would pass against the design
//! this slice rejects.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

mod support;
use support::{MODEL, ModelsRoot, arrivals, catalog_text, get, post, request, serving, status};

/// The completion path, which the stub answers with a paced stream.
const COMPLETIONS: &str = "/models/gemma3/v1/chat/completions";

/// A body of the shape a client sends, so the relay forwards a real one.
const BODY: &str = "{\"model\":\"gemma3\",\"stream\":true}";

/// How many events the pacing tests ask for, and how far apart.
const EVENTS: u32 = 5;
const GAP_MS: u32 = 150;

fn paced(extra: &str) -> String {
    catalog_text(&format!(
        "[models.gemma3.flags]\n\
         stream-events = \"{EVENTS}\"\n\
         stream-gap = \"{GAP_MS}\"\n\
         {extra}"
    ))
}

/// How long the stub takes to produce the whole stream.
fn production() -> Duration {
    Duration::from_millis(u64::from(EVENTS * GAP_MS))
}

#[test]
fn a_paced_stream_reaches_the_caller_as_it_arrives() {
    let serving = serving(&paced(""), ModelsRoot::with(&[MODEL]));

    // The timed request must find a ready child, and this is what makes it
    // one. A child starts on the first request for its entry, so timing that
    // request would measure process spawn and a health poll as well as the
    // relay: about 100ms of it on Linux and 370ms on Windows, which is enough
    // to push the first arrival past a threshold the relay had nothing to do
    // with. It is asserted rather than merely sent, because a warm-up that
    // silently failed would put the startup cost back into the measurement
    // with nobody the wiser.
    //
    // Do not remove this as redundant. Without it the assertion below measures
    // the operating system rather than the property this test is named for.
    let warmed = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&warmed),
        Some(200),
        "the child is ready before anything is timed:\n{warmed}"
    );

    let (body, times) = arrivals(serving.address(), &post(COMPLETIONS, BODY));

    assert!(
        body.contains("data: {\"n\":0}") && body.contains("data: {\"n\":4}"),
        "every event arrives:\n{body}"
    );

    let first = *times.first().expect("at least one arrival");
    let last = *times.last().expect("at least one arrival");
    let half = production() / 2;

    // Two assertions, because either alone can be fooled. Spread catches a
    // relay that batched the events; first arrival catches one that buffered
    // the whole body to end-of-file.
    assert!(
        last.saturating_sub(first) >= half,
        "the events spread over at least half their production time, or they \
         were batched; arrivals were {times:?}"
    );
    assert!(
        first <= half,
        "the first event arrives in the first half, or the whole body was \
         buffered; arrivals were {times:?}"
    );
}

#[test]
fn a_mid_stream_upstream_death_truncates_rather_than_hanging() {
    let serving = serving(
        &paced("die-after-events = \"2\"\n"),
        ModelsRoot::with(&[MODEL]),
    );

    // The read timeout in `arrivals` is the safety net: if the relay hung
    // waiting on a dead upstream, this returns late rather than never, and the
    // assertions below still describe what went wrong.
    let (body, _) = arrivals(serving.address(), &post(COMPLETIONS, BODY));

    assert!(
        body.contains("data: {\"n\":1}"),
        "what the child produced before it died arrives:\n{body}"
    );
    assert!(
        !body.contains("data: {\"n\":2}"),
        "and nothing after it does:\n{body}"
    );
}

#[test]
fn a_caller_that_hangs_up_closes_the_upstream_connection() {
    let root = ModelsRoot::with(&[MODEL]);
    let marker = root.path().join("hangup");
    // A TOML literal string, so a Windows path's separators are not read as
    // escapes. The path is built at run time from the temporary directory, so
    // no tracked file names a machine.
    let serving = serving(
        &catalog_text(&format!(
            "[models.gemma3.flags]\n\
             stream-events = \"100\"\n\
             stream-gap = \"50\"\n\
             hangup-marker = '{}'\n",
            marker.display()
        )),
        root,
    );

    let mut stream = TcpStream::connect(serving.address()).expect("the router is listening");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read timeout");
    stream
        .write_all(post(COMPLETIONS, BODY).as_bytes())
        .expect("write");

    // Read far enough to know the stream is running, then walk away from it
    // mid-answer, which is what a client that closes its tab does.
    let mut buffer = [0u8; 1024];
    let mut seen = 0usize;
    while seen < 2 {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                seen += String::from_utf8_lossy(&buffer[..read])
                    .matches("data: ")
                    .count();
            }
        }
    }
    assert!(
        seen >= 2,
        "the stream was running before the caller hung up"
    );
    drop(stream);

    assert!(
        appears(&marker),
        "the child's next write failed, which is how a model is told to stop \
         generating. Without this the child produces an answer nobody reads, \
         for as long as the answer takes."
    );
}

/// Waits for the child to record that its write failed.
///
/// Polled rather than slept on: the child notices on its next event, which is
/// one gap away, but a loaded machine can take longer and a fixed sleep would
/// be either flaky or slow.
fn appears(marker: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if marker.exists() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}
