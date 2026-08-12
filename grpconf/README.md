<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# `anomalies.pb` — every anomaly in one blob

A demo blob for protolens. It exhibits **every** annotation token
prototext-core can emit: the five wire-type names, `pack_size`, the nine
non-canonical modifiers and the fifteen invalid ones.

## Running it

```
protolens --type google.protobuf.FileDescriptorProto grpconf/anomalies.pb
```

No `--descriptor-set` is needed: `google.protobuf.FileDescriptorProto` is one
of the well-known types protolens ships with (spec 0228).

Press <kbd>w</kbd> to show the wire bytes under each line. The three severity
tiers — landmark, non-canonical, invalid — are then visible on both rows at
once, which is what the fixture exists for.

`anomalies.script` sits beside the blob and is picked up automatically, so the
session opens with a commentary pane at the top walking the sections below in
order. Press <kbd>space</kbd> to hand it the arrow keys, then
<kbd>→</kbd>/<kbd>←</kbd> to step; <kbd>space</kbd> again gives the
keys back and you can explore from wherever the step left you. `--no-script`
opens the blob without it. Specified in
`docs/specs/0271-a-script-walks-the-reader-through-the-blob.md`.

A schema *is* required, though — `--raw` will not do. Half the anomalies are
*terminal*: once a varint has no terminator or a wire type has no meaning, the
decoder cannot find the next tag and gives up on the rest of the enclosing
region. Each one is therefore wrapped in its own `message_type` submessage,
whose length prefix bounds the damage — and the decoder only descends into a
submessage when the schema says it is one. Under `--raw` those submessages stay
opaque strings and the anomalies inside them are never seen.

Every *other* example is wrapped the same way, though nothing forces it to be,
and each wrapper is introduced by a top-level `name` line carrying its heading.
A folded node shows no preview of its contents, so a heading written inside the
wrapper folds away with it; written beside it, the whole document folds down to
twenty-three readable headings. `name` is `FileDescriptorProto`'s own field 1 —
singular, but a singular field repeated on the wire renders once per occurrence.
And wherever an anomaly has an
ordinary counterpart, the two are written side by side inside that submessage —
the padded tag above, the same string with a one-byte tag below; the five-byte
`-1` above, the specified ten-byte one below. The text of the two lines is
identical and the <kbd>w</kbd> rows are not, which is the whole point.

Any descriptor set containing `descriptor.proto` also works, passed with
`--descriptor-set`; `prototext-core/fixtures/descriptor.pb` is one that is in
the repo. `prototext` resolves `google.protobuf.*` on its own too:

```
prototext decode -t google.protobuf.FileDescriptorProto grpconf/anomalies.pb
```

## What is committed

`anomalies.pb` is `#@` prototext **text**, not wire bytes, despite the
extension. Both tools decide the format by content — they peek the first
thirteen bytes for `#@ prototext:` — so the file can be committed in the form
a human reads and edits, and opened directly. There is no build step and no
second artifact to go stale.

Two consequences worth knowing:

- The `(N bytes)` protolens prints at startup is the size of the file **on
  disk**, not of the payload it encodes to. The 12 KB here becomes about 1.8 KB
  of wire bytes.
- The SPDX header lives in `anomalies.pb.license` rather than in the file: the
  format detection is a `starts_with`, so the first bytes must be
  `#@ prototext:`.

## What it shows, in order

The file is read top to bottom during a talk, so it is ordered by what an
audience gets the most from rather than by severity. Each anomaly within a
section carries its own letter — `1.a.`, `1.b.`, … — spelled out in the heading
above it, and `anomalies.script` walks them one anomaly per step in that order:

1. **Legal bytes no canonical writer produces** — `tag_ohb`, `len_ohb`,
   `val_ohb`. Varints may carry padding, so a tag, a length prefix and a value
   can each be written longer than they need to be. Every parser accepts them.
2. **Values that survive a round trip but not a re-encode** — `truncated_neg`,
   `nan_bits`. Two producers disagree and both are right.
3. **A newer producer against an older schema** — `ENUM_UNKNOWN`, plus four
   fields the schema does not declare at all, rendered by wire type
   (`varint`, `fixed64`, `fixed32`, `bytes`).
4. **A packed run** — `pack_size`, `ohb`, `neg`. Three text lines, one wire
   record; the place where the <kbd>w</kbd> row earns its keep. A second run
   beside it holds the same three numbers canonically, and is the longer of
   the two.
5. **Blob and schema disagreeing** — `TYPE_MISMATCH`, `INVALID_STRING`,
   `INVALID_PACKED_RECORDS`. Still parseable; the scan continues.
6. **Malformed wire bytes** — `TRUNCATED_BYTES`/`MISSING`, `INVALID_VARINT`,
   `INVALID_LEN`, `INVALID_FIXED64`, `INVALID_FIXED32`, `INVALID_TAG_TYPE`,
   `TAG_OOR`. One submessage each, and nothing may follow them inside it.
7. **Groups** — `group`, `etag_ohb`, `END_MISMATCH`, `ETAG_OOR`, `OPEN_GROUP`,
   `INVALID_GROUP_END`. Last, because proto2 groups are history to most of the
   audience and nothing here declares one, so they all arrive as undeclared
   fields.

Each anomaly is explained twice: by a `#` comment for a reader with the file
open in an editor, and by a **string field value** — the `name` heading above
its wrapper — for the audience looking at protolens. The comments are dropped
by the encoder and never reach the wire; the string values are the wire.

## Keeping it honest

`prototext-core/tests/anomaly_fixture.rs` encodes the fixture, re-renders it
and asserts that re-encoding the rendering gives back the same bytes, and that
the set of annotation tokens in the rendering **equals** the vocabulary. A
missing token means the fixture stopped covering something; an extra one means
the renderer grew a token nobody classified.

Specified in `docs/specs/0226-a-fixture-shows-every-anomaly.md`.
