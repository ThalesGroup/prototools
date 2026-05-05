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

The examples below use the two fixture files shipped with reproto:
`phone_number.proto` and `address_book.proto` (under
`reproto/src/reproto/tests/fixtures/`).

> **Note:** reproto does not support multi-file `FileDescriptorSet` inputs
> (i.e. `.pb` files produced with `protoc --include_imports`). Each `.pb`
> file must contain exactly one `FileDescriptorProto`. Pass all `.pb` files
> together on the command line and let reproto resolve cross-file imports.

**Step 1 — compile each `.proto` to its own descriptor set:**

Compile without `--include_imports` so each `.pb` contains a single file:

```shell
cd reproto/src/reproto/tests/fixtures
protoc --descriptor_set_out=phone_number.pb --proto_path=. phone_number.proto
protoc --descriptor_set_out=address_book.pb --proto_path=. address_book.proto
```

**Step 2 — reconstruct the `.proto` files:**

```shell
reproto --use-variant descriptor -I . -O out/ phone_number.pb address_book.pb
```

The reconstructed `.proto` files appear under `out/`:

```
out/
  phone_number.proto
  address_book.proto
```

**Selective output** — emit only `Person` and its dependencies:

```shell
reproto --use-variant descriptor -I . -O out/ --seed desc:.tutorial.Person \
  phone_number.pb address_book.pb
```

**Using the embedded descriptor variant** — `--use-variant descriptor`
supplies `descriptor.proto` from the built-in variant bundle, so no
separate descriptor `.pb` is needed (as shown above).

## CLI reference

```
python -m reproto.cli [OPTIONS] PB_FILES...
```

| Option | Description |
|---|---|
| `-I PATH` | Search path for loading imported `.pb` files |
| `-O DIR`, `--output-root DIR` | Output directory for reconstructed `.proto` files (created if absent) |
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
