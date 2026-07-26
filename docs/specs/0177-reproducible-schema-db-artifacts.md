<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0177 — reproducible schema-DB artifacts

Status: implemented
Implemented in: 2026-07-26
App: prototext-graph
Refs: docs/scoring-flaws.md (C11),
      docs/specs/0068-lazy-fds-loading.md,
      docs/specs/0059-hopcroft-test-harness.md

## Background

Compiling the same `.pb` four times in four processes produces four different
`hopcroft.rkyv` files and four different `index.rkyv` files. Measured
2026-07-26 on `prototext-core/fixtures/descriptor.pb`.

The scoring is unaffected — `list-schemas --top 15` output is byte-identical
across all four, including an 8-way tie, because every consumer of `score_all`
breaks ties on the unique FQDN and because Hopcroft is confluent: the coarsest
partition is unique regardless of worklist order, so only the *numbering*
moves. That part of flaw C11 was investigated and found not to be a scoring
bug at all.

What is lost is content-addressability of the artifact: a schema DB cannot be
verified by digest, cached or deduplicated by digest, and two DBs cannot be
diffed to tell "the schema changed" from "someone rebuilt it".

### Three independent causes

1. **`graph::build` node numbering** (`graph.rs:198`) iterates
   `merged.states.keys()`, a `HashMap`.
2. **Hopcroft block numbering during refinement** (`hopcroft.rs:231`) allocates
   `y1_id = blocks.len()` while iterating `x_in_block`, a `HashMap`.
3. **`FdsIndex` serialization.** Its four fields are `HashMap`s in an rkyv
   `Archive` struct. `ArchivedHashTable::serialize_from_iter`
   (rkyv 0.8.16, `table.rs:392-421`) probes for the **first empty slot**, so
   when two keys share a probe sequence the slot each occupies depends on which
   the source iterator yielded first; the key/value data region is likewise
   written in iteration order. `std::HashMap`'s `RandomState` is seeded per
   process.

### The `FdsIndex` fix needs no format change

The obvious repair — `HashMap` → `BTreeMap` — would change the archived layout,
and that makes a `VERSION` bump **mandatory** rather than cosmetic:
`lazy_pool.rs:108` uses `access_unchecked`, so a layout change without a bump
reads garbage as a valid archive rather than failing. Every existing DB would
need rebuilding, and lookups would go from one `FxHasher64` pass plus a SIMD
group match to O(log n) string comparisons over keys that share long prefixes
(`google.protobuf.…`) — the worst case for comparison-based search.

None of that is necessary, because the source hasher does not reach the
archive:

```rust
impl<K, V: Archive, S> Archive for HashMap<K, V, S> {
    type Archived = ArchivedHashMap<K::Archived, V::Archived>;   // no S
}
pub struct ArchivedHashMap<K, V, H = FxHasher64> { ... }
```

Archived lookups always use `FxHasher64` whatever the source used. So the
source hasher can be swapped for a deterministic one with the archived type,
the reader, the header version and lookup cost all untouched.

A fixed seed alone is not enough, though. `std::HashMap` places a key in the
first empty slot of *its* probe sequence too, so on a collision the slot each
of two keys occupies still depends on which was inserted first — and that slot
assignment is what iteration order, and hence the archived layout, is read out
of. Making the layout a function of the key set alone takes a fixed seed **and**
a fixed insertion order.

## Goals

1. Compiling the same input twice yields byte-identical `hopcroft.rkyv` and
   byte-identical `index.rkyv`.
2. No change to `index.rkyv`'s or `hopcroft.rkyv`'s format, and no `VERSION`
   bump. Existing schema DBs stay readable.
3. No change to any score, ranking, or partition.

## Non-goals

- **`HashMap` → `BTreeMap` in `FdsIndex`.** Rejected above. Nothing iterates
  these maps — all four are reached only by `.get()` (`lazy_pool.rs:157`,
  `:179`, `:200`, `:255`) — so ordered iteration buys nothing.
- **A digest check, or any caching that consumes the new reproducibility.**
  This spec only makes the artifacts reproducible.
- **Reproducibility of `reproto`'s emitted `.proto` sources.** Out of scope.

## Specification

### S1 — canonical `FdsIndex` maps

In `prototext-graph/src/fds_index.rs`, give the four maps a fixed-seed
`BuildHasher`:

```rust
pub type FxBuildHasher = std::hash::BuildHasherDefault<rkyv::hash::FxHasher64>;
```

`FxHasher64` is the hasher the archived map already uses for lookups, so this
introduces no new dependency and no second hash function.

Add the second half of the fix in the same module, as the only supported way to
populate those fields:

```rust
pub fn canonical_map<V>(
    entries: impl IntoIterator<Item = (String, V)>,
) -> HashMap<String, V, FxBuildHasher>
```

which sorts by key before inserting, so the resulting slot assignment — and
therefore iteration order and the archived bytes — depends on nothing but the
key set.

The pyo3 boundary (`prototext-graph-pyo3/src/lib.rs:101-113`) keeps plain
`HashMap` parameters and passes each through `canonical_map` when building the
struct. Changing the parameter types would drag `S` through `FromPyObject`
*and* `pyo3-stub-gen`'s `PyStubType`, and would not help anyway — the caller is
Python, whose own dict/set order is not something this crate can constrain.
Sorting at the boundary is one build-time pass over an index that is written
once.

### S2 — deterministic node numbering in `graph::build`

Collect and sort `merged.states.keys()` once, and route **all three** loops
over `merged.states` through that list, not just the node-numbering one at
`graph.rs:198`. The other two matter for the same reason:

- the unreferenced-child pass decides the order in which child FQDNs not
  defined in `states` get their IDs;
- the edge-build loop drives `LeafRegistry::range_sentinel`, which assigns each
  distinct range a `range_idx` in **first-seen** order — so it leaks into the
  Hopcroft initial partition's leaf labels, not merely into node IDs.

Sorting only `graph.rs:198` and S3 was measured to leave `hopcroft.rkyv` still
varying across four builds, which is how the third loop was found.

Node IDs, child IDs and `range_idx` are all internal — nothing outside the
builder depends on their values — so this is invisible beyond the byte layout.

### S3 — deterministic block numbering in Hopcroft refinement

At `hopcroft.rs:231`, drain `x_in_block` into a `Vec` sorted by block ID before
splitting, so `y1_id` allocation follows a fixed order.

This does not change which nodes end up in which block, only the labels: the
refinement's fixed point is the unique coarsest partition. S3 is therefore
constrained by, and consistent with,
`docs/specs/0059-hopcroft-test-harness.md:175`, which requires two map-entry
types to stay in distinct states "regardless of HashMap iteration order" — an
assertion about the partition, which already held and continues to.

### S4 — documentation

`docs/scoring-flaws.md` C11: record the measured verdict (not a scoring bug),
the three causes, and that the repair needed no format change.

## Test plan

- **Reproducibility, end to end.** Compile the same `.pb` four times in four
  separate processes and assert `hopcroft.rkyv` and `index.rkyv` are
  byte-identical across all four. Four separate *processes* is the whole point
  — `RandomState` is per process, so an in-process loop would pass vacuously.
  Venue: `reproto`'s pytest suite, which is the only place that can build a
  schema DB (it needs `reproto --schema-db-out`); same constraint as spec 0176.

  The input must also be **large enough to make Hopcroft split blocks**, which
  is where the numbering nondeterminism lives: a four-message schema produced an
  identical `hopcroft.rkyv` even *before* the fix, so the test imports
  `descriptor.proto`, `struct.proto` and `timestamp.proto` with
  `--include_imports`. Non-vacuity was confirmed by running the test against the
  pre-fix module in the nix store: 4 distinct digests.
- **Backward compatibility** needs no test: the archived layout is unchanged by
  construction, since `S` is not part of `ArchivedHashMap` and the sort only
  reorders serialization, so `VERSION` stays 4 and every reader is untouched.
- **Invariance.** `list-schemas --top 15` output is unchanged by this spec.
- `cargo test --release --no-default-features --workspace` green, and the
  reproto pytest suite green.
- `nix-build -A ci` green.
