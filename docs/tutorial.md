<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# prototools tutorial

**From first decode to schema inference — a step-by-step walkthrough**

---

## Introduction — Why prototools?

You work with protobuf data.  Maybe you extracted a `.pb` blob from a binary,
received a descriptor from a colleague, or are trying to figure out which of a
hundred message types matches the bytes in a network capture.  Standard tools
stop short:

**"I have a `.pb` binary blob, I don't know what's in it."**
`protoc --decode_raw` gives field numbers and wire types but no names, and it
silently discards non-canonical encodings.  `prototext decode` gives you
everything `protoc --decode_raw` gives, keeps every byte intact, and
round-trips losslessly.  Supply a `--descriptor-set` and `--type` and the field
numbers become readable names — and unlike `protoc --decode`, no original
`.proto` source files are required.

**"I have a binary message and a large schema DB, but I don't know which type
it is."**
`prototext list-schemas` scores the message against every type in the DB and
returns a ranked list of candidates.  With the googleapis corpus (~8 000 types)
and the lazy-loading index, this takes well under a second.

**"I have a descriptor blob extracted from a binary, but no `.proto` source."**
`reproto` reconstructs compilable `.proto` files from any `FileDescriptorProto`
or `FileDescriptorSet`, handling proto2, proto3, editions, options, and nested
types.  The output recompiles to a descriptor equivalent to the input.

**"I have an editions descriptor but my toolchain does not support editions."**
`reproto --force-proto2-output` translates an editions descriptor to
wire-compatible proto2 `.proto` source.  Field presence (`IMPLICIT`,
`EXPLICIT`, `LEGACY_REQUIRED`), group encoding (`DELIMITED`), and packed
annotations are all handled automatically.

The rest of this tutorial walks you through all four scenarios step by step.

---

## Section 1 — Setup

Clone the repo and enter the Nix development shell.  All tools and dependencies
— including `prototext`, `reproto`, `protoc`, `bat`, and the rest — are
provided automatically.

```
git clone https://github.com/ThalesGroup/prototools
cd prototools
nix-shell dev-shell.nix
```

> **Note:** all commands in this tutorial must be run inside `nix-shell
> dev-shell.nix`.  Outside that shell, tools such as `bat`, `protoc`, and the
> prototools binaries themselves may not be available.

Confirm the tools are available:

```
prototext --version
reproto --version
```

---

## Section 2 — Decode with a schema DB

The most powerful workflow starts with a pre-built schema DB.  The
`googleapis-db` Nix derivation compiles the entire public googleapis proto
corpus (~8 000 message types) into a single descriptor file with pre-built
scoring graphs for fast inference.

**Build the googleapis-db** (one-time; Nix caches the result):

```
export GOOGLEAPIS_DB=$(nix-build -A googleapis-db --no-out-link)/googleapis.desc
```

The store path contains:
- `googleapis.desc` — the compiled FileDescriptorSet
- `googleapis/hopcroft.rkyv` — the scoring graph
- `googleapis/index.rkyv` — the lazy-loading index (only types needed are loaded)
- `instances/` — pre-generated sample `.pb` files for ~400 types

**Decode a sample message.**  The `instances/` directory contains ready-made
`.pb` files.  Let's decode one — without specifying the type, letting
`prototext` infer it automatically:

```
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $(dirname $GOOGLEAPIS_DB)/instances/google/type/PostalAddress.pb
```

```
# Type: google.type.PostalAddress
# Score: 9  (matched: 9, unknown: 0, out_of_range: 0, non_canonical: 0, mismatches: 0)

revision: 1
organization: "S3NS"
address_lines: "Patchwork Montholon"
address_lines: "26 Rue de Montholon"
postal_code: "75009"
locality: "Paris"
administrative_area: "Île-de-France"
region_code: "FR"
language_code: "fr"
```

Real googleapis field names, repeated fields — inferred automatically from
the binary content alone, with no `--type` flag needed.  `protoc --decode`
requires both the type name and the original `.proto` source files; `prototext`
needs neither.

---

## Section 3 — Schema inference: the ambiguous case

Not every message infers to a unique type.  When the binary content matches
several schemas equally well, `prototext` reports all tied candidates.

Here is `google.cloud.compute.v1beta.UsableSubnetwork` — a message whose field
structure happens to match both the `v1` and `v1beta` API versions of the same
type:

```
prototext --descriptor-set $GOOGLEAPIS_DB \
    list-schemas \
    $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
```

```
- path: …/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
  types:
  - type: google.cloud.compute.v1.UsableSubnetwork
    score: 7
  - type: google.cloud.compute.v1beta.UsableSubnetwork
    score: 7
```

Two types tie.  In this case the tie is unsurprising: `v1` and `v1beta` differ
only in maturity, not in wire format.  To decode, supply the correct type
explicitly:

```
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode --type google.cloud.compute.v1beta.UsableSubnetwork \
    $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
```

```
subnetwork: "projects/my-project/regions/europe-west1/subnetworks/my-subnet"
network: "projects/my-project/global/networks/my-network"
ip_cidr_range: "10.132.0.0/20"
```

**Multi-file auto-inference** processes a batch of files and skips ambiguous
ones with a warning:

```
INST=$(dirname $GOOGLEAPIS_DB)/instances
prototext --descriptor-set $GOOGLEAPIS_DB decode -O stash/decoded \
    $INST/google/type/PostalAddress.pb \
    $INST/google/cloud/compute/v1beta/UsableSubnetwork.pb
```

```
warning: type inference issues:
- path: …/UsableSubnetwork.pb
  types:
  - type: google.cloud.compute.v1.UsableSubnetwork
    score: 7
  - type: google.cloud.compute.v1beta.UsableSubnetwork
    score: 7
```

`PostalAddress.pb` was decoded successfully (written to `stash/decoded/`);
`UsableSubnetwork.pb` was skipped with a warning.  The exit code is 0.

---

## Section 4 — Building a schema DB from scratch

The `googleapis-db` was built by `reproto`.  Here is the same complete workflow on a
smaller example: the protobuf Well-Known Types (WKT) that ship with `protoc`.

**Step 4a — compile some WKT `.proto` files into a standalone FileDescriptorSet.**

```
PROTOC_INCLUDE=$(dirname $(which protoc))/../include
protoc \
    -I$PROTOC_INCLUDE \
    --descriptor_set_out=stash/wkt.pb \
    --include_imports \
    google/protobuf/descriptor.proto \
    google/protobuf/timestamp.proto \
    google/protobuf/duration.proto \
    google/protobuf/any.proto
```

**Step 4b — build the schema DB with reproto:**

```
reproto \
    --schema-db-out=stash/wkt.desc \
    stash/wkt.pb
```

**Step 4c — use the descriptor itself as a protobuf instance.**

The `stash/wkt.pb` file you just compiled *is* a binary protobuf
(`google.protobuf.FileDescriptorSet`).  Decode it with the schema DB you just
built — this is a nice self-referential demo, and auto-inference works because
the scoring graph is present:

```
prototext --descriptor-set stash/wkt.desc \
    decode stash/wkt.pb | head -12
```

```
# Type: google.protobuf.FileDescriptorSet
# Score: 1846  (matched: 1846, unknown: 0, out_of_range: 0, non_canonical: 0, mismatches: 0)

file {
 name: "google/protobuf/descriptor.proto"
 package: "google.protobuf"
 message_type {
  name: "FileDescriptorSet"
  field {
   name: "file"
```

`prototext` inferred `google.protobuf.FileDescriptorSet` and decoded with
field names from the schema.

---

## Section 5 — Annotations and non-canonical encoding

By default, `prototext decode` outputs text with inline wire-type comments
(annotations) — required for lossless round-tripping.  Pass `--no-annotations`
to suppress them for clean, human-readable output:

```
INST=$(dirname $GOOGLEAPIS_DB)/instances
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $INST/google/type/PostalAddress.pb
```

```
#@ prototext: protoc
# Type: google.type.PostalAddress
# Score: 9  (matched: 9, unknown: 0, out_of_range: 0, non_canonical: 0, mismatches: 0)

revision: 1  #@ int32 = 1
organization: "S3NS"  #@ string = 11
address_lines: "Patchwork Montholon"  #@ repeated string = 9
address_lines: "26 Rue de Montholon"  #@ repeated string = 9
postal_code: "75009"  #@ string = 4
locality: "Paris"  #@ string = 7
administrative_area: "Île-de-France"  #@ string = 6
region_code: "FR"  #@ string = 2
language_code: "fr"  #@ string = 3
```

Each `#@ wire_type = field_number` annotation records the wire type and field
number seen on the wire.  This extra information is what makes lossless
round-tripping possible (see Section 7).

**Non-canonical encoding.**  The protobuf wire format allows several kinds of
non-canonical encoding that are semantically equivalent but byte-for-byte
different from what a normal encoder would produce.  Standard tools silently
normalise these; `prototext decode` (with annotations, the default) preserves them.  For example, varints
can carry redundant continuation bytes (OHB — Over-Hanging Bytes): the value 1
is canonically `\x01` but can also be encoded as `\x81\x00` (one extra byte,
same value).  The `val_ohb` annotation records how many such bytes were seen.

To demonstrate this, start by decoding `PostalAddress.pb` with annotations and
saving the result:

```
INST=$(dirname $GOOGLEAPIS_DB)/instances
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $INST/google/type/PostalAddress.pb > stash/PostalAddress.textpb
```

Now patch the textual form to add one over-hanging byte on the `revision`
field.  Edit `stash/PostalAddress.textpb` and change the annotation on the
`revision` line from `#@ int32 = 1` to `#@ int32 = 1; val_ohb: 1`:

```
#@ prototext: protoc
revision: 1  #@ int32 = 1; val_ohb: 1
organization: "S3NS"  #@ string = 11
…
```

Then re-encode to produce the patched binary:

```
prototext encode < stash/PostalAddress.textpb > stash/postal_patched.pb
```

Compare the first bytes before and after the patch:

```
hexdump -C $INST/google/type/PostalAddress.pb | head -1
hexdump -C stash/postal_patched.pb | head -1
```

```
00000000  08 01 5a 04 53 33 4e 53  4a 13 50 61 74 63 68 77  |..Z.S3NSJ.Patchw|
00000000  08 81 00 5a 04 53 33 4e  53 4a 13 50 61 74 63 68  |...Z.S3NSJ.Patch|
```

The patched file is one byte longer (`81 00` instead of `01`).  Now
decode it (annotations on by default):

```
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    stash/postal_patched.pb | head -6
```

```
#@ prototext: protoc
# Type: google.type.PostalAddress
# Score: -11  (matched: 9, unknown: 0, out_of_range: 0, non_canonical: 1, mismatches: 0)

revision: 1  #@ int32 = 1; val_ohb: 1
organization: "S3NS"  #@ string = 11
```

`val_ohb: 1` records one over-hanging byte on the `revision` field.  All
other fields are unchanged.

**Impact on inference.**  The patched message still infers uniquely as an
instance of `google.type.PostalAddress`, but with a slightly lower score
(-11 vs 9 for the canonical version) — non-canonical bytes are preserved
and scored, not silently discarded.

---

## Section 6 — Hidden fields

The protobuf wire format allows a field to appear more than once in a message,
even when the schema declares it `optional`.  Standard decoders follow the
proto3 rule: last value wins, earlier occurrences are silently discarded.  This
is exploitable: an attacker can prepend a secret value to an optional field;
every ordinary decoder sees only the innocuous final value, and the secret is
invisible.

**Craft a binary with a hidden `organization` field.**  Start from the annotated
`PostalAddress.pb` and insert a second occurrence of field 11 (`organization`)
*before* the legitimate value, then re-encode:

```
INST=$(dirname $GOOGLEAPIS_DB)/instances
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $INST/google/type/PostalAddress.pb \
  | sed '/^organization: "S3NS"/i organization: "Entrance secret PIN code: 666*"  #@ string = 11' \
  > stash/postal_hidden.textpb

prototext encode < stash/postal_hidden.textpb > stash/postal_hidden.pb
```

**What `protoc --decode` sees** (uses `stash/googleapis-src` produced in
Section 8; if you are following the tutorial in order, skip this step and return
to it after completing Section 8):

```
protoc --proto_path stash/googleapis-src \
    --decode google.type.PostalAddress \
    google/type/postal_address.proto \
  < stash/postal_hidden.pb
```

```
revision: 1
organization: "S3NS"
address_lines: "Patchwork Montholon"
…
```

The secret value is gone.  `protoc` — and any ordinary protobuf SDK — applies
last-write-wins and returns only `"S3NS"`.

**What `prototext decode` sees:**

```
prototext --descriptor-set $GOOGLEAPIS_DB decode stash/postal_hidden.pb
```

```
# Type: google.type.PostalAddress
# Score: 9  (matched: 9, unknown: 0, out_of_range: 0, non_canonical: 0, mismatches: 0)

organization: "Entrance secret PIN code: 666*"
organization: "S3NS"
address_lines: "Patchwork Montholon"
…
```

Both occurrences are preserved in wire order.  The hidden value is exposed.

---

## Section 7 — Lossless round-trip and `prototext encode`

Annotations make lossless round-tripping possible.  Pipe annotated output
through `prototext encode` and compare with the original:

```
INST=$(dirname $GOOGLEAPIS_DB)/instances
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $INST/google/type/PostalAddress.pb \
  | prototext encode \
  | diff - $INST/google/type/PostalAddress.pb \
  && echo byte-exact
```

```
byte-exact
```

The round-trip is byte-exact even for the patched non-canonical version:

```
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    stash/postal_patched.pb \
  | prototext encode \
  | diff - stash/postal_patched.pb \
  && echo byte-exact
```

```
byte-exact
```

The over-hanging byte on `revision` is preserved exactly.

**Without `-a`** the output is clean human-readable text with no header and no
inline annotations.  `prototext encode` requires both; feeding it unannotated
output is an error (exit 1).  Use `-a` whenever you intend to round-trip
through `prototext encode`.

---

## Section 8 — Decompile binary descriptors with `reproto`

`reproto` can reverse a `.pb` FileDescriptorSet back into `.proto` source.
Use the googleapis descriptor itself as a demonstration.
`--use-variant descriptor` tells reproto to use its own built-in copy of
`descriptor.proto` as the schema baseline, rather than reconstructing it from
the input — ensuring all well-known descriptor types resolve correctly:

```
reproto --use-variant descriptor \
    -O stash/googleapis-src \
    $GOOGLEAPIS_DB
```

Inspect the reconstructed `timestamp.proto`:

```
cat stash/googleapis-src/google/protobuf/timestamp.proto
```

```
// google/protobuf/timestamp.proto

syntax = "proto3";

package google.protobuf;

option java_package = "com.google.protobuf";
option java_outer_classname = "TimestampProto";
option java_multiple_files = true;
option go_package = "google.golang.org/protobuf/types/known/timestamppb";
option cc_enable_arenas = true;
option objc_class_prefix = "GPB";
option csharp_namespace = "Google.Protobuf.WellKnownTypes";

message Timestamp {
  int64 seconds = 1;
  int32 nanos = 2;
}
```

The reconstructed source preserves all options, package names, and field
types.  The output recompiles to a descriptor equivalent to the input.

The same workflow applies whenever you have a `.pb` FileDescriptorSet or
FileDescriptorProto and need the original `.proto` source — for example when
a descriptor has been extracted from a Go, Java, or Python binary by other means.

---

## Section 9 — Translating editions descriptors to proto2

Some toolchains (e.g. prost-reflect as of 2026) do not yet support the
editions syntax introduced in protobuf 27.  `reproto --force-proto2-output`
translates an editions descriptor to proto2 source while preserving wire
semantics.

**Start with an editions `.proto` file.**  Use the `editions_rendering.proto`
fixture from the reproto test suite as a concrete example — it exercises all
the translation decisions:

```
cat reproto/src/reproto/tests/fixtures/editions_rendering.proto
```

```
edition = "2023";

package reproto.test.rendering;

message Inner {
  int32 value = 1;
}

message AllFeatures {
  string implicit_field = 1 [features.field_presence = IMPLICIT];
  string explicit_field = 2 [features.field_presence = EXPLICIT];
  string required_field = 3 [features.field_presence = LEGACY_REQUIRED];
  repeated int32 expanded_ids = 4 [features.repeated_field_encoding = EXPANDED];
  Inner delimited_field = 5 [features.message_encoding = DELIMITED];
  int32 with_default = 6 [features.field_presence = EXPLICIT, default = 42];
  repeated int32 packed_ids = 7 [features.repeated_field_encoding = PACKED];
}
```

**Compile to a descriptor set:**

```
protoc \
    --descriptor_set_out=stash/editions_rendering.pb \
    --include_imports \
    -Ireproto/src/reproto/tests/fixtures \
    reproto/src/reproto/tests/fixtures/editions_rendering.proto
```

**Reconstruct as editions (default — no flag):**

```
reproto --use-variant descriptor \
    --proto-out=stash/out_editions \
    stash/editions_rendering.pb
```

```
cat stash/out_editions/editions_rendering.proto
```

```
// editions_rendering.proto

edition = "2023";

package reproto.test.rendering;

message Inner {
  int32 value = 1;
}

message AllFeatures {
  string implicit_field = 1 [features.field_presence = IMPLICIT];
  string explicit_field = 2 [features.field_presence = EXPLICIT];
  string required_field = 3 [features.field_presence = LEGACY_REQUIRED];
  repeated int32 expanded_ids = 4 [features.repeated_field_encoding = EXPANDED];
  Inner delimited_field = 5 [features.message_encoding = DELIMITED];
  int32 with_default = 6 [
    default = 42,
    features.field_presence = EXPLICIT
  ];
  repeated int32 packed_ids = 7 [features.repeated_field_encoding = PACKED];
}
```

**Reconstruct as proto2 (`--force-proto2-output`):**

```
reproto --use-variant descriptor \
    --force-proto2-output \
    --proto-out=stash/out_proto2 \
    stash/editions_rendering.pb
```

```
cat stash/out_proto2/editions_rendering.proto
```

```
// editions_rendering.proto

// WARNING[editions]: editions file rendered as proto2 (--force-proto2-output)
syntax = "proto2";

package reproto.test.rendering;

message Inner {
  optional int32 value = 1;
}

message AllFeatures {
  optional string implicit_field = 1;
  optional string explicit_field = 2;
  required string required_field = 3;
  repeated int32 expanded_ids = 4;
  optional group DelimitedField = 5 {
    optional int32 value = 1;
  }

  optional int32 with_default = 6 [default = 42];
  repeated int32 packed_ids = 7 [packed = true];
}
```

Each editions feature is translated to its proto2 equivalent:

| editions source | proto2 output | Notes |
|---|---|---|
| `field_presence = IMPLICIT` | `optional` | wire-compatible; proto2 gains a `has_field()` accessor that did not exist in the original |
| `field_presence = EXPLICIT` | `optional` | identical semantics |
| `field_presence = LEGACY_REQUIRED` | `required` | identical semantics |
| `message_encoding = DELIMITED` | `group DelimitedField` | group name derived from field name |
| `repeated_field_encoding = EXPANDED` | _(no annotation)_ | unpacked is proto2 default |
| `repeated_field_encoding = PACKED` | `[packed = true]` | proto2 default is unpacked |
| `default = 42` | `[default = 42]` | preserved |

The output recompiles with `protoc` without errors and is wire-compatible with
the original editions descriptor.  See `docs/reproto/force-proto2-output.md` for the
complete translation reference.

---

## Live demo

A fully scripted live demo is available at `demo/01-tutorial.sh`.  It covers
the same ground as this tutorial — decoding, schema auto-inference, non-canonical
protobufs, scoring DB construction, decompilation, and seeding/pruning — in a
step-by-step presenter format driven by `demo/prompt`.

To run it from the repo root (must be inside `nix-shell dev-shell.nix` — the
demo uses `bat`, `protoc`, and other tools provided by the dev shell):

```bash
./demo/prompt --splash prototools demo/01-tutorial.sh
```

Generated files are written under `stash/` (gitignored).  The demo takes
approximately one hour end-to-end.

---

## Further reading

- `man prototext` — full option reference for `prototext`
- `reproto --help` — all reproto modes and flags
- `prototext decode --help`, `prototext list-schemas --help` — per-subcommand help
- `docs/reproto/force-proto2-output.md` — complete reference for `--force-proto2-output` translation rules
- [prototools online docs](https://douzebis.github.io/prototools)
