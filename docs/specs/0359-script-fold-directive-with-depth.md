<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0359 — script `fold:` directive with depth

Status: implemented
Implemented in: 2026-08-25
App: protolens
Refs: docs/specs/0271-a-script-walks-the-reader-through-the-blob.md
      (the script step model this spec amends — in particular S9, which
      defines the current `fold:`/`unfold:` vocabulary);
      docs/specs/0332-fold-depth-digits.md (the `0`–`9` and `Z`
      interactive keys whose semantics `fold:` now reuses)

## Background

The current `fold:` and `unfold:` directives (spec 0271 S9) offer three
forms:

- `fold: all` — fold every foldable node.
- `fold: [<position>, ...]` — fold exactly these nodes (depth 0).
- `unfold: [<position>, ...]` — re-open these nodes one level (no depth
  control).

Spec 0271 N6 acknowledged the vocabulary was deliberately small: "three
forms, none of which control depth, because the missing depth is actually
missing." It is no longer missing. The interactive `0`–`9` keys
(`set_cursor_fold_depth`) give fine-grained control over how deep a
subtree opens; a script author needs the same control to reproduce the
exact view they are narrating.

The existing workaround is `fold: all` followed by a list of `unfold:`
entries for every node at every depth that should be visible —
`unfold: [/10, /10/1, /10/2]` to open two levels under `/10`. This is
verbose, brittle (adding a child means adding another entry), and cannot
express a partial-depth open without exhaustively listing every node.

## Goals

- **G1.** A step may declare a list of `fold:` entries, each naming a
  node and a depth, replacing both the old `fold:` and `unfold:` keys.
- **G2.** The depth vocabulary mirrors the interactive keys exactly:
  `0`–`9` map to `set_cursor_fold_depth(n)`, `Z` maps to
  `set_cursor_fold_depth(usize::MAX)` (fully open the subtree).
- **G3.** `fold: / 0` folds the entire document (root at depth 0 = close
  everything), replacing `fold: all`.
- **G4.** Existing scripts are migrated to the new syntax.

## Non-goals

- **N1.** No `fold: all` shorthand. `fold: / 0` is short enough and
  explicit about what it does.
- **N2.** No depth beyond `9` and `Z`. The interactive keys offer no
  more, and the point of mirroring them is that the author uses the
  interactive keys to find the right depth, then writes it down.
- **N3.** No per-step "reset to unfolded first" option. The reset is
  always `script_reset_folds` (clear all user folds), as today. The new
  `fold:` entries are applied on top of the reset, so a step with no
  `fold:` entries starts fully unfolded.

## Specification

### S1 — YAML format

`fold:` accepts a list of strings, each of the form `<position> <depth>`
where `<position>` is a path or search-text position (same syntax as the
existing `node:` key) and `<depth>` is one of `0`–`9` or `Z`.

```yaml
steps:
  - text: Start from a collapsed view, one message open.
    fold:
      - / 0
      - /3 Z
    node: /3/1
```

Inline list form is also valid YAML and equally accepted:

```yaml
    fold: ["/ 0", "/3 Z"]
```

Omitting `fold:` entirely is unchanged: the step starts fully unfolded.

The old `unfold:` key is **removed**. The old `fold: all` and
`fold: [<position>, ...]` forms are **removed**. Parsing any of these
produces a load error. A bare scalar for `fold:` (not a sequence) is
also a load error.

### S2 — Parsing

Each list entry is split on the last whitespace token:

- Everything before the last whitespace is the position string
  (passed to `Position::parse`).
- The last token is the depth: one of `0`–`9` or `Z` (case-sensitive).
  Any other value is a load error.

`Z` is stored as `usize::MAX` internally (matching
`toggle_cursor_fold_recursive`).

`fold:` must be a YAML sequence. A bare scalar is a load error.

### S3 — Application

`script_apply_folds` applies each entry in list order by calling
`script_fold_at(position, depth)`:

1. Resolve the position (same rules as `node:`, including baking).
2. Call `set_cursor_fold_depth(depth)` after setting `self.cursor = idx`
   — that function is refactored to delegate to a new
   `set_fold_depth(idx, depth)` that takes the target node explicitly,
   so both the interactive keys and the script path share one
   implementation. `set_cursor_fold_depth` becomes a one-liner:
   `self.set_fold_depth(self.cursor, depth)`.
3. If the position does not resolve, push a diagnostic (spec 0271 S13)
   and continue.

List order matters when two entries name overlapping subtrees (e.g.
`/ 0` then `/3 Z`): later entries override earlier ones within the
affected nodes, which is the natural author intent (collapse everything,
then open this one path).

`script_reset_folds` is called first by `script_reset`, as today — the
fold state is always rebuilt from scratch on each step.

### S4 — `Step` and `RawStep` changes

`Step.fold` changes from `Fold` (the old enum) to `Vec<FoldEntry>`:

```rust
pub struct FoldEntry {
    pub position: Position,
    pub depth: usize,   // 0–9 or usize::MAX for Z
}
pub fold: Vec<FoldEntry>,
```

`Step.unfold` is removed.

`RawStep` loses `fold: Option<Scalars>` and `unfold: Option<Scalars>`;
gains:

```rust
#[serde(default)]
fold: Option<Vec<String>>,  // sequence only; bare scalar is a load error
```

`into_step` parses each string in `fold` as `<position> <depth>`.

### S5 — Script migration

All existing `.script` files are updated to the new syntax:

- `fold: all` + `unfold: [/a, /b, ...]` becomes a `fold:` list where
  `/ 0` collapses the root and each previously-unfold'd path becomes
  `<path> Z` (or the appropriate depth if its children were selectively
  opened).
- `fold: [/a]` (fold without depth) becomes `fold: /a 0`.

## Alternatives considered

### Keep `fold: all` as a shorthand

Rejected (N1). It is a special case that would need its own code path,
and `["/ 0"]` is unambiguous and no harder to read.

### Allow a bare scalar (`fold: "/ 0"`) as a single-entry shorthand

Rejected for simplicity: a sequence is always a sequence. The one-element
case is `fold: ["/ 0"]` which is standard YAML and requires no special
parser branch.

### Use `unfold: /a 2` to add depth control to the existing `unfold:`

The two-key design (`fold:` then `unfold:`) requires the author to
think in two passes. A single `fold:` list applied in order is
equivalent — `/ 0` then `/a Z` is the same as `fold: all` then
`unfold: /a` — but requires only one key and makes the override order
explicit in the file.

### Accept depth as a separate YAML key per entry (mapping list)

```yaml
fold:
  - node: /3
    depth: 2
```

More verbose with no benefit. The `<position> <depth>` string is the
same vocabulary the interactive keys use (one character for the depth)
and is concise enough for a list.

## Test plan

1. `fold_depth_zero_collapses_root` — `fold: / 0` leaves every foldable
   node folded, matching `fold: all`'s old behavior.
2. `fold_depth_n_opens_to_n_levels` — `fold: /3 2` leaves `/3` open,
   its children open, their children folded.
3. `fold_z_fully_opens_subtree` — `fold: /3 Z` leaves every descendant
   of `/3` unfolded.
4. `fold_entries_applied_in_order` — `fold: ["/ 0", "/3 Z"]` produces
   the same state as `fold: all` + `unfold: [/3, /3/1, ...]` for the
   same subtree.
5. `unknown_depth_is_a_load_error` — `fold: ["/3 X"]` fails to parse.
6. `bare_scalar_fold_is_a_load_error` — `fold: "/ 0"` (not a sequence) fails to parse.
7. `old_unfold_key_is_a_load_error` — `unfold: [/3]` fails to parse.
8. `set_cursor_fold_depth_delegates_to_set_fold_depth` — after refactor,
   the interactive key and the script path produce identical fold state
   for the same node and depth.
9. `reuse lint` passes.

## Measured outcome

- `script.rs` unit tests cover all spec items: path/search classification,
  fold entry parsing (0–9 and Z), unknown depth error, bare scalar error,
  old `unfold:` key error, multiple wire directives error.
- `tui/tests/script.rs` integration tests confirm fold state is reproduced
  exactly when stepping back to a previous step.
- `tests/batch_script.rs` confirms `anomalies.script` walks without errors.
- All existing `.script` files migrated from `fold: all` + `unfold:` to the
  new `fold:` list syntax.
- 1255 tests pass; 0 failures.
