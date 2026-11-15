<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0089 — google.protobuf.Any expansion in prototext decode and list-schemas

**Status:** implemented
**Implemented in:** 2026-05-29
**App:** prototext, scoring-graph

---

## Background

`google.protobuf.Any` is a well-known type that wraps an arbitrary protobuf
message alongside its type URL:

```proto
message Any {
  string type_url = 1;  // e.g. "type.googleapis.com/google.rpc.Status"
  bytes  value    = 2;  // serialised protobuf binary of the wrapped message
}
```

Today `prototext decode` renders `Any` as a plain two-field message — the
`type_url` string is shown verbatim and `value` is shown as escaped bytes —
because the renderer has no expansion logic.  This is technically correct but
unhelpful: the user sees an opaque byte blob instead of the human-readable
nested message.

`google.protobuf.Any` is part of the built-in WKT set in every variant (it is
always in `wkt/SOURCES` and is injected as a fallback in `reproto/src/cli.py`
for `--use-variant any`).  A variant that does **not** include `any.proto` as
a WKT cannot produce `Any` fields — so the expansion feature is safe to assume
the type is always available in the schema pool.

For variants with their own descriptor path, the WKT identity problem is
already handled (spec 0051): the pool always contains exactly one copy of
`google.protobuf.Any` regardless of which proto path the variant uses.

---

## Goals

1. When `prototext decode` encounters a field whose schema type is
   `google.protobuf.Any`, expand the `value` bytes using the type identified
   by `type_url`, if the type is present in the schema pool.
2. The expansion is rendered inline: `value` is rendered as a named message
   block with the resolved type in its `field_decl`, rather than as raw bytes.
   `type_url` is kept as a normal string field.  No new annotation tokens are
   introduced.
3. Add a `--no-expand-any` flag to both `decode` and `list-schemas` to
   suppress expansion globally.
4. When `type_url` is absent, malformed, or the referenced type is not in
   the pool, fall back gracefully to the existing raw two-field rendering (no
   error, no crash).
5. `list-schemas` scores the `value` bytes against the type resolved from
   `type_url` when encountered inside a message being decoded.  When the
   referenced type is a known top-level root in the scoring graph, the walk
   recurses into the expanded type; otherwise `value` scores as a plain bytes
   match with no penalty.

---

## Non-goals

- Overriding the recursion depth limit (expansion is always fully recursive
  when enabled; the `--no-expand-any` flag disables all levels).
- Override mechanism to force a specific type when `type_url` is wrong or
  unknown (future work; `--type` already covers the outermost message).
- Expansion in `prototext encode` (encode is text-to-binary; the text format
  already contains the type information).
- Expansion when no `--descriptor-set` is provided and the pool contains only
  the built-in WKTs (the expanded type is very unlikely to be a WKT).
- Per-field range annotations or scoring-graph YAML format changes (no new
  fields in `NodeEntry`, `CompiledGraph`, or the YAML format).

---

## Specification

### §1 — CLI

```
prototext [--descriptor-set DESCRIPTOR] decode [--no-expand-any] [-a] [--type TYPE] FILE...
prototext [--descriptor-set DESCRIPTOR] list-schemas [--no-expand-any] FILE...
```

`--no-expand-any` suppresses all `Any` expansion globally for that invocation,
regardless of whether `--annotations` is set.  It lives at subcommand level.

`--no-expand-any` is independent of `--raw`; `--raw` already bypasses all
schema-aware rendering, so `Any` expansion never fires in `--raw` mode anyway.

### §2 — Type-URL resolution

The full type name is extracted from `type_url` as the segment after the last
`/`:

```
"type.googleapis.com/google.rpc.Status"  →  "google.rpc.Status"
"type.googleapis.com/foo.bar.Baz"        →  "foo.bar.Baz"
"google.rpc.Status"                      →  "google.rpc.Status"  (no slash)
""                                       →  fallback (no expansion)
```

The extracted FQDN is looked up in `all_schemas` (the flat
`HashMap<bare_fqn, Arc<MessageDescriptor>>` already built inside
`decode_and_render`).  If found, expansion proceeds; otherwise the raw
two-field rendering is used.

### §3 — Expansion rendering

The annotation format used throughout is the existing v2 format: tokens
separated by `"; "`, introduced by `"  #@ "`.  A `field_decl` token has the
form `[repeated|required ]TypeName[ [packed=true]] = N` where `TypeName` is
the short message/enum name or the proto scalar type keyword.

When expansion fires, the `Any` message block is rendered as follows.
Here `phones` is of type `Phone` (a normal message), which contains a field
`result` of type `google.protobuf.Any`:

```
phones {  #@ Phone = 4
  version: 1  #@ int32 = 1
  result {  #@ Any = 2
    type_url: "type.googleapis.com/google.rpc.Status"  #@ string = 1
    value {  #@ Status = 2
      code: 5  #@ int32 = 1
      message: "Not found"  #@ string = 2
    }
  }
}
```

Without annotations (`-a` absent) the `  #@ …` suffixes are omitted; the
block structure is unchanged.

There is no synthetic intermediate level.  The `Any` message renders exactly
like any other message — its wire fields (`type_url`, `value`) are shown
as-is — with one difference: the `value` field (field 2) is rendered as a
named message block using the type resolved from the sibling `type_url` field,
rather than as a raw quoted bytes literal.  The `field_decl` annotation on
`value {` carries the short name of the resolved type (`Status = 2`), which
is sufficient for `encode` to serialize the nested block back to bytes.

No new annotation tokens are introduced.  All round-trip semantics are carried
by the existing `field_decl` format: `encode` already knows how to serialize
a message block as bytes when the `field_decl` declares a message type.

**Recursive expansion:** because expansion is implemented by calling the
normal `render_message` with the resolved descriptor, any `google.protobuf.Any`
field encountered inside the expanded message is itself expanded by the same
intercept.  Expansion is fully recursive at all nesting levels.  Infinite
loops are not possible: each level consumes a finite number of bytes from the
wire.  The `EXPAND_ANY` thread-local controls all levels simultaneously —
`--no-expand-any` suppresses expansion everywhere.

**Annotations driven by the resolved type:** `render_message` is called with
`Some(resolved_desc)` as the schema.  Field names, varint interpretation,
packed detection, enum value names, non-canonical anomaly annotations — all
are determined by the resolved type's descriptor, exactly as if the user had
passed `--type google.rpc.Status` at the outermost level.

If `value` (field 2) appears before `type_url` (field 1) on the wire (unusual
but valid protobuf), rendering falls back to the plain two-field form for that
`Any` occurrence.

### §4 — Failure and fallback modes

| Condition | Behaviour |
|-----------|-----------|
| `type_url` field absent from wire | Render as plain two-field message |
| `type_url` is empty string | Render as plain two-field message |
| Last `/` segment does not match any type in pool | Render as plain two-field message; no warning |
| `value` bytes are not valid protobuf for the resolved type | Render expanded fields as far as parseable; remainder as `INVALID` anomaly (same as any corrupt sub-message) |
| `--no-expand-any` is set | Always render as plain two-field message |
| `value` field is absent from wire | Render `type_url`, then empty body for resolved type |

No warnings are emitted for unknown types; the fallback is silent.  This
matches the general prototext principle of tolerating schema mismatches.

### §5 — Round-trip semantics for encode

No new annotation tokens are introduced.  `encode` uses the existing
`field_decl` mechanism:

- `result {  #@ Any = 2` — `encode` opens an `Any` message at field 2.
- `type_url: "…"  #@ string = 1` — `encode` writes field 1 as a string,
  exactly as for any other string field.
- `value {  #@ Status = 2` — `encode` sees a message-type `field_decl` for
  field 2; it serializes the nested block to bytes and writes the result as
  the `value` bytes field.

Full lossless round-trip requires annotations (`-a`).  Without annotations,
`value { … }` renders as a readable message block but `encode` cannot
reconstruct the bytes-encoded `value` field without a `field_decl` to tell
it the type.  This is the same limitation that applies to all other
schema-dependent rendering in prototext today.

### §6 — Scoring walk expansion (decode and list-schemas)

When `--no-expand-any` is not set, the scoring walk (`walk.rs`) also expands
`Any` fields, using the same rules as the renderer.

#### Detection

For each active-state candidate in the `WT_LEN` Found branch, the walk checks
whether its child state ID equals 0.  Block ID 0 is permanently reserved for
`google.protobuf.Any` (see §9).  No runtime lookup or stored state ID is
needed.  Because the walk maintains multiple active states simultaneously (one
per schema candidate), the Any expansion path fires independently per
candidate: a candidate whose child state is 0 expands; others follow the
normal child recursion.

#### Type resolution

The FQDN is extracted from `type_url` (field 1) by scanning the `Any` payload
for a WT_LEN record at field number 1 (`extract_type_url` helper).  The FQDN
(after stripping the `<scheme>//<host>/` prefix) is then looked up in
`graph.roots`.

**Only top-level roots are considered.**  `type_url` in practice always
references a publicly-named, top-level message type.  If the FQDN is not found
in `graph.roots` — whether because it is unknown, non-root, or `type_url` is
absent or malformed — the walk falls back to scoring `value` as a plain bytes
field: one match, no penalty, no veto.

#### Recursion

When a root is found, the `ActiveEntry` for the Any group (which carries
`state_id == 0` and the original entry indices) is transformed for the
recursive call: `state_id` is replaced with `resolved_root_entry_state` (read
from `graph.roots[fqdn]`), while the `entries` vector (the indices into
`ws.scores`) is preserved unchanged.  The resulting active set is passed to
the normal recursion:

```rust
let any_active = vec![ActiveEntry {
    state_id: resolved_root_entry_state,
    entries: ae.entries.clone(),
    occurrences: Vec::new(),
}];
score_message_multi(value_payload, 0, any_active, None, ws);
```

Non-Any groups in `child_pairs` (candidates that declare a different message
type at the same field number) are unaffected and recurse normally alongside
the Any group.

Because `ws.scores` is indexed by entry index globally, scores from the inner
walk fold back into the correct per-entry counters automatically — the same
mechanism used for every other message recursion.  The `Any` field itself
counts as one match (recorded before the recursive call) regardless of what
the inner walk produces.

Recursive Any fields inside the expanded message are handled automatically by
the same `child_state_id == 0` check on the inner walk.
`ScoringOpts.expand_any = false` suppresses all levels.

### §7 — Options and state

#### Renderer (`prototext-core`)

`decode_and_render` gains an `expand_any: bool` parameter alongside
`annotations` and `indent_size`.  All existing callers pass `true`.

The flag is stored as a thread-local to avoid threading through every helper:

```rust
thread_local! {
    pub(super) static EXPAND_ANY: Cell<bool> = const { Cell::new(true) };
}
```

Set once at the top of `decode_and_render`; read in `render_len_field`.

#### Scoring walk (`scoring-graph`)

`ScoringOpts` gains:

```rust
pub expand_any: bool,  // default true
```

`expand_any` is read directly from `ScoringOpts` at the call sites that need
it; it does not need to be duplicated into `WalkState`.

Any detection in the walk is a single constant comparison: `child_state_id == 0`
(see §9).  No stored state ID or runtime lookup is required.  When block ID 0
is not present in the graph (Any absent from corpus), no transition will ever
produce `child_state_id == 0`, so the expansion path is never taken.

### §8 — Implementation sketch

#### `extract_type_url` (walk.rs helper)

```rust
/// Scan `any_bytes` for field 1 (WT_LEN) and return its UTF-8 string value,
/// or `None` if absent, empty, or not valid UTF-8.
fn extract_type_url(any_bytes: &[u8]) -> Option<&str> { … }
```

The function does a minimal wire scan (no full decode): reads tag/length pairs
until it finds field number 1 with wire type 2 (LEN), then returns the slice
as `str`.  Returns `None` on any parse error.

The FQDN is then extracted from the returned `&str` as the segment after the
last `/` (or the whole string if no `/` is present).

---

#### Renderer intercept (prototext-core)

In `render_len_field` (helpers/len_field.rs), in the `Kind::Message` branch,
after the `is_group()` guard, in place of the existing `wob_prefix_n` +
`render_message` + `write_close_brace` sequence:

```rust
if EXPAND_ANY.with(|c| c.get())
    && nested_msg_desc.full_name() == "google.protobuf.Any"
{
    if render_any_expansion(
        field_number, fs, all_schemas, tag_ohb, tag_oor, len_ohb, data, out,
    ) {
        return;
    }
    // render_any_expansion returns false on fallback → fall through to normal rendering.
}
// normal path: wob_prefix_n + render_message + write_close_brace (unchanged)
```

`render_any_expansion` is a new function in `helpers/any_field.rs`.  It owns
the full output for the `Any` field — opener, body, and closer — so it must
not be called after `wob_prefix_n` has already been written.

1. Parse `data` using the existing wire-parsing helpers to extract field 1
   (`type_url` string) and field 2 (`value` bytes), recording their order.
2. If `type_url` is absent or empty, return `false`.
3. If `value` appears before `type_url` on the wire, return `false`.
4. Extract FQDN from `type_url`; look up in `all_schemas`.  If not found,
   return `false`.
5. Write the field opener with `wob_prefix_n(field_number, Some(fs), false, out)`
   followed by the annotation line (` #@ Any = <N>\n` when annotations are on).
6. Enter indentation level (`push_indent`).
7. Write the `type_url` string line (field 1, string, with annotation `string = 1`).
8. Write `value {` opener with annotation `<ShortName> = 2`.
9. Enter a further indentation level.
10. Call `render_message(value_bytes, 0, None, Some(resolved_desc), all_schemas, out)`.
11. Exit indentation level; write the `}` closer for `value`.
12. Exit indentation level; write the `}` closer for the `Any` field.
13. Return `true`.

If `value` is absent from the wire, steps 8–11 produce an empty `value {}`
block; the outer close-brace is still written.

### §9 — Reserved block IDs and raw node IDs (scoring-graph)

To enable zero-cost Any detection in the walk without any per-node flag or
runtime lookup, block ID 0 and raw node ID 0 are permanently reserved for
`google.protobuf.Any`.  Block ID 1 and raw node ID 1 are reserved for
`google.protobuf.MessageSet` (future use).  All other blocks and raw nodes
are numbered from 2 upward.

#### Raw node ID reservation (`graph.rs` `build()`)

Before the general FQDN→ID allocation loop, `build()` pre-allocates node IDs
for the two reserved types:

```rust
const ANY_NODE_ID: u32 = 0;
const MESSAGE_SET_NODE_ID: u32 = 1;

// Pre-allocate reserved IDs (regardless of corpus content).
node_ids.insert("google.protobuf.Any".to_owned(), ANY_NODE_ID);
node_ids.insert("google.protobuf.MessageSet".to_owned(), MESSAGE_SET_NODE_ID);

// General allocation starts from 2.
let mut next_id: u32 = 2;
for fqdn in merged.states.keys() {
    node_ids.entry(fqdn.clone()).or_insert_with(|| { let id = next_id; next_id += 1; id });
}
```

If either reserved FQDN is absent from the corpus, its pre-allocated node ID
is simply never referenced by any edge and plays no role in the graph — it is
harmless dead state.

#### Hopcroft singleton classes (`hopcroft.rs` `minimize()`)

The initial partition places raw node IDs 0 and 1 each in their own singleton
class, unconditionally:

- Block 0: `{node 0}` — `google.protobuf.Any`
- Block 1: `{node 1}` — `google.protobuf.MessageSet`
- Remaining blocks: numbered from 2, allocated as normal.

This guarantees that no other node is ever merged into block 0 or block 1,
regardless of structural equivalence.  If a reserved node is absent from the
corpus (no edges reference it), its singleton block exists but has no
transitions and is unreachable — harmless.

Block ID 0 in the compiled graph therefore unambiguously identifies the Any
node.  The walk checks `child_state_id == 0` with no other overhead.

#### `num_states` interpretation

`CompiledGraph.num_states` continues to mean "total number of blocks".  With
the two reserved blocks always present, `num_states >= 2` for any valid graph.
Block IDs are in `[0, num_states)`.

#### No changes to `NodeEntry`, `CompiledGraph`, or the YAML format

`NodeEntry` gains no new fields.  `CompiledGraph` gains no `any_state_id`
field.  The YAML scoring-graph format is unchanged.  The reservation is
entirely a construction-time and walk-time convention.

#### Future: `MessageSet` (block ID 1)

When `MessageSet` support is implemented, the walk will check
`child_state_id == 1` to detect MessageSet nodes, using the same pattern.
No further structural changes will be needed.

---

## Notes

- **`type_url`-before-`value` constraint**: requiring canonical field order is
  a simplification.  Real encoders always write fields in declaration order, so
  this is safe for Google-generated payloads.  Relaxing this (by buffering) is
  future work.

- **Warning on unknown type_url**: silently falling back is intentional and
  matches the general prototext principle (see §4).  A verbose mode could emit
  a stderr note in the future; out of scope here.

---

## Testing

- TC-1: `Any` field with known `type_url` → `value` rendered as a named
  message block with resolved `field_decl`; `type_url` line present as a
  normal string field; no raw bytes for `value`.
- TC-2: `Any` field with unknown `type_url` → raw two-field rendering; no
  error.
- TC-3: `Any` field with empty `type_url` → raw two-field rendering.
- TC-4: `--no-expand-any` → raw two-field rendering even when type is known.
- TC-5: `Any` field where `value` precedes `type_url` on wire → raw rendering.
- TC-6: `Any` field with `value` bytes that are invalid protobuf for the
  resolved type → partial expansion with anomaly annotation.
- TC-7: Without annotations (`-a` absent) → expansion still fires; `value { … }`
  block is rendered without `field_decl`; output is human-readable but not
  encode-round-trippable.
- TC-7b: `--no-expand-any` without `-a` → plain two-field rendering (no
  expansion regardless of annotation mode).
- TC-8: Nested `Any` inside an expanded `Any` → both levels expanded
  automatically; field names and anomaly annotations inside the inner
  expansion are governed by the inner resolved type's descriptor.
- TC-9: `Any` field with `value` absent from wire → `type_url` rendered,
  empty `value {}` block.
- TC-10: Encode round-trip with annotations: `decode -a` → `encode` →
  bytes match original binary exactly.
- TC-11: `Any` field with `value` bytes corrupt for the resolved type →
  partial expansion rendered with `INVALID_*` anomaly annotation inline.

Scoring walk tests (in `scoring-graph/src/score/tests.rs`):

- TC-S1: Binary with `Any` wrapping a known root type → scoring recurses into
  expanded type; fields of wrapped type score as matches.
- TC-S2: Binary with `Any` wrapping an unknown type (not a root) → `value`
  scores as plain bytes match; no penalty.
- TC-S3: `expand_any: false` → `value` scores as plain bytes match.
- TC-S4: Nested `Any` inside expanded `Any` → both levels expanded.
- TC-S5: `value` precedes `type_url` on wire → plain bytes fallback.
- TC-S6: Verify block ID 0 is always assigned to `google.protobuf.Any` and
  never to any other node, even when Any has the same transition signature
  as another message in the corpus.
