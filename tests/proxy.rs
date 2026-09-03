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

mod support;
use support::{MODEL, ModelsRoot, catalog_text, get, post, request, serving, status};

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
fn a_path_outside_the_dedicated_shape_is_refused() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/v1/chat/completions"));

    assert_eq!(
        status(&reply),
        Some(404),
        "routing by the body's model field is slice 4:\n{reply}"
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

    let reply = request(
        serving.address(),
        &post("/models/gemma3/v1/echo", "{\"model\":\"something-else\"}"),
    );

    assert_eq!(status(&reply), Some(200), "the child answered:\n{reply}");
    assert!(
        reply.contains("Content-Length: 25"),
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
