<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0296 — most varints are one byte

Status: implemented
Implemented in: 2026-08-15
App: prototext-core
Refs: docs/specs/0288-the-same-bytes-are-read-once-whoever-asks.md (S7 split
        the *uncommon* varint out of the parser for exactly this reason;
        this is that split one level finer),
      docs/specs/0295-an-edge-knows-what-its-child-expects.md (the
        baseline this is measured against)

## Background

After 0295, `parse_varint` was the largest symbol outside the scoring
walk: **1.35 G instructions, 6.57%**. Callgrind's call graph gives the
shape:

| caller | calls |
| --- | ---: |
| `score_message_multi` (both frames) | 31.8 M |
| `render_message` | 14.8 M |
| `parse_wiretag` | 2.8 M |
| **total** | **49.4 M** |
| of which reach `parse_varint_uncommon` | 30 810 (0.06%) |

**27.4 instructions per call**, and 0288's cold path is essentially
never taken — so all of it is the supposedly-common case.

Two things were wrong with that case, and counters settled which
mattered. Of the 49 386 994 varints a googleapis startup reads,
**47 480 446 — 96.1% — are a single byte.** For those the loop is pure
overhead: the shift accumulator never advances, and both disqualifiers
are false by construction (`shift` is 0, so the overflow test cannot
fire; `pos` is `start + 1`, so a `0x00` here is a canonical zero rather
than overhang).

The second problem is that `VarintResult` is 56 bytes with three
`Option`s. Returned out of line it is seven word-stores through a hidden
pointer, which the caller reads back and flattens — `walk.rs`'s adapter
does precisely that, and it is what 0288's `#[inline]` was meant to
avoid. The hint was being declined.

## Goals

- **G1.** A single-byte varint costs a bounds check, a load, a compare
  and an add.
- **G2.** The 56-byte result never reaches memory on that path.
- **G3.** One implementation of the multi-byte scan, of truncation, of
  overflow and of overhang. Scoring and rendering must agree
  byte-for-byte on where every field begins and ends.
- **G4.** Output unchanged, byte for byte.

## Non-goals

- **N1.** No narrower result type for scoring. `walk.rs` already adapts
  the shape, and once the entry point inlines there is no struct left to
  narrow — the `None`s are literals at the call site and fold away. A
  second public result type would buy nothing and would put G3 at risk.

- **N2.** No SIMD or wider-load varint decoding. The win here is that
  96.1% of varints need no decoding loop at all; a faster loop addresses
  the other 3.9%.

- **N3.** No change to `parse_varint_uncommon`. At 0.06% it is not worth
  looking at, and 0288's reasoning for it still holds.

## Specification

- **S1.** `parse_varint` becomes the single-byte case and nothing else:

  ```rust
  if let Some(&b) = buf.get(start) {
      if b < 0x80 { return VarintResult { next_pos: start + 1, … }; }
  }
  parse_varint_multibyte(buf, start)
  ```

- **S2.** The previous body — loop, disqualifiers, hand-off to
  `parse_varint_uncommon` — moves unchanged into
  `parse_varint_multibyte`, `#[inline(never)]`. Moved, not copied: G3.

  Not `#[cold]`, unlike `parse_varint_uncommon`. At 3.9% this is
  uncommon, not rare, and banishing it to the far section would cost
  those calls a taken branch and an instruction-cache miss for no gain.

- **S3.** `buf.get(start)` returning `None` covers both the empty-at-end
  and the out-of-range `start`, so the multi-byte path keeps the
  `assert!(start <= buflen)` and remains the single place the bounds
  contract is stated. Behavior at `start == buf.len()` (a truncated
  result) and at `start > buf.len()` (a panic) is unchanged.

- **S4.** `#[inline(always)]`, not `#[inline]`. See below — the hint was
  measured being declined a second time.

## Alternatives considered

### `#[inline]` alone, trusting the hint

Tried first, as the cheap probe. It moved the problem rather than
solving it: LLVM inlined `parse_varint` into `walk.rs`'s adapter, which
then grew past its own threshold and stopped being inlined into
`score_message_multi`. The 56-byte struct was still materialized, one
frame further out. **20.637 → 20.432 G, −1.0%**, against −6.2% for the
split. The lesson from 0288 S7 repeats: with a body this size the
`#[inline]` hint is not load-bearing, the body size is.

### A two-byte fast path as well

Two-byte varints are the bulk of the remaining 3.9%. Not taken: it
doubles the entry point's size to chase 4% of the calls, and the entry
point staying small is the entire mechanism.

## Test plan

1. The existing `helpers` unit tests already straddle the new boundary
   exactly: `varint_zero` (a single `0x00`, the one single-byte value
   that could plausibly be mistaken for overhang), `varint_one_byte`,
   `varint_150` (two bytes), `varint_max_u64` (ten), `varint_truncated`,
   `varint_empty_at_end` (`start == len`) and both overhang cases.
2. The full workspace suite.
3. `protolens … export /` over googleapis is byte-identical to 0295's.

## Measured outcome

Dev VM (8 E-cores, two L2 clusters), googleapis (25.6 MB descriptor
set, 49 255 roots), `--descriptor-set $SET $SET quit`.

| | 0295 | 0296 |
|---|---|---|
| wall clock `-j 1`, `taskset -c 4`, median of 5 | 2.810 s | 2.645 s |
| wall clock `-j 8`, `taskset -c 0-7`, median of 22 | 1.648 s | 1.416 s |
| instructions (`-j 1`) | 20.64 G | 19.37 G |

**−6.2% instructions, −5.9% at `-j 1` and −14.1% at `-j 8`.**
`parse_varint` no longer appears in the profile at any threshold: it is
inlined and folded at all 44 call sites.

The `-j 8` win being the *larger* one reverses this campaign's usual
pattern, and the reason is that this is the first change in it that is
not confined to the walk. `render_message` and `parse_wiretag` are
14.8 M and 2.8 M of the 49.4 M calls, and both live in the serial tail —
which is exactly the part of a startup that eight workers do not shrink.

The `-j 8` figure is pooled over two interleaved runs of eleven pairs
because the first two disagreed (−16.1%, −9.3%); the spread there is
wide even in a quiet window, whereas `-j 1` reproduces to ±1%.

`export /` over the whole corpus is byte-identical to 0295's output,
5 278 322 lines.
