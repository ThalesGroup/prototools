<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0417 — protolens on upb

Status: implemented
Implemented in: 2026-10-13
App: protolens (prototext-lens), prototext-upb
Refs: docs/specs/0415-prototext-on-upb.md (the wrapper, the gate, the
      descriptor layer, the comparison and benchmark method this reuses;
      N1, which this ends);
      docs/specs/0416-prototext-on-upb-performance.md (the per-render
      cache, which protolens renders through too);
      docs/specs/0197-the-descriptor-set-is-loaded-on-demand.md (the lazy
      pool, and the equal-namespaces claim S3 revisits);
      docs/specs/0135-protolens-override-raw-tag-rewrap.md and
      docs/specs/0137-protolens-override-primitive-and-enum-candidates.md
      (content-addressed synthetic wrappers);
      docs/specs/0144-protolens-neovim-jump-to-definition.md (jump to
      definition through `source_code_info`);
      docs/specs/0123-protolens-batch-mode.md,
      docs/specs/0198-a-batch-subcommand-that-only-starts-up.md and
      docs/specs/0271-a-script-walks-the-reader-through-the-blob.md (the
      batch commands the gate drives)

## Background

Spec 0415 moved prototext to upb and set protolens aside (its N1):
protolens builds its own schema pool, and its descriptors flow into
`prototext-core`'s renderer, so it cannot stay on prost-reflect once
`prototext-core` has moved. On the `upb` branch it is out of the Cargo
workspace, out of `ci`, and its Nix outputs are not built. The branch
cannot merge until it is back.

A survey of protolens (2026-10-13) found:

- **Where prost is used.** 16 source files, 22 test files and the
  manifest: `prost-reflect` for the pool and every lookup; `prost-types`
  and `prost` to build synthetic descriptors and the descriptor export.
  No `DynamicMessage`, no text format, no global pool, no `json_name`.
- **The pool** (`decode.rs`) is lazy beside an `index.rkyv`, eager
  otherwise, empty without a schema. protolens adds files to it at run
  time through one function, `register_synthetic`, for four kinds of
  synthetic file: the one-field wrappers behind every override (named by a
  hash of field number, type, type name, packing and label; specs
  0135/0137), the schema-free `message` target, user-declared types, and
  the MessageSet `Item`. Each caller first checks the pool for the name,
  so nothing depends on prost-reflect silently skipping a duplicate file —
  which upb refuses.
- **No descriptor crosses a thread.** Workers get the scoring graph, text
  and blob only; the extension loader is a thread-local closure.
- **Workarounds that become obsolete.** prost-reflect's pool forks on
  every add (`Arc::make_mut`), so protolens never holds a descriptor
  across a load and drops a target before registering a wrapper to avoid
  a deep clone of the pool (`decode.rs` 80–84, 191–193, 1676–1687,
  1750–1753; `override_resolve.rs` 361). upb handles see later loads and
  an add never moves a definition.
- **Gaps in today's wrapper** that protolens needs:
  - `Enum::parent_file()` (override export, wrapper dependencies);
  - a definition's declaration path (`[4, i, 3, j…]`) and a
    `SourceCodeInfo` decoder, for jump to definition (`tui/neovim.rs`):
    upb does not keep `source_code_info`, but the pool keeps each file's
    original bytes (0415 S3);
  - `Type::as_str_name()` (`TYPE_MESSAGE`…), which names wrappers
    (`decode.rs` 1407), and typed `r#type()` / `label()` getters (a test).

## Goals

- **G1.** protolens loads editions files (2023, 2024), as prototext does.
- **G2.** protolens is back in the workspace, in `ci`, and in every Nix
  output that had it, with no prost-reflect, prost or prost-types in its
  dependency tree; prost then remains only in protoscan (0415 N1b), and
  the follow-up for protoscan is a small spec of its own.
- **G3.** For proto2 and proto3 schemas, protolens's batch output is
  byte-identical to the last prost-reflect build (S9), and its test suite
  passes. Every difference is explained and either fixed or listed here.
- **G4.** Performance is measured before and after and every regression
  is explained (S10), as 0415 S12 did.
- **G5.** A schema file that cannot load is reported, never silently
  missing: protolens warns as prototext does (0415 S8).

## Non-goals

- **N1.** Changing protolens's features, key bindings or rendering.
- **N2.** Changing how overrides are named, stored or exported: a saved
  override file and an exported descriptor stay byte-identical (S9).
- **N3.** protoscan (0415 N1b): it uses `prost-types` only, to read
  descriptor structures, and moves in its own small spec.
- **N4.** Spec 0416's cache: protolens benefits from it through
  `prototext-core` when it lands, and needs nothing of its own.

## Specification

### S1 — Additions to `prototext-upb`

- `Enum::parent_file()` and `Enum::parent_message()`
  (`upb_EnumDef_File`, `upb_EnumDef_ContainingType`), added to the FFI
  surface and its link test.
- `Message::declaration_path()` and `Enum::declaration_path()`: the
  `SourceCodeInfo.Location.path` of the definition — `[4, i]` for a
  top-level message, `[4, i, 3, j, …]` nested, `[5, i]` or `[…, 4, k]` for
  an enum — computed by walking the parent chain and finding each
  definition's index among its siblings (handles compare by pointer).
- In `descriptor`: `SourceCodeInfo` and `Location` (`path`, `span`, both
  packed or not on the wire), read from `FileDescriptorProto` field 9;
  `Type::as_str_name()` returning exactly prost's `TYPE_*` strings; and
  `r#type()` / `label()` getters on `FieldDescriptorProto` returning the
  typed enums, as prost's do.

### S2 — Back in the workspace

- `protolens` returns to the workspace's members and leaves `exclude`;
  the `Cargo.lock` regains its dependencies; `workspace-hack` is
  regenerated with cargo-hakari.
- Every N1 edit of spec 0415 is reverted: the workspace build copies the
  protolens binary again (`nix/rust.nix`), the `prototools` bundle, the
  completion test and both `ci` lists include it, `workspaceSrc` no
  longer subtracts it, and the dev shell's hook builds it again (the
  membership check stays, as a guard).
- `protolens/Cargo.toml` drops `prost-reflect`, `prost` and
  `prost-types`.

### S3 — The pool

- `DescriptorContext` holds a `schema::Pool`. Eager loading goes through
  `decode_pool`, lazy loading through `LazyPool` (already on upb), the
  schema-less case uses `new_pool()`: every pool is gated (0415 S4).
- `pool_mut()` goes: a upb pool takes files through `&self`. Callers that
  registered through it (`register_wrapper`, `register_message_set_item`,
  `splice_override`, the warming pass) take `&Pool`.
- The `LoadReport` of an eager load is surfaced: on the terminal before
  the TUI starts, and through the same channel as the eager-fallback
  notice (spec 0197) once it has, one line per skipped file. Completion
  (`complete.rs`) ignores skipped files, as it ignores everything but
  names.
- On the lazy path, a type the reader names with `--type` whose file
  cannot load is reported with the load error, as `prototext` reports it
  (0415 S8), rather than as a type the set does not declare. A type
  loaded on demand elsewhere (an override target, an `Any`, an
  extension) stays a miss, as in `prototext`.
- `all_type_fqdns` (the override pane's list) keeps reading the lazy
  index; spec 0197's claim that it equals the eager pool's namespace now
  holds only when nothing was skipped, and the code comment says so.
- The comments and code that guard against prost-reflect's forking pool
  go (Background), keeping the existence checks before each registration,
  which upb now needs for correctness, not just speed.

### S4 — Synthetic descriptors

- `register_synthetic` builds its `FileDescriptorProto` with
  `schema::descriptor` and adds it as bytes (`Pool::add_file`); its four
  callers are unchanged in what they build.
- A wrapper's name is unchanged: its hash input keeps prost's `TYPE_*`
  spelling, now from `Type::as_str_name()`. A test pins the name of a few
  wrappers to their pre-migration values, so a saved override file keeps
  resolving (N2).

### S5 — Fields, extensions, MessageSet

- `ParentFieldOrExt` (`tui/override_resolve.rs`) holds one
  `schema::Field` for a field or an extension, as `prototext-core`'s
  `FieldOrExt` does (0415 S6).
- The MessageSet check reads `Message::is_message_set_wire_format()`; it
  was protolens's only access to raw options.

### S6 — Export

`export_descriptor.rs` and `tui/override_export.rs` build their
`FileDescriptorSet` with `schema::descriptor`: every field they write
(`name`, `number`, `label`, `type`, `type_name`, `dependency`,
`message_type`, `syntax`) exists there. `Enum::parent_file()` (S1)
replaces the prost-reflect call. The exported bytes are unchanged (S9).

### S7 — Jump to definition

`tui/neovim.rs` finds a definition's line and column in its file's
`source_code_info`, decoded from `Pool::file_bytes(file)` (S1), at the
definition's `declaration_path()`; the fallback to line 1, column 1 is
kept. The eager and lazy pools both keep a file's original bytes, so the
`source_code_info` that `reproto --schema-db-out` writes (spec 0141) is
there.

### S8 — Tests

The test files move from `prost_types` / `prost::Message` to
`schema::descriptor` / `Encode`, as prototext's did (0415 S8), with the
same two fixes: `..Default::default()` dropped where the new structs are
fully named, and a duplicate `any.proto` added only to a pool that lacks
it.

### S9 — The comparison gate

As 0415 S11: before any code changes, the last prost-reflect build of
protolens (built from the branch's base on `main`, in a separate
worktree) records golden outputs, and the upb build must reproduce them
byte for byte. `scripts/upb-golden-protolens` holds the protolens cases:

- `export /` (prototext) of each googleapis instance, with `--type` and
  inferred, on the lazy pool; with `--type` on the eager pool, for every
  tenth instance;
- `export /` in binary, as the round trip;
- `export /` of googleapis.desc itself as a `FileDescriptorSet`, on both
  pools, and of bobapp.desc;
- `script` on the anomaly fixture and its script (`tests/fixtures/`),
  and the export it starts from;
- overrides on the anomaly fixture (a scalar, a message, a varint
  scalar, an enum and a packed run), exported in all four formats: the
  anomaly script applies none, so the wrappers are exercised here;
- `quit` on googleapis.desc (lazy, `--raw`, eager) and bobapp.desc, for
  the startup path and its messages.

protolens's own suite (1,300+ tests, overrides and exports included) is
the second half of the gate. Expected differences, to confirm, not
discover: a group-typed extension now renders as a group (0415 S6), and
a file upb refuses where prost-reflect loaded it is now skipped and
reported (S3).

### S10 — Benchmarks

Measured as 0415 S12, pinned, A/A before A/B (spec 0416 S3):

- `protolens <googleapis.desc> quit` with and without `--raw` (startup,
  spec 0198), and its peak RSS;
- `export /` of googleapis.desc;
- `script` on the anomaly fixture (override and splice path).

Results and explanations go under "Measured outcome", for review; no
threshold decides.

## Alternatives considered

### Keep protolens on prost-reflect behind an adapter

A schema trait in `prototext-core` with a prost-reflect implementation
for protolens would let the branch merge sooner. It is the dual backend
spec 0415 rejected (N6), and the adapter would be thrown away here.

### Reproduce `source_code_info` lookups through upb

upb's definitions do not keep it, and its single-file build has no
def-to-proto conversion; the original bytes are already in the pool.

### Let `add_file` skip a duplicate name, as prost-reflect did

It would make the four registration paths tolerant of a race that cannot
happen (each checks first), and would hide a real error elsewhere: a
second, different file under a name already loaded.

## Test plan

1. S1's additions: `Enum::parent_file`, both `declaration_path`s on
   nested messages and enums, `SourceCodeInfo` in both encodings,
   `Type::as_str_name` for all eighteen types, against the editions
   fixture and protoc's own output for the declaration paths.
2. A wrapper registered for each `Type` gets the same name as under
   prost-reflect (pinned values).
3. Jump to definition on a reproto-generated database finds the line
   and column it found before.
4. An eager load with a file upb refuses starts, shows the skipped file,
   and every other type is usable.
5. The gate (S9): 0 unexplained differences; protolens's suite passes.
6. Benchmarks (S10) recorded.
7. `ci`, `nixpkgs-staging` (the protolens recipe builds
   `-p prototext-lens` from the local tree), `cargo package` for every
   publishable crate, `reuse lint`, clippy and rustfmt pass.

## Measured outcome

Implemented 2026-10-13, on the `upb` branch.

- **S1:** the additions, plus `Field::parent_file` and
  `Field::is_group_like` (below). Declaration paths are tested against
  protoc's own locations: `tests/fixtures/nested.proto`, compiled with
  `--include_source_info`, places three levels of messages and enums,
  and each definition's path finds protoc's location at the line that
  declares it.
- **S2:** protolens is back in the workspace, `ci`, the bundle, the
  completion test and the dev shell's hook. `Cargo.lock` holds no `prost`
  crate; `workspace-hack` was regenerated (hakari verify passes).
- **S3:** skipped files go to stderr in batch mode too (unlike the
  eager-fallback note, they are something going wrong), and to the
  splash and the status line. A lazy `--type` whose file fails to load
  reports the load error (added to S3).
- **S4:** wrapper names unchanged: a test pins all eighteen types plus
  the packed and required keys to values computed independently from
  prost's spelling.
- **S7:** a test reads line and column from a file's own
  `source_code_info` for a top-level message, a nested one and a nested
  enum.
- **G1:** the prost-reflect protolens panics loading the editions
  fixture (`index out of bounds` in prost-reflect's feature
  resolution); this build loads it, and a batch test pins an editions
  2023 schema's packed default.
- **S9, the gate:** 1,180 of 1,180 cases byte-identical between the
  prost-reflect protolens (built from `main`, ee55add, in a separate
  worktree) and this build; the baseline is deterministic (an A/A run
  was identical). Neither expected difference occurred: no case has a
  group extension or a file upb refuses, and each is covered by a test
  instead. prototext's own gate, re-run after the naming fix below:
  1,516 of 1,516.
- **Suite:** 1,351 protolens tests before the new ones, 1,356 with them;
  the workspace, 1,784, all passing under `env -i`. clippy and rustfmt
  clean. `nix-build -A ci` passes, protolens, the completion test and
  `nixpkgs-staging-check` included; the ten publishable crates package
  and build from their `.crate` files.
- **A prototext-core fix found here.** protoc names a group-framed field
  by its type only when it is group-like (`IsGroupLike`); an editions
  DELIMITED field that is not, such as `delimited_from_file`, goes by its
  own name. Both prototext and protolens named every group by its type.
  The renderer now follows protoc's rule, and protolens's splice path
  applies the same rule when it patches an override's group header
  (previously it never patched one). Recorded in spec 0415 too.

### S10, benchmarks

Pinned to one core, hyperfine, 10+ runs; A twice (prost-reflect, for the
noise), then B (upb). Milliseconds, mean ± σ; peak RSS in MB.

| Case | A | A again | B | RSS A → B |
|---|---|---|---|---|
| `quit`, lazy, inferred | 54.2 ± 2.2 | 53.0 ± 2.7 | 48.3 ± 2.2 | 33 → 32 |
| `quit`, lazy, `--raw` | 24.8 ± 1.7 | 24.2 ± 1.3 | 23.1 ± 0.8 | 22 → 23 |
| `quit`, eager | 2,962 ± 150 | 3,013 ± 117 | 723 ± 7 | 1,000 → 253 |
| `export /` of googleapis.desc | 6,019 ± 158 | 5,977 ± 189 | 5,777 ± 137 | 1,677 → 1,676 |
| `script`, anomaly fixture | 4.8 ± 0.5 | 5.2 ± 0.6 | 4.1 ± 0.6 | 14 → 14 |

No regression. The eager load is 4.1 times faster and holds a quarter
of the memory, as prototext's did (0415 S12). The others are within or
just past the A/A spread, in upb's favor.
