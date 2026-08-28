<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0368 — advance_when: node and line predicates

Status: implemented
Implemented in: 2026-08-28
App: protolens

## Background

The existing `caret:` predicate fires when the cursor is on a named node
regardless of which field within that node the cursor is on.  Presentation
scripts sometimes need to advance based on cursor position without requiring
a specific field-name match, or need to fire only when the cursor is on a
particular child field by its proto field number.  There is also no way to
trigger on an absolute document line.

## Goals

- **G1.** Add a `node: <path>` predicate that fires when the caret is on
  the named node.
- **G2.** Add a `node: <path>:N` predicate that fires when the caret is on
  a child of the named node whose proto field number is `N`.
- **G3.** Predicates that are satisfied immediately on step entry cause the
  step to auto-advance without any user input (consistent with existing
  predicate behaviour).
- **G4.** Add a `line: n` predicate that fires when the caret is on the
  given 1-based absolute document line.

## Non-goals

- **N1.** Column-level precision within a line.
- **N2.** Ranges of field numbers or lines.

## Specification

- **S1.** The YAML key for both new predicates is a plain mapping, matching
  the existing predicate convention:
  ```yaml
  advance_when:
    - node: /a/b/c        # cursor on that node
    - node: /a/b/c:3      # cursor on a child of /a/b/c with field_number 3
    - node: f.q.d.n:2     # cursor on field 2 of the FQN-resolved node
    - line: 7             # absolute document line 7
  ```
- **S2.** The `:N` suffix is detected by `rfind(':')` on the raw string.
  If the substring after the last colon parses as a `u32 >= 1`, it is
  treated as `field_number`; otherwise the entire string is the position
  and `field_number` is `None`.
- **S3.** `node: <path>:N` evaluates as:
  `parent(cursor) == resolve(path) && tree[cursor].span.field_number == N`.
- **S4.** `line: n` is evaluated as:
  `absolute_start(cursor) + cursor_line_in_node + 1 == n`.

## Alternatives considered

- **Line-within-node index** (`:N` as 1-based rendered line within the
  node) — rejected: `cursor_line_in_node` conflates rendered layout with
  the proto schema; field number is schema-stable and what script authors
  actually want to identify a field.

## Test plan

1. `advance_when_node_predicate_path_form` — `node: /1` fires when `j`
   moves the caret to node `/1`.
2. `advance_when_node_predicate_with_field_number` — `node: /:1` fires
   when the caret moves to `/1` (a child of `/` with field_number 1);
   `node: /:2` never fires when the schema has no field 2.
3. `advance_when_line_predicate` — `line: 2` fires when `j` moves the
   caret to absolute line 2.
