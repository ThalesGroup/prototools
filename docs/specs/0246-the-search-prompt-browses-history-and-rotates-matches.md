<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0246 — the search prompt browses history and rotates matches

Status: implemented
Implemented in: 2026-08-05
App: protolens
Refs: docs/specs/0235-….md (the resumable `SearchSweep`, its origin, the
        S7 restart rule, and the two-haystack rule this spec has to give
        a position to)

## Background

Spec 0235 made `/` and `?` incremental: every keystroke restarts a
`SearchSweep` from the position the prompt opened at, and the first
match it finds is previewed without moving the cursor. That leaves
four things wrong.

1. **A pattern typed a minute ago has to be typed again.** `Enter`
   stores it in `last_search`, which only vim's empty-pattern reuse and
   `n`/`N` can reach — one pattern deep, and only the most recent.
2. **The preview shows the *first* match and nothing else.** To see the
   second one the user must commit the search and press `n`, which
   moves the cursor and records a jumplist entry — a decision, when all
   that was wanted was a look.
3. Consequently **`Enter` can only ever land on the first match.**
4. **A search stops once per row, not once per match.** A `SweepCursor`
   names a candidate — a document line, or a side-pane entry — and
   `sweep_test` takes that candidate's *first* match, so `n` on
   `a: "xx"` with pattern `x` skips straight to the next line. vim does
   not do this: its `n` visits every occurrence, including several on
   one line. Two consequences fall out of the same defect:
   - `?` and backward `n` take the first match on the previous row
     rather than the last, so they land right of where the user is
     looking;
   - nothing else in the pane can express "the next match after this
     one", which is exactly what (2) needs.

## Goals

- **G1.** `Up`/`Down` at a `/`/`?` prompt browse previously committed
  patterns.
- **G2.** `Ctrl-Right`/`Ctrl-Left` at a `/`/`?` prompt step the preview
  through the matches of the pattern as currently typed, without
  moving the cursor.
- **G3.** `Enter` commits the match the prompt is *showing*, not
  necessarily the first one.
- **G4.** Editing the pattern discards any rotation: the search
  restarts from the prompt's origin, as spec 0235 S7 already says.
- **G5.** A main-pane search stops at every **match**, not at every
  row — for the prompt, for `n` and for `N` alike.

## Non-goals

- **N1.** No history for the `:` command line. `Up`/`Down` stay unbound
  when `command_kind` is `Command`. A command history is a separate
  feature with its own completion interactions (spec 0237's `Tab`), and
  nothing here needs it.
- **N2.** No persistence across runs. The history lives in `App` and
  dies with the process; protolens writes no state file today and this
  is not the feature that should introduce one.
- **N3.** No prefix filtering on `Up` (vim's `/` prompt narrows its
  history to entries starting with what is typed). See Alternatives.
- **N4.** **The side panes stay row-granular.** Match granularity is a
  main-pane property because only the main pane has a caret column; the
  override and manage panes highlight a whole entry, so a second stop
  inside one entry would draw as nothing having happened. Their stop
  count must stay equal to their entry count.
- **N5.** No new indicator of *which* match is showing ("3 of 17").
  Counting matches means finishing the sweep, which is what 0235
  refused to do per keystroke.
- **N6.** The all-matches highlight (`search_highlight_pattern`) is not
  touched. It already tints every occurrence; only the walk's stops
  change.

## Specification

### Match granularity

- **S1.** A candidate is still a row — a row is the unit of *work*, and
  the slice budget still counts rows. What changes is that a row can be
  entered with a **bound** on which of its matches count:

  ```rust
  enum RowBound {
      /// Every match in the candidate is a stop.
      Whole,
      /// No stop here (N4: a side pane's origin entry, on the way out).
      Nothing,
      /// Only matches whose start lies in this byte range — the main
      /// pane's origin row, whose two visits split it at the caret.
      Starts(Range<usize>),
  }
  ```

  `sweep.at` becomes `Option<(SweepCursor, RowBound)>`.

- **S2.** **The origin row is visited twice**, once on the way out and
  once at the end of the wrap, instead of being skipped as it is today.
  `begin_search_sweep` therefore starts *at* `origin.at` with
  `remaining = n + 1`. `advance_sweep` gives every candidate
  `RowBound::Whole` except the one produced by `next_candidate` that
  equals `origin.at` — reached exactly once, since `next_candidate` is
  a bijection over the candidates — which gets the closing bound.

- **S3.** With the origin row's caret at byte offset `c`, the two
  bounds are:

  | | out (first visit) | back (last visit) |
  |---|---|---|
  | forward | `Starts(c+1..MAX)` | `Starts(0..c+1)` |
  | backward | `Starts(0..c)` | `Starts(c..MAX)` |
  | side pane (N4) | `Nothing` | `Whole` |

  Strictness is what makes `n` leave the match it is standing on, and
  the two halves partition the row, so a full cycle visits every match
  exactly once and returns to where it started.

- **S3a.** A stop's position is `max(match start, row indent)`, not the
  match's own start. The bounds above come from a **caret**, and spec
  0194 S3 keeps a caret out of the indentation, so a match reaching back
  into the indentation has to be compared at the column the caret will
  actually occupy. Compared at its own start instead, the row splits
  *ahead* of the stop the search just landed on and the next `n` finds
  the same stop again, forever — which is what a backward `n` over an
  indented row did before this rule. The cost is that several matches
  inside one row's indentation collapse to a single stop, which is the
  same thing the caret does to them.

- **S4.** Within an eligible range, the direction picks the match:
  forward takes the **first** eligible start, backward the **last**.
  Taking the last is what fixes Background 4's second consequence — a
  backward search over `a: "xx"` must land on the right-hand `x`.
- **S5.** Successive matches may **overlap**: the scan for the next
  eligible start resumes one character past the previous start, not
  past its end, so pattern `aa` in `aaa` has stops at 0 and 1. This is
  vim's rule and it is also the only one consistent with S3's
  partition.
- **S6.** `SearchPattern` gains `find_range_from(haystack, from)` —
  `find_range` over `haystack[from..]`, shifted back. Both of S4's
  picks are loops over it, so the `memchr2` prefilter and both of its
  ASCII guards (spec 0235 S1) are inherited unchanged.
- **S7.** `SweepHit` gains `start: usize`, the match's byte offset in
  its haystack. Rotation (S18) needs it, and it is what S3's `c` is
  compared against. `column`/`width` stay as they are — chars, for the
  caret and the tint.
- **S8.** `SearchOrigin.at` gains the caret's byte offset in its row,
  converted once at construction from `cursor_column` (chars) against
  `line_text`. Side-pane origins carry no offset; N4's
  `Nothing`/`Whole` pair needs none.
- **S9.** A **path** match (spec 0235 S19/S22 — the pattern matched the
  row's positional path, which is not on screen) is one stop per row,
  at the row's first non-blank, which is also the column it has always
  landed on. Its position is the same one S3a gives a text match in the
  indentation, so the bound admits it exactly once per cycle like any
  other match.

  The path is the row's stop only when the row's **text has no match at
  all** — a text match the *bound* merely excluded still means the row
  belongs to its text. Without that test the origin row would offer its
  text matches on one visit and a path stop on the other, and the walk
  would visit it twice per cycle. The extra scan it costs falls only on
  a bounded row, and the origin is the only row a sweep ever bounds.
- **S10.** `n`/`N` inherit all of this: `run_search` already builds its
  origin with `search_origin_for` and its walk with
  `begin_search_sweep`, so G5 costs it no new code.

### History

- **S11.** `App::search_history: Vec<String>`, oldest first, **shared
  by all three scopes**. vim keeps one search history across buffers,
  and a per-scope history would make `Up` in the manage pane skip a
  pattern typed seconds earlier in the main pane.
- **S12.** A committed pattern is pushed — whatever its source,
  including vim's empty-pattern reuse of `last_search`. An entry equal
  to one already stored is **moved** to the end rather than duplicated.
  `Esc` pushes nothing.
- **S13.** Capped at 50 entries, oldest dropped first. 50 is vim's
  default `'history'`; there is no measurement behind it and none is
  needed — the entries are short strings.
- **S14.** Browse state is one field, `App::search_browse:
  Option<SearchBrowse>` where `SearchBrowse { index: usize, draft:
  String }`; `None` means the buffer is the user's own text. `index`
  indexes `search_history`, `draft` is the buffer as it stood when
  browsing started.
  - `Up`, not browsing — do nothing if the history is empty; otherwise
    stash the buffer as `draft` and recall the **newest** entry.
  - `Up`, browsing — recall the entry before `index`; at index 0 do
    nothing (**no wrap**, and no message: a prompt is not the place for
    one).
  - `Down`, not browsing — do nothing.
  - `Down` at the newest entry — restore `draft`, leave browse state.
  - `Down` otherwise — recall the entry after `index`.
- **S15.** Recalling sets `command_buffer`, sets `command_cursor` to
  the entry's char length, and calls `restart_search_sweep()` — a
  recall *is* a pattern change, so G4 covers it too.
- **S16.** `restart_search_sweep` clears `search_browse`. That is the
  one place it is cleared and it is on the path of every editing key
  (`Char`, `Backspace`, `Delete`, `Ctrl-K`), so any edit ends the
  browse; S15's recall therefore sets `search_browse` *after* calling
  it. `start_search_prompt` and `cancel_search` clear it too, so no
  browse survives a prompt.

### Rotation

- **S17.** `Ctrl-Right` shows the next match **forward in the
  document**, `Ctrl-Left` the next one backward — absolute directions,
  independent of whether the prompt is `/` or `?`. The arrow key means
  what it draws.
- **S18.** A rotation replaces `App::search_sweep` with a fresh sweep
  from `begin_search_sweep(pattern, rotation_dir, origin')`, where
  `origin'` is the stored `search_origin` with its cursor and byte
  offset replaced by the displayed hit's `at` and `start`. S2's
  two-visit walk then does the rest: it steps off the shown match and
  cycles back to it, so a single-match pattern rotates to itself.
- **S19.** **`App::search_origin` is not touched.** It stays where the
  prompt opened, which is what `Esc` restores and what every subsequent
  edit re-searches from (G4). Only the live sweep moves.
- **S20.** `command_kind` is not touched either: `Ctrl-Left` inside a
  `/` prompt leaves it a `/` prompt, so `Enter` still records
  `SearchDir::Forward` in `last_search` and `n` still means forward.
- **S21.** A rotation key is **ignored** when there is nothing to
  rotate from — no sweep, or a sweep whose `found` is `None` (still
  walking, or finished having missed). Rotating from a guess is worse
  than not rotating.
- **S22.** A rotation runs through the normal incremental machinery: it
  sets `search_dirty` and lets `run_loop`'s idle arm walk it. It does
  **not** block. While the new sweep runs, the prompt tint returns to
  "running" (spec 0237 S11) and `search_current_cell` has no answer, so
  the previous match loses its emphasis before the next gains it. That
  is the honest report of an unfinished search and is not worth
  suppressing.

### Commit

- **S23.** No new commit logic. `commit_search` already takes the live
  sweep, runs it to completion (a no-op on a finished one, whose
  `found` therefore survives) and applies `found` — so once S18 leaves
  the rotated hit there, G3 holds. This spec's contribution to G3 is a
  **test that pins it**, because the property is currently incidental.

### Key routing

- **S24.** All four keys are new arms in `handle_command_key`'s `match
  key.code`, guarded on `matches!(self.command_kind,
  CommandLineKind::Search(_))`. The `Ctrl-Right`/`Ctrl-Left` arms must
  precede the existing plain `Left`/`Right` arms, exactly as the `ALT`
  arms already do — the function's `ctrl_or_alt` gate only matches
  `KeyCode::Char(_)`, so without a preceding arm a `Ctrl-Right` falls
  through to plain `Right` and moves the text cursor.

## Alternatives considered

**Putting the column inside `SweepCursor::Line`.** The obvious way to
make the walk match-granular: `Line(LinePos, usize)`. Rejected because
the cursor is also the *identity* of a candidate — `next_candidate`
enumerates them, `remaining` counts them, and S2's "have we come back to
the origin?" test is an equality on it. A column in there makes every
one of those mean something subtly different, and makes the slice budget
count matches rather than rows, so a row of 400 matches would become 400
slices' worth of accounting for one row's work. The bound rides
*alongside* the cursor instead, which leaves all four properties intact.

**Row-granular rotation, match-granular `n`.** Rejected on sight: the
prompt's preview and `n` must agree, or `Enter`-then-`n` skips the match
the preview had just shown.

**Prefix-filtered history (N3).** vim's `/` prompt restricts `Up` to
entries starting with what is typed. Rejected here because it stops
`Up`/`Down` being inverses unless the stash also records the filter, and
because each filtered recall still restarts a sweep, so a mistyped
prefix costs a walk and then has to be edited back out. Worth revisiting
on its own, not as a rider.

**Rotating by advancing the existing sweep instead of replacing it.**
Tempting — `advance_sweep` already sits on the right cursor. But a
finished sweep has `at == None` (that is what `is_finished` means) and
its `remaining` is spent, so continuing it needs both fields
resurrected, and backward rotation needs `dir` flipped mid-walk with the
wrap budget recomputed. Building a fresh sweep from the shown hit says
the same thing with the constructor that already gets every one of those
details right.

**Making rotation move the cursor, and `Esc` undo it.** Rejected: spec
0235 S8 draws a hard line — a sweep never moves the position, only
`Enter` does. Rotation is a sweep. Keeping the line is what makes `Esc`
a single restore of scroll and pan rather than a jumplist rewind.

**`Ctrl-N`/`Ctrl-P` for rotation or history**, as readline has it.
Rejected: they are `Char` keys, so they land in the `ctrl_or_alt` gate
among the readline editing chords — and the user asked for the arrows.

## Test plan

Match granularity:

1. `n_stops_at_every_match_on_a_line` — three occurrences on one row;
   three `n` presses give three distinct caret columns before the row
   changes.
2. `a_backward_search_lands_on_the_last_match_of_the_row` — S4, the
   Background 4 defect.
3. `a_search_wraps_back_to_the_match_it_started_on` — S2/S3's
   partition: with one match in the document, `n` returns to it.
4. `overlapping_matches_are_separate_stops` — `aa` in `aaa` (S5).
5. `the_caret_row_is_searched_ahead_of_the_caret` — a `/` prompt finds
   a match later on the caret's own row, which S2 makes reachable.
6. `a_path_match_is_one_stop_per_row` — S9: `n` over a run of rows
   matching only on their positional path visits each once.
7. `the_manage_pane_still_stops_once_per_entry` — N4.

History:

8. `up_at_a_search_prompt_recalls_the_last_committed_pattern` — and
   `command_cursor` sits at its end.
9. `down_past_the_newest_history_entry_restores_what_was_typed`.
10. `editing_after_a_history_recall_ends_the_browse` — S16.
11. `the_search_history_is_shared_across_panes` — S11.
12. `a_repeated_pattern_moves_to_the_end_of_the_history` — S12.

Rotation and commit:

13. `ctrl_right_previews_the_next_match_without_moving_the_cursor` —
    the cursor and the jumplist are untouched; `found` has moved on.
14. `ctrl_left_rotates_backward_in_a_forward_prompt` — S17.
15. `rotation_wraps_back_to_the_only_match` — S18.
16. `rotation_is_ignored_while_the_sweep_is_still_walking` — S21.
17. `enter_commits_the_rotated_match_not_the_first_one` — G3/S23, the
    point of the whole spec.
18. `typing_after_a_rotation_searches_from_the_prompts_origin` — G4:
    rotate twice, type one more character, assert the new hit is the
    first match after the *origin*.
19. `esc_after_a_rotation_restores_the_opening_view` — S19.
20. `ctrl_right_at_a_colon_prompt_does_not_rotate` and
    `up_at_a_colon_prompt_is_still_inert` — N1/S24, including that
    `Ctrl-Right` no longer reads as a plain `Right`.

## Measured outcome

No corpus measurement was taken, because the question the Measured
outcome was written to ask turned out to be answerable exactly.

S4's backward pick does turn a row's single `find_range` into a scan of
all that row's matches — but `advance_sweep` returns the moment
`sweep_test` answers, so a sweep visits **at most one matching row**. On
every other row `find_range_from` fails on its first probe, exactly as
`find_range` did. The scan therefore runs once per sweep, over one row,
and a row in the reference corpus is a rendered prototext line. The same
holds for S9's second `find_range`, which fires only on a bounded row,
and the origin is the only row a sweep ever bounds.

What did change per sweep is the wrap budget: `remaining` is `n + 1`
rather than `n`, so a full miss visits one candidate more than before.
Against spec 0235's 5 281 124 lines that is one part in five million.

Test-suite shape, as a check on the above: 719 passing tests against
699 before, with `a_sweep_step_visits_at_most_one_slice` still reporting
`n.div_ceil(SEARCH_SWEEP_SLICE)` slices — the candidate count, and so
the walk's shape, is what it was.
