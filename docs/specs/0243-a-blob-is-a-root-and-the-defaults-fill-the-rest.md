<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0243 — a blob is a root, and the defaults fill the rest

Status: implemented
Implemented in: 2026-08-05
App: reproto
Refs: docs/specs/0148-reproto-multi-root-fdp-loading.md (`-I` roots, the
        `.` argument, the W7 dedup),
        docs/specs/0149-reproto-filter-config-file.md (root-relative path
        seeds and prunes),
        docs/specs/0155-reproto-schema-db-proto-subdir-and-protolens-proto-root-fallback.md
        (G1: `-O` may sit inside the schema-DB stub only as its `proto`
        child),
        docs/specs/0239-the-schema-says-where-a-descriptor-ends.md
        (`fdp_scan_lib` embeds the WKT graph — the eval-time recursion
        this spec must not reopen),
        docs/specs/0241-a-real-call-leaves-bytes-worth-opening.md
        (ringer, which produces the blobs this is aimed at)

## Background

Reading a blob that a real program left behind currently takes four
options and a positional argument that is always the same:

```
protoscan grpconf/ringer --proto_out grpconf/ringer/pbs/
reproto --schema-db-out /tmp/ringer.desc -O /tmp/ringer/proto/ \
        -I grpconf/ringer/pbs/ .
```

Three of the five things on that line carry no information:

- `-O /tmp/ringer/proto/` is the only directory spec 0155 G1 allows
  under `/tmp/ringer.desc`'s stub anyway, so naming it is ceremony.
- `-I grpconf/ringer/pbs/` names a directory whose only reason to exist
  is that `protoscan` had to write its findings somewhere before reproto
  could read them.
- `.` is what every invocation that uses `-I` passes.

The goal is that the same work is spelled:

```
reproto --schema-db-out /tmp/ringer.desc -I grpconf/ringer
```

## Goals

- **G1.** `--schema-db-out FILE.desc` implies `-O <FILE-stem>/proto/`.
- **G2.** `-I` accepts a blob file *in addition to* a directory, and
  reads it as if `protoscan` had already expanded it into a directory of
  one `.pb` per FileDescriptorProto. A directory root keeps behaving
  exactly as it does today; the point of the addition is to skip the
  extraction step, not to replace it.
- **G3.** No positional argument means `.`.
- **G4.** None of this changes what an invocation that spells all three
  out does today.

## Non-goals

- **N1.** No temporary directory, and no `protoscan` subprocess. The
  expansion in G2 is in-memory; nothing is written where the blob lives.
  A blob is often read from a read-only path, and a scan that leaves
  artifacts behind would make a second run's `-I` ambiguous.
- **N2.** `--dry-run` and `--scoring-html-out` do **not** gain an implied
  `-O`. Neither names a directory, so there is nowhere obvious to put
  `.proto` files; `--schema-db-out` does, and that is the whole basis for
  G1.
- **N3.** The blob root is not recursive: an FDP found inside a blob is
  not itself re-scanned for further blobs.

## Specification

### G1 — the schema DB names the proto directory

- **S1.** When `--schema-db-out FILE.desc` is given and `-O`/`--proto-out`
  is not, `proto_out` defaults to `FILE.with_suffix('') / 'proto'` —
  `/tmp/ringer.desc` → `/tmp/ringer/proto`. That is exactly the one
  location spec 0155 G1 permits inside the stub directory, so the
  existing containment check accepts it without a special case.

- **S2.** A new `--no-proto-out` flag suppresses S1: `--schema-db-out`
  alone then writes only the DB, as it does today. `--no-proto-out` with
  an explicit `-O` is a `UsageError` — the two say opposite things, and
  silently letting one win is how a build script ends up writing files
  nobody asked for.

- **S3.** `--no-proto-out` joins `_SECTIONS` under `Output`, next to
  `--proto-out`. It takes no value, so `completions.sh`'s `takes_value`
  table is unchanged.

- **S4.** S1 fires *before* the `output_only_mode` check that today
  reports a missing `-O`. With S1 in place `--schema-db-out` never
  reaches that error, and the error text's `--schema-db-out FILE` line —
  which offered it as a way to *avoid* writing `.proto` files — is
  replaced by `--no-proto-out`.

### G2 — a blob is a root

- **S5.** `-I`/`--desc-root` accepts a regular file (`file_okay=True`).
  A file-shaped root is a **virtual root**: its member files are the
  FileDescriptorProtos `fdp_scan_lib.scan` accepts inside it, each at the
  root-relative path `fdp.name` with a `.proto` suffix rewritten to
  `.pb`. That is byte-for-byte the tree `protoscan --proto_out` writes,
  which is what makes the shortcut a shortcut and not a second dialect.

- **S6.** Because a virtual member has a real root-relative path,
  everything keyed on that path keeps working unchanged: the `.`
  directory argument, a named `-I`-relative argument, `-s`/`-p` path
  patterns and globs, and the W7 duplicate-name report (spec 0148
  G2-G4). `_load_files` grows one branch — `root.is_file()` — and
  nothing downstream of it changes.

- **S7.** A blob is scanned **once** per run and the result memoized on
  the `Context`, not per positional argument: `scan` walks the whole file
  and several arguments may resolve against the same root.

- **S8.** The scan yields raw fragment bytes; they are handed to the
  existing `QualFile` → `parse_qfile` → `split_fdps` path rather than
  parsed in place. `split_fdps` on a single FDP's bytes returns that one
  FDP, so the extra hop costs one parse and keeps exactly one place that
  decides what a descriptor is.

- **S9.** A blob root containing no acceptable FDP is a hard error naming
  the file, not a W1 "not found" warning about the argument. `-I` on a
  file is an explicit claim that the file holds descriptors; a warning
  buried among W1s would let a run continue and produce an empty DB.

- **S10.** `import fdp_scan_lib` is **deferred into the S5 branch**, and
  `fdpScanLib` is added to the final `reproto` package's
  `propagatedBuildInputs` only — never to `reprotoPropagatedDeps` or
  `wktRkyvDeps`. `fdp_scan_lib` embeds the freshly built WKT scoring
  graph (spec 0239 S1), and `wktRkyv` is built by `reprotoBare`; a
  top-level import would put `fdpScanLib` in `reprotoBare`'s closure and
  close the eval-time recursion `wktRkyv → reprotoBare → fdpScanLib →
  wktRkyv`. This is the same shape as `prototextGraphLib`, which
  `--schema-db-out` imports lazily for the same reason.
  `nix-instantiate -A ci` is the check.

### G2 — completion

- **S11.** `-I` loses its directories-only completion. Click derives the
  completion type from the `click.Path`: `dir_okay and not file_okay`
  emits `dir`, which `completions.sh` maps to `compopt -o dirnames`;
  once `file_okay=True` it emits `file`, i.e. bash's ordinary file-and-
  directory completion. That is the correct behavior and needs no custom
  completer — a blob has no distinguishing extension (`grpconf/ringer`
  has none at all), so there is nothing to filter on.

- **S12.** `complete_pb_files` — the `DESCRIPTOR_FILES` completer, which
  offers paths relative to each `-I` root — scans a blob root and offers
  its member paths, the same strings S5 gives those members. Without
  this, `reproto -I blob <TAB>` offers nothing at all and reads as
  broken: `_complete_paths` skips any base directory that is not a
  directory. The cost is one `fdp_scan_lib` import and one scan per
  keypress; it is bounded by the blob, and S10's deferred import means
  no other code path pays it.

### G3 — no argument means `.`

- **S13.** `DESCRIPTOR_FILES` becomes `required=False`, and an empty
  tuple is replaced by `(Path('.'),)`. `-I` already defaults to `[.]`, so
  a bare `reproto -O out/` means `reproto -O out/ -I . .` — the reading
  every other tool with a `-I` gives it.

## Alternatives considered

**Shell out to `protoscan --proto_out <tmpdir>`.** Rejected on N1 and on
the dependency: reproto would have to find a `protoscan` on `PATH` at run
time, which is exactly the sort of thing that works in the dev-shell and
fails in the installed package.

**Make `-O` default from `--schema-db-out` unconditionally, with no
`--no-proto-out`.** Rejected: `reproto --schema-db-out` is used in this
repo's own build (`wktRkyv`) to produce a DB and nothing else. Making it
also decompile the whole input set would be a silent slowdown in a
derivation, with output nobody reads.

**Let `-I <blob>` expand to `<blob-stem>/pbs/` on disk, mirroring S1.**
Rejected: S1 writes into a directory the user named as an output; a blob
is an input, and writing beside an input is not something an input-only
flag should do.

## Test plan

1. `test_schema_db_out_implies_proto_out` — `--schema-db-out t/x.desc`
   with no `-O` writes `.proto` files under `t/x/proto/`.
2. `test_no_proto_out_suppresses_the_implied_proto_out` — the same
   command with `--no-proto-out` writes the DB and creates no
   `t/x/proto/`.
3. `test_no_proto_out_with_explicit_proto_out_is_a_usage_error`.
4. `test_desc_root_may_be_a_blob` — a blob built by concatenating two
   serialized FDPs with filler bytes between them, read with
   `-I blob .`, produces the same output as reading the two `.pb` files
   from a directory.
5. `test_blob_member_paths_are_the_fdp_names` — `-p` on one member's
   name prunes it, proving S6's path identity.
6. `test_blob_with_no_descriptors_is_an_error`.
7. `test_completion_offers_blob_members` — `complete_pb_files` with a
   blob `-I` root returns the member paths (S12).
8. `test_no_arguments_means_dot` — `reproto -O out/ -I dir/` equals
   `reproto -O out/ -I dir/ .`.
9. `nix-instantiate -A ci --quiet` — S10's recursion check.

## Measured outcome

The line this spec set out to shorten:

```
protoscan grpconf/ringer --proto_out grpconf/ringer-pbs/
reproto --schema-db-out /tmp/ringer.desc -O /tmp/ringer/proto/ \
        -I grpconf/ringer-pbs/ .
```

is now

```
reproto --schema-db-out /tmp/ringer.desc -I grpconf/ringer
```

— two commands and five arguments down to one and two, with no
extracted `.pb` tree left on disk.

Verified end to end against an 11-member blob built from
`prototext-core/fixtures/descriptor.pb`: the DB and `out/proto/**`,
`--no-proto-out` suppression, the `-O` contradiction error, `-p` pruning
by member path, a named member loading alone, the empty-blob error, and
completion both bare and prefixed. `nix-instantiate -A ci --quiet`
terminates (exit 0), so S10 keeps the eval-time recursion closed.

The flag is spelled `--no-proto-out`, not the `--no-proto` this spec was
drafted with: it suppresses `--proto-out`, and naming it after the option
it cancels is what makes that readable at a glance.
