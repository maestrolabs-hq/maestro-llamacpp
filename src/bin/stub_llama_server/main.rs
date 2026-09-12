//! A stand-in for `llama-server`, so continuous integration can supervise a
//! real process and relay a real stream without a real model.
//!
//! A real server needs a multi-gigabyte model file and a graphics card.
//! Neither exists in continuous integration, so a test that requires one is a
//! test that never runs. This binary speaks the parts of the server contract
//! the router reads, and the supervision and relay paths around it are
//! identical either way: the router picks a port, builds a command line,
//! spawns, polls, reads a request head, and copies bytes.
//!
//! This file decides what the stub was asked to do; [`reply`] decides what one
//! connection is answered with.
//!
//! It is never released. The shared release workflow takes the name of one
//! binary, and that binary is `model-router`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

mod reply;
use reply::Pacing;

/// What the stub was asked to do. Every other argument is ignored on purpose:
/// the same invocation that drives `llama-server` has to drive this.
struct Options {
    host: String,
    port: u16,
    ready_after: Duration,
    exit_after: Option<Duration>,
    exit_code: u8,
    /// Which entry this stub was started as, taken from `--alias`.
    ///
    /// Every child is this same binary, so a test that asserts a request
    /// reached the right one needs the reply to say which one answered.
    alias: String,
    pacing: Pacing,
    /// Names a marker file. On the run that finds it absent, this stub exits
    /// before binding at all; on a later run that finds it present, it
    /// behaves normally. Stands in for `launch::server`'s `free_port` losing
    /// its own release-then-rebind race: the port this stub was given is
    /// never touched, so nothing outside the process can ever connect to it.
    never_bind_marker: Option<PathBuf>,
    /// The same failure, on every run rather than only the first, for
    /// proving a retry is bounded rather than unbounded.
    never_bind: bool,
}

fn main() -> ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(complaint) => {
            eprintln!("stub-llama-server: {complaint}");
            return ExitCode::FAILURE;
        }
    };

    // Checked before the bind that would otherwise make this run
    // indistinguishable from a real one: a marker not yet there means this is
    // the first run, and it exits having touched neither the port nor the
    // socket, so nothing outside this process can tell it apart from having
    // lost `free_port`'s race. Present means a prior run already paid that
    // cost, so this one behaves as asked.
    if let Some(marker) = &options.never_bind_marker {
        if !marker.exists() {
            if let Err(error) = std::fs::write(marker, b"") {
                eprintln!("stub-llama-server: cannot write never-bind marker: {error}");
            }
            return ExitCode::FAILURE;
        }
    } else if options.never_bind {
        return ExitCode::FAILURE;
    }

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

    serve(&listener, &options)
}

/// Accepts until the process ends, one thread per connection.
///
/// Threaded because a paced stream holds its connection for as long as it
/// runs. Answering in the accept loop would make a second caller wait out the
/// first one's stream, which is a property no real server has and no test
/// should have to work around.
fn serve(listener: &TcpListener, options: &Options) -> ExitCode {
    let started = Instant::now();
    for stream in listener.incoming().flatten() {
        let ready = started.elapsed() >= options.ready_after;
        let pacing = Pacing {
            events: options.pacing.events,
            gap: options.pacing.gap,
            die_after: options.pacing.die_after,
            hangup_marker: options.pacing.hangup_marker.clone(),
        };
        let alias = options.alias.clone();
        thread::spawn(move || {
            // A failed reply is a client that hung up. Nothing here is
            // durable, so the next connection is the only thing that matters.
            drop(reply::answer(stream, ready, &pacing, &alias));
        });
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
    let mut alias = String::new();
    let mut events = 3usize;
    let mut gap = Duration::ZERO;
    let mut die_after = None;
    let mut hangup_marker = None;
    let mut never_bind_marker = None;
    let mut never_bind = false;

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
            "--alias" => alias = value()?,
            "--stream-events" => events = number(&value()?, "--stream-events")?,
            "--stream-gap" => gap = Duration::from_millis(number(&value()?, "--stream-gap")?),
            "--die-after-events" => die_after = Some(number(&value()?, "--die-after-events")?),
            "--hangup-marker" => hangup_marker = Some(PathBuf::from(value()?)),
            "--never-bind-marker" => never_bind_marker = Some(PathBuf::from(value()?)),
            "--never-bind" => never_bind = true,
            _ => {}
        }
    }
    Ok(Options {
        host,
        port,
        ready_after,
        exit_after,
        exit_code,
        alias,
        pacing: Pacing {
            events,
            gap,
            die_after,
            hangup_marker,
        },
        never_bind_marker,
        never_bind,
    })
}

fn number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes a number, not '{value}'"))
}
