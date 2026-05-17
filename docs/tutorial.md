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
round-trips losslessly.  Supply a `--descriptor` and `--type` and the field
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

The rest of this tutorial walks you through all three scenarios step by step.

---

## Section 1 — Setup

Clone the repo and enter the Nix development shell.  All tools and dependencies
are provided automatically.

<button onclick="navigator.clipboard.writeText('git clone https://github.com/ThalesGroup/prototools\ncd prototools\nnix-shell')">copy</button>

```
$ git clone https://github.com/ThalesGroup/prototools
$ cd prototools
$ nix-shell
```

Confirm the tools are available:

<button onclick="navigator.clipboard.writeText('prototext --version\nreproto --version')">copy</button>

```
$ prototext --version
prototext 0.1.4

$ reproto --version
reproto, version 0.1.0
```

---

## Section 2 — Decode with a schema DB

The most powerful workflow starts with a pre-built schema DB.  The
`googleapis-db` Nix derivation compiles the entire public googleapis proto
corpus (~8 000 message types) into a single descriptor file with pre-built
scoring graphs for fast inference.

**Build the googleapis-db** (one-time; Nix caches the result):

<button onclick="navigator.clipboard.writeText('export GOOGLEAPIS_DB=$(nix-build -A googleapis-db --no-out-link)/googleapis.desc')">copy</button>

```
$ export GOOGLEAPIS_DB=$(nix-build -A googleapis-db --no-out-link)/googleapis.desc
```

The store path contains:
- `googleapis.desc` — the compiled FileDescriptorSet
- `googleapis/hopcroft.rkyv` — the scoring graph
- `googleapis/index.rkyv` — the lazy-loading index (only types needed are loaded)
- `instances/` — pre-generated sample `.pb` files for ~400 types

**Decode a sample message.**  The `instances/` directory contains ready-made
`.pb` files.  Let's decode one — without specifying the type, letting
`prototext` infer it automatically:

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB \\\n  decode \\\n  $(dirname $GOOGLEAPIS_DB)/instances/google/type/PostalAddress.pb')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode \
    $(dirname $GOOGLEAPIS_DB)/instances/google/type/PostalAddress.pb
#@ prototext: protoc
# Type: google.type.PostalAddress
# Score: 13  (matched: 13, unknown: 0, mismatches: 0, non_canonical: 0)

revision: 448
region_code: "s4678"
language_code: "s4996"
postal_code: "s4938"
sorting_code: "s8505"
administrative_area: "s7476"
locality: "s3682"
sublocality: "s1172"
address_lines: "s7447"
recipients: "s5598"
recipients: "s807"
recipients: "s346"
organization: "s8803"
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

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB \\\n  list-schemas \\\n  $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    list-schemas \
    $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
- path: …/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
  types:
  - google.cloud.compute.v1.UsableSubnetwork
  - google.cloud.compute.v1beta.UsableSubnetwork
```

Two types tie.  In this case the tie is unsurprising: `v1` and `v1beta` differ
only in maturity, not in wire format.  To decode, supply the correct type
explicitly:

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB \\\n  decode --type google.cloud.compute.v1beta.UsableSubnetwork \\\n  $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode --type google.cloud.compute.v1beta.UsableSubnetwork \
    $(dirname $GOOGLEAPIS_DB)/instances/google/cloud/compute/v1beta/UsableSubnetwork.pb
#@ prototext: protoc
subnetwork: "s5518"
ip_cidr_range: "s6809"
…
```

**Multi-file auto-inference** processes a batch of files and skips ambiguous
ones with a warning:

<button onclick="navigator.clipboard.writeText('INST=$(dirname $GOOGLEAPIS_DB)/instances\nprototext --descriptor $GOOGLEAPIS_DB decode -O /tmp/decoded \\\n  $INST/google/type/PostalAddress.pb \\\n  $INST/google/cloud/compute/v1beta/UsableSubnetwork.pb')">copy</button>

```
$ INST=$(dirname $GOOGLEAPIS_DB)/instances
$ prototext --descriptor $GOOGLEAPIS_DB decode -O /tmp/decoded \
    $INST/google/type/PostalAddress.pb \
    $INST/google/cloud/compute/v1beta/UsableSubnetwork.pb
warning: type inference issues:
- path: …/UsableSubnetwork.pb
  types:
  - google.cloud.compute.v1.UsableSubnetwork
  - google.cloud.compute.v1beta.UsableSubnetwork
```

`PostalAddress.pb` was decoded successfully (written to `/tmp/decoded/`);
`UsableSubnetwork.pb` was skipped with a warning.  The exit code is 0.

---

## Section 4 — Building a schema DB from scratch

The `googleapis-db` was built by `reproto`.  Here is the same complete workflow on a
smaller example: the protobuf Well-Known Types (WKT) that ship with `protoc`.

**Step 4a — compile some WKT `.proto` files into a standalone FileDescriptorSet.**

<button onclick="navigator.clipboard.writeText('protoc \\\n  --descriptor_set_out=/tmp/wkt.pb \\\n  --include_imports \\\n  google/protobuf/descriptor.proto \\\n  google/protobuf/timestamp.proto \\\n  google/protobuf/duration.proto \\\n  google/protobuf/any.proto')">copy</button>

```
$ protoc \
    --descriptor_set_out=/tmp/wkt.pb \
    --include_imports \
    google/protobuf/descriptor.proto \
    google/protobuf/timestamp.proto \
    google/protobuf/duration.proto \
    google/protobuf/any.proto
```

**Step 4b — build the schema DB with reproto:**

<button onclick="navigator.clipboard.writeText('reproto \\\n  --build-schema-db=/tmp/wkt.desc \\\n  /tmp/wkt.pb')">copy</button>

```
$ reproto \
    --build-schema-db=/tmp/wkt.desc \
    /tmp/wkt.pb
…
  descriptor: /tmp/wkt.desc
  graph:      /tmp/wkt/hopcroft.rkyv
  index:      /tmp/wkt/index.rkyv
```

**Step 4c — use the descriptor itself as a protobuf instance.**

The `/tmp/wkt.pb` file you just compiled *is* a binary protobuf
(`google.protobuf.FileDescriptorSet`).  Decode it with the schema DB you just
built — this is a nice self-referential demo, and auto-inference works because
the scoring graph is present:

<button onclick="navigator.clipboard.writeText('prototext --descriptor /tmp/wkt.desc \\\n  decode /tmp/wkt.pb | head -12')">copy</button>

```
$ prototext --descriptor /tmp/wkt.desc \
    decode /tmp/wkt.pb | head -12
#@ prototext: protoc
# Type: google.protobuf.FileDescriptorSet
# Score: 1846  (matched: 1846, unknown: 0, mismatches: 0, non_canonical: 0)

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

By default, `prototext decode` outputs clean, human-readable text with no
annotations — suitable for reading or diffing.  Pass `-a` / `--annotations`
to enable inline wire-type comments:

<button onclick="navigator.clipboard.writeText('INST=$(dirname $GOOGLEAPIS_DB)/instances\nprototext --descriptor $GOOGLEAPIS_DB \\\n  decode -a --type google.type.PostalAddress \\\n  $INST/google/type/PostalAddress.pb')">copy</button>

```
$ INST=$(dirname $GOOGLEAPIS_DB)/instances
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode -a --type google.type.PostalAddress \
    $INST/google/type/PostalAddress.pb
#@ prototext: protoc
revision: 448  #@ int32 = 1
region_code: "s4678"  #@ string = 2
language_code: "s4996"  #@ string = 3
postal_code: "s4938"  #@ string = 4
sorting_code: "s8505"  #@ string = 5
administrative_area: "s7476"  #@ string = 6
locality: "s3682"  #@ string = 7
sublocality: "s1172"  #@ string = 8
address_lines: "s7447"  #@ repeated string = 9
recipients: "s5598"  #@ repeated string = 10
recipients: "s807"  #@ repeated string = 10
recipients: "s346"  #@ repeated string = 10
organization: "s8803"  #@ string = 11
```

Each `#@ wire_type = field_number` annotation records the wire type and field
number seen on the wire.  This extra information is what makes lossless
round-tripping possible (see Section 6).

**Non-canonical encoding.**  The protobuf wire format allows several kinds of
non-canonical encoding that are semantically equivalent but byte-for-byte
different from what a normal encoder would produce.  Standard tools silently
normalise these; `prototext decode -a` preserves them.  For example, varints
can carry redundant continuation bytes (OHB — Over-Hanging Bytes): field 1
with value 448 is canonically `\x08\xc0\x03` but can also be encoded as
`\x08\xc0\x83\x00` (one extra byte, same value).  The `val_ohb` annotation
records how many such bytes were seen.

To demonstrate this, start by decoding `PostalAddress.pb` with annotations and
saving the result:

<button onclick="navigator.clipboard.writeText('INST=$(dirname $GOOGLEAPIS_DB)/instances\nprototext --descriptor $GOOGLEAPIS_DB \\\n  decode -a --type google.type.PostalAddress \\\n  $INST/google/type/PostalAddress.pb > /tmp/PostalAddress.textpb')">copy</button>

```
$ INST=$(dirname $GOOGLEAPIS_DB)/instances
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode -a --type google.type.PostalAddress \
    $INST/google/type/PostalAddress.pb > /tmp/PostalAddress.textpb
$ head -4 /tmp/PostalAddress.textpb
#@ prototext: protoc
revision: 448  #@ int32 = 1
region_code: "s4678"  #@ string = 2
language_code: "s4996"  #@ string = 3
```

Now patch the textual form to add one over-hanging byte on the `revision`
field.  Edit `/tmp/PostalAddress.textpb` and change the annotation on the
`revision` line from `#@ int32 = 1` to `#@ int32 = 1; val_ohb: 1`:

```
#@ prototext: protoc
revision: 448  #@ int32 = 1; val_ohb: 1
region_code: "s4678"  #@ string = 2
…
```

Then re-encode to produce the patched binary:

<button onclick="navigator.clipboard.writeText('prototext encode < /tmp/PostalAddress.textpb > /tmp/postal_patched.pb')">copy</button>

```
$ prototext encode < /tmp/PostalAddress.textpb > /tmp/postal_patched.pb
```

Compare the first bytes before and after the patch:

<button onclick="navigator.clipboard.writeText('hexdump -C $INST/google/type/PostalAddress.pb | head -1\nhexdump -C /tmp/postal_patched.pb | head -1')">copy</button>

```shell
$ hexdump -C $INST/google/type/PostalAddress.pb | head -1
00000000  08 c0 03 12 05 73 34 36  37 38 1a 05 73 34 39 39  |.....s4678..s499|

$ hexdump -C /tmp/postal_patched.pb | head -1
00000000  08 c0 83 00 12 05 73 34  36 37 38 1a 05 73 34 39  |......s4678..s49|
```

The patched file is one byte longer (`c0 83 00` instead of `c0 03`).  Now
decode it with annotations:

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB \\\n  decode -a \\\n  /tmp/postal_patched.pb | head -6')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode -a \
    /tmp/postal_patched.pb | head -6
#@ prototext: protoc
# Type: google.type.PostalAddress
# Score: 12  (matched: 13, unknown: 0, mismatches: 0, non_canonical: 1)

revision: 448  #@ int32 = 1; val_ohb: 1
region_code: "s4678"  #@ string = 2
```

`val_ohb: 1` records one over-hanging byte on the `revision` field.  All
other fields are unchanged.

**Impact on inference.**  The non-canonical encoding is still recognized as an
instance of `google.type.PostalAddress`, but with a slightly lower score
(12 vs 13 for the canonical version):

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB \\\n  list-schemas /tmp/postal_patched.pb')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    list-schemas /tmp/postal_patched.pb
- path: /tmp/postal_patched.pb
  types:
  - google.type.PostalAddress
```

The patched message still infers uniquely — non-canonical bytes are preserved
and scored, not silently discarded.

---

## Section 6 — Lossless round-trip and `prototext encode`

Annotations make lossless round-tripping possible.  Pipe annotated output
through `prototext encode` and compare with the original:

<button onclick="navigator.clipboard.writeText('INST=$(dirname $GOOGLEAPIS_DB)/instances\nprototext --descriptor $GOOGLEAPIS_DB decode -a \\\n  $INST/google/type/PostalAddress.pb \\\n  | prototext encode \\\n  | diff - $INST/google/type/PostalAddress.pb \\\n  && echo byte-exact')">copy</button>

```
$ INST=$(dirname $GOOGLEAPIS_DB)/instances
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode -a \
    $INST/google/type/PostalAddress.pb \
  | prototext encode \
  | diff - $INST/google/type/PostalAddress.pb \
  && echo byte-exact
byte-exact
```

The round-trip is byte-exact even for the patched non-canonical version:

<button onclick="navigator.clipboard.writeText('prototext --descriptor $GOOGLEAPIS_DB decode -a \\\n  /tmp/postal_patched.pb \\\n  | prototext encode \\\n  | diff - /tmp/postal_patched.pb \\\n  && echo byte-exact')">copy</button>

```
$ prototext --descriptor $GOOGLEAPIS_DB \
    decode -a \
    /tmp/postal_patched.pb \
  | prototext encode \
  | diff - /tmp/postal_patched.pb \
  && echo byte-exact
byte-exact
```

The over-hanging byte on `revision` is preserved exactly.

**Without `-a`** the output is clean human-readable text but carries no
`#@ prototext:` header.  `prototext encode` requires that header; feeding it
unannotated output is an error (exit 1).  Use `-a` whenever you intend to
round-trip through `prototext encode`.

---

## Section 7 — Decompile binary descriptors with `reproto`

`reproto` can reverse a `.pb` FileDescriptorSet back into `.proto` source.
Use the googleapis descriptor itself as a demonstration.
`--use-variant descriptor` tells reproto to use its own built-in copy of
`descriptor.proto` as the schema baseline, rather than reconstructing it from
the input — ensuring all well-known descriptor types resolve correctly:

<button onclick="navigator.clipboard.writeText('reproto --use-variant descriptor \\\n  -O /tmp/googleapis-src \\\n  $GOOGLEAPIS_DB')">copy</button>

```
$ reproto --use-variant descriptor \
    -O /tmp/googleapis-src \
    $GOOGLEAPIS_DB
Loading seed files
…
Writing reconstructed .proto files.
Skipping google/protobuf/descriptor.proto (use --emit-descriptor to include)
```

Inspect the reconstructed `timestamp.proto`:

<button onclick="navigator.clipboard.writeText('cat /tmp/googleapis-src/google/protobuf/timestamp.proto')">copy</button>

```
$ cat /tmp/googleapis-src/google/protobuf/timestamp.proto
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

## Further reading

- `man prototext` — full option reference for `prototext`
- `reproto --help` — all reproto modes and flags
- `prototext decode --help`, `prototext list-schemas --help` — per-subcommand help
- [prototools online docs](https://douzebis.github.io/prototools)
