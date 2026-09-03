//! The router's interior, exposed so the tests can link against it.
//!
//! The shipped artefact is the `model-router` binary; this library exists for
//! the one consumer named in `docs/adr/0001-one-crate-until-a-seam-is-real.md`
//! -- the test target, which has to construct a catalog and assert on its
//! fields. It carries the catalog and nothing else on purpose: a wider surface
//! would be a promise to callers who do not exist.

pub mod catalog;
