# maestro-llamacpp

maestro-llamacpp is the estate's model router. It supervises llama.cpp server
processes and exposes one OpenAI-compatible endpoint per model plus a generic
routing endpoint.

Status: the fourth slice. The router reads and validates a catalog, takes one
entry from it as far as a running `llama-server` on a loopback port, and serves
both a dedicated endpoint per model and a generic endpoint that routes by the
model a request body names, relaying a streamed reply to the caller as it
arrives. It holds models within a configured memory budget, unloading an idle
one to make room for another. Residency arrives with the later slices the
design names.

## Checking a catalog

```sh
model-router check catalog.toml
```

A usable catalog reports what it carries and exits zero:

```text
catalog.toml is valid: 4 models
```

An unusable one names every problem, each by the entry and the field it came
from, and exits non-zero. Every fault is listed rather than only the first, so
one run covers one round of edits:

```text
models.toml is not usable:
  entry 'alpha': field 'colour' is not recognised
  entry 'alpha': field 'path' is required
  entry 'alpha': field 'context_size' must be greater than zero, but is 0
  entry 'beta': field 'residency' must be one of resident or on-demand, but is 'sometimes'
```

`catalog.toml` holds the models this router serves. Every location in it is
relative to a models root supplied at run time, so the file describes a set of
models without naming the machine they sit on.

## Launching one model

```sh
model-router launch catalog.toml gemma3
```

Starts that entry, waits until it answers, then stops it again:

```text
gemma3 is ready at http://127.0.0.1:41273 after 1.4 seconds
gemma3 stopped
```

This is deliberately not a long-running command: it proves a child starts and
answers, then ends it. What `serve` does when it is signalled is a separate
question, answered under [what eviction never does](#what-eviction-does-and-what-it-never-does).

A child that never becomes ready fails when its startup budget expires, and a
child that exits while loading fails with its status rather than waiting the
budget out. Every failure names the entry it came from.

### Where the models are

Catalog locations are relative, and resolve against a models root read at run
time:

| Source | Value |
| --- | --- |
| `MAESTRO_MODELS_ROOT` | used as given, when set and not empty |
| otherwise | `models` under the home directory |

### What fits in memory

| Source | Value |
| --- | --- |
| `MAESTRO_MEMORY_BUDGET_MIB` | the ceiling in mebibytes, when set and not empty |
| otherwise | no budget, so nothing is ever unloaded |

A budget is a fact about one machine's hardware, which is why it is read from
the environment rather than written into a catalog: the catalog describes a set
of models without naming the machine they sit on. A value that is not a whole
number is refused rather than read as no budget at all, because the difference
between those two is whether anything is ever evicted.

The router says which it found at startup.

The server binary is located rather than bundled: `llama-server` is taken from
the search path, with the platform's executable suffix, so no tracked file
names one machine.

## Serving

```sh
model-router serve catalog.toml
```

Binds the public port and stays up:

```text
serving on http://127.0.0.1:8080
  http://127.0.0.1:8080/models/<model>/v1/chat/completions
  http://127.0.0.1:8080/v1/chat/completions   (routed by the body's model)
memory budget: none set, so nothing is ever unloaded (set MAESTRO_MEMORY_BUDGET_MIB)
a streamed reply is passed through as it arrives
```

An address may be given; loopback is enforced, because serving a network is a
security design this repository has not written.

### Two ways to name a model

Each model is reached at its own endpoint, so a request needs no model field to
be routed:

```sh
curl --no-buffer http://127.0.0.1:8080/models/gemma3/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"hello"}],"stream":true}'
```

The generic endpoint takes the model from the request body instead, which is
what an existing OpenAI-compatible client already sends:

```sh
curl --no-buffer http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemma3","messages":[{"role":"user","content":"hello"}],"stream":true}'
```

The path is passed to the child unchanged, and `GET /v1/models` lists the
catalog's entries without starting anything.

Reading the body costs the response nothing. The router parses the request to
learn which model answers; the reply is still copied byte for byte, and both
endpoints deliver a paced stream with the same timing.

A child starts on the first request for its entry and is kept while there is
room for it, so that request pays the startup cost and every later one finds
the child ready.

### What eviction does, and what it never does

With a budget set, a model that does not fit causes the coldest idle on-demand
model to be unloaded first. The budget is a ceiling on the estimates written in
the catalog, never on measured memory: a model that costs more than its
estimate is admitted and then fails to load, and what protects against that is
that a failed start names its entry, not that the estimate is right.

A model is never unloaded while something is reading from it. Killing a child
mid-answer would truncate the stream, which a caller cannot tell apart from a
model that finished early. When the only model that could be unloaded is busy,
the request is refused instead. That is checked at the moment a model is taken
out, not only when the decision is made: a request can arrive in between, and
emptying the slot then would leave a process running that the budget no longer
counts.

Nothing is unloaded for a start that could not have happened anyway. A model
file the models root does not carry is found before the decision, so a stale
path does not cost the operator a warm model as well as the one they asked
for. A start that fails only by being attempted -- a startup budget expiring,
a model costing more than its estimate -- cannot be prevented this way, and
the room is already gone when it does.

**The router does not survive being signalled, and its children do.** `serve`
runs until the process ends, so `stop` is never reached: on `SIGTERM` -- what
`systemctl stop`, `kill` and a container stop all send -- the router dies and
every `llama-server` it started keeps running and keeps its memory. A terminal
interrupt is different, because children are left in the process group
deliberately and `Ctrl-C` reaches them too.

The cost lands on the budget. A restarted router builds its table empty, so it
counts nothing while the orphans still hold real memory, and it will then
admit a full budget of models on top of them. Nothing in the process table
ties a stray server to the router that started it, so after a signalled stop,
look for them:

```sh
pgrep -af llama-server
```

A signal handler needs a dependency and a Windows job object, which is a
change with its own gates to clear rather than a line to add here.

**A stream is passed through as it arrives.** The router has no HTTP
dependency: it reads the request head -- the request line and the headers --
rewrites it, and copies the response back without interpreting a byte of it. A
proxy that re-frames a response is a proxy that can buffer it; one that copies
bytes cannot, which makes token-by-token delivery a property of the design
rather than a setting to get right. The request is the exception, and only on
the generic endpoint: the model is inside the body, so the body is read. What
is forwarded is still the caller's own bytes.

A caller that hangs up mid-answer closes the connection to the child, which is
how `llama-server` is told to stop generating.

Nine refusals happen before anything is forwarded, and each names what it was
about:

| Cause | Answer |
| --- | --- |
| the path names no entry the catalog carries | `404`, listing what it does carry |
| the request body announces chunked framing | `501`; send a body with a `Content-Length` |
| the child cannot be started | `502`, with the reason from the launcher |
| the child misses its startup budget | `504`, naming the budget |
| the body names no model, on the generic endpoint | `400`, saying which endpoint needs none |
| the body is larger than the router will read | `413`, naming both sizes |
| the `Content-Length` will not parse | `400`, quoting back what arrived |
| the generic endpoint is sent no declared body | `411`, naming the header it wanted |
| nothing can be unloaded to make room | `503`, naming what is holding the memory |

Once a response has begun there is no status left to send, so a failure after
that point closes the connection rather than pretending it can still answer.

## Documents

- [Model router design](docs/superpowers/specs/2026-09-03-model-router-design.md)
  -- the architecture, the catalog, and the six slices it ships in.
- [Bootstrap and catalog plan](docs/superpowers/plans/2026-09-03-bootstrap-and-catalog.md)
  -- this bootstrap, and the first slice.
- [Process supervision plan](docs/superpowers/plans/2026-09-03-process-supervision.md)
  -- the second slice.
- [Dedicated endpoint proxy plan](docs/superpowers/plans/2026-09-03-dedicated-endpoint-proxy.md)
  -- the third slice, and the measurement behind its one hard decision.
- [Generic endpoint and eviction plan](docs/superpowers/plans/2026-09-03-generic-endpoint-and-eviction.md)
  -- the fourth slice, and the five design problems it had to settle first.
- [ADR 0001](docs/adr/0001-one-crate-until-a-seam-is-real.md) -- why this is
  one crate.
- [AGENTS.md](AGENTS.md) -- how to work in this repository.

## Local commands

```sh
just install    # the toolchain and the gate tools
just setup      # wire the local hooks
just check      # the same commands CI runs, not equivalents
```
