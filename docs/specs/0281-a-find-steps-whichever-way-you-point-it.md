<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0281 — a find steps whichever way you point it

Status: implemented
Implemented in: 2026-08-13
App: protolens
Refs: docs/specs/0276-a-find-steps-through-its-matches.md (the `F`/`B`
        prompt, its `Enter` and its `Esc` — this spec amends S1, S3 and
        S4), docs/specs/0246-a-search-stops-at-every-match.md (the
        rotation and its absolute `Ctrl-←`/`Ctrl-→` pair)

## Background

Spec 0276's find prompt steps one way only. `F` opens a forward find and
every `Enter` in it goes forward; reversing means `Esc`, then `B`, then
living with a fresh prompt and a fresh origin. The one existing way to
step backward from inside a forward find is `Ctrl-←` — spec 0246's
rotation, bound at every search prompt — and it is a dead end: it moves
the match but the prompt still says `>` and the next `Enter` still goes
forward, so the reader has to keep holding `Ctrl` for as long as they
want to keep going that way.

The gesture is a browser find bar with a direction, and a find bar's
buttons are *onward* and *back*, not *forward in the document* and
*backward in the document*. What is missing is a pair of keys that mean
those two things and that leave the prompt pointing where they went.

## Goals

- **G1.** A find prompt has a **default** direction, fixed by the key
  that opened it (`F` forward, `B` backward), and an **active**
  direction, which is where the next `Enter` will step. The active
  direction starts as the default.
- **G2.** `Shift-→` steps one match in the default direction and
  `Shift-←` one match in the opposite of it — *onward* and *back*,
  relative to the gesture rather than to the document.
- **G3.** Every stepping key sets the active direction to the direction
  it just stepped, so the prompt never says one thing and does another.
- **G4.** `Enter` steps in the active direction.

## Non-goals

- **N1.** `/` and `?` gain nothing. A commit prompt has no "next step"
  to point at — its `Enter` commits — so it has no active direction, and
  `Ctrl-←`/`Ctrl-→` there keep spec 0246 S17's meaning exactly: they
  rotate the preview in an absolute document direction and leave the
  prompt's own direction, and therefore its prefix and what it will
  commit, alone.
- **N2.** `Ctrl-←`/`Ctrl-→` are not dropped or re-pointed. They stay the
  absolute pair — `Ctrl-→` is forward in the document whatever key
  opened the prompt — because that is what they mean at the other two
  prompts, and a key that means one thing at `/` and its mirror at `>`
  would be worse than either. At a find they gain only S4's assignment,
  which is what stops them being the dead end described above.
- **N3.** No new state on `App`. The active direction is a property of
  the prompt and dies with it; a field on `App` would have to be cleared
  on every path that closes one, which is the leak `CommandLineKind`
  exists to avoid (spec 0276 S1).

## Specification

- **S1.** `CommandLineKind::Search`'s `find: bool` becomes
  `find: Option<SearchDir>`: `None` for a `/`/`?` prompt, `Some(d)` for
  a find opened by the key naming direction `d`. The existing `dir`
  field becomes the **active** direction for a find, and is unchanged
  for a commit prompt.

  Widening the existing field rather than adding a third: `find` already
  answers "is this a find", every site that asks reads it as a boolean
  today, and `Option` keeps that reading (`Some(_)`) while making the
  compiler name each of them once. A separate `default: SearchDir` field
  would be meaningless at a `/` prompt and there would be nothing to
  stop it being read there.

- **S2.** `dir` is what everything downstream already reads, and all of
  it is correct for "active" without change:

  - the prefix character (spec 0276 S3) — so the prompt shows `>` while
    the next step is forward and `<` while it is backward, whichever key
    opened it;
  - `restart_search_sweep` — so editing the pattern re-searches from the
    origin in the direction the reader is currently going;
  - `accept_find`'s echo and `last_search` (spec 0276 S6) — so `n`
    repeats the direction of the last step, which is vim's `n`.

- **S3.** `Shift-→` at a find prompt sets the active direction to the
  default and rotates in it; `Shift-←` sets it to the default's reverse
  and rotates in that. At a `/`, `?` or `:` prompt both fall through to
  the plain `←`/`→` text-cursor arms, which is what they do today — the
  command line has no selection for `Shift` to extend, so the
  fall-through is a no-op the reader cannot notice.

- **S4.** `Ctrl-→`/`Ctrl-←` at a find prompt set the active direction to
  `Forward`/`Backward` before rotating. At a `/`/`?` prompt they are
  untouched (N1).

- **S5.** `Enter` at a find prompt is unchanged in code — it already
  reads `dir` — and changed in meaning by S1: it steps in the active
  direction.

- **S6.** Both new arms are guarded on `find: Some(_)` in the arm's own
  pattern, not inside its body, so that a `Shift-→` at a commit prompt
  reaches the text-cursor arm below rather than being swallowed. They
  sit *after* the `Ctrl` arms, so a `Ctrl-Shift-→` keeps meaning
  `Ctrl-→`.

## Alternatives considered

**Make `Shift-→` absolute (forward) and `Shift-←` backward.** Then the
Shift pair is a strict superset of the Ctrl pair — same directions, plus
S4's assignment — and one of the two should be deleted. Rejected because
the value of the gesture is precisely that it is *relative*: after `B`
the reader is walking backward through the document and wants a key that
means "keep going", and no absolute pair can spell that. Keeping both
readings, one per modifier, costs two match arms.

**Let `Ctrl-←`/`Ctrl-→` be relative at a find prompt.** Same two keys,
no new bindings. Rejected because it makes `Ctrl-→` mean "forward" at
`/` and "backward" at `<`, and both prompts are reachable from the same
keyboard in the same second.

**Store the default on `App` and let `dir` be the active direction
alone.** Rejected under N3: `command_kind` is set by `open_command_line`
on every prompt and so cannot carry stale state, while an `App` field
would need clearing at each of the several places a prompt ends.

## Test plan

1. `shift_arrows_step_relative_to_the_find_that_opened_the_prompt` — at
   a `B` prompt, `Shift-→` moves backward through the document and
   `Shift-←` forward; at an `F` prompt, the mirror.
2. `a_step_points_the_prompt_at_where_it_went` — after `Shift-←` in a
   `B` prompt the prefix is `>` and `Enter` steps forward; after
   `Shift-→` it is `<` again and `Enter` steps backward.
3. `ctrl_arrows_stay_absolute_and_set_the_active_direction` — at a find
   prompt `Ctrl-←` steps backward and the following `Enter` does too.
4. `a_commit_prompt_has_no_active_direction` — at `/`, `Ctrl-←` rotates
   backward but the prefix stays `/` and `Enter` still commits a forward
   search; `Shift-→` moves the text caret.
5. `an_accepted_find_repeats_in_the_direction_it_last_stepped` — `F`,
   `Shift-←`, `Esc`, then `n` continues backward.

## Measured outcome

Implemented 2026-08-13 as specified; 969 tests pass (five new).

Two things the implementation added to the spec's text:

- S4's assignment and the rotation were factored into one
  `App::step_search_match(dir)` — "rotate, and aim a find prompt where
  you went" — so `Shift-→`, `Shift-←`, `Ctrl-→` and `Ctrl-←` all reach
  the rotation through the same place, and `rotate_search_match` keeps
  spec 0246's meaning untouched for every other caller. `step_find_match
  (back: bool)` is the relative pair on top of it.
- The help had the find prompt's `Esc` landing on the match's *last*
  character — spec 0278 S5 moved it to the first on 2026-08-11 and the
  help was not updated. Corrected in the same edit that documents the
  four arrows, since it is the same four lines.
