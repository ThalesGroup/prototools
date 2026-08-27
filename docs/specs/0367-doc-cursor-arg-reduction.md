<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0367 — DocCursor argument reduction

Status: implemented
Implemented in: 2026-08-27
App: protolens

## Background

Three functions exceed clippy's `too_many_arguments` limit (8/7 or 9/7):

- `display_pieces` in `render.rs` — 8 args
- `find_in_segment` in `search_cursor.rs` — 8 args
- `find_last_in_segment` in `search_cursor.rs` — 9 args

Rather than suppressing the lint with `#[allow(clippy::too_many_arguments)]`,
two structural improvements eliminate the excess parameters without introducing
artificial wrappers.

## Goals

- **G1.** Reduce `display_pieces` from 8 to 6 parameters by grouping the two
  fold-related parameters into a `FoldState` struct.
- **G2.** Reduce `find_in_segment` from 8 to 5 and `find_last_in_segment` from
  9 to 6 parameters by grouping the four document-view parameters into a
  `DocView` struct.
- **G3.** Remove the two `#[allow(clippy::too_many_arguments)]` pragmas
  introduced as a stopgap.

## Non-goals

- **N1.** Combining `shadowed` and `annotations` into a single parameter.
  They are different kinds of data: `shadowed` is structural (a bitset of
  shadowed arena slots), `annotations` is a display preference. Grouping them
  would misrepresent their relationship.
- **N2.** Changing the semantics of any of the grouped parameters.
- **N3.** Refactoring callers beyond what is mechanically required by the new
  signatures.

## Specification

### S1 — `FoldState` struct (`render.rs`)

Introduce:

```rust
pub(super) struct FoldState {
    pub closed: bool,
    pub style: Option<Style>,
}
```

Replace the `fold_closed: bool, fold_style: Option<Style>` pair in
`display_pieces` with a single `fold: FoldState` parameter.  All call sites
pass a `FoldState { closed: …, style: … }` literal.

### S2 — `DocView` struct (`search_cursor.rs`)

Introduce:

```rust
pub(super) struct DocView<'a> {
    pub st: Structure<'a>,
    pub text: &'a [Option<Box<str>>],
    pub shadowed: Option<Arc<Vec<u64>>>,
    pub annotations: bool,
}
```

Replace the `(st, text, shadowed, annotations)` prefix in both
`find_in_segment` and `find_last_in_segment` with a single `doc: DocView<'_>`
parameter.

Update `DocCursor::with_marks` to accept a `DocView` instead of the four
separate fields, or destructure at the call site — whichever is cleaner.

All call sites in `spawn_segment_scan` and `scan_segment_inline` build a
`DocView` from the same fields they currently pass individually.

### S3 — No `#[allow]` pragmas

Neither function retains a `#[allow(clippy::too_many_arguments)]` attribute
after this change.

## Alternatives considered

### `#[allow(clippy::too_many_arguments)]`

Already applied as a stopgap in the current working tree.  Rejected because it
hides a real signal — the functions genuinely benefit from grouping — and
because the lint exists precisely to prompt this kind of tidy-up.

### Grouping `shadowed` + `annotations` into `DocView`

They already travel together, so the grouping is natural for the call sites.
The risk is implying a semantic relationship that does not exist.  Accepted
anyway: `DocView` is named after what it represents (the document as the cursor
sees it), not after either field individually, so the grouping is accurate.

### A single catch-all context struct for `display_pieces`

`display_pieces` already has `raw`, `owner`, `hints`, `first_node` in addition
to the fold pair.  Grouping all eight would be over-engineering for a function
that is called from a single site.  Grouping only the two fold parameters is
the minimum change that clears the lint.

## Test plan

1. `cargo clippy --release --no-default-features --workspace --features prebuilt-wkt -- -D warnings` passes with no `too_many_arguments` errors.
2. Existing tests pass unchanged — no behavioral change.
