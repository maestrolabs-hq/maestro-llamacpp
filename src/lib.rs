//! The router's interior, exposed so the tests can link against it.
//!
//! The shipped artefact is the `model-router` binary; this library exists for
//! the one consumer named in `docs/adr/0001-one-crate-until-a-seam-is-real.md`
//! -- the test target, which has to construct a catalog, assert on its fields,
//! and drive a real child process through its lifetime.
//!
//! Five modules, and no more than the tests and the binary between them ask
//! for: a wider surface would be a promise to callers who do not exist.
//! `startup` is here on exactly that bar -- the binary prints those lines and
//! the tests read them, and a line composed inside a binary is one no test
//! can reach.

pub mod admission;
pub mod catalog;
pub mod launch;
pub mod proxy;
pub mod startup;
