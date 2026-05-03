<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# `reproto`

Reconstructs `.proto` source files from compiled protobuf descriptor sets
(`.pb` files produced by `protoc --descriptor_set_out`).

## What it does

`reproto` takes one or more binary descriptor sets and regenerates the
original `.proto` files with correct syntax, field types, options, and
comments where source-code info is available.  It supports proto2, proto3,
and editions syntax.

Key features:

- **Selective output** — seed and prune flags let you emit only the subset
  of messages and files you care about.
- **Dependency-aware** — automatically pulls in all transitively imported
  files needed to compile the output.
- **Variant support** — pluggable descriptor variants for different protobuf
  runtimes and well-known-type bundles.

## Installation

### NixOS / nix-shell

```shell
git clone https://github.com/douzebis/prototools
cd prototools
nix-shell
pip install -e reproto/
```

### pip

```shell
pip install reproto
```

## Quick start

Compile a `.proto` file to a descriptor set, then regenerate it:

```shell
protoc --descriptor_set_out=foo.pb --include_imports foo.proto
python -m reproto.cli --use-variant descriptor -I. --proto-out=out/ foo.pb
```

The reconstructed `.proto` files appear under `out/`.

## CLI reference

```
python -m reproto.cli [OPTIONS] PB_FILES...
```

| Option | Description |
|---|---|
| `-I PATH` | Search path for loading imported `.pb` files |
| `--proto-out DIR` | Output directory for reconstructed `.proto` files |
| `--use-variant NAME` | Activate a descriptor variant (e.g. `descriptor`) |
| `--seed FQDN` | Restrict output to nodes reachable from FQDN |
| `--prune FQDN` | Exclude FQDN and its children from output |
| `--emit-descriptor` | Include `descriptor.proto` in output |
| `--dry-run` | Run all phases but skip writing files |
| `--debug` | Verbose per-phase logging |
| `--version` | Print version and exit |

## Running the tests

From the repository root inside a nix-shell:

```shell
python -m pytest reproto/src/reproto/tests/ -x -q
```
