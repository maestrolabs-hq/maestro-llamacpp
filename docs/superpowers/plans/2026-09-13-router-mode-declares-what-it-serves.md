# The router-mode surface says what it serves

## Two things a client could not learn from us

A llama.cpp client in router mode reads `/models` and decides from it which
models to offer and what may be sent to them. It reads four fields and no
others: `status`, `source`, `status.failed`, and
`architecture.input_modalities`. We were not filling the last one, and we were
listing entries the client had no way to recognise as unsuitable.

**Entries that generate nothing were being offered as chat models.** `embed`
and `rerank` appeared in the client's model list. Selecting either can only
fail: an embedding server answers one forward pass and returns a vector, a
reranker scores a pair. The client's own filter is:

```js
modelIsSelectable = (model, routerAutoload) =>
  model.status.value === "loaded" || model.status.value === "sleeping" ? true
  : routerAutoload && model.status.value === "unloaded"
    && !model.status.failed && model.source === "preset"
```

None of `status`, `source` or `failed` can carry "this is not a chat model"
without lying about one of them -- reporting `failed` for a healthy entry, or a
`source` it does not have. So the entry is left out of this surface rather than
described wrongly on it.

**Entries with a projector were offered as though they had none.** The client
reads `architecture.input_modalities` and nothing else before deciding whether
an image may go in the request. Absent, it assumes text. Seven entries name a
projector and all seven were being offered text-only.

## The change

Two predicates on `Entry`, in a new `catalog::capability`:

- `generates()` -- false when the flags start the server to embed or rerank.
  Read from the flags because the file cannot say it: the same weights serve
  either way, and only the way it is started decides which.
- `accepts()` -- `["text", "image"]` when a projector is named, else
  `["text"]`. Naming a projector is the whole condition.

`/models` filters on the first and reports the second. `/v1/models` is
unchanged and still carries everything.

That split is the point. `/models` is a menu -- what this router will serve a
client that wants to talk. `/v1/models` is a catalogue -- what exists, by name,
for a caller that already knows what it wants. Filtering both would make the
embedder unreachable rather than merely unoffered, so there is a test for each
direction and the second fails if the first is copied onto it.

`capability` is deliberately not `estimate::served`, which reads two of the
same flags. That one decides what an entry costs the device; this one decides
what a caller may send it. They read alike today only because llama.cpp spells
both with one switch, and they would part company the moment a server grew a
mode that kept no cache and still generated.

## Why this belongs in the router

Both facts were previously worked around in one client's own configuration.
That fixes them for that client and no other, and it puts knowledge in the
client that only the catalog has -- the catalog is the thing that names the
projector and sets the flags. Reported here, every client reading the surface
gets the answer.

## Done looks like

- `/models` carries only entries that generate; `/v1/models` still carries all.
- Every entry on `/models` carries `architecture.input_modalities`.
- Five new tests, each watched failing first or, where written after the code,
  confirmed by mutating the implementation until it failed.
- `just check` green.
