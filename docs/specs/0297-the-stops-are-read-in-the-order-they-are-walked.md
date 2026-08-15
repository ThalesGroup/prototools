<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0297 — the stops are read in the order they are walked

Status: implemented
Implemented in: 2026-08-15
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S3,
        which is where `auto_folded` comes from),
      docs/specs/0247-a-fold-toggle-carries-the-worst-news-below-it.md
        (S7, the one full pass this changes),
      docs/specs/0296-most-varints-are-one-byte.md (the baseline this is
        measured against)

## Background

After 0296 the walk is 70.5% of a startup and everything else is small.
The largest of the small things was not in the walk at all:

| symbol | Ir | share |
| --- | ---: | ---: |
| `own_status` (own body) | 279 M | 1.44% |
| `sip::Hasher::write` | 268 M | 1.44% |
| `hash_one`, from `own_status` | 428 M | 2.27% |
| **total** | **975 M** | **5.0%** |

The call graph attributes 4 737 284 of the 4 881 003 calls to
`hash_one` — 97% of every hash the process computes — to a single
expression, `own_status`'s `self.auto_folded.contains(&idx)`.

That is one hash and one probe per arena slot. `rebuild_status` runs
**once** in a production startup (`App::new`), so the 4.74 M calls are
one reverse linear pass over a 4.74 M-slot arena, asking a set of some
tens of thousands of stops whether each slot is one of them. The answer
is "no" 98% of the time, and finding that out costs a SipHash and a
random probe into a table of about a megabyte.

## Goals

- **G1.** The stop question is answered without hashing and without a
  random memory access.
- **G2.** One statement of the status rule. `rebuild_status` and
  `refresh_status_subtree` must not come to disagree about what a stop
  contributes.
- **G3.** `auto_folded` stays a `HashSet<usize>`. Forty-odd sites insert,
  remove, iterate, count and `retain` on it, and all of them are right.
- **G4.** Output unchanged, byte for byte.

## Non-goals

- **N1.** No change to `refresh_status_subtree`. It asks the question a
  handful of times per splice, not millions, and a bitset built per
  splice would cost more than the probes it saved.

- **N2.** No bitset representation of `auto_folded` itself. See G3, and
  see `search_cursor.rs:294` — the set is iterated to find the stops,
  which over a bitset would mean scanning 4.74 M bits to find 84 000 of
  them.

## Specification

- **S1.** `rebuild_status` flattens `auto_folded` into a local
  `Vec<u64>`, one bit per slot, before the pass:

  ```rust
  let mut stops = vec![0u64; self.tree.len().div_ceil(64)];
  for &idx in &self.auto_folded {
      stops[idx / 64] |= 1 << (idx % 64);
  }
  ```

  Filling it is O(|`auto_folded`|) — three orders of magnitude smaller
  than the arena — and it is read *in the order the loop already walks*,
  so it costs one cache line per 512 nodes and the prefetcher has it
  before the loop arrives.

- **S2.** `own_status` takes membership as a parameter,
  `fn own_status(&self, idx: usize, is_stop: bool)`, rather than asking
  for it. That is G2: the rule that a stop is at least `Unbaked` is still
  written once, and only the *sourcing* of the fact differs between the
  bulk pass and the incremental one.

- **S3.** `refresh_status_subtree` passes
  `self.auto_folded.contains(&idx)` — N1.

## Alternatives considered

### A faster hasher

Tried first, as the cheap probe: `type FoldSet = HashSet<usize,
FxBuildHasher>`, on the reasoning that the keys are dense slot numbers
this process allocated and DoS resistance buys nothing.

It removed SipHash from the profile entirely — **19.37 → 18.65 G,
−3.7%** — and was **3.3% slower** in wall clock, over two independent
quiet-window runs of five and nine interleaved pairs.

The reading is that SipHash was not the cost. It is ~130 instructions of
high-IPC arithmetic on a value already in a register, and it was
*overlapping* the load it precedes: the out-of-order window had the
probe's cache miss in flight throughout. Deleting the arithmetic did not
delete the miss, it exposed it.

This is the campaign's second instance of the same shape — the first was
0293's `slice::binary_search_by`, which was +16.5% wall clock at flat Ir
because branchless code serializes a chain of dependent misses. Both say
the same thing: **Ir is a measure of work, not of latency**, and a change
that only removes ALU work next to a miss is not obviously a change at
all.

### `node_text[idx].is_none()` as an early return

Most slots are vacant during a bounded startup render, and `own_status`
already loads `node_text[idx]` first. A stop always has text — its header
and its footer — so a vacant slot cannot be in `auto_folded`, and
returning `Ok` early would skip the probe for nearly all 4.74 M calls
with no new state at all.

Not taken. It converts a comment into load-bearing behavior: the
invariant is real but nothing enforces it, `assert_status_is_exact`
would not catch a violation because both paths run `own_status`, and
several tests insert into `auto_folded` directly. The bitset costs 600 KB
once and assumes nothing.

## Test plan

1. `tests/node_status.rs` covers a node put in `auto_folded` by hand
   followed by `rebuild_status`, which is exactly the bitset path.
2. `assert_status_is_exact`, hung off every `finalize_override_batch` in
   the suite, compares the incrementally maintained arrays against a full
   `rebuild_status` — so every splice in the suite cross-checks S3's path
   against S1's.
3. The full workspace suite.
4. `protolens … export /` over googleapis is byte-identical to 0296's.

## Measured outcome

Dev VM (8 E-cores, two L2 clusters), googleapis (25.6 MB descriptor set,
49 255 roots), `--descriptor-set $SET $SET quit`.

| | 0296 | 0297 |
|---|---|---|
| wall clock `-j 1`, `taskset -c 4`, median of 9 | 2.650 s | 2.561 s |
| wall clock `-j 8`, `taskset -c 0-7`, median of 11 | 1.441 s | 1.393 s |
| instructions (`-j 1`) | 19.37 G | 18.62 G |

**−3.9% instructions, −3.4% at `-j 1` and −3.4% at `-j 8`.**

`own_status` falls from 975 M to 166 M — the 279 M body loses the probe
and the 696 M of hashing goes to 14 M, which is the rest of the process.
The two ratios agreeing across `-j 1` and `-j 8` is what the `-j 8`
number is for: `rebuild_status` is in the serial tail, so a change there
should scale identically, and it does.

`export /` over the whole corpus is byte-identical to 0296's output,
5 278 322 lines.
