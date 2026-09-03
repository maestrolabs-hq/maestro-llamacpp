# maestro-llamacpp

maestro-llamacpp is the estate's model router. It supervises llama.cpp server
processes and exposes one OpenAI-compatible endpoint per model plus a generic
routing endpoint.

Status: the third slice. The router reads and validates a catalog, takes one
entry from it as far as a running `llama-server` on a loopback port, and serves
one dedicated endpoint per model, relaying a streamed reply to the caller as it
arrives. The generic endpoint that routes by the body's model field, residency,
and eviction arrive with the later slices the design names.

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

This is deliberately not a long-running command. Staying up until interrupted
needs a signal handler for no gain in a slice with nothing to serve; launching,
proving readiness and stopping is the whole of what this slice claims.

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
a streamed reply is passed through as it arrives
```

An address may be given; loopback is enforced, because serving a network is a
security design this repository has not written.

Each model is reached at its own endpoint, so a request needs no model field to
be routed:

```sh
curl --no-buffer http://127.0.0.1:8080/models/gemma3/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"hello"}],"stream":true}'
```

A child starts on the first request for its entry and is kept for the router's
lifetime, so that request pays the startup cost and every later one finds the
child ready.

**A stream is passed through as it arrives.** The router has no HTTP
dependency: it reads the request head -- the request line and the headers --
rewrites it, and from there copies bytes in both directions without
interpreting them. A proxy that re-frames a response is a proxy that can buffer
it; one that copies bytes cannot, which makes token-by-token delivery a
property of the design rather than a setting to get right.

A caller that hangs up mid-answer closes the connection to the child, which is
how `llama-server` is told to stop generating.

Four refusals happen before anything is forwarded, and each names the entry it
was about:

| Cause | Answer |
| --- | --- |
| the path names no entry the catalog carries | `404`, listing what it does carry |
| the request body announces chunked framing | `501`; send a body with a `Content-Length` |
| the child cannot be started | `502`, with the reason from the launcher |
| the child misses its startup budget | `504`, naming the budget |

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
- [ADR 0001](docs/adr/0001-one-crate-until-a-seam-is-real.md) -- why this is
  one crate.
- [AGENTS.md](AGENTS.md) -- how to work in this repository.

## Local commands

```sh
just install    # the toolchain and the gate tools
just setup      # wire the local hooks
just check      # the same commands CI runs, not equivalents
```
