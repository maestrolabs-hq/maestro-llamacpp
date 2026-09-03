# ADR 0001: One crate until a seam is real

- Status: Accepted
- Date: 2026-09-03

## Context

This repository ships a router in six slices: parse a catalog, launch and
supervise a child, proxy one endpoint, add swapping, add residency, then prove
it cross-platform. A layout has to be chosen before the first line, and the
estate offers two precedents that disagree.

`maestro-core` is a workspace of two crates, and the split there is
load-bearing: `protocol` is the envelope shape, with consumers that are not the
terminal client. The same manifest records what happened when that test was not
applied. Four further crates existed, held no code, and were deleted. The
comment left behind states the rule: a seam is only real when something varies
across it.

`maestro-pi-config` is a single crate with one binary, because nothing outside
it has a reason to link against it.

Measured against the six slices, nothing varies. Every slice ships inside the
same binary, and none introduces a second consumer of any module. A workspace
today would be several manifests describing one program.

## Decision

One crate, `maestro-llamacpp`, producing one binary, `model-router`. The
catalog, the supervisor and the proxy are modules inside it, not crates beside
it.

`maestro-pi-config` is therefore the structural template for the manifest, the
test layout, and the `duplication-test` input passed to the shared Rust
workflow.

## Consequences and risks

The tree stays legible while it is small, and there is one manifest to keep
current rather than several.

The risk is the mirror of the one this avoids: a single crate can grow past
the point where a split would have helped, and nothing forces the question.
Two mechanical limits approximate it. `no_module_becomes_a_dumping_ground`
fails a module over 250 lines, and the per-function lints in `Cargo.toml`
(`too_many_lines`, `cognitive_complexity`, `too_many_arguments`) fail the
shapes a size limit cannot see.

Neither of those is the condition for reopening this decision, because neither
measures coupling.

## When this is reopened

A second consumer of a module. Not a long file, and not a feeling that the
tree is getting big.

Concretely: something outside this binary needs to link against the catalog
types, or a slice arrives whose module is genuinely used by two independent
callers with different release cadences. Extracting a crate at that point is a
small, mechanical change. Collapsing a speculative workspace is not, which is
why the estate has already had to do it once.

## Update, 2026-09-03: the first half of that condition fired

Slice 1 arrived with tests that assert a parsed catalog field by field and call
the path constructor directly. A Rust integration test links against a crate's
library target, and this crate had none, so the test target became exactly the
thing this decision named: a consumer outside the binary needing the catalog
types.

The resolution is a library target, not a second crate. It is still one crate,
one manifest, and one shipped artefact; `src/main.rs` is a shell over the
library, and `src/lib.rs` exposes the catalog and nothing else. The clause
above anticipated a crate extraction because it anticipated a production
consumer. A test target is a narrower case, and it is met by the narrower
change.

The decision itself is unchanged: one crate until something varies across a
seam. A second consumer that is not a test still reopens it.
