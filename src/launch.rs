//! Turning one catalog entry into a running server, and stopping it again.
//!
//! What a caller must know to use this correctly is four things.
//!
//! [`Server::start`] blocks. It returns once the child has finished loading
//! and will answer, or once it has failed, so a caller cannot forget to wait
//! and then wonder why the first request was refused.
//!
//! Every failure names the entry it came from. A message saying only that a
//! health check failed sends the reader back to the catalog to guess which of
//! four models it was about.
//!
//! A child binds loopback only. The current router refuses a remote bind
//! deliberately, without a separate security design, and that property is
//! carried over here rather than re-decided.
//!
//! A child is not restarted. Detecting that one exited is [`Child::check`];
//! deciding what to do about it belongs to the slice that has a request in
//! flight to keep waiting, because until then there is nothing to protect.
//!
//! Locating the binary, translating an entry into a command line, choosing a
//! port, polling for readiness, and stopping a process on three operating
//! systems are all implementation and stay inside.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use crate::catalog::Entry;

mod invocation;

/// Why a server could not be located, started, or resolved.
///
/// One string rather than a set of variants: nothing chooses a branch on the
/// kind of failure, it is printed and the command exits. A variant per cause
/// would be a promise to a caller that does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(String);

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Failure {}

/// Whether a child process still exists.
///
/// Distinct from readiness, which is whether it has finished loading. A child
/// is alive long before it is ready, sometimes by minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The process is still there.
    Running,
    /// The process is gone, with the status it left behind.
    Exited(ExitStatus),
}

/// The server binary this router runs children from.
///
/// Located, never bundled: a configured path when there is one, otherwise
/// whatever the operating system finds on the search path.
// The representation arrives with the behaviour, in the commit that spawns a
// child. Until then nothing constructs one, and the field has no reader.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Server {
    binary: PathBuf,
}

/// One running server process, serving one entry on one loopback port.
// As above: constructed by the commit that spawns.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Child {
    id: String,
    address: SocketAddr,
    process: std::process::Child,
}

impl Server {
    /// Finds the server binary, or says which of the two ways it was looked
    /// for.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when a configured path does not exist, or when
    /// nothing named `llama-server` is on the search path.
    pub fn located(_configured: Option<&Path>) -> Result<Self, Failure> {
        todo!("locating the server binary arrives with the spawn")
    }

    /// Starts one entry and returns once it is ready to answer.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] naming the entry when its model file is missing,
    /// when the child exits while loading, or when it does not become ready
    /// inside the entry's startup budget.
    pub fn start(&self, _entry: &Entry, _root: &Path) -> Result<Child, Failure> {
        todo!("spawning and polling arrive with the readiness loop")
    }
}

impl Child {
    /// Where this child answers.
    #[must_use]
    pub fn endpoint(&self) -> SocketAddr {
        todo!("the address is recorded when the child is spawned")
    }

    /// Whether the process is still there.
    pub fn check(&mut self) -> Liveness {
        todo!("liveness arrives with the readiness loop")
    }

    /// Stops the child and reaps it, so no zombie is left behind.
    pub fn stop(&mut self) {
        todo!("stopping arrives with the readiness loop")
    }
}

/// The directory catalog locations resolve against.
///
/// Read from `MAESTRO_MODELS_ROOT`, falling back to `models` under the home
/// directory, which is where the current router already looks. Neither is
/// written into a tracked file.
///
/// # Errors
///
/// Returns a [`Failure`] when neither the variable nor a home directory is
/// set, because there is then nowhere to resolve against.
pub fn models_root() -> Result<PathBuf, Failure> {
    todo!("resolution arrives with the launch command")
}
