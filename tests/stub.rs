//! The stub server, tested on its own.
//!
//! Every supervision test reads this stub's health contract, so a stub that
//! answered wrongly would make those tests agree with each other about
//! nothing. Two properties are worth the file: the readiness transition is
//! observed rather than assumed, and a requested exit really happens with the
//! requested code.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// A port the operating system says is free. The window before the stub binds
/// it is a race this repository accepts and records rather than hiding; a test
/// that lost it would fail loudly on the bind, not silently pass.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("its address")
        .port()
}

/// Kills the child when the test leaves, however it leaves.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}

fn start(arguments: &[&str]) -> Running {
    Running(
        Command::new(env!("CARGO_BIN_EXE_stub-llama-server"))
            .args(arguments)
            .spawn()
            .expect("the stub binary is built by cargo test"),
    )
}

/// The status code from `GET /health`, or `None` while nothing is listening.
fn health(port: u16) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    reply.split_whitespace().nth(1)?.parse().ok()
}

/// Polls until the stub is listening, so the readiness assertions are not
/// racing process startup.
fn wait_for_listener(port: u16) -> u16 {
    for _ in 0..200 {
        if let Some(code) = health(port) {
            return code;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("the stub never listened on port {port}");
}

#[test]
fn health_reports_loading_until_the_readiness_moment_then_ready() {
    let port = free_port();
    let _running = start(&[
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--ready-after",
        "1500",
    ]);

    assert_eq!(
        wait_for_listener(port),
        503,
        "a loading server answers 503, which is what llama-server does"
    );

    for _ in 0..200 {
        if health(port) == Some(200) {
            return;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("the stub never became ready");
}

#[test]
fn unknown_arguments_are_ignored_so_a_real_invocation_drives_the_stub() {
    let port = free_port();
    let _running = start(&[
        "--model",
        "somewhere/a.gguf",
        "--jinja",
        "--ctx-size",
        "4096",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
    ]);

    assert_eq!(
        wait_for_listener(port),
        200,
        "ready immediately, and every flag it does not know stepped over"
    );
}

#[test]
fn a_path_the_stub_does_not_serve_is_not_found() {
    let port = free_port();
    let _running = start(&["--port", &port.to_string()]);
    wait_for_listener(port);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write");
    stream.shutdown(Shutdown::Write).expect("shutdown");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read");

    assert!(
        reply.starts_with("HTTP/1.1 404 "),
        "only /health is served:\n{reply}"
    );
}

#[test]
fn exit_after_exits_with_the_requested_code() {
    let port = free_port();
    let mut running = start(&[
        "--port",
        &port.to_string(),
        "--ready-after",
        "60000",
        "--exit-after",
        "250",
        "--exit-code",
        "9",
    ]);

    let status = running.0.wait().expect("the stub exits on its own");
    assert_eq!(
        status.code(),
        Some(9),
        "the exit code a test asked for, so a crash during loading can be driven"
    );
}

/// Sends one request and records when each chunk of the reply arrived.
///
/// The arrival times are the point: a stub that buffered its own output would
/// deliver every event at once, which would make the router's streaming test
/// agree with a broken relay.
fn arrivals(port: u16, request_line: &str) -> (String, Vec<Duration>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout, so a hang fails rather than blocks");
    write!(
        stream,
        "{request_line} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    stream.shutdown(Shutdown::Write).expect("shutdown");

    let started = Instant::now();
    let mut body = String::new();
    let mut times = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                times.push(started.elapsed());
                body.push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        }
    }
    (body, times)
}

#[test]
fn a_paced_stream_arrives_spread_out_rather_than_at_once() {
    let port = free_port();
    let _running = start(&[
        "--port",
        &port.to_string(),
        "--stream-events",
        "5",
        "--stream-gap",
        "100",
    ]);
    wait_for_listener(port);

    let (body, times) = arrivals(port, "POST /v1/chat/completions");

    assert!(
        body.contains("data: {\"n\":0}") && body.contains("data: {\"n\":4}"),
        "every event arrives:\n{body}"
    );
    let first = *times.first().expect("at least one arrival");
    let last = *times.last().expect("at least one arrival");
    assert!(
        last.saturating_sub(first) >= Duration::from_millis(250),
        "five events paced 100ms apart spread over at least half their \
         production time; arrivals were {times:?}"
    );
}

#[test]
fn die_after_events_truncates_the_stream() {
    let port = free_port();
    let _running = start(&[
        "--port",
        &port.to_string(),
        "--stream-events",
        "10",
        "--stream-gap",
        "20",
        "--die-after-events",
        "2",
    ]);
    wait_for_listener(port);

    let (body, _) = arrivals(port, "POST /v1/chat/completions");

    assert!(
        body.contains("data: {\"n\":1}"),
        "the events before the death arrive:\n{body}"
    );
    assert!(
        !body.contains("data: {\"n\":2}"),
        "and nothing after it does:\n{body}"
    );
}

#[test]
fn echo_reflects_the_request_line_and_every_header() {
    let port = free_port();
    let _running = start(&["--port", &port.to_string()]);
    wait_for_listener(port);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    stream
        .write_all(
            b"GET /v1/echo HTTP/1.1\r\n\
              Host: localhost\r\n\
              X-Written-By: the test\r\n\
              Connection: close\r\n\r\n",
        )
        .expect("write");
    stream.shutdown(Shutdown::Write).expect("shutdown");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read");

    assert!(reply.starts_with("HTTP/1.1 200 "), "echo answers:\n{reply}");
    assert!(
        reply.contains("GET /v1/echo HTTP/1.1"),
        "the request line comes back:\n{reply}"
    );
    assert!(
        reply.contains("X-Written-By: the test"),
        "and every header with it:\n{reply}"
    );
}
