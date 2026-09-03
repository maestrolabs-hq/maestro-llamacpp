//! The router's interior, exposed so the tests can link against it.
//!
//! The shipped artefact is the `model-router` binary; this library exists for
//! the one consumer named in `docs/adr/0001-one-crate-until-a-seam-is-real.md`
//! -- the test target, which has to construct a catalog, assert on its fields,
//! and drive a real child process through its lifetime.
//!
//! Two modules, and no more than the tests and the binary between them ask
//! for: a wider surface would be a promise to callers who do not exist.

pub mod catalog;
pub mod launch;
