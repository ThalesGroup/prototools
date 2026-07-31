<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0215 — the cursor knows which line it is on

Status: implemented (steps 1 and 2; step 3 withdrawn)
Implemented in: 2026-07-30
App: protolens
Refs: docs/specs/0210-a-node-counts-its-own-lines.md
        (`absolute_start`, `next_visible`/`prev_visible` and the
        `window_nodes` cache this spec builds on),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (the `PROTOLENS_TRACE` instrumentation the numbers below come
        from, and the `sibling_position` fix this is the sequel to —
        its title is the property this spec restores for a *key*),
      docs/specs/0194-the-cursor-is-a-caret.md
        (`caret_bounds`, `desired_column`, `CaretAnchor`),
      docs/specs/0199-the-arrow-keys-fold-before-they-leave-the-node.md
        (`carry_caret`'s anchored/free rule, preserved exactly),
      docs/protolens/rendering-worklist.md (the hot-path work this
        continues)

## Background

Reported by the user: load `googleapis.desc`, press `G`, hold `PageUp`.
Release the key and the screen keeps scrolling for a second or two —
keystrokes have queued up faster than they can be processed. Do the same
from `gg` with `PageDown` and there is no backlog at all.

The obvious explanations are both wrong. It is not the heat subsystem:
heat requests are queued and answered on a worker thread, and cannot
block the input loop. It is not direction either. Measured with a pty
driver sending 150 page keys at a fixed 30 Hz without waiting for the
app (50x200 terminal, so 48 rows to a page), mean of the `key` trace
line:

| scenario | cursor position | `key` mean |
| --- | --- | ---: |
| `gg`, then hold PageDown | start of document | **47 µs** |
| `G`, then hold PageUp | end of document | **13 624 µs** |
| `G`, 150 PageUp, then hold PageDown | end of document | **22 781 µs** |

PageDown *near the end* is the worst of the three. The variable is where
the cursor is, not which way it is going.

`move_page_up` and `move_page_down` (`navigation.rs`) are
`for _ in 0..page { self.move_up() }` — 48 independent single-line
moves. Each ends in `carry_caret` → `caret_bounds`, and `caret_bounds`
performs **three** whole-document walks:

1. `cursor_line()` → `absolute_start(cursor)` (`lines.rs`), which chases
   the `prev_sibling` chain at every level of the root path, summing
   `lines_total`.
2. `row_text` → `display_row_source(Committed(l))`, which eagerly
   computes `node_at_header_line(l)` — a `line_pos` descent — and whose
   result `row_text` then **discards** (`let (full_content, _)`).
3. `row_text` → `fold_marker(row)` → `display_row_source` a second time
   → the same descent again.

Walks 2 and 3 go through `line_pos`, which first tries `cached_line_pos`
— a binary search over the last *drawn* window. During a page jump every
intermediate position is off that window by construction, so every one
of the 48 steps misses and descends. That is ~144 full-document walks
per keypress.

On the reference corpus the root has 7 771 children, so each walk is
~7 771 random reads into a 4.5 M-node arena. 48 x 3 x 7 771 x a cache
miss is the 13-23 ms in the table. Near the top of the document the
cursor's root-level ancestor is child #0, both chases are empty, and the
same key costs 47 µs.

`absolute_start`'s own doc comment already states the rule this violates:
"That is fine for a teleport (search, jumplist, click, `gg`/`G`) and must
be kept off everything else." `carry_caret` is on the per-step path.

The cost is not confined to the page keys. `cursor_line()` is also called
several times per *frame* — `render`'s cursor-row highlight, its caret
cell, the status bar's line number, and `cursor_display_row` (which
`clamp_pan_offset` calls too) — plus `absolute_start(self.cursor)` in
`brace_pair`. That is visible in the same traces as a `draw key` mean of
3 114 µs deep in the document against 1 147 µs at the top.

## Goals

- **G1.** A page key costs a bounded number of document walks, not one
  per row of the pane.
- **G2.** No change to where the cursor or the caret ends up, for any
  key, anywhere in the document. In particular `carry_caret`'s
  anchored/free rule (spec 0199) is untouched.
- **G3.** No new correctness obligation spread across the code base. A
  cache that can go stale must fail by *missing* — costing the walk it
  was avoiding — never by returning a wrong line.

## Non-goals

- **N1.** Making `absolute_start` itself cheap. Its cost is the root's
  fanout, and removing that means giving nodes a stored line offset,
  which spec 0210 deliberately did not do (an offset must be repaired
  across every following sibling on every splice; a count need not).
  This spec reduces how *often* it is called, not what it costs.
- **N2.** The `window_nodes` cache's scope. It stays a snapshot of the
  last drawn window; step 3 does not merge the two caches.
- **N3.** Fixing `line_pos` misses in general. Only the caret path is
  addressed, because only there is the answer already known.
- **N4.** Any change to `cursor_moves` accounting (spec 0208's attention
  signal). A page key still counts as `page` moves, exactly as now.

## Specification

Three independent steps, in this order. Each is complete on its own and
each is separately measurable.

### Step 1 — hoist the caret fix-up out of the loop

- **S1.** `move_up` and `move_down` split into a stepping half and a
  caret half. `step_up`/`step_down` move `cursor`/`cursor_footer` and
  increment `cursor_moves`, exactly as today, and return whether they
  moved. `move_up`/`move_down` become that step followed by
  `carry_caret`, so single-key `k`/`j`/arrow behavior is byte-identical.
- **S2.** `move_page_up`/`move_page_down` call the stepping half `page`
  times, then `carry_caret` **once** — and only if at least one step
  succeeded, so a page key at a document end remains the exact no-op it
  is today.
- **S3.** Nothing reads `cursor_column` between the steps, which is what
  makes this safe: `carry_caret` overwrites it from `desired_column` and
  `caret_anchor`, neither of which the loop touches, so the 47 discarded
  intermediate values cannot be observed.

Effect: 144 walks per page key become 3.

### Step 2 — stop rediscovering the node the cursor already is

- **S4.** `display_row_source` splits. `display_row_text(row) -> &str`
  returns the text half only — for a committed row that is a plain
  `self.lines[l]` index and never a walk. `display_row_source` keeps its
  present signature for the callers that genuinely need the owner.
- **S5.** `window_text` and `row_text` switch to `display_row_text`,
  which removes walk 2 outright — for `row_text` it was a value it threw
  away, and for `window_text` it was never used either.
- **S6.** `fold_marker` gains a `fold_marker_of(row, owner:
  Option<usize>)` form; `fold_marker(row)` becomes
  `fold_marker_of(row, self.display_row_source(row).1)`, so existing
  callers are unchanged. `row_text` likewise gains `row_text_of(row,
  owner)`.
- **S7.** `caret_bounds` calls `row_text_of` with the owner it already
  holds: `(!self.cursor_footer).then_some(self.cursor)`. This is exactly
  what `node_at_header_line` would have returned — that function yields
  `None` for a footer line and the owning node for a header line, and
  `cursor_line()` is by definition the cursor's own header or footer
  line — so the value is reproduced, not approximated.

Effect: the remaining 3 walks per page key become 1.

### Step 3 — cache the cursor's line, keyed by the cursor

**Withdrawn, not deferred.** S12 gated this step on measurement, and
before it could be taken spec 0216 removed the thing it would have
cached. Under a frozen, level-ordered arena a node's preceding siblings
are a contiguous run, so `absolute_start`'s sum stops being ~7 771
random reads into a 4.5 M-node arena and becomes a 31 KB sequential
scan — the walk this cache exists to avoid is no longer worth avoiding.
The invalidation key would also have been unsound in the other
direction: `structural_version` never changes once there are no splices
left to make, so a stale entry could not be detected by it. See spec
0216 S23.

The rest of this section is kept as the record of what was considered.

- **S8.** `App` gains
  `cursor_line_cache: Option<(usize, bool, u64, usize)>` — the node, the
  footer flag, the `structural_version`, and the resulting line.
- **S9.** `cursor_line()` returns the cached line when all three key
  components match the current cursor and version, and otherwise walks,
  stores, and returns. **The cursor position is itself the cache key.**
  That is what satisfies G3: none of the nine sites that assign
  `self.cursor` need to know this field exists. They simply cause the
  next read to miss, which costs today's walk.
- **S10.** `step_up`/`step_down` additionally *maintain* the entry:
  `prev_visible`/`next_visible` already return how many absolute lines
  the step covered as their second tuple element, so when the entry is
  live the step subtracts or adds that delta and re-stamps, turning what
  would be 48 consecutive misses into 48 arithmetic updates. When it is
  not live the step leaves it alone and the next read walks once.
- **S11.** The key's soundness rests on one claim, which must be
  verified before this step lands rather than assumed: a node's absolute
  line can change only through `folds_changed` or the splice path in
  `override_apply`, and both bump `structural_version`. Those are today
  the only two writers of that counter. If the audit finds a third
  mutation point, it gains a bump or this step does not land.
- **S12.** Step 3 is gated on measurement. Land steps 1 and 2, re-run
  the three scenarios, and only proceed if the residual per-key and
  per-frame walk cost is still visible in the trace. It is the only one
  of the three that adds state to maintain, and it should not be paid
  for speculatively.

## Divergences from the specification as written

- **`fold_marker_of` takes the owner alone, not `(row, owner)`.** S6
  prescribed both. Once the owner is known the row is never consulted,
  so the second parameter would have been unused — a lint, and a
  misleading signature. `row_text_of` does keep both, because it reads
  the row's text.
- **Step 3 was withdrawn rather than measured** (see above).

## Test plan

1. `a_page_key_lands_where_forty_eight_single_keys_land` — from several
   positions including both document ends, assert `move_page_up` leaves
   `cursor`, `cursor_footer`, `cursor_column` and `cursor_moves` equal to
   48 `move_up` calls. This is the whole of G2 for step 1, and it is the
   test that would catch the hoist changing a clamped caret column.

   *Implemented as `a_page_key_lands_where_a_page_of_single_keys_lands`*
   — the page size is `main_area.height`, which a test sets, so naming
   a specific count in the test name would have been wrong. Run over
   every start position on a deliberately ragged fixture, in both
   directions, under all three anchors, since `Free` is the only one
   whose column depends on the rows crossed.
2. `a_page_key_at_a_document_end_is_a_no_op` — at the first line,
   `move_page_up` changes nothing at all, including `cursor_column`.
   Guards S2's "only if a step succeeded" clause.
3. `the_caret_bounds_owner_matches_the_line_lookup` — for a header-line
   cursor and a footer-line cursor, assert
   `(!cursor_footer).then_some(cursor) == node_at_header_line(cursor_line())`.
   This is S7's equivalence stated as an executable claim rather than as
   a comment.
4. `a_fold_marker_row_still_reports_its_expanded_text` — `row_text` over
   a folded node still inserts `" ... }"` after the split of S4-S6, i.e.
   the text path did not lose the fold expansion the caret can sit on.
Items 5 and 6 test step 3 and were not written, since step 3 was
withdrawn.

5. `the_cached_cursor_line_is_dropped_when_the_document_moves` (step 3)
   — read `cursor_line()` to fill the cache, fold a node *above* the
   cursor, read again, and assert the new value equals a fresh
   `absolute_start`. The fold must be above the cursor: a fold below it
   moves nothing, so it would pass even with the invalidation removed.
6. `stepping_maintains_the_cached_cursor_line` (step 3) — after a page
   key, the cached line equals `absolute_start(cursor)` computed from
   scratch. Catches a sign error in S10's delta arithmetic, which is
   otherwise silent.

## Open questions

None.

## Measured outcome

Measured 2026-07-30, same harness as the before-table: the pty driver at
50x200 on `googleapis.desc` (used as both blob and `--descriptor-set`),
150 page keys at a fixed 30 Hz with no waiting for the app,
`PROTOLENS_TRACE` parsed for the `key` and `draw` lines.

| scenario | `key` before | `key` after | |
| --- | ---: | ---: | ---: |
| `gg`, then PageDown | 47 µs | **11 µs** | 4x |
| `G`, then PageUp | 13 624 µs | **356 µs** | 38x |
| `G`, 150 PageUp, then PageDown | 22 781 µs | **163 µs** | 140x |

G1 is met: the cost of a page key no longer depends on where in the
document it is pressed to anything like the degree it did. What remains
is ~3x between the ends of a 4.5 M-node arena, against ~480x before.

Three things worth recording, because none of them is what the table
alone would suggest.

- **The cheap case got cheaper too**, 47 µs to 11 µs. Step 1 cannot
  explain that: at the top of the document the sibling chases are empty
  and the 48 hoisted `caret_bounds` calls were already nearly free. It
  is step 2 removing walk 2 outright — the `node_at_header_line` result
  `row_text` computed and then discarded on *every* row of *every*
  frame, everywhere in the document.
- **Per-frame cost did not move.** `draw key` is 1 292 µs at the top
  against 3 003-3 622 µs deep in the document, versus 1 147 µs and
  3 114 µs before. That is the figure step 3 was answerable for, and
  step 3 was withdrawn — so this is the expected result, not a
  shortfall. Spec 0216 S23 is where it is now addressed.
- **The reported symptom is only partly gone, and the remainder is a
  different subsystem.** After the `G`+PageUp burst the terminal still
  produces output for ~25 s. It is no longer a keystroke backlog: 150
  keys at 356 µs is 53 ms of input handling in total. It is heat — 384
  of the 641 frames in that run are `draw heat`, arriving at spec 0192's
  50 ms repaint interval as the worker completes requests queued for the
  rows the burst flew past. The cursor stops when the key does; what
  keeps changing is the cue column. Whether *that* wants bounding is a
  separate question from this spec's.
