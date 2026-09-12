//! What the router says in its own voice, and how a client is meant to read
//! it.
//!
//! Every reply here is authored by the router rather than relayed from a
//! child: a refusal, a listing, a preflight. `proxy.rs` proves that requests
//! reach the right child; this proves that what the router answers itself is
//! something a client library can act on without reading prose -- a status
//! that means what the specification says, a JSON envelope with a stable
//! code, and a `Retry-After` exactly when retrying can change the answer.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

mod support;
use support::{MODEL, ModelsRoot, catalog_text, get, post, request, serving, status};

/// The body of a reply, parsed as the JSON it claims to be.
fn body_of(reply: &str) -> serde_json::Value {
    let (_, body) = reply
        .split_once("\r\n\r\n")
        .expect("a reply has a head and a body");
    serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("the body is not JSON ({error}); a client library cannot read it:\n{reply}")
    })
}

/// One header's value, however its name was capitalised.
fn header(reply: &str, name: &str) -> Option<String> {
    reply
        .split("\r\n\r\n")
        .next()?
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_owned())
}

#[test]
fn a_refusal_is_a_json_error_envelope_with_a_stable_code() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/nowhere/v1/models"));

    assert_eq!(status(&reply), Some(404), "no such entry:\n{reply}");
    assert_eq!(
        header(&reply, "content-type").as_deref(),
        Some("application/json"),
        "a client library parses the body only when the reply says it is \
         JSON:\n{reply}"
    );
    let error = body_of(&reply);
    assert_eq!(
        error["error"]["code"], "model_not_found",
        "the cause has a name a program can switch on, so no client has to \
         match prose:\n{reply}"
    );
    assert_eq!(
        error["error"]["type"], "invalid_request_error",
        "and a family, which is what an OpenAI-compatible client reads \
         first:\n{reply}"
    );
    let message = error["error"]["message"]
        .as_str()
        .expect("the message is text");
    assert!(
        message.contains("nowhere") && message.contains("gemma3"),
        "the prose is still there for the reader, naming what was asked and \
         what exists: {message}"
    );
}

/// One contract across every refusal: the status the specification gives the
/// cause, and a code that names the cause and nothing else.
///
/// Cases rather than one test per refusal, because the rule is one rule and
/// only the request differs. Each case says which refusal it drives, so a
/// failure still names the cause that lost its code.
#[test]
fn every_refusal_carries_the_status_and_code_of_its_cause() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));
    let oversized = format!(
        "POST /v1/chat/completions HTTP/1.1\r\n\
         Host: router\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        64 * 1024 * 1024
    );
    let cases: [(&str, String, u16, &str); 7] = [
        (
            "a path that is no shape the router serves",
            get("/health"),
            404,
            "path_not_found",
        ),
        (
            "a body naming no model at all",
            post("/v1/chat/completions", "{\"messages\":[]}"),
            400,
            "model_missing",
        ),
        (
            "a body that is not JSON",
            post("/v1/chat/completions", "not json"),
            400,
            "body_not_json",
        ),
        (
            "a generic request with no declared body",
            get("/v1/models/gemma3"),
            411,
            "content_length_required",
        ),
        (
            "a body larger than the router will read",
            oversized,
            413,
            "body_too_large",
        ),
        (
            "a length that will not parse",
            "POST /models/gemma3/v1/echo HTTP/1.1\r\n\
             Host: router\r\n\
             Content-Length: abc\r\n\
             Connection: close\r\n\
             \r\n"
                .to_owned(),
            400,
            "malformed_content_length",
        ),
        (
            "a chunked request body",
            "POST /models/gemma3/v1/chat/completions HTTP/1.1\r\n\
             Host: router\r\n\
             Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\
             \r\n\
             0\r\n\r\n"
                .to_owned(),
            501,
            "chunked_body_not_implemented",
        ),
    ];

    for (cause, raw, expected_status, expected_code) in cases {
        let reply = request(serving.address(), &raw);
        assert_eq!(
            status(&reply),
            Some(expected_status),
            "{cause} has its own status:\n{reply}"
        );
        assert_eq!(
            body_of(&reply)["error"]["code"],
            expected_code,
            "{cause} names its cause:\n{reply}"
        );
    }
}

#[test]
fn a_request_line_with_no_method_is_a_bad_request_rather_than_not_found() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    // A request line of only whitespace, and one of only a method. Neither
    // names a path the router could fail to find, so "not found" would blame
    // the wrong thing.
    for raw in ["   \r\n\r\n", "GET\r\n\r\n"] {
        let reply = request(serving.address(), raw);

        assert_eq!(
            status(&reply),
            Some(400),
            "a request the router cannot read is the client's fault, and 404 \
             says the opposite:\n{reply}"
        );
        assert_eq!(
            body_of(&reply)["error"]["code"],
            "malformed_request",
            "named as what it is:\n{reply}"
        );
    }
}

#[test]
fn an_oversized_head_has_the_status_the_specification_gives_it() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let padding = "x".repeat(70 * 1024);
    let reply = request(
        serving.address(),
        &format!("GET /v1/models HTTP/1.1\r\nHost: router\r\nX-Big: {padding}\r\n\r\n"),
    );

    assert_eq!(
        status(&reply),
        Some(431),
        "a head too large to read has a status of its own, and a client that \
         sees 400 shrinks its body rather than its headers:\n{reply}"
    );
    assert_eq!(
        body_of(&reply)["error"]["code"],
        "request_head_too_large",
        "named as what it is:\n{reply}"
    );
}

#[test]
fn a_router_authored_reply_is_readable_by_a_browser_client() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/v1/models"));

    assert_eq!(
        header(&reply, "access-control-allow-origin").as_deref(),
        Some("*"),
        "the router binds loopback only, so what it authors is open to a \
         page on the same machine; without this a preflight passes and the \
         answer it cleared is then refused:\n{reply}"
    );
}

/// Sends a request and reads the reply with a deadline of its own, for the
/// cases where the point is that an answer arrives at all.
fn request_within(address: std::net::SocketAddr, raw: &str, deadline: Duration) -> String {
    let mut stream = TcpStream::connect(address).expect("the router is listening");
    stream
        .set_read_timeout(Some(deadline))
        .expect("a read timeout");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut reply = String::new();
    drop(stream.read_to_string(&mut reply));
    reply
}

#[test]
fn a_preflight_is_answered_by_the_router_and_starts_nothing() {
    // A models root with no files in it: starting any child would fail on the
    // missing model, so a success here is proof that none was attempted.
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[]));

    for (path, asked_with) in [
        ("/models/gemma3/v1/chat/completions", "POST"),
        ("/v1/chat/completions", "POST"),
        ("/v1/models", "GET"),
    ] {
        let reply = request_within(
            serving.address(),
            &format!(
                "OPTIONS {path} HTTP/1.1\r\n\
                 Host: router\r\n\
                 Origin: http://localhost:3000\r\n\
                 Access-Control-Request-Method: POST\r\n\
                 Connection: close\r\n\
                 \r\n"
            ),
            Duration::from_secs(10),
        );

        assert_eq!(
            status(&reply),
            Some(204),
            "a preflight asks what is allowed, and the router knows without \
             asking a child ({path}):\n{reply}"
        );
        let methods = header(&reply, "access-control-allow-methods").unwrap_or_default();
        assert!(
            methods.contains(asked_with) && methods.contains("OPTIONS"),
            "and says which methods a browser may then send ({path}):\n{reply}"
        );
        assert_eq!(
            header(&reply, "access-control-allow-origin").as_deref(),
            Some("*"),
            "open to any origin, because only this machine can reach the \
             port ({path}):\n{reply}"
        );
        assert!(
            header(&reply, "allow").is_some_and(|allow| allow.contains("OPTIONS")),
            "and the plain HTTP answer beside the browser one ({path}):\n{reply}"
        );
    }
    assert!(
        serving.loaded().is_empty(),
        "no preflight started a child: {:?}",
        serving.loaded()
    );
}

#[test]
fn a_method_that_could_not_be_an_inference_is_refused_without_starting_a_child() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[]));

    for method in ["DELETE", "PUT", "PATCH", "HEAD"] {
        let reply = request(
            serving.address(),
            &format!(
                "{method} /models/gemma3/v1/chat/completions HTTP/1.1\r\n\
                 Host: router\r\nConnection: close\r\n\r\n"
            ),
        );

        assert_eq!(
            status(&reply),
            Some(405),
            "{method} is no way to ask a model anything, so it earns a \
             refusal rather than a load:\n{reply}"
        );
        assert!(
            header(&reply, "allow").is_some_and(|allow| allow.contains("POST")),
            "and the refusal says what would have been accepted:\n{reply}"
        );
        assert_eq!(
            body_of(&reply)["error"]["code"],
            "method_not_allowed",
            "named as what it is:\n{reply}"
        );
    }
    assert!(
        serving.loaded().is_empty(),
        "a stray probe under a model prefix loaded it: {:?}",
        serving.loaded()
    );
}

#[test]
fn a_head_request_to_the_listing_carries_its_headers_and_no_body() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[]));

    let reply = request(
        serving.address(),
        "HEAD /v1/models HTTP/1.1\r\nHost: router\r\nConnection: close\r\n\r\n",
    );

    assert_eq!(status(&reply), Some(200), "the listing exists:\n{reply}");
    let (head, body) = reply
        .split_once("\r\n\r\n")
        .expect("a reply has a head and a body");
    assert!(
        body.is_empty(),
        "HEAD asks for the headers of the answer and not the answer, and a \
         client that gets both reads the body as the start of the next \
         reply:\n{reply}"
    );
    let declared: usize = header(head, "content-length")
        .and_then(|value| value.parse().ok())
        .expect("the length the GET would carry");
    let full = request(serving.address(), &get("/v1/models"));
    let (_, full_body) = full.split_once("\r\n\r\n").expect("a head and a body");
    assert_eq!(
        declared,
        full_body.len(),
        "the declared length is the GET's, so a client can size a buffer \
         from it:\n{reply}"
    );
}

#[test]
fn the_listing_is_still_the_listing_with_a_query_string() {
    let serving = serving(&catalog_text(""), ModelsRoot::with(&[]));

    let reply = request(serving.address(), &get("/v1/models?limit=100"));

    assert_eq!(
        status(&reply),
        Some(200),
        "a client library that pages its model list adds a query, and the \
         listing is the same listing:\n{reply}"
    );
    assert!(
        reply.contains("\"object\":\"list\"") && reply.contains("gemma3"),
        "answered from the catalog:\n{reply}"
    );
}
