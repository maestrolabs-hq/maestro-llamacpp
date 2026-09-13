# North star

What this estate is for, and how it would be measured if it were.

## The point

Software that can be rebuilt, reasoned about, and handed over. Every part of
that is a property somebody has to be able to check, not a quality somebody
asserts.

Three commitments follow from it:

**A machine can be rebuilt from the repository.** Configuration is captured,
not remembered. The test is a clean machine and one command.

**A decision is recorded where it is enforced.** Not in a conversation, not in
a commit message alone. An ADR beside the thing it governs.

**A gate that cannot fail is not a gate.** Every check here has been proved by
injecting the fault it exists to catch and watching it fail. A green result
nobody should trust is worse than a red one.

## How it would be measured

Honestly stated: these are targets, and most are not instrumented yet. Saying
which is which is the point of writing them down.

| Property | Target | Measured today |
| --- | --- | --- |
| Fast tier duration | under 5 minutes, p95 | no |
| Heavy tier freshness | under 8 days | no |
| Drift from the baseline | zero, continuously | yes, weekly |
| Gates proved by injection | every gate | by hand, at the time |
| Restore on a clean machine | one command, no manual steps | no |
| Decisions with a recorded ADR | every decision that would be re-derived | no |

The gap between the first column and the third is the honest state of this
estate. Closing it is the work; pretending it is closed is the failure.

## What this is not

Not a product, not a service, and not a foundation. There is one maintainer,
no support commitment, and no roadmap beyond what is in the issues.
