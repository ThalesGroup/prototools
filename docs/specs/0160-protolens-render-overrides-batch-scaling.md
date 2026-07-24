<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0160 — protolens: batch `render_overrides`/`splice_override` bookkeeping to fix large-document startup stall

Status: implemented
Implemented in: 2026-07-24
App: protolens

## Background

`App::new` (`tui/mod.rs`) ends with a single unconditional call,
`app.render_overrides(cursor)`, that walks the whole freshly-decoded
document from the root and, for every message/group-typed node (plus
a couple of narrower auto-expand/override-carrying cases —
`render_overrides`'s own doc comment, spec 0119/0120/0135), calls
`resettle_node`, which in turn calls `splice_override` whenever that
node's `rendered_as` doesn't already match its resolved type. Since
every node is freshly built by `build_tree` with `rendered_as: None`,
this *first* pass unconditionally splices every message/group-typed
node in the document — there is no schema-dependent shortcut: the
same unconditional walk happens whether or not `--descriptor-set`/
`--type` were given, because it is driven purely by `rendered_as`
being unset, not by whether a type resolved.

`splice_override` (`override_apply.rs`) is `render_overrides`'s only
mutator. Beyond decoding the node's own (typically small) tag+payload
bytes and running `self.lines`/`self.line_styles`
`Vec::splice`/`self.tree.push` for its own local content — genuine,
unavoidable per-call costs — every call *also* pays four bookkeeping
costs sized to the *whole document*, not to the node being spliced:

1. **Forward doc-chain shift** (`override_apply.rs` ~1118-1126):
   walks every node from the just-spliced node's `doc_next` to the
   end of the document, shifting each one's `text_range` by this
   splice's line-count `delta`.
2. **Ancestor closing-brace shift** (~1127-1134): walks up `idx`'s
   parent chain, shifting each ancestor's `text_range.end`.
3. **`line_to_node`/`footer_line_to_node` full rebuild** (~1136-1151):
   `.clear()`s both maps, then walks the *entire* live document chain
   (`self.first_node`'s `doc_next` chain) to repopulate them.
4. **`rebuild_visible_rows()`** (~1152): recomputes fold visibility
   for every line (`vec![false; total_lines]`).

For a single interactive edit (retyping one field via the override
pane) this is fine — one splice against a document that's usually a
few hundred to a few thousand lines. But `App::new`'s startup pass
calls `splice_override` once *per message/group node in the whole
document*, each paying all four whole-document costs. For a
reproduction fixture `/tmp/db3.desc` (a 1.1 MB serialized blob;
147,342 message/group nodes among ≈149,359 tree nodes / 196,701
rendered lines after full raw expansion), this measured **47,342**
`splice_override` calls in the single startup pass — an O(M×N)
blowup (M = splice calls, N = document size, both tracking the same
document and both in the tens-to-hundreds-of-thousands for a
multi-megabyte blob) that dominates over the (cheap, per-call) actual
decode work.

Confirmed by direct instrumentation and timing (all reverted after
measurement, no code changes described here are yet applied):

- **Unmodified `main`**: `protolens /tmp/db3.desc` (no
  `--descriptor-set`) does not finish `App::new` within a 90 s
  timeout (`user` time ≈ `real` time throughout — CPU-bound, not
  I/O-blocked). The same stall reproduces identically with a real
  schema loaded (`--descriptor-set=<meta_descriptor.pb>
  --type=google.protobuf.FileDescriptorSet`) — **not specific to
  schemaless launch**: the startup pass is unconditional regardless
  of whether a schema is available to resolve types.
- With all four whole-document costs above stubbed out for the
  duration of the measurement (keeping the genuinely per-node
  `Vec::splice`/`tree.push` work intact): **8.3 s** — over 10x
  faster, and now *bounded* rather than effectively hanging.
- Isolating either half alone is **not enough**: stubbing out only
  costs #3/#4 (hashmap rebuild + `rebuild_visible_rows`) while
  keeping #1/#2 (the shifts) still times out past 60 s; stubbing out
  only #1/#2 while keeping #3/#4 also still times out past 60 s. Each
  half is independently sufficient to reproduce the O(M×N) blowup —
  a real fix must address all four together, not just the
  "obviously safe" subset.

An initial implementation attempt at G2 used a single global
running-offset scalar (`pending_shift`) plus one snapshot value per
node, reconciled via `correction = pending_shift - node_snapshot` in
one final walk over the whole document. This is **not** correct in
general: it implicitly assumes every remaining splice in the batch
occurs *at or after* every not-yet-finalized node's own document
position, which is false whenever (a) an untouched node sits
*between* two splices processed in the same batch (it should only
inherit the earlier splice's delta, never the later one's), or, worse,
(b) the node is an *ancestor* of the node currently being spliced —
its `start` must never move due to its own descendants' growth, even
though its `end` must. The document root is the sharpest instance of
(b): it is essentially never re-spliced itself, so its snapshot is
never independently refreshed, and the single-scalar formula ends up
applying the *entire* batch's cumulative delta to `root.start`, which
must always stay `0`. Under `message_set_fixture()`'s nested MessageSet
auto-expansion (several splices within one batch, some growing content
and some later ones net-shrinking relative to earlier growth), this
compounds into a negative `usize` offset and panics — reproduced during
implementation as two MessageSet-specific test failures
(`round_trip_extract_and_encode_preserves_message_set_group_framing`,
`esc_closing_the_override_pane_restores_nested_message_set_auto_expansion`).
G2 below replaces that single-scalar approach entirely with a
per-recursion-frame *carried-down* correction, which does not have
this flaw (see G2 and Specification).

This spec covers only costs #1-#4 (call them "Phase 1" below) —
whole-document bookkeeping that is either (a) not actually read by
anything until the whole `render_overrides` walk finishes, or (b)
amortizable to O(1) per splice via a lazy running offset. The
remaining, harder cost — `self.lines.splice`/`self.line_styles.splice`
themselves, genuine `Vec::splice` memmove operations, individually
O(N) regardless of bookkeeping — is **not** addressed here (see
Non-goals); it is the residual cost behind the measured 8.3 s figure
above and would need a deeper "collect patches, materialize once"
redesign to eliminate.

## Goals

- **G1.** `render_overrides`'s whole-document bookkeeping — costs #3/#4
  above (`line_to_node`/`footer_line_to_node` rebuild,
  `rebuild_visible_rows()`) plus, per G2, the portion of costs #1/#2
  that lies *outside* the currently-processing subtree (forward shift
  beyond it, ancestor-end shift up to the document root) — runs
  exactly once per *outer* (caller-initiated) `render_overrides` call,
  not once per `splice_override` call, regardless of how many nodes
  that outer call's recursion ends up splicing. Tracked via a
  recursion-depth counter (`override_batch_depth`) on `App`,
  incremented/decremented around the outer call; the recursive descent
  itself no longer goes through the same counted method (it uses a
  separate, uncounted inner helper — see G2/Specification), so this
  counter now only ever toggles `0`/`1`, but the `0`-vs-nonzero
  distinction is still exactly what `splice_override` needs to decide
  whether it's being called standalone (must self-finalize
  immediately) or from within an active batch (defers to the outer
  call's own finalize).
- **G2.** Costs #1/#2 above, for nodes and text *inside* the
  currently-processing subtree, are replaced by a per-recursion-frame
  *carried-down* correction (detailed in Specification), not a single
  global running-offset scalar (see Background for why the
  single-scalar approach is unsound): each recursive call receives an
  `inherited: isize` line-count correction from its caller and applies
  it once, directly, to its own node's `text_range` — then, unless
  that node was itself just re-spliced (in which case its freshly
  decoded content is already positioned correctly, needing no further
  correction), passes the *same* correction down to each of its own
  children, accumulating each already-processed child's own growth
  onto the correction owed to that child's later siblings. Once a
  node's entire child loop finishes, its own closing/footer `end` is
  bumped, once, by the total growth accumulated across all of its
  children. This is O(1)-amortized per node touched (not per splice),
  the same complexity goal as the single-scalar approach, without its
  correctness bug. The portion of costs #1/#2 that lies *outside* the
  outermost call's own subtree (later siblings of that subtree, and
  the `end` of every ancestor of that subtree's root, up to the
  document root) is deferred exactly as a single ordinary
  eager splice would need it — done once, at G1's finalize point, using
  the batch's aggregate total delta.
- **G3.** This applies uniformly to *every* `render_overrides` call
  site, not just the ones passing the document root — including
  `override_select.rs`'s `close_override()` scoped call: `finalize`'s
  "outside the subtree" step (G2's last sentence) is exactly what
  makes a non-root call site correct too, without any special-casing.
- **G4.** No change in observable behavior for a single-splice
  interactive edit (retype, override activate/deactivate, etc.) —
  same final `lines`/`tree`/`line_to_node`/`footer_line_to_node`/
  visible-rows state as today, just computed with less redundant
  intermediate work when multiple splices happen in one outer call.
- **G5.** `App::new`'s startup `render_overrides` pass over
  `/tmp/db3.desc`-scale documents (tens to hundreds of thousands of
  nodes) completes in a small number of seconds rather than hanging
  indefinitely (informal target, not a hard SLA — see Non-goals: full
  elimination of the residual `Vec::splice` cost is out of scope, so
  this remains a large, order-of-magnitude improvement rather than a
  guarantee of near-instant startup on arbitrarily large blobs).

## Non-goals

- **N1.** Eliminating the `self.lines.splice`/`self.line_styles.splice`
  memmove cost itself (the residual ≈8.3 s on the reproduction
  fixture once G1/G2 land). That needs a "collect patches during the
  walk, materialize once in a single final O(N) reconstruction pass"
  redesign, which in turn requires the recursion to walk a
  scratch/local tree representation for not-yet-materialized splice
  results rather than immediately `self.tree.push`-ing them — a
  substantially larger architectural change, left for a future spec
  if the residual cost proves to still matter in practice after this
  fix lands.
- **N2.** Making `render_overrides`/`splice_override` viewport-scoped
  or lazy (only rendering what's currently visible). Rejected as an
  approach for this bug: unlike heat-cue scoring (spec 0151/0152),
  override/type resolution determines the actual *structure* of
  `self.lines`/`self.tree`, which folding, cursor navigation, search,
  and export all depend on globally — scoping it to the viewport
  would be a much larger, riskier redesign than what's needed here.
- **N3.** Any change to *what* gets spliced or *when* (auto-expand
  seeding, `resettle_node`'s re-splice-trigger condition, the
  recursion's descent condition into children) — this spec is purely
  about the bookkeeping cost paid *after* the decision to splice has
  already been made.

## Specification

No change to `protolens/src/decode.rs`/`TreeNode` — the corrected
design (see Background/G2) needs no per-node stored field at all.

### `protolens/src/tui/mod.rs`

- `App` gains two new fields, alongside `line_to_node`/
  `footer_line_to_node`:
  ```rust
  /// Spec 0160 G1: whether a `render_overrides` batch is currently
  /// active — `0` outside of one, `1` while the outer (caller-
  /// initiated) call is running. `splice_override` uses this to
  /// decide whether it must self-finalize immediately (standalone
  /// call, e.g. `override_select.rs`'s live-preview splice) or defer
  /// to the active batch's own finalize.
  override_batch_depth: u32,
  /// Spec 0160 G2: running total of line-count deltas accumulated by
  /// `splice_override` calls in the current `render_overrides` batch.
  /// Always `0` outside of an active batch.
  pending_shift: isize,
  ```
  Both initialized to `0` in `App::new`.

### `protolens/src/tui/override_apply.rs`

- `resettle_node`'s return type changes from `()` to `bool`: `true`
  iff it actually called `splice_override` and got back `Ok(())`
  (i.e. `idx` was freshly re-decoded), `false` otherwise (already
  matched `rendered_as`, or `splice_override` returned `Err`). Its one
  call site (below) needs this to distinguish "my own node was just
  re-spliced, so my children are fresh and need no further
  correction" from "my own node was untouched, so my *existing*
  children still owe whatever correction I was just given" — this
  can't be inferred from `pending_shift`'s value alone, since a splice
  whose new content happens to have the exact same line count as the
  old (`delta == 0`) must still count as "fresh".
- `render_overrides` becomes a thin outer wrapper; the actual
  (self-recursive) logic moves, unchanged in its own auto-expand-
  seeding/recursion-condition behavior, into a new private
  `render_overrides_inner(idx, inherited: isize)`, which the outer
  wrapper calls with `inherited = 0` (G1):
  ```rust
  pub(super) fn render_overrides(&mut self, idx: usize) {
      self.override_batch_depth += 1;
      self.render_overrides_inner(idx, 0);
      self.override_batch_depth -= 1;
      if self.override_batch_depth == 0 {
          self.finalize_override_batch(idx);
      }
  }
  ```
- `render_overrides_inner` (G2's carried-down correction — replaces
  costs #1/#2 for everything inside `idx`'s own subtree):
  ```rust
  fn render_overrides_inner(&mut self, idx: usize, inherited: isize) {
      // `idx`'s own node — and, if it's the leading element of a
      // packed-repeated run (spec 0135 G1), its later packed
      // siblings too, since `packed_record_extent` (inside
      // `splice_override`, called from `resettle_node` below) reads
      // the run's *last* sibling directly and can't wait for that
      // sibling's own turn later in this function's child loop —
      // needs `inherited` applied now, once, directly.
      if inherited != 0 {
          Self::shift_span(&mut self.tree[idx].span, inherited);
          // `packed_record_siblings(idx)` always returns the whole
          // run in document order, regardless of which member `idx`
          // is (an override can be triggered on *any* packed
          // element, not just the leading one — see
          // `splice_override`'s own doc comment) — so the run's true
          // leader is not necessarily `idx` itself. Shift every
          // *other* member explicitly (never a fixed `[1..]` slice,
          // which would wrongly assume `idx` is the leader: it would
          // both miss the true leader when it isn't `idx`, and
          // double-shift `idx` when it isn't).
          if self.tree[idx].span.packed_record_start.is_some() {
              for s in self.packed_record_siblings(idx) {
                  if s != idx {
                      Self::shift_span(&mut self.tree[s].span, inherited);
                  }
              }
          }
      }

      // ... existing auto-expand-seeding body, unchanged ...

      let spliced = self.resettle_node(idx);
      // Fresh children (just pushed by a splice) are already
      // positioned relative to `idx`'s now-correct start and owe
      // nothing further; pre-existing children (no splice happened)
      // still owe whatever `idx` itself was just given.
      let mut child_owed = if spliced { 0 } else { inherited };
      let initial_child_owed = child_owed;
      let mut child = self.tree[idx].first_child;
      while let Some(c) = child {
          if <existing recursion condition, unchanged> {
              let before = self.pending_shift;
              self.render_overrides_inner(c, child_owed);
              child_owed += self.pending_shift - before;
          } else if child_owed != 0 {
              Self::shift_span(&mut self.tree[c].span, child_owed);
          }
          child = self.tree[c].next_sibling;
      }
      // `idx`'s own closing/footer `end` must grow by however much
      // its children collectively grew during this loop (their own
      // splices, and/or further-nested descendants' splices).
      let growth = child_owed - initial_child_owed;
      if growth != 0 {
          self.tree[idx].span.text_range.end =
              (self.tree[idx].span.text_range.end as isize + growth) as usize;
      }
  }
  ```
  `Self::shift_span(span, delta)` is a small new private helper
  applying `delta` to both `span.text_range.start` and `.end` (the
  `as isize + delta) as usize` pattern already used inline elsewhere
  in this file, factored out since it now appears at several call
  sites above).
- `splice_override` changes (G2): with every call site now guaranteed
  to see an already-correct `self.tree[idx].span` before it's invoked
  (either by `render_overrides_inner`'s prologue above, or — for a
  standalone call, `override_batch_depth == 0` — because
  `pending_shift` is `0` and the tree is therefore already fully
  reconciled from the previous batch), `splice_override` itself no
  longer needs *any* correction logic on `old_span`/`idx`'s own span —
  remove it if present. Its only change is:
  - Remove the "Forward doc-chain shift" block (~1118-1126) and the
    "Ancestor closing-brace-line shift" block (~1127-1134) entirely;
    replace both with:
    ```rust
    self.pending_shift += delta;
    ```
  - Remove the "Full rebuild" block (~1136-1152,
    `line_to_node`/`footer_line_to_node`.clear()+rebuild +
    `rebuild_visible_rows()`) entirely.
  - At the very end (where the old code's whole-document rebuild used
    to run), self-finalize only for a standalone call:
    ```rust
    if self.override_batch_depth == 0 {
        self.finalize_override_batch(idx);
    }
    ```
    (using the post-packed-reassignment `idx`, same as the rest of the
    function already does).
  - The footer-cursor safety reset (existing) is unaffected — stays
    exactly where it is, keyed off topology (`has_children`), not off
    any shifted position.
- `packed_record_extent`: for the same reason, drop its own
  `shift_snapshot`-based `start_correction`/`end_correction` entirely
  — by the time it's called (from `splice_override`, itself called
  from `resettle_node`), every member of the packed run has already
  been corrected by `render_overrides_inner`'s prologue above (or, for
  a standalone call, is already reconciled since `pending_shift == 0`
  then). Reads `self.tree[siblings[0]].span.text_range.start` and
  `self.tree[last].span.text_range.end` directly, with no adjustment.
- New method `finalize_override_batch(idx)` — runs once, either at the
  end of an outer `render_overrides` call or at the end of a
  standalone `splice_override` call. Everything *inside* `idx`'s own
  subtree is already correct by this point (G2's carried-down
  correction, or — for a standalone call — `splice_override`'s own
  direct writes); what's left is exactly what a single eager splice on
  `idx` alone would still owe the *rest* of the document: every live
  node strictly after `idx`'s whole subtree (forward-shifted by the
  batch's total `pending_shift`), and every ancestor of `idx` (its
  `text_range.end` only, same total) — applied once for the whole
  batch instead of once per splice, followed by the
  `line_to_node`/`footer_line_to_node` rebuild and
  `rebuild_visible_rows()` (G1):
  ```rust
  fn finalize_override_batch(&mut self, idx: usize) {
      let delta = self.pending_shift;
      if delta != 0 {
          // Doc-order-last live descendant of `idx` — `last_child`,
          // walked to its own leaf, since `build_tree`/`splice_
          // override` always keep it as the doc-order-last direct
          // child (see `packed_run_is_last_child`'s existing use of
          // this same invariant).
          let mut last = idx;
          while let Some(lc) = self.tree[last].last_child {
              last = lc;
          }
          let mut after = self.tree[last].doc_next;
          while let Some(a) = after {
              Self::shift_span(&mut self.tree[a].span, delta);
              after = self.tree[a].doc_next;
          }
          let mut p = self.tree[idx].parent;
          while let Some(pi) = p {
              self.tree[pi].span.text_range.end =
                  (self.tree[pi].span.text_range.end as isize + delta) as usize;
              p = self.tree[pi].parent;
          }
      }
      self.line_to_node.clear();
      self.footer_line_to_node.clear();
      let mut cur = Some(self.first_node);
      while let Some(c) = cur {
          let node = &self.tree[c];
          self.line_to_node.insert(node.span.text_range.start, c);
          if node.span.text_range.end - 1 > node.span.text_range.start {
              self.footer_line_to_node.insert(node.span.text_range.end - 1, c);
          }
          cur = node.doc_next;
      }
      self.pending_shift = 0;
      self.rebuild_visible_rows();
  }
  ```

## Test plan

- Existing `tui/tests/override_apply.rs` suite must pass unchanged —
  single-splice behavior (G4) is not expected to change observably.
- New regression test(s) exercising a *multi-splice batch* within one
  `render_overrides` call (e.g. a message with several nested
  message-typed fields, each needing its own splice on the initial
  pass) asserting final `lines`/`line_to_node`/`footer_line_to_node`/
  visible-rows state matches what today's eager-shift code already
  produces (a "before/after refactor, same observable result"
  characterization test) — this is the primary correctness guard for
  G2's reconciliation formula.
- A packed-repeated-field regression test (spec 0135 G1's sibling
  merge, `packed_record_siblings`/`packed_record_extent`) specifically
  under a multi-splice batch, since `packed_record_extent` (called
  from `splice_override`, itself called from `resettle_node`) reads
  the run's *first and last* siblings directly, before they get their
  own turn in `render_overrides_inner`'s child loop — the `inherited`
  correction must be (and, per Specification above, is) applied
  preemptively to every packed sibling in `render_overrides_inner`'s
  prologue, not just to whichever element triggered the call. Include
  a variant where the override is triggered on a *non-leading* element
  of the packed run (an already-supported case per `splice_override`'s
  own doc comment) — this is the case that most directly exercises the
  prologue correctly shifting the true leading sibling even when it
  isn't `idx`.
- A `close_override()` scoped-call regression test (G3) confirming a
  non-root `render_overrides(idx)` call also goes through
  `finalize_override_batch` correctly (depth counter returns to `0`
  after this call too, not left elevated from some outer context that
  doesn't exist for this call site).
- Manual/perf validation (not a regression test, since it depends on
  an external large fixture not committed to the repo): confirm
  `protolens /tmp/db3.desc` (or an equivalent large blob) now
  completes `App::new` in a small number of seconds rather than
  hanging, both with and without `--descriptor-set`.
