# Idle unload implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to run this plan task by task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give memory back to the machine while the router keeps serving. An
on-demand model that nothing has asked for in longer than a configured window
is unloaded; the endpoint stays up, and the next request for it loads it
again. A resident is never touched, and a machine that sets no window keeps
today's behaviour exactly.

**Architecture:** Two new modules, forced by the size gate rather than chosen.
`src/idle.rs` holds the policy as a pure function; `src/proxy/reaper.rs` holds
the thread that runs it, modelled on `residents.rs`. `proxy::loaded` has its
one take path generalised rather than a second one added, and `Router::bind`
takes a settings value in place of the bare `Budget` it takes today. `catalog`
is unchanged, and no endpoint is added.

The first draft of this plan said "no new module in the sense that matters".
That was wrong and the gate says so: `admission.rs` and `proxy.rs` have sixteen
lines of headroom each and `slots.rs` eighteen, against the two hundred and
fifty in `tests/standards.rs`. Placement is decided in decision 9 rather than
discovered by a failing gate.

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
the existing one is generalised to take an *additional* condition it should
also refuse on. Budget eviction adds nothing. The reaper adds "and not used
since the cutoff". One take path, one place the invariant lives.

The shape matters and is fixed here: the caller's condition is **additive to a
hard-coded `!busy`**, never a replacement for it. A signature that let a caller
supply the whole predicate would let the third caller -- there is always a third
caller -- pass one that forgets the busy check, and the rule the module exists
to protect would leave the module. The busy check stays written in
`take_if_idle`; callers may only narrow what is takeable, never widen it.

### The second gap: the freshness signal is itself stale

The first gap is about a signal read too early. This one is about a signal that
was never right. `live_child` sets `last_used` **before** it hands out the
handle:

```rust
held.last_used = Instant::now();
Some(Arc::clone(&held.child))
```

Nothing updates it when the relay ends, and `answer::to` holds that handle
across the whole of `relay::run`. So `last_used` records when a request
*started*, not when the model was last busy. A model that streamed for five
minutes under a two-minute window carries a `last_used` five minutes old at the
instant it goes idle, and the next sweep unloads it -- a model that finished
answering one second ago, unloaded for being unused for five minutes.

Task 2 does not help here. Re-reading `last_used` under the lock re-reads the
same wrong value. This is not a race; it is the wrong quantity.

That falsifies both sentences this plan wants to write down: the README's "held
for at most one and a half windows" and the risk section's "cannot be unloaded
sooner than one window after its last use". Decision 8 fixes the quantity rather
than weakening the sentence, because the sentence is the feature: an operator
who reads "unused for an hour" means the model finished answering an hour ago,
not that it began answering an hour ago.

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

   One consequence, stated because the argument above is otherwise one-sided:
   a refusal is built from the same snapshot, so a sweep that empties a slot
   between the snapshot and the refusal can produce a `503` naming an entry as
   holding memory it no longer holds. It is wrong only in its explanation, it
   corrects itself on the next request, and the alternative is the admission
   lock this decision refuses. It is a cost of the decision, not an argument
   against it.

2. **The policy is a pure function, proven without a clock.** `admission`'s
   brief says the policy "is the part of eviction that is hard to get right,
   and keeping it a pure function means it can be driven exhaustively from
   four values without a machine, a model, or a clock that has to be waited
   on". Every word applies here, and the last clause is load-bearing for a
   time-based rule: `admission`'s existing tests build ages with
   `Instant::now().checked_sub(...)`, so the whole policy is provable without
   a single sleep.

   It does **not** live beside `Budget::admit`, because it cannot: decision 9.
   The property this decision is about belongs to the function -- no clock, no
   machine, no process -- and survives the move intact.

3. **The window is read from the environment, not the catalog.** As
   `MAESTRO_MEMORY_BUDGET_MIB` is. How long a machine tolerates holding memory
   it is not using is a fact about that machine, and `README.md`,
   `src/admission.rs` and `catalog.toml` all hold that the catalog "describes a
   set of models without naming the machine they sit on".
   `MAESTRO_IDLE_UNLOAD_SECONDS`:

   - unset or empty means never unload on idle, which is today's behaviour
     exactly;
   - **`0` means the same thing**, stated rather than left to arithmetic. Zero
     is how an operator writes "off" when a variable is already in a script,
     and a zero window read literally expires everything on every sweep --
     permanent thrash from what looks like a disable. It gets its own test;
   - a value that is not a whole number is refused rather than read as off, for
     the reason `Budget` refuses one: a window someone set and mistyped must
     not silently become no window at all.

4. **Idle unloading does not depend on a budget.** The two are independent
   settings answering different questions: a budget says what may be held at
   once, a window says how long unused memory may be held. A machine with no
   budget still wants its memory back. Stated because the natural mistake is
   to hang the window off `Budget`, which would make it unreachable exactly
   where it is most useful -- the workstation with plenty of memory and no
   ceiling set.

5. **The interval is derived from the window, not configured separately.**
   The reaper sweeps every half window, floored at a hundred milliseconds. A
   second knob would let an operator set an interval longer than the window
   and get a guarantee they did not expect.

   Two corrections the first draft got wrong, both about arithmetic rather
   than intent:

   - **It sleeps to a deadline, not for a duration.** `sleep(interval)` after
     a sweep makes the period `interval + sweep`, so the bound drifts by
     however long a sweep takes -- and a sweep that blocks on a stuck child
     drifts without bound. The reaper computes the next wake time and waits
     until it.
   - **The floor is a hundred milliseconds, not one second, and the bound is
     stated with its sweep.** At a one-second floor a one-second window sweeps
     every second, which is a bound of two windows rather than one and a half
     -- the sentence would have been false at the smallest window the variable
     can express. A hundred-millisecond floor only binds below a two-hundred
     millisecond window, which `MAESTRO_IDLE_UNLOAD_SECONDS` cannot express at
     all, so for every configured window the derived interval is exactly half.
     A sweep is a handful of uncontended mutex acquisitions; ten a second is
     not a cost worth a knob.

   The sentence for `README.md` is therefore: **a model is held for at most one
   and a half windows plus one sweep.** It goes there because a policy whose
   timing guarantee is not written down is a policy that will be reported as a
   bug.

6. **An idle unload prints one line.** Matching the resident-load lines slice 5
   added, and for the same reason: memory moved, and the operator has no other
   way to learn it. A router that silently unloads is a router whose next cold
   request looks like a fault.

7. **The reaper ends on a signal, and holds a `Weak<Shared>` as well.** The
   first draft said the `Weak` alone was enough -- upgrade each tick, end when
   it fails, "which needs no new signalling". That does not work, and the test
   harness is where it fails.

   `tests/support/mod.rs` builds the router, wraps it in an `Arc`, and detaches
   a thread holding a clone of that `Arc` to run `serve`, which never returns.
   So the `Router` -- and the `Shared` inside it -- is **never dropped for the
   life of the test binary**. `Weak::upgrade` therefore never fails, the reaper
   never ends, and the slice leaks exactly the thread and catalog per router
   that decision cited the forty-five orphaned stubs to avoid. `Router::stop`
   signals nothing today: it calls `Slots::clear` and returns.

   So: **`Shared` gains a stop signal that `Router::stop` sets**, and the
   reaper waits on it rather than sleeping blind -- a `Condvar` beside a
   `Mutex<bool>`, waited on with `wait_timeout` for the interval from decision
   5. `stop` sets the flag and notifies. Two properties follow, and both are
   wanted:

   - a reaper configured with a one-hour window still ends **at once** when
     `stop` is called, rather than up to half an hour later. `Serving`'s `Drop`
     calls `stop`, so every test in every binary reclaims its reaper;
   - the wait is not a sleep, so nothing needs to be interrupted or timed out
     twice.

   The `Weak` stays, and its job is now precisely stateable rather than
   overstated: it covers the case the signal cannot, a `Router` dropped without
   `stop` ever being called, where nothing remains to set a flag. The signal is
   the mechanism; the `Weak` is the net.

   One correction to the correction, for the record. The review said the claim
   "the first thread that loops forever" is false because the accept loop
   already loops forever. That is true of the loop but not of a thread: the
   accept loop is `serve`'s own body, running on whichever thread called it,
   and it ends when the process does. The precise claim, which is the one worth
   writing, is that the reaper is the first thread this router **spawns** that
   does not end on its own -- `residents::load` runs once and returns. The
   overstatement is removed either way.

8. **`last_used` is touched when the relay ends, not only when it starts.**
   The second gap above: today the field records when a request began, so any
   request longer than the window leaves the model expired the moment it goes
   idle. `Slots::touch(&self, id)` sets it to now, and `answer::to` calls it
   after `relay::run` returns and before the handle drops -- so the model is
   still busy at the moment it is touched, and becomes idle with an honest
   timestamp.

   The alternative was to leave the code alone and restate the guarantee as
   "one window after a request last *started*". That is refused: it is
   surprising in the direction that costs money. A long generation is exactly
   when a model is most in use, and unloading it the instant it finishes is the
   thrash the risk section promises is bounded. The cost of the fix is one
   uncontended mutex acquisition on a path that has just finished network I/O.

   Two edges, both benign and both stated so a reader does not have to wonder:
   a slot emptied during the relay (only `Router::stop` can do that to a busy
   slot) is found empty and touched to nothing; a slot refilled during the
   relay is touched on the new child, which makes a fresh child look fresher
   still.

9. **Where the new code lives, decided against the size gate rather than
   discovered by it.** `tests/standards.rs` counts lines before the first test
   module and allows two hundred and fifty. Measured on this branch:
   `admission.rs` 234, `proxy.rs` 234, `slots.rs` 232, `loaded.rs` 132,
   `residents.rs` 59.

   | What | Where | Why |
   | --- | --- | --- |
   | `IdleWindow` and its policy | **new `src/idle.rs`** | `admission.rs` has sixteen lines; the type with this repository's doc density is seventy-odd |
   | The reaper thread | **new `src/proxy/reaper.rs`** | modelled on `residents.rs`, which is 59 lines and the same shape |
   | The generalised take | `src/proxy/loaded.rs` | 118 lines of headroom, and the invariant must not leave the module that owns it |
   | `Slots::sweep_idle` | `src/proxy/slots.rs` | eighteen lines of headroom, which is the tightest fit in the slice |

   `src/idle.rs` is a sibling of `admission` rather than a split of it:
   turning `admission.rs` into a directory to gain room moves an existing
   module for a new module's benefit, and `mod.rs` would still carry its 234
   lines.

   **The two tight fits, and what happens if they do not fit.** `sweep_idle`
   in `slots.rs` (18 lines) and the `Shared` field, the spawn and `stop`'s
   signal in `proxy.rs` (16 lines) are estimates until they are written. If
   either exceeds its headroom, the room is made by a **mechanical extraction
   in its own commit before the feature commit**, and the extraction is named
   now so it is not invented under pressure: from `slots.rs`, the read-only
   projection trio `snapshot`, `held` and `loaded_ids`; from `proxy.rs`,
   `Shared`'s inherent `impl` block (`child` and `known`). Neither moves a
   decision, and a gate failure is reported rather than an allowlist grown.

10. **The window reaches `Slots` through a settings value, not a sixth
    argument.** The first draft never said how the configured window travels
    from the environment to the thread that uses it, and every obvious route is
    already closed:

    - `Router::bind` takes five arguments today and `clippy.toml` sets
      `too-many-arguments-threshold = 5`. A sixth fails `just check`;
    - reading the variable inside `serve` gives a parse failure nowhere to go.
      `serve` returns `()` and never returns at all, so decision 3's refusal of
      a mistyped window would have to be a panic or a silent "off";
    - setting the variable in tests is refused by the harness on purpose:
      `budgeted`'s brief says a ceiling is stated "at the call rather than
      through the environment -- which is process-global and would race every
      other test in the binary".

    So `bind` takes a small `Limits` value **in place of** the bare `Budget` --
    still five arguments -- carrying the budget and the idle window. `main`
    builds it from the environment *before* `bind`, which is where a parse
    failure can still abort startup with a message, the way `Budget::configured`
    already does. The harness gains a sibling of `budgeted` that injects a
    window directly, exactly as `budgeted` injects a ceiling.

    **This also resolves the sub-second contradiction.** `IdleWindow::new`
    takes a `Duration`, so a test states two hundred milliseconds directly;
    `IdleWindow::configured` parses whole seconds from the variable, because
    that is the granularity an operator needs. The pair mirrors `Budget::new`
    and `Budget::configured` exactly. The integration tests use the injected
    path and never touch the environment.

## Terms this slice adds to `CONTEXT.md`

Two, because both name things the vocabulary cannot currently say.

- **Idle window**: how long a loaded on-demand model may go unused before it
  is unloaded. A fact about one machine, absent by default. *Avoid*: timeout,
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
  test is a slow merge for everybody. Decision 2 keeps the policy provable
  without a clock at all; the integration tests that must involve real time
  inject a two-hundred-millisecond window through the harness path from
  decision 10 -- never through the environment, which they are forbidden -- and
  there is exactly one of them that waits for a sweep.

## Task 1 -- The policy, proven without a clock

The whole rule, as a pure function, before anything can call it.

### Steps

- [ ] `IdleWindow` in **`src/idle.rs`** (decision 9), holding
      `Option<Duration>`, with `new` taking a `Duration` directly and
      `configured` parsing whole seconds from `MAESTRO_IDLE_UNLOAD_SECONDS`.
      `new` exists for the reason `Budget::new` does: a test states the window
      it means rather than setting a process-global variable every other test
      in its binary would race against. It is also what lets an integration
      test use a sub-second window the variable cannot express.

- [ ] `IdleWindow::expired(&self, loaded: &[Loaded], now: Instant) ->
      Vec<String>`, returning the identifiers to unload. On-demand, not busy,
      and `last_used` older than the window. Coldest first, so the order
      matches `Budget::admit`'s and a reader does not have to wonder whether
      it differs.

- [ ] Unit tests, each built with `admission`'s `checked_sub` age helper so no
      test waits on a clock, one for each case listed below these steps.

- [ ] Reject a mistyped variable. `configured` returns the same shape of
      `Failure::Unavailable` `Budget::configured` does, naming the variable
      and what to do about it.

**The cases, and the shape each one takes.** Every protection case follows
`with_one_protected`'s shape: the protected entry beside an unprotected one,
asserting the result is *exactly* the expected set. A case that asserts only
"the resident is absent" passes against a function that returns an empty
vector, and six such cases would go green against a stub while proving nothing.

- an on-demand entry older than the window is named, beside a younger one that
  is not;
- a resident older than the window is not named, beside an on-demand one of
  the same age that is;
- a busy entry older than the window is not named, beside an idle one of the
  same age that is;
- an unset window names nothing whatever the ages are;
- **a zero window names nothing** (decision 3), which is the case that would
  otherwise expire everything on every sweep;
- several expired entries come back coldest first.

### Verification

`cargo test idle` fails first, then passes. The fail-first here is in two
steps, and reporting it as one would be a lie: referencing a type that does not
exist is a compile error (`E0433`), not a per-case failure. So **stub the type
and its method with `todo!()` first**, watch each assertion fail on its own
message, then implement. No test in this task constructs a `Child`, opens a
socket, or sleeps. Paste the failure lines into the commit message.

**If `tests/duplication.rs` names the two `configured` functions**, that is a
true finding rather than a nuisance: both read a whole number from a variable,
treat empty as unset, and refuse garbage. Extract that into one helper they
both call. Do not add an allowlist entry to keep two copies.

## Task 2 -- One take path, reading both signals

The change with the sharpest edge, done before anything sweeps.

### Steps

- [ ] Generalise `take_if_idle` in `src/proxy/loaded.rs` so the caller supplies
      an **additional** condition, keeping one function rather than two. The
      `!busy` check stays written inside `take_if_idle` and is not what the
      caller supplies: a caller may narrow what is takeable, never widen it.
      The signature is therefore an extra predicate over the held value, not a
      replacement predicate -- so the third caller cannot forget the invariant
      the module exists to keep. Its doc comment gains the freshness hazard: a
      snapshot's `last_used` is as stale as its busy signal, and for a
      time-based policy the staleness is the whole question.

- [ ] `Slots::unload` supplies no extra condition, so its behaviour is exactly
      what it is today. The slice-4 eviction tests are what say so.

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

- [ ] `Shared` gains the stop signal from decision 7 -- a `Condvar` beside a
      `Mutex<bool>` -- and `Router::stop` sets it and notifies before or after
      `Slots::clear`, either order, since the reaper takes no admission lock.

- [ ] `src/proxy/reaper.rs`, modelled on `residents.rs`: a loop that waits on
      that signal with `wait_timeout` until the next deadline (decision 5),
      upgrades its `Weak<Shared>`, sweeps, and prints one line per entry it
      unloaded. **It ends when the flag is set or when the upgrade fails**, and
      the flag is the one that fires in practice -- the harness never drops its
      `Router`.

- [ ] `Router::serve` spawns it only when a window is configured. No window,
      no thread -- so a machine that has not asked for this pays nothing for
      it, not even a sleeping thread.

- [ ] `Router` gains no new public method. `Router::loaded` already reports
      what is held by identifier, which is what a test needs to watch a model
      leave.

### Verification

`tests/idle_unload.rs`, every entry backed by the stub, at most two children,
with a two-hundred-millisecond window injected through decision 10's harness
path. Two of these four fail first and two are regression guards, and saying
which is which keeps the fail-first rule honest -- a guard that passes against
a stub proves nothing on the day it is written and everything on the day
somebody changes the policy:

- **fails first:** an on-demand entry goes idle past the window and
  `Router::loaded` stops naming it;
- **fails first:** the next request for it is answered, and `Router::loaded`
  names it again -- the endpoint never went away, which is the claim that
  separates this from stopping the router;
- *regression guard:* a resident outlives the window and is still named;
- *regression guard:* with no window configured, an entry idle far past any
  window is still named.

The reaper must also be shown to end: a test that builds a router with a window
and drops it asserts the thread does not outlive it. Without decision 7's
signal this is the case that hangs.

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
      and the timing guarantee from decision 5 -- **at most one and a half
      windows plus one sweep**, measured from when a request last *finished*
      (decision 8). Two constraints on its wording are below these steps.

- [ ] `CONTEXT.md`: the two terms from above.

**Two constraints on the `README.md` wording.** Do not write "expiry" or
"timeout" -- the vocabulary above rules out both. And do not imply wall-clock
time: the window is measured with `Instant`, which is monotonic and does not
advance while the machine is suspended, so a laptop that sleeps for eight hours
wakes holding everything it was holding. That is the behaviour rather than a
bug, and it is stated in a sentence rather than handled -- suspension is a
different feature with a different failure mode.

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
      and `Router::loaded` no longer naming the entry does not prove that. The
      measurement and its pass condition are below these steps.

- [ ] Request it again. Record what the reload cost, beside the resident's
      load time from slice 5's Task 5 for comparison.

- [ ] Confirm the resident is still loaded and still answering throughout.

- [ ] Record the `llama-server` version, beside the versions the earlier
      slices recorded in `catalog.toml`.

**The measurement, named so the step is falsifiable rather than a feeling.**
Resident set size of the `llama-server` process before the unload
(`ps -o rss= -p <pid>`), and after the unload the absence of that process
(`pgrep -af llama-server`). If the entry was offloaded to a GPU, also
`nvidia-smi --query-gpu=memory.used --format=csv` before and after, because
`free` never sees video memory. **Pass:** the process is gone, and the
recovered resident set size is within twenty per cent of what it held.
Anything less recovered is a finding rather than a pass.

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
  bounded by decisions 5 and 8 together -- a model cannot be unloaded sooner
  than one window after the request it last *finished* -- but bounded is not
  prevented. Decision 8 is what makes that sentence true; without it the bound
  is measured from when a request *started* and a long generation defeats it.

- **The reaper sweeps against a snapshot that is already old.** Reduced, not
  removed, by Task 2: what the reaper acts on is re-read under the lock that
  empties the slot, so the outcome is always either a correct unload or no
  unload at all. What remains is that a sweep may do less than it decided to,
  which is reported honestly by returning what went rather than what was
  chosen.

- **Estimates are still estimates.** The router reports the memory it stops
  *accounting* for. Whether the operating system reclaimed what the catalog
  said it would is Task 5's second measurement and nothing else's.

- **A hung stop blocks far more than the reaper, and this slice gives it a new
  way to happen.** `Child::drop` calls `kill` then `wait` (`src/launch.rs`),
  and the slot guard is still held while it runs -- a limitation recorded in
  slice 4 and unchanged here. The first draft of this plan said the reaper
  "blocks nothing but itself", which is false. A child that will not die
  blocks, transitively:

  1. every request for that entry, which waits on the slot;
  2. **every admission for any entry**, because another entry's `Budget::admit`
     can decide `Unload([stuck])` and then call `take_if_idle` on the stuck
     slot while holding the admission lock -- so nothing loads anywhere;
  3. `Router::loaded`, which locks each slot to build its snapshot -- and
     `support::settled` polls exactly that, so a continuous-integration leg
     **hangs until its timeout rather than failing with a message**.

  The honest statement is therefore: *the same severity as a hang during an
  admission, but newly reachable on a timer, on a machine with no traffic at
  all.* That is a real widening and it is accepted here rather than fixed,
  because the fix -- taking the value out of the slot under the guard and
  dropping it after the guard is released -- changes the take path that slice
  4's tests guard, for a pre-existing defect this slice did not introduce.
  **It is named as the next change so it is not lost**, and decision 1 keeps
  its argument on its own merits rather than on this one.

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

- **No reaping of children that exited on their own.** A child that dies by
  itself leaves its slot occupied and its estimate counted until something
  asks for it, and the reaper is the obvious place to notice. It is left out
  because the exposure is already bounded: `live_child` checks liveness under
  the lock and empties the slot when it finds an exited process, so the stale
  accounting lasts until the next request for that entry rather than forever.
  Widening the reaper from "unused" to "unused or dead" is a second policy
  under one thread, and it earns its own change.

- **No recorded list of what was unloaded.** Decision 6 prints a line, and a
  test proves an unload happened by watching the identifier leave
  `Router::loaded`. `resident_failures` is the precedent for recording instead
  of only printing, and it exists because a resident that fails to load has no
  other observable effect -- an unload does. If a test is ever written that
  must distinguish "the reaper unloaded it" from "it was never loaded", that
  is the moment to add the list, and not before.

- **No drop of the child outside the slot guard.** Named in the risks above as
  the next change this area wants. It is the fix for a pre-existing limitation
  rather than for anything this slice adds, and doing it here would put a
  change to slice 4's guarded take path inside a slice about a timer.
