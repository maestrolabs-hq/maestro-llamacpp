//! Finding the server binary, and taking one entry as far as a ready child.
//!
//! The starting half of the launch module. What a caller holds afterwards --
//! [`Child`], [`Liveness`] -- lives beside this, because those are the types
//! that outlive the call and this is only the work that produces them.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Child, Failure, Liveness, invocation, probe};
use crate::catalog::Entry;

/// What the server is called. Located on the search path, never bundled.
const BINARY_NAME: &str = "llama-server";

/// How often readiness is asked for while a model loads.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The server binary this router runs children from.
///
/// Located, never bundled: a configured path when there is one, otherwise
/// whatever the operating system finds on the search path.
#[derive(Debug)]
pub struct Server {
    binary: PathBuf,
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
        let mut child = self.spawn(entry, root)?;

        // Readiness and liveness are different questions and both are asked on
        // every pass. Liveness first: a child that died while loading is
        // reported with its status immediately, rather than waiting out a
        // budget that a dead process can never satisfy.
        let budget = Duration::from_secs(u64::from(entry.startup_timeout_seconds));
        let started = Instant::now();
        loop {
            if let Liveness::Exited(status) = child.check() {
                return Err(Failure(format!(
                    "entry '{}': the server exited while loading ({status})",
                    child.id
                )));
            }
            if probe::health(child.address) == Some(200) {
                return Ok(child);
            }
            if started.elapsed() >= budget {
                // Killed before reporting, so a failed start leaves nothing
                // behind holding a port.
                child.stop();
                return Err(Failure(format!(
                    "entry '{}': not ready within its startup budget of {} seconds",
                    child.id, entry.startup_timeout_seconds
                )));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// A running child, before anything has asked whether it is ready.
    fn spawn(&self, entry: &Entry, root: &Path) -> Result<Child, Failure> {
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
