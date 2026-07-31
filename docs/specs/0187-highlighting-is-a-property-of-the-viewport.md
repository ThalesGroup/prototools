<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0187 — highlighting is a property of the viewport

Status: implemented (S1-S5; S6's measurement half was not written — see S6)
Implemented in: 2026-07-27
App: protolens
Refs: docs/protolens/rendering-scaling-roadmap.md (S6),
      docs/protolens/rendering-flaws.md (P1, P3, P4, D4),
      docs/specs/0116-protolens-syntax-highlighting.md (§7, §8, §9),
      docs/specs/0133-protolens-annotation-toggle.md (G4),
      docs/specs/0135-protolens-synthetic-field-names.md (G2),
      docs/specs/0174-protolens-preview-byte-budget.md (S4),
      docs/specs/0185-the-preview-is-an-overlay.md,
      docs/specs/0186-the-commit-touches-only-what-moved.md

## Background

Syntax highlighting is the largest single cost in protolens, at both of
the two moments the user waits: loading a document, and committing an
override. It is also the one cost that buys nothing structural —
protolens never reads the highlighter's output back to understand the
document. Structure comes from `decode_and_render_indexed`'s `NodeSpan`s.
The highlighter's output is consumed in exactly one place: drawing a row.

### What was measured

Spec 0186 was implemented and then instrumented with per-phase timers
inside `render_overrides`. Measured on `/tmp/pdb.desc` (146,511 nodes /
193,072 lines at load, growing past a million lines after the commit),
`--release`, pinned to one core.

Committing an override that retypes the whole document:

| phase | 1st commit | 2nd commit |
|---|---|---|
| `colorize::colorize` | **12.98 s (87.1%)** | **11.33 s (83.8%)** |
| `decode_and_render_indexed` | 0.46 s (3.1%) | 0.34 s (2.5%) |
| `hints_by_line` | 0.25 s (1.7%) | 0.23 s (1.7%) |
| `render_cache` insert (deep clone) | 0.18 s (1.2%) | 0.16 s (1.2%) |
| line-map repair | 0.17 s (1.1%) | 0.20 s (1.5%) |
| `compute_descend_marks` | 0.02 s (0.1%) | 0.04 s (0.3%) |
| everything else | 0.09 s (0.6%) | 0.08 s (0.6%) |
| **total** | **14.90 s** | **13.51 s** |

One tree-sitter parse of the whole rendered document is 85% of a commit.
Every other line in the table put together is under 8%. Spec 0186 set out
to remove four whole-document passes and did so correctly; they were 1.4%
of the commit, which is why that spec's performance claim was withdrawn.

The same parse runs at load (`decode.rs:799`), where it is a large part
of the 1.65 s `decode()` stage, and it is the reason `App::new`'s startup
walk (flaw P1) costs 5.22 s rather than the cost of the walk itself.

### What it is spent on

`render.rs` reads `line_styles[i]` for as many values of `i` as the pane
has rows — around 50. The document has 193,072 lines at load and over a
million after a commit. Better than 99.99% of the highlighting work is
discarded without ever being drawn.

The residency is the same story: `Vec<Vec<(Range, SyntaxRole)>>` is
193,072 inner `Vec`s, ~4.6 MB of `Vec` headers alone before any content,
and it is deep-cloned into and out of the render cache on every splice
(flaw P4).

### Why "colorize last, once" is not the fix

The natural first idea — since coloring is cosmetic, defer it until the
overrides have settled and do it once at the end — is already what
happens for a document-wide commit: one `colorize` call, at the end, on
the settled text. It is that one call that costs 13 s. Deferring does not
reduce the number of lines that go through tree-sitter; only *not
drawing* them does.

Nor is batching cheaper per line. Measured directly, on real rendered
textproto (`prototext-core/fixtures/descriptor_protoc.txt`, 3,062 lines),
`--release`, pinned:

| input | per parse | per line |
|---|---|---|
| 24 lines | 137 µs | 5.71 µs |
| 50 lines | 273 µs | 5.46 µs |
| 100 lines | 526 µs | 5.26 µs |
| 200 lines | 1.04 ms | 5.22 µs |
| 3,062 lines (whole file) | 17.9 ms | 5.85 µs |

**The rate is flat across two orders of magnitude of input size.**
tree-sitter's per-parse setup is negligible at these sizes; the cost is
essentially linear in lines parsed. The same holds at the top end: the
in-situ commit measurements are 12.2 µs/line for one 1,067,034-line parse
against 5.4 µs/line across 465 smaller parses — the big parse is the
*worse* rate, not the better one.

This single fact settles three design questions at once:

1. There is no batching advantage to preserve, so splitting the work up
   costs nothing.
2. There is no per-parse overhead to amortize, so a viewport-sized parse
   is genuinely cheap in absolute terms (273 µs at 50 rows).
3. Any scheme that still parses the whole document — including doing it
   on a background thread — still pays the whole document's cost. Moving
   it off the main thread changes who waits, not how much work there is.

The only lever that changes the order of magnitude is the number of lines
parsed. That is what this spec is.

## Goals

- **G1.** The highlighter runs over the rows currently on screen, and
  nothing else. Cost per frame is O(pane height), independent of document
  size.
- **G2.** Per-line style hints stop existing as a document-sized
  structure. `App::line_styles`, `Decoded::style_hints`,
  `RenderedAs::line_styles`, `PreviewOverlay::line_styles` and the
  `Vec<StyleHint>` third element of `RenderCache`'s value are all deleted,
  along with every pass that builds, splices, merges or clones them.
- **G3.** Hiding a line's trailing `#@ ...` annotation (spec 0133 G4)
  stops depending on the highlighter, and starts using the prototext
  format's own definition of where an annotation begins.
- **G4.** What the user sees is unchanged. In particular the *known*
  context effect — that a line highlighted outside its enclosing message
  is not highlighted the same way — is fixed rather than generalized, and
  flaw D4's two "highlight one line in isolation" workarounds disappear
  with it.

## Non-goals

- **N1.** Making `colorize::colorize` itself faster, changing the
  grammar, or changing `queries/highlights.scm`. The parse stays exactly
  as it is; only its input shrinks.
- **N2.** Highlighting on a background thread. Considered at length and
  deliberately not the first move — see `Alternatives considered` A1 for
  the whole-document form (rejected, with reasons) and S6's Escalation 2
  for the bounded-band form (kept as a measured fallback). Both are
  strictly easier *after* this spec than before it, because after S3 the
  unit of work is one window rather than a document.
- **N3.** The other whole-document structures — `lines`, `tree`,
  `line_to_node`, `visible_rows`. Those are roadmap item S8 ("the
  document is not materialized"). This spec is the smaller, separable
  half of the same assumption and is a rehearsal for it.
- **N4.** The render cache's remaining two elements (`lines`, `spans`),
  its key, or its byte budget beyond dropping the hints term from
  `render_bytes`.
- **N5.** Flaw P1's startup walk. Removing the whole-document parse makes
  that walk much cheaper but does not remove it; that is its own fix.

## Specification

### S1. The annotation boundary comes from the format, not from the parser

`annotation_start_in` (`render.rs:12`) finds a line's trailing
`#@ ...` annotation by looking for the first `SyntaxRole::Comment` span
in that line's hints. This is the only consumer of highlighting that is
not a draw, and it is the one that blocks laziness: `selected_text`
(`mouse.rs:238`, the clipboard copy) calls `render_line_content` over an
arbitrary selected range, which may be far outside the viewport.

It is also a second, weaker definition of a boundary the format already
defines. `prototext-core`'s encoder has the authoritative one:
`split_at_annotation` (`encode_text/mod.rs:39-68`) scans right-to-left
with `memrchr` for the exact `"  #@ "` separator and falls back leftward
past a bare `#` inside a string value. That is the same function that
decides where the annotation starts when text is encoded *back* to
binary, so protolens agreeing with it is a correctness improvement
independent of performance.

Expose it — as a narrow, additive helper on `prototext_core`, e.g.

```rust
/// Byte offset in `line` at which its trailing `  #@ ...` annotation
/// starts, or `None` if it has none. The inverse view of
/// `split_at_annotation`'s first return value, exposed for callers that
/// need to *hide* an annotation rather than parse it.
pub fn annotation_start(line: &str) -> Option<usize>;
```

and have `row_content`/`row_spans` call it instead of
`annotation_start_in`. Note the small semantic difference to preserve:
`split_at_annotation` ends the field part at `p - 2` (before the two
separator spaces), where `annotation_start_in` returns `p` and the caller
compensates with `.trim_end()`. Either convention is fine as long as the
truncation point is unchanged; state which one the helper returns in its
doc comment and drop the now-redundant `trim_end` if the helper already
excludes the separator.

**`split_at_annotation` has two branches, and both must be carried over**
(resolved open question Q1). The second one is easy to miss and is
reachable in practice:

1. Rightmost `"  #@ "` separator: the field part ends at `p - 2`.
2. A line whose content *before* the `#` is entirely whitespace: the
   whole line is annotation, indentation included — it returns `("", …)`.

`annotation_start_in` has no equivalent of (2). Being tree-sitter-based
it reports the comment token's own column, so hiding the annotation
leaves the leading indentation behind as a row of blanks. Branch (2) is
not hypothetical: `render_text/packed.rs:327-344` emits exactly that
shape for an **empty packed record** (a `push_indent` followed by a
comment-only annotation), and the `#@ prototext: protoc` header is the
same shape.

`annotation_start` therefore returns `Some(0)` for a comment-only line,
matching the format's own rule, and such a line renders as empty rather
than as whitespace when annotations are hidden. This is a deliberate,
named exception to G4: protolens is made faithful to what `prototext`
itself considers annotation rather than to its own previous
approximation of it.

**Scope note.** This adds one public function to `prototext-core`. It is
a pure query over a `&str`, adds no state, and extends neither the
rendered grammar nor the encoder's accepted input — the two things
`prototext-core`'s scope discipline actually guards. If that is judged
too much, the fallback is a ~20-line copy in `protolens/src/colorize.rs`
plus a test asserting the two agree on a shared fixture; but a copy of a
format rule is exactly the kind of second definition that drifts, so the
shared helper is preferred.

### S2. The window's text is assembled before it is highlighted

`render()` already builds `window: Vec<DisplayRow>` (`render.rs:401`) and
already runs one `&mut self` pass over it before the immutable-borrow
draw closure — the heat-cue pass at `render.rs:423`, restructured that
way by spec 0154 G6 for the same borrow reason. Highlighting takes the
same shape and the same position, immediately after `window` is built.

Add:

```rust
/// The rows currently on screen, as the highlighter sees them: the
/// committed line or overlay line each `DisplayRow` draws, untruncated
/// and unfolded (fold markers and the annotation transform are display
/// insertions applied downstream, and must not reach the parser).
///
/// Rows that are not prototext at all are emitted as `""` — see the
/// non-grammar-line rule below. The result is always index-parallel
/// with `window`.
fn window_text(&self, window: &[DisplayRow]) -> Vec<String>;
```

built from `display_row_source`'s existing two arms, minus the hints
element.

#### Lines that are not prototext are blanked, not parsed

`lines` is not purely prototext. `insert_truncation_marker`
(`override_apply.rs:242-279`) writes a literal `...` row into a
truncated preview's lines, and `...` is not in the grammar. Today that
is harmless because of an invariant stated at `override_apply.rs:2560`:

> `...` is not part of the prototext grammar, and doing it after
> `colorize()` has run means the highlighter never has to parse it.

**This spec destroys that invariant.** Once highlighting happens at draw
time, the marker is in `lines` when the window is parsed. It is a syntax
error, and error recovery in this grammar swallows *following* siblings
— the effect the existing regression test
`bare_decimal_field_name_does_not_corrupt_sibling_captures` was written
for. So the marker would silently strip the color off the rows beneath
it, inside a truncated preview, which is exactly what is on screen while
the user is choosing a type.

The rule, therefore: **`window_text` emits `""` for any row that is not
grammatical prototext.** A blank line is valid, produces no captures,
and leaves the line count untouched — so `hints_by_line` still returns
exactly `window.len()` buckets and the row's bucket is empty by
construction. No index surgery and no re-insertion pass, which is why
this is preferable to omitting the row from the parser's input.

**Scope of the marker, precisely.** `insert_truncation_marker` has one
caller — `render_node_as`, under `if truncated` — and truncation is
preview-only (`confirmed_override_is_not_truncated` pins this). So a
`...` row appears only in an override-pane preview overlay, never in a
committed document. That bounds the blast radius but does not reduce the
need for the rule: the preview overlay *is* the thing on screen while
the user is comparing candidate types, which is the moment highlighting
is doing its most useful work.

The corollary is worth stating for whoever implements this: the blanking
only ever has to fire on `DisplayRow::Overlay` rows. Do not use that as
a shortcut — test it through `window_text` rather than through the
overlay arm — because the invariant being protected is "non-prototext
rows do not reach the parser", not "overlay rows are special".

At present the marker is the only such row. It is called out here rather
than special-cased at the call site so that any future non-grammar
insertion has an obvious place to be handled — the same reasoning that
put fold markers and the annotation transform downstream of the parser
in the first place.

This also supersedes S4's disposal of `insert_truncation_marker`'s
`styles` parameter: dropping the parameter is right, but the sentence
"the parser simply sees it" is not — the parser must specifically *not*
see it.

### S3. The window is parsed inside a synthetic enclosing context

Parsing the window's lines on their own is **not** equivalent to parsing
them inside the document, and the difference is not marginal. A window
scrolled into the middle of a document typically begins on a line like
`  name: "x"` — which happens to parse as a valid top-level field — and
typically contains a bare `}` with no matching `{`. That drives
tree-sitter into error recovery, and this repo already has a regression
test showing error recovery *swallows following siblings*
(`colorize.rs::bare_decimal_field_name_does_not_corrupt_sibling_captures`),
losing their captures entirely. Highlighting would visibly degrade
whenever the user is not scrolled to the top.

This is the same effect flaw D4 already documents for the two existing
"highlight this one line by itself" special cases. S6 in the scaling
roadmap claims `hints_by_line`'s newline-clipping makes per-line
highlighting sound; that claim is **wrong as stated** and this spec
supersedes it. Clipping guarantees the *output* never crosses a line; it
says nothing about the *parse* seeing the right context. The output shape
and the parse context are independent, and only the second one is at
issue here.

The fix is cheap because the rendering is deterministic in indentation: a
line at nesting level `k` is indented by exactly `k * indent_size`
spaces. Verified (resolved open question Q2): every line `render_text`
emits takes its prefix from `push_indent`, which writes exactly
`INDENT_SIZE * LEVEL` spaces — `wfl_prefix`, `wfl_prefix_n`,
`wob_prefix_n`, `write_close_brace`, and packed's empty-record line are
the complete set of writers. There are no continuation lines and no
wrapped values. The one un-indented line is the `#@ prototext: protoc`
header, which is only ever line 0, where `d0` is 0 regardless.

Also verified for step 2: `render_text` emits `{` and `}` as its only
message delimiters. The grammar additionally captures `<`/`>` and
multi-line `[...]` lists, so the brace-delta rule below is *not* a
general prototext rule — it is complete for what this renderer produces,
and would need extending if the renderer ever emitted the others.

So:

1. `d0` = (leading spaces of the window's first line) / `indent_size`,
   plus 1 if that line's first non-blank character is `}`.
2. `dn` = `d0` plus the net brace delta across the window's lines (a
   line whose trimmed end is `{` opens; a line whose trimmed content is
   `}` closes).
3. Prepend `d0` synthetic opener lines (`_ {`, `  _ {`, ... at
   increasing indentation) and append `dn` synthetic closer lines.
4. `colorize` the joined result, `hints_by_line` it, then drop the first
   `d0` and last `dn` buckets.

The result is a syntactically complete document, so no error recovery is
entered on account of the window's position, and each visible line is
parsed at its true nesting depth.

Store the result in a new `App` field, index-parallel to `window`:

```rust
/// Spec 0187 S3: syntax hints for the rows drawn by the *current*
/// frame's `window`, in window order — recomputed each `render()`, never
/// retained across frames, never document-sized. Index `i` is
/// `window[i]`'s.
window_styles: Vec<LineStyles>,
```

`display_row_source` loses its hints element; `row_content`/`row_spans`
take the window index alongside the `DisplayRow` and read
`self.window_styles[i]`.

### S4. `line_styles` and everything that maintained it are deleted

Removed outright:

- `App::line_styles` (`mod.rs:643`) and `PreviewOverlay::line_styles`
  (`mod.rs:601`).
- `Decoded::style_hints` and the `colorize`/`hints_by_line` calls at
  `decode.rs:799`, which is the whole-document parse at load.
- `RenderedAs::line_styles` (`override_apply.rs:204`) and the
  `colorize`/`hints_by_line` calls in `render_node_as`
  (`override_apply.rs:2669`, `:2704`), which is the 85%.
- `RenderValue`'s third element in `render_cache.rs:37`, and the
  `hints.len() * size_of::<StyleHint>()` term in `render_bytes`.
- `materialize_line_patches`'s parallel `new_line_styles` merge
  (`override_apply.rs:1990-2049`) and the `styles` field of the patch
  type (`:2346`).
- `insert_truncation_marker`'s `styles: &mut Vec<LineStyles>` parameter
  (`override_apply.rs:389`). Spec 0174 S4's requirement that the `...`
  marker line carry no highlighting is now met by S2's non-grammar-line
  rule: `window_text` blanks the row, so its bucket is empty by
  construction and nothing needs to carry an empty style vector.
- The `override_select.rs:811` overlay construction's `line_styles`
  field.

`colorize::hints_by_line` survives — S3 still needs it to bucket one
window's hints per row. `colorize::colorize` survives unchanged (N1).

### S5. The two header re-highlights disappear (flaw D4)

Both sites in flaw D4 exist only to repair `line_styles` after patching a
line's *text*: `decode.rs:805-811` (patching `register_wrapper`'s `"_"`
placeholder, spec 0135 G2) and `override_apply.rs:2716-2721` (the same,
plus spec 0119 §G4's rename). Both re-run `colorize` on that single line
in isolation, which is precisely the unsound primitive S3 argues against.

Under S4 there is no `line_styles` to repair. Both blocks reduce to the
text patch alone, and the patched line is highlighted in its real context
when it is next drawn. Flaw D4 is closed by deletion, not by the
reordering its "Proposed correction" suggests.

### S6. Scrolling is measured, and no cache is added until it says so

The stated concern is a held-down PageDown: does highlighting at draw
time bound the scroll rate? The Background table answers it directly.

| pane height | cost per new window | at 30 repeats/s |
|---|---|---|
| 24 rows | 137 µs | 0.4% of a core |
| 50 rows | 273 µs | 0.8% of a core |
| 200 rows (full screen) | 1.04 ms | 3.1% of a core |

Key-repeat rates top out around 25-30/s, so even a full-screen terminal
spends ~3% of one core highlighting while the key is held. It cannot
bound the scroll rate.

The property that matters more than the absolute number: this cost is
**independent of document size**. Today a large document makes every
interaction slower; after S3, scrolling a ten-million-line document costs
exactly what scrolling a hundred-line one costs.

Therefore do **not** add a cache for `window_styles` in the first cut.
Add the measurement instead:

> **As implemented:** no cache was added — S6's actual directive holds.
> The measurement was not written: `protolens/src/tui/tests/profiling.rs`
> still has only the heat-cue workload, so test-plan items 9 and 10 are
> unexercised and "scrolling is unaffected" remains an argument from the
> Background table rather than a measurement of the shipped code. The
> gate below therefore cannot fire on its own; a future scroll-latency
> complaint is what would revive it.

- Extend the existing profiling harness
  (`protolens/src/tui/tests/profiling.rs`) with a scroll workload: load
  `/tmp/pdb.desc`, then drive N page-downs at a realistic pane height,
  reporting mean and worst-case `window_styles` time per frame *and*
  total frame time, so the highlighting share is visible rather than
  inferred.
- Gate: if worst-case highlighting exceeds **2 ms**, or exceeds 25% of
  total frame time, take one of the three escalations below. Try
  Escalation 3 first — it is the only one that stores nothing — then 1,
  then 2.

**Escalation 1 — memoize the window.** Cache keyed on
`(scroll_offset, pane_height, structural_version, overlay identity)`,
conservative, cleared on any mismatch. Fixes repeated redraws of an
unchanged window (resize, message timeout, cursor move within the
window); does nothing for a genuine page-down, where every window is new.

**Escalation 2 — pre-color a band, on a worker thread.** A worker
highlights a window-aligned band of a few screens either side of the
viewport and publishes results the draw path may use if present and
must recompute if absent. This is the *bounded* form of the asynchronous
proposal in `Alternatives considered`, and it is the one worth building:
it keeps residency proportional to the band rather than the document, it
has no staleness window (a missing band entry is recomputed inline, not
drawn wrong), and its unit of work is a window — which only exists as a
concept after S3.

**Escalation 3 — shed the color while input is still queued.** Color is
the one part of a frame the user cannot use while the view is moving. So
draw a frame in black and white when there is another *significant* input
event already waiting, and in color when there is not. A held-down
PageDown or a rolling mouse wheel then costs no highlighting at all, and
the frame the user actually stops on is colored.

Three properties make this the most attractive of the three escalations,
and it should be tried first if the gate fires:

- **It adds no invalidation obligation.** Nothing is stored, so nothing
  can go stale. Escalations 1 and 2 both buy their speed with a new cache
  to invalidate; this one buys it by not doing the work.
- **It is self-regulating.** At a slow key repeat the queue is empty
  between events, every frame is colored, and the mechanism costs one
  `poll(Duration::ZERO)`. At a fast repeat every frame is monochrome,
  which is exactly when color is useless. No threshold to tune.
- **It degrades in the right direction.** The failure mode is a
  momentarily uncolored frame, not a wrongly colored one — unlike
  Escalation 2, whose worst case is drawing a band computed against text
  that has since changed.

Four things to get right, none of them optional:

1. **The settled frame must be colored, guaranteed.** If the last event
   of a burst is handled and no further redraw occurs, the user releases
   the key and is left looking at a monochrome document. Whatever shape
   the loop takes, "queue became empty" must itself be able to cause one
   more frame. This is the whole feature; the rest is optimization.
2. **"Anything queued" is the wrong test.** With the Kitty keyboard
   protocol a single keystroke delivers Press *and* Release, so one
   isolated keypress leaves an event queued and would masquerade as a
   continuing flow — every frame would be monochrome forever. The test
   must be "is a *significant* event queued", using the **same**
   classification the key dispatcher already applies (the Release-event
   filter recorded in `protolens_tui_patterns`), not a second copy of it.
   A second definition of "significant" is how these two drift apart.
3. **Mouse `Moved` is noise; `Drag` is not.** Filtering bare motion out
   of the significance test is right — an uninterrupted pointer sweep is
   the same continuous flow as a wheel roll. `Drag` must stay
   significant, since a drag-selection is a user action with a result.
   Check first whether anything consumes `Moved` at all (hover cues); if
   nothing does, dropping it from dispatch entirely is a separate small
   cleanup, not part of this.
4. **It only exists after S3.** Today highlighting is precomputed
   document-wide, so a frame has no highlighting cost to shed. This
   escalation is meaningless before the parse moves into `render()`.

The idea generalizes to any per-frame work whose output is cosmetic —
heat cues are the obvious second candidate — but widen it only on the
same evidence, per-item.

The point of the gate is that Escalations 1 and 2 are new invalidation
obligations, and this codebase's history is that new invalidation
obligations are where the bugs are (spec 0186's own G3 found a
pre-existing one on its first run). They should be paid for by a number,
not assumed. Escalation 3 is the exception that proves the rule: it is
cheaper precisely because it stores nothing.

## Alternatives considered

### A1. Color the whole document in memory, asynchronously, on a fourth thread

This is the most attractive-sounding alternative and deserves a full
accounting, because it is not obviously wrong — it directly addresses the
one thing this spec does *not* do, which is guarantee that a drawn row's
color is already computed.

**The shape.** Keep `line_styles` as a document-sized structure. After
each decode and each commit, hand the settled text to a worker thread
that colors it and publishes the result; the draw path uses whatever is
published and falls back to no color for lines not yet done. Splices
invalidate and re-enqueue.

**Why it is not the first move.**

1. **It does not reduce the work.** The flat rate in Background is the
   decisive measurement: coloring a million lines costs a million lines'
   worth of parsing whether it happens on the main thread or a worker —
   roughly 5.5 s of a fully-occupied core at the measured 5.2 µs/line,
   and 13 s on the denser lines of the real fixture. It is spent to
   produce data of which the user will look at fifty rows. It also has
   to be spent **again after every commit**, since a commit rewrites the
   text. Moving it off the main thread changes who waits, not the amount
   of work, and the machine is already running the root-type scoring
   thread.
2. **It keeps the residency this spec exists to delete.** G2 and flaw P4
   are half the motivation: 193,072 inner `Vec`s (~4.6 MB of headers
   before content) at load, growing past a million after a commit, deep-
   cloned into and out of the render cache on every splice. A worker
   populates that structure rather than removing it, and adds
   cross-thread ownership of it on top.
3. **It keeps — and worsens — the invalidation obligation.** Today
   `line_styles` is spliced synchronously in lockstep with `lines`, which
   is at least simple to reason about. Under a worker, a splice must
   invalidate a range *and* cancel or supersede in-flight work for that
   range, and the draw path must tolerate a result computed against text
   that has since changed. That is a strictly harder correctness problem
   than the one this spec removes.
4. **It has a visible failure mode this spec does not have.** The
   document appears uncolored and then repaints under the user. On a
   commit this is a multi-second uncolored window, exactly at the moment
   the user is looking to see whether their override was right.

**Where it does win**, and this is real: it makes a drawn row's color
*already available*, so a pathologically tall terminal or a
pathologically slow grammar could never affect scroll latency. That
benefit is worth having — but it is obtainable without any of the four
costs above, by bounding the pre-colored region to a band around the
viewport instead of the whole document. That is S6's Escalation 2, and it
is deliberately specified there rather than rejected here.

**And it is easier after this spec than before it.** Handing a
million-line parse to a worker means shipping and synchronizing a
document-sized result. After S3 the unit of work is one window, which
makes a worker a plain request/response with a drop-stale-response rule
and no shared mutable document-sized state at all. If the gate fires,
this alternative is reached *through* the spec, not instead of it.

### A2. Highlight lazily per line, filling `Vec<Option<LineStyles>>` slots on demand

The scaling roadmap's own S6 shape. Rejected: it keeps a document-sized
structure alive (defeating G2 and flaw P4's half of the motivation), it
keeps the splice-time invalidation obligation, and its per-line parses
are exactly the unsound-context case S3 exists to avoid. Highlighting a
*window* is both cheaper to reason about and cheaper to run — and per
Background, a 24-line parse costs 5.71 µs/line against a 200-line parse's
5.22 µs/line, so there is no per-line advantage to recover either.

## Test plan

1. **Window highlighting matches document highlighting.** For a fixture
   with several nesting levels, compare, for every possible scroll
   offset: the hints S3 produces for the window, against
   `hints_by_line(colorize(whole document))` sliced to the same lines.
   They must be equal. This is the test that S3's synthetic context
   exists to pass, and it fails without it.
1b. **A truncation marker does not decolor the rows beneath it** (S2's
   non-grammar-line rule). Take a truncated override-pane preview whose
   `...` row has several highlightable rows after it *inside the same
   window*, and assert those rows' hints are identical to what they get
   in a window with no marker. Without the blanking this fails through
   tree-sitter's error recovery — silently, only in previews, and only
   below the marker: the exact shape of bug this codebase keeps finding
   late.
2. **The synthetic context is dropped, not drawn.** Assert
   `window_styles.len() == window.len()` and that no hint's range exceeds
   its line's length, for a window starting at a deeply nested line.
3. **Annotation hiding is unchanged.** With `annotations` off, assert
   `render_line_content` output is byte-identical to today's, over a
   fixture line whose *string value* contains a literal `#@` — the case
   `split_at_annotation`'s leftward fallback exists for, and the case
   where the tree-sitter route and the format route could have
   disagreed.
3b. **Annotation hiding on a comment-only line changes, deliberately**
   (S1 branch 2). Over an **empty packed record** — the reachable
   producer of that shape, per `render_text/packed.rs:327-344` — assert
   the row hides to `""` and not to a run of blanks. This is the one
   place the spec knowingly changes what the user sees, so it gets a
   test that fails if someone "fixes" it back.
3c. **The two annotation rules agree everywhere else.** Over a fixture
   covering both branches, assert `annotation_start(line)` and the
   deleted `annotation_start_in`'s truncation point pick the same byte
   offset for every line that is not comment-only. Pins that the
   crate-boundary move is behavior-preserving apart from the single
   documented exception.
4. **Clipboard copy works outside the viewport.** With `annotations`
   off, select a range entirely below the visible window and assert
   `selected_text` returns annotation-free lines. This is the case S1
   exists for; it would panic or return unstripped text without it.
5. **The preview and the commit still agree** (spec 0185 G3). Unchanged
   in intent, but it now exercises the shared path with no styles
   carried through the overlay: preview a candidate, capture the drawn
   spans, commit it, and assert the drawn spans are identical.
6. **The render cache still round-trips.** Update
   `render_cache_stores_style_hints` — it becomes obsolete and should be
   deleted, not weakened.
7. **Cost.** The commit workload in `profiling.rs` must show `colorize`
   absent from `render_overrides` entirely, and the total for the
   document-wide commit must drop by the 85% the table above attributes
   to it. Report the new total in this spec's `Measured outcome` section.
8. **Load.** `decode()` on `/tmp/pdb.desc` must lose its whole-document
   parse; report before/after.
9. **Scroll latency** (S6's gate). Drive N page-downs on `/tmp/pdb.desc`
   at 24-, 50- and 200-row pane heights; report mean and worst-case
   highlighting time per frame and as a share of total frame time. The
   pass condition is the gate in S6, and the numbers go into
   `Measured outcome` whether or not it fires — a claim that scrolling is
   unaffected has to be a measurement, not an argument.
10. **Scroll cost is document-size-independent.** Same workload on a
    small fixture and on `/tmp/pdb.desc`; per-frame highlighting time
    must be within noise of each other. This is the property that makes
    the design worth having, and it is the one a regression would break
    first.
11. **Only if Escalation 3 is taken: the settled frame is colored.** Feed
    a burst of page-downs with no gap, then stop, and assert the *last*
    frame drawn carries styles while the intermediate ones do not. Then
    feed one single keystroke and assert its frame is colored — that is
    the Kitty Press/Release trap in Escalation 3's point 2, and it is the
    case a naive "is anything queued" test fails. Both are cheap to write
    against a synthetic event queue and neither needs a pty.

## Open questions — all resolved

- **Q1 — resolved: shared helper in `prototext-core`, returning
  `Some(0)` for a comment-only line.** `annotation_start` adds no token,
  no grammar, no state and no encoder input, so it does not touch what
  `prototext-core`'s scope discipline guards. The substantive part of the
  question turned out not to be *where* it lives but *what it returns*:
  see S1's two-branch note. protolens becomes faithful to `prototext`'s
  own annotation rule, at the cost of one named G4 exception.
- **Q2 — resolved: the invariant holds.** `push_indent` is the sole
  source of leading whitespace in `render_text`, and there are no
  continuation or wrapped lines. Details and the `<`/`>` caveat are
  folded into S3. No `line_to_node`/`span.level` fallback is needed.

  The question did, however, surface something it did not ask about: the
  `...` truncation marker's *indentation* is fine (it is copied from a
  real line, so it is always a multiple of `indent_size`), but its
  *content* is not prototext at all. That is S2's new
  non-grammar-line rule, and it is the one correction this review made
  to the design rather than to its justification.
- **Q3 — resolved: no bleed.** `_ {` produces two captures —
  `(field_name) @attribute` on `_` and `(open_squiggly)
  @punctuation.bracket` on `{` — both single-line tokens wholly inside
  the opener line. `hints_by_line` (`colorize.rs:186-200`) buckets each
  hint by the line containing `hint.range.start` and clips `col_end` to
  that line's length, so a capture cannot reach a later bucket. Assert it
  in test 2 anyway, since it is the cheap guard on a rule that lives in
  a query file this spec does not own.
