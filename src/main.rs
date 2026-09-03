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
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use maestro_llamacpp::catalog::Catalog;
use maestro_llamacpp::launch::{Server, models_root};

const USAGE: &str = "usage: model-router check <catalog>\n       \
                     model-router launch <catalog> <model>";

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
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Starts one entry, proves it answers, and stops it again.
///
/// Deliberately not long-running. Staying up until interrupted needs a signal
/// handler, and therefore a dependency, for no gain in a slice with nothing to
/// serve. Launching, proving readiness and stopping is the whole of what this
/// slice can honestly claim, and it doubles as the manual check against a real
/// `llama-server`.
fn launch(catalog: &Path, id: &str) -> Result<(), String> {
    let text = fs::read_to_string(catalog)
        .map_err(|error| format!("cannot read {}: {error}", catalog.display()))?;
    let parsed = Catalog::parse(&text)
        .map_err(|report| format!("{} is not usable:\n{report}", catalog.display()))?;
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
