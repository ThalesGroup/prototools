<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0277 — a search says which match you are on

Status: implemented
Implemented in: 2026-08-11
App: protolens
Refs: docs/specs/0235-the-prompt-answers-while-you-are-still-typing.md
        (the resumable sweep, its slice budget, and the idle arm it is
        walked from), docs/specs/0246-a-search-stops-at-every-match.md
        (what counts as a stop, the full cycle `n` walks, and the caret
        landing on the match's first character), docs/specs/0274-a-match
        -may-cross-a-line.md (the segment walk, and the occurrence tint
        that is window-bounded), docs/specs/0276-a-find-steps-through-
        its-matches.md (the find prompt, whose `Enter` is the rotation
        this number tracks)

## Background

A search answers *where is the next one* and nothing else. The reader
cannot tell one match from forty, cannot tell whether `n` is worth
pressing, and cannot tell how far through the document the current one
sits. On a blob whose document is millions of rows, "is this the only
`user_id` in here, or the first of two hundred?" is the question the
search is actually being asked, and no part of the screen answers it.

Every find bar the reader already knows answers it with `27 of 42`.

The pieces are close but none of them is it:

- the sweep enumerates candidates and finds matches, but stops at the
  first — that is its contract and what makes an incremental prompt
  answer while the pattern is still being typed (spec 0235);
- spec 0246 S18's rotation already defines a *full cycle*, so "how many
  matches" is a well-posed question with an exact answer;
- spec 0274 S13 already counts occurrences and tints each one, but only
  within the window, which is the one thing a total must not be.

## Goals

- **G1.** While a search is live, the command row shows `27 of 42` for
  the focused pane's pattern: the displayed match's place counting from
  the start of the document, and the number of matches in the whole of
  it.
- **G2.** Nothing waits for it. The indication is drawn when the answer
  exists and is absent otherwise; no keystroke, and no sweep, is ever
  delayed by the counting.
- **G3.** The pair means what the reader will assume it means: from
  `27 of 42`, pressing `n` fifteen more times reaches the last match and
  one more wraps back to `1 of 42`.

## Non-goals

- **N1.** No count for a cross-row pattern (spec 0274). Its unit of work
  is a segment scanned on a worker thread, not a row visited on this
  one, and a tally that queued segments would compete for that worker
  with the sweep the reader is waiting on. An absent count, not a wrong
  one — G2 already licenses the absence.

- **N2.** No count while the bake still owes subtrees. A total over a
  document that is still growing is not a total, and spec 0249 S13
  already draws exactly this line for the "not found" message: a claim
  about the whole document is not one the search is entitled to make
  yet.

- **N3.** No `0 of 0`. A miss is already reported, on this same row, by
  the pattern's own color (spec 0237 S11, spec 0272). One home per fact.

- **N4.** The number is not a jump target. There is no "go to match 27";
  `n`, `N` and the find prompt are how the reader moves.

- **N5.** No cap, and no `999+`. See S2 — with nothing stored per match
  there is nothing to bound but the walk, and the walk is affordable.

## Specification

- **S1.** A tally holds **two scalars, not a list of positions**:

  ```rust
  struct SearchTally {
      scope: SearchScope,
      pattern: String,
      version: u64,                   // App::structural_version
      at: Option<(SweepCursor, RowBound)>,  // None once the walk is done
      count: usize,
      total: Option<usize>,           // Some once the walk has closed
      ordinal: Option<usize>,
      of: Option<(SweepCursor, usize)>,     // the hit `ordinal` describes
  }
  ```

  `total` and `ordinal` are two facts with two different lifetimes, and
  keeping them apart is what makes the rest of this spec small: the
  total survives every movement of the reader, and only the ordinal has
  to keep up with it.

  Nothing is stored per match. The alternative — remember every match's
  position, so that the ordinal is a lookup — is rejected in
  *Alternatives*: it buys an ordinal that is never stale, at the price
  of a cap, and the cap lands on exactly the documents this feature
  exists for.

- **S2.** The walk is uncapped and exact. `SEARCH_SWEEP_SLICE`'s doc
  comment records a full sweep of googleapis.desc's 5 281 124 lines at
  **647–961 ms**, converged one slice at a time; a tally is that same
  candidate walk with the early exit removed and `find_iter` in place of
  `find`. Roughly a second, on the largest corpus in the project, for a
  job nothing is waiting on (S9) — there is no case in which a cap would
  pay for its own second notation.

- **S3.** The tally walks forward from the document's first candidate,
  whatever the search's direction. "Counting from the beginning of the
  document" is the whole meaning of the first number, so a backward
  search's `n` *decrements* it.

- **S4.** What counts is exactly the set of stops in one full cycle of
  `n`: every match in every candidate (spec 0246 S1's `Whole` bound
  everywhere, since a tally has no origin to split), one stop for a row
  the pattern matched on its path (spec 0273 S5 gives a row a single
  haystack), and nothing for a footer row.

  This is the invariant G3 rests on, and it is why the tally reuses
  `next_candidate` and `sweep_test`'s haystack choice rather than
  writing a second matching rule beside the first. It also settles the
  fold question by construction: a folded subtree contributes no
  candidates to either walk, so the tally counts what `n` can reach and
  nothing else.

- **S5.** The ordinal comes out of the same walk. The tally is given the
  displayed hit when it starts; when the walk reaches that hit it takes
  the running count as the ordinal. One walk answers both questions, and
  the first `27 of 42` therefore costs no more than the `42` alone.

  Both facts are published together, when the walk closes. An ordinal
  without a total cannot wrap (S6) and has nothing to be drawn beside
  (S8), so there is no state in which one of the two is useful alone.

- **S6.** Afterwards the ordinal is **stepped on the way out**, not
  reconstructed on the way in. A search that departs the match the
  ordinal describes carries that ordinal with it —
  `SearchSweep::from_ordinal` — and when the sweep lands, the tally
  takes `from_ordinal ± 1`, wrapping at `total`, and re-points `of` at
  the new hit. A sweep carrying no ordinal leaves the tally `Unknown`.

  Deciding at departure rather than at arrival is what makes this one
  rule instead of several. At the moment of departure the displayed hit
  is still the displayed one and the caret has not moved yet, so "am I
  leaving match *k*" is a question about the state the reader is looking
  at, rather than a guess reconstructed from where they ended up.

  Two sites begin such a sweep:

  - `rotate_search_match` — spec 0246 S18 builds it from the displayed
    hit by construction, so it carries the ordinal whenever there is
    one. This is the find prompt's `Enter` (spec 0276 S4) and
    `Ctrl-←`/`Ctrl-→`.
  - `run_search` — `n`/`N` sweep from the caret, so it carries the
    ordinal when the origin lies **within the displayed hit's extent**.
    Not `origin.column == hit.start`: spec 0246 S3 leaves the caret on
    the match's first character but spec 0276 S5 deliberately leaves it
    on the *last*, and an equality test would make an `n` straight
    after an `Esc`-accepted find look like a jump. The extent covers
    both landings, and every caret nudge inside the match besides.

    **Amended 2026-08-11 (spec 0278 S5).** There is one landing again —
    an accepted find lands on the match's first character like every
    other search — so the two-landings clause is void and the extent
    test is no longer load-bearing. It is kept as written because it
    costs nothing and still covers a caret nudged inside the match.

  `accept_find` (spec 0276 S5) applies the hit already displayed
  without re-searching, so it moves no match and steps nothing.

- **S7.** A sweep that carried no ordinal — the reader moved off the
  match, then pressed `n` — sets the ordinal `Unknown` and starts a
  **prefix** walk to recover it: the total is still valid, so the only
  missing fact is how many matches lie before the displayed hit, and the
  walk stops there rather than running to the end. Half a document on
  average, and S6 is what keeps it rare: stepping matches, however far,
  never provokes it.

- **S8.** The indication is `27 of 42`, and `? of 42` while S7's walk is
  running. The placeholder rather than dropping the whole field: the
  field keeps its width, so a reader stepping matches on a large
  document does not watch it appear and vanish, and the `?` says which
  of the two facts is the missing one. Nothing at all is drawn until
  `total` is known.

  **Amended 2026-08-11 (spec 0278 S2).** The field is drawn only where
  the row is also showing the pattern it counts — an open prompt's
  buffer or spec 0278's echo. The count is never alone on the row.

- **S9.** The tally is its own step in `run_loop`'s idle arm, and it
  runs **last** — after the sweep, the discard, the bake and the
  read-ahead. It is the only one of the five jobs there that nothing on
  screen is waiting for. It yields on `SEARCH_SWEEP_SLICE` candidates
  like the sweep, for the same reason and with the same effect on key
  latency.

- **S10.** Finishing a walk breaks the receive loop with no event, the
  way spec 0272 buys the sweep's answer its frame.
  `may_sleep_indefinitely` has no tally term and must not grow one — a
  tally is finite and a finished one owes exactly one repaint, not a
  timer. Without the break the number would first appear on the
  reader's next keystroke.

- **S11.** A tally is keyed on `(scope, pattern, structural_version)`.
  Any of the three changing drops both facts and starts a new walk. A
  change in the *displayed hit* touches only the ordinal, per S6.

- **S12.** A tally exists only while the matches are tinted — a search
  prompt is open, or `search_highlight` is set. That is the same
  condition spec 0235 S15 draws the highlight under, and
  `clear_search_highlight` drops the tally with it.

  **Amended 2026-08-11 (spec 0278 S2).** The tally still *lives* under
  this condition; it is *drawn* under the narrower one above. The tint
  outlives the row, so a `27 of 42` hidden by a movement key comes back
  the moment `n` reprints the pattern.

## Alternatives considered

**Remember every match's position.** The ordinal then needs no stepping
rule and no re-derivation at all: it is a scan of the list for the
displayed hit, correct after any movement whatever. Rejected because it
forces a cap — a one-character pattern over a five-million-row document
asks for hundreds of megabytes of positions — and the cap lands exactly
where the feature is most wanted: on googleapis-sized input any common
pattern would report `1000+` and no ordinal, which is strictly less than
what S1's two scalars give for free. The stepping rule it avoids turned
out to be S6, which is not worth a cap.

**Decide the step on arrival — compare the new hit against the old.**
The tempting shape, because it needs no field on the sweep: when a hit
lands, ask whether it is the successor of the one the ordinal describes.
Rejected: that question cannot be answered without counting the matches
between them, which is the very thing being avoided. Approximating it by
the caret's column also breaks — spec 0246 S3 lands the caret on the
match's first character and spec 0276 S5 on its last, so no single
column test covers both. At departure the answer is not inferred at all:
the search is leaving a known match in a known direction.

**Fold the counting into the sweep.** The sweep already visits every
candidate on a miss, so on the face of it the count is free. Rejected:
on a *hit* — the common case — the sweep stops at the first match by
design, and a sweep that counted would have to see the whole document
before it could report where the first one is. That is the incremental
prompt's whole reason for existing, traded away for a decoration.

**A partial total while the walk runs — `27 of 42 so far`.** Rejected:
the reader cannot tell it from a settled one at a glance, and a number
that grows while being read is worse than no number. G2's absence is
the honest state, and S8's `?` is the one place a partial answer is
worth showing, because there the missing half is named.

**Reuse spec 0274 S13's occurrence pass.** It already finds every match
and is already fast. Rejected: it is window-bounded by construction —
it exists to tint what is on screen — and widening it to the document
is not a change to it but a different walk with a different budget.

## Test plan

1. `a_tally_counts_every_match_in_the_document` — S4, including two
   matches on one row and a match the sweep would have skipped as the
   origin's own.
2. `the_ordinal_counts_from_the_start_of_the_document` — G1/S3/S5, with
   the search started from the middle so that sweep order and document
   order differ.
3. `n_steps_the_ordinal_without_walking_again` — S6: the ordinal
   advances and the tally's walk state is untouched.
4. `n_wraps_the_ordinal_at_the_last_match` — G3.
5. `a_backward_search_decrements_the_ordinal` — S3/S6.
6. `n_after_an_accepted_find_still_steps` — S6's second bullet: the
   caret is on the match's *last* character (spec 0276 S5) and the
   ordinal must step rather than re-derive.
7. `moving_the_caret_off_the_match_then_pressing_n_re_derives` — the
   departure carrying no ordinal, and S7's prefix walk recovering it.
8. `no_indication_while_the_total_is_still_walking` — G2/S8.
9. `a_keystroke_restarts_the_tally` — S11.
10. `a_cross_row_pattern_reports_no_count` — N1.
11. `a_miss_reports_no_count` — N3.
12. `the_tally_counts_in_the_manage_pane` — S4 for a side pane.

## Measured outcome

protolens over `googleapis.desc` (25.6 MB, as both schema and blob —
5 281 124 lines), a 50x200 pty pinned to `taskset -c 4-7`, the bake left
to settle first. `/user` then `Enter`, timing from the key to the first
frame that carries the number:

| | |
|---|---|
| the sweep the `Enter` runs synchronously | 0.50 ms |
| `1 of 7724` first drawn | 835, 845, 847, 850 ms |
| `n`, `n`, `n` → `2`, `3`, `4 of 7724` | not measurable |

S2 predicted "roughly a second, on the largest corpus in the project"
from `SEARCH_SWEEP_SLICE`'s 647–961 ms full sweep, and the tally lands
inside that band: the early exit removed and `find_iter` in place of
`find` cost nothing over the walk itself. The `n` steps are reported as
not measurable because the only way to read the row back is to force a
full repaint, and the 59 ms that takes is entirely the resize
round-trip — S6's arithmetic has no walk to wait for, which is
`n_steps_the_ordinal_without_walking_again`'s subject.

The 0.50 ms sweep beside the 845 ms tally is the whole argument of
*Fold the counting into the sweep*, in the two numbers: on a hit the
sweep stops at the first match, and a sweep that counted would have made
the reader wait 1700x longer to learn where it is.

Two things implementation found, neither of them in the draft.

**The number needed a second reader-side check of S11's key.** The
tally's walk is the idle arm's *last* job (S9), and the loop draws the
frame long before it reaches that arm — so the keystroke that changes
the pattern would have drawn one frame of the *old* pattern's total
beside the new pattern. `search_tally_text` therefore re-checks
`(scope, pattern, structural_version)` rather than trusting it, which is
cheap (a string compare; no regex is compiled) and covers the fold
toggle and the pane change for free.
`a_keystroke_restarts_the_tally` is the test that failed without it.

**The ordinal needed following at each of the three sweep landings.**
Same cause, opposite direction: S6's ±1 is arithmetic the tally does
when the sweep lands, and the idle arm reaches it a frame too late, so
`n` drew the *previous* ordinal for one frame. `track_search_tally` is
that arithmetic lifted out of the step, called from `search_sweep_step`
where `found` changes, and from `commit_search` and `run_search` whose
sweeps are drained on the key.
