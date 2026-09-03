# Model routing

The vocabulary of serving local models: what the router knows about, and what
each name means. Terms only, no mechanism -- the design specification carries
the architecture, and the code carries the behaviour.

## Language

**Catalog**:
The set of models this router can serve, and the settings they share. One
file, read at startup.
_Avoid_: config, registry, manifest.

**Catalog entry**:
One model in the catalog, with everything needed to serve it.
_Avoid_: record, item, definition.

**Model identifier**:
The name an entry is addressed by, both in the catalog and in the endpoint
that serves it. It names a model, never the role a model happens to fill: the
resident entry is `qwen3-06b` rather than `steward`, because a role name
becomes wrong the moment a second caller uses the same model.
_Avoid_: model name, key, slug.

**Residency**:
Whether a model is held loaded or loaded when something asks for it. Every
entry declares one.

**Resident**:
Loaded at startup and never evicted, so a caller never waits for a load.

**On-demand**:
Loaded on first use and evictable afterwards, competing for what memory the
resident models leave.

**Eviction**:
Unloading an on-demand model to make room for another. Resident models are
never candidates.

**Defaults table**:
The settings entries inherit when they do not state their own. An entry
overrides a default by setting the field; it cannot unset one.

**Flags table**:
Server settings the catalog carries and the router passes through without
interpreting. Their meaning belongs to the server, which is what keeps a
change to that server's flag surface an edit to the catalog rather than to the
code.

**Reasoning preset**:
How a model's reasoning is asked for and read back: the format that delimits
it, and the effort level requested. Only models that reason carry either.

**Memory estimate**:
What loading an entry is expected to cost. An estimate, used to decide what
fits, never a measurement.

**Startup budget**:
How long a model may take to become ready before the router gives up on it and
says so. Per entry, because a small model answers in under a second and a
large one on a cold page cache takes minutes.
_Avoid_: timeout, deadline.

**Models root**:
The directory catalog locations resolve against, supplied at run time. Its
existence is why every location in a catalog is relative, and why one catalog
is correct on every machine.
_Avoid_: model directory, weights path.

**Server binary**:
The `llama-server` executable the router runs. Located, never bundled: a
configured path if there is one, otherwise the first match on the search path.

**Child**:
One running server process, serving exactly one catalog entry on one loopback
port.
_Avoid_: instance, worker, backend.

**Invocation**:
The command line an entry becomes. Named because parity with the current
router is asserted against it directly.
_Avoid_: arguments, command.

**Generic endpoint**:
The endpoint shared by every entry, which takes the model from the request
body. What an OpenAI-compatible client already sends, which is why it exists.
_Avoid_: default endpoint, catch-all.

**Memory budget**:
The ceiling the loaded models are held within. A fact about one machine's
hardware rather than about a set of models, which is why it is not in the
catalog. Absent means nothing is ever unloaded.
_Avoid_: quota, cap.

**Admission**:
Deciding whether a wanted model may be loaded, and what must be unloaded
first. A decision about estimates, taken before anything is started.
_Avoid_: scheduling, allocation.

**Swap**:
Unloading one model so another can be loaded in its place. Named for what the
operator sees: the same address answers with a model it was not holding a
moment ago, and no configuration changed.
_Avoid_: reload, cycle, hot-swap.

**Candidate**:
A loaded model admission may unload: on-demand, and with nothing reading from
it. A resident model is never one, and neither is a busy one.
_Avoid_: victim, target.

**In-flight request**:
A request that has been handed a reference to a child and has not finished
with it. What makes a model busy, and what a decision to unload is not
allowed to interrupt.
_Avoid_: active request, pending request.

**Busy**:
Whether something is currently reading from a loaded model. What makes a model
safe from eviction, because unloading one mid-answer truncates a stream the
caller cannot tell from a finished one.
_Avoid_: locked, in use.

**Readiness**:
Whether a child has finished loading and will answer. Distinct from liveness.

**Liveness**:
Whether a child process still exists. A child can be alive and not ready for
several minutes.

**Public port**:
The one port the router listens on, where callers reach it. Distinct from the
loopback ports children bind, which no caller sees.

**Dedicated endpoint**:
The path shape that names a model, so a request needs no model field to be
routed.
_Avoid_: route, handler.

**Request head**:
The request line and the headers, ending at the first blank line. The only part
of a request the router reads.

**Relay**:
Copying bytes between the caller's connection and the child's, in both
directions, without interpreting them. Named because it is the decision rather
than the mechanism: what the router does not parse, it cannot buffer.
_Avoid_: pipe, forward, tunnel.

**Upstream**:
The connection to the child.

**Downstream**:
The connection to the caller. Used in that pair wherever a failure has to say
which side of the relay it happened on.
