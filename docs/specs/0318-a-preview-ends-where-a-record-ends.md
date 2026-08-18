<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0318 — a preview ends where a record ends

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0174-….md (`TruncShape`, the preview byte budget, the
        `...` marker), docs/specs/0185-….md (the preview is an overlay,
        and overlay rows have no node), docs/specs/0187-….md
        (`window_text` blanks the marker before the parser sees it),
        docs/specs/0193-….md (the fold margin), docs/specs/0303-….md (a
        truncated message says what it is missing),
        docs/specs/0316-….md (reverted — see N2)

## Background

A live preview cuts the candidate's interior to
`override_preview_byte_budget` (4 096 by default) so that arrowing
through the selection pane costs a bounded decode. For a message target
the cut lands wherever the count runs out — in the middle of a field —
so the last field renders as `TRUNCATED_BYTES`, and spec 0303 now makes
that field state how many bytes it is missing.

The reader sees a malformity that is not in their data. It is an
artifact of the budget, and it appears on exactly the node they are
deciding about. Spec 0174 S4 papered over it with a trailing `...` line
that *replaced* the straddling field's row; that marker is not in the
prototext grammar, so tree-sitter recovers from it by stripping captures
off the rows beneath, which is why spec 0316 removed it (see
`protolens_non_grammar_lines`).

The cut is the problem, not the marker. **Cut the payload at a top-level
wire-record boundary and nothing is truncated** — the kept prefix is a
sequence of whole records, which is exactly what a shorter message is.
No field straddles the cut, so no field can carry an annotation the full
data would not have carried.

**Why a byte budget stays.** Every rendered row costs at least one byte
of payload — a packed element is at least one byte, a record header at
least two — so a byte budget bounds rows whatever the shape of the data.
The converse is false: a row budget bounds no bytes, and bounds nothing
at all inside a packed run, where one record can produce millions of
rows. This is the property spec 0316 discarded and the reason this spec
keeps the budget in bytes.

## Goals

- **G1.** A preview of a message, group or untyped LEN node shows no
  truncation annotation that the same bytes would not show when
  committed, for every node whose top-level records fit the cap.
- **G2.** The reader can tell, without acting, whether the preview is
  the whole node, a clean prefix of it, or a ragged prefix.
- **G3.** No row of a preview is outside the prototext grammar.
- **G4.** The preview's input stays bounded in bytes, and the bound is
  one knob.

## Non-goals

- **N1.** Bounding the *commit* path. Confirming an override renders the
  node in full and always has (spec 0174 G5). Overriding the root of
  `googleapis.desc` to a packed-heavy type still produces 25 M rows and
  freezes; that is the rows-per-slot problem, it is untouched here, and
  it needs its own spec. This one makes the preview honest, not the
  document cheap.
- **N2.** Reinstating spec 0316's thesis that a preview is the real
  thing. It is not, and this spec says so on the screen: a preview is a
  *sample*, confirming may show more than the preview did, and preview
  bytes never reach the encoder, the clipboard or an export.
- **N3.** Any `...` line, or any other row that the prototext grammar
  does not accept. G3 is the standing rule, not a preference.
- **N4.** A background color for the previewed rows. See Alternatives.
- **N5.** Tinting the committed node's fold triangle. The overlay
  *covers* the target's committed rows (spec 0185), so that triangle is
  not on screen while the preview is up.

## Specification

- **S1.** `TruncShape::Exact` splits in two. `Bytes` — and only `bytes`
  — keeps today's cut-at-the-budget rule under the name `AnyByte`: any
  byte sequence is a valid `bytes` value, so a shorter one annotates
  nothing. `Message`, `Group`, and the raw (un-retyped) LEN and group
  cases take a new `RecordBoundary`. `CharBoundary`, `PackedVarint`,
  `PackedFixed` and `Never` are unchanged.

- **S2.** `RecordBoundary`'s cut. Walk the payload from offset 0,
  skipping one top-level wire record at a time: parse the tag, then skip
  a varint, 4 bytes, 8 bytes, or a length-prefixed payload according to
  its wire type. `START_GROUP` increments a depth counter and `END_GROUP`
  decrements it; a position is a boundary only at depth 0. Keep the
  first boundary at or after `soft`, provided it is at or before `hard`.

  The walk allocates nothing, does not descend into a record, and reads
  at most `hard` bytes — so its cost is the budget, by the same argument
  that justifies the budget.

- **S3.** `soft` is `override_preview_byte_budget` (unchanged default
  4 096) and `hard` is twice it. One knob, and the flag keeps its
  meaning: the number the reader sets is still the amount of payload a
  preview is guaranteed to show.

- **S4.** When no boundary lands in `soft..=hard` — one record longer
  than the room, or a length varint the walk cannot parse — cut at
  exactly `hard`. This is today's behavior, annotations and all,
  including the `TRUNCATED_BYTES` line spec 0303 elaborates. Degrading to
  the honest-but-ugly rendering is correct: the alternative is a preview
  that shows less than the reader asked for with no way to say so. A
  walk that fails before `soft` has met malformity in the data itself,
  and the same rule applies.

- **S5.** `cut_at` returns the tier alongside the length. Three tiers:

  | tier | meaning | color |
  |---|---|---|
  | `Whole` | nothing was cut | green |
  | `Clean` | cut at a top-level record boundary (S2) | yellow |
  | `Ragged` | cut at `hard`, mid-record (S4) | orange |

  Deriving the tier at the call site by comparing lengths would be
  wrong: a boundary that falls exactly on `hard` is `Clean`.

- **S6.** `PreviewOverlay` carries the tier and the bar column. The
  column is `marker_column(&lines[0])` — the same function `mouse.rs`'s
  fold hit test uses, so the bar cannot drift from where the node's own
  triangle would sit.

- **S7.** Every overlay row draws `│` (U+2502) in the fold column, in
  the tier's color, including the first and the closing brace. The
  column is free by construction: `display_row_source` returns `None`
  for an overlay row, so an overlay row has no fold marker at any depth
  (spec 0185 S4) and its margin is all spaces. There is no collision to
  guard against and no indent setting under which one can appear.

  The bar therefore does double duty — it says *these rows are the
  preview*, which the reader currently has to infer, and its color says
  how much of the node they are.

- **S8.** `insert_truncation_marker` is deleted, with the `window_text`
  special case that blanked its output (spec 0187 S2). Nothing replaces
  it: the bar is the signal, and it is drawn in the margin, so it is not
  a document row and the parser never sees it.

- **S9.** The bar pans off the left edge with the fold margin, exactly
  as the fold triangle does. Not worth a fix — a reader panned that far
  right is reading a value, not choosing a type.

## Alternatives considered

**A colored background behind the previewed rows.** The user's first
proposal, and the direct expression of the intent. Rejected on three
grounds. The ANSI-16 background palette is documented as exhausted
(`theme.rs:1140-1204`): the cursor row, a matched brace and a search hit
already hold `DarkGray`/`Gray`, `Blue`/`Cyan` and `Yellow`, and magenta
is what was left. All four are transient and a few characters wide; a
preview region is dozens of rows and would *contain* them, so the two
would have to compose. And green/yellow/orange is already protolens's
anomaly severity ramp — behind text it would read as a claim about the
data. In the fold column it cannot, because nothing else is ever drawn
there.

**Suppressing the `TRUNCATED_BYTES` annotation on the straddling
field.** Cheaper, and a lie: the same bytes annotate everywhere else in
the program, and spec 0303 exists precisely to make a cut message say
what it is missing. Suppression also cannot be local — the field's
status propagates to the ancestors' fold-glyph colors.

**Dropping the straddling field's row.** What spec 0174 S4 did before
the marker. It changes the line count silently and leaves the reader
counting fields that are not there.

**Keeping the `...` marker and teaching the grammar about it.** A
display artifact in the wire-format grammar, to serve one caller, at the
cost of every prototext consumer.

## Test plan

1. `record_boundary_cut_keeps_whole_records` — a message of 3 000
   two-byte fields cut at the default budget yields a payload that
   re-parses as a sequence of whole records with no remainder.
2. `record_boundary_preview_has_no_truncation_annotation` — the same
   preview's rendered text contains neither `TRUNCATED_BYTES` nor
   `missing`.
3. `a_record_longer_than_the_room_is_ragged` — one 10 000-byte field
   yields tier `Ragged`, a cut at exactly `hard`, and the
   `TRUNCATED_BYTES` line (S4 is behavior, not a fallback to hide).
4. `a_boundary_exactly_at_hard_is_clean` — pins S5's reason for
   returning the tier rather than deriving it.
5. `a_bad_length_varint_before_soft_does_not_panic` — the walk stops,
   S4 applies.
6. `group_payload_cuts_at_top_level_only` — an `END_GROUP` inside a
   nested group is not taken for a boundary; the cut leaves an unclosed
   group, which renders as a plain `}`.
7. `a_short_node_is_whole` — a node under `soft` yields `None` from
   `cut_at` and tier `Whole`.
8. `bytes_target_still_cuts_anywhere` — `AnyByte` is byte-for-byte
   today's `Exact`.
9. `overlay_rows_draw_the_tier_bar` — a render test asserting the bar
   character and color on the first, a middle and the closing row, for
   each of the three tiers.
10. `overlay_fold_column_is_free_at_indent_one` — pins S7's claim, which
    is the one that would silently regress if overlay rows ever gained
    an owner.
11. `every_overlay_row_is_prototext` — colorize every overlay row and
    require the result to equal what the same lines colorize to on their
    own; G3. Replaces the test that pinned the marker being blanked.
12. `measure_a_preview_renders_size` — existing; re-measure at the
    doubled worst-case input and confirm `RENDER_CACHE_MAX_BYTES` still
    holds a screenful of candidates.

## Measured outcome

Both numbers came out better than the estimate above them, and for the
same reason.

**The worst-case preview render barely grew.** `measure_a_preview_
renders_size`, extended to the overshoot case, reports:

| input | lines | text | spans | total |
| --- | --- | --- | --- | --- |
| 2 048 two-byte records, cut at `soft` | 2 050 | 38 937 B | 2 049 | 104 505 B |
| 2 047 two-byte records then one 4 099-byte record, cut at `hard` | 2 085 | 43 805 B | 2 083 | 110 461 B |

+5.7%, not the ~100% predicted. The prediction assumed the extra bytes
would arrive as more records. They cannot: the walk only overshoots
`soft` when a *single* record straddles it, and a single record is one
row however long it is. Doubling the byte cap therefore buys a
worst-case render 6% larger, and `RENDER_CACHE_MAX_BYTES` keeps
essentially the headroom it had — ~76 worst-case entries rather than
~80, against the ~40 the estimate feared.

**The boundary walk costs 23.5 µs** on 2 048 two-byte top-level records
— 11 ns per record, the shape that makes it do the most work per byte.
It runs once per `render_node_as`, i.e. once per candidate keystroke,
against a frame budget three orders of magnitude larger.
