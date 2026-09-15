# ADR 0003: An estimate is measured, never derived

- Status: Accepted
- Date: 2026-09-15

## Context

Every catalog entry declares `memory_estimate_mib`, and admission compares it
against the budget to decide what can be resident. The number decides whether a
model loads, so being wrong in either direction costs something: too low
overcommits the card, too high refuses a model that would have fitted.

`model-router bench` exists to measure it. Nothing has ever required that it
was used.

On 2026-09-15 `heretic38` was added with an estimate reasoned out rather than
measured. The reasoning was careful and used only measured inputs:
`qwen38-uncensored` had been measured at 26944 MiB at the same size and
context, and the gap between `qwen38` and `turbo38` had measured the cost of an
embedded draft head at about 1166 MiB. The sum landed near 27800, and the entry
declared 28160.

It held 25729. The derivation was out by 2431 MiB -- around 4% of a 32607 MiB
card reserved for nothing -- and the comment beside it, which honestly said
"provisional until `model-router bench` measures it", is the only reason anyone
went back to check.

Two measured parts do not make a measured sum. Whatever the arithmetic missed
-- allocator rounding, a KV layout that differs by quantisation, the draft head
costing differently on this base -- was invisible to it and obvious to the
card.

The same day, the first entry inspected through the new catalogue reporting
turned out to be adrift the other way: `qwen3-06b` declares 4096 MiB and was
measured holding 644.

## Decision

An entry's `memory_estimate_mib` is what `model-router bench` measured, plus
the margin `bench` applies. It is not inferred from a sibling entry, a
quantisation ratio, a file size, or a difference between two other
measurements.

An entry may be added before it has been benched. It then carries a comment
saying the figure is provisional and naming the command that settles it, which
is what `heretic38` did and what caused this to be caught.

The margin belongs to `bench` and is stated once, in
`Measurement::recommended_mib`: a twentieth over what was measured, rounded up
to a quarter gibibyte. A measurement is one driver, one day and one context,
and an estimate sitting exactly on it would be wrong the first time any of the
three moved. Nowhere else computes it; `GET /models` reports the measured and
declared figures and lets a reader compare them.

## Consequences and risks

An estimate is now checkable, and two ways to check it exist: run `bench`, or
read `memory.held_mib` beside `memory.declared_mib` on the catalogue while the
entry happens to be loaded.

The cost is that adding an entry needs a free card. `bench` refuses to run
beside a loaded router rather than measure the difference across somebody
else's model, so there is a window to arrange. That is the price of the number
meaning something.

The risk this does not cover: a measurement is taken as the child becomes
ready, before the context is filled. An entry whose KV cache grows with use can
hold more later than it was measured holding. Nothing here detects that, and
`held_mib` on a long-running entry is the figure to watch if it is ever
suspected.

## When this is reopened

A measurement that cannot be taken. If an entry can only run on hardware the
maintainer does not have, a declared estimate has no measured source available
and the rule has nothing to offer. Say so in the comment rather than quietly
inferring a number, which is what this decision exists to stop.
