# Bootstrap and catalog implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn this repository from a founding specification into a governed
Rust repository that passes the same required checks as its siblings, then
ship the first vertical slice from the specification: a TOML catalog that
parses, validates, and cannot represent a path anchored to one machine.

**Architecture:** One crate, one binary. The catalog is a module inside it, not
a separate crate. See "The layout decision" below for why, and Task 5 for the
architecture decision record that fixes it. Slice 1 has no server, no child
process, and no HTTP: the binary's only behaviour after Delivery 2 is to read a
catalog and report whether it is valid.

**Spec:** `docs/superpowers/specs/2026-09-03-model-router-design.md`. This plan
covers the bootstrap that specification's "Governance" section requires, and
slice 1 of its "Vertical slices" list. Slices 2 through 6 are out of scope and
are named at the end.

## The layout decision

Single crate, following `maestro-pi-config`, not the two-crate workspace of
`maestro-core`.

`maestro-core` splits `protocol` from `cli` because something varies across
that seam: the envelope shape has consumers that are not the terminal client.
Its own manifest records what happened when that test was not applied --- four
further crates existed, held no code, and were deleted. The comment left
behind is the rule this plan follows: a seam is only real when something varies
across it.

Measured against the six slices, nothing varies. Slice 1 parses a catalog,
slice 2 launches a child, slice 3 proxies one endpoint, slice 4 adds swapping,
slice 5 adds residency, slice 6 is evidence. Every one of them ships inside the
same binary, and none introduces a second consumer of any module. A workspace
today would be four manifests describing one program.

`maestro-pi-config` is the closer structural match and is therefore the
template for the crate manifest, the test layout, and the `duplication-test`
input passed to the shared Rust workflow.

Extracting a crate later is a small, mechanical change. Collapsing a
speculative workspace is not, which is why the estate has already had to do it
once.

## Global constraints

- The specification is the source of truth. Where this plan departs from it,
  the departure is named in the task and carries its reason.
- Failing test first. Every behaviour in Delivery 2 has a test that fails for
  the intended reason before the code that satisfies it exists.
- No gate is weakened. A blocked check is reported, not bypassed.
- One concern per commit. Conventional commit messages.
- English only in tracked prose.
- No tracked file names one machine. Paths are derived at run time: the home
  directory from the environment, the repository root from the working
  directory. Tests use a synthetic root.
- Every file opens with a brief saying what it is for. Rust uses `//!`,
  everything else uses a leading `#` comment. The shared `brief` job checks
  `*.rs`, `*.toml`, `*.sh`, `justfile`, and `.github/workflows/*.yml`.
- TOML is formatted with the organisation's `taplo` settings, which never
  collapse a multi-line array.
- `just check` passes before every commit.

## Verified facts about the shared gates

Recorded here so no task designs against a check that does not exist. All
evidence read from `maestrolabs-hq/.github` at `main` and from the two
reference repositories.

| Fact | Evidence |
| --- | --- |
| The fast tier is two reusable workflows: `common-fast.yml` and `rust-fast.yml` | `maestro-core/.github/workflows/ci.yml`, `maestro-pi-config/.github/workflows/ci.yml` |
| `rust-fast.yml` requires a `duplication-test` input naming a `cargo test --test <name>` target | `.github/workflows/rust-fast.yml`, `workflow_call.inputs` |
| The Rust fast tier runs format, lint, test, a Windows and macOS matrix, and audit | `.github/workflows/rust-fast.yml` |
| `common-fast.yml` runs dependency review, secret scan, prose, brief, markdown, TOML format, machine-path scan, and workflow audit | `.github/workflows/common-fast.yml` |
| The prose gate reads `prose-rules.txt` from the shared repository and scans tracked Markdown only | `common-fast.yml`, `prose` job |
| The machine-path gate greps tracked files for home-anchored and drive-anchored paths | `common-fast.yml`, `no-absolute-paths` job |
| The estate pins the toolchain at 1.98.0 with a minimum supported version of 1.85 | `maestro-core/rust-toolchain.toml`, `maestro-core/.github/workflows/heavy.yml` |
| The licence allowlist is deliberately narrow and is widened when a real dependency needs it | `maestro-core/deny.toml`, `[licenses]` comment |

Two consequences that shape the tasks:

- **The shared gates arrive for free.** Prose, brief, markdown, TOML format,
  machine paths and workflow security are inherited by calling
  `common-fast.yml`. This plan writes no local copy of any of them. The local
  `just check` mirrors the Rust tier only, exactly as the sibling repositories
  do.
- **The copy-paste gate needs a test target to exist from the first pull
  request.** `rust-fast.yml` takes the target name as a required input, so the
  skeleton cannot ship without it.

## Decisions, resolved

The specification is silent on six points that Delivery 2 cannot avoid. All six
are now settled: five stand as the defaults this plan proposed, and the sixth
was decided against its default. Task 7 needs no further confirmation.

1. **Reasoning fields.** The specification says "reasoning preset (Qwen3
   hybrid thinking effort)", one field. The current catalog carries two
   distinct keys: a parser format, and an effort that only one entry sets.
   *Default:* two optional fields, `reasoning_format` and `reasoning_effort`,
   because collapsing them would lose the distinction the current catalog
   already makes.
2. **The rest of the flags.** The specification lists eight fields. The current
   catalog carries roughly twenty settings per entry --- cache types, flash
   attention, parallelism, speculative decoding parameters, batch sizes, split
   and load modes. *Default:* the schema gains a `flags` table of string
   key-value pairs, parsed and validated as present but not interpreted until
   slice 2, which is the first slice that launches anything. Dropping them
   silently would break the parity the migration stage requires.
3. **Memory estimate.** The specification lists one per entry; the current
   catalog has only a single global fit target. *Default:*
   `memory_estimate_mib`, a non-zero unsigned integer, validated for shape and
   unused until slice 4 introduces eviction.
4. **The path root.** The specification says paths are derived from the
   environment without naming the anchor. *Default:* catalog paths are
   relative, and resolve against a models root taken from the environment,
   falling back to a documented default under the home directory.
5. **A defaults table.** The current catalog has a wildcard section carrying
   settings shared by every entry. The specification's schema does not mention
   one. *Default:* a `[defaults]` table with the same merge semantics, because
   without it every entry repeats the same five settings.
6. **The resident model identifier.** The specification says "a small
   instruction model (Qwen3 0.6B class)" without an identifier, and no such
   file is installed yet. *Resolved against the default:* the identifier is
   `qwen3-06b`, not `steward`. The steward specification in `maestro-pi-config`
   already names the endpoint `/models/qwen3-06b/v1`, and the catalog names
   models, not roles --- a role-named identifier becomes wrong the moment a
   second consumer of the same model appears. Slice 1 still does not check
   that any file exists --- existence is a launch concern, which is slice 2.

---

## Delivery 1 --- estate bootstrap

## Task 1 --- The crate, its toolchain, and its lint configuration

The smallest thing that compiles and can be linted. No catalog yet.

### Steps

- [ ] Write `Cargo.toml` for a single package named `maestro-llamacpp` with a
      binary named `model-router`. Edition 2024, Rust version 1.85, MIT. Open
      it with a `#` brief. Copy the `[lints.clippy]` block from
      `maestro-pi-config/Cargo.toml` verbatim: pedantic at warn, plus
      `too_many_lines`, `cognitive_complexity` and `too_many_arguments`.
      Leave `[dependencies]` empty with a comment saying so; Task 8 is the
      first task allowed to add one.

- [ ] Write `rust-toolchain.toml` byte-identical to
      `maestro-core/rust-toolchain.toml`. It is pinned by hash in the
      governance baseline and must not drift.

- [ ] Write `clippy.toml` and `deny.toml` byte-identical to
      `maestro-core`'s. Both open with their briefs already.

- [ ] Write `src/main.rs` with a `//!` brief and a `main` that reports no
      commands are implemented and exits non-zero, mirroring
      `maestro-core/crates/cli/src/main.rs`. An empty `main` that exits zero
      would let a broken build look like a working tool.

### Verification

```sh
cargo build --all-targets && cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
```

Expected: builds, no formatting diff, no clippy output.

```sh
cargo run --quiet 2>&1; echo "exit=$?"
```

Expected: a one-line message that no commands are implemented, and `exit=1`.

## Task 2 --- Repository hygiene, licence and changelog

The files a contributor's editor and a reviewer's diff both depend on.

### Steps

- [ ] Copy `.editorconfig`, `.gitattributes` and `.gitignore` from
      `maestro-core` verbatim. They are polyglot on purpose and already carry
      their briefs.

- [ ] Copy `.pre-commit-config.yaml` from `maestro-core` and repoint the
      `cargo-similarity` hook at this repository's target name,
      `cargo test --test duplication`, which Task 4 creates. The forward
      reference is harmless: that hook runs at the pre-push stage, and Task 6
      is the first push. Every other hook is unchanged.

- [ ] Copy `LICENSE` from `maestro-core`.

- [ ] Write `CHANGELOG.md` using `maestro-core`'s seed text: the Keep a
      Changelog and Semantic Versioning links, the note that release-please
      generates it, and an `## [Unreleased]` section stating that nothing is
      released yet.

### Verification

```sh
git status --short
```

Expected: only the intended new files, and nothing that `.gitignore` should
have caught.

```sh
diff <(git show :.editorconfig 2>/dev/null || cat .editorconfig) ../maestro-core/.editorconfig && echo IDENTICAL
```

Expected: `IDENTICAL`.

## Task 3 --- The task runner

`just check` is the local mirror of the Rust fast tier. It runs the same
commands CI runs, not equivalents.

### Steps

- [ ] Write `justfile` from `maestro-core`'s, keeping the derived `PATH`
      prologue unchanged --- it resolves the home directory portably rather
      than assuming a Unix separator, which is the reason it exists.

- [ ] Keep the `install`, `setup`, `check`, `fmt` and `doctor` recipes. Drop
      the `coverage` recipe: there is no behaviour to cover, and a recipe that
      reports nothing teaches people to ignore its output. Coverage is out of
      scope for this plan entirely, including Delivery 2 --- it belongs with
      the first slice whose branching is worth measuring.

- [ ] Confirm `check` runs exactly: `cargo fmt --all --check`, then
      `cargo clippy --all-targets --all-features -- -D warnings`, then
      `cargo test --all-targets`, then `cargo machete`, then
      `cargo deny check`.

### Verification

```sh
just doctor
```

Expected: four lines naming the resolved `just`, `cargo`, `prek` and `rustc`,
each from the repository's own toolchain rather than an inherited one.

```sh
just check
```

Expected: every command exits zero. `cargo deny check` reports no advisories
and no unmatched licence allowance on an empty dependency tree.

## Task 4 --- The gate tests that apply on day one

Three checks belong to this repository rather than to the shared workflows, and
one exists because the shared Rust workflow requires a target to call.

The shared gates are not copied. Prose, machine paths, brief, markdown and TOML
formatting are inherited by calling `common-fast.yml`, and a local
reimplementation would be a second definition of the same rule that can drift
from it. The estate has already recorded what that costs.

### Steps

- [ ] Write `tests/common/mod.rs` with a `//!` brief, a `repo_root()` that
      derives the root from `CARGO_MANIFEST_DIR`, and a `sources()` that walks
      the tree skipping `target`, `.git`, `node_modules`, `.worktrees` and
      `.superpowers`. One walk, shared, so the copy-paste gate does not fire on
      three tests doing the same traversal.

- [ ] Write `tests/language.rs`: `all_prose_is_english`, built from the code
      point ranges used in `maestro-pi-config/tests/language.rs` so the test
      file itself stays accent-free and cannot fail its own rule.

- [ ] Write `tests/documents.rs`: `no_document_links_to_a_missing_file`. Scan
      tracked Markdown for repository-relative links and assert each target
      exists. This repository's specification, plan and README already
      cross-reference each other, and a renamed file is exactly the drift no
      other gate sees.

- [ ] Write `tests/standards.rs`: `no_module_becomes_a_dumping_ground` at 250
      lines, counting lines before the first test module, copied in shape from
      `maestro-pi-config/tests/standards.rs`.

- [ ] Write `tests/duplication.rs` with the `similarity-rs` invocation and an
      allowlist of accepted pairs with reasons, following
      `maestro-pi-config/tests/duplication.rs`. The allowlist starts empty.

### Verification

```sh
cargo test --all-targets
```

Expected: every test passes. `all_prose_is_english` and
`no_document_links_to_a_missing_file` both assert they scanned a non-empty file
list, so a broken walk fails loudly instead of reporting green over nothing.

```sh
cargo test --test duplication
```

Expected: passes. Two outcomes are acceptable. If no pairs are detected, the
allowlist stays empty. If `language.rs` and `documents.rs` are reported as
similar --- both walk the same corpus and assert on what they find, which is
exactly the pair `maestro-core` had to record --- then either move the shared
scan into `common/mod.rs` or record the pair with its reason. Do not raise the
threshold.

## Task 5 --- Repository documents and continuous integration

### Steps

- [ ] Write `.github/workflows/ci.yml` calling
      `maestrolabs-hq/.github/.github/workflows/common-fast.yml@main` and
      `rust-fast.yml@main`, passing `duplication-test: duplication`. Keep the
      triggers, concurrency and permissions in this file, as the sibling
      repositories do, so a reader of this repository can see them.

- [ ] Write `.github/workflows/heavy.yml` calling the two heavy workflows with
      `msrv: "1.85"`, on a weekly schedule and manual dispatch, never a
      required context. Copy the permissions comment explaining why only the
      `common` call is granted `security-events: write`.

- [ ] Copy `.github/CODEOWNERS`, `.github/dependabot.yml`, and
      `.github/release-please/{config.json,manifest.json}` from `maestro-core`,
      adjusting the CODEOWNERS paths that name `crates/` since this repository
      has none.

- [ ] Write `AGENTS.md` adapted from `maestro-core`'s: the same four rules and
      the same enforced-rules table, with the sections that describe the
      supervisor and the protocol replaced by this repository's subject --- a
      router that supervises child processes and must behave the same on three
      operating systems. Read the `writing-for-agents` skill first
      (`mattpocock/skills`, `skills/productivity/writing-for-agents`) and apply
      it: `AGENTS.md` is always-loaded context, so every line spends the window
      on every turn. Prune no-ops, keep each rule in one source of truth, and
      phrase each rule as the behaviour to perform.

- [ ] Write `docs/adr/0001-one-crate-until-a-seam-is-real.md` recording the
      layout decision, its context, and the condition under which it is
      reopened: a second consumer of a module, not a feeling that the file is
      getting long.

- [ ] Update `README.md` to replace "founding specification only, no
      implementation yet" with the current state, and to link the plan
      alongside the specification.

### Verification

```sh
just check && git diff --check
```

Expected: gates pass, no whitespace errors.

```sh
awk 'NF{print;exit}' justfile Cargo.toml deny.toml clippy.toml rust-toolchain.toml .github/workflows/ci.yml .github/workflows/heavy.yml | grep -c '^#'
```

Expected: a count equal to the number of files listed, proving each opens with
a brief before the `brief` job says so in CI.

```sh
git grep -nI -e '/home' -e '/Users' -- ':!*.lock' || echo "No machine-anchored path."
```

Expected: `No machine-anchored path.` This is a local proxy, not the rule: the
authoritative check is the shared `no-absolute-paths` job, which Task 6 runs.
The full pattern is deliberately not reproduced here --- a file containing it
fails the gate, which is why the shared workflow excludes its own rules file.

## Task 6 --- The first pull request

### Steps

- [ ] Commit the bootstrap in the task order above, one concern per commit.

- [ ] Push the branch and open a pull request.

- [ ] Wait for checks and record which contexts ran.

### Verification

```sh
gh pr checks --watch --interval 5
```

Expected: every context passes. The set includes `common / prose`,
`common / brief`, `common / markdown`, `common / toml`,
`common / no-absolute-paths`, `common / actions-security`,
`common / secrets-scan`, `fast / rust-format`, `fast / rust-lint`,
`fast / rust-test`, `fast / rust-audit`, and both
`fast / cross-platform` legs.

- [ ] Merge with a squash and delete the branch. If any context fails, stop and
      report the failing context and its log excerpt. Do not merge with an
      override.

---

## Delivery 2 --- Slice 1: catalog parsing and validation

Scope, restated so it cannot creep: read a TOML catalog, validate it, report
errors that name the offending entry and field. No child process, no health
check, no HTTP, no port, no swapping, no residency behaviour beyond parsing the
field that declares it.

## Task 7 --- Failing tests for the schema

Written before the parser exists, and run before it exists, so each failure is
observed for the intended reason.

### Steps

- [ ] Confirm the six decisions above are settled. If any is still open, stop
      and ask; a schema written against a guess is a schema rewritten later.

- [ ] Add `tests/catalog.rs` with a `//!` brief.

- [ ] Write the golden test: a fixture catalog under `tests/fixtures/`
      carrying the four entries the specification names --- `qwen38` with its
      draft model and projector, `qwen38-semantic` with a low reasoning
      effort, `gemma3`, and the resident `qwen3-06b` entry. Assert the parsed
      value field by field: identifiers, relative paths, context sizes,
      residency, and the reasoning fields that only two entries set. Every
      path in the fixture is relative; none names a machine.

- [ ] Write one failing case per validation rule, each asserting that the error
      message contains both the offending entry identifier and the field name:
      a missing required field, an unknown field, a context size of zero, an
      unrecognised residency value, a memory estimate of zero, and a path that
      is absolute.

- [ ] Write the invariant test: a machine-anchored path cannot be represented.
      Construct the path type directly with an absolute value and assert the
      constructor refuses it. This is the local half of the estate's path rule
      --- the shared gate scans tracked files, and this proves the type cannot
      hold one at run time either.

### Verification

```sh
cargo test --test catalog 2>&1 | tail -20
```

Expected: the target fails to compile, because `catalog` does not exist yet.
This is the intended first failure.

- [ ] Record the exact failure text in the commit message for the test-only
      commit, so the red state is evidenced rather than claimed.

## Task 8 --- The parser and the path type

The least code that turns the failing tests green.

### Steps

- [ ] Add the TOML dependency and record why. A hand-written parser for a
      configuration format with nested tables and optional fields is more code
      than the ecosystem's parser and will be wrong in a way that surfaces at
      run time. `deny.toml`'s own comment sanctions this: the allowlist "holds
      exactly what the tree uses" and is widened "when a dependency arrives
      that needs it".

- [ ] Widen `[licenses].allow` in `deny.toml` to exactly the set
      `cargo deny check` reports for the resolved tree, adding a comment naming
      the dependency that required each addition. Set the list from the tool's
      output, not from memory: an unmatched allowance is itself a warning, and
      this repository's bar is warning-free.

- [ ] Write `src/catalog.rs` with a `//!` brief explaining what a catalog is
      and why paths in it are relative.

- [ ] Implement the path type first. Its constructor takes a string, rejects
      anything absolute or drive-anchored, and returns the validation error on
      refusal. Resolution against a models root is a separate method, so a
      parsed catalog is inert until something asks for a real location.

- [ ] Implement the entry and catalog types and the validation, so that every
      error carries the entry identifier and the field name.

- [ ] Implement the `[defaults]` merge: an entry inherits every default it does
      not set, and overrides are explicit.

### Verification

```sh
cargo test --test catalog
```

Expected: every test from Task 7 passes.

```sh
just check
```

Expected: all five commands pass, including `cargo deny check` with the widened
allowlist and `cargo machete` with the new dependency actually used.

```sh
cargo test --all-targets
```

Expected: the day-one gates still pass. `no_module_becomes_a_dumping_ground`
is the one most likely to fire; if `catalog.rs` approaches 250 lines, split it
by responsibility rather than raising the limit.

## Task 9 --- The entry point and the shipped catalog

A parser nothing calls is not a slice. The smallest honest surface is one
subcommand that runs the validation and reports.

### Steps

- [ ] Replace the stub `main` with argument handling for exactly one
      subcommand: validate a catalog at a given path and report. Hand-rolled,
      no argument-parsing dependency --- one subcommand and one operand does
      not justify one.

- [ ] Exit zero on a valid catalog with a one-line summary naming the entry
      count. Exit non-zero on an invalid one, printing every validation error
      rather than only the first, so one run fixes one round of mistakes.

- [ ] Add `catalog.toml` at the repository root: the real catalog, carrying the
      same four entries as the fixture, with relative paths and a `[defaults]`
      table holding the settings every entry shares. Open it with a `#` brief.

- [ ] Add a test asserting the shipped `catalog.toml` parses and validates, so
      the file that ships cannot rot away from the parser that reads it.

- [ ] Update `README.md` with the command and its output.

### Verification

```sh
cargo run --quiet -- check catalog.toml; echo "exit=$?"
```

Expected: a line naming four entries, and `exit=0`.

```sh
printf 'x' > /tmp/broken.toml && cargo run --quiet -- check /tmp/broken.toml; echo "exit=$?"
```

Expected: a validation error naming the problem, and a non-zero exit.

```sh
just check && git diff --check
```

Expected: everything passes.

## Task 10 --- The slice pull request

### Steps

- [ ] Commit as at least two commits: the failing tests, then the parser. The
      red state must be visible in history.

- [ ] Push, open a pull request, and wait for the same contexts as Task 6.

- [ ] Merge with a squash and delete the branch, or stop and report the failing
      context.

### Verification

```sh
gh pr checks --watch --interval 5
```

Expected: every context passes, including both cross-platform legs. The path
type is the first code in this repository whose behaviour differs by platform,
so the Windows leg is the one that matters here.

---

## Risks

- **The llama-server flag surface moves between releases.** The catalog carries
  those flags as opaque pairs in slice 1 and does not interpret them, so
  nothing here breaks when they change. The exposure begins at slice 2, which
  is the first slice that passes them to a process. The mitigation the
  specification names --- pinning what the catalog depends on --- belongs to
  that slice.
- **The licence allowlist widens in Task 8.** This is the first dependency in
  the repository, and it changes a governance file. It is a reviewed decision
  in its own commit, not a side effect of another change.
- **The resident entry names a file nobody has downloaded.** Slice 1 validates
  shape, not existence, so this passes. It will fail at slice 2, and that is
  the correct place for it to fail.
- **Two reference repositories disagree on test layout.** This plan follows the
  single-crate one and says so, so the next reader does not have to infer which
  was copied.

## What this plan does not do

The five remaining slices from the specification, in its order:

- **Slice 2 --- launch and supervise one llama-server child with health
  checking.** No process is spawned by this plan.
- **Slice 3 --- proxy one dedicated per-model endpoint including streamed
  responses.** No HTTP server, no port, no streaming.
- **Slice 4 --- the generic endpoint with swap-on-demand and eviction.** The
  memory estimate is parsed and validated here; nothing consumes it.
- **Slice 5 --- residency and the resident model serving the steward.** The
  residency field is parsed; no model is held loaded.
- **Slice 6 --- cross-platform evidence and governance onboarding.** The
  cross-platform legs run from the first pull request, but the branch
  protection and baseline onboarding this repository still needs are a separate
  piece of work.

Also out of scope: retiring the current external router, moving any
configuration to per-model endpoints, and the steward extension itself, which
lives in `maestro-pi-config` and depends on slice 5.
