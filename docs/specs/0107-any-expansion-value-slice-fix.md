<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0107 — Fix `Any`-expansion scoring recursion to use the `value` slice

**Status:** implemented
**Implemented in:** 2026-07-07
**Refs:** `docs/specs/0089-any-expansion.md` §6, §9
**App:** prototext-graph

---

## Background

Spec 0089 §6 documents that when the scoring walk expands a `google.protobuf.Any`
field, it must recurse into the **`value` sub-field's bytes only**:

```rust
score_message_multi(value_payload, 0, any_active, None, ws);
```

The actual implementation in `prototext-graph/src/score/walk.rs` diverges from
this: it recurses using `payload`, which is the entire `Any`-encoded body
(`type_url` field 1 + `value` field 2 together), not the isolated `value`
bytes:

```rust
// walk.rs:981 (current, buggy)
score_message_multi(payload, 0, any_active, None, ws);
```

This means the resolved type's schema is scored starting at byte 0 of the
raw `Any` body, so its own field 1 is matched against the `type_url` LEN
tag/bytes instead of against the real first field of the wrapped message.

This was discovered while investigating a real-world false-positive
`[veto] proxy.Sample — wire-type mismatch on field 1 (wire_type=2)`.
The `Sample.header` field is a `google.protobuf.Any` that resolves to
`Request`, whose real field 1 is `optional int32 type = 1` (VARINT). The
misread `type_url` tag (field 1, wire type `LEN`) collides with the expected
VARINT wire type, producing an unconditional `Verdict::Mismatch` — the
observed veto.

Root-cause was confirmed by hand-constructing minimal wire payloads locally
(against `/tmp/pdb.desc` / `/tmp/protodb`) and bisecting: a bare `Any` wrapping
a completely empty `Request` still vetoes, proving the bug is purely
positional/mechanical and unrelated to `Request`'s or
`bridge.MessageSet`'s content (the originally-reported MessageSet issue
is a separate, unrelated bug — deferred to a future spec).

The bug has two manifestations, both confirmed:

- **Hard veto**: when the resolved type's real field 1 is not `LEN`-typed
  (e.g. `VARINT`, `I32`, `I64`), the wire-type mismatch is unconditional →
  `Verdict::Mismatch` → veto.
- **Silent corruption**: when the resolved type's real field 1 happens to be
  `LEN`-typed (`string`/`bytes`/`message`), the misread `type_url` tag
  coincidentally "matches" as a bogus field-1 hit, and the real payload bytes
  are then mis-parsed as an unrecognized field 2 → the entire real content is
  swallowed as one opaque `unknowns += 1` blob, never actually scored. No
  veto, but the score is silently wrong.

Spec 0089's own "Testing" section names planned scoring-walk regression tests
`TC-S1`..`TC-S6`, none of which were ever implemented — `prototext-graph/src/score/tests.rs`
currently contains zero `Any`-expansion tests. This explains why the bug has
gone undetected since spec 0089 was implemented (2026-05-29).

---

## Goals

1. Add a helper `extract_any_value(any_bytes: &[u8]) -> Option<&[u8]>` in
   `walk.rs`, mirroring the existing `extract_type_url`, that scans an
   `Any`-encoded buffer and returns the raw byte slice of field 2 (`value`,
   `WT_LEN`), or `None` if the field is absent or the buffer is malformed.
2. Fix the recursive call in `score_message_multi`'s `Any`-expansion branch
   (`walk.rs:981`) to recurse into the extracted `value` slice instead of the
   whole `payload`.
3. Add regression tests to `prototext-graph/src/score/tests.rs` that
   reproduce both bug manifestations (hard veto, silent corruption) prior to
   the fix, and pass cleanly after it.

## Non-goals

- The `bridge.MessageSet` / `request_extensions` scoring issue from
  the original bug report — unrelated, deferred to a separate future spec.
- The `value`-before-`type_url` wire-ordering fallback (`scan_any_fields`
  returning `None`, raw LEN rendering) — already correct, untouched by this
  fix.
- Renderer-side (`prototext-core`) `Any` expansion — already correct and
  covered by spec 0095; unaffected by this fix (it is a different code path).
- Retrofitting the full `TC-S1`..`TC-S6` test suite from spec 0089 — only the
  tests needed to cover this specific regression are added here.

---

## Specification

### §1 — `extract_any_value`

Add immediately after `extract_type_url` (around `walk.rs:623`):

```rust
/// Extract the raw bytes of the `value` sub-field (field 2, `WT_LEN`) from
/// an `Any`-encoded buffer. Mirrors `extract_type_url` but returns the raw
/// slice rather than interpreting it as UTF-8, and scans for field 2 instead
/// of field 1.
fn extract_any_value(any_bytes: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    let buflen = any_bytes.len();
    while pos < buflen {
        let tag = parse_wiretag(any_bytes, pos);
        if tag.garbage.is_some() {
            return None;
        }
        let field_number = tag.field_number;
        let wire_type = tag.wire_type;
        pos = tag.next_pos;
        match wire_type {
            WT_VARINT => {
                let vr = parse_varint(any_bytes, pos);
                if vr.garbage.is_some() {
                    return None;
                }
                pos = vr.next_pos;
            }
            WT_I64 => {
                if pos + 8 > buflen {
                    return None;
                }
                pos += 8;
            }
            WT_LEN => {
                let lr = parse_varint(any_bytes, pos);
                if lr.garbage.is_some() {
                    return None;
                }
                pos = lr.next_pos;
                let len = lr.value as usize;
                if pos + len > buflen {
                    return None;
                }
                let payload = &any_bytes[pos..pos + len];
                pos += len;
                if field_number == 2 {
                    return Some(payload);
                }
            }
            WT_START_GROUP => {
                pos = parse_group_blind(any_bytes, pos, field_number)?;
            }
            WT_I32 => {
                if pos + 4 > buflen {
                    return None;
                }
                pos += 4;
            }
            _ => return None,
        }
    }
    None
}
```

An empty `value` (`value: ""`, i.e. field 2 present with zero-length payload)
returns `Some(&[])`, matching protobuf semantics (an empty message and an
absent optional message are wire-indistinguishable for `LEN` fields).

### §2 — Fix the recursion site

At `walk.rs:975-989`, change the `Some(root_state)` arm to extract and use the
`value` slice:

```rust
match resolved_state {
    Some(root_state) => {
        let value_payload = extract_any_value(payload).unwrap_or(&[]);
        // Recurse into the wrapped type: replace state_id 0
        // with the resolved root entry state; keep entry indices.
        let any_active =
            group_by_state(any_pairs.iter().map(|&(_, e)| (root_state, e)));
        score_message_multi(value_payload, 0, any_active, None, ws);
    }
    None => { /* unchanged */ }
}
```

If `value` is absent (`extract_any_value` returns `None`, e.g. an `Any` with
only `type_url` set), recurse with an empty slice — consistent with how the
walker already scores an empty buffer against an active entry set elsewhere
(fields simply remain unseen / `Verdict::Unknown` is never reached because
there are no bytes to walk).

### §3 — Regression tests

Add to `prototext-graph/src/score/tests.rs`, in a new section
`// ── Any expansion (spec 0107) ──`.

#### Fixture: `make_merged_any()`

A second `Merged` builder, parallel to `make_merged()`, with:

- `Wrapper` (root): one field, `number: 1`, `kind: ScoringKind::Node`,
  `child: Some("google.protobuf.Any".to_string())`, `label: Optional`.
- `TargetVarint` (root): one field, `number: 1`, `kind: ScoringKind::Uint32`,
  `child: None`, `label: Optional`. (Real field 1 is VARINT — reproduces the
  hard-veto manifestation, matching `Request.type`.)
- `TargetString` (root): one field, `number: 1`, `kind: ScoringKind::LenString`,
  `child: None`, `label: Optional`. (Real field 1 is LEN — reproduces the
  silent-corruption manifestation, matching `Request` → ... →
  `proxy.InfoRequest`'s `string value = 1` shape.)

`roots: vec!["Wrapper".into(), "TargetVarint".into(), "TargetString".into()]`.

No entry for `"google.protobuf.Any"` itself is needed in `states` — block ID
0 is pre-allocated by `graph::build` (spec 0089 §9) regardless.

#### Wire fixture helper: `any_bytes(type_url: &str, value: &[u8]) -> Vec<u8>`

```rust
fn any_bytes(type_url: &str, value: &[u8]) -> Vec<u8> {
    let mut b = field_len(1, type_url.as_bytes());
    b.extend(field_len(2, value));
    b
}
```

#### TC-S1: `any_expansion_recurses_into_value_only_varint`

- `value` = `field_varint(1, 42)` (i.e. `TargetVarint { field1: 42 }`).
- Wire: `Wrapper { field1: any_bytes("type.googleapis.com/TargetVarint", value) }`.
- Score entry `"Wrapper"`.
- Assert: `vetoed == false`, `mismatches == 0`.
- Before the fix: `vetoed == true` (wire-type mismatch on field 1) — this
  test must fail on the pre-fix code and pass after.

#### TC-S2: `any_expansion_recurses_into_value_only_string`

- `value` = `field_len(1, b"hello")` (i.e. `TargetString { field1: "hello" }`).
- Wire: `Wrapper { field1: any_bytes("type.googleapis.com/TargetString", value) }`.
- Score entry `"Wrapper"`.
- Assert: `matches == 2` (1 for `Wrapper.field1` itself being a valid
  Any-typed LEN field, + 1 for the recursed `TargetString.field1` real
  string match), `unknowns == 0`.
- Before the fix: `unknowns == 1` (the whole `value` bytes, including its
  own field-2 LEN tag, mis-parsed as an unrecognized field on the outer
  `type_url`+`value` body) and the true field-1 match is never recorded —
  this test must fail on the pre-fix code and pass after.

#### TC-S3: `any_expansion_empty_value`

- `value` = `&[]` (empty).
- Wire: `Wrapper { field1: any_bytes("type.googleapis.com/TargetVarint", value) }`.
- Score entry `"Wrapper"`.
- Assert: `vetoed == false` (recursing into an empty slice must not panic or
  veto — regression guard for the `extract_any_value` → `None`/empty-slice
  fallback path in §2).
- Before the fix: also vetoes (the recursion still walks the full `type_url`
  + empty-`value` body against `TargetVarint`'s VARINT field 1) — this test
  must fail on the pre-fix code and pass after.

Each test follows the existing pattern: `build_graph()`-equivalent using
`make_merged_any()`, then `score_entry(&pb, &g, "Wrapper")`.

---

## Files changed

| File | Change |
|---|---|
| `prototext-graph/src/score/walk.rs` | Add `extract_any_value`; fix recursion in `score_message_multi`'s `Any` branch to use the extracted `value` slice |
| `prototext-graph/src/score/tests.rs` | Add `make_merged_any()`, `any_bytes()`, and tests TC-S1, TC-S2, TC-S3 |

---

## Gaps and follow-up work

- The original `bridge.MessageSet` / `request_extensions` scoring
  issue remains open — tracked separately, not addressed by this spec.
- Spec 0089's `TC-S3`..`TC-S6` (unresolved type_url, non-root wrapped type,
  etc.) remain unimplemented; out of scope here since they don't cover this
  regression.
