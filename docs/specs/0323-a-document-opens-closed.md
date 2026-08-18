<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0323 — a document opens closed

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0249-… (the row budget, the two fold sets, and why
        `auto_folded` is separate from `folded`), docs/specs/0255-…
        and docs/specs/0257-… (the bake and bounded startup),
        docs/specs/0210-… and docs/specs/0254-… (`lines_visible` and
        `refresh_line_counts`), docs/specs/0216-… (the immutable
        level-order arena), docs/specs/0256-… (`HashSet::retain` is
        O(capacity)), docs/specs/0260-… (the violet margin reads
        `auto_folded` alone), docs/specs/0193-… (a folded node is one
        row)

## Background

Two problems. The second is what makes the first affordable, and each
would be worth fixing without the other.

### A document opens fully expanded

The first thing a reader does with an unfamiliar document is press `Z`
on the root and then `z`: collapse everything, then open the top level
so the shape is visible and nothing else is. protolens makes them ask
for it every time. `script_pane.rs`'s `Fold::All` exists because scripts
want the same thing.

`Z` then `z` is also the *specification* of the wanted state, and it is
not the same as "fold the root's children": unfolding one of those
children must reveal its own children **still folded**, one level per
gesture.

### The fold state is two hash sets, and four places already work around them

All of this is measured and already in the tree:

- **`node_status.rs:85`.** `rebuild_status` allocates a bitset and fills
  it from `auto_folded` *on every call*, because probing the set once
  per slot measured **5.0% of a startup** on googleapis. A faster hasher
  was tried and came out **3.3% slower on 3.7% fewer instructions** —
  the probe is the cost, not the hash.
- **`override_apply.rs:177`.** `scrub_folds_under` chooses between two
  algorithms at run time by comparing `HashSet::capacity()` against the
  descendant count, because `retain` is O(capacity) and `auto_folded`'s
  capacity peaks near **84 000 mid-bake without shrinking**. Retaining
  unconditionally took a bake drain from **5.4 s to 17.5 s**.
- **`mod.rs:1539`.** `bake_queue` exists at all because
  `HashSet::iter().next()` scans buckets from the top, so draining
  ~84 000 entries would make the last steps each scan the whole table.
- **`search_cursor.rs:306` and `script_pane.rs:231`** collect the set
  into a `Vec` and sort it to get slots in order.

So the sparsity the sets were chosen for is already false at 84 000
entries. Folding by default removes it outright: as the bake completes,
every *bracketed* slot in the arena is folded — measured at bake idle on
googleapis, **778 987** of 4 936 532 slots, 15.8%. That is a hash table
of roughly **9 MB**, grown by rehashing, to hold a fact that fits in
617 KB of bits.

An earlier draft of this paragraph said 150 MB, by assuming every arena
slot was bracketed rather than a sixth of them. The corrected figure is
small enough to be invisible in an 840 MB process, and the measurement
below confirms peak RSS does not move. **The memory saving is not what
justifies G3.** What justifies it is the CPU already measured above and
unaffected by this correction: `rebuild_status`' 5.0% of a startup and
`scrub_folds_under`'s 5.4 s → 17.5 s drain are costs of *probing and
retaining a hash table*, not of its size, and they are what the four
workarounds exist to dodge.

## Goals

- **G1.** protolens opens a document in the state `Z` on the root
  followed by `z` produces: the root open, everything below it closed,
  and unfolding a node reveals its children still closed.
- **G2.** Reaching that state adds no pass over the arena and no work to
  the bake. The initial `lines_visible` is *computed*, not repaired.
- **G3.** Fold state is one bit per arena slot, allocated once per
  document.
- **G4.** `folded` and `auto_folded` stay fully independent. Fold-by-
  default must not be expressed by writing `auto_folded`, and must not
  change what the violet margin or the `Unbaked` status rung mean.

## Non-goals

- **N1.** A setting or flag to open expanded instead. `Z` on the root is
  one keystroke and already does it; a preference would need a home,
  and there is nowhere it would be remembered.

- **N2.** Making the bake conditional on anything being unfolded. The
  bake is what makes search, export and a later unfold complete and
  instant; that a row is not currently drawn is not a reason to leave
  its bytes undecoded. Startup work is *reduced* by this spec anyway,
  because the row budget now stops at the root's children. Measured:
  true of the main thread (7.54 s against 7.58 s of CPU), which is the
  thread this claim is about. The heat workers' read-ahead bill goes the
  other way, by 4.2× — see the Measured outcome for why that is
  `Tier::Prefetch` work and is left alone.

- **N3.** Replacing `bake_queue` or `visible_stops`. A bitset answers
  "is this slot set", not "which is next"; the deques were added for the
  second question and it does not go away.

- **N4.** Shrinking `TreeNode`'s fold state into the slot itself.
  `TreeNode` is `NodeSpan` (32 B) plus three `u32` = **exactly 44 bytes
  with no padding**, pinned by `const _: () = assert!(size_of::<TreeNode>()
  == 44)`. Two `bool`s round it to 48, which is +19 MB across 4.74 M
  slots against 1.2 MB for two side bitsets — and `NodeSpan`'s spare
  bits belong to prototext-core, not to protolens's fold state.

- **N5.** A `folded_by_default` inversion, where `folded` records the
  *exceptions*. It is O(1) at startup, but every read and write site has
  to learn the inversion, `lines_visible` still has to be computed, and
  the saving over S2 is one branch in a loop that already runs.

## Specification

### The representation

- **S1.** `FoldSet` (new, `protolens/src/fold_set.rs`) is a
  `Box<[u64]>` of `slots.div_ceil(64)` words plus a `len: usize`
  maintained by `insert`/`remove`, so `len()` and `is_empty()` stay O(1)
  — `search.rs`'s "N subtrees not yet baked" and the bake's idle check
  both need them. It exposes `contains`, `insert`, `remove`, `len`,
  `is_empty`, `iter()` (ascending, skipping zero words) and `word(i)`.
  `App::folded` and `App::auto_folded` become `FoldSet`s sized from
  `tree.len()` in `App::new`; the arena is immutable (spec 0216), so
  that is one allocation each per document.

  Four workarounds go with it:
  `rebuild_status` reads `self.auto_folded.word(idx / 64)` directly and
  its per-call `vec![0u64; …]` and fill loop are deleted;
  `scrub_folds_under` loses its capacity heuristic and its
  `mem::take`/`retain` dance and becomes the unconditional
  `for &d in descendants { self.unfold(d); }`;
  `script_reset_folds` loses its `sort_unstable`, because `iter()` is
  already in slot order.
  `search_segments` keeps its sort — it needs `raw_start` order, and
  level order is not pre-order.

### The default

- **S2.** `overlay_spans` is the single writer of every arena slot
  (`build_tree` delegates to it), and it already branches on
  `span.is_message`. In that branch a bracketed node is now written
  `lines_visible: 1` rather than `lines_visible: line_count`, and its
  slot's bit is set in a `FoldSet` reached through the new
  `decode::Overlay` — the three parallel arrays `overlay_spans` writes,
  borrowed together rather than passed as three adjacent unnamed `&mut`
  parameters. Borrows and not an owning struct because the splice path
  holds each of the three somewhere different: two behind
  `Arc::get_mut` on fields of `App`, the third a field of `App` itself.
  `build_tree`'s return is likewise the named `decode::BuiltTree`
  instead of a 4-tuple, since three of its four members are `Vec`s and
  positionally interchangeable to the compiler.

  This is G2: the folded `lines_visible` is the value first written, so
  no node is ever refreshed to reach it.

- **S3.** `App::new` then opens the root — `folded.remove(root)` and one
  `refresh_line_counts(root)`, which sums the root's direct children and
  is O(root fan-out). That is the whole of the startup cost.

- **S4.** The rule is uniform: **a body no reader has asked to see is
  closed.** Because `overlay_spans` is also every splice's writer, this
  applies to a subtree the bake reveals (required by G1 — otherwise
  opening a stop dumps a screenful expanded) and equally to a subtree an
  override produces.

  That uniformity has a consequence worth stating, because it is the one
  thing about this spec that cannot be read off any single function:
  **`splice_override` is not a gesture, and the two things that call it
  are.** `resettle_node` reaches it from a commit, and
  `expand_auto_fold` reaches it from the bake — the first must open the
  node it retyped, the second must open nothing at all. So the policy
  cannot live in `splice_override`, and it does not:

  - `splice_override` is **neutral**. It reads `idx`'s bit before the
    call and puts it back afterwards
    (`override_apply.rs:1183`/`:1237`), because S2 has just folded `idx`
    along with every other bracketed slot the render wrote, and a splice
    on its own is not permission to change what the reader was looking
    at.
  - `App::open` clears the bit **before** calling in
    (`navigation.rs:436`), so the neutral restore reads `false` and the
    revealed stop stays open.
  - `resettle_node` clears it **after**, in the `Ok(())` arm
    (`override_apply.rs:122`), so a commit opens the node it retyped:
    the reader has just watched the preview draw that body and should
    not have to press `z` to see it again.
  - the bake clears nothing.

  The result the reader sees is the same everywhere — the node they
  asked about open, its children collapsed, one level per gesture.

- **S5.** Nothing about `auto_folded` changes (G4). A stop is now
  normally in *both* sets — it is both "not rendered yet" and "not asked
  for" — which `App::open` already handles explicitly, and the violet
  margin and the `Unbaked` rung keep reading `auto_folded` alone, so
  they still mean exactly "nobody has looked inside".

- **S6.** An override **preview** draws the candidate unfolded; a
  **commit** draws whatever the bits say. Nothing else is a special
  case, and the preview half needs no code: `PreviewOverlay` holds
  `rendered.lines`, a line vector `render_node_as` produces outside the
  arena, so it has no fold state to consult and cannot acquire one. That
  is the point of previewing — the reader is deciding whether this type
  is right, which is a question about the content, and a collapsed
  `{ ... }` answers nothing. On commit the content joins the document
  and S4 governs it like any other body. The item exists so the
  asymmetry is not later "fixed" in either direction.

## Alternatives considered

**Folding only the root's direct children.** Nearly free: because
`refresh_line_counts` forces `d_visible` to 0 above a folded ancestor,
and `is_folded` is consulted only for rows that are drawn, marking the
root's children alone produces the right *first frame* at O(fan-out).
Rejected because it is not the requested state: unfolding a child then
reveals its whole subtree at once, and the state the reader asks for
with `Z` then `z` discloses one level at a time.

**Setting the bits in a separate pass over the arena at startup.** A
sequential pass over 4.74 M `TreeNode`s is 208 MB of streaming for a
value the writing loop already has in a register. It also gets the
answer wrong: under bounded startup only ~7 798 slots are rendered, and
the rest have to be folded as the bake writes them, which only S2's
placement achieves.

**Keeping the sets and accepting the growth.** The measurements in the
Background are what a bake already costs *before* every bracketed slot
joins `folded`. The 17.5 s figure is the shape of the failure.

## Test plan

1. `a_document_opens_closed` — after `App::new` on a multi-level
   fixture, the root is drawn open, every child is drawn as `{ ... }`,
   and the visible row count equals `2 + child count`. G1, S2, S3.
2. `unfolding_reveals_one_level` — `z` on a child shows *its* children,
   each still collapsed. G1, S4.
3. `startup_matches_z_then_z` — the frame `App::new` produces is
   identical to the frame produced by opening expanded and then pressing
   `Z` and `z` on the root. This is the spec's definition, asserted
   directly.
4. `a_baked_subtree_arrives_folded` — drain the bake on a row-budgeted
   fixture; every newly rendered bracketed node is folded and
   `assert_line_counts_are_exact` holds throughout. S4, G2.
5. `an_override_subtree_arrives_folded` — the retyped node keeps its own
   fold state, its new children are collapsed. S4.
6. `a_preview_draws_the_candidate_unfolded` — the overlay for a
   candidate whose subtree is several levels deep shows those levels,
   and committing the same candidate collapses them. S6.
7. `the_violet_margin_still_means_unread` — a stop in both sets draws
   violet, and a user fold over a baked node does not. G4, S5.
8. `fold_set_round_trips` — unit tests for `FoldSet`: `len` after
   repeated `insert`/`remove` of the same slot, `iter()` ascending and
   word-skipping, the last partial word.
9. The existing `assert_line_counts_are_exact` and
   `assert_status_is_exact` are unchanged and must keep passing; they
   are what pins S2's initializer against `refresh_line_counts`.

## Measured outcome

All on `googleapis.desc` at scoring-graph v6, in a 50×200 pty pinned to
`taskset -c 0-7`. Convergence is **CPU quiet**, not a trace marker: spec
0263 made an idle protolens do zero timed wakeups, so once the bake and
the read-ahead are done the process stops accumulating `utime+stime`
entirely. The figure reported is the wall clock to the last CPU
movement, after which the total has not moved for 3 s. Three runs each.

### It costs 4.2× the heat-worker CPU, and that cost is not this spec's

| build | fold state | converged | CPU | peak RSS |
|---|---|---|---|---|
| before this spec | opens expanded | 12.4–13.2 s | 29.7–31.2 s | 802–836 MB |
| before this spec, then `Z` `z` | folded by hand | 19.0–19.3 s | 78.1–79.3 s | 814–851 MB |
| this spec | opens folded | 18.7–18.9 s | 77.4–78.4 s | 812–850 MB |

**The middle row is the finding.** Driving the *old* binary into this
spec's state by hand reproduces the new binary's numbers exactly. The
bill belongs to a folded document, not to how this spec arrives at one —
which is what G1 claims (`Z` then `z` *is* the specification) now shown
to hold for cost as well as for appearance.

Per thread at convergence, which is where the cost actually sits:

| build | main thread | each of 7 heat workers |
|---|---|---|
| before, expanded | 7.58 s | 2.09–2.60 s |
| before, folded by hand | 8.03 s | 9.20–9.43 s |
| this spec | 7.54 s | 9.05–9.31 s |

The main thread is flat to within noise — 7.54 s against 7.58 s — so
G2 holds and N2's "startup work is reduced by this spec anyway" is true
**of the main thread**, which is the thread it was about. Every gram of
the extra CPU is in the heat workers.

### Why the workers pay more, and why that is acceptable

A `HeatRequest` carries `range: Range<usize>` — the node's byte span —
so a query's cost tracks the size of the node on the row. Read-ahead is
budgeted in **rows**: `PREFETCH_WALK_MAX_ROWS` is 2 048
(`prefetch.rs:34`), and each wave zigzags that many *visible* rows out
from the cursor. Expanded, 2 048 visible rows are the interior of the
first file or two — small leaves. Folded, they are 2 048 whole
`FileDescriptorProto`s out of 7 771.

From `PROTOLENS_TRACE`, and matching the CPU exactly:

| | requests pushed | worker CPU | per request |
|---|---|---|---|
| expanded | 1 949 | 15.4 s | 7.9 ms |
| folded | 2 775 | 63.9 s | 23 ms |

+42% in count times ×2.9 in unit cost is the ×4.2 observed. Sweeping
the window height over 10, 25 and 50 rows moves neither build, which is
the confirmation that this is the read-ahead's budget and not the
screenful.

So `PREFETCH_WALK_MAX_ROWS` is denominated in rows and this spec changes
what a row costs — the same "back-to-back `score_all` calls on a large
`FileDescriptorSet`" its own doc comment was written to prevent,
reinstated within the letter of the budget. **Deliberately not fixed
here.** It is `Tier::Prefetch` work: speculative, preemptible, and
outranked by `Visible` and `User` the moment the reader does anything.
Nothing a reader waits on got slower — only the time to a wholly idle
machine, which no gesture blocks on.

### The fold sets

At bake idle, 8.79 s in: `slots=4936532 folded=778987 auto_folded=0`.

- **778 987** bracketed slots, 15.8% of the arena — not the 4.74 M the
  Background originally assumed. `FoldSet` therefore replaces ~9 MB of
  hash table, not 150 MB, with 617 KB of bits per set.
- `auto_folded` is **empty** at end of bake, so the second set
  contributes nothing at all to the end-state figure; its ~84 000-entry
  peak is transient and mid-bake.
- Peak RSS is flat across all three builds, confirming the corrected
  figure: 9 MB is not visible in an 840 MB process.

G3 stands on the CPU results already in the Background, not on this.

### Unchanged

`rebuild_status` reads `auto_folded.word(idx / 64)` and its per-call
`vec![0u64; …]` is gone; `scrub_folds_under` is the unconditional loop;
`script_reset_folds` lost its sort; `search_segments` keeps its sort,
which needs `raw_start` order. All four as specified in S1.
