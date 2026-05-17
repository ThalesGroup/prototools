<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0070 — Tutorial: reproto and prototext, from first steps to schema inference

**Status:** implemented
**Implemented in:** 2026-05-17
**App:** docs

---

## Purpose

New users arrive at prototools without context.  The READMEs describe what
each tool does, but there is no single narrative that shows how `reproto` and
`prototext` complement each other in a realistic workflow.  This spec defines
the structure and content of such a tutorial.

The tutorial starts at the top — showing inference against a large pre-built
schema DB immediately — then drills down: how to build such a DB, the
annotations format, lossless round-tripping, and finally reproto's decompiler.

---

## Goals

1. A self-contained, step-by-step tutorial document at
   `docs/tutorial.md` (rendered on the website).
2. Every shell session is copy-pasteable inside `nix-shell`.
3. The tutorial uses only assets that are part of the repo or derivable from
   it — no external downloads beyond what Nix already pins.
4. It opens with a motivating pitch (Section 0) that sells the tools before
   showing any commands, then proceeds top-down: decode with a schema DB →
   build a DB → auto-inference → annotations → non-canonical encoding →
   lossless round-trip → impact on scoring → decompile descriptors.

---

## Non-goals

- API documentation (covered by man pages and READMEs).
- Advanced reproto options (`--seed`, `--prune`, editions, proto2).
- Explaining the scoring algorithm internals.
- Installing prototools outside Nix (covered by READMEs).

---

## Structure

The tutorial is a single Markdown document with eight sections.  Each section
builds on the previous one.

### Section 0 — Why prototools?

A short motivating pitch (no shell commands) aimed at a reader who works with
protobuf data and wonders whether prototools is worth their time.  Three
concrete pain points, each paired with the tool that solves it:

**Pain point 1 — "I have a `.pb` binary blob, I don't know what's in it."**
Standard `protoc --decode_raw` gives field numbers and wire types but no names,
and it discards non-canonical encodings.  `prototext decode` gives the same
information but keeps every byte intact and round-trips losslessly.  With a
matching descriptor (`--descriptor`) and type (`--type`) the field numbers
become readable names — just like `protoc --decode`, but lossless and
schema-optional.

**Pain point 2 — "I have a binary message and a large schema DB, but I don't
know which type it is."**
`prototext list-schemas` scores the message against every type in the DB and
returns a ranked list of candidates.  With the googleapis corpus (~8 000 types)
and the lazy-loading index, this takes well under a second.

**Pain point 3 — "I have a descriptor blob extracted from a binary, but no
`.proto` source."**
`reproto` reconstructs compilable `.proto` files from any `FileDescriptorProto`
or `FileDescriptorSet`, handling proto2, proto3, editions, options, and nested
types.  The output recompiles to a descriptor equivalent to the input.

Close with one sentence: "The rest of this tutorial walks you through all three
scenarios step by step."

### Section 1 — Setup

One paragraph explaining prerequisites: clone the repo, enter `nix-shell`.
Confirm `prototext --version` and `reproto --version` work.

```
$ git clone https://github.com/douzebis/prototools
$ cd prototools
$ nix-shell
$ prototext --version
$ reproto --version
```

### Section 2 — Decode with a schema DB (googleapis)

Start with the most powerful scenario first.  Build the googleapis-db (one-time,
cached by Nix), point `prototext` at it, and decode a binary message by type.

**Step 2a** — build the googleapis-db:

```
$ nix-build -A googleapis-db
$ GOOGLEAPIS_DB=$(readlink -f result)/googleapis.desc
```

Explain briefly: `googleapis.desc` is a compiled FileDescriptorSet covering
~8 000 googleapis message types; the adjacent `googleapis/` directory holds
pre-built scoring graphs for fast inference.

**Step 2b** — generate a sample message and decode it with the known type:

```
$ reproto-instantiate-schema \
    --descriptor "$GOOGLEAPIS_DB" \
    --seed 42 \
    google.type.PostalAddress \
    > /tmp/postal.pb
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    decode --type google.type.PostalAddress \
    /tmp/postal.pb
```

Show the clean output (no `-a`): real googleapis field names, nested messages,
enum symbolic values — just like `protoc --decode`.

### Section 3 — Schema inference: unambiguous case

Motivate `list-schemas`: same binary blob, but pretend you don't know the type.

```
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    list-schemas /tmp/postal.pb
```

Show the YAML output with `google.type.PostalAddress` at the top.  Explain
the score briefly: more matched fields = higher confidence.

### Section 4 — Schema inference: ambiguous case and --strict

Pick a message type with few fields (e.g. `google.protobuf.Duration` — two
integer fields) to show that several types can tie.

```
$ reproto-instantiate-schema \
    --descriptor "$GOOGLEAPIS_DB" \
    --seed 42 \
    google.protobuf.Duration \
    > /tmp/duration.pb
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    list-schemas /tmp/duration.pb
```

Show that multiple types tie.  Then show `prototext decode` in auto-inference
mode on both files at once:

```
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    decode /tmp/postal.pb /tmp/duration.pb
```

Show that `/tmp/postal.pb` is decoded (unambiguous) while `/tmp/duration.pb`
emits a `warning: type inference issues:` on stderr and is skipped.  Explain
`--strict` (treat warnings as errors → exit 1).

### Section 5 — Building a schema DB from scratch

Show how the googleapis-db was built, using the WKT protos as a simpler
worked example (they ship with `protoc`, no corpus download needed).

**Step 5a** — compile the WKT `.proto` files into a single FDS:

```
$ protoc \
    --descriptor_set_out=/tmp/wkt.pb \
    --include_imports \
    google/protobuf/timestamp.proto \
    google/protobuf/duration.proto \
    google/protobuf/any.proto
```

**Step 5b** — build the schema DB with reproto:

```
$ reproto \
    --use-variant all \
    --build-schema-db=/tmp/wkt.desc \
    -I /tmp \
    wkt.pb
```

**Step 5c** — decode using the WKT descriptor themselves as instances:

```
$ prototext --descriptor /tmp/wkt.desc \
    decode --type google.protobuf.FileDescriptorSet \
    /tmp/wkt.pb
```

Show that `prototext` handles the WKT descriptor the same way it handles any
binary protobuf.  Note that by default (without `--descriptor`), `prototext`
already has the WKT schemas built in — this step just demonstrates the
general workflow.

### Section 6 — Annotations and non-canonical encoding

Introduce `-a` / `--annotations`.  Decode the same `postal.pb` with
annotations enabled:

```
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    decode -a --type google.type.PostalAddress \
    /tmp/postal.pb
```

Walk through the annotation format: wire type, field number, value.

Now hand-craft a non-canonical varint by patching a tiny protobuf.  Use a
`google.protobuf.Timestamp` (two fields) so the binary is short enough to
show in full:

```
$ reproto-instantiate-schema \
    --descriptor "$GOOGLEAPIS_DB" \
    --seed 42 \
    google.protobuf.Timestamp \
    > /tmp/ts.pb
$ od -A n -t x1 /tmp/ts.pb
```

Use `prototext decode -a` to show the canonical encoding, then produce a
non-canonical version by replacing one varint byte with an overhung equivalent
(e.g. `\x08\xaa\x00` instead of `\x08\x2a`).  Show that prototext decodes
both and annotates the overhung one with `val_ohb: 1`.

### Section 7 — Lossless round-trip and encode

Show that annotated output round-trips byte-exact:

```
$ prototext --descriptor "$GOOGLEAPIS_DB" \
    decode -a --type google.type.PostalAddress \
    /tmp/postal.pb | \
  prototext encode | \
  diff - /tmp/postal.pb && echo "byte-exact"
```

Then show the non-canonical case: the patched binary also round-trips
byte-exact because the `val_ohb` annotation preserves the extra bytes.

Explain that without `-a`, the output is clean human-readable text but cannot
be fed back to `prototext encode` losslessly (the annotation metadata is gone).

Show the impact on scoring: decode the non-canonical `ts.pb` and run
`list-schemas` — the overhung varint is a mild signal that helps distinguish
it from canonical messages.

### Section 8 — Decompile binary descriptors with reproto

Close the loop: show reproto's decompiler, which turns a `.pb` descriptor
back into `.proto` source.

Use the WKT descriptor from Section 5:

```
$ reproto --use-variant descriptor \
    -O /tmp/wkt-src \
    /tmp/wkt.pb
$ cat /tmp/wkt-src/google/protobuf/timestamp.proto
```

Show that the output is valid `.proto` source that recompiles to an equivalent
descriptor.  Mention that this is the same tool used to recover `.proto` from
descriptors embedded in Go/Java/Python binaries.

Closing paragraph: point to the man page (`man prototext`) and the reproto
README for full option reference.

---

## Files changed

- `docs/tutorial.md` — new tutorial document
- `website/content/tutorial.md` is auto-generated by the deploy workflow
  (inject step already handles `docs/*.md`)
- `docs/specs/0070-tutorial.md` — this file

---

## Notes for the implementer

- All shell sessions must be tested inside `nix-shell` before publishing.
- The `reproto-instantiate-schema` alias is available in `nix-shell`; note
  it for readers who use `cargo install` (they call `reproto instantiate-schema`
  directly via the Python module — clarify this in Section 2).
- Use realistic but short output excerpts; truncate long outputs with `...`
  and a note.
- Use `--seed 42` throughout for reproducibility.
- Verify the exact YAML output of `list-schemas` for each example before
  writing the doc (scores may shift as the scoring algorithm evolves).
