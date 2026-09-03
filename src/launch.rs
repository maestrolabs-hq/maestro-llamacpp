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
use std::process::ExitStatus;

mod invocation;
mod probe;
mod root;
mod server;

pub use root::models_root;
pub use server::Server;

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

/// One running server process, serving one entry on one loopback port.
#[derive(Debug)]
pub struct Child {
    pub(super) id: String,
    pub(super) address: SocketAddr,
    pub(super) process: std::process::Child,
}

impl Child {
    /// Where this child answers.
    #[must_use]
    pub fn endpoint(&self) -> SocketAddr {
        self.address
    }

    /// Whether the process is still there.
    pub fn check(&mut self) -> Liveness {
        match self.process.try_wait() {
            Ok(Some(status)) => Liveness::Exited(status),
            // A status that cannot be read is reported as still running. The
            // readiness loop is bounded by the entry's budget rather than by
            // this answer, so guessing at death here would only turn an
            // unreadable status into a wrong one.
            Ok(None) | Err(_) => Liveness::Running,
        }
    }

    /// Stops the child and reaps it, so no zombie is left behind.
    ///
    /// Abrupt: `SIGKILL` on the Unix platforms and `TerminateProcess` on
    /// Windows. Asking politely first needs a platform dependency the standard
    /// library does not offer, and `llama-server` holds no durable state, so
    /// an abrupt stop loses only responses in flight -- of which there are
    /// none until something can make a request.
    ///
    /// Killing a child that has already exited fails, and that failure is
    /// dropped: it means the work this method exists to do is already done.
    pub fn stop(&mut self) {
        drop(self.process.kill());
        drop(self.process.wait());
    }
}

/// A child never outlives the value that represents it.
///
/// Without this, a caller that drops a `Child` on an error path leaves a
/// server holding a port with nothing left in the program that knows about
/// it. This is the ordinary case and it is avoidable; a hard kill of the
/// router is not, and stays in the risks.
impl Drop for Child {
    fn drop(&mut self) {
        self.stop();
    }
}
