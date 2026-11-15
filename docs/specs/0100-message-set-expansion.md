<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0100 — `MessageSet` expansion in prototext decode

**Status:** implemented
**Implemented in:** 2026-06-16
**App:** prototext-core, prototext-graph, prototext-graph-pyo3, prototext, reproto

---

## Background

The protobuf wire format supports a `message_set_wire_format` option on
message types.  A message with this option set (call it a `MessageSet`) acts
as a typed container for arbitrary sub-messages using extension numbers rather
than a string `type_url`.  On the wire, a `MessageSet` payload consists of
repeated groups at field number 1, each group containing:

- field 2 (`type_id`): varint — the extension field number identifying the type
- field 3 (`message`): length-delimited bytes — the serialised sub-message

The mapping from `type_id` → message type is declared via proto2 extensions:

```proto
extend SomeMessageSet {
  optional MyMessage my_message = 12345678;
}
```

The descriptor for a `MessageSet` type contains **no regular fields** — only
`extension_range` entries and the `message_set_wire_format` option.  The
group/item structure is implicit in the wire format, not declared as fields.

`prototext decode` currently renders `MessageSet` fields as raw groups with
opaque varint `type_id` values and uninterpreted `message` bytes — identical
to the pre-spec-0089 treatment of `google.protobuf.Any`.

Unlike `Any`, a `MessageSet` type does not have a fixed well-known FQDN: its
fully-qualified name depends on the descriptor set being used.  A descriptor
set may contain multiple types that look structurally like a `MessageSet`
(e.g. `proxy.MessageSet` and `bridge.MessageSet` in one private
corpus).

The renderer applies a **structural heuristic at render time** to the
`MessageDescriptor` of each message field it encounters to detect
`MessageSet` fields — no FQDN needs to be stored or configured.

However, `prototext` almost always operates via the **lazy pool** path
(an `index.rkyv` sidecar is present whenever `reproto` has been run).
On this path the pool starts empty and loads FDPs only as needed.
Extensions are not transitively reachable from the root type, so
`msg_desc.get_extension(type_id)` finds nothing and every group falls
back to raw rendering — defeating the feature entirely.

The fix mirrors spec 0099's JIT type loading: add an `ext_to_file` map
to `FdsIndex` (built by `reproto`) so that `LazyPool` can load the
right FDP on demand before looking up the extension.

Block ID 1 is permanently reserved for `MessageSet` in the scoring graph
(spec 0089 §9), symmetric with block ID 0 for `Any`.

---

## Goals

1. At render time, detect whether the `MessageDescriptor` of a message field
   is a `MessageSet` using a structural heuristic (§1).
2. When a `MessageSet` field is detected, JIT-load the extension FDP via
   `LazyPool.get_extension(extendee_fqdn, type_id)` (§5), look up the
   extension, resolve the message type, and render `message` as a named
   block (§2).
3. Fall back gracefully — reusing the existing schemaless LENDEL cascade —
   when:
   - The heuristic does not match (field is not a `MessageSet`).
   - The `type_id` has no registered extension in the pool after JIT loading.
   - The `message` bytes are absent or malformed.
4. Add `ext_to_file: HashMap<(String, u32), String>` to `FdsIndex` and bump
   VERSION to 4, populated by `reproto/build_index.py` (§4).
5. No CLI flag is needed: the heuristic operates on the `MessageDescriptor`
   already in hand at render time, so each field is unambiguously identified
   without any user input.

---

## Non-goals

- `MessageSet` expansion in `prototext encode` (the text format already carries
  the type information via `field_decl`; this is unchanged).
- Scoring-walk expansion of `MessageSet` (future work; analogous to `Any`
  scoring walk expansion in spec 0089 §6).
- Emitting warnings when `type_id` cannot be resolved.
- Backward compatibility with existing `index.rkyv` files at VERSION 3:
  `reproto` must be re-run to regenerate the sidecar.

---

## Specification

### §1 — `MessageSet` detection (render time)

The canonical definition of a MessageSet comes from `google/protobuf/descriptor.proto`
(`MessageOptions`):

```proto
// The message must be defined exactly as follows:
//   message Foo {
//     option message_set_wire_format = true;
//     extensions 4 to max;
//   }
// Note that the message cannot have any defined fields; MessageSets only
// have extensions.
optional bool message_set_wire_format = 1 [default = false];
```

This is an authoritative, unambiguous binary flag on the message descriptor.
No name-based heuristic is needed or appropriate: multiple MessageSet-typed
messages with different names can coexist in a single `FileDescriptorSet`
(e.g. `AlphaMessageSet`, `BetaMessageSet`, `proxy.MessageSet`)
and each must be handled independently.

During rendering, whenever a `Kind::Message` field is encountered with a
`MessageDescriptor` `desc`, the message is a MessageSet if and only if **both**
conditions hold:

1. `desc.descriptor_proto().options.as_ref().and_then(|o| o.message_set_wire_format).unwrap_or(false) == true`
   — the canonical protobuf `MessageOptions` flag, read from the raw `DescriptorProto`.
2. `desc.fields().count() == 0` — no regular fields declared.  This is required
   by the `descriptor.proto` spec and ensures field 1 on the wire is always an
   implicit group, never a declared typed field.

`descriptor_proto()` is the `&DescriptorProto` accessor on `MessageDescriptor`
in prost-reflect; `message_set_wire_format` is `Option<bool>` in prost-types
`MessageOptions`.

If both conditions hold, attempt `MessageSet` expansion (§2).  Otherwise render
normally.

Multiple types satisfying these conditions may exist in the same descriptor set
(e.g. both `proxy.MessageSet` and `bridge.MessageSet` in one private
corpus).  Each is expanded independently; there is no ambiguity because
each field already carries its own `MessageDescriptor` at render time.

### §2 — Expansion rendering (`prototext-core`)

No new thread-locals are needed.  The MessageSet check (§1) is evaluated inline in
`render_len_field`, in the `Kind::Message` branch, after the `Any` intercept:

```rust
if is_message_set(&nested_msg_desc) {
    render_message_set_expansion(&nested_msg_desc, field_number, fs,
                                 all_schemas, tag_ohb, tag_oor, len_ohb,
                                 data, out);
    return;
}
```

`is_message_set` applies the three conditions from §1.

`render_message_set_expansion` (new file `helpers/message_set_field.rs`):

**Step A — open the outer block.**
Emit `<field_name> {` with annotations as for a normal known message field
(same pattern as the `Kind::Message` branch in `render_len_field`).

**Step B — scan groups.**
Parse `data` for group records (wire type 3, field number 1; closed by wire
type 4, field number 1).  For each group:

1. Extract field 2 (`type_id`, varint) and field 3 (`message`, length-delimited
   bytes).
2. JIT-load the extension FDP via `ANY_LOADER` using the sentinel key
   `"<extendee_fqdn>/<type_id>"` (e.g. `"bridge.MessageSet/208477678"`).
   The loader in `run.rs` detects this pattern and calls
   `lazy.get_extension(extendee_fqdn, type_id)` (§5).  Then call
   `nested_msg_desc.get_extension(type_id as u32)` — the prost-reflect
   `MessageDescriptor::get_extension(number)` API — which now finds the
   freshly loaded extension.
3. **If resolved** (`Some(ext_desc)`, where `ext_desc.kind()` is
   `Kind::Message(inner_desc)`):
   - Emit a virtual group-open line for field 1 using the canonical name
     `Item`: `Item {  #@ group; Item = 1`.  The bare `group` token sets the
     wire type; `Item = 1` supplies the field number via field_decl.  This
     mirrors how `render_group_field` emits known GROUP fields (LHS = type
     name, annotation = `group; TypeName = N`).
   - Emit a virtual scalar line for field 2 using the canonical name
     `type_id`: `type_id: <value>  #@ int32 = 2`.
   - Emit a typed message-open line for field 3 using the canonical name
     `message`: `message {  #@ InnerTypeName = 3` where `InnerTypeName` is
     `inner_desc.name()`.  This mirrors the `value {` annotation in
     `render_any_expansion`.  Recurse into `message` bytes with
     `render_message(..., Some(&inner_desc), all_schemas, ...)` so inner
     fields render with schema names.
   - Close the group block (`}`).
4. **If not resolved** (`None`): render the group raw via the schemaless path
   (see §3).

**Step C — close the outer block.**

Full verbatim rendered output when both extensions resolve (annotations on,
indent 1):

```
request_extensions {  #@ bridge.MessageSet = 23
 Item {  #@ group; Item = 1
  type_id: 208477678  #@ int32 = 2
  message {  #@ MyExtensionType = 3
   ...fields of MyExtensionType...
  }
 }
 Item {  #@ group; Item = 1
  type_id: 456962417  #@ int32 = 2
  message {  #@ AnotherType = 3
   ...fields of AnotherType...
  }
 }
}
```

The canonical field names `Item`, `type_id`, and `message` come from Google's
own proto2 MessageSet definition.  Fields 1, 2, and 3 have no `FieldOrExt`
schema descriptor, so the encoder relies on the annotation for field numbers
and wire types.  For the group (field 1), `group` sets the wire type and
`Item = 1` supplies the field number.  For `type_id` (field 2), `int32 = 2`
supplies both type and field number.  For `message` (field 3),
`InnerTypeName = 3` supplies the field number; inner fields render with
`inner_desc` so their names are visible.

**Round-trip explanation**: the encoder reconstructs wire bytes as follows:
- `request_extensions {  #@ ... = 23` → LEN tag at field 23, length placeholder.
- `Item {  #@ group; Item = 1` → `START_GROUP` at field 1; `group` sets wire type 3, `Item = 1` supplies field number.
- `type_id: 208477678  #@ int32 = 2` → varint at field 2, value 208477678; field number from `= 2`.
- `message {  #@ MyExtensionType = 3` → LEN tag at field 3 (from `= 3`), length placeholder.
- Sub-message content rendered recursively with `inner_desc` (field names and types visible inside).
- Closing `}` for `message` → fills LEN placeholder.
- Closing `}` for `Item` group → `END_GROUP` at field 1.
- Closing `}` for `request_extensions` → fills outer LEN placeholder.

### §3 — Lossless fallback for unresolvable payloads (`prototext-core`)

Losslessness and byte-exact round-trip are invariants that must never be
broken.  No field, no payload byte may ever be silently dropped.

**`Any`**: when `render_any_expansion` returns `false` (FQDN unresolvable),
the caller in `render_len_field` falls through to the normal `Kind::Message`
rendering path, which emits the raw two-field form (`type_url` string +
`value` bytes).  No data is lost; no change needed.

**`MessageSet`**: when `nested_msg_desc.get_extension(type_id)` returns
`None`, the group must still be rendered.  Reuse the existing schemaless
LENDEL cascade already in `render_len_field` (the `let Some(fs) = field_schema
else { ... }` block, steps 1–3):

1. Attempt to decode `message` bytes as a schemaless nested message.
2. If that fails, attempt UTF-8 string.
3. If that fails, raw escaped bytes.

No new fallback logic is introduced — `render_message_set_expansion` calls
`render_len_field(field_number=1, field_schema=None, data=message_bytes, ...)`
for the unresolved case, which naturally takes the schemaless path.

### §4 — `FdsIndex` and `reproto` changes

#### §4.1 — `FdsIndex` (`prototext-graph`)

`FdsIndex` gains one new field:

```rust
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct FdsIndex {
    pub type_to_file: HashMap<String, String>,
    pub file_to_span: HashMap<String, (u64, u64)>,
    pub dep_graph:    HashMap<String, Vec<String>>,

    /// "extendee_fqdn/field_number" → proto file name.
    /// Enables O(1) JIT-loading of extension FDPs (spec 0100 §5).
    /// extendee_fqdn has no leading dot, matching prost-reflect convention.
    /// Key format matches the ANY_LOADER sentinel key (spec 0100 §5.2).
    pub ext_to_file: HashMap<String, String>,
}
```

This bumps `FdsIndex` to **VERSION 4**.  Existing `index.rkyv` files at
version 3 are rejected with a clear error message asking the user to
re-run `reproto`.  The VERSION constant and error message are in
`prototext-graph/src/fds_index.rs` and `prototext/src/lazy_pool.rs`
respectively.

#### §4.2 — `build_fds_index` pyo3 binding (`prototext-graph-pyo3`)

The `build_fds_index` pyfunction gains one new parameter:

```rust
#[pyfunction]
fn build_fds_index<'py>(
    py: Python<'py>,
    type_to_file: HashMap<String, String>,
    file_to_span: HashMap<String, (u64, u64)>,
    dep_graph: HashMap<String, Vec<String>>,
    ext_to_file: HashMap<String, String>,   // NEW
) -> PyResult<Bound<'py, PyBytes>> {
    let index = FdsIndex { type_to_file, file_to_span, dep_graph, ext_to_file };
    ...
}
```

#### §4.3 — `build_index.py` (`reproto`)

`build_fds_index` in `reproto/src/reproto/build_index.py` is extended to
build `ext_to_file` during the existing single pass over the FDS:

```python
ext_to_file: dict[str, str] = {}

for i, fdp in enumerate(fds.file):
    name = fdp.name
    pkg = fdp.package
    prefix = f"{pkg}." if pkg else ""

    # existing: type_to_file, file_to_span, dep_graph ...

    # NEW: collect top-level extensions
    for ext in fdp.extension:
        extendee = ext.extendee.lstrip('.')   # strip leading dot
        ext_to_file[f"{extendee}/{ext.number}"] = name

    # NEW: collect extensions nested inside message types
    def collect_extensions(msg: DescriptorProto, file_name: str) -> None:
        for ext in msg.extension:
            extendee = ext.extendee.lstrip('.')
            ext_to_file[f"{extendee}/{ext.number}"] = file_name
        for nested in msg.nested_type:
            collect_extensions(nested, file_name)

    for msg in fdp.message_type:
        collect_extensions(msg, name)
```

The call to `_rust_build` gains the new argument:

```python
return bytes(_rust_build(
    type_to_file=type_to_file,
    file_to_span=file_to_span,
    dep_graph=dep_graph,
    ext_to_file=ext_to_file,
))
```

### §5 — JIT extension loading (`prototext`)

#### §5.1 — `LazyPool.get_extension`

`LazyPool` gains a new method:

```rust
pub fn get_extension(
    &mut self,
    extendee_fqdn: &str,
    field_number: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let key = format!("{extendee_fqdn}/{field_number}");
    if let Some(file) = self.index.ext_to_file.get(key.as_str()) {
        self.ensure_loaded(file.as_str())?;
    }
    Ok(())
}
```

The method loads the FDP that declares the extension, after which
`pool.get_message_by_name(extendee_fqdn).get_extension(field_number)`
will find it.  If the key is absent from `ext_to_file`, the call is a
no-op and graceful fallback applies.

#### §5.2 — `install_any_loader` in `run.rs`

The `ANY_LOADER` closure is extended to detect the sentinel key pattern
`"<extendee_fqdn>/<field_number>"`, JIT-load the extension FDP, and return
the extension's payload `MessageDescriptor` from the updated `lazy.pool`:

```rust
let loader: AnyLoader = Box::new(move |key: &str| {
    let ctx = unsafe { &mut *ctx_ptr };
    if let Some(slash) = key.rfind('/') {
        // MessageSet extension sentinel: "extendee_fqdn/field_number"
        if let Ok(number) = key[slash + 1..].parse::<u32>() {
            let extendee = &key[..slash];
            if let Some(lazy) = ctx.lazy.as_mut() {
                let _ = lazy.get_extension(extendee, number);
                // Re-fetch from the updated lazy.pool and return the
                // extension's payload type.  We cannot call
                // msg_desc.get_extension() in the caller: prost-reflect uses
                // Arc::make_mut when adding FDPs, which forks the pool Arc
                // whenever another clone exists (e.g. in ParsedSchema).
                // After the fork the caller's MessageDescriptor references
                // the old Arc without the new extension.
                if let Some(ed) = lazy.pool.get_message_by_name(extendee) {
                    if let Some(ext) = ed.get_extension(number) {
                        if let prost_reflect::Kind::Message(inner) = ext.kind() {
                            return Some(std::sync::Arc::new(inner));
                        }
                    }
                }
            }
            return None;
        }
    }
    // Normal Any path: key is a FQDN.
    if let Some(lazy) = ctx.lazy.as_mut() {
        let _ = lazy.get_message(key);
    }
    ctx.pool()
        .get_message_by_name(key)
        .map(std::sync::Arc::new)
});
```

The loader returns the extension's payload `MessageDescriptor` (not `None`)
for the sentinel pattern.  The caller (`render_message_set_expansion`) uses
this return value directly — calling `msg_desc.get_extension()` on the stale
descriptor would miss the newly-registered extension due to pool forking.

### §6 — No CLI flag needed

No `--message-set-type` flag or thread-local filter is introduced.  The
heuristic (§1) operates directly on the `MessageDescriptor` in hand at
render time; each field is unambiguously identified without user input.
Multiple `MessageSet`-shaped types in the same descriptor set are each
expanded independently.

---

## Testing

- TC-1: Eager path — decode a binary with a `MessageSet` field; all `type_id`
  values expand to named blocks.
- TC-2: Lazy path — same binary; `ext_to_file` triggers JIT loading; same
  expansion result as TC-1.
- TC-3: Unknown `type_id` (not in `ext_to_file`, not in pool) → group
  rendered via schemaless cascade (message / string / bytes); no data lost;
  no error.
- TC-4: Descriptor set with two `MessageSet`-shaped types (as a private corpus has)
  → both expanded independently; no ambiguity.
- TC-5: No `MessageSet` in descriptor → no expansion; raw rendering.
- TC-6: Round-trip: `decode -a` → `encode` → bytes match original.
- TC-7: `MessageSet` inside an expanded `Any` → both levels expanded.
- TC-8: `Any` with unresolvable `type_url` → raw two-field rendering
  (`type_url` + `value` bytes); field never suppressed; round-trip produces
  identical bytes.
- TC-9: Old `index.rkyv` (VERSION 3) → clear error asking user to re-run
  `reproto`.

---

## Notes

- **Losslessness invariant**: `prototext decode` output must always be
  re-encodable to byte-identical binary.  No field, no payload byte may ever
  be silently dropped.  For `Any`, the existing `Kind::Message` fallback path
  upholds this.  For `MessageSet`, the schemaless LENDEL cascade is the
  mechanism — reuse it, do not duplicate it.
- **`MessageDescriptor::get_extension(number)`** in prost-reflect scans
  `extensions()` for a matching field number.  It finds extensions only from
  FDPs already loaded into the pool; the `ANY_LOADER` sentinel call (§5.2)
  ensures the right FDP is loaded before this is called.
- **Canonical field names**: `Item` (group, field 1), `type_id` (int32, field 2),
  `message` (field 3) come from Google's own proto2 MessageSet definition.
  The annotation `group; Item = 1` mirrors how `render_group_field` emits
  known GROUP fields (bare `group` wire-type token + field_decl `TypeName = N`).
  The encoder reconstructs from `group` (wire type) + `= 1` (field number),
  `= 2`, and `= 3` respectively.
- **`ext_to_file` key**: `"<extendee_fqdn>/<field_number>"` where `extendee_fqdn`
  has no leading dot (matching prost-reflect convention).  This is the same
  string as the sentinel key passed to `ANY_LOADER`, detected by
  `rfind('/')` + `parse::<u32>()`.  Using a flat string key avoids rkyv
  archived-tuple lookup issues and allows direct `ArchivedHashMap::get(&str)`.
- **VERSION bump 3→4**: the rkyv zero-copy layout changes with the new
  `ext_to_file` field; old archives are incompatible and are rejected on open.
- A private corpus contains two types satisfying the MessageSet check:
  `proxy.MessageSet` (`proxy.proto`) and `bridge.MessageSet`
  (`bridge/message_set.proto`).  At render time each field
  already carries its own `MessageDescriptor`, so both are handled
  independently without any disambiguation step.
