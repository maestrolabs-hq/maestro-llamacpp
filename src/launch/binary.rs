//! Finding the server binary, and the named builds beside it.
//!
//! Separated from starting a child because finding a binary and running one
//! are different jobs, and the module-size gate on `server.rs` said so.
//!
//! The property that matters is that both callers look for exactly the same
//! file. Two spellings of "what a runtime is called" would eventually
//! disagree, and the failure would be a check that passed against a router
//! that could not start -- the worst of both, since the check is what buys the
//! confidence.

use std::path::PathBuf;

/// What the server is called. Located on the search path, never bundled.
pub(super) const BINARY_NAME: &str = "llama-server";

/// What a named runtime is called on disk.
pub(crate) fn runtime_named(runtime: &str) -> String {
    format!("{BINARY_NAME}-{runtime}")
}

/// Where a named runtime resolves to, if anywhere.
pub(crate) fn runtime_binary(runtime: &str) -> Option<PathBuf> {
    on_search_path(&runtime_named(runtime))
}

/// The first match for a name on the search path, with the platform's
/// executable suffix, so the Windows leg finds `llama-server.exe`.
pub(crate) fn on_search_path(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}
