<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0278 — a committed search leaves its pattern on the row

Status: implemented
Implemented in: 2026-08-11
App: protolens
Refs: docs/specs/0147-protolens-status-message-command-line-split.md
        (the keypress
        and mouse dismissal this rides on, and the 3-second timeout it
        must not ride on), docs/specs/0235-a-search-answers-while-it-is
        -still-being-typed.md (the prompt, and the highlight that
        outlives it), docs/specs/0276-a-find-steps-through-its-matches
        .md (the `>`/`<` prefixes, and the `Esc` this spec amends),
        docs/specs/0277-a-search-says-which-match-you-are-on.md (the
        count this gives a home to)

## Background

Spec 0277 draws `27 of 42` right-aligned on the command row for as long
as the matches are tinted. The tint outlives the prompt by design (spec
0235 S15), so the count does too — and protolens never echoes a
committed pattern, so the row it lands on is otherwise **empty**. The
reader is left with a bare `27 of 42` floating on a blank row, naming a
pattern that is nowhere on screen.

It is worse in motion. Spec 0147 G5 clears `message` on every keypress
and every mouse event, so a `not found` vanishes the instant the reader
moves — while the count, keyed on the tint, stays. Panning or scrolling
therefore *widens* the gap between the two: everything else on the row
goes and the orphan remains.

The count is not wrong. It has no parent.

## Goals

- **G1.** A committed search leaves its pattern on the command row, the
  way the prompt that was just closed had it: `/beta`, `?beta`.
- **G2.** The pattern and spec 0277's count appear together and
  disappear together. Neither is ever on the row without the other.
- **G3.** They are dismissed by the reader moving on — the next
  keypress or mouse event — and by nothing else.

## Non-goals

- **N1.** No expiry. The echo is not a `message` and must not acquire
  `MESSAGE_TIMEOUT`: a count that vanished on a timer while the reader
  was still reading the match is precisely the "watch it appear and
  vanish" spec 0277 S8 rejected when it chose `? of 42` over blanking.

- **N2.** The echo is not tied to the match being on screen. A scroll is
  a keypress, so G3 already dismisses it in the case that prompted this
  spec; where G3 does *not* fire — a scroll that leaves the match
  visible — keeping the pattern is right. A live visibility test would
  instead flicker the pair in and out as the reader scrolls past the
  match, which is the failure S8 already ruled on.

- **N3.** No echo for a miss. `not_found` is the same fact told at more
  length, spec 0277 N3 already refuses the count beside it, and one home
  per fact.

- **N4.** The echo is not editable and is not a prompt. It carries no
  cursor and swallows no keys; the key that would edit it dismisses it
  (G3). `n`, `N` and `/` are how a search continues.

## Specification

- **S1.** `App::search_echo: Option<(SearchDir, String)>`, set to the
  pattern by the three sites that land a committed search on a hit —
  `commit_search`, `run_search` (`n`/`N`) and `accept_find` — through
  one `echo_search`. An empty pattern echoes nothing; a miss echoes
  nothing (N3).

  A field of its own rather than `self.message` because of N1: a message
  auto-dismisses after `MESSAGE_TIMEOUT` and an echo must not.

- **S2.** `App::search_row_text` is the single answer to *what search
  text is the row showing*: an open search prompt's buffer, or the echo,
  each with its prefix character — `/`, `?` for a search, `>`, `<` for a
  find prompt (spec 0276 S3). A `:` prompt and a non-empty `message`
  both answer `None`.

  Two readers, one predicate: `render_command_row` draws what it
  returns, and spec 0277's count is drawn only where it returns
  something. That identity is the whole of G2 — there is no second rule
  to keep in step.

  A message outranks the echo, being news where the echo is a reminder.

- **S3.** The echo is cleared beside `message` at the two input entry
  points, `handle_key` and `handle_mouse` (spec 0147 G5), and nowhere
  else. A handler that re-echoes sets it again in the same pass, which
  is what makes `n` reprint the pattern rather than blank the row.

- **S4.** The echo spells an accepted find `/`, not `>`. Spec 0276 S6
  already makes an accepted find a committed search — it feeds the
  history and the pane's last pattern — so what it leaves behind is
  spelled the way `n` will repeat it.

- **S5 (amends spec 0276 S5/G3).** `Esc` at a find prompt leaves the
  caret on the match's **first** character, where `apply_sweep_hit` puts
  every other search landing, and `caret_to_match_end` is deleted. The
  last-character landing made the accepted find the one gesture in
  protolens whose caret did not sit where the search said the match was,
  for a benefit — the caret trailing the text, as a text editor's
  insertion point would — that a block caret resting *on* a cell (spec
  0242 S1) does not deliver anyway.

  What survives of S5 is its second half: no selection is left behind
  (spec 0276 N3), which is now the whole body of the `Main`-scope arm.

  This also retires spec 0277 S6's "the extent covers both landings"
  clause. There is one landing again, so the extent test is no longer
  load-bearing — it is kept because it costs nothing and still covers a
  caret nudged inside the match.

## Alternatives considered

**Drop the count when the prompt closes.** The VS Code reading: the
count belongs to the find widget, so closing the widget takes it. Would
have needed no echo. Rejected because it takes the count away at exactly
the moment it becomes useful — `n` after a commit is the gesture the
count exists to narrate, and under this rule `n` would show nothing.
Vim's `shortmess-=S` answers the same question the other way, and this
spec follows vim: the count lives with the echo, and `n` reprints both.

**Keep the count while the match is on screen.** Rejected as N2: it
flickers, and it makes a document-wide fact answer to the viewport.

**Make the echo a `message` and gate the count on "the message looks
like a search".** One less field. Rejected twice over: the 3-second
timeout is wrong for an echo (N1), and recognizing a search by parsing
the message back would make an unrelated message beginning with `/` show
a stale count.

## Test plan

1. `a_committed_search_echoes_its_pattern` — G1/S1.
2. `the_echo_and_the_count_leave_together` — G2/G3/S3: a movement key
   clears both; the count is not drawn without the echo.
3. `n_reprints_the_pattern` — S3's second half.
4. `an_accepted_find_echoes_a_committed_search` — S4: `>beta` while the
   find prompt is open, `/beta` once it is accepted.
5. `a_miss_echoes_nothing` — N3.
6. `the_echo_outlives_the_message_timeout` — N1.
7. `esc_accepts_a_find_at_the_start_of_the_match` — S5 (replaces spec
   0276's `…_at_the_end_of_the_match`).
8. `esc_accepts_a_cross_row_find_on_its_first_row` — S5 (replaces spec
   0276's `…_on_its_last_row`), and spec 0276 N3's surviving half.

## Measured outcome

104 added lines and 64 removed across five source files, of which
`caret_to_match_end` — 40 lines and the whole of the deleted half of
S5 — is the largest single piece. Nothing was added to the frame: the
echo is a `format!` of a pattern the app already holds, and the row is
drawn once either way.

Two things fell out of S2 that the drafting did not anticipate.

**The `find` flag stops at the prompt.** `search_row_text` needs the
`>`/`<` prefixes for an *open* find, and `/`/`?` for the echo of one
that was accepted (S4) — so `search_prefix` takes `find` as a parameter
but `search_echo` does not store it. The direction is the only thing the
echo has to remember, because spec 0276 S6 already made the accepted
find indistinguishable from a commit everywhere else.

**The count is drawn outside `cmd_area`.** Spec 0277 S8 gives the tally
its own slice of the row, and `cmd_area` — the mouse hit-test rectangle
— is deliberately the *remainder*. A render assertion that reads
`cmd_area` therefore sees the pattern and not the count; the test reads
the full terminal row instead. Worth knowing before writing any other
render test against this row.

The three tests that asserted the last-character landing were the only
fallout of S5 in the suite, which is the evidence that the landing was a
local rule and not something other code had come to depend on.
