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
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use maestro_llamacpp::admission::Budget;
use maestro_llamacpp::catalog::Catalog;
use maestro_llamacpp::idle::{IdleWindow, Limits};
use maestro_llamacpp::launch::{Server, models_root};
use maestro_llamacpp::proxy::Router;
use maestro_llamacpp::queue::Wait;
use maestro_llamacpp::{bench, startup};

const USAGE: &str = "usage: model-router check <catalog>\n       \
                     model-router bench <catalog> [model]\n       \
                     model-router launch <catalog> <model>\n       \
                     model-router serve <catalog> [address]";

/// The one public port the design names.
const DEFAULT_ADDRESS: &str = "127.0.0.1:8080";

mod check;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, catalog] if command == "check" => check::check(Path::new(catalog)),
        // Whole catalog or one entry. `check` reads the files and reasons; this
        // starts each entry and reads the card, which is the only way to tell
        // an estimate that is merely arithmetic from one that is true.
        [command, catalog] if command == "bench" => {
            report(bench::command(Path::new(catalog), None))
        }
        [command, catalog, id] if command == "bench" => {
            report(bench::command(Path::new(catalog), Some(id)))
        }
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
/// It runs until the process is asked to end, and that end is a signal, so a
/// signal is what reaches `Router::stop`: see [`stop_on_termination`]. What
/// no handler can cover, and how to find the children afterwards, is in
/// `README.md` under what eviction never does.
fn serve(catalog: &Path, address: Option<&str>) -> Result<(), String> {
    let parsed = read(catalog)?;
    let root = models_root().map_err(|failure| failure.to_string())?;
    let server = Server::located(None).map_err(|failure| failure.to_string())?;

    let wanted = address.unwrap_or(DEFAULT_ADDRESS);
    let wanted: SocketAddr = wanted
        .parse()
        .map_err(|error| format!("'{wanted}' is not an address to bind: {error}"))?;

    let budget = Budget::configured().map_err(|failure| failure.to_string())?;
    let idle_window = IdleWindow::configured().map_err(|failure| failure.to_string())?;
    // Composed before the catalog is handed over, because binding takes it.
    let limit_mib = budget.limit_mib();
    let budget_source = budget.source().to_owned();
    let idle_seconds = idle_window.seconds();
    let reserved_mib = parsed.resident_reservation_mib();

    let wait = Wait::configured().map_err(|failure| failure.to_string())?;
    let waiting = wait.waits();
    let limits = Limits::new(budget, idle_window, wait);
    let router = Router::bind(wanted, parsed, root, server, limits).map_err(|f| f.to_string())?;
    let router = Arc::new(router);
    let bound = router.address();
    println!("serving on http://{bound}");
    println!("  http://{bound}/models/<model>/v1/chat/completions");
    println!("  http://{bound}/v1/chat/completions   (routed by the body's model)");
    println!("{}", startup::budget(limit_mib, &budget_source));
    println!("{}", startup::admission_wait(waiting));
    if reserved_mib > 0 {
        println!("{}", startup::reservation(limit_mib, reserved_mib));
    }
    println!("{}", startup::idle_window(idle_seconds));
    println!("a streamed reply is passed through as it arrives");

    // Registered after the bind and before anything can start a child: a
    // signal before this point ends a router that has nothing to stop.
    stop_on_termination(Arc::clone(&router))?;
    router.serve();
    Ok(())
}

/// Ends the children when the process is asked to end, then exits.
///
/// A child is a separate process that nothing in the operating system ties to
/// this one, and `Router::serve` never returns. Without this, `SIGTERM` --
/// what `systemctl stop`, `kill` and a container stop all send -- ended the
/// router and left every server it had started running with its memory. The
/// handler covers the ways a process is asked to end: `SIGTERM`, `SIGINT` and
/// `SIGHUP` on Unix; Ctrl-C, Ctrl-Break and a closing console on Windows. What
/// it cannot cover is being killed outright -- `SIGKILL`, `taskkill /F` --
/// which no process is allowed to handle.
///
/// The stop runs on a thread of its own so that the handler returns at once
/// and a second signal is acted on rather than queued: it ends the process
/// immediately, with a failing status because the children were not waited
/// for. Stopping inside the handler would make the second signal wait for the
/// first, and a stop held up by a child that will not die would then be a
/// router nothing but `SIGKILL` can end -- the state this exists to remove.
fn stop_on_termination(router: Arc<Router>) -> Result<(), String> {
    let mut signalled = false;
    ctrlc::set_handler(move || {
        if signalled {
            eprintln!("signalled again: exiting without waiting for the children");
            std::process::exit(1);
        }
        signalled = true;
        let stopping = Arc::clone(&router);
        thread::spawn(move || {
            let held = stopping.loaded().len();
            stopping.stop();
            println!("stopping: ended {}", children(held));
            std::process::exit(0);
        });
    })
    .map_err(|error| format!("cannot handle termination signals: {error}"))
}

/// `1 child`, `2 children`, so the stop line reads as a sentence.
fn children(count: usize) -> String {
    if count == 1 {
        "1 child".to_owned()
    } else {
        format!("{count} children")
    }
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
