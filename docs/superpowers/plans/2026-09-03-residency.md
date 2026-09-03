# Residency implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship slice 5 from the specification: hold the resident entry loaded
so the caller it exists for never waits for a load. A resident is loaded when
the router starts serving, is never evicted, and counts against the budget for
as long as it is held. A resident that cannot load says so at startup and does
not take the rest of the catalog down with it.

**Architecture:** No new module. `proxy` gains one startup step and one
reporting method; `admission` and `catalog` are unchanged. The seam is
unchanged from slice 2 -- an executable that speaks the server contract -- and
this slice starts children through the same `Slots::child` path a request uses,
which is what keeps a resident load and a request for that resident from
racing each other.

**Spec:** [model router design](../specs/2026-09-03-model-router-design.md),
slice 5: "residency and the steward resident model". Slice 6 is out of scope
and is named at the end.

**Builds on:** [slice 4](2026-09-03-generic-endpoint-and-eviction.md), which
landed `admission` and `Slots`. Most of residency landed with it, which is what
makes this plan short.

## How little of this slice is new

Residency was designed in slice 1 and enforced in slice 4. Before writing any
code, read what already exists, because the temptation in this slice is to
rebuild policy that is already there and tested.

| Part of residency | Where it already lives |
| --- | --- |
| The catalog parses `residency` | `src/catalog.rs`, `Residency::parse` |
| A resident is never an eviction candidate | `src/admission.rs`, the candidate filter |
| A refusal names a resident as `, resident` | `src/admission.rs`, `holders` |
| Residents count against the budget | `src/proxy/slots.rs`, `held` reports every occupied slot |
| Both rules are unit-tested | `a_resident_entry_is_never_unloaded`, `when_every_candidate_is_exhausted_the_decision_refuses_and_says_why` |

Four things are genuinely new, and they are the whole slice:

1. Residents are loaded when `serve` starts, rather than on first request.
2. A resident that cannot load is reported at startup, and the router serves on.
3. The catalog's resident entry is renamed and repointed at a file that exists.
4. The guarantees in the table above are verified through the router rather
   than only in unit tests, because a rule proven in a pure function and a rule
   proven against running processes are different claims.

## The consequence that matters

A resident holds its estimate against the budget permanently, and nothing can
unload it. That interacts with the largest entry in a way the operator must
see before this lands.

The manual verification for slice 4 ran with `MAESTRO_MEMORY_BUDGET_MIB=25000`.
`qwen38` is estimated at 24576 MiB. Adding any resident at all makes that entry
permanently unservable under that ceiling:

| Resident estimate | Resident + `qwen38` | Under a 25000 MiB ceiling |
| ---: | ---: | --- |
| 4096 MiB | 28672 MiB | refused, permanently |
| 2560 MiB | 27136 MiB | refused, permanently |

This is not a defect in the arithmetic; it is the arithmetic working. A
resident is memory the router promises never to reclaim, so the ceiling has to
cover the resident *plus* the largest model expected to run beside it. The
operator's budget must rise to at least `resident + 24576` for the pair to
coexist, and the startup line should make the reservation visible rather than
leave it to be discovered by a refusal under load.

The single ceiling also cannot express where the memory sits. A resident kept
on the processor holds system memory and competes for no video memory at all,
but the budget counts one number for both. That is recorded as a risk below and
settled by measurement in Task 5, not decided by assertion here.

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
- `just check` passes before every commit.
- **No test in this slice starts more than two children, and none sweeps a
  range.** The Windows continuous integration leg is already near fifteen
  minutes, and Windows spawns a child roughly three times slower than Linux.
  The stub loads instantly, so every test here is cheap by construction;
  anything that needs a real model belongs to Task 5.

## Decisions this plan makes

The specification says residents are "loaded at startup and never evicted" and
stops there. Six points follow from that sentence and are not in it.

1. **Residents load on a thread, and the accept loop starts immediately.**
   `Router::bind` documents that it returns without starting anything, so
   loading belongs to `serve`. Loading before accepting would be smaller by a
   thread, and is rejected: `bind` already reserved the port, so a client
   connects successfully into the kernel's backlog and then hangs. A resident
   carrying the default startup budget would make that a five-minute hang that
   looks like a live router. A refusal is honest; a hang is not.

2. **The startup loader calls the same path a request calls.** It goes through
   `Slots::child`, not around it. A request for the resident arriving mid-load
   blocks on the admission lock, and the re-check under that lock finds the
   child running -- so the race is already handled and this slice adds no
   coordination of its own. The cost is that a slow resident load delays other
   loads, which is the cost slice 4 recorded as correct rather than as a
   limitation: two loads at once compete for the same memory.

3. **A resident that fails to load starts the router degraded, and says so.**
   Refusing to start would let one bad entry deny service to every other model:
   a missing resident file would take `qwen38` down with it, which is a worse
   outcome than the one it prevents. Per-request honesty already exists -- a
   missing model file answers 502 naming the path, verified by hand in slice 4
   -- so what is new is only that the operator learns at startup instead of
   when the first caller arrives. The startup output names the entry and the
   reason, and the router serves everything else.

4. **`Router` gains a method reporting which entries are loaded, by
   identifier.** Without it, "the resident was loaded at startup" cannot be
   asserted: any request that would observe the child is also a request that
   would have loaded it. The method locks each slot, collects identifiers, and
   **hands out no `Arc`** -- which is what keeps it clear of the slot invariant
   in `src/proxy/loaded.rs`. That invariant fails when a handle is cloned
   outside the lock, and warns against exactly this: "return one from a future
   endpoint that lists what is loaded -- and the count stops answering that
   question". Returning names is not returning handles, and the distinction is
   the reason this is safe. No endpoint is added; that is out of scope and
   named at the end.

5. **The resident entry is renamed to the model it now points at.** The catalog
   currently declares `qwen3-06b` at `llm/qwen/qwen3-0.6b/Qwen3-0.6B-Q8_0.gguf`,
   a path that does not exist on the operator's machine -- which is the 502 the
   slice 4 manual verification recorded. The operator's rule is to reuse what is
   present and download only what is not, and what is present is
   `llm/qwen/qwen3-4b/Qwen3-4B-Q4_K_M.gguf`, 2382 MiB of instruction-tuned
   weights. So the entry becomes `qwen3-4b`. The rename is mandatory rather
   than cosmetic: `CONTEXT.md` holds that an identifier names a model and never
   a role, and an entry called `qwen3-06b` serving a four-billion-parameter file
   is a name that lies.

6. **The resident's estimate is stated, not inherited.** The default is 4096
   MiB and the entry will carry 4096 MiB, which looks redundant and is not: a
   resident's cost is load-bearing for every admission decision the router will
   ever make, and a figure that must be read out of a defaults table to be
   known is a figure that will be got wrong. `gemma3` states its own for the
   same reason.

### The recorded caveat, and what settles it

The operator reports that this exact file failed to load once under the
previous Python router, undiagnosed. It is not traceable in that router's
configuration: `models.ini` carries no entry for it, so it was never served
there as a catalog entry. That leaves the report standing and unexplained, and
Task 5 is the only thing that can settle it. Both outcomes are in scope:

- **It loads.** Record the measured load time, and tighten
  `startup_timeout_seconds` in a later edit if the default of 300 seconds is
  far from what it needs.
- **It genuinely cannot load.** That is a finding, not a defeat. Record what
  the server said, then apply the operator's rule in order: what is not present
  gets downloaded. Fetch Qwen3-0.6B, name the entry for it, and the plan's
  shape is unchanged -- only the identifier and the path differ.

## Terms this slice adds to CONTEXT.md

None. `CONTEXT.md` already carries **Residency**, **Resident**, **On-demand**
and **Eviction**, defined when the catalog first parsed them. That the
vocabulary needs nothing is itself a measure of how much of this slice already
exists.

## Task 1 -- Failing tests for what is new

Three behaviours, three tests, each failing for its own reason before any of
them has code.

### Steps

- [ ] `tests/residency.rs`. A resident entry and an on-demand entry, both
      backed by the stub, served through the existing `budgeted` harness.

- [ ] `a_resident_entry_is_loaded_before_any_request_reaches_it`. Serve, wait
      for the loader, and assert the router reports the resident as loaded and
      the on-demand entry as not -- without having sent either of them a
      request. This is the test decision 4 exists for.

- [ ] `a_resident_that_cannot_load_leaves_the_rest_of_the_catalog_serving`.
      Point the resident at a location the models root does not carry. Assert
      that a request for the on-demand entry is answered normally, which is the
      whole claim of decision 3.

- [ ] `a_resident_holds_its_room_against_an_on_demand_entry`. A budget that
      fits the resident and not both. Assert the on-demand request is refused,
      that the refusal names the resident, and that the resident is still
      loaded afterwards. This is the guarantee `admission` already proves in a
      unit test, proven again through running processes.

### Verification

`cargo test --test residency` fails, and each failure names the behaviour that
is missing rather than a compile error. Paste the three failure lines into the
commit message.

## Task 2 -- Loading residents at startup

### Steps

- [ ] `Router::loaded`, returning `Vec<String>`. Locks each slot, collects the
      identifiers of the occupied ones, clones no handle. The doc comment says
      why it returns names rather than handles, referring to the slot
      invariant.

- [ ] A private function that walks the catalog's resident entries in order and
      calls `Shared::child` on each, collecting what failed.

- [ ] `Router::serve` spawns that function on a thread, then enters the accept
      loop as it does today. The brief on `serve` gains a sentence: residents
      load in the background, so the router answers while they do.

- [ ] Each failure prints one line naming the entry and the reason. Each
      success prints one line naming the entry and how long it took, because
      the operator has no other way to learn what a cold load costs.

### Verification

The three tests from Task 1 pass. `just check` passes. No test starts a third
child.

## Task 3 -- The reservation is visible at startup

The budget line printed by `serve` says what the ceiling is. It does not say
that part of it is already spoken for, which is the fact the table above shows
is easy to get wrong.

### Steps

- [ ] The startup output states the total resident reservation beside the
      budget, and what that leaves for everything else.

- [ ] When the residents alone exceed the budget, that line says so plainly.
      No new policy: `admission` already refuses the second resident and names
      the first, and this is only the reporting of it.

### Verification

A test asserting the reservation line for a catalog with one resident. Run
`model-router serve` against the repository's own `catalog.toml` with a
deliberately small budget and confirm the line reads correctly.

## Task 4 -- The catalog entry, and the documents

### Steps

- [ ] Rename `[models.qwen3-06b]` to `[models.qwen3-4b]`, repoint `path` at
      `llm/qwen/qwen3-4b/Qwen3-4B-Q4_K_M.gguf`, state
      `memory_estimate_mib = 4096`, and keep `residency = "resident"` and the
      existing flags. The comment on the entry records that the file is 2382
      MiB of weights and that the estimate covers context on top.

- [ ] Leave `n-gpu-layers` inherited from `[defaults]`, which puts the resident
      on the video device. Named here rather than left silent, because it is
      what makes the reservation compete with `qwen38` for the same memory.
      Task 5 measures the alternative; changing it before there is a
      measurement would be guessing.

- [ ] `README.md`: residency in one paragraph -- what a resident costs, that it
      is never reclaimed, and that the budget must cover the resident plus the
      largest model expected beside it.

- [ ] Do **not** touch `maestro-pi-config`. Its steward specification names the
      endpoint `/models/qwen3-06b/v1` and will need amending to whatever
      identifier Task 5 settles on. Record that as a follow-up in the pull
      request; it is another repository and another change.

### Verification

`model-router check catalog.toml` reports four models -- a rename changes what
an entry is called, not how many there are. `just check` passes.

## Task 5 -- Manual verification against the real server

Continuous integration proves this against a stub that loads instantly, which
is exactly the property a resident exists to avoid needing. This task is the
only place the slice meets a real load, and the only thing that can settle the
recorded caveat. Run by hand, on a machine with `llama-server` and the models,
with `MAESTRO_MODELS_ROOT` pointing at the operator's real models directory
rather than the fallback. Output is pasted into the pull request.

### Steps

- [ ] Export the real models root and a budget that covers the resident plus
      `qwen38`. Run `model-router serve catalog.toml` and record the startup
      output in full, including the resident's load time and the reservation
      line.

- [ ] Confirm the resident answered a completion **without** a load delay, and
      compare against the first request to `gemma3`, which pays for its load.
      That difference is the whole point of the slice; if it is not visible,
      that is a finding.

- [ ] Request `qwen38` and confirm it loads beside the resident rather than
      being refused. Then confirm the resident is still loaded and still
      answering, which is the guarantee that no unit test can prove.

- [ ] Lower the budget below `resident + qwen38` and confirm the refusal names
      the resident as the holder. This is the arithmetic from the table, met in
      person.

- [ ] Record whether the resident ran on the processor or the video device, and
      what each cost. This is what settles the risk that one ceiling counts two
      kinds of memory.

- [ ] Record the `llama-server` version, beside the versions the earlier slices
      recorded in `catalog.toml`.

### Verification

Each step either produces the recorded output or produces a finding. A step
that is not run is reported as not run, rather than implied. If the resident
cannot load at all, follow decision 5's second outcome: record what the server
said, fetch Qwen3-0.6B, rename the entry for it, and re-run this task.

## Task 6 -- The slice pull request

### Steps

- [ ] Push the branch, open the pull request, and paste Task 5's output into
      it, including any step that was not run.

- [ ] Name the follow-up for `maestro-pi-config` explicitly, with the final
      identifier.

- [ ] Wait for every check. If one fails, report which and why rather than
      working around it.

### Verification

Checks green, review requested before merge rather than after.

## Risks

- **One ceiling counts two kinds of memory.** `MAESTRO_MEMORY_BUDGET_MIB` is a
  single number, and a resident held on the processor competes for none of the
  video memory the large entries need. The budget cannot express that, so an
  operator running the resident on the processor must set a ceiling covering
  both or accept that the arithmetic is pessimistic. Measured in Task 5, not
  decided here.

- **The estimate is still an estimate.** A resident that costs more than its
  figure is admitted, fails to load, and is reported at startup -- which is the
  same failure mode slice 4 recorded, arriving earlier and more visibly.

- **A hung stop blocks admissions.** `kill` and `wait` run under the slot and
  admission guards, recorded as a limitation in slice 4. Residents are never
  evicted, so that path does not newly apply to them; but the startup loader
  holds the admission lock, so a resident whose start times out and whose stop
  then hangs blocks every other load for as long as the hang lasts. Carried
  forward, not fixed.

- **Nothing is left running by a failed start.** `Server::start` stops the child
  before reporting a timeout, a child that exits while loading is already gone,
  and a spawn that fails produced none. Nothing is written into a slot on
  failure, so nothing is held and nothing is accounted. Stated here because it
  was asked, and because the answer being "already handled" is worth recording
  rather than rediscovering.

- **The recorded load failure is unexplained.** It is not traceable in the
  previous router's configuration, so Task 5 is the first real evidence either
  way. The fallback path is written into decision 5 so that outcome costs a
  download rather than a redesign.

## What this plan does not do

- **No endpoint listing what is loaded.** `Router::loaded` is a method, not a
  route. An endpoint is a public interface with its own shape and its own
  compatibility promise, and nothing needs one yet.

- **No signal handler.** A router ended by a signal still leaves its children
  running; that is recorded in `README.md` and in `serve`'s brief, and it needs
  a dependency and a Windows job object. It is a change of its own and grows no
  more urgent here: a resident is exactly as orphanable as an on-demand child
  was yesterday.

- **No eviction of residents under pressure.** A resident that will not fit is
  refused and reported. Reclaiming one would make residency a preference rather
  than a guarantee, which is not what the specification says it is.

- **No slice 6.** Cross-platform continuous integration evidence and governance
  onboarding are the next slice, and nothing here anticipates them.
