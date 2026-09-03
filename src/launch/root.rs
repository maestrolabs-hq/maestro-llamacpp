//! Where catalog locations resolve against.
//!
//! Slice 1 decided that every location in a catalog is relative, so that one
//! catalog is correct on every machine. This is the other half of that
//! decision: the root those locations resolve against, read at run time and
//! never written into a tracked file.

use std::path::PathBuf;

use super::Failure;

/// Where the models root is configured. The estate prefix keeps it
/// recognisable beside the other variables a machine carries.
const VARIABLE: &str = "MAESTRO_MODELS_ROOT";

/// The directory catalog locations resolve against.
///
/// The variable when it is set, otherwise `models` under the home directory,
/// which is where the current router already looks.
///
/// # Errors
///
/// Returns a [`Failure`] when neither the variable nor a home directory is
/// set, because there is then nowhere to resolve against.
pub fn models_root() -> Result<PathBuf, Failure> {
    // An empty value is treated as unset. `export MAESTRO_MODELS_ROOT=` is a
    // plausible slip, and honouring it would resolve every model against
    // nothing at all and then blame the catalog for the missing file.
    if let Some(configured) = std::env::var_os(VARIABLE).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(configured));
    }

    home_directory()
        .map(|home| home.join("models"))
        .ok_or_else(|| {
            Failure::Unavailable(format!(
                "no models root: set {VARIABLE}, or run somewhere with a home \
                 directory this router can read"
            ))
        })
}

/// The home directory, from whichever variable the platform keeps it in.
fn home_directory() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable).map(PathBuf::from)
}
