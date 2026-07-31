<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the override collection

*last verified: 2026-07-31*

## Executive summary

The override collection is the persistent record of every piece of schema
knowledge the user has attached to this document — "node at this
position is really this type," recorded durably enough to survive being
saved to disk, reloaded against a re-decoded document, and re-applied.
It is deliberately independent from the splice mechanism described
in [arena-and-batch.md](arena-and-batch.md): the collection is *what the user
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
bumps a depth counter (`override_batch_depth`), delegates the actual
recursive walk to `render_overrides_inner`, and — only once the counter
returns to zero — runs a single batch-finalization step.

The counter's job is narrower than "re-entrancy counter" suggests: no
call site anywhere invokes `render_overrides` from inside a walk (the
recursion is `render_overrides_inner` calling *itself*, which never
touches the counter), so it only ever holds 0 or 1. What it actually
encodes is a *mode flag* read by `splice_override`: non-zero means
"defer finalization to the active batch," zero means "you are a
standalone splice — finalize immediately yourself."

The zero case used to be the live preview, which is why it exists. Since
spec 0185 made the preview an overlay, **production has exactly one
`splice_override` caller and it is always inside a batch**; only tests
reach the standalone path. Reading the counter as genuine nesting
support overstates what is exercised.

`render_overrides_inner` walks the tree in document order (pre-order,
left to right) from a given starting node. At each node it:

1. Seeds an auto-expansion override (see
   [document-tree.md](document-tree.md)) if this is an unseeded
   Any/MessageSet candidate.
2. Resolves the node's currently-applicable override (if any), by origin
   priority, with auto-entry staleness demotion applied, and falls back
   to the node's *natural* type — what the parent's own schema says this
   field's type should be — when no override applies at all. This
   fallback is what makes clearing an override behave as "revert to what
   the schema says," not "revert to raw."
3. Compares the resolved target against the node's stored `rendered_as`
   provenance, and calls `splice_override` only if they differ. As
   described in [arena-and-batch.md](arena-and-batch.md), the splice
   itself does not touch the rendered buffers — it queues a deferred
   line patch.
4. Recurses into children — using the same recursion-gate widening for
   Any/MessageSet candidates described in
   [document-tree.md](document-tree.md), and only where
   `compute_descend_marks` says the rendering can change at all.

Because step 3 is a cheap no-op check when nothing changed, calling
`render_overrides` from the document root after *any* collection change
(activation, deactivation, rename, kind rotation, wholesale collection
replace) is always correct — it never re-splices anything that didn't
actually change, so there's no need for callers to reason about which
subset of the tree a given collection edit could have affected.

### Batch finalization: the text, and nothing else

A splice defers exactly one thing — writing the rendered lines — and
`finalize_override_batch` settles it once, when the outermost
`render_overrides` call returns (or when a standalone splice finishes).
It merges the batch's queued line patches into `lines` in a single pass,
bumps `structural_version` so the read-ahead walk restarts, and
re-clamps the pan. A batch that queued no patch at all returns
immediately: no patch means no text was replaced, so there is nothing to
repair.

Line *counts* are already correct by that point. `splice_override` fixes
its own target's and carries the change up its ancestors as it goes,
because the rest of the batch derives its patch positions from them.
Nothing below the splice needs touching, because no node stores a
position — see [rendering.md](rendering.md).

This is why the deferral exists at all: it makes a batch of splices cost
one line-buffer rewrite instead of one per splice. What used to make the
finalizer expensive was a second walk over *every node after the splice*
fixing stored line numbers, plus a full rebuild of two line-index maps;
both are gone, and with them the reason browsing candidates on a large
document used to pay a whole-document cost per highlighted row.

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
