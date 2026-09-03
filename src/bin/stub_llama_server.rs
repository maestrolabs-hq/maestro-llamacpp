//! A stand-in for `llama-server`, so continuous integration can supervise a
//! real process without a real model.
//!
//! A real server needs a multi-gigabyte model file and a graphics card.
//! Neither exists in continuous integration, so a test that requires one is a
//! test that never runs. This binary speaks the part of the health contract
//! the router reads -- `/health`, 503 while loading and 200 once ready -- and
//! nothing else. The supervision path around it is identical either way: the
//! router picks a port, builds a command line, spawns, polls, and kills.
//!
//! It is never released. The shared release workflow takes the name of one
//! binary, and that binary is `model-router`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

/// What the stub was asked to do. Every other argument is ignored on purpose:
/// the same invocation that drives `llama-server` has to drive this.
struct Options {
    host: String,
    port: u16,
    ready_after: Duration,
    exit_after: Option<Duration>,
    exit_code: u8,
}

fn main() -> ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(complaint) => {
            eprintln!("stub-llama-server: {complaint}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match TcpListener::bind((options.host.as_str(), options.port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "stub-llama-server: cannot bind {}:{}: {error}",
                options.host, options.port
            );
            return ExitCode::FAILURE;
        }
    };

    // Exiting is a timer rather than a branch in the accept loop: a crash
    // during loading has to be observable while the socket is still being
    // served, which is exactly what a test drives with this.
    if let Some(after) = options.exit_after {
        let code = options.exit_code;
        thread::spawn(move || {
            thread::sleep(after);
            std::process::exit(i32::from(code));
        });
    }

    let started = Instant::now();
    for stream in listener.incoming().flatten() {
        let ready = started.elapsed() >= options.ready_after;
        // A failed reply is a client that hung up. Nothing here is durable,
        // so the next connection is the only thing that matters.
        drop(answer(stream, ready));
    }
    ExitCode::SUCCESS
}

/// Reads the known arguments and steps over the rest.
///
/// A value is only consumed for a flag this stub knows, so a bare flag it
/// does not know costs nothing and a valued one has its value skipped as an
/// unknown flag in turn.
fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut host = "127.0.0.1".to_owned();
    let mut port = 0u16;
    let mut ready_after = Duration::ZERO;
    let mut exit_after = None;
    let mut exit_code = 0u8;

    let mut args = args;
    while let Some(argument) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--host" => host = value()?,
            "--port" => port = number(&value()?, "--port")?,
            "--ready-after" => {
                ready_after = Duration::from_millis(number(&value()?, "--ready-after")?);
            }
            "--exit-after" => {
                exit_after = Some(Duration::from_millis(number(&value()?, "--exit-after")?));
            }
            "--exit-code" => exit_code = number(&value()?, "--exit-code")?,
            _ => {}
        }
    }
    Ok(Options {
        host,
        port,
        ready_after,
        exit_after,
        exit_code,
    })
}

fn number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes a number, not '{value}'"))
}

/// Answers one request. `/health` carries the readiness contract; everything
/// else is a path this stub does not serve.
fn answer(mut stream: TcpStream, ready: bool) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;

    let health = line.starts_with("GET /health ");
    let (status, body) = match (health, ready) {
        (true, true) => ("200 OK", "{\"status\":\"ok\"}"),
        (true, false) => ("503 Service Unavailable", "{\"status\":\"loading\"}"),
        (false, _) => ("404 Not Found", "{\"error\":\"not found\"}"),
    };

    write!(
        stream,
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )?;
    stream.flush()
}
