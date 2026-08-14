<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0291 — a name is looked up, not searched for

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0107-any-expansion-value-slice-fix.md
        (established that an `Any` resolves its `type_url` against the
        graph's roots, and recurses into `value` alone)

## Background

Spec 0107's `Any` arm answers a `type_url` by walking `graph.roots`:

```rust
ws.graph.roots.iter()
    .find(|r| r.fqdn.as_str() == fqdn)
    .map(|r| r.state_id.to_native())
```

On googleapis there are 49 255 roots, and `fqdn` is an `ArchivedString` —
so every candidate costs a relative-pointer deref before the comparison
even starts. Instrumented over a protolens startup on that corpus:

| | count |
| --- | ---: |
| walks (`score_subset` calls) | 24 |
| `Any` resolutions | **23 313** |

That is ~971 resolutions per walk, each scanning on average about a third
of the root list before it hits. Callgrind put the cost at **8.73% of the
whole process** in rkyv's `string/repr.rs` alone (4.21 G Ir, ~383 M
`as_str()` calls), plus most of a further 1.61% in `slice/cmp.rs`
(`__memcmp_avx2_movbe`, 1 477 977 calls).

The walk's only other name→state question — the one on every field tag —
was answered by a sorted table and a binary search from the start. This
one was left linear because when spec 0107 was written the corpora were
small enough that it did not show.

## Goals

- **G1.** Resolve a `type_url` in time independent of the number of roots.
- **G2.** Return exactly what the scan returned, including on a miss and
  including which root wins when two share an fqdn.

## Non-goals

- **N1.** *No change to the archived graph format.* The index is derived
  at run time from what is already stored.
- **N2.** *Not a process-wide cache.* See Alternatives — the build is
  measured at 1/971 of the lookups it serves, so hoisting it above the
  walk buys nothing and costs a parameter through the public API.

## Specification

- **S1.** `WalkState` holds `any_index: Option<HashMap<&str, u32>>`,
  mapping root fqdn to root state id.
- **S2.** It is built on the first `Any` a walk resolves. A payload that
  contains no `Any` — or a walk with `expand_any` off — builds nothing.
- **S3.** Insertion is `entry().or_insert()`, so the **first** root of a
  duplicated fqdn wins. That is what `find` returned; G2.
- **S4.** A miss is `None`, and the caller's existing `None` arm is
  unchanged: the `value` scores as an opaque bytes match, no recursion.

## Alternatives considered

### Sort `roots` by fqdn and binary-search

Free at run time and needs no map — but `graph.roots` order is
load-bearing. `score_subset` returns its scores in `roots` order and its
callers merge on that, and `partition_roots` hands out indices into
`graph.roots`. Reordering the vector means touching all of it; adding a
*second*, sorted index vector means changing the archived format, which
is a version bump for something a run-time map already solves.

### Build the index once per process rather than once per walk

`score_subset` takes `&ArchivedCompiledGraph`, not the owning
`LoadedGraph`, so a process-wide cache means either a new parameter
threaded through the public entry points or a keyed global. Both are real
costs. The measured amortization is ~971 lookups per build, i.e. the
24 builds are under 0.1% of what they replace — there is nothing left to
win.

## Test plan

1. `any_expansion_recurses_into_value_only_varint`,
   `..._string`, `any_expansion_empty_value` — unchanged, and cover the
   hit path.
2. `any_expansion_leaves_an_unresolvable_type_url_alone` — **new**. The
   miss is the branch this spec rewrote (a `find` running off the end
   became a `get` returning `None`) and it had no guard.
3. A full `protolens … export /` over googleapis must be byte-identical
   to the previous binary. G2 across the whole corpus, not a fixture.

## Measured outcome

`export /` over googleapis: **byte-identical across 5 278 322 lines**.

protolens startup on googleapis, `taskset -c 0-7 … -j 8`, the two
binaries interleaved, medians of 5:

| | before | after |
| --- | ---: | ---: |
| startup | 2.021 s | **1.707 s** |
| sweep only (startup − 1.376 s serial floor) | 0.645 s | **0.331 s** |

**−15.6% of startup, −48.7% of the sweep**, and all five interleaved
pairs agree in sign with non-overlapping ranges — well clear of this
machine's 1-2% noise.
