<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the target blob

*last verified: 2026-07-16*

## Executive summary

Every protolens session starts from one immutable byte buffer — the "blob"
— which is never mutated after load. What *does* change over a session is
how much of the blob's structure protolens currently understands, which is
tracked entirely in the [document tree](document-tree.md) and the
[override collection](override-collection.md), not in the blob itself.
Coordinates shown to the user (byte ranges in the status line, extract
filenames) are always translated back to the *caller's* original blob
numbering, hiding an internal bookkeeping detail: protolens actually
decodes a slightly larger buffer than the one the user gave it.

## Technical detail

### The wrap-everything invariant

`decode_and_render_indexed` (from `prototext_core`) decodes *the fields of
a message*, not "a value of some type" in isolation — there is no
API for asking it to decode a bare byte range as, say, a lone `int32`.
protolens sidesteps this by never handing it a bare range: even the
top-level document is first wrapped in a synthetic single-field envelope
(field number 1) before the very first decode. This is why `App` tracks
both `blob` (the wrapped buffer actually decoded) and `wrapper_offset`
(the width of that synthetic outer tag+length prefix) — every
user-visible byte offset subtracts `wrapper_offset` to undo it.

The same wrapping happens again, on a much smaller scale, every time an
override is applied to some node deep in the tree ([see the splice
mechanic](arena-and-batch.md)): that node's own payload bytes are
re-wrapped under its real field number and re-decoded independently. The
top-level wrap and a mid-document override-triggered wrap are the same
operation; the root is not special-cased.

### Payload extraction is generic, not per-kind

Every displayed byte range — message, group, or scalar — is shown
*payload-only*: any framing (a length-delimited tag's length prefix, or a
group's `START_GROUP`/`END_GROUP` tags) is stripped before display or
extraction. This stripping is driven purely by wire type, not by whether
the node "is a message": a packed-repeated scalar element and a bytes
field are both wire-type LEN and get the same length-prefix treatment a
message would, while a still-schema-unresolved LEN-wire field is
indistinguishable from an embedded message until (or unless) an override
resolves it — which is exactly the situation the override system exists
to progressively narrow.

One boundary case: a *packed-repeated element*'s range is already
payload-only by construction (there is no per-element framing inside a
packed run to strip), so extraction detects and skips this case rather
than attempting to strip framing that isn't there.

### Extraction is display-state-independent

`extract.rs` always operates on the full underlying blob/line buffers,
never on whatever the user currently has folded, panned, or scrolled to.
Binary extraction slices bytes directly; text extraction slices and
dedents the already-rendered `#@ prototext`-annotated lines so the result
is self-contained and independently re-decodable (its own header records
enough to reopen it without the original document). This deliberate
independence from UI state is what makes `x`/`:extract` predictable: what
you get is always "this node, completely," never "this node, as currently
displayed."
