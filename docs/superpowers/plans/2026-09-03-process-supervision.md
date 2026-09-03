# Process supervision implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship slice 2 from the specification: turn one catalog entry into a
running `llama-server` child on a loopback port, wait until it is ready to
answer, and stop it again. Every failure names the entry it came from.

**Architecture:** One deep module, `launch`. Its interface is two types a
caller holds -- `Server` and `Child` -- carrying five methods between them,
plus the two they hand back, `Failure` and `Liveness`. Locating the server
binary, translating an entry into a command line, choosing a port, polling for
readiness, and stopping a process on three operating systems are all
implementation and stay inside it. The seam is the
server binary itself: an executable that speaks the health contract. Two
adapters satisfy it -- the real `llama-server`, and a stub this repository
builds for its tests -- so the seam is real rather than hypothetical.

**Spec:** [model router design](../specs/2026-09-03-model-router-design.md),
slice 2: "launch and supervise one llama-server child with health checking".
Slices 3 to 6 are out of scope and are named at the end.

**Builds on:** [slice 1](2026-09-03-bootstrap-and-catalog.md), which landed the
catalog. This plan is the first consumer of a parsed entry, and therefore the
first code that resolves a relative location against a models root.

## The seam, and why the stub is not a mock

A real `llama-server` needs a multi-gigabyte model file and a graphics card.
Neither exists in continuous integration, so a test that requires one is a test
that never runs. The temptation is to put a trait in front of process spawning
and pass a fake in tests. That would move the seam to the wrong place: the code
that spawns, polls and kills is exactly the code slice 2 exists to get right,
and a trait in front of it means continuous integration never exercises it.

So the seam sits one level lower, at the executable. `Server::start` takes a
path to a binary and runs it. Continuous integration points it at
`stub-llama-server`, a binary in this repository that binds a port and answers
`/health`. Manual verification points it at the real `llama-server`. The
supervision code path -- pick a port, build the command line, spawn, poll,
detect exit, kill, reap -- is byte-identical in both cases. Nothing is mocked;
one adapter is simply cheaper to run.

This satisfies the rule that one adapter is a hypothetical seam and two make it
real. It also means the Windows and macOS legs of the fast tier run real
process supervision on every pull request, which is the only evidence that
matters for a program whose stated risk is platform process semantics.

The cost is a second binary target. It is built by `cargo test --all-targets`
and therefore by every leg of the fast tier, and it is not released: the shared
release workflow takes the name of one binary as an input. A feature flag was
considered and rejected -- the fast tier runs `cargo test --all-targets`
without `--all-features`, so a feature-gated test would be skipped in
continuous integration while reporting green, which is the failure
`tests/duplication.rs` already warns about in its own words.

## The red commit, without bypassing a hook

Slice 1 recorded a conflict: `cargo clippy --all-targets` runs at commit time
and compiles the test tree, so a commit whose tests reference a module that
does not exist cannot pass its own hook. Slice 1 disclosed a single
`--no-verify` for that commit.

This plan does better. The red commit carries the tests **and** the module
skeleton: every type and signature the tests name, with `todo!()` bodies.
The tree compiles, so clippy passes; the tests run and fail at `todo!()`, so
the red state is real and observed. The failure is also more precise than a
compile error, because it proves the interface shape is right before any
behaviour exists.

Two mechanical notes. Bind skeleton parameters with a leading underscore so
the unused-variable lint stays quiet, and remove the underscores in the green
commit. The pre-push hook runs `cargo test --all-targets` against the working
tree, not each commit, so the red commit pushes fine once the green one sits
on top of it.

## Global constraints

- The specification is the source of truth. Where this plan departs from it,
  the departure is named in the task and carries its reason.
- Failing test first, and watched. Every behaviour has a test that fails for
  the intended reason before the code that satisfies it exists, and the
  failure text goes in the commit message.
- No gate is weakened, and no hook is bypassed. A blocked check is reported.
- One concern per commit. Conventional commit messages.
- English only in tracked prose.
- No tracked file names one machine. The models root is resolved at run time;
  tests build a synthetic root under the system temporary directory.
- Every file opens with a brief. Rust uses `//!`, everything else `#`.
- Children bind loopback only. The current router refuses a non-loopback bind
  deliberately, and that property is carried over rather than re-decided.
- `just check` passes before every commit.

## Parity evidence

The invocation must reproduce what the current router builds, or the migration
stage the specification calls "parity" is not parity. These rules are read
from `llama_switchyard/core.py` and `config/models.ini`, not recalled.

| Catalog field | Argument | Evidence |
| --- | --- | --- |
| `path` | `--model` | `models.ini`, `model =` |
| `draft_path` | `--model-draft` | `models.ini`, `model-draft =` |
| `projector_path` | `--mmproj` | `models.ini`, `mmproj =` |
| `context_size` | `--ctx-size` | `SHORT_FLAGS`, `c` maps to `--ctx-size` |
| `reasoning_format` | `--reasoning-format` | `models.ini` |
| `reasoning_effort` | `--reasoning-effort` | `models.ini` |
| identifier | `--alias` | `model_command`, `--alias model.alias` |

The flags table follows `_option_arguments`, which has three cases and one
lookup:

- a value of `true` yields the bare flag, so `jinja = "true"` becomes
  `--jinja`;
- a value of `false` yields the negated flag, so `x = "false"` becomes
  `--no-x`;
- anything else yields flag and value as two arguments;
- five keys are short forms with different long spellings: `c` to
  `--ctx-size`, `ctk` to `--cache-type-k`, `ctv` to `--cache-type-v`, `fa` to
  `--flash-attn`, `np` to `--parallel`.

Note that `fa = "on"` is not the boolean case. `on` is neither `true` nor
`false`, so it becomes `--flash-attn on`, which is what the current router
sends today.

## Decisions this plan makes

The specification is silent on eight points slice 2 cannot avoid. Each carries
its reasoning. All eight were reviewed and approved as written before Task 1;
what follows is the record of what was decided and why.

1. **Restart policy: slice 2 does not restart. It reports.** Detecting that a
   child exited is in scope; deciding what to do about it is not. An honest
   restart needs a backoff, a crash-loop limit, and an answer to "what happens
   to the request that was in flight" -- and that last question cannot be
   answered before slice 3, because until then there are no requests. A child
   that dies on a bad flag would otherwise be restarted forever. Slice 4
   introduces swap-on-demand, which relaunches naturally, and that is the
   slice where a policy can be written against a real caller.

2. **Shutdown is abrupt, and graceful shutdown is deferred with a reason.**
   `Child::kill` is `SIGKILL` on the Unix platforms and `TerminateProcess` on
   Windows. Sending `SIGTERM` first needs a platform dependency the standard
   library does not offer. `llama-server` holds no durable state, so an abrupt
   stop loses only in-flight responses -- of which there are none until slice
   3. That is the slice that should pay for the dependency, because it is the
   first one with something to lose.

3. **Port selection binds port zero, reads the assignment, and closes.** The
   operating system picks a free loopback port; the number is handed to the
   child. The window between closing the probe socket and the child binding is
   a real race, and this plan does not pretend otherwise: if another process
   takes the port, startup fails with a message naming the entry and the port.
   Slice 2 does not retry. The alternative -- keeping the socket open and
   passing the descriptor -- is not portable to Windows, and a fixed base port
   plus an offset collides with whatever else is on the machine.

4. **The health probe is hand-written; the HTTP dependency is slice 3's
   decision.** Slice 2 needs one `GET /health` and the status code from the
   reply. That is roughly thirty lines over `TcpStream`, with `connect_timeout`
   and `set_read_timeout` from the standard library. Slice 3 needs streamed
   responses, connection reuse and concurrency, and should choose a library
   against those requirements rather than inherit one picked for a status
   line. The probe sits behind the module interface, so replacing it later is
   local.

5. **Readiness and liveness are different questions, and both are asked.**
   Liveness is whether the process still exists, answered by `try_wait`.
   Readiness is whether the model finished loading, answered by `/health`
   returning 200; `llama-server` answers 503 while loading. `Server::start`
   returns only when the child is ready, so a caller cannot forget to wait. It
   polls both: a child that exits during loading fails immediately with its
   exit status rather than waiting out the whole budget.

6. **The startup budget is a catalog field, defaulting generously.** The
   current router allows 30 seconds for every model. A 27-billion-parameter
   model on a cold page cache takes considerably longer, so a single global
   value is either too tight for the large entries or meaningless for the small
   ones. This plan adds `startup_timeout_seconds` to the entry schema and the
   defaults table, defaulting to 300. The failure message names the entry and
   the budget it exhausted.

7. **`stop-timeout` leaves the flags table.** The current router keeps a set of
   settings it consumes itself and never forwards, and `stop-timeout` is one of
   them. This repository's catalog carries it in `[defaults.flags]`, which
   `CONTEXT.md` defines as settings passed through without interpretation. Pass
   it through and `llama-server` rejects an unknown argument, so every launch
   fails. Filter it in code and the recorded meaning of the flags table becomes
   a lie. Slice 2 has no graceful stop and therefore no consumer for it, so the
   smallest honest action is to delete the key now and reintroduce it as a
   supervision field in the slice that implements graceful shutdown.

8. **The models root is read from `MAESTRO_MODELS_ROOT`.** Slice 1 decided
   locations are relative and resolve against a root supplied at run time,
   without naming where that root comes from. The estate prefix keeps it
   recognisable beside the other variables a machine carries, and the fallback
   is `models` under the home directory, which is where the current router
   already looks. Both are documented in `README.md`; neither is written into
   a tracked file.

## Terms this slice adds to CONTEXT.md

Written into the glossary as they settle, not batched at the end.

- **Server binary** -- the `llama-server` executable the router runs. Located,
  never bundled: a configured path if there is one, otherwise the first match
  on `PATH`.
- **Child** -- one running server process, serving exactly one catalog entry on
  one loopback port. _Avoid_: instance, worker, backend.
- **Invocation** -- the command line an entry becomes. Named because parity
  with the current router is asserted against it directly.
- **Readiness** -- whether a child has finished loading and will answer.
  Distinct from liveness.
- **Liveness** -- whether a child process still exists. A child can be alive
  and not ready for several minutes.
- **Startup budget** -- how long a child may take to become ready before the
  router gives up on it and says so.

---

## Task 1 -- The stub server

Test infrastructure, and complete before anything depends on it. This is the
second adapter at the seam, so it ships green rather than red: nothing about
the stub is the behaviour under test.

### Steps

- [ ] Add `src/bin/stub_llama_server.rs` with a `//!` brief stating plainly
      that this binary exists so continuous integration can supervise a real
      process without a real model, and that it is never released.

- [ ] Register it in `Cargo.toml` as a second `[[bin]]` named
      `stub-llama-server`, with a comment recording why it is not behind a
      feature: the fast tier runs `cargo test --all-targets` without
      `--all-features`, so a gated stub would be skipped while reporting green.

- [ ] Parse only the arguments it needs and ignore every other, so the same
      invocation that drives `llama-server` drives this: `--host`, `--port`,
      `--ready-after` in milliseconds, `--exit-after` in milliseconds,
      `--exit-code`.

- [ ] Bind the given host and port. Answer `GET /health` with 503 and a JSON
      body until `--ready-after` has elapsed, then 200 and `{"status":"ok"}`.
      Answer anything else with 404. This is the contract `llama-server`
      publishes, and the stub speaks the part of it slice 2 reads.

- [ ] With `--exit-after`, exit with `--exit-code` at that moment regardless of
      readiness, so a test can drive a crash during loading.

- [ ] Add `tests/stub.rs`: start the stub directly, assert it answers 503
      before its readiness moment and 200 after, and assert `--exit-after`
      exits with the requested code. A stub nobody tests makes every test that
      uses it vacuous.

### Verification

```sh
cargo test --test stub
```

Expected: passes. The 503-then-200 transition is observed, not assumed.

```sh
just check
```

Expected: all five commands pass. `cargo machete` sees no new dependency
because there is none.

## Task 2 -- Failing tests and the module skeleton

Written before any behaviour exists, and run before it exists.

### Steps

- [ ] Add `src/launch.rs` with a `//!` brief, and `pub mod launch;` in
      `src/lib.rs`. State the interface in the brief: what a caller must know
      is that `start` blocks until the child is ready or fails, that failure
      names the entry, and that a child binds loopback only.

- [ ] Write the skeleton: `Server`, `Child`, `Failure`, `Liveness`, with
      `Server::located`, `Server::start`, `Child::endpoint`, `Child::check`,
      `Child::stop`. Every body is `todo!()`. Parameters take a leading
      underscore.

- [ ] Add `src/launch/invocation.rs`, private to the module, with
      `fn of(entry, root, port) -> Vec<OsString>` as `todo!()`, and its
      `#[cfg(test)]` unit tests written in full. Assert every row of the parity
      table above, plus the three flag cases, plus the five short-form
      spellings, plus that `fa = "on"` is not treated as a boolean. Assert the
      three optional locations are absent from the command line when the entry
      omits them, rather than present and empty.

- [ ] Add `tests/supervision.rs` driving the public interface with the stub:
      a child becomes ready and its endpoint answers; a child that never
      becomes ready fails when the budget expires, with the entry name and the
      budget in the message; a child that exits during loading fails with its
      exit status rather than waiting out the budget; `stop` terminates it and
      `check` reports the exit afterwards; two children started together get
      different ports.

- [ ] Add `tests/models_root.rs`: resolution reads the environment and falls
      back to a documented default under the home directory. Its own target,
      because it stays red until Task 6 while `tests/supervision.rs` turns
      green at Task 5, and one target that is half green tells a reader
      nothing.

- [ ] Add to `tests/supervision.rs` the case that a missing model file fails
      with a message naming the entry and the resolved location. Slice 1
      validated shape and deferred existence to here, which is the correct
      place for it to fail. This one takes a synthetic root directly, so it
      does not wait for Task 6.

- [ ] Write the test helper that builds a synthetic models root under
      `std::env::temp_dir()` with placeholder files and removes it on drop. No
      dependency: a unique subdirectory name and a `Drop` guard is a dozen
      lines, and the estate's path rule allows the temporary directory.

### Verification

```sh
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: clean. The skeleton compiles, which is what lets this commit pass its
own pre-commit hook without `--no-verify`.

```sh
cargo test --test supervision --test catalog 2>&1 | tail -30
cargo test --lib 2>&1 | tail -20
```

Expected: the new tests fail at `todo!()` and slice 1's still pass. Record the
exact failure text in the commit message for this test-only commit, so the red
state is evidenced rather than claimed.

## Task 3 -- The invocation

Pure translation, no input or output. The first thing to turn green because
everything else depends on the command line being right.

### Steps

- [ ] Implement `invocation::of`. Resolve the three locations against the
      models root, emit the fixed arguments from the parity table, then the
      flags table in sorted order so two runs produce the same command line.

- [ ] Emit `--host 127.0.0.1` and `--port` from the caller, and `--alias` from
      the entry identifier, so a response names the model the caller asked for.

- [ ] Implement the three flag cases and the short-form lookup as a small
      table, sited beside the parity evidence in a comment naming
      `models.ini` and the function it came from.

### Verification

```sh
cargo test --lib
```

Expected: every unit test from Task 2 passes.

```sh
cargo test --all-targets 2>&1 | tail -20
```

Expected: the invocation tests pass; the supervision tests still fail, because
nothing spawns yet. A green run here would mean the supervision tests are not
testing what they claim.

## Task 4 -- Locating the binary, choosing a port, spawning

### Steps

- [ ] Implement `Server::located`: a configured path if given, otherwise the
      first `llama-server` on `PATH`, resolved with the executable suffix the
      platform uses so the Windows leg finds `llama-server.exe`. Failure says
      which of the two was tried.

- [ ] Implement port selection: bind `127.0.0.1:0`, read the assigned port,
      drop the listener, pass the number on. Record the race in a comment
      rather than a retry loop.

- [ ] Implement the spawn: build the invocation, check the model file exists
      before spawning so a missing file is reported as a missing file rather
      than an exit status, and start the child with its output captured.

- [ ] Do not detach the child. Keeping it in the router's process group means a
      terminal interrupt reaches it; detaching would orphan it. The Windows
      equivalent, a job object, needs a platform dependency and is deferred to
      the risks section rather than half-done here.

### Verification

```sh
cargo test --test supervision 2>&1 | tail -30
```

Expected: still failing, but now at readiness rather than at `todo!()`. The
message moves from "not yet implemented" to a health-check failure, which is
the evidence that spawning works before polling does.

## Task 5 -- Readiness, liveness, and stopping

### Steps

- [ ] Implement the health probe over `TcpStream`: connect with a timeout,
      send `GET /health HTTP/1.1` with `Host` and `Connection: close`, read the
      status line, return the code. Nothing else from the reply is read.

- [ ] Implement the readiness loop: poll `/health` on a fixed interval until
      200, and on every pass ask `try_wait` whether the child is still alive.
      An exit during loading returns immediately with the exit status. The
      budget comes from the entry, and its expiry kills the child before
      reporting, so a failed start leaves nothing behind.

- [ ] Implement `Child::check` returning liveness, and `Child::stop` as kill
      then `wait`, so no zombie is left on the Unix platforms.

- [ ] Make every failure name the entry. A message that says only that a health
      check failed sends the reader back to the catalog to guess which entry it
      was, which is the failure slice 1's report design exists to avoid.

### Verification

```sh
cargo test --test supervision
```

Expected: every test in this target passes, including the two children on
different ports. `tests/models_root.rs` is still red, and stays red until
Task 6 implements resolution.

## Task 6 -- The models root, the command, and the catalog fix

A module nothing calls is not a slice.

### Steps

- [ ] Implement models-root resolution: read `MAESTRO_MODELS_ROOT`, and fall
      back to `models` under the home directory taken from the environment.
      Document both in `README.md`. No tracked file names a machine.

- [ ] Add `model-router launch <catalog> <id>`: parse the catalog, locate the
      binary, start that entry, print the endpoint and how long readiness took,
      then stop the child and print that it stopped. Exit zero.

      This is deliberately not a long-running command. Staying up until
      interrupted needs a signal handler, and therefore a dependency, for no
      gain in a slice with nothing to serve. Launching, proving readiness and
      stopping is the whole of slice 2, and it doubles as the manual
      verification command in Task 7.

- [ ] Remove `stop-timeout` from `[defaults.flags]` in `catalog.toml` and from
      the test fixture, with the reason in the commit message: the current
      router consumes it rather than forwarding it, and slice 2 has no
      graceful stop to consume it. Add `startup_timeout_seconds` to the schema,
      the defaults table and the fixture, per decision 6.

- [ ] Update `README.md` with the new command and its output, and `CONTEXT.md`
      with the six terms above.

### Verification

```sh
cargo run --quiet -- launch catalog.toml qwen38
```

Expected: without a models root, a message naming the missing location and a
non-zero exit. This is the correct failure on a machine with no models.

```sh
cargo test --all-targets && just check && git diff --check
```

Expected: everything passes, no whitespace errors.

## Task 7 -- Manual verification against the real server

Continuous integration proves the supervision path with the stub. This task
proves the invocation against the thing it was written for. It is run by hand,
on a machine that has both, and its output is pasted into the pull request.

### Steps

- [ ] On a machine with `llama-server` and the models installed, export the
      models root and run `model-router launch catalog.toml gemma3`, the
      smallest entry.

- [ ] Record the printed endpoint and readiness time in the pull request.

- [ ] Confirm the four default flags are accepted by the installed
      `llama-server`: `jinja`, `n-gpu-layers`, `fit`, `fit-target`. The first
      two are long-standing; the last two are newer and are exactly the kind of
      flag the specification warns moves between releases. If any is rejected,
      that is a catalog edit, not a code change, which is the property the
      flags table exists to preserve.

- [ ] Record the `llama-server` version the mapping was verified against, in
      `catalog.toml` as a comment. The parity table is only true against a
      version, and an unpinned claim rots silently.

- [ ] Repeat with `qwen38` if the hardware allows, to observe a real startup
      time against the 300-second budget. If it is not run, say so rather than
      implying it was.

### Verification

The command prints an endpoint and a readiness time, then reports the child
stopped, and exits zero. Anything else is a finding for the pull request.

## Task 8 -- The slice pull request

### Steps

- [ ] Commit in task order, one concern per commit, with the red commit's
      failure text recorded in its message.

- [ ] Push, open a pull request, and include the Task 7 output.

- [ ] Wait for checks. The set is the same as slice 1: `common / prose`,
      `common / brief`, `common / markdown`, `common / toml`,
      `common / no-absolute-paths`, `common / actions-security`,
      `common / secrets-scan`, `fast / rust-format`, `fast / rust-lint`,
      `fast / rust-test`, `fast / rust-audit`, and both
      `fast / cross-platform` legs.

- [ ] Merge with a squash and delete the branch, or stop and report the failing
      context with its log excerpt. Do not merge with an override.

### Verification

```sh
gh pr checks --watch --interval 5
```

Expected: every context passes. The cross-platform legs matter more here than
anywhere so far: they are the only evidence that spawning, polling and killing
behave the same on Windows and macOS, and slice 2 is the first code in this
repository where that is in genuine doubt.

---

## Risks

- **The flag surface is now live.** Slice 1 carried the flags table without
  interpreting it, so a change upstream cost nothing. Slice 2 passes them to a
  process that rejects what it does not recognise, so a rename upstream now
  breaks every launch. Task 7 pins the version the mapping was verified
  against; the mitigation for a future break is a catalog edit, which is the
  property the flags table exists to preserve.

- **`stop-timeout` proves the risk is not theoretical.** It sits in the shipped
  catalog today, the current router never forwards it, and forwarding it would
  fail every launch. Task 6 removes it. The other four defaults are checked by
  hand in Task 7 rather than assumed.

- **Startup time varies by two orders of magnitude.** A small model is ready in
  under a second; a large one on a cold page cache can take minutes. The budget
  is per-entry for that reason, and its default is generous rather than tight,
  because a budget that expires on a healthy model teaches people to raise it
  without reading it.

- **The port race is real and unmitigated.** Between closing the probe socket
  and the child binding, another process can take the port. Slice 2 reports it
  and does not retry. A retry belongs with the slice that has a caller to keep
  waiting.

- **A killed router orphans its children.** They are not detached, so a
  terminal interrupt reaches them on the Unix platforms. A hard kill of the
  router does not, and on Windows there is no process-group equivalent without
  a job object and the dependency it needs. Slice 2 does not solve this, and
  says so rather than appearing to.

- **The stub can drift from the real server.** It answers the part of the
  health contract slice 2 reads, and nothing keeps it honest but Task 7. If the
  contract changes upstream, continuous integration stays green while the real
  path breaks. This is the price of the seam, and it is bounded by how little
  of the contract slice 2 depends on: one path, two status codes.

## What this plan does not do

- **Slice 3 -- proxy one dedicated per-model endpoint including streamed
  responses.** No HTTP server, no proxying, no streaming. The health probe is
  a client and reads one status line; it is not the beginning of a server.
- **Slice 4 -- the generic endpoint with swap-on-demand and eviction.** Nothing
  reads the memory estimate, and no child is stopped to make room for another.
  The restart policy deferred by decision 1 belongs here.
- **Slice 5 -- residency and the resident model serving the steward.** The
  residency field is parsed and ignored. No model is held loaded, and nothing
  starts at startup.
- **Slice 6 -- cross-platform evidence and governance onboarding.** The
  cross-platform legs run, and from this slice they run something worth
  running, but the branch protection and baseline onboarding this repository
  needs are separate work.

Also out of scope: graceful termination and the platform dependency it needs
(decision 2), retiring the current external router, and moving any Pi
configuration to per-model endpoints.
