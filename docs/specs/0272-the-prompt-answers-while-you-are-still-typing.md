<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0272 — the prompt answers while you are still typing

Status: implemented
Implemented in: 2026-08-10
App: protolens
Refs: docs/specs/0235-a-search-answers-while-it-is-still-being-typed.md
        (the resumable `SearchSweep` and its three prompt colors),
        docs/specs/0222-the-text-lives-in-the-nodes.md (the byte cursor a
        node's lines are read through),
        docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (a packed
        run is one arena node),
        docs/specs/0263-the-machine-sleeps-when-nobody-is-waiting.md (the
        untimed sleep this spec has to break out of),
        docs/specs/0249-a-large-document-answers-the-user-first.md (the
        `Status::Unbaked` violet)

## Background

Spec 0235 gave the search prompt three states — running, matched, and
missed — and each was wrong in a different way.

**It stalled.** `sweep_test` read each candidate with `line_text(pos)`,
which resolves the line through `line_offset` — O(that line's index
inside its owner). A packed run is a *single* arena node (spec 0216
S22), so all of its elements are lines of one text and a walk across the
run is quadratic in the run's length. Measured on `taskset -c 4-7`,
release, one `repeated int32` field:

| elements | sweep |
|---------:|------:|
| 4 000 | 0.31 s |
| 8 000 | 0.64 s |
| 16 000 | 2.33 s |
| 32 000 | 9.21 s |

A 300 000-element run — ordinary in a serialized tensor — extrapolates
to about a quarter of an hour. That is not a slow prompt; it is a prompt
that never answers.

**It did not repaint.** `may_sleep_indefinitely` (spec 0263 S2) lists
six reasons not to sleep untimed, and the search is in none of them. The
step that settles a sweep's answer sets `search_dirty` and reports
`Progressed`, so the frame is owed on the *next* pass through the
receive loop — and until spec 0263 the 250 ms activity tick delivered
it by accident. With the sleep untimed the loop parks in `rx.recv()`
holding the flag, and the prompt keeps the color of a question already
answered until some unrelated event wakes it. In practice: until the
user presses a key to ask why. The same omission is why rotating
through matches with `Ctrl-Left`/`Ctrl-Right` did not move the starker
highlight — rotation restarts a sweep whose hit lands asynchronously and
sets the same flag.

**The running color was orange.** `search_running_style` borrowed
`Tier::NonCanonical`, whose own comment names it "Yellow Orange" and
whose hue is 36°, chosen precisely so it could not be mistaken for a
yellow. On the command row it read as what it is.

And the missed color said more than the search knew: while the bake
still owes subtrees, the sweep never saw their text, so red — "this
pattern is not in the document" — is a claim it is not entitled to make.

## Goals

- **G1.** A sweep across a packed run costs time linear in the run's
  length.
- **G2.** The prompt's color follows the sweep's answer with no
  keystroke required.
- **G3.** The three prompt states are told apart by hue alone: yellow
  while running, default once matched, red once conclusively missed.
- **G4.** A miss taken over an incomplete document is reported as such,
  and revised when the document completes.

## Non-goals

- **N1.** No change to what the sweep *searches* — the candidate order,
  the wrap budget, the scopes and the path haystack are spec 0235's and
  spec 0246's, untouched.
- **N2.** No line-offset index and no byte-offset field on `LinePos`.
  The walk's steps are overwhelmingly to the adjacent line, so carrying
  one cursor across the walk is enough; storing offsets would cost
  memory on every line to serve one caller.
- **N3.** No fourth prompt hue for the provisional miss. It reuses the
  bake's violet, which is already on screen beside it.

## Specification

- **S1.** `SearchSweep` carries `offset: usize` — where its current
  line begins in its owner's text, spec 0222 S4's byte cursor — and
  `sweep_test` reads through `line_text_at(pos, offset)`.
  `advance_sweep` steps the cursor with `stepped_offset`, which scans
  to the neighboring newline when the step stays inside one node and
  falls back to `line_offset` otherwise (a different node, a bracketed
  node's derived closing brace, or a non-adjacent line, where
  `line_offset` is already cheap or is paid once). The field is zero
  and unread for the two side panes, whose candidates are list entries.

- **S2.** A miss is *provisional* when it was taken while the bake still
  owed subtrees — `search_miss_is_conclusive(scope)` is
  `scope != Main || auto_folded.is_empty()`. The sweep records it on the
  step that exhausts the walk, rather than deriving it on demand: once
  the bake finishes, "is the document complete now" can no longer tell a
  miss that was always final from one taken too early. A finished sweep
  whose miss was provisional and whose document is now complete is
  restarted, exactly once, at the first step after the last subtree
  lands. The prompt draws a provisional miss in
  `theme::search_unbaked_style` — spec 0249 S12's `Status::Unbaked`
  violet, the same color the fold margin is drawing against the very
  subtrees the answer is missing — and `App::not_found` qualifies its
  message from the same predicate.

- **S3.** `run_loop` breaks out of its receive loop when
  `app.search_frame_owed()`, ahead of the sleep decision. Breaking
  rather than shortening `deadline`, because there is nothing left to
  wait for; `take_search_dirty` past the loop clears it, so it fires
  once. `may_sleep_indefinitely`'s six inputs are unchanged.

- **S4.** `search_running_style` gets its own hue-60° yellow,
  `caret_rgb::{DARK,LIGHT}_SEARCH_RUNNING`, instead of borrowing
  `Tier::NonCanonical`.

## Alternatives considered

**Store a byte offset on `LinePos`.** It would make every caller O(1)
rather than only the walking ones, but `LinePos` is constructed
everywhere and the field would have to be kept right at each site; the
renderer has been walking with a carried cursor since spec 0222 without
needing it.

**Index each node's line starts.** Correct and O(1), but it is a
per-node allocation proportional to the line count, paid by every node
in the document to serve a search that visits each line once.

**Give the search a term in `may_sleep_indefinitely`.** It would work,
but the six existing terms are all *deferred* repaints — things owed
within an interval, which is why they shorten a deadline. A settled
sweep is owed now, and the honest expression of that is to stop
receiving.

**Restart a provisional miss when the bake finishes, from the bake.**
That puts knowledge of the search in `bake_step`. Asking on the sweep's
own next step is one predicate in one place, and the bake runs only
while the sweep reports `Idle` (spec 0255 S5's ordering), so the
question is asked exactly once anyway.

## Test plan

1. `a_sweep_across_a_packed_run_is_linear_in_its_length` — 32 000
   packed elements, a miss, under 2 s. Before S1 it took 9.2 s.
2. `a_sweep_reaches_the_right_line_of_a_packed_run_both_ways` — the
   carried cursor lands on the same line the re-derived one did, in both
   directions.
3. `a_settled_search_forces_a_repaint_with_no_events` — an idle
   `run_loop` with a pattern typed draws a frame a control run does not.
4. `an_unbaked_document_tints_a_miss_violet_rather_than_red` — the
   prompt and `not_found`'s message both answer to `auto_folded`.
5. `search_prompt_is_yellow_while_sweeping_and_red_when_finished` —
   the three states remain distinct.

## Measured outcome

The packed-run sweep, same machine and method as the table above:
4 000 → 1.3 ms, 8 000 → 1.9 ms, 16 000 → 3.7 ms, 32 000 → 7.5 ms,
128 000 → 29.9 ms. Linear, and ~1230× at 32 000 elements. Slice counts
are unchanged (5, 9, 17, 33, 129), which is what confirms the walk
itself was not shortened.

Two hypotheses were investigated and refuted, recorded so they are not
re-investigated: the sweep is **not** starved in `run_loop` (it runs
first in the `TryRecvError::Empty` arm), and `sibling_position` is
**not** a second quadratic (it is `idx - sibling_block(idx).start + 1`).
A 55.8 s sweep observed over a flat synthetic document is a fixture
artifact — `sibling_block` scans linearly for a *root*'s block, and that
fixture makes every node a root, where production has exactly one.
