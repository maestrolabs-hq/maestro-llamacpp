# ADR 0004: The context a client is told is the whole window

- Status: Accepted
- Date: 2026-09-15

## Context

`GET /models` reports `meta.n_ctx` for each entry, taken from the catalog's
`context_size`, which is what the router hands `llama-server` as `--ctx-size`.

In llama.cpp that number is the whole window. The prompt and everything
generated from it share it. A request whose prompt alone reaches `n_ctx` has no
room left to answer, and the server refuses it:

```json
{"code":400,"message":"request (133726 tokens) exceeds the available context
size (131072 tokens), try increasing it","type":"exceed_context_size_error",
"n_prompt_tokens":133726,"n_ctx":131072}
```

On 2026-09-15 an agent using the router hit this and could not recover. The
detail that matters is *why* it could not: compaction works by sending the
conversation to a model and asking for a shorter one. Once the conversation on
its own fills the window, the one operation that would rescue it no longer
fits. The client had compacted at 131,069 tokens against an `n_ctx` of 131,072,
and the compaction request came to 133,726.

The client had copied `meta.n_ctx` into its own `contextWindow` and then spent
it as a prompt budget. Nothing in the reported field says it is not one.

## Decision

`meta.n_ctx` stays the whole window, unreserved. It is llama.cpp's number, it
is what the child was started with, and a client that wants to know the window
is asking about the window.

The reserve is the client's, and it is not optional. A caller that fills the
window it was told about has no room to answer, and a caller that plans to
compact needs room for the compaction pass on top of that.

This repository states the semantics rather than hiding them:

- `n_ctx` is the total, the same figure `--ctx-size` received.
- A client budgets its prompt at `n_ctx` minus room to generate, minus room
  for whatever it does when the prompt gets too long.

The router does not publish a second, smaller number for clients to use
instead. Two numbers where one is authoritative is how the first mistake
happened; a client reading `n_ctx` and subtracting is doing arithmetic it
already has to do for every other provider it speaks to.

## Consequences and risks

The semantics are now written down where somebody wiring a client to this
router will read them, which is the only durable fix -- the machine-local
correction that went with this decision was editing one client's configuration,
and that does not survive the next client.

The risk is that this is a convention and not a gate. Nothing here stops the
next client from copying the total into a prompt budget and rediscovering the
deadlock. A gate would mean the router refusing a request whose prompt leaves
less than some reserve, and that is the router inventing a policy on behalf of
callers whose generation lengths it does not know.

What makes that acceptable is the failure mode: the server already refuses, in
one message, naming both numbers. The refusal is clear. What was missing was
anyone having said what `n_ctx` counts.

## When this is reopened

A client that cannot be configured. If something worth using reads `n_ctx` and
has no way to express a reserve, the choice is between not using it and the
router publishing a budget. At that point the second number earns its keep, and
it should be named for what it is -- a suggested prompt budget -- rather than
quietly replacing the window.
