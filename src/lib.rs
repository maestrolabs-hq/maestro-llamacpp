//! The router's interior, exposed so the tests can link against it.
//!
//! The shipped artefact is the `model-router` binary; this library exists for
//! the one consumer named in `docs/adr/0001-one-crate-until-a-seam-is-real.md`
//! -- the test target, which has to construct a catalog, assert on its fields,
//! and drive a real child process through its lifetime.
//!
//! Seven modules, and no more than the tests and the binary between them ask
//! for: a wider surface would be a promise to callers who do not exist.
//! `startup` is here on exactly that bar -- the binary prints those lines and
//! the tests read them, and a line composed inside a binary is one no test
//! can reach. `memory` is here because a test states the figures a machine
//! would report, and a probe it cannot build is a probe it cannot state.
//! `gguf` is here because the tests write files in that format and read them
//! back through the same code the catalog derives from.

pub mod admission;
pub mod bench;
pub mod catalog;
pub mod gguf;
pub mod idle;
pub mod launch;
pub mod memory;
pub mod proxy;
pub mod queue;
pub mod startup;
