<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0257 — the first pane does not wait for the last line

Status: draft
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S1/S3,
        the row budget and `auto_folded`; S8, expand-on-arrival; S13,
        the "not yet baked" caveat on a search miss),
      docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the idle-arm bake this hands the remainder to, and rule 2's
        `bounded_confirms` flag),
      docs/specs/0258-a-revealed-subtree-is-the-subtree-it-would-have-been.md
        (**a prerequisite** — without it a bounded startup silently
        stops expanding `Any` below the first screenful),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        arena, which is still built whole and is the floor left over),
      docs/specs/0198-a-subcommand-that-only-starts-up.md (`quit`, whose
        meaning S4 changes)

## Background

Spec 0249 bounded a *confirm* to a screenful and spec 0255 baked the
remainder in the idle loop. Opening a file was never bounded: it renders
the whole document before the first frame.

Measured on `googleapis.desc` (25.6 MB), typed root, release build,
`taskset -c 4-11`, timers around each phase (`quit`, so no terminal
probes and no frame):

| phase | s | % |
|---|---|---|
| blob + arena build + root-type inference | 0.68 | 8 |
| `decode_and_render_indexed` — the whole document into one ~232 MB `String` | 2.21 | 25 |
| `from_utf8` + `text.lines()` → 5 279 383 separate `String`s | 1.04 | 12 |
| `build_tree` — spans into slots, one `Box<str>` per node | 1.48 | 17 |
| main.rs + `App::new` preamble | 0.85 | 10 |
| `rebuild_status` | 1.22 | 14 |
| `App::new`'s `render_overrides` pass | 0.09 | 1 |
| dropping the document at exit | 1.13 | 13 |
| **total** | **8.72** | |

`--raw` is 4.65 s over 3 668 694 lines. Roughly fifty of those lines
reach the screen.

Two things in that table are corrections rather than news:

- **`main.rs:518-522`'s comment is wrong.** It names `App::new`'s
  `render_overrides` pass the largest startup phase, at "5.2 s". It is
  **94 ms**. It was written when the pass re-spliced far more than it
  does now; nothing has re-measured it since.
- **Root-type inference is ~0.04 s here, not the ~0.9 s a whole-corpus
  sweep costs.** `--raw` skips the sweep and still spends 0.63 s before
  the render, so that prelude is the blob map and the arena build, and
  the sweep adds almost nothing on top of it.

### The bound already exists, one caller away

`DecodeRenderOpts::row_budget` and `IndexingTextSink::undescended` are
already in prototext-core, already honored by `len_field`, `any_field`
and `message_set_field`, and already consumed by `splice_override`.
`render_resolved` is the one renderer call in the program that passes
`..Default::default()` and gets `None`. `build_tree` is literally
`overlay_spans(…, &[])` with a `debug_assert!(stopped.is_empty())` on
top.

Threading the budget through — a throwaway probe, reverted — gives:

| `PROTOLENS_STARTUP_BUDGET` | startup | lines | stops |
|---|---|---|---|
| unbounded (today) | 8.72 s | 5 279 383 | — |
| 50 | **1.14 s** | 15 595 | 7 771 |
| 500 | **1.16 s** | 16 044 | 7 770 |
| 5000 | **1.14 s** | 20 714 | 7 854 |
| 50, `--raw` | **1.13 s** (from 4.65 s) | 15 596 | 7 772 |

**7.6x**, and flat in the budget. That flatness is spec 0249 S1's
"output = budget + breadth" seen from the other end: under a 7 771-file
root the breadth term is three orders of magnitude larger than any
screenful, so the budget is not what costs anything. It also means the
number need not be tuned.

`rebuild_status` shrinks along with it without being touched: it is
O(arena) in *iterations*, but `own_status` on a vacant slot returns
after one `Option` test and one hash lookup.

### What a bounded startup breaks, and would have broken silently

**Spec 0258 is a prerequisite.** The bake does not re-apply override
resolution to the subtree it reveals, so spec 0120's `Any` and
MessageSet auto-expansion never runs on baked content — a live defect in
specs 0249/0255 today, narrow enough to have gone unreported because it
takes a root override plus scrolling to reach.

It blocks this spec because today the startup render is followed by one
`render_overrides` pass over the whole document. Bounding the render
without 0258 would quietly stop expanding every `Any` below the first
screenful of every file opened. Spec 0258 carries the transcript, the
mechanism and the fix.

## Goals

- **G1.** Opening a document costs a screenful, not a document. The
  first frame is drawn without any part of startup being proportional
  to the rendered line count.
- **G2.** The first frame shows what it shows today. Every row on it is
  final; no row on it is a fold that pops open a moment later.
- **G3.** The document a bounded startup plus a full bake produces is
  the document an unbounded startup produces — the same text, the same
  counts at every node, the same exported bytes — at every budget and
  with no budget at all. Spec 0258's G1 is what makes this reachable at
  all; this spec inherits it rather than restating it.

## Non-goals

- **N1.** The remaining 0.64 s: the blob map, the arena build and
  inference. The arena is the addressing every slot index means (spec
  0216), so it cannot be deferred the way text can — a different spec,
  and now the largest single item in a cold open.
- **N2.** Making the total work smaller. As in spec 0256 N1, this moves
  work rather than removing it: the same ~5 s of rendering happens, in
  the idle loop, where nobody is waiting. Peak memory is unchanged at
  the end and much lower until then.
- **N3.** The triple materialization — one ~232 MB `String`, then
  5.28 M separate `String`s, then a `Box<str>` per node — of which the
  middle copy alone is 1.04 s. A bounded startup defers it; removing it
  is independent of this spec and worth its own.
- **N4.** Restructuring `rebuild_status`. Its 1.22 s is a consequence of
  a fully rendered document, not of the sweep, and S5 records why the
  sweep is left alone.
- **N5.** Export and clipboard of an unbaked document. `push_subtree_lines`
  walks into a stop's vacant children and emits an empty pair of braces,
  so an interactive `:export` during a bake silently writes a truncated
  document. This is pre-existing (spec 0255 left it), it is not made
  worse in kind by this spec, and fixing it is a question about what an
  export of a partially-baked document should *mean* — refuse, wait, or
  force the bake — which deserves deciding on its own. Recorded here
  because this spec is what makes it reachable in the first ten seconds
  of every session.

## Specification

### S1. The document render takes a row budget

`render_resolved` gains a `row_budget: Option<usize>` parameter, passes
it into `DecodeRenderOpts`, and threads `rendered.undescended` into
`build_tree`. `build_tree` gains an `undescended: &[u32]` parameter and
returns `overlay_spans`' `stopped` list alongside the tree and the text;
its `debug_assert!(stopped.is_empty())` goes with the assumption it
encoded. `Decoded` carries the stops to `App::new`.

Nothing else in the renderer changes: the budget is a `line_count >=
budget` test in `TextSink`, independent of `emit_header` and
`initial_level`, and every helper that can stop already calls
`note_undescended`.

`Decoded::total_lines` is the bounded count. Its only production reader
is the `indexing N lines` phase line; `App::total_lines()` is derived
from the tree's counters and stays correct throughout the bake.

### S2. The budget is the terminal's real height, read before the alternate screen

`main.rs` obtains it from `crossterm::terminal::size()`, subtracts the
two chrome rows the same way the first frame will, and clamps to
`App::MIN_EXPAND_ROWS`. When `size()` fails there is no interactive
session to bound (S4), so the question does not arise.

Getting the number wrong is cheap in both directions and this is what
removes the pressure to be exact:

- **too small** (the terminal was resized between the measurement and
  the first frame, or the user starts in wire mode, where a document
  line costs two rows) — the visible stops are expanded by the frame
  itself, through spec 0249 S8's `note_visible_stops` and the bake's
  `Visible` arm, before the user can act on them;
- **too large** — the table above says it costs nothing measurable.

Deliberately *not* `document_pane_height()`: `main_area` is written by
`render_main_pane` and is `Rect::default()` until the first frame, so
asking `App` would return zero.

### S3. `App::new` receives the stops and the flag

`App::new` seeds `auto_folded` and `bake_queue` from `Decoded`'s stops
and calls `refresh_line_counts` on each — the same three things
`splice_override` does with `overlay_spans`' return value, for the same
reason: a stop is rendered as a header and a footer, and folding it is
what makes it one row instead of an empty pair of braces.

`bounded_confirms` becomes a constructor parameter instead of an
assignment inside `run_loop`. It has to be true *before* `App::new`'s
`render_overrides` pass, or that pass's own splices — Any expansion, a
seeded root — would render unbounded and undo the bound in the one place
it is hardest to notice. Spec 0255 rule 2's requirement that the flag be
explicit is preserved: it is now explicit at the constructor rather than
one statement into the event loop, and there is one fewer window in
which `App` disagrees with itself.

### S4. Only a session with a loop to bake in is bounded

`main.rs` passes a budget when it is about to enter the TUI, and `None`
otherwise. A headless `export` renders whole, exactly as spec 0256 S3
requires and for the same reason: nothing would ever drain the stops.

`quit` is bounded, and this changes what it measures. It exists to time
startup (spec 0198); after this spec, startup *is* the bounded path, and
timing the unbounded one would be timing something no user experiences.
Its phase line reports the bounded line count, which is the honest
number for what it did. Timing a whole-document render is what `export`
is for.

### S5. `rebuild_status` is left alone

It sweeps the whole arena in reverse and that is what makes it correct
and simple. Under a bounded startup nearly every slot is vacant,
`own_status` returns after one `Option` test and one `auto_folded`
lookup, and the 1.22 s goes with the document that caused it. Nothing to
change; recorded so the next reader does not go looking for a phase that
no longer shows up in a profile.

## Alternatives considered

**Render lazily, one node at a time, as rows are drawn.** This is the
"JIT" shape the row budget replaced. Spec 0249 Part E settled it: a
bounded render is a *full* render in which every undescended node is
folded, the cut therefore falls on a node boundary by construction, and
there is no stale text and no just-in-time line. Per-row lazy rendering
brings back a cut that can land anywhere and a fallback path for text
that is not there yet.

**Keep the render whole and bound only the text.** Deferring
`from_utf8`/`lines()`/`build_tree` while `decode_and_render_indexed`
still runs whole saves 2.5 s of the 4.7 s and none of the 2.2 s that is
the actual decode. It also keeps the ~232 MB `String` alive across
startup. Half the win for most of the complexity.

**Bound the render but keep the whole-document `render_overrides`
pass.** The pass walks the rendered tree; below the first screenful
there is nothing rendered to walk, so it would silently do less rather
than fail. That is spec 0258's defect wearing a different hat, and it is
why 0258 is a prerequisite rather than a nice-to-have.

**Build the arena lazily too.** The arena is what a slot index *means*
(spec 0216): `first_child`, `parent`, `raw_start` and every navigation
step read it, and it is what makes the render's spans addressable at
all. Deferring it means deferring the index space itself. N1.

**Leave `quit` unbounded so it keeps timing the same thing.** Then the
one subcommand whose purpose is to measure startup measures a path no
interactive session takes. Changing what it measures is the point, not a
side effect.

## Test plan

1. `a_bounded_startup_leaves_stops` — `render_resolved` with a budget
   returns a non-empty stop list, `App::new` has them all in
   `auto_folded` and in `bake_queue`, and every stop's `lines_visible`
   is 1.
2. `a_baked_startup_is_the_unbounded_startup` — the G3 anchor, and the
   assertion the spec rests on. Build the same fixture twice, bounded
   and not, drain the bounded one, and compare `document_lines()` and
   every node's `lines_total`/`lines_visible`. Parametrized from
   `App::MIN_EXPAND_ROWS` up: a budget of 1 buys only the header, so the
   walk cannot descend and the drain produces a raw document — which is
   why the clamp exists and why the sweep must start above it.
3. `a_headless_startup_renders_whole` — with no budget, `auto_folded` is
   empty after `App::new` and the document is complete. The S4 guard,
   and the only thing between a scripted `export` and a truncated file.
4. `a_first_frame_over_a_bounded_document_has_no_visible_stop` — draw
   one frame at the budget's height and assert no drawn row is in
   `auto_folded`. G2, and the check that S2's arithmetic matches the
   renderer's.
5. `a_bounded_startup_expands_any_below_the_screenful` — a fixture whose
   `Any` falls outside the budget, opened bounded and drained, expands.
   Spec 0258's mechanism, asserted here on the startup path that is this
   spec's reason for needing it.
6. Corpus, not a test: bounded startup on `googleapis.desc`, drain,
   export, `cmp` against today's export. Same recipe as spec 0256's,
   including that `--raw` is mandatory when the point is to exercise a
   root override.

## Measured outcome

Filled in at implementation. The probe above (S1's threading only, on
one binary, `taskset -c 4-11`) puts the target at **8.72 s → ~1.14 s**
typed and **4.65 s → ~1.13 s** raw, with the residual dominated by N1's
arena build.
