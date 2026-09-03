//! The `model-router` binary.
//!
//! Two things so far: read a catalog and say whether it is usable, and take
//! one entry from it as far as a running server that answers. Routing comes
//! with the later slices, and until then a command that pretended to would be
//! worse than none.
//!
//! Argument handling is hand-written. Two subcommands taking two operands do
//! not earn a dependency, and the dependency would have to be justified to
//! the same gates as a real one.

use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use maestro_llamacpp::admission::Budget;
use maestro_llamacpp::catalog::Catalog;
use maestro_llamacpp::launch::{Server, models_root};
use maestro_llamacpp::proxy::Router;

const USAGE: &str = "usage: model-router check <catalog>\n       \
                     model-router launch <catalog> <model>\n       \
                     model-router serve <catalog> [address]";

/// The one public port the design names.
const DEFAULT_ADDRESS: &str = "127.0.0.1:8080";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, catalog] if command == "check" => check(Path::new(catalog)),
        [command, catalog, id] if command == "launch" => match launch(Path::new(catalog), id) {
            Ok(()) => ExitCode::SUCCESS,
            Err(complaint) => {
                eprintln!("{complaint}");
                ExitCode::FAILURE
            }
        },
        [command, catalog] if command == "serve" => report(serve(Path::new(catalog), None)),
        [command, catalog, address] if command == "serve" => {
            report(serve(Path::new(catalog), Some(address)))
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Whatever a command had to say when it could not do its work.
fn report(outcome: Result<(), String>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(complaint) => {
            eprintln!("{complaint}");
            ExitCode::FAILURE
        }
    }
}

/// Serves every entry in the catalog, on its own endpoint and on the shared
/// one.
///
/// Long-running by design, unlike `launch`: now there is something to serve.
/// It returns only when the process is ended -- and because that end is
/// usually a signal, `Router::stop` is never reached and the children outlive
/// it. What that costs, and how to find them afterwards, is in `README.md`
/// under what eviction never does. A handler needs a dependency and a Windows
/// job object, so it is a change of its own rather than a line here.
fn serve(catalog: &Path, address: Option<&str>) -> Result<(), String> {
    let parsed = read(catalog)?;
    let root = models_root().map_err(|failure| failure.to_string())?;
    let server = Server::located(None).map_err(|failure| failure.to_string())?;

    let wanted = address.unwrap_or(DEFAULT_ADDRESS);
    let wanted: SocketAddr = wanted
        .parse()
        .map_err(|error| format!("'{wanted}' is not an address to bind: {error}"))?;

    let budget = Budget::configured().map_err(|failure| failure.to_string())?;
    // Said at startup rather than left to be discovered: whether anything is
    // ever evicted is the difference between a router that swaps models and
    // one that fills memory until a load fails, and an operator who mistyped
    // the variable would otherwise find out only under load.
    let budget_line = match budget.limit_mib() {
        Some(limit) => format!("memory budget: {limit} MiB, so models are unloaded to make room"),
        None => "memory budget: none set, so nothing is ever unloaded \
             (set MAESTRO_MEMORY_BUDGET_MIB)"
            .to_owned(),
    };

    let router = Router::bind(wanted, parsed, root, server, budget).map_err(|f| f.to_string())?;
    let bound = router.address();
    println!("serving on http://{bound}");
    println!("  http://{bound}/models/<model>/v1/chat/completions");
    println!("  http://{bound}/v1/chat/completions   (routed by the body's model)");
    println!("{budget_line}");
    println!("a streamed reply is passed through as it arrives");

    router.serve();
    Ok(())
}

/// One usable catalog, or why it is not.
fn read(catalog: &Path) -> Result<Catalog, String> {
    let text = fs::read_to_string(catalog)
        .map_err(|error| format!("cannot read {}: {error}", catalog.display()))?;
    Catalog::parse(&text)
        .map_err(|report| format!("{} is not usable:\n{report}", catalog.display()))
}

/// Starts one entry, proves it answers, and stops it again.
///
/// Deliberately not long-running: proving that a child starts, answers and
/// ends is the whole of what this command claims, and it doubles as the manual
/// check against a real `llama-server`. Serving is `serve`'s job, and what
/// that does about signals is recorded there.
fn launch(catalog: &Path, id: &str) -> Result<(), String> {
    let parsed = read(catalog)?;
    let entry = parsed
        .entry(id)
        .ok_or_else(|| format!("{} carries no model called '{id}'", catalog.display()))?;

    let root = models_root().map_err(|failure| failure.to_string())?;
    let server = Server::located(None).map_err(|failure| failure.to_string())?;

    let started = Instant::now();
    let mut child = server
        .start(entry, &root)
        .map_err(|failure| failure.to_string())?;
    println!(
        "{id} is ready at http://{} after {:.1} seconds",
        child.endpoint(),
        started.elapsed().as_secs_f64()
    );

    child.stop();
    println!("{id} stopped");
    Ok(())
}

/// Reports whether a catalog is usable, and why not when it is not.
///
/// Every problem is printed, not the first, so one run of this command covers
/// one round of edits to the file.
fn check(catalog: &Path) -> ExitCode {
    let text = match fs::read_to_string(catalog) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {}: {error}", catalog.display());
            return ExitCode::FAILURE;
        }
    };

    match Catalog::parse(&text) {
        Ok(parsed) => {
            println!(
                "{} is valid: {} models",
                catalog.display(),
                parsed.entries.len()
            );
            ExitCode::SUCCESS
        }
        Err(report) => {
            eprintln!("{} is not usable:", catalog.display());
            for problem in report.problems() {
                eprintln!("  {problem}");
            }
            ExitCode::FAILURE
        }
    }
}
