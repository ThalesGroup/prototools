<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# `prototext`

Lossless, bidirectional converter between binary protobuf wire format and
human-readable text.  Three promises:

1. **Lossless round-trip** — `binary → text → binary` is byte-for-byte
   identical for any input: well-formed, non-canonical, or malformed.
2. **protoc-compatible** — for canonical protobuf messages the text output is
   identical to `protoc --decode`.
3. **Schema-aware** — supply a compiled `.pb` descriptor and a root message
   type to get field names, proto types, and enum values.  Works without a
   schema too: every field is then decoded by wire type and field number.

## Installation

### NixOS / nix-shell

Build and install from the repo:

```shell
git clone https://github.com/douzebis/prototools
cd prototools
nix-build            # result/bin/prototext
```

Man page and shell completions are installed automatically.

Or enter a development shell with `prototext` on `PATH`, completions and man
page activated for the current session:

```shell
nix-shell
```

### cargo install

```shell
cargo install --locked prototext
```

`--locked` is required: it uses the dependency versions that were tested at
release time.

> **Man pages:** `cargo install` does not install man pages.  To generate
> them locally, run:
>
> ```shell
> cargo install --locked --bin prototext-gen-man prototext
> prototext-gen-man ~/.local/share/man/man1
> ```

## Shell Completions

When installed via `nix-build`, completion scripts are installed automatically.
When using `nix-shell`, bash completions are activated for the current session.

For a `cargo install` setup, add one line to your shell's startup file:

```bash
# bash (~/.bashrc)
source <(PROTOTEXT_COMPLETE=bash prototext | sed \
  -e '/^\s*) )$/a\    compopt -o filenames 2>/dev/null' \
  -e 's|words\[COMP_CWORD\]="$2"|local _cur="${COMP_LINE:0:${COMP_POINT}}"; _cur="${_cur##* }"; words[COMP_CWORD]="${_cur}"|')
```

```zsh
# zsh (~/.zshrc)
source <(PROTOTEXT_COMPLETE=zsh prototext)
```

```fish
# fish (~/.config/fish/config.fish)
PROTOTEXT_COMPLETE=fish prototext | source
```

## Quick start

`google.protobuf.*` types are embedded — no descriptor file needed.  Decode
any `.pb` descriptor that `protoc` produces:

```
$ protoc -o timestamp.pb google/protobuf/timestamp.proto

$ prototext -d -t google.protobuf.FileDescriptorSet timestamp.pb
#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "google/protobuf/timestamp.proto"  #@ string = 1
 package: "google.protobuf"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "Timestamp"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "seconds"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT64  #@ Type(3) = 5
   json_name: "seconds"  #@ string = 10
  }
  ...
 }
}
```

Encode back to binary and verify the round-trip is byte-exact:

```
$ prototext -d -t google.protobuf.FileDescriptorSet timestamp.pb | \
    prototext -e | diff - timestamp.pb && echo "byte-exact"
byte-exact
```

**Non-canonical encoding** — protobuf varints can carry redundant continuation
bytes and still decode to the same value.  Standard tools discard these bytes;
`prototext` preserves them via inline annotations:

```
$ printf '\x08\xaa\x00' | prototext -d
#@ prototext: protoc
1: 42  #@ varint; val_ohb: 1
```

Field 1 = 42, but encoded in three bytes instead of the canonical two
(`val_ohb: 1` records the one redundant byte).  The round-trip is still
byte-exact:

```
$ printf '\x08\xaa\x00' | prototext -d | prototext -e | od -A n -t x1
 08 aa 00
```

**Canonicality verification** — annotation modifiers flag every
wire-level deviation from the canonical encoding: overlong varints
(`tag_ohb`, `val_ohb`, `len_ohb`, `packed_ohb`), non-canonical NaN bit
patterns (`nan_bits`), truncated payloads (`MISSING`), mismatched or
open groups (`END_MISMATCH`, `OPEN_GROUP`), and out-of-range field
numbers (`TAG_OOR`).  Repeated optional fields and interleaved fields
are visible as duplicate or out-of-order field names in the text output.
Together these make `prototext -d` a practical tool for auditing whether
a binary message conforms to the canonical encoding rules.

For full usage see `man prototext` or the
[online docs](https://douzebis.github.io/prototools).
