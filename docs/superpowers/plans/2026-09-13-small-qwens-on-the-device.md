# The small Qwens go to the device, and the retrieval pair get a rate

## Why the pinning existed, and why it does not now

`qwen3-4b` and `qwen3-06b` were pinned to the processor with
`n-gpu-layers = 0`, and the reason was sound when it was written: both were
candidates for a **resident** steward, and a resident reserves its estimate
against the budget permanently. On the card that reservation was about 7.5 GiB
out of a 29,347 MiB budget, which refused every 27B entry outright. The catalog
records the measurement that settled it -- `qwen38` exiting mid-load and
answering 502.

Two things changed since. Both entries became on-demand, so nothing is reserved
while they are not running; and the budget became the whole card rather than
nine tenths of it. The condition the pinning was protecting against no longer
exists.

What the pinning cost was visible in the last full sweep:

| entry | processor | device | |
|---|---:|---:|---|
| `qwen3-06b` | 10.2 tok/s | **474.6** | 46x |
| `qwen3-4b` | 1.1 tok/s | **260.9** | 237x |

A 4B model at roughly one token a second is not a slow option, it is an
unusable one. The device figures are what these weights are for.

## What it costs, and why that is affordable

Measured with `bench`, cache quantised to `q8_0` as every other entry that
keeps one on the device now is -- at 40960 tokens the cache was the larger half
of the bill, and both were still at f16 because pinning had made it moot.

| entry | was | now | declared |
|---|---:|---:|---:|
| `qwen3-06b` | 1024 | 3756 | 4096 |
| `qwen3-4b` | 1024 | 6225 | 6656 |

Verified that this does not recreate the failure the pinning prevented: both
were loaded, then `qwen38` was requested, and the router evicted both and
loaded the flagship at 31,147 MiB of 32,607. Eviction is what makes an
on-demand entry different from a resident one, and it is the whole reason this
is safe now when it was not before.

## The retrieval pair had no rate at all

`embed` and `rerank` printed `--` in the rate column, because `bench` measured
tokens a second and neither generates. They are not slow, they were unmeasured.

`Throughput` is now an enum of `Generated` (tokens a second, the server's own
figure from `timings.predicted_per_second`) and `Scored` (passages a second,
timed from this side because nothing reports it). Keeping both in one type
means a report cannot print a passage rate under a heading that says tokens --
they are different work and differ by orders of magnitude.

| entry | rate |
|---|---:|
| `embed` | 184.2 seq/s |
| `rerank` | 193.8 seq/s |

Measured over one request carrying 32 passages rather than 32 requests carrying
one, because a reranker is asked for a whole candidate list in practice and
measuring it a passage at a time would report the round trip rather than the
model.

The rate machinery moved to `bench::rate` on the way, which is what kept
`bench.rs` under the module gate: it went from 207 lines to 207 with the new
work added, because the old `rate`, `ask` and `body_of` left with it.

## Done looks like

- Both Qwens answer 200 on the device, and `qwen38` still loads beside them by
  eviction.
- `bench` prints a figure and a unit for all fourteen entries, none `--`.
- `just check` green.
