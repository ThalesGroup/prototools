<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the override collection

*last verified: 2026-07-25*

## Executive summary

The override collection is the persistent record of every piece of schema
knowledge the user has attached to this document — "node at this
position is really this type," recorded durably enough to survive being
saved to disk, reloaded against a re-decoded document, and re-applied.
It is deliberately independent from the tree-splicing mechanism described
in [document-tree.md](document-tree.md): the collection is *what the user
knows*, the splice pass is *how that knowledge gets drawn on screen*.
Keeping the two separate is what allows the whole collection to be
replaced wholesale (`:restore-overrides`) without protolens needing to
reconcile old and new tree state field by field — a full re-render pass
recomputes everything from the fresh collection alone.

## Technical detail

### Three ways to say "this node" — origins

An override needs a way to identify *which* node(s) it applies to that
survives the document being re-decoded (a session reload, or a saved
collection applied to a re-decoded document). Three such identifications
exist, in increasing order of generality:

- **`Path`** — an exact positional path (`/1/2/3`-style, purely
  structural, no schema knowledge required). Matches at most one node.
  This is what every override starts life as when first created from the
  override-select pane or `:type-as`.
- **`PathField`** — a parent's positional path plus a field number.
  Matches every child of that specific parent with that field number —
  useful for a repeated field where every element should get the same
  treatment.
- **`FqdnField`** — a parent *type* (by fully-qualified name) plus a
  field number, with no positional anchor at all. Matches every node,
  anywhere in the document, whose parent resolved to that type and whose
  own field number matches. This is the override that "follows the
  schema" rather than "follows the position" — the natural choice once a
  type is known to recur throughout a document.

Resolution priority when more than one origin's override could apply to
the same node is `Path > PathField > FqdnField` — the most specific
identification wins. Rotating an existing override between these three
kinds (the manage pane's `z`/`Z`) is a re-derivation, not a fresh
override: it recomputes what a `PathField`- or `FqdnField`-style origin
*would* look like for the same currently-affected node(s) and swaps the
origin in place, preserving the entry's own identity (activation state,
display-name override) across the rotation.

Sort order for the collection is lexicographic by each origin's own
display label, then by type — deliberately *not* grouped by kind first,
so origins naturally interleave in a stable, predictable order regardless
of which kind each happens to be.

### Auto vs. manual: two provenances, one activation model

Every entry carries an `auto: bool`. Auto entries are seeded automatically
by the Any/MessageSet expansion described in
[document-tree.md](document-tree.md) — never round-tripped through YAML,
and always demoted (deactivated, without being deleted) rather than
removed when their governing context changes, so that Any/MessageSet
resolution stays self-repairing as the user edits ancestor overrides
around them. The important design choice is that auto and manual entries
share **one** activation/resolution mechanism throughout — the same
"at most one active entry per origin" invariant, the same render-pass
demotion detection, the same manage-pane display (auto entries just
render in a muted style, per [manage-pane.md](manage-pane.md)). Auto is a
provenance tag consulted at a few specific decision points (delete
behavior, YAML serialization), not a parallel code path.

### The render pass: resolve, compare, splice — as one batch

`render_overrides` is the single function that keeps the displayed
document consistent with the current collection. It is a thin wrapper: it
bumps a re-entrancy counter (`override_batch_depth`), delegates the
actual recursive walk to `render_overrides_inner`, and — only once the
counter returns to zero, i.e. only for the *outermost* call — runs a
single batch-finalization step. Recursive calls made while already inside
a batch (e.g. auto-expansion triggering a nested `render_overrides` on a
freshly-revealed Any payload) skip finalization entirely and let the
outermost call pay for it once.

`render_overrides_inner` walks the tree in document order (pre-order,
left to right) from a given starting node. At each node it:

1. Applies any line-count correction already known to be owed to this
   node from an earlier sibling's splice earlier in the *same* batch,
   before doing anything else — so every subsequent step sees an
   up-to-date `text_range`.
2. Seeds an auto-expansion override (see
   [document-tree.md](document-tree.md)) if this is an unseeded
   Any/MessageSet candidate.
3. Resolves the node's currently-applicable override (if any), by origin
   priority, with auto-entry staleness demotion applied, and falls back
   to the node's *natural* type — what the parent's own schema says this
   field's type should be — when no override applies at all. This
   fallback is what makes clearing an override behave as "revert to what
   the schema says," not "revert to raw."
4. Compares the resolved target against the node's stored `rendered_as`
   provenance, and calls `splice_override` only if they differ. As
   described in [document-tree.md](document-tree.md), the splice itself
   does not touch the rendered buffers or downstream `text_range`s
   immediately — it queues a deferred line patch and adds to a
   batch-wide running line-count total.
5. Recurses into children — using the same recursion-gate widening for
   Any/MessageSet candidates described in
   [document-tree.md](document-tree.md) — carrying forward whatever
   line-count correction this node's own splice (if any) owes to its
   children, and accumulating each child's own growth so later siblings
   and the node's own closing/footer line stay correct.

Because step 4 is a cheap no-op check when nothing changed, calling
`render_overrides` from the document root after *any* collection change
(activation, deactivation, rename, kind rotation, wholesale collection
replace) is always correct — it never re-splices anything that didn't
actually change, so there's no need for callers to reason about which
subset of the tree a given collection edit could have affected.

### Batch finalization: one O(remaining document) pass, paid once per call

Everything a splice defers — line-buffer materialization, and correcting
every node whose `text_range` a splice's line-count delta invalidated —
is settled in a single step, once, when the outermost `render_overrides`
call (or a standalone splice such as a live-preview update, which is
always its own one-node batch) is about to return:

1. Flatten every queued line patch from this batch into the rendered
   `lines`/`line_styles` buffers, in one pass proportional to the final
   document length.
2. If the batch's running line-count delta is non-zero, walk the
   document-order chain (`doc_next`) forward from the end of the
   outermost spliced node's own subtree to the end of the document,
   shifting every node's `text_range` by that delta, and walk the
   `parent` chain upward from the outermost spliced node correcting each
   ancestor's own closing extent.
3. Fully rebuild the `line_to_node`/`footer_line_to_node` lookup tables
   by walking `doc_next` from the very first node.
4. Reset the batch's running delta, and rebuild the visible-row cache.

Steps 2 and 3 are each proportional to *the size of the document from the
spliced node onward* — not to the size of what actually changed. This is
paid once per `render_overrides` call regardless of how many nodes inside
that call were actually spliced, which is a deliberate trade-off (see
spec 0160): it is far cheaper than paying an O(document) correction after
*every individual splice*, but it is not O(1) or O(change-size), and on a
large document (order-of-a-million nodes) it is expensive enough to be
felt directly — every live-preview update in
[override-select-pane.md](override-select-pane.md) is its own standalone
batch, so browsing candidates on such a document pays this full cost on
every highlighted row, not just on commit. Spec 0163's own non-goals
section flagged this exact cost as a known, deferred problem requiring a
larger document-representation change to fix properly, rather than a
targeted one.

### Persistence: YAML, hash-checked, root-preserving

Saving writes the whole collection plus two hashes (the target blob's,
and the loaded descriptor set's) so a later load can warn — never
block — on a mismatch. Loading silently drops any entry that no longer
resolves against the current tree/descriptor pool (a `Path` whose
position no longer exists, a `FqdnField` whose type or field was
removed), rather than failing the whole load over one stale entry. One
deliberate exception to "wholesale replace": the document root's own type
is preserved across a load unless the loaded file defines its own active
root entry, because root's type is the one piece of information that
can never be re-derived from the schema (`natural_type` always looks
*upward* to a parent's field descriptor, and root has none) — losing it
silently would cascade into every schema-typed descendant reverting to
raw.
