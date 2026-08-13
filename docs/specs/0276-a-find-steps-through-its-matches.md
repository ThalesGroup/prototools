<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0276 — a find steps through its matches

Status: implemented
Implemented in: 2026-08-11
App: protolens
Refs: docs/specs/0235-the-prompt-answers-while-you-are-still-typing.md (the
        incremental prompt, its origin, and the highlight that outlives
        it), docs/specs/0246-a-search-stops-at-every-match.md (the
        origin split, the rotation, the history),
        docs/specs/0274-a-match-may-cross-a-line.md (a hit may end on
        another row; every occurrence is tinted)

## Background

`/` is a *commit* prompt: it asks a question once, `Enter` answers it and
closes. Stepping to the second match means leaving the prompt and pressing
`n`, and correcting the pattern means opening `/` again and retyping it.
The pieces of the other idiom — the browser's find bar, where the prompt
stays open and `Enter` walks the matches — are all already built and
merely unreachable in one gesture:

- the prompt already tints every occurrence, with a distinct style for the
  current one (spec 0274 S13);
- `rotate_search_match` already steps that current one without disturbing
  the origin (spec 0246 S18), and `show_sweep_hit` already brings it into
  view (spec 0235 S13);
- all three panes already search through the same prompt (`SearchScope`).

What is missing is a prompt that binds `Enter` to the rotation instead of
to the commit, and starts from the last pattern rather than from nothing.

## Goals

- **G1.** `F` and `B` open a *find* prompt — forward and backward — in
  whichever pane has focus, pre-filled with that pane's last pattern.
- **G2.** `Enter` steps the current match forward (`F`) or backward (`B`)
  and leaves the prompt open, bringing the new match into view.
- **G3.** `Esc` accepts: it closes the prompt and puts the caret on the
  end of the match currently shown. *(Amended 2026-08-11 by spec 0278
  S5: on the **start** of the match, as every other search landing
  does.)*
- **G4.** An accepted find is a search like any other — it feeds the
  pattern history and the pane's last pattern, so `n`/`N` continue it.

## Non-goals

- **N1.** There is no abandon. With `Enter` bound to the rotation, `Esc`
  accepting is the whole exit vocabulary of the gesture, and a find that
  has moved the view has moved it. (`Backspace` on an emptied buffer
  still cancels the prompt, as it does for `/` — that is the existing
  text-entry rule and not a second exit worth advertising.)

- **N2.** `F` does not become a second way to spell `/`. The two prompts
  differ in exactly two keys and are told apart by their prefix
  character; nothing else about the search — the origin, the incremental
  re-search per keystroke, the wrap, the path matching — is duplicated
  or varied.

- **N3.** No selection is left behind. Spec 0274 S12 leaves a `/`-committed
  cross-row match selected because its extent is otherwise invisible; an
  accepted find keeps its highlight, so the extent is already on screen
  and a selection would be a cue the reader did not ask for.

## Specification

- **S1.** `CommandLineKind::Search` gains a field: `Search { dir:
  SearchDir, find: bool }`. *(Amended 2026-08-13 by spec 0281 S1: the
  field is `find: Option<SearchDir>`, carrying the find's default
  direction, and `dir` becomes the active one.)* A field rather than a fourth variant,
  because a variant leaves every existing `matches!(…, Search(_))` test
  — the history browse, the incremental restart, the pattern tint —
  quietly answering "no" for the new prompt. Widening the existing
  variant makes the compiler name all sixteen sites and forces a
  decision at each.

  The mode therefore lives in `command_kind`, which `open_command_line`
  sets on every prompt, so it cannot leak from one prompt to the next.

- **S2.** `F`/`B` bind in all three panes, next to `/` and `?`, and open
  through one `App::open_find(dir)`: prefill from the focused pane's own
  last pattern, then start the sweep immediately. The immediate start is
  the difference that matters — `/` opens on an empty buffer with
  nothing to search, while a find opens with an answer already owed.

- **S3.** The prompt's prefix character is `>` for a forward find and `<`
  for a backward one, beside `/`, `?` and `:`. Punctuation rather than
  the letter that was pressed: the buffer is pre-filled, so `F` as a
  prefix would render `Ffoo` and read as a typo.

- **S4.** `Enter` in a find prompt is `rotate_search_match(dir)` and
  nothing else — where spec 0281 S5 makes `dir` the *active* direction,
  so it steps whichever way the reader last pointed the prompt — the buffer, the cursor, the origin and the highlight
  all stay. Spec 0246 S18's rotation restarts from the displayed match
  and cycles, so a single-match pattern rotates to itself rather than
  reporting a miss.

- **S5.** `Esc` in a find prompt accepts the displayed hit: it closes the
  buffer, records the pattern (S6), applies the hit the way a commit
  does — `record_jump`, `unfold_ancestors`, the highlight kept — and
  then moves the caret to the match's **last character**:

  - a single-row hit: `column + width - 1`;
  - a cross-row hit (spec 0274 S11): the row in `hit.end`, at one before
    the recorded exclusive end column;
  - a path hit: no move. The path is not on screen (spec 0235 S20), so
    it has no last character to land on and the row it names is the
    whole of the answer.

  The last character rather than the cell after it, because protolens's
  caret is a block that rests *on* a cell (spec 0242 S1) — and at the
  end of a row the cell after the match does not exist.

  **Amended 2026-08-11 (spec 0278 S5).** The caret lands on the match's
  **first** character instead, which is where `apply_sweep_hit` puts
  every other search landing, and `caret_to_match_end` is deleted. The
  three bullets above and this paragraph are void; the block-caret
  argument in them survives only as the reason the *old* landing was the
  last character rather than the cell after it, and a block caret
  resting on a cell never delivered the trailing-insertion-point benefit
  the rule was reaching for. What survives of S5 is the sentence before
  the colon, plus N3's "no selection is left behind".

- **S6.** An accepted find pushes its pattern to the shared history and
  sets the focused pane's `last_*_search` to `(dir, pattern)`, exactly
  as `Enter` at a `/` prompt does. `n`/`N` afterwards repeat it, and a
  later `F` opens pre-filled with it.

- **S7.** A find that is showing no match accepts nothing: `Esc` restores
  the view the prompt was opened from and leaves the position alone,
  which is `/`'s `Esc` unchanged. Applying "the current match" when
  there is none is the only alternative, and there is nothing to apply.

- **S8.** In the two side panes S5 degenerates to `apply_sweep_hit`'s own
  behavior — the highlight moves to the matched entry. There is no caret
  there to place at an end, so S5's second half is main-pane only.

- **S9.** A find in a side pane *previews* its current match: the
  highlight moves as the sweep finds and as `Enter` rotates, rather than
  only at the accept. A side pane tints nothing (spec 0274 S13's
  occurrence pass is the main pane's row renderer), so its current match
  is shown by the highlight or not at all, and G2 would otherwise be
  stepping something invisible.

  Only a find previews. A `/` prompt in a side pane can still be
  abandoned, and this move is not undone — which is N1's "a find that
  has moved the view has moved it" read for a pane whose position *is*
  its view. It also makes S7 weaker there than in the main pane: an
  `Esc` on a miss restores the scroll but leaves the highlight where the
  last matching prefix put it. That is the same trade the reader made by
  choosing a prompt without an abandon.

## Alternatives considered

**Bind the rotation to `n`/`N` inside the prompt instead of `Enter`.**
Rejected: `n` is a character, and a prompt whose text field swallows
letters cannot also spend them on navigation without a mode inside the
mode. `Enter` is free precisely because the find has nothing to commit.

**Make `F` a modifier on the existing `/` — open `/` and let `Enter`
rotate whenever the buffer was pre-filled.** Rejected: it makes
`Enter`'s meaning depend on how the buffer got its contents, so the same
key at the same-looking prompt would sometimes close it and sometimes
not. The two prompts are worth telling apart on screen (S3).

**Reuse `Ctrl-Left`/`Ctrl-Right` and skip the new prompt entirely.**
They already rotate. Rejected as the whole feature rather than as a
mechanism: they do not pre-fill, they do not accept, and the reader
asked for a gesture that ends with the caret on the match.

## Test plan

1. `f_opens_a_find_prompt_prefilled_with_the_last_pattern` — G1: after a
   `/` search, `F` opens with that pattern and a sweep already running.
2. `enter_in_a_find_prompt_steps_to_the_next_match` — G2/S4: the prompt
   stays open and the displayed hit advances; `B` steps the other way.
3. `esc_accepts_a_find_at_the_end_of_the_match` — G3/S5: the caret lands
   on the match's last character, not its first. *(Amended 2026-08-11:
   renamed `esc_accepts_a_find_at_the_start_of_the_match`, and it now
   asserts the other landing — spec 0278 test-plan item 7.)*
4. `esc_accepts_a_cross_row_find_on_its_last_row` — S5's second bullet,
   and N3: no selection is left engaged. *(Amended 2026-08-11: renamed
   `esc_accepts_a_cross_row_find_on_its_first_row`; only the N3 half of
   what it covers survives — spec 0278 test-plan item 8.)*
5. `an_accepted_find_is_repeatable_with_n` — G4/S6.
6. `esc_on_a_find_with_no_match_restores_the_view` — S7.
7. `f_finds_in_the_manage_pane` — S8/S9/G1 for a side pane: the
   highlight previews and steps while the prompt is open, and `Esc`
   leaves it on the entry shown.

## Measured outcome

The gesture cost roughly 150 lines, almost all of them the two rebound
keys and the caret placement. Nothing in the sweep, the origin split,
the rotation or the tint changed — the Background's claim that the
pieces were already built held, and `open_find` is six lines.

S1's bet paid twice. Widening `Search` into a struct variant made the
compiler name sixteen sites; fifteen wanted `Search { .. }` and one —
`restart_search_sweep` — wanted `dir` and would have been silently
wrong under a fourth variant, because a find that is not a `Search(_)`
does not re-search as it is typed.

S9 was not in the draft and is the one thing implementation found. A
side pane tints nothing, so `Enter` there stepped a match the reader
could not see: `f_finds_in_the_manage_pane` first failed asserting a
highlight that had not moved, and the fix was to let a find — and only
a find, which has no abandon — preview through `apply_sweep_hit`.
