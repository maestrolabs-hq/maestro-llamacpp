//! Routing one dedicated endpoint, through the interface a caller holds.
//!
//! Everything the router does here is real: it binds a port, accepts a
//! connection, parses a head, starts a child, opens a socket to it and copies
//! bytes. The stub stands in for `llama-server` at the same seam slice 2 put
//! it, so the only thing avoided is a multi-gigabyte model and a graphics
//! card.
//!
//! The properties that are about time rather than content live in
//! `streaming.rs`. A failure here means routing; a failure there means the
//! relay.

use std::thread::sleep;
use std::time::{Duration, Instant};

mod support;
use support::{MODEL, ModelsRoot, catalog_text, get, health, post, request, serving, status};

#[test]
fn an_unknown_identifier_is_not_found_and_names_what_the_catalog_carries() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/nowhere/v1/models"));

    assert_eq!(status(&reply), Some(404), "no such entry:\n{reply}");
    assert!(
        reply.contains("nowhere"),
        "the refusal names what was asked for:\n{reply}"
    );
    assert!(
        reply.contains("gemma3"),
        "and what the catalog does carry, so the reader can correct it:\n{reply}"
    );
}

#[test]
fn a_path_that_is_no_shape_the_router_serves_is_refused() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/health"));

    assert_eq!(
        status(&reply),
        Some(404),
        "the router serves two shapes and no others:\n{reply}"
    );
}

/// A second model file, so routing between two entries is observable.
const SECOND_MODEL: &str = "cache/qwen/qwen3-8b.gguf";

/// A catalog carrying two entries, which is the smallest catalog in which
/// "the request reached the right child" can mean anything at all.
fn two_entries() -> String {
    catalog_text(&format!("\n[models.qwen38]\npath = \"{SECOND_MODEL}\"\n"))
}

#[test]
fn the_body_decides_which_child_answers() {
    let serving = serving(&two_entries(), ModelsRoot::with(&[MODEL, SECOND_MODEL]));

    for wanted in ["gemma3", "qwen38"] {
        let body = format!("{{\"model\":\"{wanted}\"}}");
        let reply = request(serving.address(), &post("/v1/echo", &body));

        assert_eq!(status(&reply), Some(200), "a child answered:\n{reply}");
        assert!(
            reply.contains(&format!("alias: {wanted}")),
            "the child started for '{wanted}' is the one that answered, \
             observed at the child rather than assumed at the router:\n{reply}"
        );
        assert!(
            reply.contains(&format!("body: {body}")),
            "the child received the caller's own bytes. The router read this \
             body to learn which model answers, and forwarding what it parsed \
             rather than what it was given would change a request it does not \
             own:\n{reply}"
        );
    }
}

#[test]
fn the_generic_endpoint_strips_no_prefix_from_the_path() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(
        serving.address(),
        &post("/v1/echo", "{\"model\":\"gemma3\"}"),
    );

    assert!(
        reply.contains("POST /v1/echo HTTP/1.1"),
        "the caller's own path is what the child is asked for, because the \
         model was named in the body rather than taken out of the path:\n{reply}"
    );
}

#[test]
fn a_body_naming_no_such_model_is_not_found_and_names_the_catalog() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(
        serving.address(),
        &post("/v1/chat/completions", "{\"model\":\"nowhere\"}"),
    );

    assert_eq!(status(&reply), Some(404), "no such entry:\n{reply}");
    assert!(
        reply.contains("nowhere") && reply.contains("gemma3"),
        "named the same way the dedicated endpoint names it:\n{reply}"
    );
}

#[test]
fn a_body_naming_no_model_at_all_is_a_bad_request() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(
        serving.address(),
        &post("/v1/chat/completions", "{\"messages\":[]}"),
    );

    assert_eq!(
        status(&reply),
        Some(400),
        "the generic endpoint has nothing else to route on:\n{reply}"
    );
    assert!(
        reply.contains("/models/"),
        "and says which endpoint needs no such field:\n{reply}"
    );
}

#[test]
fn the_listing_answers_from_the_catalog_without_starting_anything() {
    // A models root with no files in it: starting any child would fail on the
    // missing model, so a 200 here is proof that none was attempted.
    let serving = serving(&two_entries(), ModelsRoot::with(&[]));

    let reply = request(serving.address(), &get("/v1/models"));

    assert_eq!(
        status(&reply),
        Some(200),
        "the router answers this:\n{reply}"
    );
    assert!(
        reply.contains("gemma3") && reply.contains("qwen38"),
        "every entry the catalog carries:\n{reply}"
    );
    assert!(
        reply.contains("\"object\":\"list\""),
        "in the shape an OpenAI-compatible client expects:\n{reply}"
    );
}

#[test]
fn a_request_reaches_the_child_with_the_prefix_stripped() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));

    assert_eq!(status(&reply), Some(200), "the child answered:\n{reply}");
    assert!(
        reply.contains("GET /v1/echo HTTP/1.1"),
        "the child is asked for the path without the prefix, observed at the \
         child rather than assumed at the router:\n{reply}"
    );
    assert!(
        reply.contains("Connection: close"),
        "and asked to close, so the response ends at end-of-file:\n{reply}"
    );
    assert!(
        !reply.contains("Host: router"),
        "the caller's Host named the router, and the child is not it:\n{reply}"
    );
}

#[test]
fn a_body_is_forwarded_to_the_child_with_its_headers() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let body = "{\"model\":\"something-else\"}";
    let reply = request(serving.address(), &post("/models/gemma3/v1/echo", body));

    assert_eq!(status(&reply), Some(200), "the child answered:\n{reply}");
    assert!(
        reply.contains(&format!("body: {body}")),
        "the body itself reaches the child, byte for byte. This endpoint names \
         its model in the path and never reads the body, so what arrives here \
         is what was copied straight through:\n{reply}"
    );
    assert!(
        reply.contains(&format!("Content-Length: {}", body.len())),
        "the declared length reaches the child unchanged:\n{reply}"
    );
    assert!(
        reply.contains("Content-Type: application/json"),
        "as does every header the router has no opinion about:\n{reply}"
    );
}

#[test]
fn a_chunked_request_body_is_refused_rather_than_mangled() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(
        serving.address(),
        "POST /models/gemma3/v1/chat/completions HTTP/1.1\r\n\
         Host: router\r\n\
         Transfer-Encoding: chunked\r\n\
         Connection: close\r\n\
         \r\n\
         0\r\n\r\n",
    );

    assert_eq!(
        status(&reply),
        Some(501),
        "decoding a chunked body to re-encode it upstream is work with no \
         caller:\n{reply}"
    );
    assert!(
        reply.contains("gemma3"),
        "and the refusal still names the entry it was about:\n{reply}"
    );
}

#[test]
fn an_entry_whose_child_never_becomes_ready_is_a_gateway_timeout() {
    let serving = serving(
        &catalog_text(
            "startup_timeout_seconds = 2\n\
             \n\
             [models.gemma3.flags]\n\
             ready-after = \"600000\"\n",
        ),
        ModelsRoot::with(&[MODEL]),
    );

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));

    assert_eq!(status(&reply), Some(504), "the budget ran out:\n{reply}");
    assert!(
        reply.contains("gemma3"),
        "naming the entry, so the reader is not sent to the catalog to \
         guess:\n{reply}"
    );
}

#[test]
fn an_entry_whose_child_cannot_start_is_a_bad_gateway() {
    // A root with no files in it: the model the entry names is not there, and
    // slice 2 refuses before it spawns anything.
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[]));

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));

    assert_eq!(
        status(&reply),
        Some(502),
        "the child could not be started:\n{reply}"
    );
    assert!(reply.contains("gemma3"), "naming the entry:\n{reply}");
}

#[test]
fn stopping_a_router_ends_the_children_it_started() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));
    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));

    // The stub reflects the Host it was given, which the rewrite set to the
    // child's own address. That is how this test learns a port the router
    // never told anyone about.
    let endpoint = reply
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .expect("the echo carries the address the child was reached on")
        .to_owned();
    assert_eq!(
        health(endpoint.as_str()),
        Some(200),
        "the child answers while the router holds it"
    );

    drop(serving);

    // Polled rather than slept on: killing a process is not instantaneous and
    // a fixed wait would be either flaky or slow.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if health(endpoint.as_str()).is_none() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!(
        "the child at {endpoint} outlived the router that started it. A child \
         is a separate process, and nothing in the operating system ends it \
         when this one goes."
    );
}
