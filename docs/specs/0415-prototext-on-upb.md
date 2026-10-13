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

- **N1. protolens.** protolens builds its own schema pool and passes its
  descriptors into `prototext-core`'s renderer, so it cannot stay on
  prost-reflect once `prototext-core` moves, and moving it is not
  mechanical. On the `upb` branch it is left out of the Cargo workspace
  (its sources unchanged) and the dev shell skips building it; the Nix
  outputs that include it are not built on the branch. Its move to upb
  is the branch's second step, after this one succeeds, and must land
  before the branch is merged.
- **N1b. protoscan.** It uses `prost-types` only, to read descriptor
  structures, and is unaffected. Its move off prost belongs with
  protolens's.
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
- Source: Google publishes upb's single-file build only inside the
  `google-protobuf` crate (`libupb/`), and crates.io carries only its
  0.36 releases. The pin is therefore protobuf v36.2, taken from
  `google-protobuf` 0.36.2-release, newer than the nixpkgs pin (34.1),
  which it accepts. The files are copied from the crate's tarball; the
  crate itself is never a dependency.
- REUSE: the vendored files are annotated BSD-3-Clause, Google LLC, in
  `REUSE.toml`.
- The crate does not depend on `google-protobuf`: its bundled upb is an
  implementation detail of a beta crate.

### S2 — The FFI surface and the safe wrapper

- `extern "C"` declarations are written by hand (no `bindgen`), and only
  for public functions, never `_upb_…` or `UPB_PRIVATE` symbols. They
  cover the pool, file, message, field, oneof and enum definitions; no
  message reflection (S5 needs none).
- upb.c is compiled with `UPB_BUILD_API`, which makes upb's
  `UPB_API_INLINE` functions real symbols, as Google's own Rust binding
  does. Only upb.c may be: the header then *defines* those functions, and
  a second translation unit including it would define them twice.
- What `upb.h` declares `static inline` is re-exported by a C shim
  (`prototext-upb/src/shim.c`), compiled separately, without
  `UPB_BUILD_API`. It holds two functions:
  - `prototext_upb_add_file`: parses a serialized `FileDescriptorProto`
    into an arena (the generated parser is inline), adds it, and frees the
    arena, so upb's arenas and status struct stay on the C side; upb
    copies everything it keeps into the pool;
  - `prototext_upb_message_set_wire_format`: the message option, read
    through the generated accessor.
- Both are compiled with `-fvisibility=hidden` (S10).
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
- Names are borrowed for the handle's lifetime, not re-validated as
  UTF-8 on each call: upb checks every identifier it builds to be
  `[A-Za-z_][A-Za-z0-9_]*` joined by dots, and the one name it does not
  check, a file's, is checked to be UTF-8 before the file reaches upb. A
  debug assertion keeps the claim honest. `json_name`, which upb stores
  unchecked, is converted lossily.

### S3 — Loading a set

- `Pool::add_file(&self, bytes) -> Result<File, LoadError>` adds one
  serialized `FileDescriptorProto`. Its imports must be in the pool
  already; a failed add leaves the pool unchanged. A second file of the
  same name is refused, whatever its content (prost-reflect skipped it
  silently, so a test could add a file it believed was its own and get
  another). `LoadError` carries upb's status message.
- `Pool::load_set(&self, bytes) -> LoadReport`:
  1. split the `FileDescriptorSet` into files, reading only each file's
     name and imports, in safe Rust;
  2. order them so every file follows its imports, walking iteratively so
     no chain of imports exhausts the stack;
  3. add them in that order, each through the gate (S4);
  4. skip a file whose name is unreadable or a duplicate, whose import is
     missing or was skipped, or that the gate or upb refuses, and record
     `(name, reason)` in `LoadReport.skipped`. Files on an import cycle
     are skipped together.

  It never fails as a whole: bytes that are not a `FileDescriptorSet` at
  all set `LoadReport.set_error`, and nothing loads.
- The pool keeps each loaded file's original bytes
  (`Pool::file_bytes(name)`): upb's definitions do not keep
  `source_code_info`, and the single-file build has no def-to-proto
  conversion. Options need no bytes: they are reachable from the
  definitions.

### S4 — The gate in front of upb

`prototext-upb` sits below `prototext-core`, whose wire parser the gate
uses, so a pool takes its gate as a parameter (`Pool::with_gate`) and
`prototext-core` supplies it (`schema::new_pool`): every pool it builds
passes each file through spec 0266's probe before upb's parser sees it.
A file is admitted only if it reads as one well-formed message: every
top-level field framed, the bytes consumed exactly, no malformation, no
forgiven tail cut.

The probe checks one level: nested payloads are framed by lengths it
checks, but what is inside them is upb's to parse. So upb never sees a
broken top-level frame, but can see damaged bytes inside a field. Its
parser is Google's most fuzzed code (OSS-Fuzz), and test plan 3 adds
mutated sets of our own, with upb compiled under AddressSanitizer.

### S5 — Descriptor structures without prost

Code that reads or writes descriptor structures without reflection —
splitting a set, completion (`complete.rs`), the lazy pool's fallback
files, tests that build fixtures — moves from `prost-types` to
`prototext_upb::descriptor`: plain structs with `prost-types`' names and
fields, for the parts of `descriptor.proto` the workspace uses, with
`encode_to_vec`, `encode` and `decode` as prost spells them. Code moving
off prost changes its imports and little else.

The layer is pure Rust, not upb message reflection: building and reading
a few fields needs no reflection, and keeping it out of C keeps the FFI
surface to definitions. Decoding keeps the fields declared and skips the
rest, so nothing round-trips through it: a descriptor that must stay
whole reaches a pool as its original bytes (`descriptor::files_of_set`),
and the pool keeps them (S3).

### S6 — The schema interface in `prototext-core`

- `prototext-core::schema` re-exports `prototext-upb`'s handles
  (`Pool`, `File`, `Message`, `Field`, `Oneof`, `Enum`, `EnumValue`,
  `Kind`, `Cardinality`, `LoadReport`) and its descriptor layer, so
  callers name `prototext_core::schema::Message`, never a protobuf crate.
  A second set of types wrapping the first would add a layer and nothing
  else.
- The handles keep the prost-reflect method names the renderer used
  (`get_field`, `get_extension`, `kind`, `cardinality`, `is_group`,
  `get_value`, …), so the renderer's change is mostly its imports:

| prost-reflect | upb |
|---|---|
| `DescriptorPool::decode` | `schema::decode_pool` → `Pool::load_set` (S3), gated (S4) |
| `get_message_by_name`, `all_messages` | `upb_DefPool_FindMessageByNameWithSize`; the pool's files, walked |
| `MessageDescriptor::get_field`, `fields`, `full_name`, `name`, `parent_file` | `upb_MessageDef_*` |
| `MessageDescriptor::get_extension` | `upb_DefPool_FindExtensionByNumber` |
| `FieldDescriptor::kind` | `upb_FieldDef_Type`, `_MessageSubDef`, `_EnumSubDef` |
| `cardinality` | `upb_FieldDef_IsRepeated`, `_IsRequired` |
| `is_group` | `upb_FieldDef_Type == kUpb_FieldType_Group` (delimited encoding too) |
| declared packing, from options and syntax (spec 0371) | `upb_FieldDef_IsPacked` (feature-resolved) |
| `EnumDescriptor::get_value`, `values` | `upb_EnumDef_FindValueByNumber`, `_Value` |
| `options.message_set_wire_format` | the shim (S2) |

- The renderer's `FieldOrExt` holds one `Field` for a field or an
  extension, the variant only choosing how it is named on a line; it
  resolves kind, cardinality, framing and declared packing once, when
  built, because each is a call into upb (S12). A group-typed extension
  is now framed as a group; prost-reflect answered `false` for every
  extension.
- The gate (S4) is `prototext-core`'s: `schema::gate`, built on
  `render_text::probes_as_message`, and `schema::new_pool`, which every
  pool `prototext-core` builds uses.
- The public API changes, so `prototext-core` goes to 0.4.0. `pub use
  prost_reflect::MessageDescriptor` (`lib.rs`) is removed;
  `render_as_text(data, root: Option<&schema::Message>, opts)` and
  `ParsedSchema::root_descriptor()` take the interface's type. Callers
  that only pass `root_descriptor().as_ref()` through need no change
  beyond the version. The release notes say so.
- `parse_schema` keeps its signature and fails only when the bytes are
  not a set, or the root message is missing or not loadable; that error
  lists the skipped files. Skipped files are in
  `ParsedSchema::load_report()`. `decode_pool` returns the report beside
  the pool.
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
3. **Damaged input, with upb's C under AddressSanitizer.** 20,000
   mutations of the editions fixture (bit flips, insertions, deletions,
   truncations) and 2,000 random byte strings are loaded with no gate, so
   upb parses all of it, and every definition that loads is walked.
   `scripts/upb-asan` runs those tests with upb.c compiled with
   `-fsanitize=address`, and checks the binaries are instrumented.
   Coverage-guided fuzzing goes to ClusterFuzzLite, Google's GitHub
   Action, rather than a workflow of our own: a `cargo-fuzz` target on
   `Pool::load_set` and `Pool::add_file`, run briefly on each change to
   `prototext-upb` and at length on a weekly schedule that skips weeks
   with no push, with its corpus kept between runs and crashes reported.
   It brings the nightly toolchain and sanitizers in its own image, which
   the dev-shell does not have. Set up after the branch's second step.
4. **Handles.** A handle obtained before a later `add_file` stays valid
   and equal; handles are used from several threads at once.
5. **The panic.** The set that made prost-reflect panic (kept
   downstream) loads, and its report lists what was skipped.
6. **The gate (S11)** passes, with every difference listed.
7. **Two upb copies.** A Python process uses `google.protobuf`'s pool,
   then the extension's, then Python's again, and all three answer; the
   extension's dynamic symbol table holds no `upb_` symbol, checked with
   `ctypes` (which finds only exported symbols) in its pytest suite.
8. **Downstream acceptance (outside this repository's CI).** On the
   production set above, an extension declared in an editions file
   renders by name, not by number.
9. **Benchmarks (S12)** are run, and their results and explanations are
   recorded under "Measured outcome" for review.
10. `ci`, `cargo package --verify` for each publishable crate but
    protolens, and `reuse lint` pass; protoscan's tests pass unchanged.
    (`nixpkgs-staging` and protolens's tests belong to the branch's second
    step, N1.)

## Measured outcome

Implemented 2026-10-13, up to the branch's second step (protolens, N1).
Measured on the development VM (Intel Core Ultra 7 165U E-cores), pinned
to one core with the bench lock, against the last prost-reflect build of
the same commit.

- **S1–S3:** `prototext-upb` vendors upb from protobuf v36.2 (1.5 MB,
  241 KB as a `.crate`). 30 tests: the editions table of test plan 1
  matches Python's upb on all 15 fields; every load-report case of test
  plan 2; handles across adds and threads.
- **S4–S9:** prost, prost-reflect and prost-types are gone from the
  dependency trees of `prototext-upb`, `prototext-core`,
  `prototext-schema`, `prototext` and `prototext_codec_lib` (checked with
  `cargo tree`); `workspace-hack` was regenerated by cargo-hakari to
  match. protoscan keeps `prost-types` (N1b).
- **S10:** the extension exports no upb symbol; 12 of 12 extension tests
  pass beside Python's protobuf.
- **S11:** 1,516 of 1,516 golden cases byte-identical (stdout, stderr and
  exit status): 375 googleapis instances decoded typed, typed without
  annotations, inferred and scored; the repository's instances; three
  descriptor sets decoded as descriptors; the anomaly fixture typed and
  raw; a type that does not exist. No difference to explain.
- **Tests:** 420 workspace tests pass (the 231 of the crates in scope
  before the change, plus the wrapper's and two schema tests); clippy and
  rustfmt clean; nine publishable crates package and build from their
  `.crate` files; `reuse lint` passes.
- **S12, reported for review:**

| Benchmark | prost-reflect | upb | Change |
|---|---|---|---|
| eager load of googleapis.desc (CLI, no sidecar) | 2.070 s ± 0.048 | 0.703 s ± 0.034 | −66% |
| its peak RSS | 722,860 KiB | 252,088 KiB | −65% |
| lazy load of one root's closure | 6.6 ms ± 0.5 | 5.6 ms ± 0.8 | within noise |
| decode with inference | 33.5 ms ± 1.3 | 33.6 ms ± 2.9 | within noise |
| decode against the built-in WKT | 2.7 ms ± 1.1 | 2.7 ms ± 0.7 | within noise |
| decode googleapis.desc (25.6 MB) as a set | 856.6 ms ± 33.0 | 815.5 ms ± 28.1 | −4.8% |
| Criterion A1, no schema | 215.3 µs | 203.2 µs | −4.3% |
| Criterion A2, schema and annotations | 370.0 µs | 390.1 µs | **+5.8%** |
| Criterion B1, encode | 623.0 µs | 621.5 µs | no change |

  - The 25.6 MB decode first measured +15% (988.9 ms). A profile put it in
    per-occurrence calls into upb (`Field::kind` 7.8%, feature lookups
    about 10%) and UTF-8 re-validation of names (6.5%); resolving a
    field's properties once per occurrence (S6) and trusting upb's
    identifier check (S2) turned it into −4.8%, golden cases unchanged.
  - A2 renders a small document with many distinct fields, so the
    remaining per-occurrence lookup dominates it. Unchanged schema-free
    benchmarks drifted by 0 to +4% in the same run, so its true cost is
    about 2 to 6%. Spec 0416 proposes a per-render field cache for it.
- **A behaviour change, and a bug it exposed.** A group-typed extension
  (`extend Host { optional group Ext = 100 {…} }`) rendered under
  prost-reflect as an untyped `100 { … }` marked `TYPE_MISMATCH`, because
  prost-reflect answers `is_group() == false` for every extension. upb
  resolves the group, and the first upb build named it by its group type,
  `Ext {`, where protoc writes `[g.ext] {`: the group header ignored that
  the field was an extension, a path no extension had reached before. It
  now writes `[g.ext]`, as protoc does, and a test pins both the text and
  the byte-exact round trip. No golden case has a group extension.
- **Not done:** the ClusterFuzzLite setup (test plan 3), after the
  branch's second step; test plan 5 and 8, which are downstream.
