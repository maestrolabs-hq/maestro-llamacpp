# maestro-llamacpp

maestro-llamacpp is the estate's model router. It supervises llama.cpp server
processes and exposes one OpenAI-compatible endpoint per model plus a generic
routing endpoint.

Status: the first slice. The router reads and validates a catalog. It does not
launch a server, open a port, or route anything yet -- those arrive with the
later slices the design names.

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

## Documents

- [Model router design](docs/superpowers/specs/2026-09-03-model-router-design.md)
  -- the architecture, the catalog, and the six slices it ships in.
- [Bootstrap and catalog plan](docs/superpowers/plans/2026-09-03-bootstrap-and-catalog.md)
  -- this bootstrap, and the first slice.
- [ADR 0001](docs/adr/0001-one-crate-until-a-seam-is-real.md) -- why this is
  one crate.
- [AGENTS.md](AGENTS.md) -- how to work in this repository.

## Local commands

```sh
just install    # the toolchain and the gate tools
just setup      # wire the local hooks
just check      # the same commands CI runs, not equivalents
```
