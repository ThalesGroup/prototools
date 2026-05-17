<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# `reproto`

Reconstructs `.proto` source files from compiled protobuf descriptor sets
(`.pb` files produced by `protoc --descriptor_set_out`).

## What it does

`reproto` takes one or more binary or text-format descriptor sets and
regenerates the original `.proto` files with correct syntax, field types, and
options.  It supports proto2, proto3, and editions syntax.

Key features:

- **Accurate reconstruction** — output recompiles to a descriptor set
  equivalent to the input.
- **Binary and text-format input** — accepts both `.pb` (binary
  `FileDescriptorSet`) and `.textpb` (text-format) descriptor files.
- **Multi-FDP input** — a single `.pb` compiled with `--include_imports` is
  split automatically; individual per-FDP `.pb` files are also accepted.
- **Dependency-aware** — resolves all transitively imported files needed to
  compile the output.
- **Selective output** — seed and prune flags let you emit only the subset
  of messages and files you care about, with full dependency closure.
- **Schema DB builder** — `--build-schema-db` compiles a `.desc` descriptor
  together with a Hopcroft scoring graph and lazy-loading index, ready for
  use with `prototext list-schemas`.
- **Variant support** — pluggable descriptor variants for different protobuf
  runtimes and well-known-type bundles.

## Installation

### NixOS / nix-shell

reproto depends on `prototext_codec`, a compiled Rust extension that is
built as part of the prototools Nix derivation.

**User shell** — `reproto` and `reproto-instantiate-schema` on `PATH`, man
page and completions activated:

```shell
git clone https://github.com/douzebis/prototools
cd prototools
nix-shell          # enters shell.nix → user-shell
```

**Development shell** — adds pyright, pytest, and source-tree wiring for
working on reproto itself:

```shell
nix-shell dev-shell.nix
```

## Quick start

### Decompile a descriptor set

Compile a `.proto` to a descriptor, then reconstruct it:

```shell
protoc --descriptor_set_out=my.pb --include_imports my.proto
reproto --use-variant descriptor -O out/ my.pb
cat out/my.proto
```

`--use-variant descriptor` supplies `descriptor.proto` from the built-in
variant bundle, so no separate descriptor `.pb` is needed.

For selective output — emit only `Person` and its dependencies:

```shell
reproto --use-variant descriptor -O out/ --seed desc:.tutorial.Person my.pb
```

### Build a schema DB for `prototext list-schemas`

```shell
reproto --build-schema-db=my.desc my.pb
prototext --descriptor my.desc list-schemas unknown.pb
```

`--build-schema-db` writes `my.desc` (the merged FileDescriptorSet),
`my/hopcroft.rkyv` (compiled scoring graph), and `my/index.rkyv`
(lazy-loading index).  No `-O` is required when only the DB is needed.

## `reproto-instantiate-schema`

Generates pseudo-random binary protobuf instances from a `.desc`
FileDescriptorSet.  Useful for populating test corpora and sanity-checking
that a schema is well-formed.

```
reproto-instantiate-schema --descriptor my.desc --seed 42 -O out/ \
    google.type.PostalAddress google.protobuf.Timestamp
```

Options:

| Option | Description |
|---|---|
| `--descriptor FILE` | `.desc` FileDescriptorSet to load |
| `-O DIR` | Root directory for output `.pb` files |
| `--seed INT` | PRNG seed (default: 0) |
| `--max-depth INT` | Maximum recursion depth for nested messages (default: 4) |
| `--max-repeated INT` | Maximum elements for repeated fields (default: 3) |
| `-q` / `--quiet` | Suppress per-file progress messages |

Non-Nix users: `reproto-instantiate-schema` is the nix-shell alias for
`python -m reproto.instantiate_cli`.

## CLI reference

```
reproto [OPTIONS] PB_FILES...
```

| Option | Description |
|---|---|
| `-I PATH` | Search path for loading imported `.pb` files |
| `-O DIR`, `--output-root DIR` | Output directory for reconstructed `.proto` files |
| `--use-variant NAME` | Activate a descriptor variant (e.g. `descriptor`, `all`) |
| `--seed FQDN` | Restrict output to nodes reachable from FQDN |
| `--prune FQDN` | Exclude FQDN and its children from output |
| `--build-schema-db FILE` | Build `.desc` + scoring graph + index (no `-O` needed) |
| `--emit-scoring-graphs` | Write per-file scoring-graph YAML alongside `.proto` output |
| `--emit-descriptor` | Include `descriptor.proto` in output |
| `--dry-run` | Run all phases but skip writing files |
| `--debug` | Verbose per-phase logging |
| `--version` | Print version and exit |

## Running the tests

From the repository root inside the development shell:

```shell
nix-shell dev-shell.nix
python -m pytest reproto/src/reproto/tests/ -x -q
```
