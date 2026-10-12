<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0415 — prototext on upb, Google's C protobuf runtime

Status: draft
App: prototext, prototext-core, prototext-schema, prototext-pyo3
Refs: docs/specs/0100-message-set-expansion.md (MessageSet extensions
      loaded on demand: what S7 keeps);
      docs/specs/0106-lazy-pool-nested-schema-staleness.md and
      docs/specs/0197-the-descriptor-set-is-loaded-on-demand.md (the lazy
      pool, and the copy-on-write trap this removes);
      docs/specs/0248-an-extension-resolves-without-loading-the-world.md
      (lazy extension loading, kept);
      docs/specs/0171-wire-format-bounds-arithmetic-and-recursion-depth-caps.md
      (the hardened wire parser S4 reuses as the gate);
      docs/specs/0266-a-payload-with-any-flaw-in-it-is-not-a-message.md
      (the probe rule S4 applies);
      docs/specs/0370-packed-on-the-wire-renders-as-packed.md (packed
      records render as packed whatever the field declares: unchanged);
      docs/specs/0153-score-one-single-entry-fast-path.md (the decode
      timings S12 repeats); docs/specs/0090-cli-review.md (removed
      `instantiate-schema`, so S9 can remove its module)

## Background

prototext reads schemas through prost-reflect 0.16, which does not
support editions: any `syntax` other than `proto2` or `proto3` is
`DescriptorErrorKind::UnknownSyntax`. A pool is built from a whole set at
once (`DescriptorPool::decode`), so one editions file fails the set.

Editions are not marginal. On the descriptors extracted from one
production package (measured downstream): 2,797 distinct files, of which
1,441 are editions (932 `EDITION_2023`, 508 `EDITION_2024`, 1
`EDITION_UNKNOWN`), 941 proto2 and 415 proto3. Rendering that package's
own descriptors needs 836 of them, 231 editions. On that set,
`prototext --descriptor-set … decode` does not report the error: it
panics inside prost-reflect while formatting it (`index out of bounds`,
`prost-reflect-0.16.3/src/descriptor/error.rs:782`).

prost-reflect is also the source of two known faults the workspace works
around: its `is_packed()` is wrong for proto3 fields whose options carry
only an extension (docs/prototext/PROST-ISSUES.md), and its pool forks on
every load while a descriptor is held (specs 0106, 0197, 0248).

upb, Google's C runtime, is the engine of the Python, Ruby and PHP
protobuf runtimes. reproto already runs on it, through Python's
`google._upb._message`, so the scoring automaton is built from upb's
resolution of packing, delimited encoding and enum closedness. Its
single-file build (`upb.c`, `upb.h`) contains a descriptor pool and
feature-resolved field queries (`upb_FieldDef_IsPacked`, `_HasPresence`,
`_IsRequired`, `_Type` returning group for delimited encoding,
`upb_FileDef_Edition`), and messages with reflection that keep unknown
fields and extensions. Its C interface is versioned with protobuf and
carries no stability promise; private symbols are marked `UPB_PRIVATE(…)`
or prefixed `_upb_`.

## Goals

- **G1.** prototext loads descriptor sets containing editions files
  (2023, 2024) with Google's own feature resolution.
- **G2.** Loading tolerates incomplete and hostile sets: a file whose
  imports are missing, or that upb refuses, is skipped and reported;
  every loadable file is loaded. No input panics or crashes.
- **G3.** For schemas made only of proto2 and proto3 files, prototext's
  output is byte-identical to the last prost-reflect build (S11). Every
  difference is explained, and either fixed or listed in this spec as a
  prost-reflect fault.
- **G4.** upb is the only protobuf engine of the prototext crates:
  prost-reflect, prost and prost-types leave the `Cargo.toml` of
  `prototext-core`, `prototext-schema`, `prototext` and `prototext-pyo3`.
- **G5.** Performance is measured before and after, and every
  regression is explained (S12).
- **G6.** It builds wherever the workspace builds today: crates.io,
  nixpkgs, the workshop image. No system library, no libclang.

## Non-goals

- **N1. protolens and protoscan.** They get only the mechanical changes
  needed to build against `prototext-core`'s new API, with their tests
  green. Their own move to upb, and the removal of prost from the whole
  workspace, is a follow-up spec. Until then prost and prost-types stay
  in their `Cargo.toml`.
- **N2. reproto.** It stays Python. Its port to Rust on this wrapper is
  a later spec; S2 is shaped so that the wrapper can grow the message
  reflection reproto needs without a redesign.
- **N3.** Changing prototext's output format, its rendering heuristics,
  or the scoring automaton. `prototext-graph` does not use prost-reflect
  and is untouched; the graph format does not change.
- **N4.** upb's text-format encoder and generated-code support, and the
  `protobuf` crate.
- **N5.** The demos (`demo/bobapp`, `grehack2026/game`) keep their own
  prost dependency for their generated messages. They are not workspace
  members, but they depend on `prototext-core` by path and are adapted to
  its new API.
- **N6.** Two schema backends behind a Cargo feature. The comparison is
  against recorded outputs (S11); the branch is the fallback.

## Specification

### S1 — Vendored upb

- A new workspace crate, `prototext-upb`, holds a vendored copy of upb's
  single-file build (`upb.c`, `upb.h`, and `third_party/utf8_range`) from
  one pinned protobuf release, compiled by `build.rs` with `cc` and linked
  statically.
- `prototext-upb/vendor/VERSION` records the release tag, the source path
  of each file and its SHA-256. `scripts/update-upb <tag>` refreshes both.
- The pin matches, or follows closely, the protobuf of the workspace's
  nixpkgs pin, so that upb accepts every edition the pinned `protoc`
  produces. The newest edition prototext loads is the pin's; a newer one
  is a skipped file (S3), not an error.
- REUSE: the vendored files are annotated BSD-3-Clause, Google LLC, in
  `REUSE.toml`.
- The crate does not depend on `google-protobuf`: its bundled upb is an
  implementation detail of a beta crate.

### S2 — The FFI surface and the safe wrapper

- `extern "C"` declarations are written by hand (no `bindgen`), and only
  for public functions, never `_upb_…` or `UPB_PRIVATE` symbols. They
  cover the pool, file, message, field, oneof and enum definitions, and
  generic message reflection (decode, encode, get, set, iterate set
  fields), which S5 needs.
- What `upb.h` declares only inline (`UPB_INLINE`, `UPB_API_INLINE`) is
  re-exported under `prototext_upb_` names by a C shim compiled with upb
  (`prototext-upb/src/shim.c`). This includes parsing a serialized
  `FileDescriptorProto` into an arena, which `upb_DefPool_AddFile`
  requires. The exact list is fixed against the pinned `upb.h`.
- A test fails if any declaration has no linkable symbol, so an upgrade
  that turns a function inline is caught at once.
- **Handles are owned, not borrowed.** `Pool` is an
  `Arc<PoolInner>`; a definition handle (`Message`, `Field`, `Enum`, …)
  holds an `Arc` to the pool and a pointer. Handles are cheap to clone,
  `Send` and `Sync`, and keep the pool alive. Reason: the lazy pool adds
  files while handles are held (S7), and callers keep descriptors in
  long-lived state and hand them to threads; a lifetime-bound handle would
  forbid both.
- **Concurrency.** upb definitions are immutable once added and stay
  valid when more files are added, so queries on a handle already
  obtained (a field's type, a message's fields) need no lock. The pool's
  symbol tables, however, change when a file is added, so name and number
  lookups on the pool and `add_file` go through an `RwLock` inside
  `PoolInner`: shared for lookups, exclusive for adds. Files are added
  from inside rendering (the Any and MessageSet loader), so the wrapper
  documents and tests that an add never invalidates a handle, and that a
  lookup is never made while the same thread holds the write lock.
- Strings returned by upb are borrowed for the pool's lifetime or copied,
  never assumed NUL-terminated beyond what `upb.h` states.

### S3 — Loading a set

- `Pool::add_file(bytes) -> Result<File, LoadError>` adds one serialized
  `FileDescriptorProto`. Its imports must be in the pool already; a
  failed add leaves the pool unchanged. `LoadError` carries upb's status
  message.
- `Pool::load_set(bytes) -> (Pool, LoadReport)`:
  1. split the `FileDescriptorSet` into files;
  2. pass each through the gate (S4);
  3. order them so every file follows its imports;
  4. add them in that order;
  5. skip a file the gate rejects, whose import is missing or was
     skipped, or that upb refuses, and record `(name, reason)` in
     `LoadReport.skipped`. Files on an import cycle are skipped together.

  It never fails as a whole: a set with nothing loadable gives an empty
  pool and a full report.
- The pool keeps each loaded file's original bytes
  (`Pool::file_bytes(name)`): upb's definitions do not keep
  `source_code_info`, and the single-file build has no def-to-proto
  conversion. Options need no bytes: they are reachable from the
  definitions.

### S4 — The gate in front of upb

Every file's bytes pass prototext's own wire parser before reaching
upb's C parser. A file is admitted only if it parses as a message that
consumes its bytes exactly, with no malformation (the probe rule of spec
0266), within the depth cap of spec 0171. upb then parses only
well-formed protobuf; what it still sees from a hostile source is
adversarial content in well-formed messages, which its definition
builder validates and Google fuzzes continuously (OSS-Fuzz).

### S5 — Descriptor structures without prost

Code that reads or writes descriptor structures (`FileDescriptorSet`,
`FileDescriptorProto`, …) without reflection on them — splitting a set,
completion (`complete.rs`), tests that build fixtures — moves from
`prost-types` to a typed layer in `prototext-upb::descriptor`: readers
and builders for the fields the workspace uses, implemented with upb
message reflection over `descriptor.proto`'s own definitions. Unlike
prost, upb keeps unknown fields and extensions, so a descriptor that
passes through this layer loses nothing.

### S6 — The schema interface in `prototext-core`

- `prototext-core` stops naming any protobuf crate's types. Its `schema`
  module defines `Schema`, `Message`, `Field`, `Enum`, `Oneof`, `Kind`,
  `Cardinality` as its own types, implemented on S2:

| Today (prost-reflect) | Interface | upb |
|---|---|---|
| `DescriptorPool::decode` | `Schema::load(fds)` + `LoadReport` | S3 |
| `get_message_by_name`, `all_messages` | `Schema::message`, `messages` | `upb_DefPool_FindMessageByName`; the pool's files |
| `MessageDescriptor::get_field`, `fields`, `full_name`, `name`, `parent_file` | `Message::field(n)`, `fields`, `full_name`, `name`, `file` | `upb_MessageDef_*` |
| `get_extension` | `Schema::extension(extendee, n)` | `upb_DefPool_FindExtensionByNumber` |
| `FieldDescriptor::kind` | `Field::kind` | `upb_FieldDef_Type`, `_MessageSubDef`, `_EnumSubDef` |
| `cardinality` | `Field::cardinality` | `upb_FieldDef_IsRepeated`, `_IsRequired` |
| `is_group` | `Field::is_group` | `upb_FieldDef_Type == kUpb_FieldType_Group` (covers delimited encoding) |
| `is_packed` | `Field::is_packed` | `upb_FieldDef_IsPacked` (feature-resolved) |
| `parent_file().syntax()` (presence, `render_text/mod.rs`) | `Field::has_presence` | `upb_FieldDef_HasPresence` |
| `is_map`, `is_synthetic` | `Message::is_map_entry`, `Oneof::is_synthetic` | `upb_MessageDef_IsMapEntry`, `upb_OneofDef_IsSynthetic` |
| `EnumDescriptor::get_value`, `values` | `Enum::value(n)`, `values` | `upb_EnumDef_FindValueByNumber`, `_Value` |
| `options.message_set_wire_format` (`message_set_field.rs`) | `Message::is_message_set_wire_format` | message options, read through S2 |

- The public API changes, so `prototext-core` goes to 0.4.0. `pub use
  prost_reflect::MessageDescriptor` (`lib.rs`) is removed;
  `render_as_text(data, root: Option<&schema::Message>, opts)` and
  `ParsedSchema::root_descriptor()` take the interface's type. Callers
  that only pass `root_descriptor().as_ref()` through need no change
  beyond the version. The release notes say so.
- `parse_schema` keeps its signature and fails only when the root message
  is missing or not loadable. Skipped files are in
  `ParsedSchema::load_report()`.
- The `prost-bug-workaround` feature goes: there is no prost-reflect left
  to work around.

### S7 — `prototext-schema` (lazy pool)

`lazy_pool.rs` keeps its design: a file is added the first time one of
its types is needed (spec 0100), after its imports, in order. A file
that cannot be added is recorded as skipped, as in S3, and lookups into
it return `None`. The rule "never hold a descriptor across a later load"
(specs 0106, 0197) no longer applies and its comments go.

### S8 — `prototext` and `prototext-pyo3`

- `run.rs`: `--descriptor-set` loads through S3. Each skipped file is a
  warning (`warning: <file>: skipped (<reason>)`); the command fails only
  if the requested `--type` is not loadable.
- `protocraft/mod.rs`: lookups by name go through the interface.
- `complete.rs` and the tests that build fixtures: S5.
- `prototext-pyo3`: follows S6; its Python API is unchanged.

### S9 — `instantiate`

`prototext-core::instantiate` has had no caller since spec 0090 removed
`prototext instantiate-schema`, and is its last `DynamicMessage` user. It
is removed; the release notes say so.

### S10 — Two upb copies in one Python process

reproto imports both `google.protobuf` (upb) and our extensions, which
now embed their own upb. The extensions must not export upb's C symbols,
so the two copies cannot bind to each other's functions, whatever their
versions.

### S11 — The comparison gate

Before any code changes, the last prost-reflect build records golden
outputs on:

- prototext's fixtures and test inputs;
- `wkt-db` and `googleapis-db`, rendering every root type's instances
  that the existing tests and benchmarks use;
- bobapp's descriptors and captures.

The upb build must reproduce them byte for byte. Each difference is
listed under "Measured outcome" with its cause. Expected ones, to confirm
rather than discover: prost-reflect's proto3 `is_packed` fault, and files
upb refuses where prost-reflect accepted them (upb validates more, for
example JSON-name conflicts). A file upb refuses is a skipped file under
G2, and its consequences on the output are listed.

### S12 — Benchmarks

Measured before (prost-reflect) and after (upb), same machine, pinned
cores (`docs/prototext/bench-process.md`), reported under "Measured
outcome":

- eager load of `googleapis-db` (the pool alone), and the time to drop it;
- lazy load of one root's import closure, for the roots specs 0197
  measured;
- `prototext decode --type` on a small and a 200 KB blob against
  `googleapis-db` (spec 0153's cases);
- the Criterion codec benchmark (`prototext-core/benches/codec.rs`);
- peak RSS of each.

No threshold decides the merge. Each result is reported with its noise,
and every regression is investigated and explained; the results are then
reviewed and the merge decided on them.

## Alternatives considered

### The `protobuf` crate (4.x)

No runtime reflection on its Rust side, and the release is labeled beta.
Depending on it only to reach its compiled upb would tie the workspace to
an implementation detail.

### rust-protobuf 3.7

Runtime reflection, but it reads editions files as proto2 without an
error: wrong packing and encoding instead of a refusal.

### Keep prost-reflect, and rewrite editions files to proto2/proto3 first

Avoids C, but reimplements feature resolution, where a subtle difference
from Google's rules would go unnoticed. prost-reflect would still panic
on some inputs and still load sets all at once.

### Add editions to prost-reflect upstream

The same reimplementation risk, in a crate we do not maintain, on its
schedule.

### Link a system upb

upb is rarely packaged on its own, its C interface changes between
protobuf versions, and crates.io users would need it installed.

### `bindgen`

Adds libclang to every build. The surface (S2) is small enough to write
and review by hand.

### Isolate upb (subprocess, WebAssembly)

Removes C from the process that handles hostile input, at the cost of a
process boundary or a WebAssembly runtime in a command-line decoder. The
gate (S4), upb's own fuzzing and ours (test plan 3) cover the risk at a
fraction of the cost.

### Two backends behind a Cargo feature

Gives a runtime fallback after the switch, at the cost of writing and
keeping the S6 interface twice. Recorded outputs (S11) give the same
evidence, and the branch gives the fallback.

## Test plan

1. **Wrapper and feature resolution.** Editions 2023 and 2024 fixtures
   produced by the pinned `protoc` cover delimited fields, packed and
   expanded repeated fields, `LEGACY_REQUIRED`, closed enums, and feature
   inheritance at file, message and field level. Their resolved
   attributes (`is_packed`, `has_presence`, `is_group`, `cardinality`)
   match what Python protobuf reports for the same set. Python protobuf
   is upb too, so this checks our wiring, not upb's semantics.
2. **Loading never panics.** Each yields a `LoadReport`:
   - a missing import (the file and its dependents skipped, the rest
     loaded);
   - an import cycle;
   - a duplicate file name;
   - an `EDITION_UNKNOWN` file, and an edition newer than the pin;
   - a file the gate rejects (truncated, malformed);
   - random bytes.
3. **Fuzzing.** A `cargo-fuzz` target on `Pool::load_set`, run briefly in
   CI and at length before merge.
4. **Handles.** A handle obtained before a later `add_file` stays valid
   and equal; handles are used from several threads at once.
5. **The panic.** The set that made prost-reflect panic (kept
   downstream) loads, and its report lists what was skipped.
6. **The gate (S11)** passes, with every difference listed.
7. **Two upb copies.** A Python process imports `google.protobuf`, loads
   a descriptor through it, imports `prototext_codec`, renders, and both
   work; `nm -D` on the extension shows no `upb_` symbol.
8. **Downstream acceptance (outside this repository's CI).** On the
   production set above, an extension declared in an editions file
   renders by name, not by number.
9. **Benchmarks (S12)** are run, and their results and explanations are
   recorded under "Measured outcome" for review.
10. `ci`, `nixpkgs-staging`, `cargo package --verify` for each changed
    crate, and `reuse lint` pass; protolens's and protoscan's tests pass
    unchanged.

## Measured outcome

Filled in at implementation.
