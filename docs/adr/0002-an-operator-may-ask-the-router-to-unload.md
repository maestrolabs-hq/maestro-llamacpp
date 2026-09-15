# ADR 0002: An operator may ask the router to unload

- Status: Accepted
- Date: 2026-09-15

## Context

The router decides residency on its own. A request for an entry that is not
running starts it; an entry nothing has used for the idle window is swept. Both
directions are automatic, and between them they are the whole policy.

That policy has no seat for the person who owns the machine. Making room for
something the router is not going to be asked for -- a bench, another program
that wants the card, a measurement that has to happen now -- means waiting for
the idle window to expire on a model nobody is using.

This is not hypothetical. Benching `heretic38` on 2026-09-15 meant waiting 720
seconds for `gemma4` to age out of a card it was holding and nobody was reading
from. The alternative on offer was stopping the router, which takes every other
entry down with it and is a far larger hammer than the job needed.

The machinery to do it properly already exists and is already careful.
`Slots::unload` takes a slot only when nothing is reading from it, re-reading
that signal at the moment it acts rather than trusting an earlier snapshot, and
reports the entry it refused. Admission calls it to make room. Nothing else can.

## Decision

`DELETE /models/<id>` asks the router to unload one entry.

- **200** when the slot is now empty, whether this emptied it or it was already
  gone. The caller asked for room; the room is there.
- **409** when something is reading from it. A busy entry is never taken, and
  the refusal names it.
- **404** when the catalog has no such entry.

`/models/<id>` with nothing after it is currently a refusal -- the dedicated
shape requires a model *and* a path -- so the spelling collides with nothing.

Two properties make this a smaller decision than adding a mutating endpoint
usually is.

**The surface already mutates residency.** `POST /v1/chat/completions` starts a
model that was not running. That is a side effect on the card, taken on behalf
of an unauthenticated caller, and it is the router's entire purpose. Unloading
is the same authority pointed the other way, and it is the safer direction: the
worst outcome is a model that has to load again.

**It cannot take work away from anyone.** The busy check is the same one
admission relies on, and a refusal is the whole answer -- this never waits for
a reader to finish and never interrupts one.

## Consequences and risks

An operator can free the card without stopping the router, which is what the
720 seconds bought nothing.

The risk is a caller that unloads in a loop and makes every request pay a cold
start. Nothing here prevents that. The bind is the containment: the router
listens on loopback, so the set of callers is the set of local processes, which
is the same set that can already send it a completion.

A second risk is that this is the first endpoint that is not either a proxy or
a read of the catalog, so it is the first place a future operator verb would
naturally go. That is a direction, and directions accumulate. The guard is that
each one has to argue, as this did, that it is authority the surface already
has rather than a new kind.

## When this is reopened

A verb that is not residency. Unloading is the inverse of a load the router
already performs by itself; anything that changes what an entry *is* -- its
flags, its context, its estimate -- is editing the catalog through the wire,
and the catalog is a file under review for reasons that have nothing to do with
this decision.

Also reopened by a bind that is not loopback. The containment argument above is
the bind, and it does not survive the router listening on an address somebody
else can reach.
