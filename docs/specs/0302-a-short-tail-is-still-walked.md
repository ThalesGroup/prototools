<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0302 — a short tail is still walked

Status: implemented
Implemented in: 2026-08-15
App: prototext-core, protolens
Refs: docs/specs/0216-….md (the maximal arena walk; S2/S14, descent is
        unconditional; S4, a malformed region is a node like any other),
      docs/specs/0299-….md (the `message` override keyword; S1, the
        schema-free synthetic descriptor)

## Background

Spec 0299 added `message` as an override keyword so a user can open an
untyped blob — or any LEN field that the probe refused — as a schema-free
message. That works for every well-formed LEN field. It fails for a
`TRUNCATED_BYTES` field.

A `TRUNCATED_BYTES` field's declared length extends past the end of the
buffer. The arena walk produces a **leaf** for it: `ArenaSink::malformed`
receives the call and pushes one flat node, with no children. When the user
then overrides that node as `message`, `decode_and_render_indexed` decodes
the available bytes and produces child spans. `overlay_spans` asserts that
every child span has a corresponding arena slot — and panics, because no
children were allocated.

The mismatch is a gap in spec 0216's "maximality" claim (S2/S14):

> A payload judged not to be a message is one that a schema — or a user
> override, which is the point of the app — could still declare one, and
> the render would then need nodes the decomposition never created.

That reasoning applies exactly to a TRUNCATED_BYTES field: its available
bytes may contain perfectly valid sub-fields, and a `message` override
needs slots for them.

**The gRPConf demo trigger.** Opening `grpconf/stage/boblog` untyped,
overriding the root (`/`) as `message`, then navigating to the last entry
(field 9): the entry was cut short by 1 024 bytes and shows as
`TRUNCATED_BYTES`. Overriding `/9` as `message` to inspect its partial
contents panics the process.

## Goals

- **G1.** A `message` override on a `TRUNCATED_BYTES` field succeeds and
  renders the available bytes as a schema-free message — the same content
  the preview already showed.
- **G2.** The arena walk remains the single source of truth for which nodes
  can exist. No new "second chance" path is added to `overlay_spans` or to
  the splice machinery.

## Non-goals

- **N1.** Do not change the `TRUNCATED_BYTES` annotation or its rendered
  appearance on the original (un-overridden) node. The text shown when the
  user has not applied an override is unchanged.
- **N2.** Do not attempt to recover the declared-but-missing bytes. The
  override opens what is present; nothing invents what is not.
- **N3.** Do not change `ProbeSink`. A `TRUNCATED_BYTES` field still
  disqualifies its parent from being a message (spec 0266). The override
  is the reader's explicit decision to proceed anyway.

## Specification

- **S1.** `ArenaSink::malformed` in `prototext-core/…/arena.rs` gains a
  special case for `MalformedKind::TruncatedBytes`. Instead of pushing a
  leaf, it:
  1. Claims a slot for the node (as `begin_nested` does), setting
     `raw_start` to the field's tag byte and leaving `raw_end` to be
     backpatched.
  2. Adjusts `raw_base`, `parent`, and `depth` to enter the payload frame.
     The payload starts at `raw_range.end - raw.len()` within the current
     frame, where `raw` is the available payload bytes passed to `malformed`
     by `render_message`.
  3. Calls `render_message(raw, 0, None, None, false, self)` to walk the
     available bytes exactly as any other nested message is walked. Children
     found there receive their own arena slots.
  4. Restores `raw_base`, `parent`, and `depth`, then backpatches `raw_end`
     to the end of the field's declared range.
  If `raw` is empty (zero available payload bytes), no children can exist;
  the leaf path is taken unchanged.

- **S2.** No change to `slots_for_spans`, `overlay_spans`, or
  `splice_override`. The fix is entirely in the arena-build phase; the
  splice path already works once the arena contains the right slots.

- **S3.** The `none` keyword and the `message` keyword in protolens's
  override pane are stored as bare keyword strings (not FQDNs) in
  `override_candidates`. `render_node_as` intercepts `none` as `(None,
  None)`. The `message` keyword resolves via `wrapper_target_for` to the
  schema-free synthetic descriptor (spec 0299 S1).

  `status_type_label` in `override_display.rs` handles `message` in the
  non-`is_message` branch (for a node where the override is stored but the
  commit has not yet produced a message span — e.g., TRUNCATED_BYTES before
  this spec's arena fix, or during a preview). Returns
  `("message", Some("message"))` rather than falling through to
  `format_fqdn_label` and producing `.message [enum]`.

- **S4.** The committed `none` keyword in `override_select.rs` and in
  `render_node_as` is the literal string `"none"` (lowercase), matching
  all other override keywords. The old `"protolens_internal.None"` FQDN
  sentinel is replaced everywhere. Tests are updated accordingly.

## Alternatives considered

### Reframe the length varint on the commit path

Built and reverted. `truncate_interior` was reused with `budget =
field_bytes.len()`, which compares the total field length against the
payload length and always returns `None` — no rewrite happens. A purpose-
built `reframe_to_actual_length` (replacing the bogus declared length with
the actual payload length) would fix the decode, but the decoded render
still produces children with no arena slots, causing the same `overlay_spans`
panic. Root cause: the fix must be in the arena, not in the splice path.

### Allow `overlay_spans` to create new slots on demand

Rejected: it would break the invariant that the arena is the only source
of structural truth, and it would require placeholder slots to be inserted
at arbitrary positions into the level-order layout — a complex rewrite of
a data structure whose correctness spec 0216 carefully establishes.

### Block the `message` override on TRUNCATED_BYTES nodes

Considered. The rejection check in `:override` could detect `TRUNCATED_BYTES`
and refuse the commit. But the preview already produces the right content;
refusing the commit is a worse user experience than fixing the arena. The
arena fix is localized and the invariant it extends is already stated in
spec 0216's commentary.

## Test plan

1. `truncated_bytes_arena_has_children` — build the arena for a blob whose
   last LEN field is truncated; assert the TRUNCATED_BYTES node has at
   least one child slot in the arena.
2. `message_override_on_truncated_bytes_commits` — construct an `App` with
   a truncated blob, override the TRUNCATED_BYTES node as `message`, assert
   the commit succeeds and the node becomes bracketed (`is_message = true`).
3. `status_line_shows_message_not_enum` — for a node with an active
   `message` override but `is_message = false` (preview state or TRUNCATED_BYTES
   before the arena fix), `status_type_label` returns `("message",
   Some("message"))`.
4. `none_keyword_is_lowercase` — `override_candidates[0]` is `"none"` and
   `render_node_as` on that candidate produces a raw render (no splice).

## Measured outcome

All four tests from the test plan pass. Overriding a TRUNCATED_BYTES node
as `message` now commits without panicking and renders the available bytes
as nested content. The `grpconf/stage/boblog` trigger (field 9, cut short
by 1 024 bytes) no longer crashes the process.
