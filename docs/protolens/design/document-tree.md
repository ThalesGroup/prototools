<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the document tree

*last verified: 2026-07-25*

## Executive summary

The document tree is protolens's own navigation structure, built once
over the flat list of fields `prototext_core` decoded and then kept
alive — and selectively rebuilt — for the rest of the session as the user
applies overrides. Two properties define almost everything about how it
behaves: nodes are stored in **post-order**, not document order, so a
parallel document-order chain is maintained explicitly; and an override
never rebuilds the whole tree, only **splices** the one affected subtree
in place. Understanding these two properties up front makes the rest of
the tree's code read as consequences rather than arbitrary choices.

## Technical detail

### Post-order storage, explicit document-order chain

`prototext_core::decode_and_render_indexed` returns its flat `NodeSpan`
list in post-order (a container's own entry comes *after* all of its
descendants) — a natural consequence of how a recursive-descent decoder
finishes a subtree before it can know that subtree's own final extent.
protolens's `build_tree` does not fight this; it accepts post-order as the
array's native layout (array index has no navigational meaning) and
separately threads a `doc_next`/`doc_prev` linked chain, sorted by each
node's actual `raw_range.start`, purely for document-order traversal.
Every "move to next/previous node" operation in the UI walks this chain,
never the array. `build_tree` itself is a single linear pass using a
stack of "subtree roots still being completed," an approach chosen
because it matches the post-order input directly rather than requiring a
second pass to reconstruct parent/child relationships.

### The splice: the tree's one mutation primitive

Once built, the tree is never rebuilt wholesale in response to an
override — that would invalidate every index a live UI element (cursor,
fold set, jumplist, override-collection origin) might be holding.
Instead, `splice_override` replaces exactly one node's own rendering and
descendants, in place, leaving that node's own array index — and every
untouched node's index — unchanged. Concretely, applying an override to
node `idx`:

1. Re-wraps `idx`'s payload bytes under the new target type (or leaves
   them unwrapped for a "raw" target) and decodes that in isolation —
   the same [synthetic-wrapper trick](target-blob.md) used for the whole
   document, applied locally. Live preview (see
   [override-select-pane.md](override-select-pane.md)) may additionally
   cap this decode to a bounded number of spans, so an interactive
   preview over a huge subtree stays cheap; every other caller (a real
   commit, `render_overrides`, auto-expansion) is unbounded.
2. Builds a small local tree over just that decode, and appends its
   nodes to the *end* of the global array — it does not attempt to
   reuse or renumber any existing slot.
3. Rewrites `idx`'s own entry to describe the new subtree's root (so
   anything referencing `idx` transparently sees the new content), and
   stitches the local tree's remaining nodes in as `idx`'s new
   descendants. It also relinks `idx` into the `doc_next`/`doc_prev`
   chain: the new subtree is spliced in where the *old* one used to sit,
   found by walking forward from `idx`'s own (pre-splice) `doc_next`
   past every node that belonged to the old subtree, until the first
   live node genuinely outside it — that node becomes the new subtree's
   `doc_next` seam.
4. Does **not** touch the rendered text/style buffers directly. Instead
   it records the affected line range and its replacement lines as a
   pending, deferred patch; the buffers are only actually rewritten once
   per *batch* of splices (a single `render_overrides` pass, or a single
   standalone splice such as a live-preview update), described in
   [override-collection.md](override-collection.md)'s render-pass
   section. `text_range`s of nodes *downstream* of `idx` are similarly
   left stale until that same batch-end step corrects them.
5. Orphans the old subtree's nodes (they stay in the array, unreachable
   from any live pointer, and are scrubbed out of the fold set so stale
   entries can't hide unrelated content) rather than compacting the
   array.

This "always append, never renumber or compact" discipline is why a node
can be re-overridden any number of times over a session without ever
invalidating another node's index — the trade-off is that the array
grows monotonically and accumulates orphaned entries, which is accepted
as a cheap, session-scoped cost.

### The seam `doc_next` does *not* directly give you

It is tempting to assume `idx.doc_next` is the seam step 3 needs — that
because `doc_next` is a document-order successor, and a node's own
descendants can never be its successor, `idx.doc_next` must point
somewhere outside `idx`'s subtree. That is true of a *leaf*, and it is
true of `idx` immediately after `build_tree` if `idx` is childless. It is
**false in general**, and specifically false in exactly the case the
splice machinery cares about: as soon as `idx` has any descendants,
`idx`'s document-order successor *is* its own first descendant, because
document order is pre-order — a container's opening line precedes its
children's lines. So for any node with children, `idx.doc_next` points
*into* `idx`'s subtree.

This is why step 3 does not use `idx.doc_next` as the seam but as a
*starting point*: `doc_next_after_subtree`
(`override_apply.rs:120-134`) walks forward from it, skipping every node
in `idx`'s previously-collected descendant set, and returns the first
node genuinely outside. That returned node — not `idx.doc_next` — is the
seam the new subtree's document-order-last node is stitched onto, and it
is also where the batch-end downstream-correction walk
(`finalize_override_batch`, [override-collection.md](override-collection.md))
begins bringing every later node's stale `text_range` back in sync.

Because of this, `doc_next`/`doc_prev` are load-bearing far beyond
ordinary "next/previous" navigation, and any code outside
`splice_override` itself that touches them must reproduce this
recomputation. A 2026-07-25 bug did not. The override-select pane's
live-preview retry path (re-previewing a different candidate on the same
node, [override-select-pane.md](override-select-pane.md)) truncates the
array back to a watermark before re-splicing. It nulled
`first_child`/`last_child` defensively — correct, since both pointed into
the truncated range — but left `doc_next` untouched, on the mistaken
"successor is always outside the subtree" reasoning above. Since
`doc_next` pointed at the previous preview's first child, i.e. *at or
past the watermark*, truncation left it dangling at an index that the
very next splice would make valid again by pushing fresh nodes into those
same slots. The result was a **cycle** wired into the document-order
chain, which hung `finalize_override_batch`'s forward walk forever.

The fix (`override_select.rs:774-797`) is to recompute the seam with
`collect_descendants` + `doc_next_after_subtree` and store it into
`idx.doc_next` *before* `tree.truncate(watermark)` runs — while
`first_child`/`last_child` still describe the pre-truncation subtree the
descendant set is derived from. Ordering matters: after the truncation
the information needed to compute the seam no longer exists.

The lesson generalizes: `doc_next` is a different kind of pointer from
`first_child`/`last_child`/`doc_prev` in that it *may* point into the
subtree being discarded and *may* point outside it, and which one is the
case depends on whether `idx` currently has descendants. Neither
"clear it, we're discarding this subtree" nor "leave it, it points
outside" is correct unconditionally; only recomputing it is.

### `rendered_as`: provenance, not just "is there an override"

Each node's `rendered_as` field records *what it was last spliced as* —
not merely whether an override currently applies to it. This distinction
matters because the interesting question when deciding whether to
re-splice is never "is an override active right now," but "does the
currently active resolution (override, or its absence) differ from what's
already on screen." Comparing against stored provenance is what correctly
detects a *demotion* — an override that used to apply and no longer does
— and not just fresh promotions or retypes; a node whose override was
just removed still needs to fall back to its natural, schema-inferred
type, and `rendered_as` is what tells the render pass that a splice is
needed to make that happen.

### Any/MessageSet auto-expansion is a recursion-gate widening, not a special path

The render pass that walks the tree applying overrides (`render_overrides`
— described fully in [override-collection.md](override-collection.md))
normally only recurses into nodes already known to be message/group
shaped. Any and MessageSet fields start out as plain scalar (LEN-wire)
fields, which would ordinarily never be visited at all. Rather than give
these two shapes a separate traversal path, the recursion gate is widened
just enough to let exactly these two structural shapes through
(`is_auto_expand_candidate`), so that on first visit they can be
auto-resolved to a concrete type and folded into the ordinary override
machinery from then on. The gate is kept narrow deliberately: recursing
into *every* scalar LEN-wire field unconditionally was an earlier bug
(an ordinary string/bytes field being wrongly demoted to a raw dump), so
the condition is written to match only the two known Any/MessageSet field
shapes, not "any field that might conceivably be a message."
