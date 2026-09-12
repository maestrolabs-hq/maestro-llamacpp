//! The `check` command: reading a catalog and saying whether it is usable.
//!
//! Split from `main.rs` along the seam the module-size gate exposed: that file
//! is the argument table, and this is one command. Reading a catalog against
//! its root is more than parsing -- an estimate nobody declared is derived
//! from the files, an estimate that disagrees with them is reported, and every
//! model file the catalog does not name becomes an entry of its own -- so it
//! carries more explanation than an argument match should have to hold.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use maestro_llamacpp::catalog::{Catalog, Report};
use maestro_llamacpp::launch::models_root;

/// Reports whether a catalog is usable, and why not when it is not.
///
/// Every problem is printed, not the first, so one run of this command covers
/// one round of edits to the file.
pub fn check(catalog: &Path) -> ExitCode {
    let text = match fs::read_to_string(catalog) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {}: {error}", catalog.display());
            return ExitCode::FAILURE;
        }
    };

    // Read against the root when there is one, so an estimate nobody declared
    // is derived from the files and an estimate that disagrees with them is
    // said out loud. Text-only parsing is the fallback rather than the default:
    // it is what a machine holding none of the models can do, and all it can
    // check is shape.
    let reading = models_root()
        .ok()
        .filter(|root| root.is_dir())
        .map(|root| Catalog::read(&text, &root));

    match reading {
        Some(Ok(reading)) => {
            println!(
                "{} is valid: {} models, {} declared and {} found under the root",
                catalog.display(),
                reading.catalog.entries.len(),
                reading.declared(),
                reading.discovered()
            );
            // Notes are not failures. An estimate that disagrees with its
            // files is worth hearing and is not a reason to refuse the
            // catalog: the declared value is kept, and the operator decides.
            for note in &reading.notes {
                println!("  {note}");
            }
            ExitCode::SUCCESS
        }
        None => match Catalog::parse(&text) {
            Ok(parsed) => {
                println!(
                    "{} is valid: {} models (shape only -- no models root on \
                     this machine, so nothing was checked against its files)",
                    catalog.display(),
                    parsed.entries.len()
                );
                ExitCode::SUCCESS
            }
            Err(report) => complain(catalog, &report),
        },
        Some(Err(report)) => complain(catalog, &report),
    }
}

/// Every problem a catalog has, each by the entry and field it came from.
///
/// One run covers one round of edits, which is why every fault is listed
/// rather than only the first.
fn complain(catalog: &Path, report: &Report) -> ExitCode {
    eprintln!("{} is not usable:", catalog.display());
    for problem in report.problems() {
        eprintln!("  {problem}");
    }
    ExitCode::FAILURE
}
