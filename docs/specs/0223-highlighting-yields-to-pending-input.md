<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0223 — highlighting yields to pending input

Status: implemented
Implemented in: 2026-08-01
App: protolens
Refs:
- `docs/specs/0187-highlighting-is-a-property-of-the-viewport.md` —
  scoped the highlight pass to the drawn rows, which is what makes it
  cheap enough to be *skippable* rather than something to cache. This
  spec makes that per-frame pass conditional. 0187 is implemented and is
  not reopened.
- `docs/specs/0152-protolens-heat-cue-background-scoring-thread.md` (G8)
  — the input-reader thread and the single `AppEvent` channel `run_loop`
  waits on. §S1 hangs a counter off it.
- `docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md` —
  the same goal, from the other side: 0192 removed a per-frame cost that
  varied with position, this removes one that varies with input rate.
- `docs/specs/0222-the-text-lives-in-the-nodes.md` — deliberately
  independent (its N3). The two touch the same function,
  `render_main_pane`, and nothing else.

## Background

Tree-sitter is the largest single term in a frame, and it is not close.
Measured through a pty at 50×200 on `googleapis.desc` (25.6 MB), 48
drawn rows, `PROTOLENS_TRACE` reading `render … us=`:

| µs per frame | top of document | after `G` (end) |
| --- | --- | --- |
| `window` (descent + walk) | 5–62 | 17–89 |
| **`styles` (tree-sitter)** | **364–486** | **220–491** |
| `heat` | 49–102 | 33–69 |
| `ovr` | 11–42 | 6–19 |
| `lines` (row spans) | 52–80 | 32–103 |
| whole `terminal.draw` | 1249–2637 | 658–1556 |

`refresh_window_styles` reparses the ~50 visible rows on **every** frame:
`window_text` clones them into a fresh `Vec<String>`, `window_styles_for`
frames them with synthetic `_ {` / `}` lines to recover the opening
depth, joins the whole thing into one `String`, and hands it to
`colorize::colorize`. Nothing is retained across frames, by design —
0187 chose recomputation over a cache because the window is small.

That is the right trade for a frame the user is looking at. It is the
wrong trade for a frame the user is scrolling past. Holding `PageDown`,
or rolling the wheel, produces a stream of frames of which only the last
is read, and each one pays 220–490 µs to color text that is on screen
for ~30 ms before being replaced.

## Goals

- **G1.** While input is outstanding, a frame does not pay for syntax
  highlighting.
- **G2.** **The frame the user actually reads is always in color.** A
  scroll that ends must settle to a highlighted screen with no further
  input.
- **G3.** Nothing but syntax highlighting is dropped. The cursor, the
  caret, heat cues, the override bold and the selection are how the user
  knows where they are mid-scroll; they must survive.

## Non-goals

- **N1. Not a general degradation framework.** One decision, one input,
  one term dropped. No frame-budget accounting, no tiering, no other
  pass made conditional. If a second term ever wants the same treatment,
  it can share the predicate — it does not need an abstraction built for
  it in advance.
- **N2. Not caching the highlight result.** Spec 0187 considered and
  rejected a cache; this spec does not revisit it. Skipping work
  entirely is strictly better than remembering it.
- **N3. Not throttling or coalescing input.** Every event is still
  delivered and still acted on; the cursor lands exactly where the same
  keystrokes would have put it today. Only the *painting* changes.
- **N4. Not touching where the line text comes from.** That is spec
  0222.

## Specification

### S1. "Input is outstanding" is a counter on the event channel

`InputReaderHandle::spawn` (`tui/event.rs:39`) sends every terminal
event down one `mpsc::Sender<AppEvent>`; `run_loop` receives them. Add
an `Arc<AtomicUsize>` incremented by the reader immediately before
`send` and decremented by `run_loop` once per event taken off the
channel. The predicate is `count.load() > 0`.

An `mpsc` receiver cannot be peeked, which is why this is a counter
alongside the channel rather than a length query on it.

**There is no single `recv`.** `run_loop`'s receive is a loop with two
receiving arms — a `try_recv` at its head, interleaved with read-ahead,
and a `recv_timeout` in the `PrefetchStep::Idle` case — and it can also
break with no event at all when a deadline comes due mid-read-ahead.
Both arms feed one `let received = loop { … };` binding, so the
decrement goes immediately after that binding: one site, reached by
every path, and by construction unable to miss an event or count one
twice.

**The counter belongs to the channel, not to the reader thread.** The
Neovim handoff shuts the reader down and spawns a fresh one, while
events the outgoing reader already sent are still queued waiting to be
counted down. So `spawn` takes the counter as an argument, and `run`
owns it beside the channel; a counter created per reader would let those
decrements land on a new zero and wrap.

**The counter must not count `AppEvent::HeatWorkerProgress.`** Only
`AppEvent::Term` increments it. The heat worker emits progress
continuously while the cues for a new viewport resolve — exactly during
a scroll — so counting those would hold the display monochrome for as
long as the worker is busy, which is a different and much longer
condition than "the user is still scrolling".

### S2. Mouse motion never reaches the channel

`EnableMouseCapture` turns on any-motion reporting, so the terminal
sends a `MouseEventKind::Moved` on essentially every pixel the pointer
crosses. `handle_mouse` already discards them (`mouse.rs:12-24`) — but
it discards them *after* they have been queued, so they would count
toward §S1 and a hovering mouse would suppress highlighting on a screen
nobody is scrolling.

Move the filter to the reader thread: a `Moved` event is dropped before
`send`, never entering the channel. `handle_mouse`'s own guard stays —
it is the contract for the function, and the tests call it directly.

This *removes* code rather than adding it. `run_loop`'s receive loop
already had a `Moved` arm that dequeued one and `continue`d without
producing an event, so that a hovering pointer could not starve
read-ahead (spec 0164 G7). With the reader filtering, that arm has
nothing left to match and is deleted.

Wheel events (`ScrollUp`/`ScrollDown`) and `Drag` are **not** filtered.
A rolling wheel is precisely the flow this spec exists for, and a drag
is an active selection.

### S3. A monochrome frame is the absence of styles, not a second path

`row_spans` already reads
`self.window_styles.get(window_index).unwrap_or(&NO_STYLES)`
(`render.rs:630`), so an empty `window_styles` renders correct,
unhighlighted text with no other change. The monochrome frame is
therefore:

```rust
if input_pending {
    self.window_styles.clear();
} else {
    self.refresh_window_styles(&window);
}
```

`input_pending` is a plain `bool` field on `App`, sampled from the
counter by `run_loop` immediately before `terminal.draw`. Not the shared
counter itself: highlighting has to be decided once for the whole frame,
and a counter read from inside `render` could answer differently for two
panes of the same one. It also keeps the `Arc` out of `App`, so the
render tests set the flag directly.

`clear()` and not "skip the refresh": leaving the previous frame's
`window_styles` in place would apply one window's colors to a different
window's rows, which is worse than no color at all.

Everything in G3 is applied downstream of this, in the `text_lines`
closure — heat chrome via `heat_chrome`, the override bold, the
cursor-row tint, the selection `REVERSED`, and both carets. None of them
consult `window_styles`. So the degraded frame is "no syntax coloring",
not "no color": the user can still see where the cursor is.

### S4. The settled frame is repainted in color

This is G2 and it is the whole risk of the feature. Without it,
releasing `PageDown` leaves the screen permanently gray, because nothing
schedules another frame once input stops.

Drawing a monochrome frame sets a dirty flag. `run_loop` already has
exactly this shape for heat cues — `heat_dirty`, `last_heat_frame`, and
a deadline that wakes the `recv_timeout` — and the flag joins it: when
the loop is about to block with the flag set and the counter at zero, it
draws one more frame, this time with styles.

**Settle interval.** Key autorepeat is roughly 30/s, so the counter
momentarily reaches zero between repeats and a naive "repaint the
instant it is empty" would recolor between every pair of frames, giving
back the cost and adding flicker. The repaint therefore waits for the
counter to have been zero for a short interval. Start at **50 ms** — one
autorepeat gap at 30/s is ~33 ms, and 50 ms is under the ~100 ms at
which a delay stops reading as instant — and confirm it against a real
held key rather than against this paragraph (test-plan item 5).

**As implemented this clause is a backstop, not the mechanism.** Every
terminal event forces its own frame (`event_forces`), and the counter is
sampled *before* the draw while events are consumed *after* it. So the
event that takes the counter to zero is always followed by a draw that
samples zero and colors itself, and the deferred repaint never has to
fire — confirmed by the measurement below, where no frame is drawn for
reason `styles`. The flag is kept anyway: that argument is a property of
four `*_forces` clauses that will keep changing, and its failure mode is
a screen that stays gray until the user touches something. It makes the
recolor owed rather than incidental, at the cost of ten lines.

## Alternatives considered

### Time-budget the frame instead of counting input

Draw monochrome when the previous frame overran some millisecond budget.
Self-tuning, and it would cover causes this spec does not. Rejected: it
makes the display's appearance a function of machine speed and of
whatever else the box is doing, so the same keystroke colors on one run
and not on the next, and a bug report becomes unreproducible. "Is there
another event waiting" is a fact about the user's intent, not about the
hardware, and it is exactly the question worth asking.

### Highlight on a background thread

The parse leaves the frame entirely and the result arrives a frame or
two late. It removes the cost in *all* cases, not only under load.
Rejected for now on cost-of-machinery: it needs a window-identity token
so a late result is not applied to a scrolled-away viewport, and a
policy for the first frame at a new position, which is precisely the
frame the user is looking at. This spec is a dozen lines and gets most
of the win; the thread can supersede it later on its own evidence.

### Drop the heat pass too under the same predicate

`heat` is 33–102 µs, a fifth of `styles`, and unlike highlighting it is
*information* rather than presentation — a cue that blinks out while
scrolling is a worse trade than color that does. G3 keeps it
deliberately. N1's point is that the predicate is available if a future
measurement changes this.

## Test plan

1. `a_terminal_event_raises_the_pending_count_and_a_worker_event_does_not`
   — drive `InputReaderHandle`'s counter directly with both `AppEvent`
   variants. This is §S1's load-bearing distinction and the one that
   silently disables the feature if it is got wrong.
2. `mouse_motion_never_enters_the_channel` — a `Moved` event is dropped
   by the reader; a `ScrollDown` and a `Drag` are not.
3. `a_pending_frame_has_no_syntax_styles_but_keeps_its_chrome` — render
   one frame with the counter positive and one with it at zero over the
   same window: the second has non-empty `window_styles`, the first has
   empty; both carry the caret, the cursor-row tint and the heat glyph
   at identical positions. Guards G3.
4. `stale_styles_are_cleared_not_left` — scroll, then render a pending
   frame, and assert `window_styles` is empty rather than holding the
   previous window's entries. The specific defect §S3 warns about.
5. `a_settled_frame_is_repainted_with_styles` — with the counter back at
   zero and no further events, the loop must draw one more frame and
   that frame must have styles. **G2's acceptance test**; if only one
   test from this plan survives, it is this one.

   **Not written as a unit test, deliberately.** Per §S4's note the
   deferred repaint is unreachable from `run_loop` as it stands: the
   counter can only reach zero by way of a terminal event, and every
   terminal event already forces the next frame. A test would have to
   fabricate a counter state the reader thread cannot produce, and would
   then assert on a code path the running program never takes. G2 is
   covered instead by item 3 (counter zero ⇒ styles present) and by item
   6's colored-frame count, which is the end-to-end evidence the loop
   really does settle.
6. **The measurement.** The same pty script the Background table came
   from (50×200, 48 rows), but with the keys sent back-to-back rather
   than at a 1.5 s gap, so the queue is genuinely non-empty: report
   `styles` and the whole `draw` per frame, before and after. Expect
   `styles` at ~0 for every frame but the last, and the whole `draw` down
   by roughly its share. Also report the number of *colored* frames — one
   is the target; several means §S4's settle interval is too short.

## Measured outcome

Same pty harness as the Background table (50×200, 48 drawn rows,
`googleapis.desc`, `PROTOLENS_TRACE`), 60 PageDowns.

**With the keys 0.5 ms apart — a genuinely backed-up queue:**

| | colored frame | monochrome frame |
| --- | --- | --- |
| frames | 14 | 57 |
| `styles` µs | 320–480 | **0** |
| whole `draw` µs (mean) | 1194 | **632** |

A 47% cut in the whole frame, which is more than `styles`' own share:
`lines` falls from 50–80 µs to ~19 and `window` from 4–6 to 1–2, because
`row_spans` with no hints emits one span per row instead of one per
token, and ratatui then has less to diff. The screen settles back to
color at the end of the burst, and **no frame is drawn for reason
`styles`** — §S4's backstop is never needed, as that section now records.

**With the keys 30 ms apart — ordinary key autorepeat — nothing
changes at all.** Every frame stays colored at 350–500 µs, because a
1.3 ms frame drains a 33 ms repeat completely and the queue is empty at
every draw. This is the honest scope of the win: it is not "holding
PageDown on this machine", it is any stream that outruns the frame —
mouse-wheel inertia, an ssh/tmux link delivering a burst at once, a
slower box, or a wider terminal. Where the queue never backs up the
feature correctly does nothing.
