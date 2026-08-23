<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0347 — score: truncated as a count, not a bool

Status: implemented
Implemented in: 2026-08-23
App: prototext-graph (src/score/walk.rs), prototext (src/run.rs)
Refs: docs/specs/0310-truncated-not-vetoed.md (introduced EntryScore::truncated)

## Background

`EntryScore::truncated` is a `bool`.  `cut_or_veto` sets it to `true`
whenever a frame ends because bytes ran out rather than because a declared
boundary was reached.  The score formula charges a flat −5 regardless of
how many nodes were truncated.

A protobuf message can have multiple truncated sub-messages: each one fires
its own `cut_or_veto` call and sets `truncated = true` on the surviving
entries — but subsequent calls are no-ops because the field is already `true`.
The score header therefore prints `truncated` (a bare flag) even when several
sub-messages were cut.  The count is lost.

Additionally, the text output already annotates every truncated node
individually (`TRUNCATED_MESSAGE; MISSING: N`, `TRUNCATED_BYTES;
MISSING: N`), so the per-node information exists in the render path but is
not summarised in the score.

Reproduction: `prototext --descriptor alice/app.desc decode bob/logfile`
reports `truncated` as a bare word in the score header, with no count.

## Goals

- **G1.** `EntryScore::truncated` becomes `u64`, counting the number of
  distinct nodes (frames) that were cut.
- **G2.** The score formula charges −5 per truncated node (i.e.
  `−5 * truncated` with `truncated: u64`), unchanged in rate.
- **G3.** The score header and YAML output print `truncated: N` instead of
  bare `truncated`, consistent with the other counters.  Zero is suppressed
  as before (it would appear on every non-truncated decode).

## Non-goals

- **N1.** The −5 coefficient is not revisited here.
- **N2.** The render path's per-node `TRUNCATED_MESSAGE` / `TRUNCATED_BYTES`
  annotations are not changed.

## Specification

**S1.** Change `EntryScore::truncated` from `bool` to `u64` in
`prototext-graph/src/score/walk.rs`.

**S2.** Replace every `ws.scores[e].truncated = true` in `cut_or_veto` with
`ws.scores[e].truncated += 1`.

**S3.** Update the `score()` method:

```rust
- 5 * self.truncated as i64   // unchanged in rate; truncated is now u64
```

The cast is already present; changing the field type makes no difference
to this line.

**S4.** In `prototext/src/run.rs`, change `InferredType::truncated` and
the internal `Breakdown::truncated` structs from `bool` to `u64`, and
propagate the field through all copy sites.

**S5.** In `inferred_header`, change the truncated formatting from the
bare-flag style to a counter, suppressed when zero:

```rust
let cut = if inferred.truncated > 0 {
    format!(", truncated: {}", inferred.truncated)
} else {
    String::new()
};
```

**S6.** In the YAML/detailed-score output path (`write_type_result`), emit
`truncated: N` as a plain integer (already done via `writeln!` — just
change the type).

## Alternatives considered

**Keep `bool`, add a separate `truncated_count: u64`.** Redundant: the bool
is always `truncated_count > 0`.  Two fields for one concept.

**Charge a fixed −5 regardless of count (keep bool, change display only).**
Loses the scoring signal that a message with five truncated sub-messages is
worse evidence than one with one.  The count is available for free; there is
no reason to discard it.

## Test plan

1. `single_truncated_node_counts_one` — a blob truncated at the top level;
   assert `truncated == 1` and `score == matches - 5`.
2. `two_truncated_sub_messages_count_two` — a blob with two sub-message
   fields each declared longer than the buffer; assert `truncated == 2` and
   `score == matches - 10`.
3. `no_truncation_suppressed` — a complete blob; assert `truncated == 0`
   and the score header does not contain `truncated`.
4. `header_format` — assert the score header prints `truncated: 2` (not
   bare `truncated`) when `truncated == 2`.

## Measured outcome

`EntryScore::truncated` is now `u64` throughout. Score header suppresses all
zero attributes (including `truncated`); non-zero `truncated` appears as
`truncated: N`. Full nix build clean, all tests pass.
