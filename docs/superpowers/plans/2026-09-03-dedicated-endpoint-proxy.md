# Dedicated endpoint proxy implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship slice 3 from the specification: listen on the router's public
port, serve one dedicated per-model endpoint, and proxy it to the child that
serves that entry. A streamed response reaches the caller token by token, a
caller that hangs up does not leave a child generating into nothing, and every
refusal names the entry it was about.

**Architecture:** One deep module, `proxy`. Its interface is one type a caller
holds, `Router`, carrying three methods: `bind`, `address`, and `serve`.
Behind it sit the accept loop, reading a request head, rewriting it, starting
a child through slice 2 and keeping it, copying bytes in both directions, and
turning every failure into a status a caller can act on. The seam is unchanged
from slice 2 -- an executable that speaks the server contract -- and this slice
uses more of that contract than the health path.

**Spec:** [model router design](../specs/2026-09-03-model-router-design.md),
slice 3: "proxy one dedicated per-model endpoint including SSE streaming".
Slices 4 to 6 are out of scope and are named at the end.

**Builds on:** [slice 2](2026-09-03-process-supervision.md), which landed
`launch`. This plan is the first caller with a request in flight, which is why
several questions slice 2 deferred -- what happens to a request when a child
dies, and whether a stop can afford to be graceful -- become answerable here.

## The decision this slice turns on

Slice 2 deferred the HTTP dependency to this plan, on the grounds that slice 3
needs streamed responses and should choose against that requirement rather
than inherit a library picked for a status line. That deferral was correct,
and the requirement decides the question more sharply than expected.

The requirement was measured rather than argued. A scratch crate outside this
repository ran a paced upstream -- five Server-Sent Events, one every 150
milliseconds -- behind two proxy designs, with a client recording the elapsed
time at which each event arrived.

| Design | Arrivals |
| --- | --- |
| `tiny_http`, response built from a reader, chunked threshold 1 | 750, 750, 750, 750, 750 ms |
| Byte copy, response never parsed | 150, 300, 450, 600, 750 ms |

The library buffered the whole body to end-of-file and delivered it in one
piece. The byte copy delivered each event as it arrived. Both took the same
total time, which is why total duration is not the property to assert; the
difference is entirely in when the first event arrives.

That is not a complaint about `tiny_http`, and this plan does not claim the
library cannot be coaxed into flushing. It is the reason to prefer a design in
which the question cannot be asked. **A proxy that re-frames a response is a
proxy that can buffer; a proxy that copies bytes cannot.** Streaming stops
being a setting to get right and becomes a property of understanding nothing
about the response.

### What that costs, and what it buys

The router therefore adds **no HTTP dependency**. It reads the request head --
the request line and the headers, which arrive first and are small -- rewrites
it, and from that point copies bytes in both directions without interpreting
them.

What it buys, beyond the streaming property:

- `deny.toml` is untouched. Every candidate examined offers MIT among its
  terms, so the existing `allow = ["MIT"]` would have covered the licences
  either way. `cargo deny check` reports no new licence, no new source and no
  new advisory surface, because there is no new dependency. `cargo machete`
  likewise stays quiet.
- The style is the one already in the tree. Slice 2 is blocking, threaded and
  standard library only. A runtime-based client would have made the router two
  programs in one file.
- Ending the response at end-of-file removes chunked-response parsing from the
  slice entirely, because the upstream connection is asked to close.

What it costs, stated plainly rather than minimised: this repository now
contains hand-written HTTP. That is a real liability and the mitigations are
narrow rather than reassuring. The router binds loopback, serves one machine's
own traffic, reads only a head it bounds in size, and refuses request framing
it does not implement instead of guessing. Nothing here is a general-purpose
server, and the module brief says so, so that the next reader does not mistake
it for one.

The rejected alternatives, and why:

- **`tiny_http`** buffered the response in the measurement above, which is the
  one property this slice exists to preserve. Four transitive dependencies for
  something that would then need working around is a poor trade.
- **`hyper` with a runtime** streams properly and is the correct choice for a
  general server. It brings an asynchronous runtime into a synchronous
  codebase, which is a rewrite of slice 2's supervision, not an addition to
  it. If a later slice needs connection reuse, concurrency limits and
  backpressure at a scale this one does not, that is the slice to pay for it.
- **A client library such as `ureq`** solves the half of the problem that was
  not hard. Forwarding to a loopback child is one connection and one write;
  the difficulty is entirely in not interfering with the response.

One caveat on the scope of that measurement, since the table above invites the
assumption that every alternative was run: only `tiny_http` was. `hyper`, and
`axum` which is built on it, were ruled out on judgement rather than data, and
the judgement is recorded here so that a later reader can disagree with it on
the same terms. Both bring an asynchronous runtime that nothing else in this
crate needs -- slice 2's supervision is blocking and threaded throughout -- so
adopting either means rewriting work that is already landed and green in order
to serve one endpoint on loopback. Nor would a benchmark have settled it. Both
stream correctly when driven correctly, so the contest they would win is the
one this plan already treats as the wrong question. The point of the byte copy
is not that it streams faster; it is that a proxy which never parses a
response has no buffering behaviour to configure, to verify, or to regress. A
larger framework does not strengthen a correctness-by-construction argument,
it exchanges it for a correctness-by-configuration one. If a later slice needs
what those frameworks are genuinely for -- connection reuse, concurrency
limits, backpressure -- that is the slice to pay for the runtime, and to take
the benchmark with it.

## The seam, and why the stub still holds

The seam is where slice 2 put it: an executable that speaks the server
contract. Slice 2 read the health part of that contract; this slice reads the
completions part. The stub gains the ability to emit a paced stream and to die
partway through one, and it is still the second adapter rather than a mock.

Everything the module under test does is real. It binds a port, accepts a
connection, parses a head, opens a socket to the child, writes a rewritten
head, and copies bytes. Nothing in that path is replaced in tests. The stub
only removes the need for a multi-gigabyte model and a graphics card, which is
the same thing it removed in slice 2.

A second fake was considered and rejected. A fake at the connection level
would put the seam in front of the byte copying, and the byte copying is what
this slice exists to get right.

## The red commit, without bypassing a hook

Slice 2 established this and it is reused unchanged. The red commit carries
the tests **and** the module skeleton: every type and signature the tests
name, with `todo!()` bodies. The tree compiles, so `cargo clippy --all-targets`
passes at commit time; the tests run and fail at `todo!()`, so the red state is
real and observed rather than claimed. Skeleton parameters take a leading
underscore, removed in the green commit.

## Global constraints

- The specification is the source of truth. Where this plan departs from it,
  the departure is named in the task and carries its reason.
- Failing test first, and watched. Every behaviour has a test that fails for
  the intended reason before the code that satisfies it exists, and the
  failure text goes in the commit message.
- No gate is weakened, and no hook is bypassed. A blocked check is reported.
- One concern per commit. Conventional commit messages.
- English only in tracked prose.
- No tracked file names one machine.
- Every file opens with a brief. Rust uses `//!`, everything else `#`.
- The router binds loopback only, as children do. A remote bind is a security
  design this repository has not written.
- `just check` passes before every commit.

## Decisions this plan makes

The specification is silent on nine points slice 3 cannot avoid. Each carries
its reasoning.

1. **No HTTP dependency; the response is copied, never parsed.** Measured
   above. The router reads the request head and nothing of the response body.
   This is the decision every other one here follows from.

2. **One upstream connection per request, closed at the end.** The rewritten
   head carries `Connection: close`, so the response ends at end-of-file and
   the relay copies until the socket closes. This removes chunked-response
   parsing from the slice, and Server-Sent Events are unaffected: a stream
   framed by connection close is a stream. Connection reuse is an optimisation
   for a load this router does not have, and it belongs with the slice that
   measures one.

3. **The child starts on first request and is kept for the router's
   lifetime.** Not at bind time, which is residency and belongs to slice 5;
   not stopped afterwards, which is eviction and belongs to slice 4. First
   request pays the startup cost, and every later one finds the child ready.

4. **Concurrency: starting is exclusive, forwarding is not.** The child map
   sits behind one lock. A request takes the lock, finds or starts the child,
   clones the handle out, and **releases the lock before it proxies anything**.
   Two requests to a started entry therefore stream simultaneously, which is
   what `llama-server` supports and what the current router does. Holding the
   lock across the relay would serialise every caller behind the slowest
   stream, which for a generating model is minutes.

   The honest limit: one lock over the whole map means a request that starts a
   child blocks requests for _other_ entries while that start runs. With one
   endpoint in this slice that is unobservable. Slice 4 has several entries and
   eviction, and needs per-entry state; naming it here is cheaper than
   discovering it there.

5. **The request body is forwarded by `Content-Length`, and chunked request
   framing is refused.** A body of a declared length is copied exactly. A
   request carrying `Transfer-Encoding: chunked` gets `501 Not Implemented`
   naming the entry, because implementing chunked request decoding to
   re-encode it upstream is work with no caller: the clients this router
   serves send JSON with a length. A refusal that says what it does not
   implement is better than a body silently mangled.

6. **The `model` field in the request body is passed through untouched.** The
   endpoint already selects the model, and the child is started with
   `--alias <id>`, so it answers under the name the caller used. Rewriting the
   field would mean parsing and re-serialising JSON, which means a second
   dependency and a buffered request body, to change a value the endpoint has
   already decided. Slice 4 routes _by_ that field and is where reading it
   earns its cost. The risk that `llama-server` validates a mismatched field is
   real and Task 7 is where it is checked against the real server.

7. **Error mapping: four failures, four distinct answers.**

   | Failure | Answer |
   | --- | --- |
   | The path names no entry in the catalog | `404`, naming the identifier asked for and listing those the catalog carries |
   | The child does not become ready inside its budget | `504`, naming the entry and the budget it exhausted |
   | The child fails to start, or the upstream connection is refused | `502`, naming the entry and the failure from `launch` |
   | The upstream fails **after** the response has begun | no status is possible; the downstream connection is closed |

   The fourth deserves its own sentence. Once a status line and some bytes have
   been forwarded, a proxy cannot retract them and send `502` instead. Closing
   the connection is the only honest signal, and it is what the caller's client
   library is built to notice. A router that buffered the response in order to
   keep the option of a late error would be trading the property this whole
   slice exists to preserve for a nicer message.

8. **A caller that hangs up closes the upstream connection.** When a write
   downstream fails, the relay stops and drops the upstream socket. Dropping
   closes it, and a closed connection is how `llama-server` is told to stop
   generating. The child itself is untouched: it is shared, and a second
   caller's stream is nobody else's business. Without this a client that
   disconnects mid-answer leaves a model generating tokens into a socket
   nobody reads, for as long as the answer takes.

9. **The public address is `127.0.0.1:8080`, configurable, and loopback is
   enforced.** The specification names one public port. The command takes an
   optional address so a test can ask for port zero and be told which port it
   got, and a non-loopback address is refused with a message saying that
   serving a network is a security design this repository has not written.

## Terms this slice adds to CONTEXT.md

Written into the glossary as they settle, not batched at the end.

- **Public port** -- the one port the router listens on, where callers reach
  it. Distinct from the loopback ports children bind, which no caller sees.
- **Dedicated endpoint** -- the path shape that names a model, so a request
  needs no model field to be routed. _Avoid_: route, handler.
- **Request head** -- the request line and the headers, ending at the first
  blank line. The only part of a request the router reads.
- **Relay** -- copying bytes between the caller's connection and the child's,
  in both directions, without interpreting them. Named because it is the
  decision, not the mechanism: what the router does not parse, it cannot
  buffer. _Avoid_: pipe, forward, tunnel.
- **Upstream** -- the connection to the child. **Downstream** -- the connection
  to the caller. Used in that pair wherever a failure has to say which side of
  the relay it happened on.

---

## Task 1 -- The stub learns to stream

Test infrastructure, and complete before anything depends on it. It ships
green: nothing about the stub is the behaviour under test.

### Steps

- [ ] Extend `src/bin/stub_llama_server.rs` to serve one more path than
      `/health`. Keep the brief accurate: it now speaks two parts of the
      contract, and the reason is unchanged.

- [ ] Add the arguments that drive a stream, following the existing pattern of
      reading known flags and stepping over the rest: `--stream-events N`,
      `--stream-gap MS`, and `--die-after-events N`.

- [ ] Serve a streamed reply on any path ending `/v1/chat/completions`:
      status 200, `Content-Type: text/event-stream`, `Connection: close`, no
      `Content-Length`, then `N` events of the form `data: {"n":I}` separated
      by blank lines, sleeping `--stream-gap` before each and **flushing after
      each**. A stub that buffers its own output would make the streaming test
      vacuous, which is the one way this task can quietly ruin the slice.

- [ ] With `--die-after-events N`, close the connection abruptly after the
      `N`th event without finishing the stream, so the mid-stream upstream
      failure has something to fail against.

- [ ] Serve `/v1/echo`: reply `200` with a plain-text body carrying the
      request line and every header the stub received, one per line. This is
      how the rewriting tests observe what actually arrived upstream, rather
      than asserting against what the router believes it sent.

- [ ] Extend `tests/stub.rs`: a paced stream arrives spread out rather than at
      once when read directly from the stub; `--die-after-events` truncates;
      `/v1/echo` reflects the request line and headers.

### Verification

```sh
cargo test --test stub
```

Expected: passes. The stub's own pacing is observed here, so that when Task 5
asserts the same property through the router, a failure means the router
rather than the stub.

```sh
just check
```

Expected: all five commands pass. `cargo machete` sees no new dependency
because there is none.

## Task 2 -- Failing tests and the module skeleton

Written before any behaviour exists, and run before it exists.

### Steps

- [ ] Add `src/proxy.rs` with a `//!` brief, and `pub mod proxy;` in
      `src/lib.rs`. The brief states the interface: `bind` reserves the public
      port and returns immediately, `serve` runs until the process ends,
      `address` is how a test learns the port it was given, the router reads a
      request head and copies everything else, and it binds loopback only.
      It also says plainly that this is not a general-purpose HTTP server, so
      no future reader mistakes it for one.

- [ ] Write the skeleton: `Router` with `bind`, `address` and `serve`, every
      body `todo!()`, parameters underscored. Reuse `launch::Failure` rather
      than adding a second failure type: nothing branches on the kind, and a
      second one would be a promise to a caller that does not exist.

- [ ] Add `src/proxy/head.rs`, private to the module, with the request-head
      type and its parse and rewrite functions as `todo!()`, and its
      `#[cfg(test)]` unit tests written in full. Pure translation, no input or
      output, so every rule below is asserted directly: the path splits into
      an identifier and a suffix; a path that does not begin `/models/` is
      refused; a path with no suffix after the identifier is refused;
      `Content-Length` is read; `Transfer-Encoding: chunked` is detected; the
      rewritten head keeps the method, carries the suffix as its path, sets
      `Host` to the child's address and `Connection: close`, and preserves
      every other header as received.

- [ ] Add `tests/proxy.rs` driving the public interface with the stub: an
      unknown identifier is `404` and names what it knows; a request reaches
      the child with the prefix stripped, asserted through `/v1/echo`; a
      chunked request body is `501`; an entry whose child never becomes ready
      is `504` naming the budget; an entry whose child cannot start is `502`.

- [ ] Add `tests/streaming.rs` as its own target, carrying the three
      properties that are about time rather than content: a paced stream
      arrives spread out, a mid-stream upstream death truncates rather than
      hanging, and a caller that hangs up closes the upstream connection.
      Separate from `tests/proxy.rs` because these are the tests most likely
      to be flaky on a loaded machine, and a reader who sees this target fail
      should think about timing rather than about routing.

### Verification

```sh
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: clean. The skeleton compiles, which is what lets this commit pass
its own pre-commit hook without `--no-verify`.

```sh
cargo test --test proxy --test streaming 2>&1 | tail -40
cargo test --lib 2>&1 | tail -20
```

Expected: the new tests fail at `todo!()`, and slices 1 and 2 still pass.
Record the exact failure text in the commit message, so the red state is
evidenced rather than claimed.

## Task 3 -- The request head

Pure translation, no input or output. First to turn green, because everything
downstream depends on the rewriting being right.

### Steps

- [ ] Implement parsing: read the request line and headers up to the first
      blank line, **bounded** in both the number of headers and total bytes,
      and refuse anything larger rather than growing a buffer for whatever
      arrives. A router that reads an unbounded head from a socket is a router
      with a memory bug waiting for a bad client.

- [ ] Implement the path split: `/models/<id>/<suffix>` yields the identifier
      and `/<suffix>`. Anything else is refused, with the refusal naming what
      shape was expected.

- [ ] Implement the rewrite: same method, the suffix as the path, `Host` set
      to the child's address, `Connection: close`, and every other header
      passed through as received. Header names are compared case-insensitively,
      because a client is entitled to send `content-length`.

- [ ] Read `Content-Length`, and detect `Transfer-Encoding: chunked` so the
      caller of this module can refuse it.

### Verification

```sh
cargo test --lib
```

Expected: every unit test from Task 2 passes.

```sh
cargo test --all-targets 2>&1 | tail -20
```

Expected: the head tests pass; `tests/proxy.rs` and `tests/streaming.rs` still
fail, because nothing listens yet. A green run here would mean those tests are
not testing what they claim.

## Task 4 -- Listening, entries, and children

### Steps

- [ ] Implement `Router::bind`: refuse a non-loopback address with the reason,
      bind the listener, and return a router carrying the catalog, the models
      root, the located `Server`, and an empty child map. `address` reports
      what the operating system gave, so a test can ask for port zero.

- [ ] Implement `serve`: accept connections and hand each to a thread. One
      thread per connection, because a streamed response occupies its thread
      for as long as the answer takes and this router serves one machine.

- [ ] Implement lookup and start: take the lock, find the entry in the
      catalog, find or start the child, clone the handle, **release the lock**,
      per decision 4. Hold `Arc<Child>` so the child lives as long as the map
      does and dies with the router, which slice 2's `Drop` already guarantees.

- [ ] Implement the four refusals that happen before any byte is forwarded:
      the three from decision 7 -- unknown identifier, start failure, budget
      expiry -- and the chunked request body from decision 5, which Task 3
      detects and this task answers. Each names the entry. Write them as a
      complete small reply with a `Content-Length`, since these are the
      router's own answers rather than something it relays.

### Verification

```sh
cargo test --test proxy 2>&1 | tail -30
```

Expected: the four refusal tests pass, and the forwarding test still fails,
because nothing is relayed yet. The failure has moved from `todo!()` to a
missing reply, which is the evidence that routing works before relaying does.

## Task 5 -- The relay

The heart of the slice.

### Steps

- [ ] Connect upstream to the child's endpoint, write the rewritten head, then
      copy exactly `Content-Length` bytes of request body if there is one.

- [ ] Copy the response with a small buffer, writing and **flushing
      downstream after every read**, until the upstream reaches end-of-file. A
      buffered writer that flushed on a full buffer would batch a stream, which
      is the exact regression `tests/streaming.rs` exists to catch.

- [ ] Stop on a downstream write failure and drop the upstream socket, per
      decision 8. A caller that hung up stops the model generating.

- [ ] Treat an upstream failure after the first byte as decision 7's fourth
      row: no status, close downstream. Nothing is buffered in order to keep a
      late error available.

### Verification

```sh
cargo test --test streaming -- --nocapture
```

Expected: all three pass. The pacing assertion is the one that matters, and it
is written as two assertions, because either alone can be fooled:

- the spread between the first and last arrival is at least half of what the
  stub was asked to produce, which fails if the events are batched;
- the first arrival is no later than half the total production time, which
  fails if the whole body is buffered to end-of-file.

Total elapsed time is deliberately **not** asserted: the measurement in this
plan's opening section shows a buffering proxy and a streaming one finishing at
the same moment, so a duration assertion would have passed against the design
this slice rejects.

```sh
cargo test --all-targets
```

Expected: everything passes.

## Task 6 -- The command, and the documents

A module nothing calls is not a slice.

### Steps

- [ ] Add `model-router serve <catalog> [address]`: parse the catalog, locate
      the binary, bind, print the address and the endpoint shape it serves,
      and run until the process is ended. Unlike `launch`, this one is
      long-running by design, because now there is something to serve.

- [ ] Update `README.md` with the command, its output, an example request
      against a dedicated endpoint, and a sentence saying that a stream is
      passed through as it arrives.

- [ ] Update `CONTEXT.md` with the terms above.

- [ ] Record in `README.md` that the router has no HTTP dependency and reads
      only the request head, so the property is discoverable by someone who
      never reads this plan.

### Verification

```sh
cargo run --quiet -- serve catalog.toml 127.0.0.1:0
```

Expected: prints the bound address and the endpoint shape, and stays up.

```sh
cargo run --quiet -- serve catalog.toml 0.0.0.0:8080
```

Expected: refused, with the message that serving a network is a security
design this repository has not written.

```sh
cargo test --all-targets && just check && git diff --check
```

Expected: everything passes, no whitespace errors.

## Task 7 -- Manual verification against the real server

Continuous integration proves the relay against the stub. This task proves it
against the thing it was written for, and it is the only place decision 6 is
tested. Run by hand, on a machine with `llama-server` and the models, with
`MAESTRO_MODELS_ROOT` pointing at the operator's real models directory rather
than the fallback. Output is pasted into the pull request.

### Steps

- [ ] Export the real models root, run `model-router serve catalog.toml`, and
      confirm it prints its address and stays up.

- [ ] Send a non-streamed completion to `/models/gemma3/v1/chat/completions`
      and record the reply. Confirm the `model` field of the reply carries the
      identifier, which is what `--alias` is for.

- [ ] Send the same request with `"stream": true` and confirm events arrive
      progressively rather than in one piece. `curl --no-buffer` with
      timestamps is enough; the point is to observe it against the real server,
      because the stub's pacing is this repository's own invention and the real
      one is not.

- [ ] Send a request whose body carries a `model` field that disagrees with the
      endpoint, and record what the real server does with it. This is the check
      decision 6 rests on. If it rejects the request, that is a finding and the
      decision is revisited with evidence rather than defended.

- [ ] Interrupt a stream partway through and confirm the server stops
      generating rather than continuing to produce an answer nobody reads.

- [ ] Record the `llama-server` version this was verified against, beside the
      version slice 2 recorded in `catalog.toml`.

### Verification

Each step either produces the recorded output or produces a finding. A step
that is not run is reported as not run, rather than implied.

## Task 8 -- The slice pull request

### Steps

- [ ] Commit in task order, one concern per commit, with the red commit's
      failure text in its message.

- [ ] Push, open a pull request, and include the Task 7 output.

- [ ] Wait for checks: `common / prose`, `common / brief`,
      `common / markdown`, `common / toml`, `common / no-absolute-paths`,
      `common / actions-security`, `common / secrets-scan`,
      `fast / rust-format`, `fast / rust-lint`, `fast / rust-test`,
      `fast / rust-audit`, and both `fast / cross-platform` legs.

- [ ] Merge with a squash and delete the branch, or stop and report the
      failing context with its log excerpt. Do not merge with an override.

### Verification

```sh
gh pr checks --watch --interval 5
```

Expected: every context passes. The cross-platform legs carry particular
weight here: connection teardown is the part of this slice most likely to
differ between platforms, and those legs are the only evidence that the relay
and its cancellation behave the same on Windows and macOS.

---

## Risks

- **Streaming can regress silently, and only one test would notice.** Every
  other test in this repository asserts content, and content is identical
  whether or not a response was buffered. `tests/streaming.rs` is the only
  gate on the property, which makes it the one test in this slice that must
  not be quietly weakened when it turns flaky. If it becomes unreliable on a
  loaded machine, the fix is longer gaps and more generous margins, never a
  softer assertion.

- **The stub's pacing is this repository's own invention.** It emits events on
  a timer because that is convenient to drive; the real server emits them as a
  model produces tokens. If `llama-server` changes how it frames a stream --
  chunked rather than connection-close, say -- continuous integration stays
  green while the real path breaks. Task 7 is the only thing that catches it,
  and the exposure is bounded by how little the relay depends on: it copies
  bytes and never reads the framing.

- **Connection teardown differs between platforms.** A socket dropped on one
  platform may take longer to reach the far end on another, and the
  cancellation test asserts something the operating system has to deliver.
  This is the risk the cross-platform legs exist to expose; if the test proves
  unstable there, the honest response is to assert the router's own behaviour
  rather than the operating system's timing.

- **Hand-written HTTP is a liability with narrow mitigations.** The head is
  bounded, the framing this slice does not implement is refused rather than
  guessed, and the listener is loopback. None of that makes the parser
  general-purpose, and nothing but review keeps it from being extended as
  though it were. The module brief carries the warning.

- **One lock across the child map serialises starts.** Unobservable with one
  entry, and named in decision 4 as slice 4's problem. The risk is that slice
  4 inherits it without noticing, which is why it is written down twice.

- **Decision 6 is unverified until Task 7.** The `model` field is passed
  through on the reasoning that the endpoint has already chosen the model. If
  the real server validates that field, the decision is wrong and Task 7 is
  where that is found. It is listed here so that a failure there is understood
  as a planned check rather than a surprise.

## What this plan does not do

- **Slice 4 -- the generic endpoint with swap-on-demand and eviction.** No
  `/v1` that routes by the body's model field, so nothing reads that field and
  nothing needs a JSON parser. No memory estimate is read, and no child is
  stopped to make room for another. The per-entry state that decision 4 defers
  belongs here, as does the restart policy slice 2 deferred.
- **Slice 5 -- residency and the resident model.** Children start on first
  request, never at startup. The residency field is still parsed and ignored.
- **Slice 6 -- cross-platform evidence and governance onboarding.** The legs
  run, and this slice gives them more worth running, but branch protection and
  baseline onboarding are separate work.

Also out of scope: connection reuse between the router and its children
(decision 2), chunked request bodies (decision 5), graceful termination and the
platform dependency it needs, retiring the current external router, and moving
any Pi configuration onto dedicated endpoints.
