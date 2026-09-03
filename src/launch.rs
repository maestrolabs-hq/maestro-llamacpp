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
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::catalog::Entry;

mod invocation;

/// What the server is called. Located on the search path, never bundled.
const BINARY_NAME: &str = "llama-server";

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
    pub fn located(configured: Option<&Path>) -> Result<Self, Failure> {
        if let Some(path) = configured {
            return if path.is_file() {
                Ok(Self {
                    binary: path.to_path_buf(),
                })
            } else {
                Err(Failure(format!(
                    "the configured server binary is not there: '{}'",
                    path.display()
                )))
            };
        }

        on_search_path(BINARY_NAME)
            .map(|binary| Self { binary })
            .ok_or_else(|| {
                Failure(format!(
                    "no server binary was configured, and no '{BINARY_NAME}' \
                     was found on the search path"
                ))
            })
    }

    /// Starts one entry and returns once it is ready to answer.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] naming the entry when its model file is missing,
    /// when the child exits while loading, or when it does not become ready
    /// inside the entry's startup budget.
    pub fn start(&self, entry: &Entry, root: &Path) -> Result<Child, Failure> {
        // Checked before spawning, so a missing model is reported as a missing
        // model rather than as whatever exit status the server chooses for it.
        let model = entry.path.resolve(root);
        if !model.is_file() {
            return Err(Failure(format!(
                "entry '{}': no model file at '{}'",
                entry.id,
                model.display()
            )));
        }

        let port = free_port().map_err(|error| {
            Failure(format!(
                "entry '{}': no loopback port was free: {error}",
                entry.id
            ))
        })?;

        // The child is deliberately not detached. Keeping it in this process
        // group means a terminal interrupt reaches it; detaching would orphan
        // it. The Windows equivalent is a job object, which needs a dependency
        // and is recorded as a risk rather than half-built here.
        //
        // Output goes nowhere, and both alternatives were tried and rejected.
        // A pipe nobody drains blocks the child once its buffer fills, and
        // llama-server logs heavily through exactly the window this slice
        // waits out. Inheriting is worse: a child then holds whatever stdout
        // its parent had, so an orphan keeps a test harness's captured pipe
        // open and the harness waits for an end-of-file that never comes.
        // Draining threads would keep the log, and belong to the slice that
        // has somewhere to put it.
        let process = Command::new(&self.binary)
            .args(invocation::of(entry, root, port))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                Failure(format!(
                    "entry '{}': the server binary '{}' would not start: {error}",
                    entry.id,
                    self.binary.display()
                ))
            })?;

        Ok(Child {
            id: entry.id.clone(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            process,
        })
    }
}

impl Child {
    /// Where this child answers.
    #[must_use]
    pub fn endpoint(&self) -> SocketAddr {
        self.address
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

/// The first match for a name on the search path, with the platform's
/// executable suffix, so the Windows leg finds `llama-server.exe`.
fn on_search_path(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

/// A loopback port the operating system says is free.
///
/// Binding zero, reading the assignment and closing leaves a window in which
/// something else can take the port before the child binds it. That race is
/// real and this slice does not pretend otherwise: it reports the failure and
/// does not retry. The alternatives are worse -- passing the descriptor to the
/// child is not portable to Windows, and a fixed base port with an offset
/// collides with whatever else is already on the machine.
fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind((invocation::HOST, 0))?;
    let port = listener.local_addr()?.port();
    Ok(port)
}
