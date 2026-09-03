# Generic endpoint and eviction implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship slice 4 from the specification: serve the generic `/v1`
endpoint, routing each request to a model named by its own body, starting that
model if it is not running and unloading another to make room when it will not
otherwise fit. The acceptance bar is parity: the model entries the operator's
agent already carries keep working with no edit.

**Architecture:** The `proxy` module gains a second endpoint shape and a new
neighbour. A request head now resolves to one of two shapes -- a dedicated
endpoint, which names its model in the path, or the generic endpoint, which
names it in the body. Reading that field means the request body is buffered and
parsed, which is the first thing this router has parsed that it previously
copied; the response relay is untouched and a test proves it. A new deep
module, `admission`, decides what may be loaded: it takes a memory budget, what
is loaded now, and what is wanted, and answers whether the wanted entry fits,
what to unload if it does not, and why it cannot be served if nothing can be
unloaded. That module touches no process and no socket, so its policy is tested
as a pure function.

**Tech Stack:** Rust 2024, toolchain 1.98.0, MSRV 1.85. One new dependency,
`serde_json`, argued below. The response relay stays standard-library byte
copying, as slice 3 left it.

**Spec:** [model router design](../specs/2026-09-03-model-router-design.md),
slice 4: "generic `/v1` with swap-on-demand and eviction". Slices 5 and 6 are
out of scope and are named at the end.

**Builds on:** [slice 3](2026-09-03-dedicated-endpoint-proxy.md), which landed
`proxy` and recorded two things this plan is obliged to settle: that one lock
across the child map serialises starts across unrelated entries, and that the
`model` field in a request body is passed through unread.

## Global Constraints

- The specification is the source of truth. Where this plan departs from it,
  the departure is named in the task and carries its reason.
- Failing test first, and watched. Every behaviour has a test that fails for
  the intended reason before the code that satisfies it exists, and the
  failure text goes in the commit message.
- No gate is weakened, and no hook is bypassed. A blocked check is reported.
- One concern per commit. Conventional commit messages.
- English only in tracked prose and identifiers.
- No tracked file names one machine. In a test, use a synthetic root.
- Every file opens with a brief. Rust uses `//!`, everything else `#`.
- The router binds loopback only, as children do.
- No module exceeds 250 code lines, counted before the first test module.
- `just check` passes before every commit.

---

## What parity means, and how it is asserted

The router this replaces is not a program anyone in the estate wrote. It is
`llama-server` itself, started in its own router mode with a preset catalog,
`--models-autoload`, and `--models-max 1`. Everything the current setup does
about swapping is that server's behaviour, not a policy this estate chose.

Two consequences follow, and they pull in opposite directions.

**The contract is small and must be matched exactly.** The agent's model
entries all name one address, `http://127.0.0.1:8080/v1`, with an
OpenAI-compatible API. What actually crosses that boundary is
`POST /v1/chat/completions` carrying a `model` field, `GET /v1/models`, and
streamed responses. That is the parity surface, and it is asserted three ways:

- by test, on the shapes: a request whose body names an entry reaches that
  entry's child with the path the client asked for, and `GET /v1/models` lists
  what the catalog carries;
- by test, on the streaming property: the generic endpoint delivers a paced
  stream with the same two assertions slice 3 wrote for the dedicated one;
- by hand, in Task 9, against the real server, by pointing the operator's own
  agent configuration at this router with no edit to that configuration.

**The internal policy is deliberately not matched.** `--models-max 1` keeps at
most one model loaded because that server has no idea what any model costs.
This router has a memory estimate per entry and a budget, so it can hold a
small model and a large one at once -- which is the whole reason residency
exists in the design, and it cannot work at all under a count of one. Slice 4
therefore ships a budget policy rather than a count, and says so plainly. A
reader comparing behaviour will see this router keep two models loaded where
the old one kept one. That is the improvement, not a regression, and asserting
parity on the internal policy would have frozen the design out of its own
specification.

---

## The decision this slice turns on

Slice 3 added no HTTP dependency, and the argument was that a proxy which never
parses a response cannot buffer one. That argument was about the response.
Slice 4 has to read a field out of the request, and the same reasoning run
forwards reaches the opposite conclusion.

The router must extract the value of the top-level `model` key from a JSON
object. Searching the bytes for `"model"` is wrong in three ways that a test
written by the person who wrote the search will not catch:

```json
{"messages":[{"role":"user","content":"my model is gemma3"}],"model":"qwen38"}
{"messages":[{"role":"user","content":"say \"model\": \"x\""}],"model":"qwen38"}
{"response_format":{"model":"json"},"model":"qwen38"}
```

The first hides the key inside a string value. The second hides it behind an
escape. The third puts a decoy at a deeper nesting level. Extracting the right
value from all three needs a scanner that tracks string boundaries, escape
sequences and nesting depth -- which is a JSON parser, written here, guarding
the decision of which model answers. A bug in it routes a request to the wrong
model and returns a plausible answer from it. Nothing downstream notices.

So the shape of the two decisions is the same and the answers differ because
the requirements do:

| Slice | Requirement | Consequence |
| --- | --- | --- |
| 3 | The response must **not** be interpreted | Hand-written, because parsing is the risk |
| 4 | The request body **must** be interpreted, correctly | A library, because parsing by hand is the risk |

### The dependency, and what it costs

`serde_json`, version 1. It adds three crates to the tree that were not there
-- `itoa`, `memchr`, `zmij` -- and reuses `serde_core`, which `toml` already
pulls in.

The licence question was measured rather than asserted, on the same footing
slice 3 measured its streaming claim. A scratch crate outside this repository
took `serde_json` and `toml` together and ran `cargo deny check licenses`
against this repository's exact `deny.toml`:

```text
licenses ok
```

Every new crate satisfies the existing `allow = ["MIT"]`: `itoa` is
`MIT OR Apache-2.0`, `memchr` is `Unlicense OR MIT`, and `zmij` is `MIT`.
**`deny.toml` is not widened by this slice**, and a task that finds otherwise
should stop rather than add an allowance.

What it costs, stated rather than minimised: the crate's dependency tree
doubles, from five crates to eight. `cargo machete` must stay quiet, which it
will because the dependency is used. And the router now parses something a
caller sent, which is a class of input handling it did not have before -- so
the body it parses is bounded, exactly as the head has been bounded since
slice 3, and a body larger than the bound is refused rather than allocated for.

Rejected alternatives:

- **A hand-written scanner.** Argued above. Roughly a hundred lines of subtle
  code in the one place a mistake is silent.
- **A smaller JSON crate.** `tinyjson`, `microjson` and their neighbours are
  smaller, and that is the whole of the case for them. The reason to take a
  dependency here is that the parsing is done correctly by something many
  people have already broken and fixed; a less-used crate keeps the dependency
  and gives up the reason for it.
- **Extracting the field without a full parse**, by streaming the body through
  a tokenizer until the top-level `model` key is seen. Correct in principle and
  cheaper in memory, but it is the hand-written scanner again, wearing a hat.

---

## The five problems this slice cannot avoid

### 1. Parsing the request body, and proving the relay is untouched

The generic endpoint reads the `model` field, so the request body is read into
memory, parsed, and forwarded. **The response relay is not touched by this
slice.** Those are two different directions and the distinction is worth
stating flatly, because "the router now buffers" is a sentence that could be
read either way.

- **Request side.** The body is bounded by `Content-Length`, read in full up to
  a limit, parsed, and written upstream from the buffer. A request body is
  complete before it is sent; nothing streams into the router.
- **Response side.** Unchanged. The bytes the child sends are copied and
  flushed as they arrive, with nothing between them and the caller.

The proof is behavioural, not structural. `tests/streaming.rs` gains a test
that drives **the generic endpoint** with the same paced stub and asserts the
same two properties slice 3 wrote: the spread between first and last arrival is
at least half the production time, and the first arrival is no later than half.
If a future change routes the generic endpoint through anything that re-frames
a response, that test fails. Asserting that the code calls `relay::run` would
prove less: the property is that events arrive early and spread out, and that
is what is measured.

The body limit is 8 MiB, and a request declaring more is refused `413` naming
the limit. That is far above a conversation and far below a size worth
allocating on a stranger's say-so. A multimodal request carrying a large
base64 image is the realistic way to approach it, and the refusal names the
limit so the reader knows what to change rather than guessing.

The dedicated endpoint is untouched: it names its model in the path, so it
never reads the body and never pays the limit.

### 2. Per-entry state, replacing the one lock across the map

Slice 3 recorded the limit in its own words: one lock over the whole map means
a request that starts a child blocks requests for *other* entries while that
start runs. A model load is seconds to minutes, so with several entries this is
the difference between a router and a queue.

The fix comes from a property the current code does not exploit: **the set of
entries is fixed when the router binds.** The catalog is parsed before
`Router::bind` and never changes, so the map's keys never change either. A map
whose keys never change needs no lock; only its values do.

```text
slots: HashMap<String, Slot>          // built once at bind, never mutated
Slot { state: Mutex<Option<Loaded>> } // one lock per entry
Loaded { child: Arc<Child>, last_used: Instant }
```

That gives two paths with different costs:

- **A child that is already running** is found by locking only its own entry.
  Nothing global is touched, and a request for one entry cannot be delayed by
  anything happening to another.
- **A child that has to be started** additionally takes one admission lock,
  held across the decision, any eviction, and the start.

The second half looks like the limitation being reintroduced, so here is why it
is not. Loading two models at once is not something to protect: they compete
for the same graphics memory and the same disk bandwidth, and on this
repository's own reference machine two of the catalog's entries cannot both be
resident at all. Serialising loads is the correct behaviour, and doing it under
one lock is also what makes the budget decision sound -- two threads that each
decided independently that their model fits would both be right and the machine
would still be out of memory.

So the limitation slice 3 named is removed where it hurt, kept where it helps,
and the difference is that it is now deliberate. Lookups never take a global
lock; starts serialise on purpose.

### 3. Where the memory budget comes from

**Configured, from the environment, never detected.**

Detection was rejected on three grounds, in order of weight. It reports the
wrong number: a graphics card's total memory is not what is available to this
router, because a desktop compositor, another process, or the operator's own
work already hold some of it. It needs a vendor dependency per platform, and
this repository builds on three. And it fails silently in the direction that
hurts -- a budget detected too high overcommits, and the machine starts swapping
or the driver kills a child, which surfaces as a model that mysteriously died
rather than as a number someone can read.

A configured budget that is wrong is a number in an environment variable that a
person can look at and change. That is the whole argument.

It is read from the environment rather than written in the catalog, and that
placement is not arbitrary. The catalog's own brief says it "describes a set of
models without naming the machine they sit on", which is why every location in
it is relative. A memory budget is a fact about one machine's hardware. Putting
it in the catalog would break the property slice 1 established and the file
states about itself. The per-model estimates stay in the catalog, correctly: a
model's size is a fact about the model, not about where it is loaded.

So the pattern already established by `MAESTRO_MODELS_ROOT` is followed exactly:

| Source | Value |
| --- | --- |
| `MAESTRO_MEMORY_BUDGET_MIB`, set and not empty | used as given |
| otherwise | no budget; nothing is evicted |

An unset budget means **no eviction**, and the `serve` command prints one line
saying so at startup. The alternative -- refusing to serve without a budget --
was rejected because it breaks parity for anyone who has not set it, and
guessing a budget was rejected for the reasons above. An operator who has not
configured a budget gets today's behaviour plus the generic endpoint, and a
line of output telling them what they are not getting.

### 4. What happens to an in-flight request when its child is evicted

**Nothing. A child with a request in flight is never an eviction candidate.**

The mechanism exists and is already proven. A `Child` is held behind an `Arc`,
and `Drop for Child` kills the process. A relay clones that `Arc` and holds it
for the whole response, so removing the child from its slot does not stop it:
the process ends when the last reference goes, which is when the relay
finishes. `Router::stop` already relies on this, and its own documentation says
so -- "a child still relaying a response is held by that relay and stops when
it finishes".

Slice 4 turns that from an incidental property into a stated rule with a check.
A child is a candidate only when, **while holding that entry's lock**, its slot
holds the only reference to it:

```rust
Arc::strong_count(&loaded.child) == 1
```

Holding the lock is what makes the count trustworthy. A relay can only obtain a
reference by locking the slot first, so a thread that holds the lock and sees a
count of one knows no other thread is about to clone it. Without the lock the
count would be a guess that goes stale between reading it and acting on it.

That reasoning is a rule the type has to keep, not an observation about today's
code, so it is stated as an invariant of the slot type. Slice 1 did the same
for the path type, whose constructor refuses an anchored path; the difference
is that a path type can enforce its invariant and this one cannot. Nothing in
the compiler stops a later change from cloning the `Arc` somewhere else, which
is precisely why the rule has to be written where that change would be made.

The wording below is the invariant. The task that builds the slot type carries
it into the doc comment verbatim:

```text
# Invariant

A reference to a loaded child is obtainable only by locking its slot. Every
path that hands out an `Arc<Child>` -- the fast path, the slow path, and
`Router::stop` -- clones it while holding this lock and never before. That is
what makes `Arc::strong_count` a sound busy signal: a thread that holds the
lock and sees one reference knows no other thread is about to take a second.
A change that clones the `Arc` by any other route breaks eviction safety
silently -- a busy child becomes a candidate, and its process is killed under
a live response.
```

The failure it describes is silent, which is the reason it earns a stated
invariant rather than a comment. A child killed under a live response does not
raise anything: the caller sees a stream that stops early, which is
indistinguishable from a model that finished.

The consequence is stated rather than hidden: **eviction does not free memory
immediately if the evicted child is busy** -- which is exactly why a busy child
is not a candidate. Only idle children are unloaded, and their memory is freed
before the new child is started, because the `Arc` is dropped inside the
admission lock.

### 5. Two concurrent requests that together exceed the budget

The smallest honest behaviour: **unload idle candidates until the wanted entry
fits; if it still does not fit, refuse `503` naming the entries that hold the
memory.**

The admission lock makes the sequence deterministic. Two requests for two
entries do not race: the first takes the lock, decides, unloads what it may,
starts its child and releases. The second then takes the lock and decides
against the state the first one left. There is no window in which both believe
they fit.

Deadlock is avoided by construction rather than by care. There is one admission
lock, it is taken before any slot lock during a start, and it is never held
across a relay. A single lock order exists, so no cycle can form. This is worth
naming because the obvious alternative -- per-entry locks alone, with one
request holding entry A while it reaches for entry B to evict it, and another
doing the mirror image -- deadlocks the router on the first collision, and the
symptom is a router that stops answering with no error anywhere.

The two rejected alternatives:

- **Queue the request until room appears.** An unbounded queue is a hang
  wearing a queue's clothes. A bounded one needs a wait timeout, which is a
  second budget nobody has configured and which would be confused with the
  startup budget that already exists.
- **Evict anyway.** Killing a live stream to serve another caller is exactly
  what problem 4 forbids, and the caller whose answer vanished has no way to
  know why.

A refusal is immediately actionable: it names what is holding the memory, the
client can retry, and nothing that was working stops working.

---

## The seam, and why the stub still holds

Unchanged from slices 2 and 3: the seam is an executable that speaks the server
contract, and the stub is the second adapter at it. This slice needs the stub to
answer as more than one model, which it already can -- the router starts one
child per entry, and the stub does not care which entry it was started for.

The stub gains nothing in this slice except what Task 1 adds for observing
which child answered.

The eviction policy is tested **without any process at all**. `admission` takes
numbers and returns a decision; the budget and the estimates are catalog values,
not measurements, so a test drives them directly and exhaustively. That is the
point of putting the policy in its own module: the part that is hard to get
right is the part that needs no child, and the part that needs a child is a
lookup and a start that slices 2 and 3 already proved.

## The red commit, without bypassing a hook

Slices 2 and 3 established this and it is reused unchanged. The red commit
carries the tests **and** the module skeleton: every type and signature the
tests name, with `todo!()` bodies. The tree compiles, so
`cargo clippy --all-targets` passes at commit time; the tests run and fail at
`todo!()`, so the red state is real and observed rather than claimed. Skeleton
parameters take a leading underscore, removed in the green commit.

## Terms this slice adds to `CONTEXT.md`

Written into the glossary as they settle, not batched at the end. `CONTEXT.md`
is a glossary: these say what a word means, never how it works.

- **Generic endpoint** -- the endpoint that serves every model at one path, and
  learns which one a request wants from the body it carries. Distinct from a
  dedicated endpoint, which learns it from the path. *Avoid*: catch-all,
  default endpoint.
- **Memory budget** -- how much memory the router may hold models in at once.
  A property of the machine, supplied at run time, never of the catalog.
  *Avoid*: memory limit, capacity, quota.
- **Eviction candidate** -- a loaded model the router may unload to make room.
  Only an on-demand model with nothing reading from it is ever one. *Avoid*:
  victim, target.
- **In-flight request** -- a request whose response has begun and not finished.
  The model answering one is never an eviction candidate.
- **Swap** -- unloading one model and loading another because a request asked
  for the second and both will not fit. Named for what the operator sees, which
  is one model giving way to another. *Avoid*: rotate, cycle.
- **Admission** -- deciding whether a model may be loaded now, and what must be
  unloaded first. The decision, not the loading.

---

## Task 1 -- The stub says which entry it was started as

Test infrastructure, complete before anything depends on it, and it ships
green: nothing about the stub is the behaviour under test.

A test for the generic endpoint has to prove a request reached the child for
`gemma3` rather than the child for `qwen38`. Both are the same stub binary, so
the reply has to carry something that distinguishes them, and the invocation
already carries it: slice 2 passes `--alias <id>`.

**Files:**

- Modify: `src/bin/stub_llama_server/main.rs`, `src/bin/stub_llama_server/reply.rs`
- Test: `tests/stub.rs`

### Steps

- [ ] Read `--alias` into `Options` in `main.rs`, alongside the flags already
  parsed, defaulting to an empty string. Follow the existing pattern exactly:
  a known flag consumes its value, an unknown one does not.

- [ ] Pass the alias through to `reply::answer`, either as a field on the
  struct it already takes or as a second parameter, whichever reads better.
  Keep both briefs accurate.

- [ ] Include the alias in the `/v1/echo` reply, on its own line, as
  `alias: <value>`. That path already exists to let a test observe what reached
  the child rather than trusting what the router believes it sent; this is the
  same purpose extended by one fact.

- [ ] Add a test to `tests/stub.rs` that starts the stub with `--alias gemma3`
  and asserts `/v1/echo` reports it.

- [ ] Commit.

```sh
git add src/bin/stub_llama_server tests/stub.rs
git commit -m "test: let the stub report which entry it was started as"
```

### Verification

```sh
cargo test --test stub
```

Expected: passes, including the new test. The alias is observed here directly
from the stub, so that when Task 6 asserts routing through the router, a
failure means the router rather than the stub.

```sh
just check
```

Expected: all five commands pass. `cargo machete` sees no new dependency,
because Task 5 is where the dependency arrives.

## Task 2 -- The admission policy, as a pure decision

The hardest part of the slice, and it touches nothing. Written first for that
reason: a policy that needs no process can be tested exhaustively, and
everything after this is wiring.

**Files:**

- Create: `src/admission.rs`
- Modify: `src/lib.rs`, adding `pub mod admission;`
- Test: unit tests inside `src/admission.rs`

**Interfaces:**

- Consumes: `catalog::Residency`, and nothing else from this crate.
- Produces: `Budget`, `Loaded`, `Decision`, `Budget::new`, and `Budget::admit`.
  `Router::bind` takes a `Budget` in Task 7, and `main.rs` builds one from the
  environment in Task 8.

`Budget::new(limit_mib: Option<u32>)` is public and separate from the
environment reader Task 3 adds, because a test must be able to state a budget
directly. Environment variables are process-global, and Rust runs tests in
parallel within a target: a test that sets one to drive a behaviour is a test
that fights every other test in its binary. Only `main.rs` reads the
environment.

### Steps

- [ ] Write `src/admission.rs` with its brief, the types below carrying
  `todo!()` bodies, and a `#[cfg(test)] mod tests` holding every case listed.
  The brief states the interface: what a budget is, that a candidate is an idle
  on-demand model, that a resident model is never a candidate, and that the
  decision is returned rather than acted on.

```rust
/// What the router may hold models in at once.
pub struct Budget {
    /// The ceiling in mebibytes, or `None` when none was configured.
    limit_mib: Option<u32>,
}

/// One model the router has loaded, as admission needs to see it.
pub struct Loaded {
    /// Which entry it is.
    pub id: String,
    /// What it was estimated to cost.
    pub memory_estimate_mib: u32,
    /// Whether it may ever be unloaded.
    pub residency: Residency,
    /// Whether anything is reading from it.
    pub busy: bool,
    /// When it last answered, so the coldest is unloaded first.
    pub last_used: Instant,
}

/// What admission decided.
pub enum Decision {
    /// There is room. Start it.
    Fits,
    /// There is room once these are unloaded, coldest first.
    Unload(Vec<String>),
    /// There is not, and this says what is holding the memory.
    Refuse(String),
}
```

- [ ] Write one test per behaviour: with no limit configured anything fits and
  nothing is unloaded, even when far more is loaded than any machine has; with
  room to spare a wanted entry fits and nothing is unloaded; when it does not
  fit the coldest idle on-demand entry is unloaded, and only as many as are
  needed; a resident entry is never unloaded even when idle and sufficient; a
  busy entry is never unloaded even when on-demand and coldest; when every
  candidate is exhausted the decision refuses and names the entries holding the
  memory; an entry already loaded fits without unloading anything even when the
  budget is exhausted, because serving it costs nothing new; and a wanted entry
  whose own estimate exceeds the whole budget refuses immediately rather than
  unloading everything and failing anyway.

- [ ] Run the tests and watch them fail at `todo!()`.

```sh
cargo test --lib admission 2>&1 | tail -30
```

Expected: every new test fails at `todo!()`. Record the exact text in the
commit message.

- [ ] Implement the policy. Sum what is loaded. If the wanted entry is already
  among them, `Fits`. If its estimate alone exceeds the limit, `Refuse` naming
  that. Otherwise take candidates -- on-demand, not busy, not the wanted entry
  -- sorted oldest `last_used` first, and add them to the unload list until the
  sum plus the wanted estimate is within the limit. If the candidates run out
  first, `Refuse` with a message naming the loaded entries and which of them
  are busy.

- [ ] Run the tests and watch them pass.

```sh
cargo test --lib admission
```

- [ ] Commit.

```sh
git add src/admission.rs src/lib.rs
git commit -m "feat: decide what may be loaded and what must be unloaded"
```

### Verification

```sh
cargo test --lib && cargo clippy --all-targets --all-features -- -D warnings
```

Expected: clean. The module has no input or output, so nothing here is timing
dependent and nothing needs a process.

## Task 3 -- The budget, read from the environment

**Files:**

- Modify: `src/admission.rs`
- Test: `tests/memory_budget.rs`, following `tests/models_root.rs`

### Steps

- [ ] Write `tests/memory_budget.rs` asserting that a value in
  `MAESTRO_MEMORY_BUDGET_MIB` is used as given, that an empty value is treated
  as unset, that a value which is not a number is refused with a message naming
  the variable and what it carried, and that unset yields no limit.

  **All four cases go in one test function**, following
  `tests/models_root.rs:26` exactly, including its safety comment and its
  restore of the original value at the end. Environment variables are
  process-global and Rust runs the tests in a binary concurrently, so four
  functions mutating one variable would race and fail for reasons unrelated to
  the code. One function removes the race rather than papering over it with a
  lock. This is also why this target exists at all: it is the only place in the
  slice that touches the environment, and Task 7's eviction tests deliberately
  do not.

- [ ] Run it and watch it fail.

```sh
cargo test --test memory_budget 2>&1 | tail -20
```

Expected: fails to compile, or fails at `todo!()`. Record the text.

- [ ] Implement `Budget::configured`, mirroring `launch::root::models_root`
  exactly: read the variable, treat an empty value as unset, and return
  `Budget::new(None)` when it is absent. A value that will not parse is a
  `Failure::Unavailable` naming the variable and the value, because a budget
  someone tried to set and typoed must not silently become no budget at all.
  This function has exactly one caller, `main.rs`, and Task 7's tests do not
  use it: they call `Budget::new` instead.

- [ ] Run it and watch it pass.

```sh
cargo test --test memory_budget
```

- [ ] Commit.

```sh
git add src/admission.rs tests/memory_budget.rs
git commit -m "feat: read the memory budget from the environment"
```

### Verification

```sh
cargo test --all-targets 2>&1 | tail -20
```

Expected: everything passes. Nothing reads the budget yet; this task only makes
it available.

## Task 4 -- Two endpoint shapes

`src/proxy/head.rs` is at 206 code lines against a limit of 250, and adding a
second path shape would take it over. The shape decision is also a separate
concern from what a head contains, so it moves out rather than being squeezed
in.

**Files:**

- Create: `src/proxy/endpoint.rs`
- Modify: `src/proxy/head.rs`, `src/proxy.rs`

**Interfaces:**

- Produces: `Endpoint`, which `head::parse` returns inside `Head` and
  `answer::to` branches on in Task 6.

### Steps

- [ ] Create `src/proxy/endpoint.rs` with its brief, the type below carrying
  `todo!()`, and its unit tests written in full.

```rust
/// Which endpoint a path addressed, and what the child is asked for.
pub(super) enum Endpoint {
    /// `/models/<id>/<suffix>`: the path names the model.
    Dedicated { id: String, suffix: String },
    /// `/v1/<suffix>`: the body names the model.
    Generic { suffix: String },
    /// `/v1/models`: the router answers this itself.
    Listing,
}
```

- [ ] Cover these cases: `/models/gemma3/v1/chat/completions` is `Dedicated`;
  `/v1/chat/completions` is `Generic`, carrying the whole path as its suffix;
  `/v1/models` and `/v1/models/` are both `Listing`; `/models/gemma3` with
  nothing after it is refused; and a path that is neither shape is refused with
  a message naming both shapes the router serves. Note that `/v1/models` and
  `/models/` are different prefixes and neither contains the other, so no
  ordering trap exists between them.

- [ ] Run the tests and watch them fail.

```sh
cargo test --lib endpoint 2>&1 | tail -20
```

- [ ] Implement the shapes, and move the path split out of `head::parse` onto
  them. `head::parse` keeps reading the method, the headers, `Content-Length`
  and the chunked flag; `Head` carries an `Endpoint` in place of its `id` and
  `suffix` fields; `Head::rewrite` takes the suffix from whichever shape it
  holds and is otherwise unchanged.

- [ ] Run the tests and watch them pass, including every slice 3 head test,
  updated where they named `head.id` directly.

```sh
cargo test --lib
```

- [ ] Confirm the size gate is satisfied by both files.

```sh
cargo test --test standards
```

- [ ] Commit.

```sh
git add src/proxy/endpoint.rs src/proxy/head.rs src/proxy.rs
git commit -m "refactor: name the endpoint a path addressed"
```

### Verification

```sh
cargo test --all-targets 2>&1 | tail -20
```

Expected: everything passes, including all of slice 3's proxy and streaming
tests. This task changes no behaviour: the dedicated endpoint works exactly as
before, and the two new shapes are parsed but not yet served.

## Task 5 -- The model field, and the body that carries it

**Files:**

- Create: `src/proxy/body.rs`
- Modify: `Cargo.toml`, `Cargo.lock`, `src/proxy.rs`
- Test: unit tests inside `src/proxy/body.rs`

**Interfaces:**

- Produces: `body::read`, which `answer::to` calls in Task 6.

### Steps

- [ ] Add the dependency, then confirm the licence position rather than
  assuming it.

```sh
cargo add serde_json@1
cargo deny check licenses
```

Expected: `licenses ok`. If it is not, **stop and report**. Do not add an
allowance to `deny.toml`; this plan measured the outcome above, and a different
one is new information that belongs in the pull request.

- [ ] Create `src/proxy/body.rs` with its brief and `todo!()` bodies. The brief
  states the bound, and why the body is read at all.

```rust
/// The most a request body may declare before it is refused.
pub(super) const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Reads exactly the declared body, and says which model it names.
pub(super) fn read(
    reader: &mut impl std::io::Read,
    content_length: usize,
) -> Result<(Vec<u8>, String), Failure>;
```

- [ ] Write the unit tests, driven from a `Cursor` so no socket is involved: a
  plain body naming a model yields it; a body whose `model` key also appears
  inside a string value yields the real one, using the `"my model is gemma3"`
  case from this plan; a body with an escaped quote before the key yields the
  real one; a body with a nested `model` at a deeper level yields the top-level
  one; a body with no `model` field is refused with a message saying the
  generic endpoint needs one; a body that is not JSON is refused and the
  message says so; a `Content-Length` above `MAX_BODY_BYTES` is refused
  **before** reading, naming the limit; and a body shorter than its declared
  length is refused rather than parsed from what arrived.

- [ ] Run the tests and watch them fail.

```sh
cargo test --lib body 2>&1 | tail -30
```

Expected: every test fails at `todo!()`. Record the text.

- [ ] Implement. Refuse a declared length over the bound before allocating
  anything. Read exactly `content_length` bytes with `read_exact`. Deserialise
  into a struct carrying only `model`, which ignores every other field, and
  return the buffer alongside the name so the caller forwards the bytes it
  already has rather than re-serialising them.

- [ ] Run the tests and watch them pass.

```sh
cargo test --lib body
```

- [ ] Commit.

```sh
git add Cargo.toml Cargo.lock src/proxy/body.rs src/proxy.rs
git commit -m "feat: read the model a request body names"
```

### Verification

```sh
just check
```

Expected: all five pass, including `cargo machete`, which sees the new
dependency used, and `cargo deny check`, which reports the licences clean
without `deny.toml` being touched.

## Task 6 -- Serving the generic endpoint

**Files:**

- Modify: `src/proxy.rs`, `src/proxy/answer.rs`
- Test: `tests/proxy.rs`

### Steps

- [ ] Add tests to `tests/proxy.rs`, with a catalog carrying two entries so
  routing is observable: a request to `/v1/chat/completions` whose body names
  `gemma3` reaches the child started for `gemma3`, asserted through the alias
  line Task 1 added to `/v1/echo`; the same request naming the second entry
  reaches the second child; the path the child is asked for is the caller's own
  path unchanged, because the generic endpoint strips no prefix; a body naming
  an entry the catalog does not carry is `404` listing what it does carry,
  exactly as the dedicated endpoint answers; a body with no `model` field is
  `400`, saying the generic endpoint needs one and that the dedicated endpoint
  does not; `GET /v1/models` lists every entry without starting anything, with
  the test asserting no child is running afterwards; and the dedicated endpoint
  still works unchanged.

- [ ] Run them and watch the new ones fail while slice 3's still pass.

```sh
cargo test --test proxy 2>&1 | tail -40
```

Record the text.

- [ ] Branch on the `Endpoint` in `answer::to`. `Dedicated` keeps the existing
  path exactly. `Generic` reads the body through `body::read`, looks the name
  up in the catalog, and from there is identical to the dedicated path, except
  that the buffered body is written upstream instead of being copied from the
  reader. `Listing` is answered by the router from the catalog and returns
  without touching any child.

- [ ] Implement the listing as a complete reply with a `Content-Length`, like
  the other answers the router gives itself, serialised with `serde_json`, one
  element per catalog entry in catalog order:
  `{"object":"list","data":[{"id":"<id>","object":"model","owned_by":"maestro-llamacpp"}]}`.

- [ ] Run the tests and watch them pass.

```sh
cargo test --test proxy
```

- [ ] Commit.

```sh
git add src/proxy.rs src/proxy/answer.rs tests/proxy.rs
git commit -m "feat: route a request by the model its body names"
```

### Verification

```sh
cargo test --all-targets 2>&1 | tail -20
```

Expected: everything passes. Eviction is not wired yet, so every entry asked
for is simply started and kept -- which is slice 3's behaviour extended to a
second way of naming a model.

## Task 7 -- Per-entry slots, and eviction

The task the previous six were arranged to make small.

**Files:**

- Modify: `src/proxy.rs`
- Test: `tests/eviction.rs`

**Interfaces:**

- Consumes: `admission::{Budget, Loaded, Decision}` from Task 2.

### Steps

- [ ] Write `tests/eviction.rs` as its own target, because a reader who sees it
  fail should think about the policy rather than about routing. Every test
  states its budget with `Budget::new` and its estimates in the catalog text,
  so nothing here measures memory and nothing here touches the environment.
  Cover: two entries whose estimates
  both fit are both loaded at once and both answer; a second entry that does
  not fit causes the first to be unloaded and the second to answer, asserting
  the first child's port stops answering by polling rather than sleeping,
  following `stopping_a_router_ends_the_children_it_started`; **a child with a
  stream in flight is not unloaded**, driven by starting a long paced stream on
  the first entry, requesting the second from another thread, and asserting the
  first stream arrives complete and intact -- this is the end-to-end half of
  problem 4's invariant, and the unit test beside the slot type is the other;
  when the only entry that could be unloaded is busy, the second request is
  refused `503` and the message names the busy entry; and with no budget
  configured both entries load regardless of their estimates and nothing is
  unloaded.

- [ ] Run them and watch them fail.

```sh
cargo test --test eviction 2>&1 | tail -40
```

Record the text.

- [ ] Add a `Budget` parameter to `Router::bind`, after `server`, and hold it
  in `Shared`. The router accepts its budget rather than reading one, so a test
  states the budget it means directly and `main.rs` remains the only place the
  environment is read. Update the existing callers in `main.rs` and
  `tests/support/mod.rs`; `serving` passes `Budget::new(None)` unless a test
  asks otherwise, which keeps every slice 1 to 3 test behaving exactly as it
  does now.

- [ ] Replace the child map with slots. Build `HashMap<String, Slot>` in
  `Router::bind`, one per catalog entry, each holding
  `Mutex<Option<Loaded>>`. The map is never mutated afterwards, so it needs no
  lock of its own. Add the admission lock beside it, a `Mutex<()>` whose only
  job is to serialise starts. `Router::stop` clears every slot, dropping every
  `Arc` the router holds, which is what it did before through the map.

  Carry problem 4's invariant into the slot type's doc comment verbatim, under
  an `# Invariant` heading, matching how `RelativePath` in
  `src/catalog/path.rs` carries its own. This is the rule the compiler cannot
  keep, so the type has to say it: a reference to a loaded child is obtainable
  only by locking its slot.

- [ ] Add the unit test that states the invariant, beside the slot type:
  `a_slot_reports_busy_while_a_reference_it_handed_out_lives`. Put a `Loaded`
  into a slot and assert it reads idle; clone the `Arc` the way a relay does
  and assert the same slot now reads busy; drop the clone and assert it reads
  idle again. It needs no server and no port -- the point is not that the
  count works, which is standard-library behaviour, but that a reader who
  changes how references are handed out finds a test named for the rule they
  are about to break.

- [ ] Implement the fast path: look up the slot, lock it, and if it holds a
  live child, update `last_used`, clone the `Arc`, release the lock and relay.
  No global lock is taken. Confirm liveness with `Child::check` while the lock
  is held, so a child that exited is not handed to a relay that will fail on
  it.

- [ ] Implement the slow path: take the admission lock, gather what is loaded
  by locking each slot in turn and reading its estimate, residency, busy flag
  (`Arc::strong_count == 1` means idle) and `last_used`, then ask
  `Budget::admit`. On `Fits`, start the child and store it. On `Unload(ids)`,
  lock each named slot and take the `Loaded` out, dropping the `Arc` so the
  process ends before the next one starts, then start the wanted child. On
  `Refuse(message)`, answer `503` with the message, having started nothing.
  Release the admission lock before relaying anything; it is never held across
  a response.

- [ ] Run the tests and watch them pass.

```sh
cargo test --test eviction -- --nocapture
```

- [ ] Commit.

```sh
git add src/proxy.rs tests/eviction.rs
git commit -m "feat: unload an idle model to make room for another"
```

### Verification

```sh
cargo test --all-targets && just check
```

Expected: everything passes. Slice 3's streaming tests are the ones to watch:
they exercise the fast path through the new slot structure, and a failure in
them means the relay was disturbed by this task, which it must not be.

## Task 8 -- The command, and the documents

A module nothing calls is not a slice.

**Files:**

- Modify: `src/main.rs`, `README.md`, `CONTEXT.md`

### Steps

- [ ] Call `admission::Budget::configured` in `serve`, pass the result to
  `Router::bind`, and print one line stating what it found: the configured
  value, or that no budget is set and nothing will be evicted. This is the only
  place in the crate that reads `MAESTRO_MEMORY_BUDGET_MIB`.

- [ ] Update `serve` to print the generic endpoint alongside the dedicated one.

- [ ] Update `README.md` with the generic endpoint and how it routes, the
  budget variable in the same table style as `MAESTRO_MODELS_ROOT`, what
  eviction does and what it will never do, and the three new refusals -- `400`
  for a missing model field, `413` for an oversized body, and `503` when
  nothing can be unloaded. Say plainly that the response relay is unchanged and
  still copies bytes.

- [ ] Update `CONTEXT.md` with the six terms above, as a glossary and nothing
  more.

- [ ] Add this plan to the document list in `README.md`.

- [ ] Commit.

```sh
git add src/main.rs README.md CONTEXT.md
git commit -m "docs: describe the generic endpoint and the memory budget"
```

### Verification

```sh
cargo run --quiet -- serve catalog.toml 127.0.0.1:0
```

Expected: prints the bound address, both endpoint shapes, and the budget line.

```sh
cargo test --all-targets && just check && git diff --check
```

Expected: everything passes and no whitespace errors. `cargo test --test
documents` matters here, because this task adds links.

## Task 9 -- Manual verification against the real server

Continuous integration proves this against the stub. This task proves it
against the thing it was written for, and it is the only place parity is really
tested. Run by hand on a machine with `llama-server` and the models, with
`MAESTRO_MODELS_ROOT` pointing at the operator's real models directory rather
than the fallback. Output is pasted into the pull request.

Record the `llama-server` version, beside the one slices 2 and 3 recorded in
`catalog.toml`.

### Steps

- [ ] Export the real models root and a budget large enough for one large entry
  but not two, run `model-router serve catalog.toml`, and confirm it prints the
  address, both endpoints and the budget.

- [ ] Request `GET /v1/models` and confirm the reply lists the catalog's
  entries.

- [ ] Send a non-streamed completion to `/v1/chat/completions` with a body
  naming `gemma3`, and confirm the reply's `model` field carries that name.

- [ ] Send the same request with `"stream": true` and confirm events arrive
  progressively rather than in one piece. `curl --no-buffer` with timestamps is
  enough; the point is to observe it against the real server.

- [ ] **The real swap.** With `gemma3` loaded, send a request naming `qwen38`,
  whose estimate makes both together exceed the budget. Confirm `gemma3` is
  unloaded, `qwen38` loads and answers, and record how long the swap took. Then
  send a `gemma3` request again and confirm it swaps back. This is the step the
  whole slice exists for.

- [ ] **Parity, unmodified.** Point the operator's own agent configuration at
  this router without editing it -- the entries already name
  `http://127.0.0.1:8080/v1` -- and confirm a real conversation works against
  at least two different models in the same session. This is the acceptance bar
  in its plainest form: if an existing entry needs an edit, parity was not
  reached.

- [ ] Interrupt a stream partway through and confirm the server stops
  generating.

- [ ] Record what a request naming an unknown model returns, and what one with
  no `model` field returns.

### Verification

Each step either produces the recorded output or produces a finding. A step
that is not run is reported as not run, rather than implied.

## Task 10 -- The slice pull request

### Steps

- [ ] Commit in task order, one concern per commit, with the red commits'
  failure text in their messages.

- [ ] Push, open a pull request, and include the Task 9 output.

- [ ] Wait for checks: `common / prose`, `common / brief`,
  `common / markdown`, `common / toml`, `common / no-absolute-paths`,
  `common / actions-security`, `common / secrets-scan`, `fast / rust-format`,
  `fast / rust-lint`, `fast / rust-test`, `fast / rust-audit`, and both
  `fast / cross-platform` legs.

- [ ] Merge with a squash and delete the branch, or stop and report the failing
  context with its log excerpt. Do not merge with an override.

### Verification

```sh
gh pr checks --watch --interval 5
```

Expected: every context passes. The cross-platform legs carry particular weight
for this slice: eviction stops a process and starts another immediately
afterwards, and how quickly a stopped process releases its port is exactly the
kind of thing that differs between platforms.

---

## Risks

- **A timing assertion can measure the operating system rather than the
  router.** Slice 3 hit this: a first-arrival assertion was measuring child
  spawn, which is roughly 100 milliseconds on Linux and 370 on Windows, and the
  Windows leg failed for a reason the relay had nothing to do with. The fix was
  a warm-up request, and it is a rule for this slice too: **any assertion about
  when something happens must warm the child first, or measure something that
  does not include a spawn.** The eviction tests are the exposed ones, because
  they start a second child by definition. Where a swap has to be timed, time
  it from the moment the first child stops answering, not from the request.

- **`Arc::strong_count` is a sound signal only under the slot lock.** The busy
  check in problem 4 is correct because a relay can only obtain a reference by
  locking the slot first. If a later change hands out a reference by some other
  route, that reasoning silently stops holding and a busy child becomes
  evictable. This is the one rule in the slice that nothing enforces: the
  compiler is indifferent, and the end-to-end tests keep passing because a
  busy child is only killed when the budget happens to be tight. Problem 4
  states it as an invariant of the slot type, the task that builds that type
  carries the wording into its doc comment, and
  `a_slot_reports_busy_while_a_reference_it_handed_out_lives` puts it in front
  of a reader who is looking at tests rather than at documentation. Three
  placements for one rule is deliberate: a reader arriving from any of the
  three directions meets it.

- **A budget is an estimate, and estimates are wrong.** The catalog's numbers
  are what someone typed. A model that costs more than its estimate will be
  admitted and then fail to load, or worse, load and push something else out of
  memory below the router's notice. The specification already carries this
  under "VRAM accounting is estimation, not measurement"; slice 4 makes it
  load-bearing for the first time. The mitigation is that a failed start is
  reported with the entry's name, not that the number is right.

- **Eviction makes the router's behaviour depend on history.** Until this
  slice, the same request produced the same work every time. Now it depends on
  what else has been asked for and when. A bug reproducible only after a
  particular sequence is a different class of bug from anything in slices 1 to
  3, and the pure-function shape of `admission` is the mitigation: the policy
  can be replayed exactly from four values without a machine.

- **Buffering the request body is a new class of input handling.** The router
  now allocates for something a caller sent. It is bounded, and the bound is
  refused before allocation, but a large multimodal request is a realistic way
  to reach it and the failure will look like a router problem rather than a
  size problem. The refusal names the limit for that reason.

- **`serde_json` doubles the dependency tree.** Five crates become eight. The
  licence position was measured and is clean, and the crate is the most
  scrutinised JSON implementation available, but the tree is no longer small
  enough to hold in the head. `cargo deny` and `cargo machete` are what keep it
  honest.

- **Parity is asserted by hand, once.** Everything continuous integration knows
  about parity is what this repository's own tests assert against its own stub.
  The only evidence that the operator's existing configuration works unchanged
  is Task 9, run by a person. If that step is skipped, the slice's stated
  acceptance bar is unproven, whatever the checks say.

## What this plan does not do

- **Slice 5 -- residency and the resident model.** No entry is started at bind
  time. The `residency` field is read by the admission policy, which already
  refuses to make a resident entry an eviction candidate, so slice 5 adds
  starting them and nothing else: **it introduces no exception to the policy
  written here.** That was deliberate, and it is why the candidate filter is
  written as "on-demand and idle" rather than "not the one we want".
- **Slice 6 -- cross-platform evidence and governance onboarding.** The legs
  run, and this slice gives them more to prove, but branch protection and
  baseline onboarding are separate work.

Also out of scope, each with the reason it was left:

- **Restarting a child that died.** Slice 2 deferred a restart policy to "the
  slice that has a request in flight", and slice 3 deferred it again. This
  slice detects a dead child on the fast path and starts a fresh one, which is
  a swap rather than a restart: there is no backoff, no crash-loop limit, and a
  child that dies on every start will be started again on every request. A real
  restart policy needs those, and needs to decide what happens to the request
  that was in flight when the child died -- which is still not answerable,
  because that request has already been answered with a truncated response by
  slice 3's decision 7.
- **Unloading a model that nothing has asked for in a while.** Eviction here is
  demand-driven only: nothing is unloaded until something else needs the room.
  An idle timeout is a second policy with its own configuration, and no caller
  has asked for it.
- **Connection reuse between the router and its children**, chunked request
  bodies, and graceful termination -- all still out, all for the reasons slices
  2 and 3 gave.
- **Retiring the external Python router.** The specification's migration stages
  put retirement after parity is proven, and Task 9 proves parity for one
  operator on one machine. Retirement is its own change.
