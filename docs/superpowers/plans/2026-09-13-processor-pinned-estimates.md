# Estimates for entries that never touch the device

## The problem

The estimator has four terms -- weights, cache, fragmentation, overhead -- and
three of them describe memory the *device* holds. Nothing in it had ever read
`n-gpu-layers`, so an entry that pins every layer to the processor was charged
for weights and a cache it keeps in host memory instead.

Found by audit rather than by failure, because two declared figures were
covering it:

| entry | derived | measured on the device |
|---|---:|---:|
| `qwen3-4b` | 9285 MiB | 811 MiB |
| `qwen3-06b` | 6145 MiB | 795 MiB |

Both declare 1024, so admission used the right number and nothing misbehaved.
What was broken was the derivation underneath: `check` reported a discrepancy
on every run that was not one, and the next pinned entry added without a hand
figure would have been refused room it never wanted -- eleven times over.

This is the same shape as the four faults already fixed in this module. A term
that is real in general was applied to a case where it does not hold.

## The change

`runs_on_processor(flags)` beside `keeps_no_cache` and `predicts_tokens`, read
from the flag the server keys on rather than inferred from the file. The file
describes a model; the flags describe a way of running one, and only the
second can say where the layers went.

When it fires, the weights and the draft cache are zeroed before the arms that
use them. That collapses fragmentation with the weights in both arms at once,
since it is a percentage of them, and leaves the overhead standing alone --
which the module doc already said is what such an entry pays, because a
resident holding no layers on the device had been measured paying it.

Only an explicit `0` counts. A partial offload costs the device a share that no
flag states, and a guess at it would put a figure *below* the truth into the
one term the budget cannot afford to under-read. Charged in full instead.

## Done looks like

- `check` no longer reports a gap for either pinned entry -- both derive 1024.
- Both still measure under it: 802 and 813 MiB by `bench`.
- A test that fails before the change and passes after, plus a unit test whose
  assertions were confirmed by mutating the helper two ways.

## Carried in the same change

Two catalog inconsistencies the audit turned up beside it, neither of which
needed code:

- `gemma3` was the last cache-keeping entry on f16 while every neighbour used
  `q8_0`. Set to match: 1553 MiB measured to 1486, which is 67 MiB and changes
  nothing about what fits. Set for consistency, not for the saving.
- `qwen3-06b` was missing `fa`, `np` and `kv-unified`, which its twin
  `qwen3-4b` has carried all along -- an omission rather than a distinction.

And one deletion: the retired `Qwen3-Embedding-0.6B` GGUF, which lost the
retrieval bake-off to bge-m3 and had been reported as found-but-undeclared on
every `check` since.
