<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# prototools

A collection of protobuf utilities for working with binary descriptors and
wire-format messages.

→ **[Tutorial: from first decode to schema inference](docs/tutorial.md)**

## Tools

### `prototext`

<img src="docs/logos/prototext.png" alt="prototext logo" width="120" align="right"/>

Lossless, bidirectional converter between binary protobuf wire format and
human-readable text.  Decodes with or without a schema; encodes annotated
prototext back to byte-exact binary.  Supports schema inference: given a
descriptor DB it ranks all candidate types and picks the best match.

→ See [`prototext/README.md`](prototext/README.md)

### `protoback`

Reconstructs compilable `.proto` source files from binary or text-format
`FileDescriptorSet` / `FileDescriptorProto` blobs, and builds the schema
databases prototext and protolens decode with.  Supports proto2, proto3,
and editions.  Output recompiles to a descriptor equivalent to the input.
It succeeds reproto, its Python predecessor, retired after 0.4.0.

`protoback instantiate` generates pseudo-random binary protobuf instances
from a `.desc` FileDescriptorSet: test corpora, and a quick sanity-check
that a descriptor is well-formed.

```
protoback --schema-db-out my-db.desc my.desc
protoback instantiate --descriptor-set my-db.desc --seed 42 -O out/ \
    google.type.PostalAddress google.protobuf.Timestamp
```

### `protoscan`

Scans binary files (shared libraries, executables, firmware images) for
embedded `FileDescriptorProto` blobs and extracts them as individual `.pb`
files, one per proto file descriptor found.  The extracted blobs can be
fed directly to `protoback` to recover `.proto` source.

```
protoscan --proto-out extracted/ some_binary
protoback --builtin descriptor -O src/ -I extracted/ extracted/*.pb
```

### `git seriate` and `git seriate-fdp`

Stores undated copies of the same files as snapshots, and rebuilds one
linear git history from them, ordered by how the files evolve rather than
by their dates. `git seriate-fdp` does the same for the protobuf
descriptors embedded in executables, and says what such a history can't
hold in one schema database, as a filter file for `protoback`.

```
git seriate add found/rack-a        # one snapshot per copy
git seriate settle                  # the history, in sorted order
```

→ See the [tutorial](docs/git-seriate/tutorial.md)

## Installation

### Rust CLIs (`prototext`, `protoscan`, `protolens`, `git seriate`)

```shell
cargo install prototext
cargo install protoscan
cargo install prototext-lens    # installs the protolens program
cargo install git-seriate       # git seriate
cargo install git-seriate-fdp   # git seriate-fdp
```

`protolens` is published as `prototext-lens`: the `protolens` crate on
crates.io is an unrelated project. Its editor key opens the declaration
of a type in your `VISUAL` or `EDITOR`, as git does; a Neovim it starts is
reused across presses.

`protoback` is not on crates.io: build it from this repository
(`cargo install --path protoback`, or `nix-build -A protoback`).

### Python libraries

```shell
pip install prototext-codec     # Python extension (lossless codec)
pip install prototext-graph     # Python extension (scoring graph)
pip install fdp-scan            # Python extension (descriptor scanner)
```

### NixOS / nix-shell

```shell
git clone https://github.com/ThalesGroup/prototools
cd prototools
nix-shell          # user shell — all tools on PATH
```

## License

MIT — see [`LICENSES/MIT.txt`](LICENSES/MIT.txt).
