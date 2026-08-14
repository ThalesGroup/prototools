<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0289 — one answer serves every candidate that asked it

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0288-the-same-bytes-are-read-once-whoever-asks.md (shared
        the packed *decode* across candidates; this shares the *check*),
      docs/specs/0175-packed-and-expanded-repeated-scalars.md (the two
        encodings whose elements this checks),
      docs/specs/0179-active-entry-field-widths.md (no allocation in the
        walk's steady state)

## Background

Spec 0288 memoized the decode of a packed varint payload per token, and
measured the result on googleapis: `parse_varint` fell from 35.99% of
`protolens --descriptor-set $SET $SET quit` to 1.54%, and the whole
process from 140.96 G to 88.25 G Ir.

It did not touch the check. `check_varint_value`'s call count was
unchanged **to the digit** — 875 123 235 — against 25 019 063 distinct
elements. That is a sharing factor of 35.0: every packed element is
checked thirty-five times, and thirty-five times it produces the same
answer. Post-0288 the check was the single largest term in the process,
at 27.65% self.

The redundancy is structural. The element loop sits inside
`for ae in active.iter_mut()`, so it runs once per candidate group. But
the verdict on one value is a pure function of `(graph, node, val,
overhang)`; `ae` is read only to decide *which score rows* the resulting
penalties are added to. Every group resolving to the same `child`
therefore recomputes a bit-identical answer and breaks at the identical
element.

## Goals

- **G1.** Derive a packed run's verdict once per distinct child under one
  LEN token, instead of once per candidate group.
- **G2.** Byte-identical scoring output. This is a redundancy removal, not
  a change of policy.
- **G3.** No allocation in the walk's steady state (spec 0179).

## Non-goals

- **N1.** *No second implementation of the range rules.* The obvious way
  to make the check cacheable is to write a summary — a min/max, or a
  first-violating index — and answer from that. It would duplicate the
  wire-type and gap semantics in a second place, and the two copies would
  drift. `check_varint_value` is instead **purified**: the one existing
  body keeps the rules and simply returns its findings rather than
  writing them. See Alternatives.
- **N2.** No cross-token cache. The memo is cleared per token. A shared
  one would need the payload in its key, which is the expensive part.
- **N3.** No change to which values veto, which count as non-canonical, or
  which count as out-of-range.

## Specification

- **S1.** `check_varint_value` returns a `ValueVerdict { vetoed,
  non_canonical, out_of_range }` and takes `(graph, node, val, overhang)`.
  It no longer takes `&mut WalkState` or `&ActiveEntry`. This is the whole
  reason the check becomes cacheable: every quantity it produces is a
  function of the leaf and the value, and of nothing else.
- **S2.** `apply_value_verdict(ws, ae, v)` is the half that genuinely
  depends on the candidate group — it adds the verdict's penalties to
  every entry of `ae`. It stays per candidate.
- **S3.** `packed_run_verdict(graph, node, elements)` sums a whole run
  into one `ValueVerdict`, in element order, breaking on the first veto.
  Order and break are those of the loop it replaces, so the accumulated
  counts remain the offenders *before* the break — which is what spec
  0288 S4 required of the per-element form.
- **S4.** `WalkState::elem_verdicts: Vec<(u32, ValueVerdict)>` memoizes S3
  per token, keyed on the child state id. Cleared per token; `mem::take`n
  by the `WT_LEN` arm for the same two reasons as `packed_scratch` —
  capacity must survive the token, and `apply_value_verdict` needs `ws`
  mutably while the derivation runs.
- **S5.** The memo is a linearly-scanned `Vec`, not a map. The scan is
  over *distinct children under one tag*, which is a handful; at that size
  a scan beats hashing, and it allocates once.

## Alternatives considered

### A summary structure (min, max, first violation)

Rejected as N1. It answers a more general question than is asked — it
would let a *novel* range be tested without re-reading the payload — and
that generality is what forces the range semantics to be written a second
time. It also cannot produce `out_of_range` **counts**, only a boolean,
because the count depends on how many elements fall outside the specific
range. Memoizing the whole verdict answers the question actually asked
and keeps one copy of the rules.

### Keying the memo on the node pointer rather than the child id

Equivalent, and no cheaper: `find_node` is already called before the
lookup. The child id is the key the surrounding code already has.

### A two-level varint cache shared across worker threads

Proposed during the 0288 work: a global table caching the parse of
*anomalous* varints, consulted on the cold path. Measured and rejected —
the cold path (`parse_varint_uncommon`) takes 30 810 calls and 631 334 Ir,
0.0007% of the process. There is nothing there to save, and the anomaly
bitmap alone would exceed the 2 MB cluster L2.

## Test plan

1. Full workspace release suite — 1042 + 113 + 93 + … pass unchanged.
2. `protolens --descriptor-set $SET $SET export /` over googleapis,
   diffed against a pre-0288 binary. Byte-identical output is the
   correctness witness: this spec must change instruction counts and
   nothing else.
3. `bin/profile startup` under callgrind, for the instruction count.
4. Interleaved `-j 1` / `-j 8` wall clock, for the Amdahl decomposition.

## Measured outcome

Machine: the 8-vCPU VM (host CPUs 4-11, all E-cores). Corpus:
googleapis, 49 255 scoring roots. Pinned `taskset -c 4` / `-c 4-11`.

**Instructions** (`bin/profile startup`, whole process):

| | pre-0288 | post-0288 | post-0289 |
| --- | ---: | ---: | ---: |
| PROGRAM TOTALS | 140.96 G | 88.25 G | **48.21 G** |
| `parse_varint` self | 50.7 G (35.99%) | 1.36 G (1.54%) | 1.355 G (2.81%) |
| `check_varint_value` self | 24.4 G (17.31%) | 24.4 G (27.65%) | **inlined away** |
| `propagate_vetoes` self | 6.35 G (4.5%) | 6.35 G | 6.35 G (13.18%) |

−45.4% against 0288, −65.8% against the baseline. The drop is larger than
`check_varint_value`'s own 24.4 G because the per-element loop that drove
it went with it. `check_varint_value` and `packed_run_verdict` no longer
appear as symbols at all — once the derivation runs per distinct child
rather than per candidate, both inline into `score_message_multi`.
`parse_varint` is unchanged in absolute terms, as intended: this spec
touches the check, not the decode.

**Wall clock**, medians of 3, the two binaries interleaved within one
script so neither is systematically measured on a hotter or more
power-capped package:

| CPUs | pre-0288 | post-0289 | gain |
| ---: | ---: | ---: | ---: |
| 1 | 15.170 s | 8.870 s | 1.71x |
| 4 | 4.799 s | 3.127 s | 1.53x |
| 8 | 3.162 s | **2.313 s** | 1.37x |
| speedup 1 → 8 | 4.80x | **3.83x** | |

**2.92x fewer instructions bought 1.71x of single-threaded wall clock.**
Same conversion loss 0288 recorded, now larger: the instructions deleted
were the high-IPC inlined ones, so they returned less than their share.

Amdahl `T(n) = S + P/n`, fitted on n=1 and n=8 and then **validated at
n=4** (predicts 4.878 / 3.250 against 4.799 / 3.127 measured, within 4%):

| | serial `S` | parallel `P` |
| --- | ---: | ---: |
| pre-0288 | 1.447 s | 13.723 s |
| post-0289 | 1.376 s | 7.494 s |

The serial floor is **unchanged**, as two walk-only changes should leave
it. All of the win is in `P`: **1.83x**.

**What this means for the next change.** The serial 1.376 s is now
**59.5% of a 2.313 s 8-CPU startup**, up from 45.7%. Halving the sweep
again would buy 0.47 s, i.e. 20%. The sweep is still worth working on,
but the serial phase has become the larger half and each further sweep
win is worth less than the last.

**Measurement trap, recorded because it cost two wrong conclusions.**
This VM has CPUs **0-7 only**. `taskset -c 4-11` does not fail — it
silently narrows to **4-7**, a four-CPU mask. A first pass at the table
above used it, labelled the result "8 cores", and produced both a bogus
Amdahl fit (serial 2.36 s) and a bogus theory that battery mode
suppresses single-core turbo asymmetrically. Neither survived the
`-c 0-7` re-run: the battery penalty is **uniform** (1.62x at `-j 1`,
1.65x at `-j 8`), and the previously recorded "4.72x on 8 cores" for
pre-0288 was **correct** — it is 4.80x here.
