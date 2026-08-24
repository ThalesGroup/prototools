<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0352 — NodeKind replaces is_message

Status: implemented
Implemented in: 2026-08-24
App: prototext-core, protolens
Refs: docs/specs/0212-the-span-is-a-third-as-wide.md (NodeSpan layout and
        the 32-byte size pin),
      docs/specs/0343-wire-and-label.md (wire_and_label byte, the packing
        that freed the byte this spec reuses)

## Background

`NodeSpan::is_message: bool` distinguishes nested message/group nodes from
scalar leaves. It does not distinguish the three readings a schema-blind
`WT_LEN` field admits: the renderer picks one of `message`, `string`, or
`bytes` based on a parse probe and a UTF-8 check, but that decision is not
surfaced back to protolens. The only field in the span that hints at it is
`wire_and_label`, which carries `WT_LEN` for all three.

The consequence: when the user presses `t` on a primitive node inside an
untyped `message` (no FQDN on the parent), `natural_type` returns `None`
(no schema to consult), and the override pane opens on `none` — the lexico
fallback row 0 — instead of on `string` or `bytes`, whichever the renderer
actually chose.

## Goals

- **G1.** Replace `is_message: bool` with `kind: NodeKind` — a `u8`-repr
  enum that occupies the same one byte and carries no size penalty.
- **G2.** `NodeKind` covers every wire-type reading the renderer can
  produce, with `WT_LEN` split into three variants matching the renderer's
  own cascade order (spec 0097): `Message`, `String`, `Bytes`.
- **G3.** `NodeKind::Message` subsumes both `WT_LEN`-framed messages and
  `WT_START_GROUP`-framed groups — the framing is already in
  `wire_and_label`; `kind` answers "structural shape", not "wire framing".
- **G4.** Every existing `span.is_message` call site in protolens migrates
  to `span.kind == NodeKind::Message` with identical semantics.
- **G5.** `toggle_override`'s Step C uses `span.kind` to derive a
  non-`None` prefill for schema-blind primitive nodes: `NodeKind::String`
  → `"string"`, `NodeKind::Bytes` → `"bytes"`, `NodeKind::Varint` →
  `"varint"` (no override keyword exists; Step C falls to default),
  etc. — so `t` on a `string` node opens on `string`, not `none`.

## Non-goals

- **N1.** *Encoding schema-declared types in `NodeKind`.* A declared
  `int32` and a schema-blind varint are both `NodeKind::Varint`. Schema
  information is available via `type_fqdn` + the pool; duplicating it in
  `kind` would require keeping two sources in sync.
- **N2.** *Splitting `Message` by framing.* Groups already set
  `is_message: true`; the distinction between LEN-framed and
  START_GROUP-framed messages is already in `wire_and_label`. Splitting
  `NodeKind::Message` into `Message` and `Group` would add a variant for
  no consumer benefit.
- **N3.** *Changing the 32-byte size pin.* `NodeKind` is a `u8`-repr enum
  in the same byte as `is_message`. The pin in `sink.rs` stays at 32.

## Specification

- **S1.** Define in `prototext-core`:

  ```rust
  /// The structural reading the renderer chose for this node.
  ///
  /// Replaces `is_message: bool` (spec 0352). One byte, `#[repr(u8)]`,
  /// so `NodeSpan`'s 32-byte layout is unchanged.
  ///
  /// For `WT_LEN` nodes the renderer tries, in order (spec 0097):
  /// message parse → UTF-8 check → opaque bytes. The variant records
  /// which rung succeeded, giving consumers the renderer's conclusion
  /// without re-doing the work.
  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  #[repr(u8)]
  pub enum NodeKind {
      /// `WT_VARINT` — a base-128 integer.
      Varint,
      /// `WT_I32` — four bytes, little-endian.
      Fixed32,
      /// `WT_I64` — eight bytes, little-endian.
      Fixed64,
      /// `WT_LEN`, payload is valid UTF-8 (and did not parse as a message).
      String,
      /// `WT_LEN`, payload is opaque bytes (not valid UTF-8, not a message).
      Bytes,
      /// `WT_LEN` parsed as a nested message, or `WT_START_GROUP`.
      Message,
  }
  ```

- **S2.** Replace `pub is_message: bool` with `pub kind: NodeKind` in
  `NodeSpan`. The size assertion at 32 bytes must still pass.

- **S3.** Every site in `prototext-core` that wrote `is_message: true`
  writes `kind: NodeKind::Message`. Every site that wrote
  `is_message: false` writes the appropriate variant based on the wire type
  and the renderer's actual decision at that point:
  - scalar fields with schema (`Kind::String` → `NodeKind::String`,
    `Kind::Bytes` → `NodeKind::Bytes`, all varint kinds →
    `NodeKind::Varint`, `Kind::Float`/`Kind::Fixed32`/`Kind::Sfixed32` →
    `NodeKind::Fixed32`, `Kind::Double`/`Kind::Fixed64`/`Kind::Sfixed64`
    → `NodeKind::Fixed64`);
  - schema-blind LEN field: `NodeKind::String` if `from_utf8` succeeds,
    `NodeKind::Bytes` otherwise (the message probe already failed, having
    reached the scalar path);
  - malformed nodes: use the wire type the tag claimed
    (`WT_VARINT`→`Varint`, `WT_LEN`→`Bytes`, `WT_I32`→`Fixed32`,
    `WT_I64`→`Fixed64`, anything else→`Bytes` as a safe fallback).

- **S4.** In protolens, migrate every `span.is_message` read:
  - `span.is_message` → `span.kind == NodeKind::Message`
  - `!span.is_message` → `span.kind != NodeKind::Message`
  - `is_message: true` / `is_message: false` field initialisers →
    `kind: NodeKind::Message` / the appropriate variant.

- **S5.** In `toggle_override` (Step C, `override_select.rs`), extend the
  non-message branch. Currently:

  ```rust
  } else {
      self.natural_type(self.cursor).map(Some)
  }
  ```

  Change to: if `natural_type` returns `Some`, use it as before. If it
  returns `None`, derive a keyword from `span.kind`:
  - `NodeKind::String` → `Some(Some("string".to_string()))`
  - `NodeKind::Bytes`  → `Some(Some("bytes".to_string()))`
  - `NodeKind::Varint` → `Some(Some("int32".to_string()))` (first entry
    from `override_keywords_for_wire_type(WT_VARINT)`)
  - `NodeKind::Fixed32` → `Some(Some("fixed32".to_string()))` (first
    entry from `override_keywords_for_wire_type(WT_I32)`)
  - `NodeKind::Fixed64` → `Some(Some("fixed64".to_string()))` (first
    entry from `override_keywords_for_wire_type(WT_I64)`)
  - `NodeKind::Message` cannot reach here (handled by the `is_message`
    branch above).

  **Erratum (2026-08-24):** the original S5 said `Varint`/`Fixed32`/
  `Fixed64` → `None` ("the default path is correct"). In practice,
  `open_override_on_default` opens in `Inferred` mode, and the scorer
  returns message-type candidates for those byte ranges (the payload
  bytes are often accidentally parseable as protobuf fields). The
  highlighted candidate is wire-incompatible, and the live preview fires
  a TYPE_MISMATCH annotation immediately. The fix: supply the first
  wire-compatible keyword so `open_override_on_type` falls through to
  lexicographic mode (primitive keywords are never in the inferred list).
  The user can still press `i` to reach inferred mode and pick a message
  FQDN if they believe the bytes represent a nested message.

## Alternatives considered

**Re-doing the UTF-8 check in protolens.** Protolens has the blob and
`raw_range`, so it could call `std::str::from_utf8` itself. This
duplicates the renderer's work and risks drift if the renderer's heuristic
ever changes. Recording the decision in the span is the single source of
truth.

**A separate `shape: Option<Shape>` field.** `Shape` already exists and
covers the vocabulary. But adding a ninth field to `NodeSpan` would break
the 32-byte pin (spec 0212 S8 makes the pin an equality, not a bound).
`NodeKind` achieves the same information in the existing `is_message` byte.

**Keeping `is_message` and adding a `is_string: bool` alongside it.**
Would require a tenth byte, again breaking the pin, and would leave three
booleans with a nonsensical combination (`is_message && is_string`).

## Test plan

1. `node_kind_size_unchanged` — `size_of::<NodeSpan>()` is still 32 after
   the change (the existing assertion already provides this).
2. `schema_blind_utf8_len_is_string` — a schema-blind LEN node whose
   payload is valid UTF-8 gets `kind: NodeKind::String`.
3. `schema_blind_non_utf8_len_is_bytes` — a schema-blind LEN node with
   non-UTF-8 payload gets `kind: NodeKind::Bytes`.
4. `schema_blind_message_len_is_message` — a schema-blind LEN node that
   parses as a nested message gets `kind: NodeKind::Message`.
5. `group_node_is_message` — a `WT_START_GROUP` node gets
   `kind: NodeKind::Message`.
6. `t_on_string_node_opens_on_string` — pressing `t` on a schema-blind
   `string` node inside an untyped `message` opens the override pane with
   `string` highlighted, not `none`.
7. `t_on_bytes_node_opens_on_bytes` — same for a `bytes` node.
8. `t_on_fixed64_node_opens_on_fixed64` — pressing `t` on a schema-blind
   `fixed64` node opens in lexicographic mode with `fixed64` highlighted,
   not on an inferred message candidate.
9. `t_on_fixed32_node_opens_on_fixed32` — same for a `fixed32` node.
10. `t_on_varint_node_opens_on_int32` — pressing `t` on a schema-blind
    `varint` node opens in lexicographic mode with `int32` highlighted.

## Measured outcome

`NodeKind` defined in `prototext-core/src/serialize/render_text/sink.rs`,
exported from `render_text/mod.rs` and `lib.rs`. `NodeSpan::is_message:
bool` replaced by `NodeSpan::kind: NodeKind` — same 32-byte size (assertion
still passes). `IndexMark::is_message` similarly replaced. `scalar_kind`
derived before `value` is moved in `IndexingTextSink::scalar_field`,
applying the same UTF-8 check the inner sink uses for schema-blind LEN
nodes. `toggle_override` Step C extended with a `span.kind`-based fallback
for `String`, `Bytes`, `Varint`, `Fixed32`, and `Fixed64` nodes when
`natural_type` returns `None`. The `Varint`/`Fixed32`/`Fixed64` cases
open in lexicographic mode on the first wire-compatible keyword (`int32`,
`fixed32`, `fixed64` respectively), preventing the TYPE_MISMATCH preview
that the original `None` → `open_override_on_default` path produced.

15 `is_message` field reads and 8 field initialisers migrated across
protolens (decode.rs, extract.rs, override_apply.rs, command_line.rs,
heat_cue.rs, override_display.rs, override_select.rs, popup.rs,
shadow_sweep.rs, wire.rs, and 9 test files). 1 test assertion in
prototext-core's mod.rs tests migrated.

1218 protolens tests, 25 theme tests, 3 batch tests, 134 prototext-core
tests — all pass.
