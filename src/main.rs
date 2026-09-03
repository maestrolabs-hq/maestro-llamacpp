//! The `model-router` binary.
//!
//! Slice 1 gives it one thing to do: read a catalog and say whether it is
//! usable. Serving models comes with the later slices, and until then a
//! command that pretended to would be worse than none.
//!
//! Argument handling is hand-written. One subcommand taking one operand does
//! not earn a dependency, and the dependency would have to be justified to
//! the same gates as a real one.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use maestro_llamacpp::catalog::Catalog;

const USAGE: &str = "usage: model-router check <catalog>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, catalog] if command == "check" => check(Path::new(catalog)),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
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
