<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0156 — protolens: rename `extract` to `export`, add `xb`/`xp`/`xdb`/`xdp` export-format chord, rename `save-overrides`/`restore-overrides` to `save`/`restore`

Status: implemented
Implemented in: 2026-07-22
App: protolens

## Background

Today, protolens's `:` command line has `:extract [--binary|--text] <path>`
(default `--text`, i.e. `#@ prototext` rendering) to write the cursor
node's data to a file, a bare `x` key that pre-fills the command line
with `extract <default path>` (spec 0113 D21/D23), and separate
`:save-overrides`/`:restore-overrides` commands (spec 0117 §4) with `s`/
`r` shortcuts that pre-fill those two command lines. Batch (non-
interactive) mode mirrors `:extract` as the `extract` CLI subcommand
(spec 0123), taking the same node (bare positional-path notation) and
format.

Renaming `extract` to `export` reads better against a third export
*content*: not just the node's own **data** (as `binary`/`prototext`),
but that node's **schema** (a synthetic `FileDescriptorSet` describing
it — useful for feeding a subtree's type definition back into
`reproto`/`protolens` elsewhere). Because a schema, like data, can
itself be serialized either as raw binary bytes or rendered as
prototext, there are really four export combinations, not three:
data-as-binary, data-as-prototext, schema-as-binary,
schema-as-prototext. A single leader key (`x` for e**x**port) followed
by a key naming the serialization (`b`/`p`), with a `d` sub-leader for
the schema pair, is more discoverable than extending the existing
bare-`x`-then-edit-the-flag-by-hand flow.

## Goals

- **G1.** Rename the `:extract` TUI command to `:export` — `COMMANDS`
  registry entry, `run_command` dispatch, all user-facing message-line
  text (`"export: missing path"`, `"exported to {path}"`, `"export
  error: {e}"`), help overlay text, and doc comments referencing
  `:extract`/`extract`.
- **G2.** `:export`'s format flags become `--binary`, `--prototext`
  (renamed from `--text`), `--descriptor-binary`, and
  `--descriptor-prototext` (new — G6/G7). Exactly one may be given; the
  default when none is given is unchanged: `--prototext` (spec 0113
  D21). Batch mode's `--format` value gets the equivalent rename/
  additions under G8.
- **G3.** Replace the bare `x` shortcut with a chord, extending the
  existing `pending_g`-style leader-key pattern (spec 0113) one level
  deeper for the schema pair:
  - `x` arms a `pending_x` leader state (silently). The *next* keypress
    resolves it:
    - `xb` → pre-fills `export --binary <path>`
    - `xp` → pre-fills `export --prototext <path>`
    - `xd` arms a second, nested leader state (silently — does *not*
      pre-fill anything by itself)
    - any other key (including a second `x`) cancels the chord without
      pre-filling anything, and is then processed normally by the rest
      of `handle_key` — "falls through unswallowed", same as an
      off-chord key after a lone `g`.
  - From the `xd` sub-state, the *next* keypress resolves it:
    - `xdb` → pre-fills `export --descriptor-binary <path>`
    - `xdp` → pre-fills `export --descriptor-prototext <path>`
    - any other key cancels the whole chord (falls through unswallowed,
      same rule as above — there is no bare-`xd` behavior).

  There is no bare-`x` and no bare-`xd` behavior: a lone `x` or `xd`
  press only arms its leader state and returns; it never opens the
  command line by itself.
- **G4.** `xb`/`xp`'s `<path>` is today's `default_extract_path()`,
  unchanged: `<blob_stem>.<start>-<end>.<short_type>.pb` (spec 0113
  D23's rationale — the extension doesn't leak binary vs. prototext —
  still applies to these two).
- **G5.** `xdb`/`xdp`'s `<path>` is a new
  `default_export_descriptor_path()`, reusing `default_extract_path()`'s
  stem/short-type logic but:
  - extension `.desc` instead of `.pb` (for both binary and prototext
    serialization — same "extension doesn't leak serialization" logic
    as G4, applied one level up: it doesn't leak *format* here either,
    it only says "this is a descriptor export");
  - the range segment (`<start>-<end>`) is replaced with the literal
    text `no range` when the cursor is on the document root (`self.
    cursor == self.first_node` — the root's `display_range` always
    spans the whole logical blob, and it never has a schema field name
    of its own, per `field_name_for`'s existing doc comment);
  - otherwise, the range segment is replaced with the cursor node's own
    schema field name (`parent_field(self.cursor)`'s `.name()`) when
    the parent's schema resolves it;
  - otherwise (no schema field name, not the root), falls back to the
    numeric `<start>-<end>` range, same as G4.

  So: `<blob_stem>.no range.<short_type>.desc`, or
  `<blob_stem>.<field_name>.<short_type>.desc`, or
  `<blob_stem>.<start>-<end>.<short_type>.desc`.
- **G6. `--descriptor-binary`/`--descriptor-prototext`'s content
  (design v0).** Either flag (TUI and batch alike) writes a
  `FileDescriptorSet` describing **one synthetic message type built
  from the cursor node's own live-tree children** — not the cursor's
  own natural/resolved type, and not any node's data.
  - **G6a. Eligibility.** The cursor node must be a message or group
    (`span.is_message`). Its `type_fqdn` need not resolve — nothing in
    G6b–G6d below depends on the cursor's own resolved type, only on
    its live children (G6c) and, where present, overrides targeting it
    (G6c tiers 1–2). Otherwise: hard error, nothing written (e.g.
    `"export --descriptor: cursor node is not a message/group"`).
  - **G6b. Synthetic message name.** `field_name_for(self.cursor)` (the
    existing helper, unmodified — spec 0117's override-`name`-aware,
    then parent-schema-aware, then field-number-string-fallback
    resolution, including its documented "the document root is not
    special-cased, falls through to field number 1" behavior) with its
    first character capitalized; if that first character is an ASCII
    digit (the field-number-fallback case, which includes the
    document-root case: `field_name_for(first_node)` is always `"1"`),
    the whole string is prefixed with `F` first (e.g. `"5"` → `"F5"`,
    `"1"` → `"F1"`), so the result is always a syntactically valid
    proto identifier (message names can't start with a digit).
  - **G6c. Field list.** One field per distinct `field_number` among
    the cursor's direct live-tree children (`first_child`/
    `next_sibling` chain — one level only, never recursive; several
    children sharing a field_number, e.g. separate wire occurrences of
    a repeated field, collapse to one exported field, keyed off the
    first such child in traversal order). For each:
    - `name`: `field_name_for` of the (first) child with this
      field_number, passed through `synthetic_field_name` (a resolvable
      name is left untouched; the field-number-string fallback is
      prefixed with `f`, e.g. `"5"` -> `"f5"`, so the result is always a
      syntactically valid proto identifier — the field-name analogue of
      G6b's `synthetic_message_name`, but without its capitalization
      step, since field names carry no capitalization convention).
    - `number`: the field_number itself.
    - `label`: `LABEL_REPEATED` when `parent_field(child)` resolves and
      is itself a repeated field, *or* when more than one live child
      shares this field_number; `LABEL_OPTIONAL` otherwise.
    - `type`/`type_name`, resolved in priority order (first match
      wins) — only `PathField`/`FqdnField`-kind overrides are ever
      consulted; `Path`-kind overrides are never consulted here (a
      `Path` override retypes one specific node *instance*, not
      "every field with this number" — applying it per-field-number
      would be ambiguous whenever several live children share a
      field_number, per this spec's design discussion):
      1. An active `PathField` override whose `path` is exactly the
         cursor's own positional path and whose `field` matches this
         field_number (spec 0117 §1's `path:field` notation).
      2. An active `FqdnField` override whose `fqdn` matches the
         cursor's own resolved `type_fqdn` (only considered when the
         cursor's `type_fqdn` is `Some`) and whose `field` matches this
         field_number.
      3. `natural_type(child)` (existing helper, schema-resolved via
         `parent_field(child)` — naturally yields nothing when the
         cursor's own `type_fqdn` doesn't resolve, same as tier 2,
         requiring no separate G6a gate for that case).
      4. A wire-type-derived primitive guess, keyed off the (first)
         child's own `wire_type` (a packed-record child's *effective*
         wire type is its reconstructed `WT_LEN` framing, same
         convention `complete_type_as_fqdn` already follows):

        | wire_type       | guessed keyword |
        |-----------------|------------------|
        | `WT_VARINT`     | `int64`          |
        | `WT_I32`        | `fixed32`        |
        | `WT_I64`        | `fixed64`        |
        | `WT_LEN`        | `bytes`          |
        | `WT_START_GROUP`| *(none)*         |

      An override target `Some(fqdn)` (tiers 1–2) is resolved against
      `ctx.pool()`: a message FQDN → `Type::Message`/
      `type_name = ".{fqdn}"`; an enum FQDN → `Type::Enum`/
      `type_name = ".{fqdn}"`; unresolvable → hard error (`"export
      --descriptor: override target '{fqdn}' not found"`). An override
      target `None` (raw) → `Type::Bytes`, `type_name` cleared. A
      `WT_START_GROUP` child reaching tier 4 with no override/natural
      type resolving → hard error for the whole export (`"export
      --descriptor: field {n} is an untyped group — not supported"`);
      every other tier-4 case always succeeds (the table above is total
      for the three non-group wire types).
  - **G6d. Declared dependencies and synthetic file shape.** The
    resulting `FileDescriptorSet` holds exactly one file — the new
    synthetic file (holding the message built in G6b/G6c). It does
    *not* embed any other file's content: for every field whose
    `Message`/`Enum` type (post-G6c) resolves into some file, that
    file's name is listed in the synthetic file's own `dependency`
    field (deduplicated), exactly like a proto `import` statement —
    the referenced file's `FileDescriptorProto` itself is never copied
    into the output set. This applies uniformly, with no special-
    casing: a dependency that happens to be the cursor's own original
    file is declared exactly like any other (no exclusion), and a
    dependency that happens to be a well-known type (e.g.
    `google/protobuf/descriptor.proto`) is declared exactly like any
    other (no embedding) — the consumer is expected to already have,
    or separately obtain, every declared dependency.

    The synthetic file itself:
    - `name`: `"<message name>.export.proto"`.
    - `package`: unset — the synthetic message lives at its bare name,
      top-level, in no package. G6b's naming no longer claims to
      preserve any real type's FQDN, so there is no package identity to
      preserve.
    - `message_type`: the one `DescriptorProto` from G6b/G6c. No
      `oneof_decl` is emitted (v0 has no oneof-membership tracking for
      live-tree children — see Non-goals).
    - `syntax`: `"proto2"`, unconditionally — every field always
      carries an explicit `label` (`LABEL_OPTIONAL`/`LABEL_REPEATED`,
      G6c), which proto2 renders explicitly (`optional`/`repeated`)
      rather than relying on proto3's implicit-presence convention; a
      fixed, always-valid syntax also avoids depending on any one
      field's own parent file (which may differ per field, and
      needn't even resolve at all per G6a).
    - `dependency`: the file names directly referenced by any field,
      as described above.
- **G7. `--descriptor-prototext`'s meta-schema.** Rendering the
  `FileDescriptorSet` built by G6 as prototext (rather than writing its
  raw bytes) reuses the existing `#@ prototext` rendering pipeline,
  which needs a schema *for* `FileDescriptorSet` itself. protolens has
  no embedded/vendored fallback schema (spec 0111 Goal 2) — this must
  come from the already-loaded `--descriptor-set`. Locate it as
  follows: walk `ctx.pool().files()` for a file whose `name()` ends in
  `descriptor.proto` (matches `google/protobuf/descriptor.proto`,
  `proto2/descriptor.proto`, or any other path ending that
  way — deliberately not keyed to any one package/variant, since
  reproto's own variant/canonization mechanism, spec 0086/0150, decides
  what ends up in a given schema-db, and protolens never special-cases
  a variant) that also declares top-level messages named
  `FileDescriptorSet` and `FileDescriptorProto` (by simple/local name,
  not FQDN — works whichever package the file declares, `google.
  protobuf`, `proto2`, or anything else). Use the resolved
  `FileDescriptorSet` `MessageDescriptor` as the render target. If no
  file matches by name, or the matched file lacks either message, the
  loaded `--descriptor-set` does not include `descriptor.proto`:
  `--descriptor-prototext` is unavailable — hard error (e.g. `"export
  --descriptor --prototext: no descriptor.proto (with FileDescriptorSet/
  FileDescriptorProto messages) found in the loaded --descriptor-set;
  use --descriptor-binary instead, or rebuild the schema-db so
  descriptor.proto is included"`). `--descriptor-binary` is unaffected
  either way — it never needs a schema, only `encode_to_vec()`.
- **G8.** Batch CLI: rename the `extract` subcommand to `export`
  (`Command::Extract` → `Command::Export`) — same name as the TUI
  command. Its existing positional `path` argument (bare positional-
  path notation, e.g. `/1/2` — never `path:field`/`fqdn:field`
  override-origin notation) is unchanged. Its `--format` value becomes
  a single 4-value enum whose names directly mirror the TUI chord names
  (G3): `binary`, `prototext`, `descriptor-binary`,
  `descriptor-prototext` (`ExtractFormatArg::Binary`/`Prototext`/
  `DescriptorBinary`/`DescriptorPrototext`).
- **G9.** Batch mode: `--format=descriptor-binary` or
  `--format=descriptor-prototext` without `--load-overrides` is a hard
  error (batch has no interactive type-inference loop — spec 0123 — so
  without a loaded override collection there is generally no
  `PathField`/`FqdnField` override for G6c to apply). Checked ahead of
  path resolution/G6, so it's independently observable/testable from
  G6's own errors.
- **G10.** Rename `:save-overrides`/`:restore-overrides` to `:save`/
  `:restore` — `COMMANDS` registry, `run_command` dispatch, tab-
  completion's argument-completion dispatch (`start_tab_completion`),
  help overlay text, the `s`/`r` shortcut pre-fill text in
  `manage_pane.rs`, and doc comments/message-line text referencing the
  old names (e.g. `"save-overrides: missing path"` →
  `"save: missing path"`).

## Non-goals

- N1. No TUI-side equivalent of G9's "`--load-overrides` required"
  gate — the interactive session's own live override collection already
  serves the same purpose G6c needs: there both `PathField`/`FqdnField`
  overrides and resolvable natural types are ordinarily already
  present.
- N2. No change to `--binary`, `-o`/`--output`, or `--load-overrides`'s
  existing behavior beyond the renames/additions above.
- N3. No change to the node-path parameter's syntax or semantics —
  batch `export`'s positional `path` stays bare positional-path
  notation; this was already correct (confirmed against spec 0123).
- N4. G6c never looks at nested/descendant overrides (only the cursor's
  *own* positional path for `PathField`, the cursor's *own* resolved
  `type_fqdn` for `FqdnField`), and never consults `Path`-kind
  overrides for field typing. (`field_name_for`'s own, separate,
  `Path`-priority-first override lookup for G6b's *naming* step is
  unaffected by this restriction — it answers a different question,
  "what is this node's own display name", not "what type does this
  field number have".)
- N5. No message-level metadata beyond fields is preserved: no oneof
  tracking, no options, no reserved ranges/names, no nested types, no
  extensions. Deferred; not needed for the common "export this node's
  field shape" use case G6 targets.
- N6. G7's `descriptor.proto` lookup is exactly the one heuristic
  described there (file name suffix + `FileDescriptorSet`/
  `FileDescriptorProto` simple-name presence) — no attempt to resolve a
  differently-named "descriptor of descriptors" beyond that.

## Specification

### `protolens/src/tui/mod.rs`

`COMMANDS`:

```rust
const COMMANDS: &[&str] = &[
    "export",
    "quit",
    "type-as",
    "type-as-raw",
    "save",
    "restore",
    "proto-root",
];
```

New field on `App`, next to `pending_g`, replacing the previous draft's
plain `bool` with a 3-state enum (G3's nested `xd` sub-state):

```rust
/// Export-chord leader state (spec 0156 G3): `None` (no chord armed),
/// `Leader` (a lone `x` was just pressed), `Descriptor` (`xd` was just
/// pressed — one more key selects binary vs. prototext).
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum ExportChord {
    None,
    Leader,
    Descriptor,
}

pending_x: ExportChord,
```

initialized `ExportChord::None` alongside `pending_g: false`.

Help overlay text (replacing the current `x`/`:extract`/`:save-
overrides`/`:restore-overrides` entries):

```text
  x                arm the export chord — xb/xp pre-fill ":export"
                   for binary/prototext data; xd arms a second chord
                   — xdb/xdp pre-fill ":export" for binary/prototext
                   schema
  :export [--binary|--prototext|--descriptor-binary|--descriptor-prototext] <path>
                   export the cursor node to <path> — default
                   ...
  s                pre-fill ":save <default path>"
  r                pre-fill ":restore "
  :save <path>
  ...
  :restore <path>
  ...
  Tab              complete a filesystem path (save/restore argument)
```

(Exact wording/wrapping to match the surrounding help text's existing
style; the module doc comment at the top of `mod.rs` referencing
`` `:extract`/`x` `` is updated to `` `:export`/`x` ``.)

### `protolens/src/tui/key_dispatch.rs`

Chord handling, placed at the same tier as the existing `gg`/
`pending_g` block (after the empty-tree early-return, before the main
`match key.code`):

```rust
// `x<b|p|d<b|p>>` chord (export-format leader key, spec 0156 G3): a
// first `x` press arms `ExportChord::Leader` silently; the next
// keypress either fires a data export (`b`/`p`), arms
// `ExportChord::Descriptor` (`d`, no fire yet), or cancels the chord
// (falls through unswallowed) — same pattern as the `gg` chord above,
// adapted for a two/three-key selection rather than a repeated key.
match self.pending_x {
    ExportChord::Leader => {
        self.pending_x = ExportChord::None;
        match key.code {
            KeyCode::Char('b') => {
                self.prefill_export("--binary", self.default_extract_path());
                return;
            }
            KeyCode::Char('p') => {
                self.prefill_export("--prototext", self.default_extract_path());
                return;
            }
            KeyCode::Char('d') => {
                self.pending_x = ExportChord::Descriptor;
                return;
            }
            _ => {} // falls through, processed normally below
        }
    }
    ExportChord::Descriptor => {
        self.pending_x = ExportChord::None;
        match key.code {
            KeyCode::Char('b') => {
                self.prefill_export(
                    "--descriptor-binary",
                    self.default_export_descriptor_path(),
                );
                return;
            }
            KeyCode::Char('p') => {
                self.prefill_export(
                    "--descriptor-prototext",
                    self.default_export_descriptor_path(),
                );
                return;
            }
            _ => {} // falls through, processed normally below
        }
    }
    ExportChord::None => {
        if key.code == KeyCode::Char('x') {
            self.pending_x = ExportChord::Leader;
            return;
        }
    }
}
```

The old `KeyCode::Char('x') => { ... }` arm inside the main `match
key.code` block is removed (no bare-`x`/bare-`xd` behavior — G3).

New small helper, and `default_export_descriptor_path`, next to
`default_extract_path`:

```rust
/// Pre-fills the command line with `export <flag> <path>` and opens it
/// (spec 0156 G3) — shared by all four chord resolutions.
fn prefill_export(&mut self, flag: &str, path: String) {
    let buf = format!("export {flag} {path}");
    self.command_kind = CommandLineKind::Command;
    self.command_cursor = buf.chars().count();
    self.command_buffer = Some(buf);
}

/// Propose a default `xdb`/`xdp`/`:export --descriptor-*` path (spec
/// 0156 G5): same `<blob_stem>`/`<short_type>` construction as
/// `default_extract_path`, but `.desc` extension, and the range
/// segment replaced with `no range` at the document root, or the
/// cursor node's schema field name when resolvable — falling back to
/// the numeric range only when neither applies.
pub(super) fn default_export_descriptor_path(&self) -> String {
    let stem = self
        .blob_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extract".to_string());
    let node = &self.tree[self.cursor].span;
    let short_type = node
        .type_fqdn
        .as_deref()
        .and_then(|f| f.rsplit('.').next())
        .unwrap_or("node");
    let segment = if self.cursor == self.first_node {
        "no range".to_string()
    } else if let Some(field) = self.parent_field(self.cursor) {
        field.name().to_string()
    } else {
        let range = self.display_range(self.cursor);
        format!("{}-{}", range.start, range.end)
    };
    let filename = format!("{stem}.{segment}.{short_type}.desc");
    match self.blob_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            dir.join(filename).to_string_lossy().into_owned()
        }
        _ => filename,
    }
}
```

### `protolens/src/export_descriptor.rs` (new)

The G6/G7 field-resolution and closure-building logic, independent of
TUI/batch plumbing so both can share it:

```rust
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
};
use prost_reflect::{DescriptorPool, FileDescriptor};
use std::collections::HashSet;

/// One resolved field for the synthetic message (spec 0156 G6c) —
/// `referenced_file` is `Some` only for `Message`/`Enum` fields, and
/// feeds `build`'s declared-dependency list (G6d).
pub struct ResolvedField {
    pub number: u64,
    pub name: String,
    pub label: Label,
    pub r#type: Type,
    pub type_name: Option<String>,
    pub referenced_file: Option<FileDescriptor>,
}

/// Builds the `FileDescriptorSet` `export --descriptor-*` writes (spec
/// 0156 G6d): one synthetic file named `"{message_name}.export.proto"`
/// holding one message (`message_name`, `fields` — already fully
/// resolved by the caller per G6b/G6c), `syntax = "proto2"`, no
/// package. The file only *declares* the dependencies (import
/// statements) needed to resolve every field's `Message`/`Enum` type
/// — it never embeds a dependency's own content, so the output is a
/// single-file `FileDescriptorSet`.
pub fn build(message_name: &str, fields: Vec<ResolvedField>) -> FileDescriptorSet {
    let field_protos: Vec<FieldDescriptorProto> = fields
        .iter()
        .map(|f| FieldDescriptorProto {
            name: Some(f.name.clone()),
            number: Some(f.number as i32),
            label: Some(f.label as i32),
            r#type: Some(f.r#type as i32),
            type_name: f.type_name.clone(),
            ..Default::default()
        })
        .collect();

    let message = DescriptorProto {
        name: Some(message_name.to_string()),
        field: field_protos,
        ..Default::default()
    };

    // G6d: declared dependencies only (import-style), deduplicated by
    // file name — no embedding of a dependency's own content.
    let mut seen: HashSet<String> = HashSet::new();
    let mut dependency: Vec<String> = Vec::new();
    for f in &fields {
        if let Some(file) = &f.referenced_file {
            if seen.insert(file.name().to_string()) {
                dependency.push(file.name().to_string());
            }
        }
    }

    let synthetic = FileDescriptorProto {
        name: Some(format!("{message_name}.export.proto")),
        message_type: vec![message],
        dependency,
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };
    FileDescriptorSet {
        file: vec![synthetic],
    }
}

/// G7: locates `descriptor.proto`'s `FileDescriptorSet` message inside
/// `pool`, by file-name suffix + simple-name presence of both
/// `FileDescriptorSet` and `FileDescriptorProto` — deliberately not
/// keyed to any one package (works for `google.protobuf.*`,
/// `proto2.*`, or any other variant's canonized/uncanonized names).
/// `None` when no such file/message pair is found (G7's hard-error
/// case, raised by the caller).
pub fn locate_file_descriptor_set_type(
    pool: &DescriptorPool,
) -> Option<prost_reflect::MessageDescriptor> {
    pool.files()
        .find(|f| f.name().ends_with("descriptor.proto"))
        .and_then(|f| {
            let has_fdp = f.messages().any(|m| m.name() == "FileDescriptorProto");
            has_fdp
                .then(|| f.messages().find(|m| m.name() == "FileDescriptorSet"))
                .flatten()
        })
}
```

(The field-resolution step that produces `Vec<ResolvedField>` from the
cursor's live-tree children — G6c's tier 1–4 lookups — lives on `App`
in `override_apply.rs`, below, since it needs `self.tree`/
`self.overrides`/`self.ctx.pool()`.)

### `protolens/src/tui/override_apply.rs`

New helper, replacing the previous draft's `own_path_field_overrides`
(now two-tiered, and consumed by G6c rather than implementing the
whole rule itself):

```rust
/// `idx`'s own active override target for field number `field`, tried
/// in priority order (spec 0156 G6c tiers 1–2): an active `PathField`
/// override whose `path` is exactly `idx`'s own positional path, else
/// an active `FqdnField` override whose `fqdn` matches `idx`'s own
/// resolved `type_fqdn` (only tried when that resolves). `Path`-kind
/// overrides are never consulted here — see N4.
pub(super) fn own_field_override(&self, idx: usize, field: u64) -> Option<Option<String>> {
    let path = self.positional_path(idx);
    let path_field = self.overrides.entries().iter().find_map(|e| match &e.origin {
        OverrideOrigin::PathField { path: p, field: f }
            if e.active && *p == path && *f == field =>
        {
            Some(e.r#type.clone())
        }
        _ => None,
    });
    path_field.or_else(|| {
        let fqdn = self.tree[idx].span.type_fqdn.as_deref()?;
        self.overrides.entries().iter().find_map(|e| match &e.origin {
            OverrideOrigin::FqdnField { fqdn: f, field: fld }
                if e.active && f == fqdn && *fld == field =>
            {
                Some(e.r#type.clone())
            }
            _ => None,
        })
    })
}

/// Builds the `ResolvedField` list for `export --descriptor-*` (spec
/// 0156 G6c): one entry per distinct field_number among `idx`'s direct
/// live-tree children, typed via `own_field_override` (tiers 1–2),
/// `natural_type` (tier 3), or a wire-type-derived primitive guess
/// (tier 4). `Err` only for an untyped `WT_START_GROUP` field (G6c) or
/// an override target that doesn't resolve in `ctx.pool()`.
pub(super) fn resolve_export_fields(
    &self,
    idx: usize,
) -> Result<Vec<export_descriptor::ResolvedField>, String> {
    // Walk `first_child`/`next_sibling`, group by `field_number`
    // (first occurrence wins for name/wire_type/label-multiplicity),
    // then for each group resolve type/type_name/referenced_file via
    // the tier 1-4 priority described in G6c, erroring per the rules
    // there. Implementation detail; shape only.
    todo!()
}
```

### `protolens/src/tui/command_line.rs`

`run_command` dispatch:

```rust
match resolve_command(name) {
    Ok("export") => self.run_export(tokens.collect()),
    Ok("quit") => self.should_quit = true,
    Ok("type-as") => self.run_type_as(tokens.collect()),
    Ok("type-as-raw") => self.run_type_as_raw(),
    Ok("save") => self.run_save_overrides(tokens.collect()),
    Ok("restore") => self.run_restore_overrides(tokens.collect()),
    Ok("proto-root") => self.run_proto_root(tokens.collect()),
    Ok(other) => unreachable!("resolve_command returned unregistered command: {other}"),
    Err(e) => self.message = e,
}
```

(`run_save_overrides`/`run_restore_overrides` function *names* are
left as-is internally — only the user-facing command names change;
renaming the Rust functions themselves is optional polish, not
required by G10.)

`start_tab_completion`'s argument-completion dispatch: `Ok("save-
overrides") | Ok("restore-overrides")` → `Ok("save") | Ok("restore")`.

`run_extract` renamed to `run_export`, gaining the two descriptor
formats (G2/G6/G7) and G1's message-text renames:

```rust
#[derive(PartialEq)]
enum ExportFormat {
    Binary,
    Prototext,
    DescriptorBinary,
    DescriptorPrototext,
}

/// `export [--binary|--prototext|--descriptor-binary|
/// --descriptor-prototext] <path>` — default format is `#@ prototext`
/// text (0113 D21). The two `--descriptor-*` flags build and write a
/// `FileDescriptorSet` per spec 0156 G6/G7, instead of slicing the
/// node's own data.
pub(super) fn run_export(&mut self, args: Vec<&str>) {
    let mut format = ExportFormat::Prototext;
    let mut path_parts = Vec::new();
    for a in args {
        match a {
            "--binary" => format = ExportFormat::Binary,
            "--prototext" => format = ExportFormat::Prototext,
            "--descriptor-binary" => format = ExportFormat::DescriptorBinary,
            "--descriptor-prototext" => format = ExportFormat::DescriptorPrototext,
            other => path_parts.push(other),
        }
    }
    if path_parts.is_empty() {
        self.message = "export: missing path".to_string();
        return;
    }
    let path = path_parts.join(" ");
    match format {
        ExportFormat::DescriptorBinary | ExportFormat::DescriptorPrototext => {
            let as_prototext = format == ExportFormat::DescriptorPrototext;
            self.message = match self.export_descriptor(&path, as_prototext) {
                Ok(()) => format!("exported to {path}"),
                Err(e) => e,
            };
        }
        ExportFormat::Binary | ExportFormat::Prototext => {
            let extract_format = if format == ExportFormat::Binary {
                extract::ExtractFormat::Binary
            } else {
                extract::ExtractFormat::Text
            };
            let node = &self.tree[self.cursor];
            match extract::extract(Path::new(&path), extract_format, &self.blob, &self.lines, node) {
                Ok(()) => self.message = format!("exported to {path}"),
                Err(e) => self.message = format!("export error: {e}"),
            }
        }
    }
}

/// Shared core of `xdb`/`xdp` (TUI) and batch's
/// `--format=descriptor-binary`/`descriptor-prototext` (spec 0156 G6/
/// G7): resolves the cursor node's synthetic fields (G6a–c), builds
/// the `FileDescriptorSet` (`export_descriptor::build`), and writes it
/// to `path` — as raw bytes, or (`as_prototext`) rendered through the
/// `#@ prototext` pipeline against G7's located meta-schema.
pub(crate) fn export_descriptor(&self, path: &str, as_prototext: bool) -> Result<(), String> {
    let span = &self.tree[self.cursor].span;
    if !span.is_message {
        return Err("export --descriptor: cursor node is not a message/group".to_string());
    }
    let message_name = export_naming::synthetic_message_name(self.field_name_for(self.cursor));
    let fields = self.resolve_export_fields(self.cursor)?;
    let fds = export_descriptor::build(&message_name, fields);
    use prost::Message as _;
    let bytes = fds.encode_to_vec();
    if as_prototext {
        let fds_type = export_descriptor::locate_file_descriptor_set_type(self.ctx.pool())
            .ok_or_else(|| {
                "export --descriptor --prototext: no descriptor.proto (with \
                 FileDescriptorSet/FileDescriptorProto messages) found in the \
                 loaded --descriptor-set; use --descriptor-binary instead, or \
                 rebuild the schema-db so descriptor.proto is included"
                    .to_string()
            })?;
        let text = /* render `bytes` as prototext against `fds_type`, same
                       pipeline `:export --prototext` already uses for data */;
        std::fs::write(path, text).map_err(|e| format!("export error: {e}"))
    } else {
        std::fs::write(path, bytes).map_err(|e| format!("export error: {e}"))
    }
}
```

(`export_naming::synthetic_message_name` is G6b's small capitalize/
digit-prefix helper, sketched here for shape rather than as exact
code.)

`run_save_overrides`/`run_restore_overrides` message-text renames
(`"save-overrides: ..."` → `"save: ..."`, `"restore-overrides: ..."` →
`"restore: ..."`, `"save-overrides error: ..."` → `"save error: ..."`,
etc.) — mechanical, no behavior change.

### `protolens/src/tui/manage_pane.rs`

`s`/`r` shortcut pre-fill text:

```rust
let buf = format!("save {}", self.default_save_overrides_path());
...
let buf = "restore ".to_string();
```

### `protolens/src/main.rs`

```rust
enum Command {
    /// Export one node's rendering (or, for the two descriptor
    /// formats, its synthetic schema) and exit, without entering the
    /// interactive TUI.
    Export {
        /// Field path of the node to export, in positional-path
        /// notation ...
        path: String,

        #[arg(long = "load-overrides")]
        load_overrides: Option<PathBuf>,

        /// Output format. Defaults to `prototext`.
        #[arg(long = "format", value_enum)]
        format: Option<ExtractFormatArg>,

        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
enum ExtractFormatArg {
    Binary,
    Prototext,
    DescriptorBinary,
    DescriptorPrototext,
}
```

(clap's `ValueEnum` derive kebab-cases variant names by default, so
`DescriptorBinary`/`DescriptorPrototext` already parse as
`--format=descriptor-binary`/`--format=descriptor-prototext` with no
extra `#[value(name = ...)]` attributes needed — matching G8.)

Batch dispatch (`Some(Command::Export { path, load_overrides, format,
output })` arm), G9's check inserted right after `--load-overrides` is
applied, ahead of path resolution:

```rust
let is_descriptor = matches!(
    format,
    Some(ExtractFormatArg::DescriptorBinary) | Some(ExtractFormatArg::DescriptorPrototext)
);
if is_descriptor && load_overrides.is_none() {
    eprintln!(
        "error: --format=descriptor-binary/descriptor-prototext requires \
         --load-overrides (batch mode has no interactive type-inference loop)"
    );
    return ExitCode::FAILURE;
}
```

then, where the old code resolved `idx`/wrote `bytes` for the plain
`extract` path, the two descriptor formats take the `App::
export_descriptor` path instead of `App::extract_bytes`:

```rust
let Some(idx) = app.resolve_path(&path) else { ... };
app.set_cursor(idx); // export_descriptor reads self.cursor, mirroring the TUI
if is_descriptor {
    let as_prototext = format == Some(ExtractFormatArg::DescriptorPrototext);
    if let Err(e) = app.export_descriptor(&output_path_as_str, as_prototext) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    return ExitCode::SUCCESS;
}
// ... existing extract_bytes path for Binary/Prototext, unchanged ...
```

(Batch's `-o`/`--output`-vs-stdout choice for either `--descriptor-*`
format: since both are inherently writing a `FileDescriptorSet`'s
bytes or text, the existing stdout-writing branch is reused unchanged
— no `-o` writes to stdout, same as `--binary`/`--prototext` do
today.)

## Test plan

- `protolens/src/tui/tests/key_dispatch.rs` (or a new test module):
  - `x` then `b`/`p` each pre-fill `command_buffer` with `export
    --binary/--prototext <path>` and open the command line
    (`command_kind == Command`).
  - `x` then `d` then `b`/`p` each pre-fill `command_buffer` with
    `export --descriptor-binary/--descriptor-prototext <path>` and open
    the command line.
  - `x` then any other key (e.g. `j`) clears `pending_x` back to
    `None`, does *not* open the command line, and still performs that
    key's normal action (cursor moves for `j`).
  - `x` then `d` then any other key (e.g. `j`) clears `pending_x` back
    to `None`, does *not* open the command line, and still performs
    that key's normal action.
  - `x` then `x` again: chord cancels (no pre-fill), `pending_x` stays
    `None` afterward (not re-armed).
  - Plain `x` alone, or `x` then `d` alone (no follow-up key yet):
    `command_buffer` stays `None`.
- `default_export_descriptor_path` unit tests:
  - cursor at `first_node` (root) → segment is `no range`, extension
    `.desc`.
  - cursor on a node whose parent's schema resolves its field name →
    segment is that field name.
  - cursor on a node with no resolvable schema field name, not the
    root → segment is the numeric `<start>-<end>` range (same as
    `default_extract_path`).
- G6b naming unit tests (whatever module hosts
  `synthetic_message_name`):
  - a resolvable field name, e.g. `"person"` → `"Person"`.
  - the field-number fallback, e.g. `"5"` → `"F5"`.
  - the document-root case, `field_name_for(first_node) == "1"` →
    `"F1"`.
- G6c field-naming unit tests (`synthetic_field_name`):
  - a resolvable field name, e.g. `"inner"` → `"inner"` (case
    untouched, unlike `synthetic_message_name`).
  - the field-number fallback, e.g. `"5"` → `"f5"`.
  - the document-root case, `field_name_for(first_node) == "1"` →
    `"f1"`.
- `export_descriptor::build`/`resolve_export_fields` unit tests
  (fixture `DescriptorPool`s and live trees built the same hand-crafted
  way as `extract.rs`'s existing round-trip test):
  - a cursor with only primitive-wire-type children, no schema, no
    overrides → each child's guessed keyword matches G6c's table,
    `dependency` is empty.
  - a cursor whose schema resolves a child's natural type to a message
    defined in a *different* file → that file's name is listed in the
    synthetic file's `dependency`, but its content is not embedded (the
    output set has exactly one file); a second child field referencing
    a type in the exported node's *own* original file also succeeds and
    is declared too (confirming G6d's "no exclusion" behavior).
  - an active `PathField` override at the cursor's own path retypes the
    matching field to the override's target FQDN (tier 1); an active
    `FqdnField` override at the cursor's own `type_fqdn` is used only
    when no matching `PathField` override exists (tier 2); an override
    at a *different* path (e.g. a descendant's) has no effect (N4).
  - an active override with target `None` (raw) turns that field into
    `bytes`.
  - two live children sharing one field_number collapse to one
    exported field, `LABEL_REPEATED`.
  - a `WT_START_GROUP` child with no resolvable/overridden type →
    `Err(...)`.
  - cursor node is a scalar leaf (not `is_message`) → `App::
    export_descriptor` returns `Err("... is not a message/group")`.
- `export_descriptor::locate_file_descriptor_set_type` unit tests:
  - a pool containing a file named `google/protobuf/descriptor.proto`
    with both `FileDescriptorSet`/`FileDescriptorProto` messages →
    resolves.
  - the same, but the file named `proto2/descriptor.proto`
    and packaged `proto2` (uncanonized) → still resolves (name-suffix +
    simple-name match, package-agnostic — G7).
  - a pool with no `...descriptor.proto`-suffixed file → `None`.
  - a pool with a `...descriptor.proto`-suffixed file missing one of
    the two required messages → `None`.
- `protolens/src/tui/tests/command_line.rs`:
  - `resolve_command("export")`/`"save"`/`"restore"` resolve correctly
    (including unambiguous prefixes); `"extract"`, `"save-overrides"`,
    `"restore-overrides"` no longer resolve (`COMMANDS` no longer lists
    them).
  - `:export --binary`/`--prototext <path>` round-trip exactly as
    `:extract --binary`/`--text` did before this spec.
  - `:export --descriptor-prototext <path>` against a `--descriptor-set`
    lacking `descriptor.proto` → G7's error message, no file written.
  - `Tab` completion still completes filesystem paths for `save`/
    `restore`'s argument.
- Batch CLI (`main.rs` integration or unit test, mirroring spec 0123's
  existing `extract` tests):
  - `export --format=descriptor-binary <path>` without
    `--load-overrides` → `ExitCode::FAILURE`, stderr mentions
    `--load-overrides` (G9).
  - `export --format=descriptor-binary --load-overrides <file> <path>`,
    target eligible (G6a) → succeeds, writes a decodable
    `FileDescriptorSet`.
  - `export --format=descriptor-prototext --load-overrides <file>
    <path>`, target eligible, `--descriptor-set` includes
    `descriptor.proto` → succeeds, writes prototext.
  - `export --format=binary`/`--format=prototext <path>` still succeed
    exactly as `extract --format=binary`/`--format=text` did before.
