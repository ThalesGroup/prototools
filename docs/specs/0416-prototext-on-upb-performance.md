<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0416 — prototext on upb: performance improvements

Status: draft
App: prototext-core, prototext-upb
Refs: docs/specs/0415-prototext-on-upb.md (S12: the benchmarks this
      answers; the `FieldOrExt` change it builds on);
      docs/prototext/bench-process.md (pinning, the bench lock);
      docs/specs/0173-score-all-and-render-hot-path-allocations.md
      (deterministic counts where timers are too noisy);
      docs/specs/0248-an-extension-resolves-without-loading-the-world.md
      (the extension loader the cache must not get ahead of)

## Background

Spec 0415 moved prototext's schemas from prost-reflect to upb. Measured
before and after (0415 S12), on the same machine, pinned:

| Benchmark | prost-reflect | upb | Change |
|---|---|---|---|
| Criterion A2: decode and render `descriptor.pb` (17 KB), schema and annotations | 369.99 µs | 390.11 µs | +5.8% |
| CLI: decode googleapis.desc (25.6 MB) as a `FileDescriptorSet` | 856.6 ms | 815.5 ms | −4.8% |
| CLI: eager load of googleapis.desc | 2.070 s | 0.703 s | −66% |

A2 is the one regression. Both renders use the same schema
(`descriptor.proto`); A2's document is small and has many distinct
fields, each seen a few times, so the cost paid per field *occurrence*
dominates it. The 25.6 MB decode amortizes the same cost over far more
occurrences per field, and gains from upb elsewhere.

What a field occurrence costs, today:

- the lookup itself: `upb_MessageDef_FindFieldByNumber`, then on a miss
  `upb_DefPool_FindExtensionByNumber` under the pool's read lock;
- `FieldOrExt::new`, which resolves kind, cardinality, framing and
  packing once per occurrence (0415's fix for the 25.6 MB decode): about
  six calls into upb, none inlinable across the C boundary, plus a new
  handle (an atomic reference count) when the kind names a message or an
  enum;
- `FieldOrExt::kind`, which clones that handle again on each call.

prost-reflect paid none of this: its field properties are precomputed
Rust structs, and a lookup is an index.

The unchanged, schema-free benchmarks (`parse_varint`, random bytes)
moved by 0 to +4% between the two runs, so part of A2's +5.8% is drift;
the true cost is somewhere between about 2% and 6%.

## Goals

- **G1.** A field seen again costs one hashed lookup, not a lookup plus
  six calls into upb.
- **G2.** A2 is back within its measured noise of the prost-reflect
  baseline, with that noise measured, not assumed.
- **G3.** Output is unchanged: spec 0415's golden corpus stays
  byte-identical, and every test passes.

## Non-goals

- **N1.** Changing upb, or reaching into its private structures
  (`_upb_…`, `UPB_PRIVATE`) for speed.
- **N2.** Link-time optimization across the C/Rust boundary
  (`-flto` with `linker-plugin-lto`): toolchain-dependent, and it would
  bind the build to one compiler pairing in nixpkgs.
- **N3.** protolens, which is not on upb yet (0415 N1).

## Specification

- **S1. A per-render field cache.** For the duration of one render
  (`decode_and_render` and its indexed variant), `prototext-core` keeps a
  thread-local map from `(message definition, field number)` to the
  resolved `FieldOrExt`, or to "no such field":
  - keyed by the message definition's address, which is stable for the
    pool's life (0415 S2), and the field number;
  - the value is shared (`Rc<FieldOrExt>`), so a hit clones nothing but
    a reference count local to the thread;
  - a miss is cached after the extension loader has been tried, so a
    field the loader cannot resolve is not retried in the same render
    while the pool is unchanged;
  - the map is cleared when the render that installed it returns,
    including on an early `CodecError` (a guard, as `ExtLoaderGuard`
    does), so no answer outlives the render it was computed in.

  A hit is always still right: definitions are immutable, so a field
  that was found stays found. A miss can go stale: a file loaded
  mid-render, for an `Any` or another extension, may declare an
  extension an earlier lookup missed. So `Pool` gains a generation
  counter, incremented by each successful `add_file`, and a cached miss
  is trusted only while the generation it was recorded at is current;
  otherwise the lookup runs again.
- **S2. Borrowed kinds.** `FieldOrExt::kind` returns `&Kind`. Callers
  that keep a nested message take their own clone where they need one,
  instead of every call cloning.
- **S3. Measuring noise first.** Before comparing, run the same binary
  against itself twice (A/A) and report that spread beside each A/B
  result; a change within the A/A spread is reported as no change.
  Where wall time stays ambiguous, report the instruction count
  (callgrind, `bin/profile`) as well, which is deterministic.

## Alternatives considered

### A pool-wide cache in `prototext-upb`

Survives across renders, but every lookup would take the pool's lock
and hash anyway, which costs about what the upb calls it saves, and its
entries would need invalidating rules the per-render cache does not.

### Precompute every field of every message at load

What prost-reflect does. On googleapis it would allocate for 58,777
types whatever the blob uses, giving back much of the 2.9× memory saving
of eager load (0415 S12) to speed up the few fields a render touches.

### Resolve properties lazily inside `FieldOrExt`

Calls upb only for the properties a path actually reads. It removes
some calls per occurrence but keeps the lookup and the handle per
occurrence; the cache removes all three for every repeat.

## Test plan

1. A test renders a document with a field seen many times and checks,
   through a counter in test builds, that the field is resolved once.
2. The cache is empty after a render returns, and after one that fails
   with `CodecError`.
3. A render whose extension loader adds a file mid-render resolves the
   extension it was asked for (spec 0248's test, kept green), and a miss
   recorded before a file is added is looked up again after it.
4. Spec 0415's golden corpus: 0 differing cases.
5. Benchmarks: A2 and the 25.6 MB decode, A/A then A/B, reported under
   "Measured outcome" for review.

## Measured outcome

Filled in at implementation.
