<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0249 — a large document answers the user first

Status: draft (S4 already implemented — see below)
App: protolens
Refs: docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        immutable arena: the structure exists before any rendering
        does, which is the whole premise here),
      docs/specs/0097-raw-recursive-lendel.md (the
        message/string/bytes cascade for an unknown LEN payload — the
        verdict S4 caches),
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (a folded
        node is one row whatever is beneath it — the invariant this
        spec is built on),
      docs/specs/0222-the-text-lives-in-the-nodes.md (`node_text`: each
        slot owns its lines, and a bracketed node stores only its
        header because the footer is derived),
      docs/specs/0210-a-node-counts-its-own-lines.md (nodes store line
        *counts*, not positions — a subtree can change size without
        moving a coordinate anyone else holds),
      docs/specs/0171-wire-format-bounds-arithmetic-and-recursion-depth-caps.md
        §S4 (`at_depth_cap` — the existing "stop recursing here"
        check, and the precedent for S1's),
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
        (the preview *byte* budget — what S1 replaces, and why),
      docs/specs/0235-a-search-answers-while-it-is-still-being-typed.md
        (the sweep and the measured full-document numbers the haystack
        rule protects),
      docs/specs/0244-a-pan-may-run-past-either-end-of-the-content.md
        (`PaneScroll`'s signed `skip`, which is why the budget is
        `viewport_height + 1`),
      docs/specs/0247-a-fold-toggle-carries-the-worst-news-below-it.md
        (the status ladder S11 extends),
      docs/specs/0251-a-cached-render-is-read-not-copied.md (the render
        cache; S5's splice takes on hit),
      docs/specs/0207-where-the-override-memory-work-stands.md (records
        the render clone as the outstanding item)

## Background

Confirming a type override at the root of a large document freezes
protolens for about five seconds. This spec makes that frame immediate.

### The ≈5 s is the render, and invalidation is already scoped

`splice_override` (`tui/override_apply.rs`) renders **first**, on the
event thread (`:877`, `render_node_as`), and only then vacates slots —
the loop at `:894-905` clears `folded`, `tree`, `node_text`,
`heat_states` and status for `collect_descendants(idx)` alone.

So there is no scoped-invalidation scheme left to invent: invalidation
is already confined to the affected subtree, and it is cheap (a walk
writing `None`, milliseconds even at 4.7 M slots). There is a
synchronous, unbounded render to bound.

### One entry is many splices

`Origin` (`override_pane.rs:142-146`) has three kinds. `Path` names one
node; `PathField` matches that field under every node at the path — a
repeated field is many; `FqdnField` matches that field in *every*
message of the type, anywhere in the document. Each match is spliced
separately via `resettle_node`, so one confirmed entry can touch
thousands of scattered subtrees. Any design assuming "one subtree is
unbaked at a time" is wrong.

### Where the ≈5 s actually goes

Measured 2026-08-07 with throwaway timers inside `splice_override`,
`render_node_as` and `mark_fresh_subtree`, on `googleapis.desc`
(25.6 MB), opened `--raw` and then overridden at `/` to
`google.protobuf.FileDescriptorSet` via `export / --load-overrides`,
pinned with `flock -x … taskset -c 4-7`. One `resettle_node`, one
splice, one `render_overrides_inner` call.

| phase | time | size |
|---|---|---|
| `render_node_as` | **2.09 s** | |
| — `prototext-core` render | 1.06 s | 249.7 MB of text |
| — split into `String`s | 0.43 s | 5 278 324 lines |
| — `RenderCache::insert` clone | 0.56 s | (spec 0251's subject) |
| `collect_descendants` | 0.08 s | 2 864 189 nodes |
| vacate loop (:894-905) | 0.10 s | same |
| `overlay_spans` | 0.68 s | 5 278 324 lines, 4 499 336 spans |
| `refresh_line_counts` | 1.2 µs | O(depth) |
| `refresh_status_subtree`/`_ancestors` | **0.48 s** | |
| dropping the temporary line vector | 0.09 s | 5 278 324 `String`s |
| `mark_fresh_subtree` | 0.10 s | 2 829 366 fresh, 1 target |
| unattributed | ≈ 0.40 s | |
| **total** | **4.12 s** | |

Three things this settles.

**The renderer is a quarter of it.** `prototext-core` is 1.06 s of
4.12 s. Deferring only the render — the obvious reading of "make the
render async" — leaves 3 s on the event thread. What has to be bounded
is the whole splice.

**Everything else scales with the rendered document.** `overlay_spans`,
the status roll-up, the vacate loop, the split and the drop are one
pass each over 2.8–5.3 M items, 1.4 s together. That is exactly why
bounding the *rows* is cheap: it bounds the text, the line count, the
span count and the status roll-up in one move, so every row of the
table shrinks together.

**The root override is the typed startup, re-done.** Startup with
`-t google.protobuf.FileDescriptorSet` costs 5.35 s: arena 0.51 s,
render 1.10 s (249.7 MB, 4 499 336 spans), split 0.49 s (5 278 324
lines), `build_tree` 0.70 s, `App::new` 1.38 s. The same `--raw`
startup costs 2.89 s (103.5 MB, 3 668 694 lines, 2 864 190 spans). The
override reproduces the typed figures exactly, which is the expected
identity and a check on the numbers: the arena is not rebuilt (spec
0216), everything else is.

The ≈0.40 s unattributed sits inside `splice_override` between the
timed phases; no decision here turns on it. Spec 0207's "about 1
second", recorded 2026-07-31, is **superseded rather than reconciled** —
it does not say what it timed, and the corpus, the arena design and the
status roll-up have all changed since.

### The search is what the pre-baked lines are for

A full-document search miss on googleapis (5 281 124 lines) costs
**183–272 ms** folding and 276–422 ms case-sensitive (spec 0235), and
the sweep never lowercases the haystack — `memchr2` runs over the
needle's first character's two cases and folds only at the positions it
proposes (`tui/mod.rs:302-318`). There is no "we walked every line
anyway" credit to spend against re-rendering. The lines are baked once
and read many times; anything that makes the haystack more expensive to
obtain has to answer to those numbers.

### The arena already knows the structure

The arena is built from the bytes by a schema-free maximal wire walk,
before any rendering (spec 0216). It holds, for every node, where it
starts, where it ends and where its children begin. The structure of a
large node is therefore **not** something rendering has to discover;
rendering rediscovers what is already stored. That is what lets a
render be stopped early and still produce an exact structure.

One thing the arena's *shape* could not answer, because the walk
descends into every LEN payload whatever it looks like: whether spec
0097's cascade would render an unknown payload as a nested message or
as a string. See S4 — that is now cached, and is the only part of this
spec already implemented.

## Goals

- **G1. Confirming an override draws the next frame immediately**, at
  the viewport it was already at, however large the retyped node is.
- **G2. A drawn line is always a stored line.** Everything drawn comes
  from `node_text`, as it does today. Rendering is deferred; *reading*
  is not indirected.
- **G3. The pre-baked lines stay the search haystack**, so a
  full-document miss stays within 1 s on googleapis.
- **G4. Counts are exact at every moment.** `lines_total = 1 + Σ
  children + 1`, `assert_status_is_exact`, the scrollbar and row→node
  resolution hold continuously, never "once the bake lands".

## Non-goals

- **N1. `node_text` is not made lazy, and no line is rendered per
  frame.** Rendering *is* deferred — that is the whole spec — but the
  unit of deferral is a spliced region, never a line materialized for
  one frame. Memory is not the objective; latency is. See Alternatives.
- **N2. No progress bar over a synchronous commit.** The freeze it
  would annotate is what this spec removes.
- **N3. The search's matching semantics are unchanged** (specs 0235,
  0246) — but its *completeness* is not. S13 says so explicitly; this
  is a deliberate, visible, converging change, not an invariant kept.
- **N4. Nothing here retunes the sweep** (spec 0250) **or the render
  cache's budget** (spec 0251).

## Specification

The design in one sentence: **a bounded render is a full render in
which every node not descended into is folded.** A folded node is
already exactly one row whatever is beneath it (spec 0193), so the
bounded result is a complete, exactly-counted structure — not a
truncation, and not a provisional one.

Rendering stays on demand: confirming an override renders a screenful,
scrolling to a fold renders another, the rest bakes behind. What does
*not* happen is answering an individual line at draw time. Every
bounded render is **rendered → spliced → drawn**, in that order, so a
region's text, counts, spans and status are in place before anything
reads them. Hence G2, and hence no pair of rendering paths that must
agree byte for byte.

### The bounded render

- **S1. A render can be bounded by rows emitted. — IMPLEMENTED
  2026-08-07.** The budget is a count of rendered document lines,
  checked where the renderer is about to recurse. Once spent, a nested
  field is emitted with an empty body and reported as *undescended*.

  It is a **row** budget, not a byte budget (spec 0174) and not a depth
  cap (spec 0171 §S4): the cut then falls on a node boundary by
  construction, so the emitted lines are the unbounded render's lines,
  produced by the same renderer with the same annotations. Nothing has
  to be verified about their agreement.

  The precedent is in the same function: `render_len_field` already
  short-circuits recursion at the depth cap. A row budget is the same
  shape of check with a different degradation — an empty nested body
  rather than opaque bytes, because the node must stay foldable.

  **The walk stays depth-first, in document order.** This is a choice
  against the alternative of expanding level by level, and it is what
  makes the frame after `Enter` *final*: the rows on screen are the
  true first `viewport_height + 1` lines of the new rendering, so they
  will still be there, unchanged, when the bake completes. Overriding
  the googleapis root therefore shows the interior of `file[0]` at full
  depth, and folds `file[1..7770]` — not "every first-level child
  folded", which is what a level-by-level budget would give and which
  would visibly rearrange itself as the bake landed.

  **The budget is `viewport_height + 1`**, the `+1` being spec 0244's
  signed `skip`, which can leave the top and bottom rows partially
  visible. No further margin: the bake covers the whole document within
  seconds, so hedging against the next scroll step hedges against
  something that will be baked before the user reaches it.

- **S2. The output is `budget + right-frontier breadth`, and that is
  the price of G4.** Emission stops descending, not walking: the walk
  unwinds and still emits the siblings it had not reached, one folded
  row each, at every level. They are needed — the parent's
  `lines_total`, the scrollbar extent and "can the user scroll down"
  are all wrong otherwise — and they are cheap, because in document
  order they land *below* the viewport and each is `indent + key + " {"`
  with no payload touched. Closing braces cost nothing at all: a
  bracketed node stores only its header and the footer is derived (spec
  0222).

  At the googleapis root that is ~7 771 one-line entries to support a
  50-row screen, against 5.28 M rows unbounded.

  **Measured 2026-08-07** (`prototext-core/examples/bounded_render.rs`,
  googleapis.desc as its own `FileDescriptorSet`, `taskset -c 4-7`):

  | budget | wall | text | rows | spans | stops |
  |---|---|---|---|---|---|
  | 1 | 5.1 ms | 357 KB | 15 542 | 7 771 | 7 771 |
  | 51 | **4.9 ms** | 360 KB | 15 599 | 7 820 | 7 770 |
  | 101 | 5.2 ms | 362 KB | 15 650 | 7 863 | 7 770 |
  | 1 001 | 5.6 ms | 401 KB | 16 549 | 8 638 | 7 769 |
  | 10 001 | 8.5 ms | 739 KB | 25 612 | 15 985 | 7 804 |
  | none | **2.0 s** | 239 MB | 5 278 322 | 4 499 335 | 0 |

  Three corrections to what is written above.

  **The frontier is what a bounded render costs, not the budget.** From
  budget 1 to budget 101 the output moves by 108 rows and the wall time
  not at all: 15 540 of the ~15 600 rows are the unwind. A screenful is
  free on top of a fixed ~5 ms, so the budget could be far larger before
  anything noticed — which retires the worry behind S10's "overshooting
  by up to one screenful".

  **A frontier entry is one *stored* line but two *rendered* ones.**
  7 770 stops hold 15 540 rows: a header and a `}`. Both statements in
  S2 are right and they are about different things — `node_text` keeps
  the header alone and derives the footer (spec 0222), so the ~7 771
  figure is the storage and the drawn row, while the render text and
  `lines_total` see 2 each.

  **Every row of the ≈5 s table shrinks by 300-600x, together**, as
  claimed: text 239 MB → 360 KB (664x), spans 4 499 335 → 7 820 (575x),
  rows 5 278 322 → 15 599 (338x). The renderer itself is 2.0 s → 4.9 ms
  (410x).

  The depth-first rule is visible in the numbers: at budget 51 exactly
  one root child is missing from the stop list, at budget 101 that child
  is rendered whole and one node *inside the next file* is stopped
  instead. That is "the interior of `file[0]` at full depth, and folds
  `file[1..7770]`", not a level-by-level fan-out.

  **Stated limit: a wide-and-flat document cannot be bounded by
  folding.** A scalar child is not foldable, so a message with millions
  of direct scalar fields renders millions of rows whatever the budget.
  A packed run is one arena slot, which is why googleapis is safe; that
  is an observation about this corpus, not a proof.

- **S3. An undescended node is folded, and an auto-fold is not a user
  fold. — IMPLEMENTED 2026-08-07.** They are tracked in separate sets.
  `folded: HashSet<usize>` is the user's; a landing bake clears only
  its own, so folds the user made never pop open by themselves.

  The asymmetry is confined to the *writers*. Every read goes through
  `App::is_folded(idx)`, because on screen the two are the same row and
  every operation that acts on a folded node acts on both kinds — a
  user cannot be asked to know which set a fold came from. Unfolding
  likewise goes through `App::unfold(idx)`, which clears both with a
  non-short-circuiting `|`: a node can be in both sets (the user folds
  something a bounded render already stopped at), and leaving it in one
  of them would redraw it collapsed right after an unfold gesture.

  The set's invariant is "this node's body has not been rendered", so
  `splice_override` removes `idx` from it before overlaying — the
  render just above rendered that body. A bounded render is what puts
  it back (S5).

- **S4. The schema-free verdict comes from the arena, not from a
  probe. — IMPLEMENTED 2026-08-07.** To emit a parent's row for a
  child, the renderer must know whether the child is `field { … }`
  (foldable, one row when folded) or `field: "…"` (a scalar). With a
  schema the field declares it; without one, spec 0097's cascade
  decides, and deciding needs a structural probe of the *whole*
  payload — the exact recursive cost the budget exists to avoid.

  `Arena::probes_as_message(slot)` supplies it as a bit lookup. Two
  properties:

  - It is a function of the immutable bytes, so no override can
    invalidate it. Computed once, at load.
  - It is **reported by the one site that computes it** —
    `render_len_field` hands it to `Sink::begin_nested` on
    `NestedKind::Message { probed_as_message: Option<bool> }`, `None`
    meaning no probe ran because a schema decided. `ProbeSink` is
    untouched. This was not cosmetic: the first attempt re-derived the
    verdict from a malformity tally inside `ArenaSink` and disagreed
    with the real cascade on **49 050 of 855 344 nodes (5.7%)**,
    because it also answered for groups, which never face a cascade.

  Cost on googleapis.desc (25.6 MB, 4 737 283 slots, `taskset`-pinned):
  arena build **244–256 ms → 338–349 ms**, i.e. **+95 ms (+38%)**, plus
  ~590 KB for the bitset. 806 294 slots probe as a message. Verified
  slot by slot against a real `ProbeSink` over the whole corpus.

  This is the piece that makes a bounded render possible on an unknown
  document rather than only on a schema-covered one, which is why it
  was implemented first and independently.

### Confirming, invalidating, baking

- **S5. Confirming an override splices a row-bounded render and queues
  the unbounded one. — MECHANISM IMPLEMENTED 2026-08-07; no caller
  passes a budget yet.** The work stays synchronous on the event
  thread, in the order it has today — render, splice, draw — because at
  one screenful it is cheap enough to be. Every phase of the ≈5 s table
  shrinks with the row count.

  `splice_override` takes `row_budget: Option<usize>`, hands it to
  `render_node_as`, and folds every node the render reports stopping
  at. The translation from the render's span indices to arena slots
  happens inside `overlay_spans`, because that is where `slots` is
  derived and where the assertion that every span *has* a slot already
  runs — the report is validated by the same machinery as the spans it
  accompanies, rather than by a second copy of it.

  `row_budget` took the place of a **dead `is_preview` parameter**.
  Spec 0185 S3 made the live preview an overlay that calls
  `render_node_as` and stops, so no preview has reached
  `splice_override` since; every call site in the crate, production and
  test, passed `false`. The two budgets are asserted never to combine:
  a byte budget (spec 0174) cuts wherever the byte count runs out and
  needs the `...` marker to say so, while a row budget cuts on a node
  boundary and says so by folding. A row budget also never meets the
  render cache, which spec 0251 S5 confined to the preview path.

  What is left is the policy — which callers ask for a budget, and how
  big — because until the bake exists (S11) a bounded confirm would
  leave the document truncated with nothing to fill it back in.

- **S6. Invalidating a subtree keeps its header and drops its
  descendants.** This is the multi-site case, and it costs no render at
  all in the common one: **a node's own header line does not depend on
  its own type.** The key comes from the *parent's* descriptor and the
  annotations describe the parent-frame wire record (tag, length);
  changing the node's type changes its *children*. So an ordinary
  `--as <fqdn>` override keeps the existing header string, drops the
  descendants' text, and marks the node auto-folded — O(1) per site,
  no renderer call.

  Two cases do rewrite the header and must be bounded separately:
  `--field-name`, which replaces the key, and the `Any` /`MessageSet`
  expansion paths, which change how the node's own line reads.

- **S7. Invalidation stays scoped to the affected subtrees.** It
  already is (`collect_descendants`) and it must stay that way: a
  global invalidation would fold the whole document for a one-field
  override, and almost nothing would be searchable.

- **S8. Scrolling to an auto-fold expands it — the same operation as
  S5, later and smaller.** A fresh row budget for what just came into
  view, rendered, spliced, then drawn. Unfolding one by hand is the
  same call.

- **S9. Freshness is per slot; a generation guards only the
  write-back.** A per-slot fresh/stale bit (one bitset, ~0.6 MB at
  4.74 M slots) records what the last override invalidated. A global
  era counter exists solely so that a bake started in era E writes its
  result only if the era is still E — a second override landing
  mid-bake must not be overwritten by the first one's tail. The era
  does **not** invalidate globally (S7).

- **S10. The viewport re-anchors on the node, never on the line
  number.** When a bake lands and a subtree's real height replaces its
  folded one, the node the user is on keeps its screen row; absolute
  line numbers shift once, at that moment.

  This matters because of the multi-site property: a bake landing far
  above the viewport also changes the document's height. Anchoring on
  the line number would make the view jump every time a background bake
  completed somewhere the user cannot see.

  `PaneScroll.index` is a document *line* (spec 0244), so this is an
  explicit recomputation, not a change of type: hold the caret's slot
  across the splice, take its new absolute line, and `set_top` so that
  the slot lands on the terminal row it occupied before.

  **It runs on every splice batch, confirm included — not only when a
  bake lands.** For a single-site `Path` origin below the anchor,
  confirming moves nothing: S7's scoped invalidation leaves every row
  above untouched, and the node keeps the screen row it had. A
  `PathField` or `FqdnField` origin is the opposite case and is the one
  that governs: its sites are scattered document-wide, *including above
  the viewport*, and each off-screen site collapses from its real height
  to one folded row the moment the override is confirmed. The line count
  above the caret therefore changes at confirm, by a lot, and grows back
  as the bake lands.

  **So absolute line numbers are unstable for the duration of a bake,
  and that is accepted.** The alternative is the ≈5 s. What is *not*
  accepted is the caret or the viewport moving: they are anchored on
  slots and recomputed after every batch, so the user's row stays put
  while the numbers beside it change.

  A node that starts half way down the viewport still needs no special
  start point — which is why S1's budget is not reduced by the node's
  offset into the screen. Overshooting by up to one screenful is one
  screenful of cheap render; undershooting leaves a hole at the bottom.

- **S10b. A bounded render is issued only for a site that intersects the
  viewport.** A multi-site origin has no single "overridden node" to
  bound the budget around. At most a screenful of sites can be visible,
  so at most a screenful of bounded renders is issued at confirm; every
  other site takes S6's header-keep-and-auto-fold path and calls the
  renderer not at all. Confirm-time *render* work is bounded by the
  viewport, not by the site count.

  **Confirm-time invalidation work is not.** `collect_descendants` is
  O(subtree), and a whole-document `FqdnField` override's subtrees can
  partition the document — millions of slot writes and, worse, millions
  of `Box<str>` frees, on the event thread. This is allocator work the
  ≈5 s table never measured, because today the render dwarfed it. It is
  open question 6.

- **S10a. An anchor whose slot stops being rendered climbs to its
  nearest rendered ancestor.** The slot itself never disappears: the
  arena is immutable (spec 0216) and a splice allocates and frees no
  slots, so an index stays valid for the life of the document. What can
  disappear is the node's *rendering* — an `FqdnField` override can
  match a node's **parent** and flatten it to `bytes`, after which the
  child's bytes are inside an opaque scalar and it has no row, no
  `node_text`, and no line number to anchor to.

  The rule, for both the caret and the pane anchor: walk `parent[]`
  until a rendered slot is found. It is O(1) per step and terminates —
  the root is always rendered — over a chain that is 13 deep on
  googleapis against a cap of 1000. The user lands on the ancestor that
  swallowed the node, which is where the override they just made
  actually took effect.

  This is distinct from an auto-fold: a folded node *is* rendered, as
  one row, and is a perfectly good anchor.

- **S11. The bake is the same work, moved.** It renders each queued
  subtree unbounded, splices it under the S9 guard, and clears its
  auto-fold. It is off the event thread, interruptible between
  subtrees, and it writes `node_text` — which is storage — without
  populating the render cache (spec 0251 S9).

  **It draws no frame per subtree.** By S1's depth-first rule the
  visible rows after a confirm are already final, so a landing bake
  changes nothing on screen except the document's total height — the
  scrollbar thumb, the line-count footer and S13's remainder. One
  entry can be thousands of splices, so a frame per splice is
  thousands of frames redrawing the same rows, which is exactly what
  spec 0245 removed. The bake requests a redraw on a coalescing
  interval instead, plus one when it finishes.

  A user scrolling into unbaked territory meanwhile does not wait for
  the bake: they hit an auto-fold and S8 expands it on the spot.

### Saying what is not yet known

- **S12. The status ladder gains `Unbaked`, in violet.** It becomes
  `Ok < Unbaked < Unknown < NonCanonical < Invalid`, colored with
  `style_for(AnnotationLandmark)` — the same violet as `pack_size`'s
  length-prefix accent (spec 0225, amended 2026-08-06).

  The rank looks wrong and is right: an unbaked subtree might hide an
  `Invalid`, so ranking it just above `Ok` means a *known* bad sibling
  still wins the fold toggle's color, while "everything known is fine,
  something has not been looked at" reads violet — provisional. It
  never claims `Ok`. Without this, spec 0247's promise that a toggle
  carries the worst news below it is simply false over an auto-fold.

- **S13. A search reports what is not yet baked.** A folded region has
  no text, so a search during a bake sweeps the baked slots at today's
  speed and reports a remainder — the number of subtrees still queued.
  It is complete again once the bake catches up, and with S7 the
  remainder is bounded by the override, so a search anywhere else is
  unaffected.

  Two surfaces, deliberately: a **steady** violet dot while a bake is
  running, and the count in the search's own result line —
  `3 matches (412 subtrees not yet baked)`. The dot carries the ambient
  state; the result line carries the consequence, and it is where the
  user is actually looking when they care.

  Steady, not flashing: a blink is a timer-driven redraw every ~500 ms
  for the whole bake, which reopens spec 0245's rule that a frame is
  drawn only when something changed. Violet, not red: red is `Invalid`
  in spec 0247's ladder, and a bake in progress is not an error — the
  dot and the fold markers should agree at a glance.

## Open questions

1. **Where does the row budget live, and what does an undescended node
   emit? — ANSWERED 2026-08-07, implemented with S1.** Three decisions,
   each with its reason.

   **The budget rides on the sink.** `TextSink::line_count` already is
   the counter the budget compares against, so a thread-local beside
   `DEPTH` would be a second copy of a number the sink holds. The
   decisive argument is a different one: `ProbeSink` must *not* see the
   budget. A probe that stopped early would decide spec 0097's
   LEN-cascade differently, turning a presentational budget into a
   structural change in what the document is. On the trait the budget
   is two methods with defaults — `row_budget_spent` returning `false`
   and `note_undescended` ignoring the call — so `ProbeSink` and
   `ArenaSink` opt out by saying nothing, which is exactly the property
   wanted. A thread-local is ambient and would have had to be *masked*
   at every probe site instead.

   **An undescended node emits its `begin_nested`/`end_nested` pair
   with no body.** Not a distinct token, not an empty `{}` special
   case: the renderer's recursion sites are wrapped in a `descend`
   helper that skips the body and reports back, and everything on
   either side runs unchanged. So a sink that never asks for a budget
   cannot tell an undescended node from an empty one, and neither the
   arena build nor the probe needed a single line of change. The row
   the user sees is a folded header — spec 0193's one row whatever is
   beneath it — because S3's auto-fold set is what makes it fold, not
   the renderer.

   **The report is `IndexedRender::undescended: Vec<u32>`, span
   indices, not a `NodeSpan` field.** `NodeSpan` is exactly 32 bytes
   with no padding hole (spec 0212 S8) and protolens holds ~4.5 M of
   them in three places at once, so a `bool` would cost ~54 MB to carry
   a fact that is false for every unbounded render. The side vector is
   bounded by budget + frontier and is empty whenever `row_budget` is
   `None`. Span indices also mean the report is validated by the same
   `slots_for_spans` map the spans themselves go through.

   That map was the risk named here, and it is closed: **the silent
   drop is now an `assert!`** in `overlay_spans` (`decode.rs`). A span
   with no arena slot is a structural disagreement between the render
   and the maximal walk, and the overlay it produces is wrong wherever
   it lands, so it fails loudly. Verified against 761 protolens tests
   and a real 2.8 M-node googleapis root-override splice.

   **Stated limit: a group cannot be bounded.** A group has no length
   prefix, so its extent is knowable only by parsing through to
   `END_GROUP`. Skipping the walk would leave the cursor at the group's
   start and the parent would read the group's children as its own
   siblings — a *wrong* structure, not an unexpanded one. So
   `render_group_field` descends unconditionally, and a document whose
   rows are mostly inside one group is bounded only down to that
   group's size. This is the same reason `ProbeSink` recurses into
   groups despite `treat_len_as_opaque`. Groups are proto2-only and
   rare; no corpus document hits this.

2. **What do the two header-rewriting cases cost at scale?** S6 is O(1)
   per site for `--as <fqdn>`, but `--field-name` and the
   `Any`/`MessageSet` paths need one row rendered per site, each
   carrying the renderer's per-call setup (descriptor resolution,
   `install_ext_loader`). Measure setup against per-row separately: if
   setup dominates, the sites must be batched into one call. This is
   the same measurement that prices S5.

3. **What happens to a user fold inside a subtree that is baked?**
   Folds are keyed on slot and the slots survive (spec 0216), so the
   expectation is that they persist — but the invalidation path
   currently clears `folded` for the whole affected subtree
   (`override_apply.rs:894-905`). Decide whether that is a bug being
   preserved or a behavior worth keeping, and record it in S3.

4. **Where does the bake's cue live?** S13's dot is the "unobtrusive
   cue that a background job is running" that specs 0204/0205/0209
   claim as their subject. Either it lands there or this spec
   supersedes them — decide before it grows a second home.

5. **Is `node_text`'s shape worth changing while it is being
   reworked?** It is `Vec<Option<Box<str>>>` over ~4.74 M slots:
   ~76 MB of pointers and ~4.7 M individual heap allocations before any
   text. Flattening it into one arena `String` with per-slot `(u32,
   u32)` offsets plus a small overlay for spliced slots keeps every
   property here. Orthogonal, and should not be bundled — but if it is
   ever done, doing it while this spec is already rewriting every
   writer is the cheap moment.

6. **What does confirm-time invalidation cost for a whole-document
   `FqdnField` override?** S10b names it: dropping the descendants'
   text is O(subtree) per site, and a type that occurs everywhere makes
   that O(document) — millions of slot writes and millions of
   `Box<str>` frees, on the event thread, *after* the bounded render has
   made everything else fast. It is the existing vacate loop
   (`override_apply.rs:894-905`): `node_text[d] = None` drops one
   `Box<str>` per descendant, while the loop's other four writes are
   plain stores. Nobody has measured it, because until now the 4.12 s
   render hid it.

   **The likely answer is not to move the work but to delete it.** Once
   S9's stale bit is the authority on whether a slot's text is worth
   anything, `is_some()` no longer has to mean "fresh" — so confirm can
   flip bits and leave the strings where they are, and each old
   `Box<str>` is dropped by whoever overwrites the slot, on the bake
   thread, spread across the bake. Confirm-time frees go to zero, and
   the memory was resident anyway. Measure first, but expect this shape.
   The argument does not extend to `tree`/`heat_states`/`status`: those
   are allocation-free and can keep being cleared eagerly. Open question
   5's flattened `node_text` would dissolve it too, by making a drop an
   offset write.

## Alternatives considered

**Answer a stale line just-in-time.** An earlier draft of this spec: a
slot keeps its old text, and an accessor renders on demand whatever is
stale. It needs a dual-source accessor whose two paths must return
identical bytes, a per-call setup cost nobody has measured, and an
answer to "which node is row *r*" inside a region that was never
rendered. It cannot work where it is most needed anyway: the new
rendering has a different line count than the old one, so a per-row
answer would not correspond to the row the structure says is there.
Folding is exact by construction and needs none of it.

**Use the existing byte-budgeted preview render as the instant frame.**
Also an earlier draft, and attractive because the render already exists
and is already interactive (spec 0174, `is_preview: true`, driven per
keystroke by `preview_override_highlight`). Dropped because a *byte*
budget does not cut on a node boundary: the two renders need not agree
on the lines they share, so the view would change visibly when the bake
landed, and the property had to be established by test rather than by
construction. Its `...` marker is also not prototext and must be kept
away from the parser (spec 0187). A row budget gives the prefix
property for free.

**Synthesize an unbaked node's folded row at display time and emit
nothing.** Would remove S2's frontier tail. Rejected: it puts a second
source of drawn text back in — the thing G2 exists to prevent — to save
7 771 trivial rows.

**Make `node_text` lazy to save memory.** Rejected as N1. Memory is not
the complaint; the at-rest 0.94 GiB is affordable. Laziness costs the
two things that matter: the search would have to render its own
haystack (order seconds, missing G3 by 5-25x), and a lazy line still
has to be counted before the viewport can skip it.

**Defer the line counts as well as the text.** The general version of
S1: draw the viewport by rendering forward and leave the subtree's
height unknown until the bake lands. It gives progressive reveal, and
it costs a third state for every invariant currently phrased as "counts
are exact" (G4). The fold makes it unnecessary — an undescended node
counts as one row, exactly, today.

**Land the override entirely folded and bake behind the fold.** The
degenerate case of S1, budget zero. Counts stay exact for free, but it
shows the user *nothing* of what they just asked for. A budget of one
viewport shows them a screenful for the same structural cost, which is
why the budget is a number and not a flag.

**A global era number as the invalidation mechanism.** Appealing —
invalidation becomes O(1) — and rejected as the *invalidation* rule
(S7): bumping a global era folds the whole document for a one-field
override. Per-slot invalidation is not a new mechanism either:
`splice_override` already writes over exactly the affected slots. The
era survives in S9 for the one job a per-slot bit cannot do.

**Let the search read stale text rather than skip it.** Rejected. A
match would land on the right node with text nobody can see, and
highlight a column computed from a rendering that no longer exists.

**A progress bar over the existing synchronous commit.** The smaller
change, and seriously considered: 5 s is paid once and the document is
fast again afterward. Rejected because a progress bar makes a freeze
legible rather than absent (G1). Two facts also weaken "once":
`resettle_node` re-applies confirmed overrides, and one entry can be
thousands of splices.

## Test plan

1. `a_row_budgeted_render_is_the_start_of_the_full_one` — the bounded
   lines equal the corresponding lines of the unbounded render byte for
   byte, and the first line not emitted belongs to a node reported as
   undescended. S1. **Write it first**: it is the property everything
   else assumes, and it should hold by construction.
2. `an_undescended_node_is_reported_and_folded` — every undescended
   node lands in the auto-fold set and none of them in the user's. S1,
   S3.
3. `the_frontier_is_emitted_as_folded_rows` — for a node whose budget
   runs out inside its first child, every later sibling is present as
   one row, and `lines_total` matches what the viewport can scroll
   through. S2, G4.
4. `counts_stay_exact_with_auto_folds` — after a bounded splice
   `lines_total` is exact, `assert_status_is_exact` holds, and the
   scrollbar's extent matches the drawn document. G4.
5. `an_unknown_payload_is_bounded_without_a_probe` — a bounded render
   over a document with no schema produces the same shape as the
   unbounded one for the rows it emits, driven by
   `Arena::probes_as_message`. S4. *(The arena side already has tests:
   a declined payload, a defect that does not reach the parent, an
   unterminated group, and a whole-corpus cross-check against a real
   `ProbeSink`.)*
6. `a_confirmed_override_draws_before_it_bakes` — confirming on a
   root-sized node produces a frame without waiting for the unbounded
   render, and the overridden node keeps its viewport row. G1, S5, S10.
7. `an_ordinary_retype_renders_no_header` — an `--as <fqdn>` override
   over many sites issues zero renderer calls for the headers, and the
   headers drawn are byte-identical to the ones before. S6.
8. `a_renamed_field_rewrites_only_its_own_header` — the `--field-name`
   case does re-render, and only the header. S6.
9. `an_override_marks_only_its_own_subtree_stale` — the fresh/stale
   bitset differs only over `collect_descendants`. S7.
10. `a_stale_subtree_expands_when_it_scrolls_into_view` — and the rows
    it then occupies equal the unbounded render at that node. S8.
11. `a_bake_clears_auto_folds_and_keeps_user_folds` — S3, S11, and open
    question 3.
12. `a_second_override_wins_over_an_in_flight_bake` — a bake started
    before a second override does not write over the second override's
    slots. S9.
13. `an_auto_fold_reads_unbaked_not_ok` — the toggle's color is violet
    over an auto-fold whose children are all fine, and stays red when
    one of them is `Invalid`. S12.
14. `a_search_reports_the_unbaked_remainder` — a search during a bake
    returns the baked matches plus a non-zero remainder, and re-running
    it after the bake returns the full set. S13.
15. `the_viewport_holds_its_node_when_a_bake_lands` — including a bake
    landing above the viewport. S10.
16. `a_node_starting_mid_viewport_does_not_move` — confirm a `Path`
    override on a node whose header is half way down the screen; every
    row above it is byte-identical and the header stays on its terminal
    row. S10.
17. `a_multi_site_override_holds_the_caret_while_the_numbers_move` — an
    `FqdnField` origin with sites above the viewport: the caret's slot
    keeps its terminal row across confirm, and its absolute line number
    does change. S10. This is the case that `Path` does not exercise.
18. `only_visible_sites_are_rendered` — an `FqdnField` confirm issues
    renderer calls for the sites intersecting the viewport and no
    others, whatever the site count. S10b.
19. `an_anchor_swallowed_by_its_parent_climbs` — an `FqdnField`
    override that flattens the caret's *parent* to `bytes` leaves the
    caret on that parent, with a drawn row, and the pane anchored on
    it. S10a.

## Measured outcome

Filled in at implementation. It must include: time from `Enter` on a
root override to the next frame, before and after, **with the same
phase breakdown as the 4.12 s table**, so it is visible that every row
shrank together; the renderer's per-call setup cost against its per-row
cost, and the total for a `--field-name` override at its real site
count (open question 2); the cost of expanding one auto-fold; the
unbounded bake's wall time and whether the UI stays responsive across
it; a search run during a bake, reporting both the swept time and the
size of the remainder; and a re-run of the full-document search
confirming G3 against 183–272 ms. State plainly anything that did not
improve.

`Arena::probes_as_message` (S4) is already measured above: +95 ms on
the arena build, ~590 KB, 806 294 of 4 737 283 slots.
