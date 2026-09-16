//! Finding the server binary, and taking one entry as far as a ready child.
//!
//! The starting half of the launch module. What a caller holds afterwards --
//! [`Child`], [`Liveness`] -- lives beside this, because those are the types
//! that outlive the call and this is only the work that produces them.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::binary::{BINARY_NAME, on_search_path, runtime_binary, runtime_named};
use super::port::free_port;
use super::{Child, Failure, Liveness, invocation, probe};
use crate::catalog::Entry;

/// How often readiness is asked for while a model loads.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How many spawns one `start` makes before a lost port race is reported as
/// the child failing. Every attempt after the first follows a loss.
const ATTEMPTS: usize = 4;

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
                Err(Failure::Unavailable(format!(
                    "the configured server binary is not there: '{}'",
                    path.display()
                )))
            };
        }

        on_search_path(BINARY_NAME)
            .map(|binary| Self { binary })
            .ok_or_else(|| {
                Failure::Unavailable(format!(
                    "no server binary was configured, and no '{BINARY_NAME}' \
                     was found on the search path"
                ))
            })
    }

    /// Starts one entry and returns once it is ready to answer.
    ///
    /// Retries when a spawn dies before a single connection to it ever
    /// succeeded. `free_port` releases a port before the child binds it, and
    /// under enough concurrent spawns something else takes it first; the
    /// child then exits on its own first bind attempt, having never been
    /// reachable at all. That is not a model that failed to load -- it is
    /// this launcher losing a race with itself -- and a fresh port is the
    /// honest fix, because that race cannot be closed at the source.
    ///
    /// Bounded at [`ATTEMPTS`], and bounded is the whole of the requirement:
    /// a child that never binds must fail rather than spin. The bound was one
    /// retry until a Windows runner lost the race on both attempts of a
    /// single run. Each loss is independent, so the odds fall away with every
    /// attempt, while the cost against a child that genuinely cannot start is
    /// one more spawn that exits at once.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] naming the entry when its model file is missing,
    /// when the child exits while loading (on the last attempt, if the
    /// earlier ones were lost races), or when it does not become ready inside
    /// the entry's startup budget.
    pub fn start(&self, entry: &Entry, root: &Path) -> Result<Child, Failure> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match self.attempt(entry, root) {
                Ok(child) => return Ok(child),
                Err((failure, lost)) if !lost || attempts == ATTEMPTS => return Err(failure),
                Err(_) => {}
            }
        }
    }

    /// One spawn, polled until it answers, dies, or exhausts its budget.
    ///
    /// The second element of an error is whether the child died without ever
    /// answering a single health probe -- the signature `start` retries on.
    /// A process is reported alive on the very first liveness check
    /// regardless of what it goes on to do, since that check runs as soon as
    /// `spawn` returns and a just-exec'd process has not yet had a chance to
    /// fail; liveness alone cannot tell a lost port race from a child that
    /// ran for a while and then genuinely crashed. Whether a probe ever
    /// connected can, because a child that lost the race never bound the
    /// port at all.
    fn attempt(&self, entry: &Entry, root: &Path) -> Result<Child, (Failure, bool)> {
        let mut child = self
            .spawn(entry, root)
            .map_err(|failure| (failure, false))?;

        let budget = Duration::from_secs(u64::from(entry.startup_timeout_seconds));
        let started = Instant::now();
        let mut ever_connected = false;
        loop {
            if let Liveness::Exited(status) = child.check() {
                return Err((
                    Failure::Unavailable(format!(
                        "entry '{}': the server exited while loading ({status})",
                        child.id
                    )),
                    !ever_connected,
                ));
            }
            match probe::health(child.address) {
                Some(200) => return Ok(child),
                Some(_) => ever_connected = true,
                None => {}
            }
            if started.elapsed() >= budget {
                // Killed before reporting, so a failed start leaves nothing
                // behind holding a port.
                child.stop();
                return Err((
                    Failure::NotReady(format!(
                        "entry '{}': not ready within its startup budget of {} seconds",
                        child.id, entry.startup_timeout_seconds
                    )),
                    false,
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Where this entry's model file is, if the models root carries it.
    ///
    /// Separate from [`Server::start`] so a caller can ask before doing
    /// something it cannot undo. Eviction is the caller that needs it: ending
    /// a warm model to make room, and only then discovering that the wanted
    /// one has no file, leaves the operator with neither.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the catalog names a file that is not there,
    /// which is a stale path, an unmounted root, or a half-finished download.
    pub fn model_file(entry: &Entry, root: &Path) -> Result<PathBuf, Failure> {
        let model = entry.path.resolve(root);
        if model.is_file() {
            Ok(model)
        } else {
            Err(Failure::Unavailable(format!(
                "entry '{}': no model file at '{}'",
                entry.id,
                model.display()
            )))
        }
    }

    /// A running child, before anything has asked whether it is ready.
    /// The binary this entry is served from.
    ///
    /// The one the router was started with, unless the entry names a runtime.
    /// A named one resolves on the search path as `llama-server-<name>`, which
    /// is how an operator points at a second build without the catalog
    /// carrying a path: the catalog says *which*, the machine says *where*.
    ///
    /// Resolved per start rather than once, because one router serves entries
    /// that need different builds and a single binary chosen at startup cannot
    /// be right for both.
    fn binary_for(&self, entry: &Entry) -> Result<PathBuf, Failure> {
        let Some(runtime) = entry.runtime.as_deref() else {
            return Ok(self.binary.clone());
        };

        runtime_binary(runtime).ok_or_else(|| {
            Failure::Unavailable(format!(
                "entry '{}' needs the '{runtime}' runtime, and nothing named \
                 '{}' is on the search path",
                entry.id,
                runtime_named(runtime)
            ))
        })
    }

    fn spawn(&self, entry: &Entry, root: &Path) -> Result<Child, Failure> {
        // Checked before spawning, so a missing model is reported as a missing
        // model rather than as whatever exit status the server chooses for it.
        // Checked again here rather than trusted from a caller: `start` is
        // reachable directly, and a precondition only some callers honour is
        // not a precondition.
        let _model = Self::model_file(entry, root)?;

        let port = free_port().map_err(|error| {
            Failure::Unavailable(format!(
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
        let process = Command::new(self.binary_for(entry)?)
            .args(invocation::of(entry, root, port))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                Failure::Unavailable(format!(
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
