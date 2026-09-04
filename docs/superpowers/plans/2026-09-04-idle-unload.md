# Idle unload implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give memory back to the machine while the router keeps serving. An
on-demand model that nothing has asked for in longer than a configured window
is unloaded; the endpoint stays up, and the next request for it loads it
again. A resident is never touched, and a machine that sets no window keeps
today's behaviour exactly.

**Architecture:** No new module in the sense that matters. `admission` gains a
second pure policy function beside `Budget::admit`; `proxy` gains a reaper
thread beside the resident loader it already starts; `proxy::loaded` has one
take path generalised rather than a second one added. `catalog` is unchanged,
and no endpoint is added.

**Spec:** [model router design](../specs/2026-09-03-model-router-design.md).
This slice is **not** one of the six the specification orders, and that
departure is argued in the next section rather than assumed.

**Builds on:** [slice 4](2026-09-03-generic-endpoint-and-eviction.md), which
landed `admission`, `Slots` and the take-under-one-lock rule this slice
depends on entirely, and [slice 5](2026-09-03-residency.md), which landed the
startup thread this slice's reaper is modelled on.

## Why this is a slice at all

The founding specification names six slices and this is none of them. It is
proposed because operating the router surfaced a gap the specification did not
anticipate, and the evidence is on the record rather than asserted here.

The specification describes eviction as on-demand models competing "for the
remaining memory budget under an explicit eviction policy". That policy, in
`Budget::admit`, is driven entirely by a *wanted* entry: it is asked whether
something new may be loaded, and answers by naming what must go first. It has
no other trigger. A router that is merely idle -- nothing wanted, nothing
loading -- never calls it, so nothing is ever unloaded.

The consequence measured on the operator's workstation: a router left serving
held 7.29 GiB for one resident and 4.55 GiB for one on-demand model, for a
total near twelve gigabytes, with no request outstanding and none expected. A
budget does not help. The budget is a ceiling on what may be held at once, not
an instruction to hold less than it when nothing needs the room.

So the gap is structural rather than a defect: eviction answers "what must go
so this can load", and nothing answers "this has not been wanted for an hour".
Closing it is a policy addition, which is why it earns a slice rather than a
patch.

## The two rules this slice must not break

Both already exist and are already proven. This slice reuses them; the whole
risk is in reusing them wrongly.

| Rule | Where it lives | What proves it |
| --- | --- | --- |
| A resident is never unloaded | `src/admission.rs`, the candidate filter | `a_resident_entry_is_never_unloaded` |
| A busy model is never unloaded | `src/proxy/loaded.rs`, `take_if_idle` | `a_child_with_a_stream_in_flight_is_not_unloaded` |

The second is the one to be careful with. `src/proxy/loaded.rs` documents why
`Arc::strong_count` may be trusted at all -- every handle is cloned under the
slot's lock and nowhere else -- and `Slots::snapshot` documents why a signal
read in a snapshot may not be acted on later:

> an entry read as idle here can have a reader before the decision reaches its
> slot. That is why `Slots::unload` reads the signal again at the moment it
> acts instead of trusting this.

A reaper is a second reader of that snapshot, so it inherits the same hazard.
The adversary found this class in slice 4 -- recorded there as B1, a
time-of-check-to-time-of-use race between deciding an entry is idle and
emptying its slot -- and the fix was to read the signal again under the one
lock acquisition that empties the slot. This slice does the same, with one
addition the next section names.

## The gap in the obvious design

The obvious design is: snapshot what is loaded, pick the entries whose
`last_used` is older than the window, call `take_if_idle` on each. That design
is wrong, and reviewing it against `take_if_idle` is what shows why.

`take_if_idle` re-reads exactly one signal: whether the child is busy. It does
not re-read `last_used`. For budget eviction that is correct and sufficient --
the room is needed, and an entry that was idle a moment ago and is idle now is
a legitimate candidate however recently it answered. For idle unloading it is
not sufficient, because freshness *is* the policy:

1. the reaper snapshots and sees `gemma3` last used forty minutes ago;
2. a request arrives; `live_child` sets `last_used` to now and clones the
   handle, so the model is busy;
3. the relay finishes; the handle drops; the model is idle again;
4. the reaper reaches `gemma3`'s slot and calls `take_if_idle`, which sees
   nothing reading and empties the slot.

The model was used one millisecond ago and has just been unloaded for being
idle for forty minutes. Nothing is corrupted -- the accounting stays honest
and no stream is truncated, because the busy check did its job -- but the
guarantee "a steadily-used model is never a candidate" is false, and under a
short window a model in continuous use could be unloaded repeatedly.

**So the take path re-reads both signals, under the one acquisition.** Rather
than write a second take function that differs from the first in one
condition -- two subtly different copies of the rule `src/proxy/loaded.rs`
exists to protect, and the shape `tests/duplication.rs` is there to catch --
the existing one is generalised to take the condition it should refuse on.
Budget eviction passes "not busy". The reaper passes "not busy, and not used
since the cutoff". One take path, one place the invariant lives.

## Decisions this plan makes

1. **The reaper takes no admission lock.** It locks each slot in turn and
   nothing else, so idle unloading never delays a load. This is safe in the
   direction that matters: the reaper only ever *removes*, so a concurrent
   `Slots::admit` that snapshotted an entry the reaper has since taken finds
   more room than it counted on, which is conservative rather than wrong, and
   its own `take_if_idle` on the now-empty slot returns true because "an empty
   slot is nothing to take and counts as taken". The reverse direction, a
   reaper adding to what a decision counted, cannot happen because the reaper
   adds nothing. Taking the admission lock would be defensible and is refused
   for a reason: a sweep that blocks admissions makes reclaiming memory
   compete with serving, and the whole point of the slice is that it should
   not be noticed.

2. **The policy is a pure function in `admission`, beside `Budget::admit`.**
   That module's brief says the policy "is the part of eviction that is hard
   to get right, and keeping it a pure function means it can be driven
   exhaustively from four values without a machine, a model, or a clock that
   has to be waited on". Every word of that applies here, and the last clause
   is load-bearing for a time-based rule: `admission`'s existing tests build
   ages with `Instant::now().checked_sub(...)`, so the whole policy can be
   proven without a single sleep.

3. **The window is read from the environment, not the catalog.** As
   `MAESTRO_MEMORY_BUDGET_MIB` is. How long a machine tolerates holding memory
   it is not using is a fact about that machine, and `CONTEXT.md` holds that
   the catalog "describes a set of models without naming the machine they sit
   on". `MAESTRO_IDLE_UNLOAD_SECONDS`: unset or empty means never unload on
   idle, which is today's behaviour exactly; a value that is not a whole
   number is refused rather than read as off, for the reason `Budget` refuses
   one -- a window someone set and mistyped must not silently become no window
   at all.

4. **Idle unloading does not depend on a budget.** The two are independent
   settings answering different questions: a budget says what may be held at
   once, a window says how long unused memory may be held. A machine with no
   budget still wants its memory back. Stated because the natural mistake is
   to hang the window off `Budget`, which would make it unreachable exactly
   where it is most useful -- the workstation with plenty of memory and no
   ceiling set.

5. **The interval is derived from the window, not configured separately.**
   The reaper sleeps for half the window, floored at one second. A second knob
   would let an operator set an interval longer than the window and get a
   guarantee they did not expect, and the derived one is stateable in a
   sentence: a model is held for at most one and a half windows. That sentence
   goes in `README.md`, because a policy whose timing guarantee is not written
   down is a policy that will be reported as a bug.

6. **An idle unload prints one line.** Matching the resident-load lines slice 5
   added, and for the same reason: memory moved, and the operator has no other
   way to learn it. A router that silently unloads is a router whose next cold
   request looks like a fault.

7. **The reaper holds a `Weak<Shared>`, not an `Arc`.** It is the first thread
   in this router that loops forever -- `residents::load` runs once and ends --
   and a forever-loop holding an `Arc<Shared>` keeps that `Shared` alive after
   the `Router` is dropped, which in a test binary means one leaked thread and
   one leaked catalog per router constructed. Slice 3 measured what that class
   of leak costs when it went unnoticed: forty-five stub servers outlived the
   test binaries that started them. The reaper upgrades its `Weak` each tick
   and ends when the upgrade fails, which needs no new signalling.

## Terms this slice adds to `CONTEXT.md`

Two, because both name things the vocabulary cannot currently say.

- **Idle window**: how long a loaded on-demand model may go unused before it
  is unloaded. A fact about one machine, absent by default. _Avoid_: timeout,
  expiry, time to live.
- **Reaper**: the thread that unloads models that have outlived the idle
  window. Named for what it does rather than when it runs, because "the timer"
  says nothing about what it is allowed to touch.

**Eviction** stays as it is and is deliberately not widened to cover this.
`CONTEXT.md` defines it as "unloading an on-demand model to make room for
another", and that definition is exactly what this slice does *not* do: there
is no other model, and no room is being made for anything. Two triggers with
one name would make every sentence about either of them ambiguous.

## Global constraints

- The specification is the source of truth. This slice extends it, and the
  extension is argued above rather than assumed.
- Failing test first, and watched. Every behaviour has a test that fails for
  the intended reason before the code that satisfies it exists, and the
  failure text goes in the commit message.
- No gate is weakened, and no hook is bypassed. A blocked check is reported.
- One concern per commit. Conventional commit messages.
- English only in tracked prose. No tracked file names one machine.
- Every file opens with a brief. Rust uses `//!`, everything else `#`.
- `just check` passes before every commit.
- **No test sleeps for longer than it must, and none starts more than two
  children.** The cross-platform legs are required contexts now, so a slow
  test is a slow merge for everybody. Decision 2 exists to keep the policy
  provable without a clock; the integration tests that must involve real time
  use a window measured in hundreds of milliseconds, and there is exactly one
  of them that waits for a sweep.

## Task 1 -- The policy, proven without a clock

The whole rule, as a pure function, before anything can call it.

### Steps

- [ ] `IdleWindow` in `src/admission.rs`, holding `Option<Duration>`, with
      `new` taking the value directly and `configured` reading
      `MAESTRO_IDLE_UNLOAD_SECONDS`. `new` exists for the reason `Budget::new`
      does: a test states the window it means rather than setting a
      process-global variable every other test in its binary would race
      against.

- [ ] `IdleWindow::expired(&self, loaded: &[Loaded], now: Instant) ->
      Vec<String>`, returning the identifiers to unload. On-demand, not busy,
      and `last_used` older than the window. Coldest first, so the order
      matches `Budget::admit`'s and a reader does not have to wonder whether
      it differs.

- [ ] Unit tests, each built with the existing `checked_sub` helper so no test
      waits on a clock: an on-demand entry older than the window is named; one
      younger is not; a resident older than the window is not; a busy entry
      older than the window is not; an unset window names nothing whatever the
      ages are; several expired entries come back coldest first.

- [ ] Reject a mistyped variable. `configured` returns the same shape of
      `Failure::Unavailable` `Budget::configured` does, naming the variable
      and what to do about it.

### Verification

`cargo test admission` fails first with each case naming its own missing
behaviour, then passes. No test in this task constructs a `Child`, opens a
socket, or sleeps. Paste the failure lines into the commit message.

**If `tests/duplication.rs` names the two `configured` functions**, that is a
true finding rather than a nuisance: both read a whole number from a variable,
treat empty as unset, and refuse garbage. Extract that into one helper they
both call. Do not add an allowlist entry to keep two copies.

## Task 2 -- One take path, reading both signals

The change with the sharpest edge, done before anything sweeps.

### Steps

- [ ] Generalise `take_if_idle` in `src/proxy/loaded.rs` so the caller supplies
      what makes a slot takeable, keeping one function rather than two. Its
      doc comment gains the freshness hazard: a snapshot's `last_used` is as
      stale as its busy signal, and for a time-based policy the staleness is
      the whole question.

- [ ] `Slots::unload` passes "not busy", which is what it passes today. Its
      behaviour must not change, and the slice-4 eviction tests are what say
      so.

- [ ] A unit test in `loaded.rs` beside the two that are there: a slot whose
      child was used after the cutoff is refused and left exactly as it was,
      driven with `Arc<()>` and synthetic instants rather than a process, as
      its neighbours are.

### Verification

`cargo test --test eviction` and `cargo test --test residency` pass unchanged
-- this task must be invisible to them. The new unit test fails first with the
old signature, then passes.

## Task 3 -- The reaper

### Steps

- [ ] `Slots::sweep_idle(&self, catalog: &Catalog, window: &IdleWindow) ->
      Vec<String>`: build the same `held` snapshot admission uses, ask the
      window what has expired, take each under the generalised path from Task
      2, and return what actually went. Returning what went rather than what
      was chosen is the point -- an entry that gained a reader between the
      snapshot and the take stays, and the caller must not report it as
      unloaded.

- [ ] `src/proxy/reaper.rs`, modelled on `residents.rs`: a loop that sleeps for
      half the window, upgrades its `Weak<Shared>`, sweeps, and prints one line
      per entry it unloaded. It ends when the upgrade fails.

- [ ] `Router::serve` spawns it only when a window is configured. No window,
      no thread -- so a machine that has not asked for this pays nothing for
      it, not even a sleeping thread.

- [ ] `Router` gains no new public method. `Router::loaded` already reports
      what is held by identifier, which is what a test needs to watch a model
      leave.

### Verification

`tests/idle_unload.rs`, every entry backed by the stub, at most two children,
with a window of a few hundred milliseconds:

- an on-demand entry goes idle past the window and `Router::loaded` stops
  naming it;
- the next request for it is answered, and `Router::loaded` names it again --
  the endpoint never went away, which is the claim that separates this from
  stopping the router;
- a resident outlives the window and is still named;
- with no window configured, an entry idle far past any window is still named.

## Task 4 -- What the operator is told

### Steps

- [ ] `startup::idle_window(...)` in `src/startup.rs`, beside `budget` and
      `reservation`, saying whether idle unloading is on and with what window,
      or that it is off and which variable turns it on. The existing lines are
      the precedent: a setting an operator must reason about is printed, not
      left to be discovered.

- [ ] `serve` prints it beside the budget line. Its unit test asserts both
      branches, as `the_budget_line_says_whether_anything_is_ever_unloaded`
      does.

- [ ] `README.md`: a short section saying what the window does, that residents
      are exempt, that the first request after an unload pays the load again,
      and the timing guarantee from decision 5 -- at most one and a half
      windows.

- [ ] `CONTEXT.md`: the two terms from above.

### Verification

`just check` passes. Run `model-router serve catalog.toml` with the variable
set and unset, and confirm the line reads correctly both ways.

## Task 5 -- Manual verification against the real server

Continuous integration proves this against a stub that loads instantly, which
hides the only cost the slice has. This task is where that cost is measured.
Run by hand, with `MAESTRO_MODELS_ROOT` pointing at the real models directory.
Output is pasted into the pull request.

### Steps

- [ ] Serve with a short window. Record the startup lines, request `gemma3`,
      record the memory the machine reports while it is loaded.

- [ ] Wait out the window. Record the unload line, and the memory the machine
      reports afterwards. **The number that matters is the second one:** the
      slice's entire claim is that the operating system got the memory back,
      and `Router::loaded` no longer naming the entry does not prove that.

- [ ] Request it again. Record what the reload cost, beside the resident's
      load time from slice 5's Task 5 for comparison.

- [ ] Confirm the resident is still loaded and still answering throughout.

- [ ] Record the `llama-server` version, beside the versions the earlier
      slices recorded in `catalog.toml`.

### Verification

Each step either produces the recorded output or produces a finding. A step
not run is reported as not run rather than implied.

## Task 6 -- The slice pull request

### Steps

- [ ] Push the branch, open the pull request, paste Task 5's output including
      any step not run.

- [ ] Request review before merge rather than after. Ask the reviewer
      specifically at the Task 2 seam: one take path serving two policies is
      the change most likely to be subtly wrong, and it is the one the tests
      are least able to catch on their own.

- [ ] Read the checks once. The cross-platform legs are required contexts and
      take minutes; do not hold a session open watching them.

## Risks

- **The window is guesswork until it is used.** Too short and a model
  thrashes: unloaded, wanted, loaded, unloaded. Too long and the memory is
  held anyway. Nothing here can pick the number, which is why it is a machine
  setting with no default rather than a value this plan asserts. The thrash is
  bounded by decision 5 -- a model cannot be unloaded sooner than one window
  after its last use -- but bounded is not prevented.

- **The reaper sweeps against a snapshot that is already old.** Reduced, not
  removed, by Task 2: what the reaper acts on is re-read under the lock that
  empties the slot, so the outcome is always either a correct unload or no
  unload at all. What remains is that a sweep may do less than it decided to,
  which is reported honestly by returning what went rather than what was
  chosen.

- **Estimates are still estimates.** The router reports the memory it stops
  *accounting* for. Whether the operating system reclaimed what the catalog
  said it would is Task 5's second measurement and nothing else's.

- **A hung stop blocks the reaper.** `kill` and `wait` run under the slot
  guard, recorded as a limitation in slice 4 and unchanged here. A child that
  will not die holds its slot lock, and the reaper blocks on it. Because the
  reaper holds no admission lock (decision 1), it blocks nothing but itself --
  which is a strictly better outcome than the same hang during an admission,
  and is the second reason decision 1 is worth its argument.

- **One more thread on a router that already has one per connection.** The
  reaper is one thread for the life of the process, spawned only when the
  window is set. Named because thread count is the kind of cost that is easy
  to add without noticing, not because this one is large.

## What this plan does not do

- **No idle unloading of residents.** A resident is memory the router promises
  never to reclaim. Reclaiming one after an hour would make residency a
  preference rather than a guarantee, which is not what the specification says
  it is. An operator who wants the steward model reclaimed makes it on-demand.

- **No per-entry window.** The budget is machine-wide with no per-entry
  override and has not needed one; the same argument holds until an entry is
  shown to need a different window from its neighbours. Adding it later is a
  catalog field and a fallback, which is a small change made on evidence
  rather than a guess made now.

- **No unloading on memory pressure from outside the router.** The router acts
  on its own catalog's estimates. Watching the machine's real memory and
  reacting to it is a different feature with a different failure mode, and
  nothing needs it yet.

- **No signal handler, still.** A router ended by a signal leaves its children
  running, recorded in `README.md` and in `serve`'s brief. The reaper makes
  that less likely to matter -- an idle machine ends up holding less -- and
  does not address it. It remains a change of its own.

- **No endpoint reporting what is loaded or when it was last used.**
  `Router::loaded` is a method, not a route, for the reason slice 5 recorded:
  an endpoint is a public interface with its own compatibility promise, and
  handing out handles would break the slot invariant it is careful to keep.
