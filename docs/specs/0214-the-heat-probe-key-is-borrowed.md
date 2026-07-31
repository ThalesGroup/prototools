<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0214 — the heat probe key is borrowed

Status: implemented
Implemented in: 2026-07-30
App: protolens
Refs: docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (the shared `HeatCaches` this spec adds a method to),
      docs/specs/0154-protolens-heat-cue-progressive-display.md
        (`HeatState`'s two independently-arriving halves, which is why
        `current_score` is read on every unsettled node),
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        G9 (the promoting `peek` whose semantics this spec preserves
        exactly),
      docs/specs/0208-attention-follows-the-cursor.md S3 (the per-node
        tier the four call sites pass),
      docs/protolens/rendering-worklist.md (the hot-path work this
        continues)

## Background

`HeatCaches::current_score` is a `TieredBounded<(usize, String),
Option<i64>>` — the exact score of a node's *currently assigned* type
over its payload range, keyed by that range's start offset and the type
name.

`TieredBounded::peek` takes `&K`, and its index is a `std` `HashMap`. A
`std` map can only be probed by a **borrowed form of the whole key**
(`K: Borrow<Q>`), and `(usize, String)` has none: the borrowed
counterpart would be `(usize, &str)`, and `Borrow` can only hand back a
reference to something the key already holds, not a rebuilt tuple. So
every read had to *construct an owned key it then threw away*:

```rust
caches.current_score.peek(&(start, key.to_string()), tier)
```

One heap allocation and one copy of a fully-qualified protobuf type name
— routinely 40-60 bytes — per lookup, at four sites on two threads:

| site | when |
| --- | --- |
| `heat_cue.rs` `heat_cue_resolve` | once per unsettled node, per frame |
| `heat_cue.rs` `recheck_pending_heat_states` | once per pending node, on **every** worker progress event |
| `heat_worker.rs` `heat_lookup_ex`'s `current_ready` gate | once per lookup |
| `heat_worker.rs` the worker's `covers_current` check | once per popped request |

The second is the one that matters. `recheck_pending_heat_states` loops
over `pending_heat_recheck`, and that set is only non-empty while results
are still arriving — which is exactly when the worker is emitting
progress events fastest. So the allocation count is (pending nodes) ×
(progress events), and both terms grow together while the user scrolls
into unvisited document.

A fifth site, `heat_worker.rs`'s `current_score.upsert`, is *not* part of
this: an insert genuinely has to own its key.

## Goals

- **G1.** No allocation on any `current_score` lookup.
- **G2.** Identical cache semantics. `peek` is a promoting read (spec
  0164 G9); the tier argument and its effect on eviction ranking are
  unchanged at every site.
- **G3.** One way to read the map, so a future call site cannot
  reintroduce the allocation without deliberately reaching past it.

## Non-goals

- **N1.** Changing the key type. Interning the name to an `FqdnId` would
  also remove the allocation and shrink every stored key, but the worker
  thread would then need read access to `App`'s `FqdnTable` to turn an id
  back into the name `inferred_score` wants — a new cross-thread shared
  structure, and a lock on the scoring path, to save the same allocation
  this spec saves for free. Reconsider only if the key's *size* becomes
  the problem.
- **N2.** Adding `hashbrown` to protolens for its `Equivalent` trait, or
  waiting for `HashMap::raw_entry` to stabilize. Either would give a
  genuinely borrowed probe, but both are a dependency-level answer to a
  four-call-site problem.
- **N3.** `current_type_key`'s own `Option<String>` return, which
  allocates once per call upstream of all four sites. Real, larger, and a
  separate change — it needs the callers to stop needing an owned name at
  all.

## Specification

- **S1.** `HeatCaches` gains a private `probe: (usize, String)` field,
  initialized empty in `new`.
- **S2.** `HeatCaches` gains
  `peek_current(&mut self, start: usize, key: &str, tier: Tier) ->
  Option<Option<i64>>`, which overwrites both halves of `probe` — `.0` by
  assignment, `.1` by `clear()` + `push_str` — and forwards to
  `current_score.peek(&self.probe, tier)`. Borrowing `self.probe`
  immutably while `self.current_score` is borrowed mutably is a
  disjoint-field borrow and needs no cell.
- **S3.** All four lookup sites call it. `current_score` stays `pub(super)`
  because the three `upsert` sites still need it directly.
- **S4.** The probe holds nothing meaningful between calls and is
  documented as such, on the field.

## Test plan

1. `peek_current_does_not_carry_the_previous_probe_key` — a hit on
   `"MsgLonger"`, then a miss on `"Msg"`, then the same hit again. The
   prefix case is the one that matters: a truncating reset instead of
   `clear()` + `push_str` would leave `"MsgLonger"` in the buffer and
   report a hit for a type that was never cached.
2. `peek_current_does_not_carry_the_previous_probe_offset` — the same for
   the `usize` half, which would otherwise answer for the wrong node's
   payload.
3. `worker_uses_cheap_fast_path_when_only_current_is_missing` rewritten
   to assert through `peek_current`, so the existing round-trip coverage
   exercises the new path rather than bypassing it.

## Open questions

None.

## Measured outcome

Not separately benchmarked. The change removes a known, counted
allocation on a path with no other behavior change, and the existing
harnesses cannot isolate it: `PROTOLENS_TRACE` reports per-phase frame
times, in which one `malloc` of a short string is far below the noise,
and there is no allocation-counting harness in this crate (the lesson
recorded in `benchmark_noise_floors` is to prefer a counting allocator to
a timer for exactly this kind of question — building one is worthwhile,
and is not this spec).

What can be stated exactly is the count: four allocation sites become
zero, and the per-`HeatCaches` total becomes one buffer that reaches the
longest type name the session probes and then stops growing.
