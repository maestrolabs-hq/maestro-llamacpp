# Model router design

Founding specification for maestro-llamacpp. This document defines the
router's purpose, architecture, catalog, migration path, and delivery order.
This founding commit contains documentation only; no implementation exists yet.

## Purpose

Replace the current Python model router (llama-switchyard, which lives outside
the estate and is ungoverned) with a Rust router that follows the estate
structure. The Python router is the only component of the estate currently
living outside it: it serves the estate's local models while escaping the
estate's gates, governance, and portability rules. This repository brings that
capability inside.

## Architecture

One process, one public port (8080).

Endpoints:

- `/v1` is the generic OpenAI-compatible endpoint. It routes by the request's
  `model` field with swap-on-demand, byte-compatible with today's behavior so
  Pi's existing model entries keep working unchanged.
- `/models/<id>/v1` is a dedicated per-model endpoint. It needs no `model`
  field: the endpoint itself selects the model.

The router supervises llama-server child processes on internal loopback ports
and proxies requests to them, including Server-Sent Events streaming
passthrough for token-by-token responses.

## Residency

Each catalog entry declares residency:

- **resident** models are loaded at startup and never evicted;
- **on-demand** models load on first request and compete for the remaining
  memory budget under an explicit eviction policy.

Residency replaces the earlier idea of separate dedicated server processes: a
resident model's endpoint is always warm, so no second process is needed to
protect it from swaps. The first resident model is a small instruction model
(Qwen3 0.6B class) serving the steward extension defined in the
maestro-pi-config specification.

## Catalog

One TOML catalog, replacing the current `models.ini`. Each model entry
declares:

- identifier;
- GGUF path;
- optional draft model for speculative decoding;
- optional multimodal projector;
- context size;
- reasoning preset (Qwen3 hybrid thinking effort);
- residency;
- memory estimate.

Paths are derived from the environment; no path ever names a machine.

Current models to carry over: `qwen38` with its MTP draft and projector,
`qwen38-semantic` (low reasoning effort preset), `gemma3`, plus the new
resident small model.

## Cross-platform

Windows, macOS, and Linux are all supported targets. The llama-server binary
location is configured or resolved from `PATH`; the router never bundles it.
Process supervision must handle each platform's process semantics.

## Governance

Full estate structure: Rust workspace, the same gates (formatting, lint as
errors, tests, English-only prose, no machine paths, conventional commits),
shared CI workflows, ADRs, and governance onboarding. This founding commit
contains documentation only.

## Migration

Three stages, strictly ordered:

1. **Parity.** The generic `/v1` replicates the current swap behavior; Pi is
   unchanged.
2. **Adoption.** Pi model entries move to per-model endpoints; the steward is
   wired to the resident endpoint.
3. **Retirement.** The Python router is retired after parity is proven; its
   repository remains as reference until then.

## Vertical slices

Each slice ships behind its own tests, strictly ordered:

1. catalog parsing and validation;
2. launch and supervise one llama-server child with health checking;
3. proxy one dedicated per-model endpoint including SSE streaming;
4. generic `/v1` with swap-on-demand and eviction;
5. residency and the steward resident model;
6. cross-platform CI evidence and governance onboarding.

## Honest risks

- SSE proxying under concurrent requests during a swap is the hardest
  correctness problem in the design.
- VRAM accounting is estimation, not measurement.
- Windows process supervision differs materially from the Unix platforms.
- The llama-server flag surface changes across llama.cpp releases, so the
  catalog pins what it depends on.
