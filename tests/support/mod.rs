//! Shared by the tests that drive real processes.
//!
//! Separate from `common`, which walks the repository for the prose and size
//! gates. These two sets of helpers have nothing to do with each other, and a
//! single module carrying both would be a module named after where it sits
//! rather than what it does.
//!
//! Every test target compiles all of this and uses a subset: supervision
//! drives children directly and never sends a request, the proxy tests send
//! requests and never look at a child. Rust has no notion of a partially used
//! module, so the alternative to the allowance below is one support file per
//! target with the shared parts copied between them -- which the duplication
//! gate would rightly refuse.
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The stub server, which stands in for `llama-server` wherever a test needs
/// a process that answers the health contract without a model behind it.
#[must_use]
pub fn stub_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stub-llama-server"))
}

/// The status code from `GET /health`, or `None` when nothing answered.
///
/// Hand-written because neither of this repository's two dependencies speaks
/// HTTP. One request and one status line do not earn a third.
pub fn health(address: impl ToSocketAddrs) -> Option<u16> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    reply.split_whitespace().nth(1)?.parse().ok()
}

/// A models root with placeholder files in it, removed when the test ends.
///
/// Under the system temporary directory, which the estate's path rule allows
/// because it names a platform rather than a machine. The files are empty:
/// nothing in this slice reads a model, it only checks that one is there.
pub struct ModelsRoot {
    root: PathBuf,
}

impl ModelsRoot {
    /// Creates a root carrying each of the given relative locations.
    ///
    /// # Panics
    ///
    /// If the temporary directory cannot be written, which is a broken
    /// machine rather than a failing test.
    #[must_use]
    pub fn with(files: &[&str]) -> Self {
        let unique = format!(
            "maestro-llamacpp-{}-{:?}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("a clock after 1970")
                .as_nanos(),
            std::thread::current().id()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("a writable temporary directory");
        for file in files {
            let path = root.join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("a writable temporary directory");
            }
            fs::write(&path, b"").expect("a writable placeholder");
        }
        Self { root }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for ModelsRoot {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.root));
    }
}

/// How long a test waits on a socket before calling it a hang.
///
/// Generous, because a loaded continuous-integration machine is slow and this
/// is a safety net rather than an assertion. Nothing here is timing the
/// router; the tests that do that assert their own margins.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Opens a connection that fails rather than blocks.
fn connected(address: impl ToSocketAddrs) -> TcpStream {
    let stream = TcpStream::connect(address).expect("the router is listening");
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("a read timeout, so a hang fails rather than blocking the suite");
    stream
}

/// Sends one raw request and reads the whole reply as text.
///
/// Hand-written for the same reason `health` is: neither of this repository's
/// two dependencies speaks HTTP. One request and one reply do not earn a
/// third.
pub fn request(address: impl ToSocketAddrs, raw: &str) -> String {
    let mut stream = connected(address);
    stream.write_all(raw.as_bytes()).expect("write");
    let mut reply = String::new();
    // The result is dropped rather than expected: a reply the far end cut
    // short is a legitimate outcome for several of these tests, and what
    // arrived before the cut is what they assert on.
    drop(stream.read_to_string(&mut reply));
    reply
}

/// Sends one raw request and records when each chunk of the reply arrived.
///
/// The arrival times are the point. A relay that buffered a stream would
/// deliver every event in one read, and the reply text would be identical
/// either way -- which is why no test in this repository asserts a stream by
/// its content alone.
pub fn arrivals(address: impl ToSocketAddrs, raw: &str) -> (String, Vec<Duration>) {
    let mut stream = connected(address);
    stream.write_all(raw.as_bytes()).expect("write");

    let started = Instant::now();
    let mut body = String::new();
    let mut times = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                times.push(started.elapsed());
                body.push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        }
    }
    (body, times)
}

/// A request head with no body, ready to send.
#[must_use]
pub fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: router\r\nConnection: close\r\n\r\n")
}

/// A request carrying a JSON body of the length it declares.
#[must_use]
pub fn post(path: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\n\
         Host: router\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
}

/// The status code a reply carries, or `None` when it carried none.
#[must_use]
pub fn status(reply: &str) -> Option<u16> {
    reply.split_whitespace().nth(1)?.parse().ok()
}

/// The one model file every entry in these tests points at.
pub const MODEL: &str = "cache/gemma/gemma-3-1b.gguf";

/// A catalog carrying one entry called `gemma3`, plus whatever the test adds.
///
/// Written as text rather than built as a value, because that is how a
/// catalog reaches the router in the field, and a test that skipped the parser
/// would be agreeing with itself about the shape.
#[must_use]
pub fn catalog_text(extra: &str) -> String {
    format!(
        "version = 1\n\
         \n\
         [defaults]\n\
         context_size = 4096\n\
         residency = \"on-demand\"\n\
         memory_estimate_mib = 512\n\
         startup_timeout_seconds = 30\n\
         \n\
         [models.gemma3]\n\
         path = \"{MODEL}\"\n\
         {extra}"
    )
}

/// A router bound to an ephemeral port, serving on a thread of its own.
///
/// The models root is held here so it outlives the router: dropping it would
/// remove the files the router resolves entries against while it is still
/// serving them.
pub struct Serving {
    address: std::net::SocketAddr,
    router: std::sync::Arc<maestro_llamacpp::proxy::Router>,
    _root: ModelsRoot,
}

/// Ends the router's children when the test that started them ends.
///
/// Without this every test leaves a server process behind: `serve` never
/// returns, so the router is never dropped, so the children it holds are never
/// stopped -- and a child is a separate process that outlives the one that
/// started it.
impl Drop for Serving {
    fn drop(&mut self) {
        self.router.stop();
    }
}

impl Serving {
    /// Where the router is listening.
    #[must_use]
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// Which entries hold a child, without asking any of them for anything.
    #[must_use]
    pub fn loaded(&self) -> Vec<String> {
        self.router.loaded()
    }

    /// Residents the startup loader could not load.
    #[must_use]
    pub fn resident_failures(&self) -> Vec<String> {
        self.router.resident_failures()
    }
}

/// Waits until the router satisfies a condition, or fails saying what it saw.
///
/// Residents load on a thread of their own, so a test that asserted the moment
/// it started serving would be racing the loader rather than testing it.
///
/// Nothing here asserts a duration. The deadline is a hang guard, generous
/// because a loaded continuous-integration machine is slow and because Windows
/// spawns a child roughly three times slower than Linux; a test that turned
/// that difference into an assertion would fail on the platform rather than on
/// the behaviour.
///
/// # Panics
///
/// If the condition has not arrived by the deadline, reporting what was loaded
/// and what failed so the failure names a state rather than only a timeout.
pub fn settled(serving: &Serving, expected: &str, done: impl Fn(&Serving) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if done(serving) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "the router never {expected}; loaded {:?}, resident failures {:?}",
        serving.loaded(),
        serving.resident_failures()
    );
}

/// Binds a router on an ephemeral port and starts serving it.
///
/// # Panics
///
/// If the catalog is not usable or the port cannot be bound, which is a broken
/// test rather than a failing one.
#[must_use]
pub fn serving(catalog: &str, root: ModelsRoot) -> Serving {
    budgeted(catalog, root, None)
}

/// The same, under a stated memory budget.
///
/// Separate from `serving` so the tests that are not about eviction say
/// nothing about it, and so the ones that are state their ceiling at the call
/// rather than through the environment -- which is process-global and would
/// race every other test in the binary.
///
/// # Panics
///
/// If the catalog is not usable or the port cannot be bound, which is a broken
/// test rather than a failing one.
#[must_use]
pub fn budgeted(catalog: &str, root: ModelsRoot, limit_mib: Option<u32>) -> Serving {
    let parsed = maestro_llamacpp::catalog::Catalog::parse(catalog).expect("a usable catalog");
    let server = maestro_llamacpp::launch::Server::located(Some(&stub_binary()))
        .expect("the stub binary is built by cargo test");
    let router = maestro_llamacpp::proxy::Router::bind(
        "127.0.0.1:0".parse().expect("a loopback address"),
        parsed,
        root.path().to_path_buf(),
        server,
        maestro_llamacpp::admission::Budget::new(limit_mib),
    )
    .expect("an ephemeral loopback port");

    let router = std::sync::Arc::new(router);
    let address = router.address();
    // Detached on purpose: `serve` never returns, so there is nothing to join.
    // The accept loop outlives the test; what must not outlive it is the
    // children, which `Serving`'s Drop ends.
    let serving = std::sync::Arc::clone(&router);
    std::thread::spawn(move || serving.serve());
    Serving {
        address,
        router,
        _root: root,
    }
}
