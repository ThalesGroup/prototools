<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0061 — prototext subcommand CLI refactor

**Status:** draft
**App:** prototext

---

## Background

The current `prototext` CLI uses flags to select the operating mode:

```
prototext -d [OPTIONS] [PATH...]
prototext -e [OPTIONS] [PATH...]
prototext --list-schemas [OPTIONS] [PATH...]
prototext instantiate-schema [OPTIONS] TYPE...
```

`-d` and `-e` are flat top-level flags, while `instantiate-schema` is a
subcommand.  `--list-schemas` is a flag that can be combined with `-d`.
This mixed flat/subcommand design has grown unwieldy as the command set
expands.

Two separate options handle the descriptor/schema source today:
`--descriptor PATH` for a standalone `.pb` descriptor, and `--db PATH` for
a full schema DB (a `.rkyv` scoring graph plus a sibling `schemas.pb`
descriptor set).  These are unified in this spec under a single `--descriptor`
option — see §2.

This spec defines a clean subcommand-based CLI.

---

## Goals

1. Restructure the CLI as `prototext [GLOBAL-OPTIONS] COMMAND [COMMAND-OPTIONS] [PATH]...`
   with five subcommands: `decode`, `encode`, `list-schemas`,
   `instantiate-schema`, `score`.
2. Move shared options (`--descriptor`, `-I`, `-o`, `-O`, `-q`) to global position,
   before the verb.
3. Replace the current `--descriptor` (standalone FDP) and `--db` with a single unified
   `--descriptor DESCRIPTOR_FILE` option (§2).  The descriptor is resolved
   by decreasing priority: (1) `--descriptor` value, (2) `PROTOTEXT_DEFAULT_DESCRIPTOR`
   env var, (3) built-in fallback (google.protobuf.descriptor).  If a sibling
   Hopcroft rkyv file is also present, it is loaded automatically and enables
   scoring and auto-inference.
4. Add a `score` subcommand (§7).
5. The old flags (`-d`, `-e`, `--list-schemas`) may be kept as hidden aliases
   during a transition period, but are not part of this spec's scope.

## Non-goals

- Changes to the encode/decode/instantiate/scoring logic.
- Changes to the shell completion model.
- Removing the hidden legacy flags (a future cleanup spec).
- Adding new scoring output formats beyond what is specified here.

---

## Specification

### §1 — Help synopsis

```
prototext [OPTIONS] <COMMAND>

Commands:
  decode              Decode binary protobuf to prototext
  encode              Encode prototext to binary protobuf
  list-schemas        Score input(s) against the DB and list candidate types
  instantiate-schema  Generate a pseudo-random valid protobuf instance for a type
  score               Score input(s) against a known schema type

Options:
      --descriptor <DESCRIPTOR_FILE>  FileDescriptorSet for type lookup and scoring
                             (see §2 for format and DB-backed mode)
                             [env: PROTOTEXT_DEFAULT_DESCRIPTOR=]
  -I, --input-root <DIR>     Resolve positional paths and globs relative to DIR.
                             Absolute positional paths are an error when set
  -o, --output <PATH>        Write output to PATH (single input only;
                             exclusive with --output-root)
  -O, --output-root <DIR>    Write output files under DIR, mirroring the input
                             tree (exclusive with --output and --in-place)
  -q, --quiet                Suppress warnings on stderr (errors still print)
  -h, --help                 Print help
  -V, --version              Print version
```

### §2 — The `--descriptor` option and descriptor resolution

`--descriptor DESCRIPTOR_FILE` points to a descriptor file.  It subsumes and
replaces both the current `--descriptor` (standalone `.pb`) and `--db` options.

**Format**: the file may be a raw binary `FileDescriptorSet`, a `#@`
prototext-format `FileDescriptorSet`, or a single `FileDescriptorProto`
(treated as a single-FDP descriptor set).  It is loaded transparently
regardless of format.  Any file extension is accepted; `.desc` is conventional
but not required.

**Resolution order** (first match wins):

1. `--descriptor` command-line value.
2. `PROTOTEXT_DEFAULT_DESCRIPTOR` environment variable.
3. Built-in fallback: the embedded `google.protobuf.descriptor` covering all
   `google.protobuf.*` types.

**Hopcroft graph auto-detection**: given a descriptor path `<stem>.desc` (or
any path `<stem>.<ext>`), prototext checks for the sibling file
`<stem>/hopcroft.rkyv`.  If it exists, it is loaded as the compiled Hopcroft
scoring graph.

Two resulting modes:

- **Isolated descriptor** — no `<stem>/hopcroft.rkyv` present.  Provides type
  lookup via `--type`.  `score` works with an isolated descriptor when
  `--type` is given (scoring walk uses the wire bytes and type structure; no
  Hopcroft graph is needed).  Auto-inference and `list-schemas` are not
  available.

- **DB-backed descriptor** — `<stem>/hopcroft.rkyv` is present.  Provides type
  lookup, Hopcroft-based scoring, auto-inference in `decode`, and the
  `list-schemas` subcommand.

**On-disk layout** produced by `reproto --build-schema-db PATH.desc`:

```
PATH.desc                  ← FileDescriptorSet of all loaded FDPs
PATH/hopcroft.rkyv         ← compiled Hopcroft scoring graph
```

This replaces the previous `stress.rkyv` + `<stem>/schemas.pb` layout.

---

### §3 — `decode` subcommand

```
prototext [OPTIONS] decode [COMMAND-OPTIONS] [PATH]...

Decode binary protobuf to lossless prototext.

Arguments:
  [PATH]...  Input files, glob patterns, or directories (recursive).
             Reads from stdin when omitted

Options:
  -t, --type <NAME>     Decode as this fully-qualified message type.
                        Looks up the descriptor in --descriptor /
                        PROTOTEXT_DEFAULT_DESCRIPTOR, falling back to the
                        embedded WKT descriptor
  -i, --in-place        Rewrite each input file in place (exclusive with
                        --output-root). Files are read fully before writing
      --assume-binary   Treat PATH arguments as raw binary protobuf; skip
                        #@ prototext auto-detection on the input files
      --annotations     Emit inline wire-type/field-number comments (default).
                        Required for lossless round-trip encode
      --no-annotations  Suppress inline wire-type/field-number comments.
                        Output will not round-trip losslessly
  -h, --help            Print help
```

Behavior:

- No `--type`, single input, DB-backed descriptor present: auto-infer type
  from the scoring graph.  Unique top scorer → decode with that type, print
  inferred FQDN to stderr.  Tied → print tied FQDNs to stderr as a YAML list,
  no decode.
- No `--type`, multiple inputs: error; `--type` is required for batch decode.
- No `--type`, no DB-backed descriptor: schemaless decode (fields rendered by
  wire type and field number only).
- With `--type`: look up the descriptor in the loaded descriptor set (or
  embedded WKT) and decode.

---

### §4 — `encode` subcommand

```
prototext [OPTIONS] encode [COMMAND-OPTIONS] [PATH]...

Encode prototext to binary protobuf.

Arguments:
  [PATH]...  Input files, glob patterns, or directories (recursive).
             Reads from stdin when omitted

Options:
  -i, --in-place  Rewrite each input file in place (exclusive with
                  --output-root). Files are read fully before writing
  -h, --help      Print help
```

---

### §5 — `list-schemas` subcommand

```
prototext [OPTIONS] list-schemas [COMMAND-OPTIONS] [PATH]...

Score input(s) against the DB and list candidate types, score-descending.
Ties within a score level are ordered lexicographically by FQDN.
Requires a DB-backed descriptor (hopcroft.rkyv must be present).

Arguments:
  [PATH]...  Input files, glob patterns, or directories (recursive).
             Reads from stdin when omitted

Options:
      --top <N>  Print only the top N entries (score-descending, ties broken
                 by FQDN). When absent or 0, only entries tying at the
                 highest score are printed
  -h, --help     Print help
```

Output is YAML to stdout:

```yaml
- path: foo.pb
  types:
    - com.example.Foo
    - com.example.Bar
- path: bar.pb
  types:
    - com.example.Baz
```

---

### §6 — `instantiate-schema` subcommand

```
prototext [OPTIONS] instantiate-schema [COMMAND-OPTIONS] <TYPE>...

Generate a pseudo-random valid protobuf instance for one or more message types.
Output is #@ prototext with type and seed hint comments.
Multiple TYPEs require --output-root (-O).

Arguments:
  <TYPE>...  Fully-qualified message type names (e.g. google.protobuf.Timestamp)

Options:
      --seed <N>            Integer seed (default 0). Effective PRNG seed is
                            SHA256(N + ":" + FQDN) → StdRng
      --max-depth <N>       Maximum recursion depth for nested messages
                            (default 4)
      --max-repeated <N>    Maximum number of elements for repeated fields
                            (default 3)
      --p-optional <FLOAT>  Probability of populating an optional field
                            (default 0.7)
  -h, --help                Print help
```

Descriptor is sourced from `--descriptor` / `PROTOTEXT_DEFAULT_DESCRIPTOR`,
falling back to the embedded WKT descriptor.

Output format: `#@` prototext with a `# type:` hint comment after the magic
line.  The current implementation emits `# ground_truth:` instead of
`# type:`; this spec renames it to `# type:` for consistency with the new
`--type` option name.  The `# seed:` line is retained unchanged.

---

### §7 — `score` subcommand

The `score` subcommand computes the same Hopcroft-based score as
`list-schemas`, but for a single known schema type rather than ranking all
types.  Given a `--type` and one or more protobuf inputs, it outputs the
match breakdown (matches, unknowns, mismatches, non_canonical) for each input.

For this initial implementation, `score` requires a DB-backed descriptor
(`hopcroft.rkyv` must be present).  Support for isolated-descriptor scoring
(without a Hopcroft graph) is deferred to a future spec.

```
prototext [OPTIONS] score [COMMAND-OPTIONS] [PATH]...

Score input(s) against a known schema type and report the match breakdown.

Arguments:
  [PATH]...  Input files, glob patterns, or directories (recursive).
             Reads from stdin when omitted

Options:
  -t, --type <NAME>    Required. Fully-qualified root message type name.
                       Looked up in --descriptor / PROTOTEXT_DEFAULT_DESCRIPTOR
                       or the embedded WKT descriptor
      --assume-binary  Treat PATH arguments as raw binary protobuf; skip
                       #@ prototext auto-detection on the input files
  -h, --help           Print help
```

Output is YAML to stdout, one entry per input:

```yaml
- path: foo.pb
  score: 14
  matches: 9
  unknowns: 2
  mismatches: 0
  non_canonical: 0
- path: bar.pb
  vetoed: true
```

`path` is `<stdin>` when reading from stdin.  A vetoed entry has only the
`path` and `vetoed: true` keys.

---

### §8 — Constraints

Enforced at startup, before any I/O:

- `--output` and `--output-root` are mutually exclusive.
- `--output-root` and `--in-place` are mutually exclusive.
- `--input-root` and `--output-root` resolving to the same directory is an
  error (use `--in-place` instead).
- Absolute positional paths with `--input-root` given is an error.
- Two input files resolving to the same output path is an error.
- `--in-place` with stdin input is an error.
- `--output-root` with stdin input is an error.
- `list-schemas`, `score`, and `decode` auto-inference require a DB-backed
  descriptor (`hopcroft.rkyv` sibling must exist); an isolated descriptor is
  an error for these modes.
- `score` requires `--type`.

`--in-place` is only valid for `decode` and `encode`.

---

### §9 — Implementation notes

**`DescriptorContext`** (`run.rs`): a new struct that encapsulates the result
of resolving `--descriptor`:

```rust
pub struct DescriptorContext {
    /// Decoded descriptor pool for type lookup.
    pub pool: prost_reflect::DescriptorPool,
    /// Compiled Hopcroft scoring graph, if <stem>/hopcroft.rkyv exists.
    pub graph: Option<LoadedGraph>,
}
```

`DescriptorContext::load(path: Option<&Path>) -> Result<DescriptorContext, String>`
reads the descriptor file (binary or `#@` prototext FDS/FDP), builds the
pool, then checks for `<stem>/hopcroft.rkyv` and loads it if present.
When `path` is `None`, uses only the embedded WKT descriptor and no graph.

**`Cli` struct** (`lib.rs`): global options (`descriptor`, `input_root`,
`output`, `output_root`, `quiet`) move to the top-level struct.  The current
`decode`/`encode` booleans, `list_schemas`/`top` flags, and the nested
`InstantiateSchema` subcommand are replaced by a `Command` enum with five
variants: `Decode`, `Encode`, `ListSchemas`, `InstantiateSchema`, `Score`.
The `db` field is removed.

**`run()`** (`run.rs`): dispatches on `cli.command`.  Builds a
`DescriptorContext` once at startup from `cli.descriptor`, then passes it
into each subcommand handler.

**`complete.rs`**: `complete_db_path` is removed; `complete_pb_files` is
repurposed (or renamed) to complete any file for `--descriptor`.

**Legacy aliases**: `-d`/`--decode` and `-e`/`--encode` may be kept as
hidden clap aliases on the `decode` and `encode` subcommands.  `--db` may
be kept as a hidden alias for `--descriptor`.  These are optional.

---

## Files changed

| File | Change |
|---|---|
| `prototext/src/lib.rs` | Restructure `Cli`: global `descriptor` replaces `db`; five-variant `Command` enum replaces flat flags |
| `prototext/src/run.rs` | Dispatch on new `Command` variants; introduce `DescriptorContext`; add `Score` handler; remove `schemas_pb_from_db` |
| `prototext/src/complete.rs` | Remove `complete_db_path`; update `--descriptor` completer |
