<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0340 — the help overlay is a pane

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0339-one-search-three-panes.md (the search engine and
        its display half, which this adds a fourth scope to rather than
        a fourth dialect of), docs/specs/0244-a-pan-may-run-past-
        either-end-of-the-content.md (`PaneScroll`, reused unchanged),
        docs/specs/0246-the-search-prompt-browses-history-and-rotates-
        matches.md (N4 — a list pane's stop is its whole row),
        docs/specs/0126-protolens-focus-independent-keys.md (`F1` opens
        the overlay from any focus; this keeps that and adds the
        overlay's own vocabulary), docs/specs/0236-an-override-is-
        edited-as-one-command.md (S16/S20 — `f`/`b` page in every pane,
        `q` is unbound in every pane, the overlay included)

## Background

The `F1` help overlay is the one surface in protolens that a reader
cannot search, and the one that has no cursor. It holds ~320 lines of
`HELP_TEXT` — the longest single body of text the app draws — and the
only way through it is `j`/`k` moving a raw `help_scroll`, with nothing
selected and no way to ask where a key is documented. The reader who
opens `F1` to find out what `Ctrl-O` does has to eyeball 320 lines.

Every other pane already answers this. The main pane has a caret and
`/`; both side panes have a highlight, a `PaneScroll`, and — since spec
0339 — a search that tints as it is typed. The help overlay has a
`usize`.

Two things stand in the way, both small and both incidental:

1. `handle_key` dispatches `help_open` **above** `command_buffer.
   is_some()` (`key_dispatch.rs:536-543`), so a prompt opened over the
   help could never receive a key. The overlay swallows everything it
   does not itself bind.
2. `help_scroll` is a bare `usize` rather than a `PaneScroll`, so it
   cannot be a search origin and cannot be restored by `Esc`.

Nothing else is missing. The candidate model the two side panes use —
a flat list addressed by `SweepCursor::Index` — is exactly what a
`&[&str]` is.

## Goals

- **G1.** The help overlay has a cursor: one highlighted row that
  `j`/`k` and the rest of the navigation vocabulary move, with the
  viewport following it, instead of a viewport that moves with nothing
  selected.
- **G2.** `/`, `?`, `n`, `N`, `F` and `B` work in the help overlay and
  mean there what they mean everywhere else, tint included.
- **G3.** This is reached by **factoring, not by addition**. The three
  list panes differ only in which fields they name; after this change
  the sweep addresses all three through one arm each rather than one
  arm per pane. The measure is the number of places in `search.rs` that
  have to name a particular pane: adding the help scope must *reduce*
  it, not raise it. (This was first written as "must make `search.rs`
  smaller", which is the wrong measure — see Measured outcome.)

## Non-goals

- **N1.** No column caret in the help overlay — no `h`/`l`, `w`/`b`,
  `^`/`$`, no selection and no `Ctrl-C` copy of a help line. The main
  pane's caret is built on tree nodes and on a fold/heat/wire column
  model that a `&[&str]` has no referent for; a second text-editing
  surface is a new implementation, which G3 rules out. The cursor here
  is a *row*, as it is in the two side panes, and spec 0246 N4 already
  says a list pane's stop is its whole row.
- **N2.** The help overlay is still a modal, not a fourth pane in the
  layout. It is not reachable by `Tab`, it does not split the frame,
  and `F1`/`Esc` remain its only exits.
- **N3.** `HELP_TEXT` stays a `&[&str]` const. No structure (sections,
  key/description pairs) is introduced for the search to exploit — a
  match is a match on a line, and a section heading is a line like any
  other.
- **N4.** No cross-row (`Multi`) pattern in the help overlay, for spec
  0339 S6's reason: it is a list, and the `\n` between two of its rows
  is an artifact of the drawing.

## Specification

### The cursor

- **S1.** `App` gains `help_highlight: usize`, and `help_scroll`
  changes type from `usize` to `PaneScroll`. Both are reset by `F1`,
  as `help_scroll` already is.
- **S2.** `handle_help_key` binds, with the same meanings these keys
  have in the manage pane:
  `j`/`Down`/`Ctrl-N` and `k`/`Up`/`Ctrl-P` move the cursor one row;
  `f`/`PageDown` and `b`/`PageUp` move it one page; `Home` and
  `End`/`G` go to the ends. `Ctrl-Up`/`Ctrl-Down` pan the view without
  moving the cursor, and `Ctrl-Left`/`Ctrl-Right` pan it horizontally
  — the help's longest lines exceed a 70%-wide modal.
- **S3.** The cursor row is drawn in the same style the side panes give
  their highlight, and `clamp_scroll_to_visible` keeps it in view. The
  mouse wheel still pans without moving the cursor, as it does in every
  other pane.

### The search

- **S4.** `SearchScope` gains `Help`. `search_scope()` returns it when
  `help_open`, ahead of the other three: while the overlay is up it is
  the only thing the reader can be looking at.
- **S5.** `handle_key`'s `help_open` branch moves **below** the
  `command_buffer.is_some()` branch, so a prompt opened over the help
  is fed by the command line rather than swallowed by the overlay. The
  help modal is 70% of the frame's height and centered, so the command
  row is already visible underneath it and nothing in the layout moves.
- **S6.** The help's candidate text is the `HELP_TEXT` line itself —
  which is also what is drawn, so unlike either side pane the haystack
  and the drawn text coincide. `tint_matches` is still given the drawn
  text, because spec 0339 S3's rule is that it always is.
- **S7.** `render_help` tints, gated on
  `active_search_scope() == SearchScope::Help` exactly as the two side
  panes are (spec 0339 S5).

### The factoring that pays for it

- **S8.** `SearchScope::{Override, Manage, Help}` are recognized as
  *list panes* and addressed through one set of accessors rather than
  one match arm each. The five facts a list pane has are its highlight,
  its `PaneScroll`, its pan offset, its candidate count and its
  per-index text; every scope-dispatched site in `search.rs` needs some
  subset of exactly those. After this change each such site has a
  `Main` arm and a list arm, not four arms.
- **S9.** The three `last_*_search` fields become one array indexed by
  scope. `last_search_for` / `set_last_search_for` /
  `search_highlight_pattern` stop being three-way matches and become
  indexing. This is what makes a fourth pane cost no field.
- **S10.** Landing on a list pane's row is per-pane (the override pane
  previews, the manage pane clears a pending kind, the help pane only
  scrolls), so `apply_sweep_hit` keeps a small per-scope settle call.
  That is the one thing the three genuinely do not share.

## Alternatives considered

**Add a fourth arm to each of the fourteen `match scope` sites.** The
obvious implementation, and what G3 exists to forbid. It would grow
`search.rs` by ~14 arms that each restate what the `Override` arm next
to them already says, and would leave the fifth pane costing the same
again.

**Give the help overlay a full caret, as the main pane has.** Rejected
as N1: it is a second text-editing surface, and the main pane's is
built on the tree, the fold column and the heat field, none of which
exist here.

**Give the help its own small search rather than a `SearchScope`.**
This is what the side panes had before spec 0339 and what 0339 was
written to undo. A fourth dialect — with its own history, its own
tally, its own idea of what `n` repeats — is the thing being removed,
not a cheaper way to add a feature.

**Leave `help_scroll` a `usize` and land searches by scrolling.** Then
the current match has no marker but its tint, `Esc` has nothing to
restore, and `n` resumes from a scroll the reader also moves with
`j`/`k` — so a repeat silently skips matches.

## Test plan

1. `the_help_overlay_has_a_cursor` — `j`/`k` move `help_highlight` and
   not `help_scroll`; the viewport follows only when the cursor would
   leave it.
2. `help_navigation_matches_the_manage_panes` — `f`/`b`, `Home`,
   `End`/`G` and `Ctrl-N`/`Ctrl-P` land where the same keys land in the
   manage pane, on a list of the same length.
3. `a_prompt_over_the_help_is_not_swallowed` — with the overlay open,
   `/` opens the command line and the following characters reach the
   buffer rather than `handle_help_key`. Pins S5's tier order.
4. `a_slash_tints_the_help_overlay` — the matched characters of a help
   line carry `search_current_style`, another line holding the same
   substring carries `search_match_style`.
5. `n_in_the_help_repeats_only_its_own_search` — the help's last
   pattern is independent of the other three panes'. Pins S9's array.
6. `esc_restores_the_help_view` — scroll and pan return to where the
   prompt found them and the cursor never moved.
7. `the_help_cursor_is_the_search_landing` — a committed search moves
   `help_highlight`, and a following `n` resumes from it.
8. `no_match_crosses_two_help_lines` — pins N4.

## Measured outcome

Implemented 2026-08-20. All four gates clean, the test suite run twice
(with and without `COLORTERM`).

**G3's measure was wrong as first written, and is corrected above.**
`search.rs` did *not* shrink: it went from 2341 to 2445 lines, +104.
The accessor layer S8 asks for is six exhaustive `match scope` bodies,
and six four-armed matches are more text than the twelve scattered
three-armed ones they replace. Line count was never the thing worth
holding steady.

What did fall is the number of places that name a particular pane at
all — the count that actually predicts what a fifth pane costs:

| | before | after |
|---|---|---|
| mentions of `SearchScope::Override` in `search.rs` | 12 | 7 |
| `last_*_search` fields on `App` | 3 | 1 array |
| scopes | 3 | 4 |

Seven, not twelve, with a fourth pane added: six accessors
(`list_count`, `list_highlight`, `list_text`, `list_view`,
`list_view_mut`, `set_list_highlight`) and nothing else. Every other
scope-dispatched site now reads `if scope.is_list()`.

Two things the plan did not anticipate:

1. **`set_list_view` and `list_scroll_mut` are one accessor.** Written
   separately they were two four-armed matches over the same three
   fields, differing only in which of them the caller wanted.
   `list_view_mut` returns the scroll, the pan *and* the drawn height,
   and serves both `Esc`-restore and `show_sweep_hit`'s centering.
2. **The auto-pan needs a `last_help_highlight`.** `render_help` calls
   `clamp_scroll_to_visible` only when the cursor has actually moved,
   exactly as `render_override_pane` does — without the guard, a
   `Ctrl-Up` pan is undone by the very next frame.

S5's tier inversion was the whole of the blocker: with the overlay
dispatched below the command line, `/` needed no layout change at all,
since the modal is 70% of the frame and the command row is already
drawn underneath it.

The test-plan's item 4 fixture had to be loosened. `"caret one
character"` occurs on four `HELP_TEXT` lines, not two — the `h`/`l`
pair near the top and a second pair 138 rows down. The test asserts the
shape it needs (two hits inside the drawn window, none after) rather
than a hit count, so an edit to the help moves the expectation with it.
