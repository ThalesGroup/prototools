<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0235 — a search answers while it is still being typed

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0195-search-is-case-aware-and-the-backward-walk-is-linear.md
        (S2, `SearchPattern` and the smartcase rule this makes cheap;
        S4, `n`/`N`),
      docs/specs/0222-the-text-lives-in-the-nodes.md
        (S6, the line-level walk `jump_to_match` uses, and the measured
        cost of a full-document miss),
      docs/specs/0223-a-frame-under-input-skips-the-parse.md
        (S3, `input_pending` — the queue this reads to yield),
      docs/specs/0233-the-caret-is-drawn-the-same-wherever-it-rests.md
        (S3, a cue over the document is a background patch, never an
        inversion),
      docs/specs/0194-the-cursor-is-a-caret.md
        (S8, a match lands on its first character; S10, `record_jump`
        and the jumplist a preview must not touch),
      docs/specs/0199-the-arrow-keys-fold-before-they-leave-the-node.md
        (S1, `CaretAnchor` — a path match lands at `Home`),
      docs/specs/0208-attention-follows-the-cursor.md
        (S1, `Ctrl-a`/`Ctrl-e` in the main pane, which the command line
        should not contradict),
      docs/specs/0113-protolens-tui.md (D25, `positional_path`)

## Background

protolens's `/` is a *modal* search: the pattern is typed blind into the
command bar, and nothing happens until `Enter`. Four consequences, all
observable today.

### 1. Nothing is shown until the pattern is finished

There is no feedback while typing, so a pattern that will not match is
only discovered after it is complete, and the only repair is to retype
it. vim has had `'incsearch'` since 5.0 and every editor since has
copied it; this is the single largest gap between protolens's search and
what a user arrives expecting.

### 2. Matches are invisible once the search lands

`jump_to_match` moves the caret to the match's first character (spec
0194 S8) and stops there. On a row like

```
  labels { key: "region" value: "europe-west1" }
```

a search for `region` puts one inverted cell on the `r` and marks
nothing else — not the extent of the hit, and not the three other hits
on screen. vim draws both, as `CurSearch` and `Search`.

### 3. The command line eats control keys

`handle_command_key`'s `KeyCode::Char(c)` arm (`command_line.rs:221`)
carries no modifier guard, and the only `CONTROL` arms above it are
`Ctrl-f` and `Ctrl-b`. So `Ctrl-a` reaches that arm as
`KeyCode::Char('a')` and **inserts a literal `a` into the pattern**;
`Ctrl-e` inserts an `e`, `Ctrl-u` a `u`, and so on. The motions that do
exist (`Left`/`Right`/`Home`/`End`/`Alt-f`/`Alt-b`/`Ctrl-f`/`Ctrl-b`/
`Backspace`/`Delete`) are correct; it is the unbound ones that are
actively wrong.

### 4. A full-document search cannot run inside a keystroke

Measured on googleapis.desc (5 278 324 lines, 238 MB of rendered text),
a `/` that matches nothing:

| pattern | cost |
|---|---|
| lowercase (smartcase folds) | 1.63–2.05 s |
| contains an uppercase char (`memchr`) | 269–499 ms |

The dominant cost is `starts_with_folded` (`tui/mod.rs:264`), which runs
`char::to_lowercase` at **every byte position** of the haystack. An
all-lowercase pattern is the case a user actually types, so this is the
figure that matters.

Every intermediate prefix of a pattern being typed is likelier to miss
than the finished pattern is. So a search that runs *inside* the
keystroke that triggers it — which is what the current synchronous
`jump_to_match` is — cannot be made incremental at any pattern length.
The search has to stop being a function call and become a task.

## Goals

- **G1.** Typing into a `/` prompt shows the answer as it is typed,
  without the keyboard ever waiting for it: a keystroke abandons the
  search in flight and starts a new one, and returns immediately.
- **G2.** Each change to the pattern searches from the position the
  prompt was *opened* at, so `Backspace` exactly undoes the keystroke
  before it.
- **G3.** A preview moves the *view*, never the cursor. `Enter` moves
  the cursor to the match; `Esc` restores the view and leaves the cursor
  where it always was.
- **G4.** A match brought into view is centered — vertically and
  horizontally — unless it was already visible, in which case nothing
  moves.
- **G5.** The current match is drawn strongly over its whole extent, and
  every other match on screen weakly — during typing, after `Enter`, and
  after `n`/`N`.
- **G6.** The command line is a complete single-line editor: no key
  silently inserts a letter instead of moving.
- **G7.** All three panes (main, override, manage) get G1–G5. The prompt
  is already shared; the behavior should be too.
- **G8.** In the main pane, a node can be reached by its positional path
  through the same prompt.

## Non-goals

- **N1.** Regular expressions, still (spec 0195 N1). Plain substrings.
- **N2.** Cross-line matching. Already true — `jump_to_match` tests one
  `line_text` at a time — and restated here so a later change to the
  walk does not lose it by accident. A pattern containing `\n` matches
  nothing, and that is correct rather than a limitation to fix.
- **N3.** Re-folding on `Esc`. A sweep unfolds ancestors to bring its
  match into view, and `Esc` does not put those folds back. vim does not
  either (`'foldopen'` contains `search`); and the alternative — not
  unfolding — means the previewed match is invisible on exactly the rows
  where a preview is most wanted.
- **N4.** Highlighting more than one pane at a time. The prompt routes
  by focus, so the pattern belongs to the focused pane; painting a stale
  pattern into an unfocused pane is noise.
- **N5.** A `:nohlsearch` command. `Esc` outside the prompt does the job
  (S15) and protolens has no reason to grow a command for it.
- **N6.** Searching on a worker thread. See A2 — the obstacle is
  ownership of the rendered text, not parallelism.
- **N7.** Reporting sweep progress ("42% searched"). The pattern's tint
  (S10) already says "no answer yet", and a percentage of a scan the user
  will interrupt in 200 ms is chrome nobody reads.

## Specification

Six steps, each shippable on its own. Step 2 is the substance; step 1
comes first only because it makes every measurement in step 2 smaller.

### Step 1 — the fold gets a prefilter

- **S1.** `SearchPattern::find`'s case-insensitive arm gains a
  first-character prefilter. When the needle's first `char` is ASCII
  **and the haystack is entirely ASCII**, candidate positions come from
  `memchr2` over that character's two cases and only those positions run
  `starts_with_folded`; anything else keeps today's every-position walk.
  The observable semantics do not change — this is the same predicate,
  evaluated at fewer places.

  The evidence is Background 4's table: the case-sensitive row is the
  same walk *with* a `memchr`, and it is 6× faster.

  Both guards are needed, and the haystack one is the one that is easy
  to miss. A prefilter is the same predicate only where folding is one
  byte to one byte, which is true of ASCII and false of Unicode in both
  directions: `U+212A` KELVIN SIGN folds to `k`, and the `İ` that spec
  0195 S2 exists to handle folds to `i` plus a combining dot. Either in
  a haystack would lose a match the every-position walk finds. The
  *needle* cannot hold them — a pattern containing an uppercase
  character is case-sensitive and never reaches this arm — so the
  haystack is where the guard belongs. Rendered textproto is ASCII
  outside the occasional string value, so the fallback is rare.

### Step 2 — the search becomes a resumable sweep

- **S2.** A search is a value on `App`, not a call:

  ```rust
  struct SearchSweep {
      pattern: SearchPattern,
      dir: SearchDir,
      origin: SearchOrigin,
      /// Where the walk has got to. `None` once it has finished.
      at: Option<SweepCursor>,
      /// The best match so far, if any.
      found: Option<SweepHit>,
      /// How many candidates remain before the whole document has been
      /// seen — the wrap budget, decremented per candidate.
      remaining: usize,
  }
  ```

  It is advanced by `App::search_sweep_step() -> SweepStep`, which
  visits at most `SEARCH_SWEEP_SLICE` candidates and returns
  `Progressed` or `Idle` — deliberately the same shape as
  `prefetch_step` (`prefetch.rs:226`), because it plugs into the same
  socket.

- **S3.** The sweep is driven from `run_loop`'s existing `try_recv`
  loop (`terminal.rs:598-632`), in the `TryRecvError::Empty` arm, ahead
  of read-ahead:

  ```rust
  Err(mpsc::TryRecvError::Empty) => match app.search_sweep_step() {
      SweepStep::Progressed => { /* deadline check, then continue */ }
      SweepStep::Idle => match app.prefetch_step() { /* as today */ },
  },
  ```

  Nothing else about the loop changes. It already declines to sleep
  while background work remains — `recv_timeout` is reached only once
  `prefetch_step` reports `Idle` — so a live sweep keeps the loop awake
  by the same route read-ahead already does, and a finished one lets it
  sleep again. The deadline re-check the `Progressed` arm already
  performs applies unchanged: an expiring message or a due heat repaint
  must not be starved by a sweep any more than by read-ahead.

  The sweep is tried **first** because it is answering a question the
  user is actively waiting on, where read-ahead is speculative.

- **S4.** `SEARCH_SWEEP_SLICE` is a responsiveness knob and nothing
  else: it changes no result, only how long a keystroke may wait behind
  a slice. Background 4 gives 1.63 s over 5 278 324 lines ≈ 310 ns per
  candidate, so 1000 candidates is ~310 µs of worst-case added key
  latency — while a slice of 1 would spend 5.28 M `try_recv` calls,
  around 10% of the sweep, on channel overhead. 1000 is the starting
  value; the measurement that confirms it belongs next to the constant.

- **S5.** A sweep forces a frame when its result *changes* — a new
  match, or the walk finishing with none — and not once per slice,
  which would draw at several hundred frames a second. One more
  `..._forces` term alongside the five at `terminal.rs:678-690`.

- **S6.** Opening a `/` or `?` prompt records a `SearchOrigin`: for the
  main pane `scroll_offset`, `pan_offset` and `cursor_pos()`; for the
  override and manage panes their own highlight index, scroll and pan.
  It is consumed by `Esc` and dropped by `Enter`.

- **S7.** Any change to `command_buffer` while `command_kind` is
  `Search(_)` — insertion, `Backspace`, `Delete` — **replaces** the
  sweep: the one in flight is dropped, the origin is restored, and a new
  sweep starts from the origin in the prompt's direction. Abandoning a
  sweep is a struct assignment; there is nothing to cancel and nothing
  to join.

  Unconditionally, with no exception for "the pattern only grew": that
  symmetry is the entire reason `Backspace` reads as an undo. An empty
  buffer starts no sweep and restores the origin.

- **S8.** A sweep does not move the cursor. It scrolls and pans (S12),
  unfolds the ancestors of its match (N3), and sets the highlight
  (S14) — and that is the whole of its effect on the pane. In
  particular it does **not** call `record_jump`, because the cursor it
  would record leaving has not left.

- **S9.** `Enter` commits: the cursor moves to the sweep's match — this
  is the only point at which it moves, and the only point at which
  `record_jump` fires — `last_*_search` is written, the origin is
  dropped and the highlight stays on. On an `Enter` arriving before the
  sweep has finished, the sweep is run to completion synchronously
  first: the user has stopped typing and has asked for the answer, so
  the wait is now the answer's cost rather than a keystroke's.

  The existing vim convention of `Enter` on an empty buffer re-using the
  last pattern (`command_line.rs:118-128`) is unchanged.

- **S10.** No `pattern not found` message is set while the prompt is
  open. That message and the prompt share one row, so writing it per
  keystroke would flicker the prompt away under the user's hands.
  Instead the typed pattern itself is drawn in `search_unmatched_style`
  whenever the live sweep has no match — which covers both "finished,
  nothing there" and "still looking", deliberately: from the user's
  seat those are one fact, and the next keystroke or `Enter` resolves
  it. The message returns at `Enter`.

- **S11.** `Esc` drops the sweep, restores the origin's scroll and pan,
  closes the prompt and clears the highlight. The cursor needs no
  restoring — by S8 it never moved.

### Step 3 — a match is centered, or left alone

- **S12.** Bringing a match into view is *not* the minimum nudge
  `clamp_scroll_to_visible` performs. The rule, applied independently on
  each axis:

  - if the match is already fully within the drawn region on that axis,
    that axis does not move;
  - otherwise that axis is set so the match is centered in it, clamped
    to the document's own bounds.

  Vertically the match's display row against
  `scroll_offset .. scroll_offset + pane_height`; horizontally the
  match's whole extent against
  `pan_offset .. pan_offset + width - FOLD_FIELD_WIDTH`, so a match
  wider than the pane centers on its start rather than jittering.

  "Already visible, so do not move" is what makes a sweep across nearby
  matches readable — the alternative recenters the pane on every hit and
  the text appears to swim.

- **S13.** The centering runs inside `render`, driven by a flag the
  sweep sets and `render` clears. The pane height and width are known
  nowhere else — which is already why `clamp_scroll_to_cursor` is called
  from there.

  It does not fight the cursor-follow clamp at `render.rs:1070`: that
  clamp is gated on `cursor_row` having changed, and by S8 it has not.

### Step 4 — the matches are drawn

- **S14.** Highlighting is resolved **per frame, for the window only** —
  the same rule and the same reason as spec 0187 S3's syntax pass. Each
  drawn row's text is scanned for every occurrence of the pattern; the
  sweep's own match is drawn in `search_current_style`, the rest in
  `search_match_style`.

  Both are backgrounds, never inversions, on spec 0233 S3's rule:
  inversion is the caret's idiom and is not shared. They must be
  distinguishable from each other, from `cursor_row_style` and from
  `brace_match_style`, since all four can land on one row.

  Order within a row: the row cue, the selection, the brace partner, the
  search highlight, then the caret — the caret applied last so it still
  wins its own cell.

- **S15.** One flag, `search_highlight`, is on while a `Search(_)`
  prompt is open and stays on after a commit and after `n`/`N`. `Esc`
  clears it: inside the prompt as part of S11, and outside the prompt as
  its own binding (N5). The pattern highlighted is the live
  `command_buffer` while the prompt is open, and the focused pane's
  `last_*_search` otherwise.

- **S16.** `restyle_char` (`render.rs:135`) generalizes to a character
  *range*. It is the same walk over the same coordinate system, and that
  coordinate system is the one to use: characters of the *drawn* row,
  because `row_text` and `row_spans` disagree on byte offsets wherever a
  folded node's `{ ... }` summary is spliced in. A match's index is
  converted from row column to drawn character exactly as the brace
  partner already is at `render.rs:1201-1207` — `FOLD_FIELD_WIDTH`,
  minus `pan_offset`, bounded by the pane's width.

### Step 5 — the command line is a real editor

- **S17.** `Ctrl-a` and `Ctrl-e` bind to start-of-line and end-of-line,
  aliasing the existing `Home`/`End` arms — matching what they already
  do in the main pane (spec 0208 S1), so the same two keys mean the same
  thing on both halves of the screen.

- **S18.** The `KeyCode::Char(c)` arm gains a guard: a `CONTROL`-
  modified character is *ignored*, not inserted. Background 3 is a
  defect and this is its fix; without it every future `Ctrl-` binding
  has to be added defensively to stop a stray letter appearing.

### Step 6 — a line has two haystacks

- **S19.** In the main pane, every candidate line offers **two**
  haystacks to the same pattern, with no shape test and no special
  syntax:

  1. the line's rendered text, as today;
  2. the line's owning node's positional path, rendered `/a/b/c` by
     `positional_path` (`navigation.rs:976`).

  A line matches if either does. There is no pattern reserved to one or
  the other: `4/2` is tried against both and matches `/1/4/23` on the
  path, and `2` is tried against both too.

- **S20.** A path match's landing column is the row's **Home anchor** —
  its first non-blank, spec 0199 S1's `CaretAnchor::Home`. The path is
  not on screen, so it has no column of its own to land on, and the row
  itself is what the match is about. A text match keeps today's rule and
  lands on the matched character (spec 0194 S8, `CaretAnchor::Free`).
  When both haystacks match one line, the text match wins the column,
  because it is the one the user can see.

- **S21.** The path is formatted into a `String` reused across the whole
  sweep — cleared and rewritten per candidate, not allocated per
  candidate. At 5.28 M lines a fresh `String` each time would cost more
  than the matching does. `positional_path` itself is O(depth), depth 13
  on googleapis.desc, with an O(1) `sibling_position` since spec 0216.

- **S22.** Only the **current** match is tinted when it matched on the
  path; other visible path matches are not. S14's muted tint marks
  matches the user can *see*, and a path match has nothing visible to
  mark, so it would have to tint the whole row — and under S19 a short
  pattern matches most rows' paths, which would tint most of the pane.
  The current path match gets one cell, at the Home anchor S20 names,
  in `search_current_style`: exactly where the caret will land on
  `Enter`.

  The honest cost of S19, recorded here rather than discovered later: a
  one- or two-character pattern now matches most lines, so `n` on one
  steps through nearly every row. That is inherent to two haystacks and
  bites only patterns too short to be worth typing.

- **S23.** The override and manage panes list FQDNs, not nodes. They
  have one haystack; S19–S22 do not apply there.

## Alternatives considered

### A1. Reserve a pattern shape for paths

A pattern of only digits and `/`, with at least one `/`, matches paths
*instead of* text; anything else matches text only. `4/2` is then a path
query and `2` is unambiguously a text query.

Rejected in favor of S19's union, which is simpler to state and to
predict: there is no rule to remember about which patterns mean what,
and no pattern that becomes unsearchable as text. The shape test's own
advantage — that short patterns stay sparse — is bought back where it
actually shows, in the rendering (S22), rather than in the matching.

### A2. Sweep on a worker thread

The obvious reading of "non-blocking", and the codebase already has the
machinery (`heat_worker.rs`, `AppEvent::HeatWorkerProgress`).

Rejected on ownership, not on complexity. The searched text is
`node_text: Vec<Option<Box<str>>>`, owned by `App` and rewritten by
every override splice. Sharing it means either `Arc::make_mut` — a deep
clone of 4.7 M boxed strings on the first splice after a search — or an
`RwLock` that a two-second sweep holds while a splice blocks on it. The
heat worker escapes this because what it shares is `Arc<Blob>` and the
scoring graph, both genuinely immutable; the rendered text is not.

S2's resumable sweep gets the same two properties the thread was wanted
for — the keyboard never waits, and a sweep is abandoned rather than
finished — with no sharing, no lock, and an abort that is a struct
assignment.

### A3. A bounded preview: search N lines, and the rest only on `Enter`

Keeps the search synchronous and caps its cost per keystroke.

Rejected: it makes the preview's answer depend on how far the budget
happened to reach, so `Enter` can land somewhere the preview never
showed. The resumable sweep costs the same per keystroke and always
converges on the true first match.

### A4. Search the visible window only while typing

Cheaper still, and trivially responsive.

Rejected because it inverts what incremental search is *for*. The point
is to find something that is not on screen; a preview that can only find
what is already visible answers a question nobody asked.

### A5. Move the cursor during the preview

vim's `'incsearch'` appears to do this, and it needs no separate
centering rule — the existing cursor-follow clamp would scroll for free.

Rejected because the cursor is what the rest of the UI acts on: the
local statusline, the heat cue, `t`, `x`. Dragging it through every
intermediate match makes those read as noise, and it puts the jumplist
and the caret anchor in play for a position the user has not chosen.
Moving only the view keeps a preview a preview.

### A6. Cache a document-wide match list on commit

Compute every match once, and let `n`/`N` and the highlight index it.

Rejected on size before anything else: a common pattern over 5.28 M
lines is a list of millions of positions, rebuilt on every keystroke
during a preview, and invalidated by every override splice. The window
scan S14 specifies is bounded by the pane height and needs no
invalidation at all.

### A7. Leave the highlight on until a new search replaces it

Simpler than S15's flag — no way to turn it off, so no state.

Rejected: a persistent highlight the user cannot dismiss is why vim
ships `:nohlsearch` and why nearly every vim configuration binds it to
something. Having just searched for `id`, the user is left reading a
document with a third of its characters tinted.

### A8. Reuse `Modifier::REVERSED` for the current match

It is what several terminal tools do, and it needs no new theme entry.

Rejected by spec 0233: inversion is the caret's idiom and sharing it is
precisely the defect that spec removed. Two inversions three characters
apart on one row cannot be told from one wide one.

## Test plan

1. `typing_into_a_search_prompt_scrolls_to_the_match` — after `/`, `f`,
   `o` and enough sweep steps, the match is on screen and the cursor has
   not moved (G1, G3).
2. `a_keystroke_abandons_the_sweep_in_flight` — a buffer change mid-
   sweep leaves a sweep whose pattern is the new one and whose walk is
   back at the origin (S7).
3. `a_sweep_step_visits_at_most_one_slice` — `search_sweep_step` on a
   document longer than `SEARCH_SWEEP_SLICE` returns `Progressed`
   having visited exactly a slice, and returns `Idle` only once the
   walk has finished (S2, S4).
4. `a_finished_sweep_lets_the_loop_sleep_again` — `search_sweep_step`
   reports `Idle` with no sweep live, which is the condition
   `run_loop` needs to reach `recv_timeout` (S3).
5. `a_sweep_forces_a_frame_only_when_its_result_changes` — slices that
   find nothing new set no redraw force; the slice that finds the match
   sets one (S5).
6. `backspace_undoes_the_keystroke_before_it` — `/`, `f`, `o`, `o`,
   `Backspace` leaves the view exactly where `/`, `f`, `o` did (G2, S7).
7. `esc_puts_the_view_back_and_never_moved_the_cursor` — scroll and pan
   equal their pre-`/` values, and `cursor_pos()` never changed at any
   point (G3, S6, S8, S11).
8. `a_preview_records_no_jumplist_entry` — `back_stack` grows by exactly
   one over a whole `/foo` + `Enter`, and by zero over `/foo` + `Esc`
   (S8, S9).
9. `enter_finishes_an_unfinished_sweep` — `Enter` pressed while the
   sweep is mid-document still lands on the true first match (S9).
10. `an_unmatched_pattern_tints_itself_and_sets_no_message` — the
    prompt row still shows the prompt while the sweep is running and
    after it finds nothing; the `pattern not found` message appears
    only at `Enter` (S10).
11. `a_match_already_on_screen_moves_nothing` — neither `scroll_offset`
    nor `pan_offset` changes (S12).
12. `a_match_off_screen_is_centered_on_both_axes` — a match below the
    window and right of the pan lands at the middle row and the middle
    column (G4, S12).
13. `every_visible_match_is_tinted_and_the_current_one_differently` — a
    row with three occurrences: the sweep's own in
    `search_current_style` over its whole extent, the other two in
    `search_match_style` (G5, S14).
14. `the_highlight_survives_the_commit_and_n` — still drawn after
    `Enter`, and after `n` the strong tint has moved to the new match
    (G5, S15).
15. `esc_outside_the_prompt_clears_the_highlight` (S15).
16. `the_search_highlight_yields_its_cell_to_the_caret` — a caret inside
    a match keeps `caret_style` (S14's ordering).
17. `ctrl_a_and_ctrl_e_move_the_command_cursor` — and, the point of
    Background 3, leave the buffer's *contents* unchanged (S17, S18).
18. `a_control_modified_letter_is_not_inserted` — `Ctrl-u` into a search
    prompt leaves the pattern alone (S18).
19. `a_pattern_is_tried_against_the_path_and_the_text` — `4/2` finds the
    node at `/1/4/23` even though no line contains `4/2`, and on a
    document where one line's *text* also contains `4/2` both lines are
    reached in document order by `n` (G8, S19).
20. `a_path_match_lands_on_the_home_anchor` — the caret sits on the
    row's first non-blank with `CaretAnchor::Home`, while a text match
    on the same document lands on the matched character with
    `CaretAnchor::Free`; a line matching both ways takes the text
    column (S20).
21. `a_path_match_tints_only_the_current_row` — the current path
    match's Home-anchor cell carries `search_current_style` and no
    other row is tinted, on a short pattern that matches most paths
    (S22).
22. `the_side_panes_match_text_only` — a pattern that would match a
    node's path finds nothing in the override pane (S23).
23. `the_prefilter_preserves_smartcase` — S1 changes no result: the
    existing smartcase, multi-character-mapping and leading-space tests
    of spec 0195 pass unchanged, and a non-ASCII-initial pattern still
    matches.

## Measured outcome

googleapis.desc opened as `google.protobuf.FileDescriptorSet` —
**5 281 124 lines** — searched for a pattern that matches nothing.
Five runs; the ranges below are their spread.

### The matcher, against Background 4

Background 4's table measured the text haystack only, so this row does
too. `zzqqzz` folds under smartcase; `ZZQQZZ` does not and takes the
`memchr` arm S1 left alone.

| pattern | before S1 | after S1 |
|---|---|---|
| `zzqqzz` (lowercase, folds) | 1.63–2.05 s | **183–272 ms** |
| `ZZQQZZ` (case-sensitive) | 269–499 ms | 276–422 ms |

A **7x** improvement on the case a user actually types, and none on the
case-sensitive arm, which is what a prefilter confined to the folding
path should produce. The folding arm is now the *faster* of the two:
`memchr2` over a two-byte alphabet beats `str::find`'s two-way search
on a six-character needle.

### The path haystack costs more than the prefilter saved

S19's second haystack is built for every candidate the text misses,
which on a full miss is every candidate:

| | text only | both haystacks |
|---|---|---|
| `zzqqzz` | 183–272 ms | 644–935 ms |
| `ZZQQZZ` | 276–422 ms | 811 ms–1.24 s |

≈ 95–120 ns per line, or roughly two thirds of the whole sweep. S21's
reused buffer removes the allocation but not the walk to the root or
the formatting. This is not a regression against Background 4 — the
feature did not exist — but it is where a further optimization would go,
and it is the reason a full sweep did not get three times cheaper.

### The slice, and what a keystroke waits for

`SEARCH_SWEEP_SLICE = 1000` is unchanged from the spec's estimate, and
the measurement confirms the arithmetic it was chosen by: 5 282 slices,
**worst slice 222–797 µs**, typically 250–400 µs. That worst slice is
the whole of the added keystroke latency, since a keystroke is served as
soon as the slice in flight returns.

Converging a full no-match sweep through `search_sweep_step` costs
**647–961 ms** against the 644–935 ms of the same walk run in one call
— the slicing itself is below the noise. So the answer to a `/` that
matches nothing arrives in about a second, during which every keystroke
is served in well under a millisecond.
