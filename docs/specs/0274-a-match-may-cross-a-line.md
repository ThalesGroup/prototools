<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0274 — a match may cross a line

Status: draft
App: protolens
Refs:
- docs/specs/0273-a-pattern-is-a-path-or-a-regex.md — the path/regex
  dispatch, the smartcase rule and the literal tier, all kept; its
  newline refusal is what this spec lifts.
- docs/specs/0235-a-search-that-answers-while-you-type.md — the
  resumable `SearchSweep`, its slot in `run_loop`'s idle arm, and the
  rule that a hit is never revised.
- docs/specs/0246-a-search-stops-at-every-match.md — the origin row
  visited twice and split at the caret.
- docs/specs/0272-the-prompt-answers-while-you-are-still-typing.md —
  the prompt's three colors and the frame that buys the answer.
- docs/specs/0222-a-node-owns-its-text.md — why there is no document
  string to search.
- docs/specs/0255-a-bounded-confirm-finishes-in-the-background.md — the
  bake, and what an unbaked stop looks like from the line walk.
- docs/specs/0242-the-main-pane-selection.md — the anchor/caret model a
  multi-row hit is expressed in.
- docs/specs/0263-an-idle-protolens-sleeps.md — the untimed sleep the
  worker must not break.

## Background

Spec 0273 refuses any pattern **every** string of whose language contains
`\n`: `SearchPattern::new` answers `multi-line patterns are not
supported` for `id\nvalue`, `\n+` and `a(\n|\r\n)b`. That refusal is the
boundary of destination A. What was asked for is a search that reads the
document the way VS Code's does — the row break is a character, a match
may cross it, and the result is a multi-row selection.

Two things make this more than lifting the refusal.

**There is no string to search.** Spec 0222 gives each arena slot its own
text and derives a bracketed node's closing brace rather than storing it;
the document is never materialized, and on `googleapis.desc` it would be
~250 MB if it were. `sweep_test`'s haystack is one rendered row, produced
by `line_text_at`. A pattern that crosses a row needs a haystack that
does too.

**The single-row sweep is on the main thread on purpose.** Spec 0235 S3
put it *first* in `run_loop`'s idle arm, ahead of discard, bake and
read-ahead, so that a pattern being typed answers before background work
runs. It can be resumed a thousand candidates at a time because a
candidate is a row and no state crosses rows. A cross-row engine has
state by construction, and holding the main thread until it converges
would freeze the keyboard for the length of the document.

For scale: a full-document miss over `googleapis.desc` (5 278 324 rows)
costs 700 ms through 0273's literal tier and 773 ms through its regex
engine.

## Goals

- **G1.** `/id\nvalue` compiles, matches where the text says it should,
  and matches nothing else.
- **G2.** The document has one meaning for every non-path pattern: the
  rendered rows joined by `\n`. Whether a pattern happens to take the
  fast path is invisible in the result.
- **G3.** A hit that spans rows is reported over its whole extent, and
  `Enter` leaves it as a main-pane selection.
- **G4.** The keyboard stays free while such a search runs. No keystroke
  waits on it, the prompt keeps showing spec 0272's colors, and an idle
  protolens still has no timed wakeup.
- **G5.** Everything already on 0273's path keeps that path and its
  current cost.
- **G6.** A search over a partly baked document answers about the
  document as it stands, and says so when it cannot be sure.

## Non-goals

- **N1. No capped window.** The intermediate design — a sliding buffer
  of `SEARCH_MATCH_ROWS` rows on the main thread — is not built. It
  buys a bounded slice at the price of a cap the reader can hit
  silently, and this spec's worker removes the reason to want it.
- **N2. No multi-line path pattern.** A path pattern (0273 S2) is
  digits and slashes; it can neither contain nor admit `\n`, and it
  matches a node rather than text. It takes 0273's path arm unchanged.
- **N3. No document-wide byte monoid.** Adding `bytes_total` beside
  `lines_total` would cost 4 B on a 44 B `TreeNode` × 4.74 M slots
  (~19 MB) and one more signed difference in 0254's climb — affordable,
  but its price is really in the renderer, where a byte tally would have
  to be threaded through `IndexingTextSink` beside the line tally that
  `decode.rs`'s `text_range` is. Nothing reads it: S6's cursor counts
  bytes as it hands out chunks, and `Cursor::total_bytes` is allowed to
  answer `None`.
- **N4. The worker is not the seated crew.** Spec 0270's `Crew` exists
  to run many short parts of one heat query across every physical core.
  A multi-line search is one long indivisible scan; splitting it across
  cores would put a seam inside the haystack, which is exactly what S4
  is built to prevent. One thread, spawned per search.
- **N5. `\A` and `\z` stay refused.** 0273 S12's reason survives
  intact and gets stronger: the haystack is now a *segment*, so those
  anchors would bind to a boundary the reader cannot see and that moves
  as the bake progresses. `^` and `$` mean what they say.
- **N6. Non-current matches are not tinted for a worker pattern.**
  See S11.
- **N7. Side panes keep the single-row path unconditionally.** The
  override and manage panes list FQDNs and entries, not a document.
  There is no row-joining semantics for them to be faithful to, and no
  "next line" for a match to cross.

## Specification

### The document, and which patterns can ignore that it is one

- **S1.** For every non-path pattern the haystack is **the rendered rows
  joined by `\n`**, in document order. `multi_line(true)` and
  `dot_matches_new_line(false)` (0273 S6) are unchanged and now mean what
  they normally mean: `^`/`$` bind to a row break, `.` does not cross
  one.

- **S2.** The single-row sweep of 0273 survives as an **optimization,
  applied exactly when the pattern cannot tell the difference** — which
  is when no string in its language contains `\n`. `hir_admits_newline`
  decides:

  | node | admits `\n` |
  |---|---|
  | `Literal` | its bytes contain `\n` |
  | `Class` | some range covers `\n` |
  | `Repetition` | `max != Some(0)` and the sub-expression admits |
  | `Capture` | the sub-expression admits |
  | `Concat` | **any** part admits |
  | `Alternation` | **any** branch admits |
  | `Empty`, `Look` | no |

  It is an *any*-node question but still cannot be written with
  `fold_hir`, which descends into a repetition's sub-expression without
  asking whether it may repeat: `fold_hir` would route `a\n{0}b` to the
  cursor engine.

  0273 S11's `hir_needs_newline` — every string in the language contains
  `\n` — is the different question, and it goes away with the refusal it
  served. `\s*id` admits a newline and needs none; `id\nvalue` does both;
  `[0-9]+` does neither. Routing on `admits` is what buys G2: routing on
  `needs` would leave `a(\n|\r\n)b` matching across a row while `a\s*b`
  did not, which is a distinction no reader can be asked to hold.

  Case folding cannot change the answer: `\n` has no case variants.

- **S3.** `SearchPattern` gains a fourth variant carrying a
  `regex_cursor::engines::meta::Regex`. A pattern is compiled into
  exactly one of the four, so the two engines never both hold the same
  pattern and the only surface they must agree on is builder
  configuration: `multi_line`, `dot_matches_new_line`, `case_insensitive`
  and `size_limit`, all set from the same values 0273 S6/S9 already
  compute from the shared HIR.

### Segments — what an unbaked document means

- **S4.** The searchable document is a sequence of **segments**: maximal
  runs of rows that are contiguous in the finished document. A bake stop
  is bracketed, folded, and draws a header and a footer with nothing
  between, so the false seam is *inside* it: one segment ends on the
  stop's header row, and the next begins on its footer row. There are
  one more segments than there are stops in `auto_folded`.

  **Each segment is its own haystack.** A match must lie wholly within
  one; the walk skips the hole and resumes on the far side.

  Three consequences, all wanted:
  - A haystack never contains a false seam, so a hit is never an
    artifact of missing text. The hazard is structurally absent rather
    than guarded against.
  - When the bake is done `auto_folded` is empty, there is exactly one
    segment, and this degenerates to a plain whole-document search.
  - A hit found before the bake finishes is real and final, but is not
    necessarily the *nearest* match in the finished document. That is
    already spec 0235's contract — a hit is never revised, and only a
    miss is marked provisional.

  User folds are **not** holes. `next_line` walks `lines_total` and
  descends into folded children, so a folded subtree's rows are searched
  today and stay searched; `show_sweep_hit` already unfolds what hides a
  hit.

- **S5. A bake step never changes the inside of a segment, but it does
  change the segmentation.** A segment is delimited by exactly the
  `auto_folded` stops the bake expands, and `expand_auto_fold` splices
  under one stop, so every other segment's rows are untouched. What the
  expansion does at that one boundary depends on what it reveals: if the
  revealed body contains further stops the hole is replaced by smaller
  holes, and if it contains none the hole closes and the two segments on
  either side **join into one**.

  So a segment's *interior* verdict is stable under baking — a run of
  rows that missed will miss again — but a joined segment is not a
  segment that has been scanned. It contains two seams that never existed
  while its parts were separate, and a match may cross either.

  This is why S9 makes the segment the unit of work but **freezes the
  queue**: rescanning a join is rescanning everything on both sides of
  it, and the bake produces a join every few tens of milliseconds.

### The cursor

- **S6.** The worker searches a segment through a `Cursor`
  implementation over the arena:
  - a **chunk** is one node's `node_text` entry, or the derived closing
    brace of a bracketed node (0222 S2), which the cursor holds in a
    small owned buffer of its own. Empty entries are skipped, since
    `Cursor::chunk` may not return an empty slice.
  - `offset()` counts bytes **from the segment's first row**, not from
    the document's. `total_bytes()` answers `None`; `Input` treats that
    as `usize::MAX` and clamps `span.end` when `advance` first returns
    false, so the length need not be known in advance.
  - `advance`/`backtrack` step the document-order walk. Backtracking is
    mandatory for regex-cursor and is available here by arithmetic over
    the level-ordered arena (0216).
  - chunks never split a codepoint, so `utf8_aware()` is `true` and every
    regex feature is supported.

- **S7.** **The cursor is the abort point.** A `find` call cannot be
  interrupted from outside, but it calls `advance` once per chunk.
  `advance` returns `false` when the sweep's epoch has moved on, which
  ends the search as cleanly as reaching the end of the data; the worker
  then discards its result because it knows it aborted. No poll interval,
  no cancellation protocol, and the check is one relaxed atomic load per
  node rather than per byte.

### Ownership

- **S8.** `App::node_text` becomes `Arc<Vec<Option<Box<str>>>>`, and
  `App::tree` with it — a segment scan reads a node's line count and
  fold state as well as its text, so the two travel together. (`App::arena`
  is already an `Arc`, and is never written after the build.) The worker
  holds a clone of each for the length of **one segment scan**. Every
  main-thread writer — `splice_override`, a bake step — aborts the
  worker and joins it first, so the write always sees a refcount of 1 and
  copies nothing. Memory cost: one refcount pair per vector.

  The write goes through one accessor per field (`tree_mut`,
  `node_text_mut`) whose first act is the abort, so there is no way to
  write either field without halting the scan and nothing to remember at
  the call sites. The accessor is `Arc::get_mut(..).expect(..)`, not
  `Arc::make_mut`: `make_mut` needs `TreeNode: Clone`, and the copy it
  would silently reach for on a missed abort is the whole 200 MB
  document. `get_mut` asserts the invariant the line above establishes
  instead of papering over its absence.

  The idle-arm ordering already makes the race impossible (S10 keeps
  discard, bake and read-ahead from running while a segment is in
  flight), so the `Arc` buys nothing at run time. It buys the property
  that Rust can *check* the argument, and that a future reordering of the
  idle arm fails to compile instead of tearing a `Box<str>` out from
  under a running scan.

  The rejected alternative is `Vec<Option<Arc<str>>>`: ~75 MB of
  refcount headers over 4.7 M allocations, and it would pin a superseded
  render's text alive behind a worker, defeating spec 0256's whole
  purpose.

### The worker and the loop

- **S9. The unit of work handed to the worker is one segment, not one
  search.** The sweep holds the queue of segments S14 orders; each
  `search_sweep_step` either hands the next one out or collects the
  finished one's verdict.

  This is the whole of the answer to bake starvation. The bake cannot
  run while the worker holds the document (S8), so if the worker held it
  for the length of a search the bake would stop for that long — ten
  seconds on a large document, against the ~700 ms the single-row sweep
  costs today. Segment-at-a-time bounds the stall to the largest
  segment, and the bound is self-limiting: segments are many and small
  exactly when the bake has the most left to do, and only become one
  large segment once there is no bake left to starve.

  **The queue is frozen when the search starts.** It is the segmentation
  as it stood at that moment; segments the bake creates or joins
  afterwards do not enter it, and the search ends when the frozen queue
  is exhausted. Nothing is re-scanned across a yield, because by S5 a
  bake step cannot change the interior of a segment already answered
  for — and a segment the bake *joins* is not offered to the search at
  all, because scanning it means scanning both sides again. The material
  that only a join makes reachable is covered by S15's second pass, not
  by growing the queue.

  One thread per segment scan, spawned when the segment is handed out
  and joined when it reports or is abandoned. Spawning is tens of
  microseconds against a scan that is at minimum milliseconds, and
  join-on-abandon is what makes S8's refcount argument a one-liner
  rather than a handshake. The thread calls `affinity::widen()` on entry
  (0264).

  It reports through the existing channel with a new
  `AppEvent::SearchWorkerProgress`, exactly as the heat worker does, so
  spec 0263's untimed `rx.recv()` still wakes for it and an idle
  protolens still arms no timer. Like `HeatWorkerProgress` it is not
  counted by `InputPending`.

- **S10.** `SweepStep` gains **`Waiting`**: a segment scan is in flight.
  `run_loop`'s idle arm treats it as it treats `Progressed` for the
  purpose of the other three jobs — discard, bake and read-ahead are all
  skipped — and as it treats `Idle` for the purpose of sleeping, falling
  through to `recv`/`recv_timeout`. The main thread must not spin: a
  segment scan can run for seconds, and the drawing core is reserved
  (0265) precisely so that it is free.

  **Collecting a finished segment's verdict returns `Idle`.** That is
  the yield, and it has to be `Idle` rather than `Progressed` because
  `Progressed` `continue`s past discard, bake and read-ahead. So the
  pass that collects a verdict lets each of the other three take one
  step; the pass after that hands out the next segment.

  The bake therefore gets one step — up to `BAKE_ROW_BUDGET`, ~22 ms —
  per segment scanned. Whether that is enough is the one number this
  spec cannot argue its way to; test-plan item 18 measures it.

### The result

- **S11.** A hit's extent is two positions — `(LinePos, column)` for the
  start and for the end — rather than 0246's single row and width. A
  single-row hit is the case where the two rows are equal, and every
  existing caller reads it exactly as it reads a `SweepHit` today.

  Converting the engine's byte span to those positions is done by the
  cursor, which knows which node it handed out and at what offset. This
  is why S6's offsets need be self-consistent only within a segment, and
  why N3's monoid would not help: the row and column inside the node
  still want a scan of that node's text, and the coarse half is free.

- **S12.** **`Enter` on a hit that spans more than one row leaves it
  selected** — `select_anchor` at the match's start, the caret on its
  last character, `select_engaged` set. That is the multi-row result the
  request asked for, and spec 0242's renderer, `Ctrl-c` and
  `selected_columns` need to learn nothing. Spec 0242 S3 clears the
  selection *before* dispatch, so `apply_sweep_hit` sets it afterwards
  and the ordering already holds.

  A single-row hit sets no selection. Its behavior is settled by 0246 and
  does not change.

- **S13.** While the prompt is open, a worker pattern tints **only the
  current hit**, over its whole extent, using S11's two positions and the
  same per-row column arithmetic `selected_columns` does. Other
  occurrences on screen are not tinted (N6): finding them means running
  the multi-line engine over the visible window every frame, and the
  per-frame budget is the thing spec 0272 exists to defend.
  `render.rs`'s `find_range_from` loop is a single-row construction and
  stays on 0273's patterns only.

- **S14.** Direction, origin and wrap keep 0246's shape, expressed in
  bytes. The forward walk covers the origin's segment from the caret's
  offset to its end, then each following segment whole, wraps, then each
  preceding segment whole, and closes with the head of the origin's
  segment up to and including the caret — the same partition of the
  origin that makes a full cycle visit every match once. `Input::set_start`
  and `set_end` express the two halves; the caret's byte offset within
  its segment is obtained once by summing chunk lengths, which is O(1)
  per node and touches no text.

  Backward is the last match whose start precedes the caret, found by
  iterating forward and keeping the last. There is no reverse scan in the
  meta API, so this reads the prefix once. That cost is the worker's.

- **S15.** A miss is **provisional** iff every segment in the frozen
  queue missed and `auto_folded` is non-empty —
  `search_miss_is_conclusive` unchanged.

  With S9's frozen queue this gives a **two-pass** shape, and the second
  pass is what makes the first one's fragmentation harmless:

  1. A pass over the document as it stands. It answers from what has
     been rendered, and reports a miss as provisional.
  2. `search_sweep_step`'s existing "ask again once the bake lands"
     arm restarts the search when `auto_folded.is_empty()`. There is
     then exactly one segment, and the pass is a plain, seamless
     whole-document search.

  The gate is `auto_folded.is_empty()` — the *whole* bake, not one step
  — so this fires **once**, not after every bake step. Total worst-case
  work for a pattern that never matches is therefore two documents'
  worth of bytes, and it is bounded whatever the bake does in between.

  **A search iteration is triggered by the reader or by the bake
  landing, never by a periodic event.** That is the property that
  bounds the work, and it is why a provisional miss is a resting state
  rather than a polling loop. The reader's own gestures are the third
  and most immediate way out of one, and each already begins a fresh
  sweep and so re-segments against whatever the bake has managed:
  `n`/`N` (`run_search`), re-committing at the prompt, and Ctrl-Right/
  Ctrl-Left (`rotate_search_match`).

  The provisional violet therefore carries weight it did not carry
  before: with a frozen queue it is the only thing that distinguishes
  "not there" from "not there yet".

- **S16.** **A provisional miss rotates; a conclusive one does not.**
  Spec 0246 S21 makes `rotate_search_match` a no-op when the sweep has
  no displayed hit, which was correct when a miss was final — there was
  nothing to rotate to. Under S15 a provisional miss is an open
  question, so Ctrl-Right/Ctrl-Left on one begins a fresh sweep from
  `search_origin` in the pressed direction, re-segmenting. A miss that
  `search_miss_is_conclusive` accepts stays a no-op, and a sweep still
  walking stays a no-op (S21's other half — there is a pending answer
  already).

### Dependency

- **S17.** `regex-cursor = { version = "0.1.5", default-features =
  false, features = ["perf-inline"] }`. `default-features` matters: the
  crate's default set pulls `ropey`, which is an adapter for a rope we do
  not have and cannot use — our text is per-node and deliberately never
  contiguous.

  What is being taken on, measured against `regex-automata` 0.4.7: the
  crate **depends on** upstream and re-exports it, so every automaton is
  built by upstream code — the Thompson compiler, dense and lazy DFAs,
  `Prefilter`, `Look`, `Captures`, literal extraction. What is forked is
  the search *drivers*: 4 890 non-test code lines, of which roughly half
  are verbatim upstream and about 800 are new. The new lines are
  concentrated in three places: `Input`'s 8-byte look-around window
  across a chunk seam, `literal.rs`'s seam-crossing prefilter with its
  ambiguity resolution, and the chunk bookkeeping threaded through each
  search loop. 16 `unsafe` sites, all the same `next_unchecked!` DFA
  transition copied from upstream's own loops.

  What the fork *removes* is worth knowing: `OnePass`,
  `BoundedBacktracker`, `ReverseDFA`, `ReverseHybrid`, and the
  `ReverseSuffix`/`ReverseInner` meta strategies. The meta engine is Core
  only — prefilter, then hybrid/DFA, then PikeVM. That is the path a
  long unanchored scan takes anyway.

  Accepted risks, stated so they are not rediscovered: the seam-crossing
  prefilter is the one place a bug would be ours to find; the crate is
  unmaintained enough (last release 2025-02-26, pinned to
  `regex-automata` 0.4.7 while upstream is at 0.4.18) that a security
  bump would not reach us through it; and its own `lib.rs` calls it a
  prototype. Against that: 60 k recent downloads and `tree-house`, which
  is Helix's tree-sitter layer, exercising the same shape of workload.

## Alternatives considered

**Feed the document through a lazy DFA byte by byte.** `regex-automata`
does expose `next_state(cache, state, byte)`, so this is possible — the
claim in 0273's notes that it is not was wrong. It is still the wrong
tool. It gives up the prefilters, which work by *skipping* to candidate
offsets and need random access, so all 5.28 M rows would go through the
DFA. Match start would need a reverse DFA run anyway. And there is no
stream to feed: 0222 means the byte sequence does not exist until
protolens builds it, which is the cursor either way.

**Materialize the document into one `String` for the search.** ~250 MB
on `googleapis.desc`, allocated per search, freed per keystroke. It also
resurrects exactly the `App::lines` array that 0222 deleted.

**Keep it on the main thread with a bounded resume.** Resuming a
cross-row engine means carrying its cache and its partial match across
slices, and the partial match may be arbitrarily long. The one shape
that does bound it is N1's capped window, which caps the reader instead.

**Split the segment across the seated crew.** N4. Any split is a seam,
and a seam is the one thing S4 is built to avoid.

**Give the worker the whole search and block the bake for its
duration.** This is what the first draft of S9 said, and it is wrong.
The bake would stop for as long as the search runs — seconds on a large
document, and again on every keystroke, since an edit restarts the
sweep. Segment-at-a-time costs nothing to get instead, because S5 makes
a segment's interior stable under baking.

**Let segments the bake creates join the running queue.** The second
draft of S9, and also wrong. A bake step that closes a hole *joins* the
segments on either side (S5), and the joined segment has two unscanned
seams — so honoring it means rescanning both sides. A join lands every
~22 ms and each one is larger than the last, so a pattern that never
matches would rescan a growing prefix of the document tens of times per
second and converge only when the bake did: quadratic work for a search
that has already failed. The frozen queue plus S15's second pass gets
the same final answer for two documents' worth of bytes.

**Let the bake run and abort the worker whenever it mutates.** The
mirror image, and worse: a bake step lands every ~22 ms, so a search
long enough to matter would be aborted before it converged, every time,
forever. Yielding *between* segments is the same idea placed where it
terminates.

**Refuse to search at all until the bake finishes.** Would remove S4
and S5 entirely and always give a final answer. Rejected because it
removes spec 0235's whole premise for the case that needs it most: a
reader who opens a 25 MB descriptor set and types a pattern would get
no answer, and no partial answer, for the length of the bake.

**Route on `hir_needs_newline`.** Would make this spec a much smaller
change, and would leave `a(\n|\r\n)b` and `a\s*b` disagreeing about
whether the document has rows in it. G2 is worth the larger change.

## Test plan

1. `a_pattern_that_can_match_a_newline_reaches_the_worker` — the S2
   table, one case per row, including that `\s*id` and `[^a]` route to
   the worker while `[0-9]+` and `id` do not.
2. `a_pattern_that_cannot_match_a_newline_takes_the_single_row_path` —
   0273's variants are still built for every pattern it built them for,
   and the literal tier still fires.
3. `a_match_across_two_rows_is_found_at_the_right_place` — `id\nvalue`
   on a fixture, asserting both endpoints.
4. `a_match_does_not_cross_a_bake_hole` — a fixture with one
   `auto_folded` stop, a pattern that would match only across the seam,
   and a miss.
5. `a_match_after_a_bake_hole_is_still_found` — the same fixture with
   the match wholly inside the following segment. This is the rule the
   user corrected: reaching a hole is not the end of the search.
6. `every_segment_missing_is_a_provisional_miss` — and one segment
   hitting is not.
7. `a_multi_row_hit_becomes_a_selection` — `Enter`, then
   `selection_span` equals the match's extent.
8. `a_single_row_hit_still_sets_no_selection` — S12's second half.
9. `the_two_engines_agree_on_a_pattern_that_admits_no_newline` — a
   property test over a fixture: for patterns where
   `hir_admits_newline` is false, the worker path and the single-row
   path report the same first hit. This is what makes S2 an
   optimization rather than a second semantics.
10. `an_aborted_search_stops_at_the_next_chunk` — S7, via a cursor
    counting `advance` calls after the epoch moves.
11. `a_segment_scan_does_not_let_the_bake_run` — S10: `bake_step` is
    not called while `search_sweep_step` reports `Waiting`.
12. `the_bake_takes_a_step_between_two_segments` — S9's yield, the
    other half of item 11 and the one that would silently regress.
13. `a_segment_the_bake_creates_does_not_join_the_queue` — S9's frozen
    queue and S15's two passes, on a fixture where expanding a stop
    joins two segments over a match that was not reachable when the
    search started. The first pass must report a *provisional* miss
    without growing its queue; the second, after
    `auto_folded.is_empty()`, must find the match.
14. `a_provisional_miss_rotates_and_a_conclusive_one_does_not` — S16,
    both halves: Ctrl-Right on a provisional miss begins a fresh sweep
    and re-segments, while on a conclusive miss and on a sweep still
    walking it remains 0246 S21's no-op.
15. `an_idle_protolens_still_sleeps_untimed` — the existing 0263
    re-exec-over-a-pty test, unchanged, with a completed worker search
    in the session.
16. `a_haystack_anchor_is_still_refused` — N5, `\Aid` and `id\z`, now
    that a segment exists for them to bind to.
17. Cost of the routing change: re-run 0273's test-plan item 14
    harness (`/tmp/search_time.py`, shortening its post-`Enter` drain
    first) on `zzqqxx` to confirm G5 — the single-row path's 700 ms is
    unchanged — and take a first number for `zzqqxx\nqq` on the worker.
18. **Bake convergence under a live multi-line search.** Drive
    `googleapis.desc` over a pty, open `/` and hold a worker pattern
    that misses, and time the bake to `auto_folded.is_empty()` against
    the same bake with no search running. S9's one-step-per-segment
    yield is a guess; this is the number that says whether it stands,
    and it is the item most likely to send the spec back.

## Measured outcome

**Test-plan item 18 — the bake under a live multi-line miss.** protolens
driven over a 50x200 pty on `googleapis.desc` (25.6 MB, as both schema and
blob), pinned to `taskset -c 4-11`, timing the bake from the first trace
line to `auto_folded.is_empty()`. Three runs each:

| | bake converged |
|---|---|
| no search | 8.55, 8.53, 8.53 s |
| `/zzqq\nxxww` (a miss) held live | 10.82, 11.31, 11.12 s |

**S9's yield stands.** The bake is slowed by about 30% and is not
starved: it finishes while the sweep is still walking. The sweep hands
out 7 248 segments between t=2.64 s and t=10.40 s — more than the 6 879
the document starts with, because S15 restarts it each time the bake
changes the document underneath it, and it is still cheaper to restart
than to hold the reader's answer back.

The two costs are not additive in the way the naive reading of S10
suggests: a segment scan runs on a worker while the main thread sleeps in
`poll(2)`, so the +2.5 s is the collect-and-hand-out passes and the
restarts, not the scanning.
