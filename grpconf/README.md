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
protolens --descriptor-set prototext-core/fixtures/descriptor.pb \
          --type google.protobuf.FileDescriptorProto \
          grpconf/anomalies.pb
```

Press <kbd>w</kbd> to show the wire bytes under each line. The three severity
tiers — landmark, non-canonical, invalid — are then visible on both rows at
once, which is what the fixture exists for.

`anomalies.script` sits beside the blob and is picked up automatically, so the
session opens with a commentary pane at the top walking the sections below in
order. Press <kbd>space</kbd> to hand it the arrow keys, then
<kbd>Ctrl-→</kbd>/<kbd>Ctrl-←</kbd> to step; <kbd>space</kbd> again gives the
keys back and you can explore from wherever the step left you. `--no-script`
opens the blob without it. Specified in
`docs/specs/0271-a-script-walks-the-reader-through-the-blob.md`.

`--descriptor-set` is not optional. Half the anomalies are *terminal*: once a
varint has no terminator or a wire type has no meaning, the decoder cannot find
the next tag and gives up on the rest of the enclosing region. Each one is
therefore wrapped in its own `message_type` submessage, whose length prefix
bounds the damage — and the decoder only descends into a submessage when the
schema says it is one. Under `--raw` those submessages stay opaque strings and
the anomalies inside them are never seen.

Any descriptor set containing `descriptor.proto` works;
`prototext-core/fixtures/descriptor.pb` is used above because it is in the
repo. `prototext` needs none at all — it resolves `google.protobuf.*` on its
own:

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
  disk**, not of the payload it encodes to. The 10 KB here becomes about 1.6 KB
  of wire bytes.
- The SPDX header lives in `anomalies.pb.license` rather than in the file: the
  format detection is a `starts_with`, so the first bytes must be
  `#@ prototext:`.

## What it shows, in order

The file is read top to bottom during a talk, so it is ordered by what an
audience gets the most from rather than by severity:

1. **Legal bytes no canonical writer produces** — `tag_ohb`, `len_ohb`,
   `val_ohb`. Varints may carry padding, so a tag, a length prefix and a value
   can each be written longer than they need to be. Every parser accepts them.
2. **Values that survive a round trip but not a re-encode** — `truncated_neg`,
   `nan_bits`. Two producers disagree and both are right.
3. **A newer producer against an older schema** — `ENUM_UNKNOWN`, plus four
   fields the schema does not declare at all, rendered by wire type
   (`varint`, `fixed64`, `fixed32`, `bytes`).
4. **A packed run** — `pack_size`, `ohb`, `neg`. Three text lines, one wire
   record; the place where the <kbd>w</kbd> row earns its keep.
5. **Blob and schema disagreeing** — `TYPE_MISMATCH`, `INVALID_STRING`,
   `INVALID_PACKED_RECORDS`. Still parseable; the scan continues.
6. **Malformed wire bytes** — `TRUNCATED_BYTES`/`MISSING`, `INVALID_VARINT`,
   `INVALID_LEN`, `INVALID_FIXED64`, `INVALID_FIXED32`, `INVALID_TAG_TYPE`,
   `TAG_OOR`. One submessage each, for the reason given above.
7. **Groups** — `group`, `etag_ohb`, `END_MISMATCH`, `ETAG_OOR`, `OPEN_GROUP`,
   `INVALID_GROUP_END`. Last, because proto2 groups are history to most of the
   audience and nothing here declares one, so they all arrive as undeclared
   fields.

Each anomaly is explained twice: by a `#` comment for a reader with the file
open in an editor, and by a **string field value** for the audience looking at
protolens. The comments are dropped by the encoder and never reach the wire;
the string values are the wire.

## Keeping it honest

`prototext-core/tests/anomaly_fixture.rs` encodes the fixture, re-renders it
and asserts that re-encoding the rendering gives back the same bytes, and that
the set of annotation tokens in the rendering **equals** the vocabulary. A
missing token means the fixture stopped covering something; an extra one means
the renderer grew a token nobody classified.

Specified in `docs/specs/0226-a-fixture-shows-every-anomaly.md`.
