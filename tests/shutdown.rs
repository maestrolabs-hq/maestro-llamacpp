//! A signalled router ends its children before it goes.
//!
//! `SIGTERM` is what `systemctl stop`, a container stop and a plain `kill`
//! send, and a child is a separate process that nothing in the operating
//! system ends when its parent does. So this drives the real binary rather
//! than the library: the handler that turns a signal into `Router::stop` lives
//! in `main`, and only a process can be signalled.
//!
//! Unix only, and not for want of trying. There is no portable way to send a
//! console control event to another process from a test: `GenerateConsoleCtrlEvent`
//! reaches every process attached to the test's own console, this one
//! included, and a process started with its own console cannot be reached at
//! all. The Windows leg still compiles this crate to nothing and runs the rest.
#![cfg(unix)]

mod support;

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use support::{MODEL, ModelsRoot, catalog_text, get, health, request, status, stub_binary};

/// A search path whose `llama-server` is the stub, so the binary under test
/// finds a child the way it does in the field: by name, on `PATH`.
struct SearchPath {
    directory: PathBuf,
}

impl SearchPath {
    fn with_stub() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "maestro-llamacpp-path-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).expect("a writable temporary directory");
        // The suffix is derived even here, where it is always empty: the rule
        // is that no file name assumes a platform, and a rule with an
        // exception is a rule that gets copied without it.
        let name = format!("llama-server{}", std::env::consts::EXE_SUFFIX);
        std::os::unix::fs::symlink(stub_binary(), directory.join(name))
            .expect("a symlink in the temporary directory");
        Self { directory }
    }

    /// The current search path with this directory in front of it.
    fn value(&self) -> OsString {
        let mut paths = vec![self.directory.clone()];
        if let Some(inherited) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&inherited));
        }
        std::env::join_paths(paths).expect("a search path with no separator in it")
    }
}

impl Drop for SearchPath {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.directory));
    }
}

/// The router binary, serving, and ended when the test leaves however it
/// leaves.
struct Router {
    process: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Router {
    /// Starts `model-router serve` on an ephemeral port and reads the address
    /// it reports.
    fn serve(catalog: &std::path::Path, root: &ModelsRoot, search: &SearchPath) -> Self {
        let mut process = Command::new(env!("CARGO_BIN_EXE_model-router"))
            .arg("serve")
            .arg(catalog)
            .arg("127.0.0.1:0")
            .env("PATH", search.value())
            .env("MAESTRO_MODELS_ROOT", root.path())
            // Neither is under test, and either inherited from the shell
            // would make the router do something this test did not ask for.
            .env_remove("MAESTRO_MEMORY_BUDGET_MIB")
            .env_remove("MAESTRO_IDLE_UNLOAD_SECONDS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the router binary is built by cargo test");
        let stdout = BufReader::new(process.stdout.take().expect("a piped stdout"));
        Self { process, stdout }
    }

    /// The address the router says it is serving on, from its first line.
    fn address(&mut self) -> SocketAddr {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("the router's first line");
        let address = line
            .trim()
            .strip_prefix("serving on http://")
            .unwrap_or_else(|| {
                panic!(
                    "the first line names the address; got {line:?}, and stderr said:\n{}",
                    self.stderr()
                )
            });
        address.parse().expect("an address after the scheme")
    }

    /// Sends the signal a service manager sends.
    ///
    /// Through the `kill` command, because `std`'s `Child::kill` sends
    /// `SIGKILL`, which no process can handle and which is the one case this
    /// test is not about.
    fn terminate(&self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.process.id().to_string()])
            .status()
            .expect("the kill command");
        assert!(status.success(), "kill -TERM reached the router");
    }

    /// The exit status, if the router ends before the deadline.
    fn exited_within(&mut self, deadline: Duration) -> Option<std::process::ExitStatus> {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if let Some(status) = self.process.try_wait().expect("the router's status") {
                return Some(status);
            }
            sleep(Duration::from_millis(25));
        }
        None
    }

    /// Whatever the router has written to stdout since the address line.
    ///
    /// Read only once the process has ended, so this cannot block on a pipe
    /// that is still open.
    fn rest_of_stdout(&mut self) -> String {
        let mut text = String::new();
        drop(self.stdout.read_to_string(&mut text));
        text
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        if let Some(mut stderr) = self.process.stderr.take() {
            drop(stderr.read_to_string(&mut text));
        }
        text
    }
}

impl Drop for Router {
    fn drop(&mut self) {
        // Signalled rather than killed, so a router that can stop its
        // children on a signal does so here too, whichever way the test ended.
        // Only a router that ignores the signal is then killed outright.
        if self.process.try_wait().ok().flatten().is_none() {
            self.terminate();
            if self.exited_within(Duration::from_secs(5)).is_none() {
                drop(self.process.kill());
            }
        }
        drop(self.process.wait());
    }
}

/// Polls until the child's port stops answering, or fails saying it did not.
fn assert_goes_quiet(endpoint: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if health(endpoint).is_none() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!(
        "the child at {endpoint} outlived the router that was signalled to \
         stop. This is the process a service manager's stop leaves behind, \
         holding its memory."
    );
}

#[test]
fn a_terminated_router_ends_its_children_and_exits_cleanly() {
    let root = ModelsRoot::with(&[MODEL]);
    let catalog = root.path().join("catalog.toml");
    fs::write(&catalog, catalog_text("")).expect("a catalog in the temporary directory");
    let search = SearchPath::with_stub();

    let mut router = Router::serve(&catalog, &root, &search);
    let address = router.address();

    // One request, so a child exists to be orphaned.
    let reply = request(address, &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&reply), Some(200), "a child answered:\n{reply}");
    // The stub reflects the Host it was given, which the rewrite set to the
    // child's own address. That is how this test learns a port the router
    // never told anyone about.
    let endpoint = reply
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .expect("the echo carries the address the child was reached on")
        .to_owned();
    assert_eq!(
        health(endpoint.as_str()),
        Some(200),
        "the child answers while the router holds it"
    );

    router.terminate();

    let exit = router
        .exited_within(Duration::from_secs(20))
        .unwrap_or_else(|| {
            panic!(
                "the router was still running 20 seconds after SIGTERM; stderr said:\n{}",
                router.stderr()
            )
        });
    assert_eq!(
        exit.code(),
        Some(0),
        "a signalled router stops its children and exits cleanly. No code at \
         all means the signal's default action ended it before it could stop \
         anything: {exit}"
    );
    assert_goes_quiet(&endpoint);

    let said = router.rest_of_stdout();
    assert!(
        said.contains("ended 1 child"),
        "the router says how many children it ended on the way out:\n{said}"
    );
}
