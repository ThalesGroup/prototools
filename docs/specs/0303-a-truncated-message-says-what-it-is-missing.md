<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0303 — a truncated message says what it is missing

Status: implemented
Implemented in: 2026-08-16
App: prototext-core, protolens
Refs: docs/specs/0302-….md (spec 0302 S1: the arena descends into a
        TRUNCATED_BYTES field's available bytes, giving the arena child
        slots that a `message` override can fill),
      docs/specs/0299-….md (spec 0299: `message` as an override target;
        the zero-field synthetic descriptor; `reframe_to_actual_length`
        on the commit path),
      docs/specs/0241-….md (the real-call fixture `boblog`; field 9 is
        truncated — the gRPConf trigger for spec 0302)

## Background

Spec 0302 lets a user override a `TRUNCATED_BYTES` node as `message`.
After the commit the node becomes a proper bracketed message and its
available bytes are shown as children. What is lost in the process is
the truncation information: the original render had

```
9: "\n\014…"  #@ TRUNCATED_BYTES; MISSING: 1024
```

After the override commits, the node renders as

```
9 {  #@ message = 9
  12: …
  …
}
```

The `MISSING: 1024` annotation is gone. Exporting this as binary is
safe: `extract_binary` reads `blob[raw_range]` — the original arena
bytes — so the declared length is preserved verbatim regardless of any
override. But exporting as prototext is not safe: the encoder sees

```
9 {  #@ message = 9
```

and opens a `Frame::Message`, which at close time writes the varint for
the *actual content length* — which is 1024 bytes less than the
original declared length. A consumer that re-encodes from prototext and
compares byte-for-byte will find a discrepancy.

**Why not groups.** Groups have no declared length; their extent is
determined by parsing through to the matching `WT_END_GROUP` tag. A
group whose content is cut short produces `OPEN_GROUP`, which already
says "the end tag was never found." There is no `declared - actual` gap
to measure, and no "TRUNCATED_GROUP" to emit. The fix is LEN-only.

### Relationship to `prototext decode`

`prototext decode` without `--raw` never descends into a
`TRUNCATED_BYTES` node (spec 0266 / `ProbeSink` disqualifies it), so
the rendered output for a well-formed document never contains
`TRUNCATED_MESSAGE`. The annotation only appears when a field that
prototext-core would otherwise render as a flat bytes node is instead
opened by a `message` override — which is a protolens-only operation.

A future scoring tier that penalises truncation instead of vetoing it
(spec 0238 N6, deferred) would create a path where `prototext decode`
itself descends. In that case `TRUNCATED_MESSAGE` would appear in raw
decoder output. The spec is written to accommodate this without
requiring a second pass: the renderer, not the caller, decides whether
to emit the annotation.

## Goals

- **G1.** When the renderer opens a `TRUNCATED_BYTES` field as a
  message (after a `message` override commits), the header line carries
  `TRUNCATED_MESSAGE; MISSING: N` so the missing byte count survives in
  the prototext output.
- **G2.** The prototext encoder reads `MISSING: N` from a
  `TRUNCATED_MESSAGE` header and inflates the length varint accordingly,
  making `export --prototext` followed by `prototext encode` reproduce
  the original declared length.
- **G3.** The grammar's `highlights.scm` recognizes `TRUNCATED_MESSAGE`
  as an `@annotation.invalid` keyword (same tier as `TRUNCATED_BYTES`).
- **G4.** `annotation.rs` in protolens explains `TRUNCATED_MESSAGE` in
  the hover popup.

## Non-goals

- **N1.** Do not change the binary export path. `extract_binary` already
  reads the original arena bytes and is correct.
- **N2.** Do not change `ProbeSink` or the scoring walk. A truncated
  field still disqualifies its parent from being a message under the
  probe (spec 0266). The annotation only appears when the user has
  explicitly overridden the field as `message`.
- **N3.** Do not invent the missing bytes. The committed render shows
  only what is present; `MISSING: N` is a count, not a placeholder.
- **N4.** Do not add a new `MalformedKind` variant for a truncated
  message. The arena's `TruncatedBytes` kind is already the right
  signal; the new annotation is a render-time decision by `TextSink`,
  not a new structural concept.
- **N5.** Do not change `decode_and_render` (the non-indexed,
  non-protolens path). `prototext decode --raw` uses it, and it never
  descends into truncated fields under the current probe rules. When
  spec 0238 N6's penalty tier arrives it can route through the same
  renderer machinery and gain the annotation for free.
- **N6.** Do not add `TRUNCATED_GROUP`. Groups have no declared length
  prefix; the concept has no well-defined `MISSING` value. `OPEN_GROUP`
  already covers the truncated-group case.

## Specification

### Renderer — `prototext-core`

- **S1.** `TextSink` gains a `missing_payload_bytes: Option<u64>` field,
  defaulting to `None`. It is consumed on the first `begin_nested` call
  and reset to `None` immediately, so it never leaks onto inner nodes.

  The caller that knows a field is truncated and is about to open it as
  a message is `render_node_as` (in protolens `override_apply.rs`): on
  the commit path, after `reframe_to_actual_length` rewrites the
  tag+length prefix, the missing count is the difference between the
  original declared length and the actual payload length, computed by
  `missing_bytes_for` in `preview_truncate.rs` before the reframe
  replaces `field_bytes`. How to expose this to the renderer is
  addressed in S3.

- **S2.** `TextSink::begin_nested` takes the value of
  `self.missing_payload_bytes` on entry (clearing it). For
  `NestedKind::Message`, if that value is `Some(n)`, the header line
  gains `TRUNCATED_MESSAGE; MISSING: N` after the standard annotation
  tokens. For `NestedKind::Group`, the value is ignored — groups have no
  declared length, so the annotation is meaningless there.

  The format follows the existing `TRUNCATED_BYTES` convention:
  - `TRUNCATED_MESSAGE` is appended after `{  #@` and the field_decl
    or wire-type token.
  - `MISSING: N` follows, separated by `;`.

  Example rendered output for a truncated LEN field after a `message`
  override:
  ```
  9 {  #@ message = 9; TRUNCATED_MESSAGE; MISSING: 1024
    12: …
  }
  ```

- **S3.** The signal flows from `render_node_as` to `TextSink` via the
  existing `DecodeRenderOpts` struct. A new optional field
  `missing_payload_bytes: Option<u64>` is added to `DecodeRenderOpts`.
  `decode_and_render_indexed` reads it and calls
  `sink.set_missing_payload_bytes(n)` before `render_message`. This
  avoids threading a new parameter through the entire render call stack.

  `render_node_as` computes the value from the original `field_bytes`
  (before `reframe_to_actual_length` replaces them) using
  `missing_bytes_for(&field_bytes)`: the original declared length minus
  the actual payload length. `None` when the field is not truncated.
  Only the commit path sets this; the preview path leaves it `None`.

- **S4.** No change to `overlay_spans`, `slots_for_spans`, or any
  arena structure. The annotation is purely a text-level addition to the
  header line; the span's `is_message` flag and child count are
  unaffected.

### Encoder — `prototext-core`

- **S5.** `Ann` (in `encode_annotation.rs`) gains a boolean field
  `truncated_message: bool`. `parse_annotation` sets it when it
  encounters `TRUNCATED_MESSAGE` as a bare token (same path as
  `OPEN_GROUP`). The existing `missing_bytes_count` field already
  handles `MISSING: N`.

- **S6.** The open-brace handler in `encode_text/mod.rs` checks
  `ann.truncated_message` when pushing a `Frame::Message`. When true, it
  stores `ann.missing_bytes_count` as `Frame::Message::missing`.

  `Frame::Message` gains an optional `missing: Option<u64>` field. At
  close time it is passed to `fill_placeholder` as a new `missing_extra`
  parameter, which adds it to the compacted content length before
  encoding the varint — so the declared length in the re-encoded binary
  matches the original pre-truncation wire size.

  It must be a separate addend rather than a reduction of `acw`. `acw`
  is placeholder *waste* — a handful of bytes — and `fill_placeholder`
  writes `child_len_raw - frame_acw`; subtracting a four-digit `missing`
  from it saturates at zero and writes the raw length instead of the
  inflated one. The waste that propagates to the parent frame is
  unaffected by `missing_extra`: the parent accounts for the bytes this
  field actually occupies in the buffer, not for the length it declares.

### Grammar and highlighting

- **S7.** `reproto/tree-sitter-textproto/highlights.scm`: add
  `"TRUNCATED_MESSAGE"` to the `@annotation.invalid` list alongside
  `"TRUNCATED_BYTES"`.

### protolens annotation vocabulary

- **S8.** `protolens/src/annotation.rs`: add `"TRUNCATED_MESSAGE"` to
  the `INVALID` array and to the `clause` match, with explanation:
  `"the declared length runs past the end of the message; the available
  bytes are shown"`.

## Alternatives considered

### Carry `missing` on the span rather than on `DecodeRenderOpts`

A `missing_payload_bytes: Option<u64>` field on `Span` would make the
information structurally available at any layer. Rejected: `Span` is in
`prototext-core`'s public API, the field would be present for every
span (almost always `None`), and it would couple the codec's data model
to a piece of UI-level information that is only meaningful for one
specific user action.

### Emit the annotation in `splice_override` by patching the header line

`splice_override` already patches the field name in the header line (G2
in spec 0299). Rejected: the patch would duplicate the annotation
serialisation logic from `AnnWriter`, and the logic for deciding whether
to emit it would have to inspect the rendered text rather than the
domain state. Encoding it as a `DecodeRenderOpts` field keeps the
decision in the caller and the serialization in the renderer.

### Block `export --prototext` on nodes with active `message` overrides
on truncated fields

Considered. The simplest correctness guarantee is to refuse the export
and tell the user to use `--binary`. Rejected: `export --binary` is
already the safe path for the same reason (it reads the original arena
bytes), and protolens already documents it. Refusing prototext export
would be a surprise to users who have opened a truncated field as
`message` and edited nothing else — and it is avoidable.

### Add `TRUNCATED_GROUP`

Groups have no declared length prefix; the `MISSING` count has no
definition for a group. A group whose content is cut short is
`OPEN_GROUP`, not a new kind of truncation. Rejected before
implementation.

## Test plan

1. `truncated_message_header_carries_missing` — build an
   `IndexingTextSink` with `missing_payload_bytes` set, render a
   truncated LEN field opened as a message; assert the header line
   contains `TRUNCATED_MESSAGE; MISSING: N` with the correct `N`.
2. `encoder_inflates_length_for_truncated_message` — encode a prototext
   string whose header has `TRUNCATED_MESSAGE; MISSING: N`; assert the
   output length varint equals the actual content length plus `N`.
3. `export_prototext_roundtrip_on_truncated_blob` — open the
   `TRUNC_WITH_CHILDREN` fixture from spec 0302's tests, override the
   root as `message`, then the `TRUNCATED_BYTES` child as `message`;
   export as prototext; re-encode the prototext; assert the re-encoded
   binary has the same declared length varint as the original blob.
4. `highlights_include_truncated_message` — verify
   `highlights.scm` lists `TRUNCATED_MESSAGE` in `@annotation.invalid`.
5. `annotation_explains_truncated_message` — verify
   `annotation.rs`'s `clause` map has an entry for `TRUNCATED_MESSAGE`.
