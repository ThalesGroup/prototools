<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0307 — a fabricated byte says so

Status: implemented
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0225-….md (S3, which refused the wrapper root's wire
        row; S11's four-hue palette and its deliberately uniform hex
        foreground, which this leaves alone),
      docs/specs/0306-….md (which narrowed the row instead, and whose
        N1 this reverses),
      docs/specs/0192-….md (S2's three emphasis targets on the document
        row, one of which this reuses)

## Background

Every document protolens opens is wrapped. `Blob` prepends a real
field-1 `WT_LEN` tag and length prefix so that one coordinate frame
serves the wrapped and the unwrapped case (`blob.rs:51`), and slot 0 is
that wrapper. Two to four bytes of every blob in memory are protolens'
own.

Spec 0225 S3 refused to draw the wrapper root's wire row at all, to
avoid showing them. Spec 0306 found that this threw away the file to
hide the prefix — on an untyped blob the document is one flat line, so
refusing that line refused every byte — and narrowed the slice to
`wrapper_offset..` instead.

The narrowing works, and it left one thing behind. Having removed the
tag, S3 of that spec had to call what remained `Framing::Raw`, whose
contract is "bytes no framing claims — trailing content inside a message
that no child took" (`wire.rs:77-80`). But these bytes *are* claimed, by
a record protolens itself wrote. So the one row that is the whole
document is also the only LEN node in protolens that does not show its
own framing: on `CUT_SHORT` it reads

```text
0a 03 61 62 63 0a 10 73 68 6f 72 74
```

where every other length-delimited node on the screen reads
`2|08:0c[…]`. The prefix is gone, and so is any sign that it was ever
there.

0306 N1 rejected drawing it dimmed, on the ground that "a byte that is
there 'but greyed out' still has to be counted past by a reader checking
an offset". That is answered rather than overruled. The wire row has no
offset gutter; the coordinates a reader counts against are
`display_range`'s, which already subtract `wrapper_offset` and are not
touched here. What the prefix costs a reader is two to four columns at
the very start of one row, in exchange for that row showing the same
structure as every other.

## Goals

- **G1.** The wrapper root's row is framed like any other LEN node's.
- **G2.** A byte protolens invented is drawn unlike every byte of the
  file, in a channel the wire row is not already using.
- **G3.** Hovering one says, in words, that it is not in the file.
- **G4.** The document row's `1` — the same fabricated field, spelled as
  a name rather than as bytes — carries the same mark.

## Non-goals

- **N1.** Changing `display_range`, `Blob`, or `wrapper_offset`. The
  prefix stays outside every coordinate the UI prints, so no displayed
  offset is ever negative and the wrapper's own node still displays as
  `[0, n)`. This is 0306 N2, unchanged.
- **N2.** A general provenance channel. `Blob`'s prefix is the only
  fabricated byte protolens has; one flag on one slice is the whole
  requirement.
- **N3.** Varying the hex foreground at all. S11's reason for a uniform
  foreground stands, and S2 records the trial that confirmed it.
- **N4.** The preview overlay's wire row. `preview_wire_row` paints
  `overlay.bytes`, a buffer the preview re-encoded for itself, and the
  header it shows is its own, not `Blob`'s. Whether a preview should
  admit to synthesizing a header is a separate question about a separate
  buffer.

## Specification

- **S1.** `WireSlice` gains `synthetic_end: usize` — the absolute offset
  before which the row's bytes are protolens' rather than the user's,
  and `0` on every slice but one. `wire_slice` sets it to
  `wrapper_offset` under the predicate 0306 already used
  (`wrapper_offset > 0 && self.parent(idx).is_none()`) and otherwise
  hands back exactly what `head_or_tail` returned —
  `Framing::Tagged` included. `without_the_wrapper_prefix` is deleted.

  Set unconditionally on both of the root's rows. A bracketed root's
  footer starts at its last child's end, which is at or past
  `wrapper_offset`, so the flag costs that row nothing and no second
  predicate is needed to say so.

- **S2.** A fabricated byte keeps every color it would otherwise have —
  a fabricated tag is still a tag, and wearing `Region::Tag`'s hue is
  what makes it readable as one — and is set in **italic**.

  Not a color, because the wire row has no color left to spend. The
  background is spoken for twice already: S11's hue for what a byte is
  *for*, taken over by a tier's band when it is malformed. And the
  foreground is the one thing keeping dense hex legible over whichever
  band it lands on, which is what S11 kept it uniform for. A green
  foreground was tried there first and withdrawn on sight: against the
  blue an undeclared field wears, it was hard to read.

  Italic is the better shape for the fact besides. Provenance is not a
  property of the byte, the way its region and its tier are — it is a
  note that the byte is not the reader's, which is what a typographer
  sets an interjection in someone else's text in.

- **S3.** Both halves of a split tag and the bar between them take it.
  `tag_head` draws one byte as `2|08`; the byte is fabricated whole, so
  all six columns say so.

- **S4.** `theme::SYNTHETIC` is the modifier, a plain `const` — a
  `Modifier` needs no theme argument and no `OnceLock` cache, which is
  most of what a color would have cost. One name, used by both rows, so
  the wire row's marking and the document row's cannot drift apart.

- **S5.** `wire_box` leads with one line — `these bytes are not in the
  file — protolens wraps every document in a field 1` — when the hit
  lies wholly inside the prefix. It leads rather than trails because
  that is why the reader hovered: the answer to "why is this slanted"
  comes before "field 1", not after it.

  The test is `hit.bytes.end <= wrapper_offset` on the root, and a part
  is wholly inside the prefix or wholly outside it — the length varint
  ends exactly where the payload begins, so no coalesced part straddles
  the boundary.

- **S6.** The prefix counts against `WIRE_ROW_MAX_BYTES` like any other
  drawn byte. Two to four of sixty-four, on one row of the document; a
  special case to buy them back would be arithmetic in the elision
  counter for no reader's benefit.

- **S7.** The wrapper root's **key** on the document row takes
  `theme::SYNTHETIC` too. `1` there is the same invented field number
  that the wire row's prefix spells in hex, and the wire row is not
  always open — `w` is a toggle, and the document row is what a reader
  looking for the file's shape reads instead.

  It rides `row_spans`' existing per-segment weight, alongside spec 0192
  S2's emphasis, under the same predicate the wire row uses
  (`wrapper_offset > 0` and the row's node has no parent). Only the key:
  the row's other `Attribute` segment is its annotation's field number,
  which repeats what the key already says, and a second slanted run on
  one row reads as noise rather than as a cue.

  No color here either, and for a stricter reason than S2's — the
  document row's colors are the grammar's, and `Attribute` is the one
  role both palettes leave unstyled precisely so that emphasis can land
  on it.

## Alternatives considered

**Keep 0306's narrowing and mark the withheld prefix with `??`.** That
glyph already means "bytes the message does not contain" — a truncation.
These bytes are contained; they are just not the user's. One mark for
two different absences is worse than either alone.

**A `Region::Synthetic`.** `Region` decides banding and its five
variants are the parts of a record. These bytes have parts: the
wrapper's tag, type digit and length are exactly `Tag`, `Type` and
`Len`, and saying otherwise would lose the hue that makes them
readable as a tag. Provenance is orthogonal to region and belongs in its
own channel.

**Signal by offset rather than by a flag —"bytes below the payload
threshold".** The threshold is only expressible in absolute blob
coordinates, and every coordinate the rest of the UI shows has
`wrapper_offset` subtracted from it. A rule stated in a frame the UI
does not use would have to be re-derived at each consumer.

## Test plan

1. Rewrite `w_on_a_flat_wrapper_root_shows_the_whole_file`: on
   `CUT_SHORT` the slice is the whole node, `framing` is `Tagged`, and
   `synthetic_end == wrapper_offset`. Its bytes are the prefix followed
   by the file, byte for byte.
2. Invert `a_wrapper_root_never_shows_a_fabricated_byte` into
   `a_fabricated_byte_is_drawn_as_one`: over both root shapes, every
   drawn column of every byte below `wrapper_offset` carries
   `theme::SYNTHETIC` and no column of any byte at or above it does.
   Read off the finished spans, so what is measured is what is drawn.
3. `each_line_claims_its_own_bytes`: row 0 becomes the wrapper's header
   rather than the empty string. Its footer stays empty, now because the
   tail is empty rather than because the head was narrowed away.
4. `every_byte_appears_exactly_once_in_document_order`: the expectation
   becomes the whole blob, prefix included. That is the partition
   `head_or_tail` guarantees by construction, which 0306 was the one
   exception to.
5. `wire_box` over a prefix byte leads with S5's line; over the first
   payload byte it does not.
6. Nothing else moves: no slice but the wrapper root's has a non-zero
   `synthetic_end`, so every other row's spans are unchanged.
7. S7: on a wrapped fixture the root row's key is `1` and carries
   `theme::SYNTHETIC`; a child row's key does not. About the key alone,
   not about the whole row — the ANSI-16 fallback palette already spends
   italic on `Comment`, so every row's `#@` annotation wears it in a
   terminal without `COLORTERM`, which the Nix sandbox is.

## Measured outcome

**Implemented 2026-08-16.**

`tui/wire.rs` — `WireSlice::synthetic_end`, set in `wire_slice` under
0306's own predicate; `without_the_wrapper_prefix` deleted; `Painter`
carries the offset as an index into its own row and `Painter::tint`
applies `theme::SYNTHETIC` at the three call sites that emit a byte's
columns (`byte`, `tag_head`'s two halves, and the bar). `theme.rs` —
`pub const SYNTHETIC: Modifier = Modifier::ITALIC`. `tui/popup_wire.rs`
— `is_synthetic`, and S5's line pushed ahead of the region lines.
`tui/render.rs` — S7's `synthetic_key`, folded into the first
`Attribute` segment's weight in `row_spans`.

The flat case is the whole of the gain: on `CUT_SHORT`, where 0225 drew
nothing and 0306 drew `0a 03 61 62 63 …`, the row now reads `2|08:0c[…`
with the first six columns slanted.

`head_or_tail`'s partition is exact again —
`every_byte_appears_exactly_once_in_document_order` compares against the
whole blob, where under 0306 the wrapper root was its one exception.

Green: 1074 protolens unit tests (84 in `wire::`), 25 `batch_export`, 3
`batch_script`, and the workspace in release. Test 2 and test 7 were
each checked non-vacuous by neutering the flag they measure.

A green foreground was implemented first, per S2, and withdrawn after
looking at it: over the blue band an undeclared field wears, it was hard
to read. `theme::synthetic_text` and its two RGB constants were removed
with it.
