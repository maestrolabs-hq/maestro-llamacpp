//! Which build of the server an entry is served from.
//!
//! One router serves entries that need different builds. A model with a
//! speculative sidecar the stock server cannot load needs a patched one; every
//! other entry must go on being served by the server the router was started
//! with. A single binary chosen at startup cannot be right for both, which is
//! what `runtime` exists to say.
//!
//! Unix only, because the fixture is a second copy of the stub placed on the
//! search path under a chosen name, and the assertion is which of the two
//! copies answered. Both legs of that are path manipulation the Windows leg
//! would need its own fixture for.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

mod support;
use support::{MODEL, ModelsRoot, get, request, serving, status, stub_binary};

/// A directory on the search path carrying the stub under a chosen name.
///
/// Unique per run so one test's runtime cannot be found by another, and so
/// what answered is attributable to the copy this test placed.
struct OnPath {
    directory: PathBuf,
}

impl OnPath {
    /// Places the stub under `llama-server-<runtime>` and puts it on `PATH`.
    fn carrying(runtime: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "maestro-runtime-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).expect("a writable temporary directory");
        fs::copy(
            stub_binary(),
            directory.join(format!("llama-server-{runtime}")),
        )
        .expect("the stub is built by the tests");

        // SAFETY: the router resolves the search path when it starts a child,
        // and these tests run in one process. Prepending is additive: nothing
        // already on the path stops being findable.
        let inherited = std::env::var("PATH").unwrap_or_default();
        unsafe {
            std::env::set_var("PATH", format!("{}:{inherited}", directory.display()));
        }
        Self { directory }
    }
}

impl Drop for OnPath {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.directory));
    }
}

/// One entry, optionally naming a runtime.
fn one_entry(runtime: Option<&str>) -> String {
    let names = runtime.map_or_else(String::new, |name| format!("runtime = \"{name}\"\n"));
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
         {names}"
    )
}

#[test]
fn an_entry_naming_a_runtime_is_served_from_that_build() {
    let _placed = OnPath::carrying("fastmtp");
    let serving = serving(&one_entry(Some("fastmtp")), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&reply),
        Some(200),
        "the named build was found on the search path and answered:\n{reply}"
    );
}

#[test]
fn an_entry_naming_a_runtime_that_is_not_there_says_so_rather_than_falling_back() {
    // Nothing is placed on the path for this one. A router that quietly used
    // the stock server would start a child that cannot load what the entry
    // needs, and the reader would meet a tensor-shape error rather than a
    // missing file.
    let serving = serving(&one_entry(Some("absent")), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&reply),
        Some(502),
        "a runtime that is not there is a failure to start, not a fallback:\n{reply}"
    );
    assert!(
        reply.contains("absent") && reply.contains("llama-server-absent"),
        "the failure names the runtime and the binary it looked for:\n{reply}"
    );
}

#[test]
fn an_entry_naming_no_runtime_uses_the_server_the_router_was_started_with() {
    let serving = serving(&one_entry(None), ModelsRoot::with(&[MODEL]));

    let reply = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(
        status(&reply),
        Some(200),
        "an entry that asks for nothing keeps today's behaviour exactly:\n{reply}"
    );
}

#[test]
fn a_runtime_that_could_name_a_file_elsewhere_is_refused_by_the_catalog() {
    // A catalog is configuration. Configuration that can choose an arbitrary
    // executable is configuration that can do anything, so the name is
    // restricted to what a binary suffix can safely be.
    for bad in ["../evil", "with space", "Upper", "with/slash", ""] {
        let report = maestro_llamacpp::catalog::Catalog::parse(&one_entry(Some(bad)))
            .expect_err("a runtime that is not a plain name is refused");
        let said = report.to_string();
        assert!(
            said.contains("runtime"),
            "the refusal names the field for '{bad}': {said}"
        );
    }
}
