//! Shared by the tests that drive real processes.
//!
//! Separate from `common`, which walks the repository for the prose and size
//! gates. These two sets of helpers have nothing to do with each other, and a
//! single module carrying both would be a module named after where it sits
//! rather than what it does.
//!
//! Partially used by design: each test binary that includes this takes the
//! helpers it needs, so an item unused by one of them is expected rather than
//! a leftover.
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{SystemTime, UNIX_EPOCH};

/// The stub server, which stands in for `llama-server` wherever a test needs
/// a process that answers the health contract without a model behind it.
#[must_use]
pub fn stub_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stub-llama-server"))
}

/// The status code from `GET /health`, or `None` when nothing answered.
///
/// Hand-written because this repository has one dependency and it parses
/// TOML. One request and one status line do not earn a second.
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

/// Kills a child when the test leaves, however it leaves.
///
/// A test that fails an assertion still has to not leak a server process, and
/// a `Drop` guard is the only thing that survives a panic.
pub struct Running(pub Child);

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
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

/// Polls until something answers on the address, returning its status code.
///
/// # Panics
///
/// If nothing answers within the window, which is the failure the caller is
/// asserting against rather than a condition to report.
pub fn wait_for_health(address: SocketAddr, expected: u16) -> u16 {
    for _ in 0..400 {
        if let Some(code) = health(address)
            && code == expected
        {
            return code;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("nothing answered {expected} on {address}");
}
