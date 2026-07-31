<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the document tree

*last verified: 2026-07-31*

## Executive summary

The document tree is protolens's navigation structure over the decoded
blob. Its **storage and shape** — how it is built, why it is in level
order, what a splice does to it — are described once, in
[arena-and-batch.md](arena-and-batch.md), and not repeated here. This
file covers what the tree *means* to the code above it: how a node
remembers what it was last rendered as, and how Any/MessageSet fields
get folded into the ordinary override machinery.

Two facts from the arena file are worth restating because everything
below leans on them: a node keeps its slot for the life of the document,
and the arena describes all the structure the bytes admit while any one
rendering shows only part of it.

## Technical detail

### `rendered_as`: provenance, not just "is there an override"

Each node records *what it was last spliced as* — not merely whether an
override currently applies to it. The interesting question when deciding
whether to re-splice is never "is an override active right now" but
"does the currently active resolution, override or absence of one,
differ from what is already on screen".

Comparing against stored provenance is what correctly detects a
**demotion** — an override that used to apply and no longer does. A node
whose override was just removed still needs to fall back to its natural,
schema-inferred type, and `rendered_as` is what tells the render pass
that a splice is needed to make that happen. Fresh promotions and
retypes would be detectable without it; demotions would not.

Since [spec 0213](../../specs/0213-the-provenance-is-one-word.md) the
node holds a 4-byte `ProvenanceId` and the value itself lives once in
`App::provenance`. The comparison is one `u32` against another, and the
four states it distinguishes are unchanged (`protolens/src/provenance.rs`
enumerates them).

The pair is interned **whole**, not by its two halves: the type half
needs three distinct not-a-type-name values — no override, explicit raw,
and never rendered — which a shared `FqdnId` cannot express.

`ProvenanceTable` reserves exactly one sentinel, `NOT_RENDERED`, because
its only caller interns and can therefore only hold a real id. Add a
lookup that does *not* insert and it needs a second reserved value, for
the same reason `FqdnTable` has two (see the README's `prototext_core`
boundary section).

### Any/MessageSet auto-expansion is a recursion-gate widening, not a special path

The render pass that walks the tree applying overrides
(`render_overrides` — described fully in
[override-collection.md](override-collection.md)) normally only recurses
into nodes already known to be message- or group-shaped. Any and
MessageSet fields start out as plain scalar LEN-wire fields, which would
ordinarily never be visited at all.

Rather than give these two shapes a separate traversal path, the
recursion gate is widened just enough to let exactly these two
structural shapes through (`is_auto_expand_candidate`), so that on first
visit they can be auto-resolved to a concrete type and handled as
ordinary overrides from then on.

The gate is kept narrow deliberately. Recursing into *every* scalar
LEN-wire field unconditionally was an earlier bug — an ordinary
string/bytes field wrongly demoted to a raw dump — so the condition
matches only the two known Any/MessageSet field shapes, not "any field
that might conceivably be a message".

Note this is protolens's own machinery, not `prototext_core`'s:
`decode_and_render_indexed` is always called with `expand_any` and
`expand_message_set` **off**.
