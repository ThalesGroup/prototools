<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0357 — script `select:` and `search:` directives

Status: implemented
Implemented in: 2026-08-25
App: protolens
Refs: docs/specs/0271-a-script-walks-the-reader-through-the-blob.md (the
      script step model this spec extends);
      docs/specs/0242-the-selection-is-a-span-of-characters.md (the
      selection machinery `select:` reuses);
      docs/specs/0235-the-search-prompt.md (the search machinery
      `search:` reuses)

## Background

The cursor-row highlight (`cursor_row_style`, `#2A2D2E` on dark theme) is
intentionally subtle — a barely-there lift off the background, appropriate
for normal use. On a projector the node the presenter is pointing at is
hard to spot from the back of the room.

Two complementary directives make a step more legible on a projector:

- `select: true` engages the selection background (`DARK_SELECTION`,
  `#264F78`) on the caret's current line — the same prominent blue that a
  `Shift`-motion produces. It reuses the existing rendering path with no
  new visual machinery.

- `search: <pattern>` fires the existing search highlight on a given
  regex, exactly as if the presenter had typed `/pattern Enter` with the
  caret at the start of the step's node. All matching text in the document
  is tinted the same yellow the interactive search uses.

Both may appear on the same step.

## Goals

- **G1.** A step may declare `select: true`. The header line of the caret
  node (placed by `node:`, or the root if `node:` is absent) is selected,
  giving it the prominent selection background.
- **G2.** A step may declare `search: <pattern>`. All occurrences of the
  pattern in the document are highlighted using the existing search tint,
  as if `/pattern Enter` had been typed from the node's first character.
- **G3.** `script_reset` clears both the selection and the search
  highlight, so a step without these directives starts clean.
- **G4.** The selection and search set by the directives are normal: the
  user can extend, replace, or clear them with the usual keys.

## Non-goals

- **N1.** No multi-line selection. Only the caret's current line (the
  node's header) is selected. Selecting an entire subtree across many rows
  would be visually overwhelming.
- **N2.** No implicit `select: true` when `node:` is present. Requiring
  an explicit directive keeps old scripts unchanged.
- **N3.** `select:` does not accept a position. It always selects the
  caret line (the `node:` target). A free-standing position would require
  placing the caret there, conflicting with `node:`.
- **N4.** `search:` does not move the caret. It only highlights.

## Specification

### S1 — YAML format

```yaml
- text: |
    protolens opened without a type name and found one anyway.
  node: /2
  select: true
  search: SearchTextRequest
```

`select:` accepts the boolean `true` (YAML unquoted). Any other value is
a load error.

`search:` accepts a non-empty string, treated as a regex exactly as the
interactive `/` prompt does.

Both are optional and default to absent (no selection, no search).

### S2 — `select: true` — what is selected

`script_apply_select` is called after `script_apply_cursor` has placed
the caret on the `node:` target (or the root). It sets:

```
select_anchor  = Some(CursorPos { node: self.cursor,
                                  line_in_node: 0,
                                  column: text_len - 1 })
select_engaged = true
cursor_line_in_node = 0
cursor_column  = 0
```

The anchor is at the end of the line and the caret at column 0.
`selection_span` orders the two ends, so the full header line is
selected regardless of which end the caret rests on. Placing the caret
at column 0 keeps the cursorline tint on the left where the presenter's
eye naturally starts.

Because the anchor and the caret are on the same node, no conflict with
`node:` arises.

If the caret line has zero characters (empty line), `select: true` is a
no-op for that step (nothing to select).

### S3 — `search: <pattern>` — what is searched

`script_apply_search` is called after `script_apply_select`. It:

1. Sets `search_origin` to a `SearchOrigin` whose `at` field is the first
   character of the `node:` target's header line (node index =
   `self.cursor`, `line_in_node = 0`, column = 0), scope = `Main`.
2. Calls `commit_search(SearchDir::Forward, pattern)`.

The result is identical to the user opening `/`, typing `pattern`, and
pressing Enter while the caret was at column 0 of the node header: the
first match at or after that position is jumped to and tinted strongly;
all other matches are tinted weakly. The search tally appears.

If the pattern does not compile as a regex, the error is appended to
`state.diagnostics` (spec 0271 S13) and no search is started. The rest
of the step is applied normally.

### S4 — Application order

The full `script_apply` sequence becomes:

```
script_reset
script_apply_folds
script_apply_cursor      ← places caret (node:)
script_apply_wire
script_focus             ← adjusts scroll
script_apply_select      ← select: true (caret already positioned)
script_apply_search      ← search: pattern
prefill                  ← command: prefill
```

### S5 — `script_reset`

`script_reset` already calls `clear_selection()` (handles `select:`).
It must also call `clear_search_highlight()` to drop any search left by a
previous step's `search:` directive.

`cancel_search()` must **not** be used here — it would restore the scroll
position saved at prompt open time, which has no meaning inside
`script_reset` and would fight the step's `node:` placement.

### S6 — Step struct extension

`Step` gains two fields:

```rust
pub select: bool,           // default false
pub search: Option<String>, // default None
```

`RawStep` gains:

```rust
#[serde(default)]
select: bool,
#[serde(default)]
search: Option<String>,
```

`into_step` passes them through directly (no `Position::parse` needed for
either).

## Alternatives considered

### `select: <position>`

The original design accepted a position argument, allowing `select:` and
`node:` to point to different nodes. This is incompatible with the
anchor+caret selection model (spec 0242): the selection's moving end *is*
the caret, so the anchor and the caret must be on the same row for the
highlight to cover a single line. Changing `node:` to be "scroll target
only" would be a breaking change. `select: true` avoids the conflict
entirely.

### Implicit selection when `node:` is set

Rejected (N2): would visually change every existing script.

### `search:` moving the caret to the first hit

Rejected (N4): it would fight `node:`, which already controls caret
placement.

## Test plan

1. `select_directive_highlights_the_caret_line` — a step with
   `node: /1` and `select: true` produces a non-None `selection_span`
   covering the header line of `/1`, with the caret at column 0.
2. `select_is_cleared_at_the_next_step` — advancing clears the selection.
3. `search_directive_highlights_pattern` — a step with `search: foo`
   produces a non-None `search_sweep` whose pattern matches `"foo"`.
4. `search_is_cleared_at_the_next_step` — advancing clears the search
   highlight (`search_sweep` is `None`, `search_highlight` is `false`).
5. `select_and_search_may_coexist` — a step with both directives applies
   both without error.
6. `reuse lint` passes.

## Measured outcome

All six test-plan items pass. `select: true` engages the selection with
the caret at column 0 (anchor at end, caret at start). `search: pattern`
fires `commit_search` with a pre-set `search_origin` at column 0 of the
node header and records the pattern via `set_last_search_for` so `F`/`n`/`N`
reuse it. `script_reset` calls `clear_search_highlight()` to clear both
the sweep and `search_highlight` between steps.
