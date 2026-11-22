<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0453 — git-seriate-fdp: git-seriate for descriptors embedded in executables

Status: implemented
Implemented in: 2026-11-22
App: git-seriate-fdp | git-seriate | fdp-scan | prototext-core | protoback
Refs: docs/specs/0450-git-seriate-a-history-from-snapshots.md (S1 this
        crate's place, S2 the canonicalizer's contract, S5 step 3),
      docs/specs/0452-git-seriate-the-store-and-the-history.md (the
        plugin traits this crate implements, and the binary it extends),
      docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md,
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md
        (the scanner, its refusals and its options)

## Background

`git-seriate` stores snapshots of any files and rebuilds a history from
them; what is specific to a kind of file comes through plugins (0452 S2).
This spec is the first kind: the `FileDescriptorProto` records (FDPs)
embedded in executables, which `fdp-scan` finds. A snapshot is what one
executable embeds; its items are its descriptors, stored as prototext's
lossless text so that the history reads as a history of `.proto` files.

One executable's descriptors and another's differ in more than their
history: code generators alter what they embed. Measured downstream on
one build of one package: a C++ binary's FDP, the same file's in a Go
binary (fields in another order), and in another C++ binary (`json_name`
on every field) are one version written three ways. Read raw, the sort
takes those flavors for history. The canonicalizer (0450 S2) removes what
a generator adds or reorders, and nothing the source can say.

## Goals

- **G1.** One binary, `git-seriate-fdp` (run as `git seriate-fdp`): every
  `git seriate` command, with the protobuf plugins, plus `extract`, which
  adds executables as snapshots.
- **G2.** The extractor is `fdp-scan`, against the stock
  `descriptor.proto` or the caller's; every refused candidate is recorded,
  with its reason.
- **G3.** Items are stored as prototext text and read back to the same
  bytes: checked on every write.
- **G4.** A canonicalizer with two rules, versioned as the rule set
  `fdp`, composable with a caller's own rule sets.
- **G5.** The history of the generic tests holds for descriptors: the same
  snapshots in any order of extraction give the same history.

## Non-goals

- **N1.** `advise`: its own spec, next (0450 S5 step 4).
- **N2.** A comparator for descriptors. The text comparator reads the
  rendered text; with the canonicalizer, the flavors it was misled by are
  gone. A descriptor comparator (facts per message, a field reserved as
  evidence of a deletion) is for when the text one is shown wanting.
- **N3.** Sources other than files on disk: package formats, gRPC
  reflection. A caller's own binary adds those (0452 S2).
- **N4.** Rules specific to one build system, such as the
  `option_dependency` a Python build adds naming the file's own module: a
  caller's rule set, composed with this one (S4).
- **N5.** Scanning a file stored in pieces without joining them (0450 N4).

## Specification

### S1 — The crate

`git-seriate-fdp` (0450 S1): a library with the plugins of S2–S4, and the
binary of S5. Depends on `git-seriate`, `fdp-scan`, `prototext-core`
(and through it `prototext-upb`, whose message descriptors the renderer
and R2 read), and `prototext` for the embedded `descriptor.proto`. This is
the one crate of the three that pulls in upb (0450 S1).

`git-seriate`'s command line becomes a library entry point,
`git_seriate::cli::run(args, &Plugins, extra)`, which the `git-seriate`
binary calls with the generic plugins, and `git-seriate-fdp` with its own
and the `extract` command. One parser and one set of messages for both.

### S2 — The extractor

`FdpExtractor { schema: &Schema, options: fdp_scan::ScanOptions }`, where
`Schema` is the stock one (`scan_explained_options`) or a caller's graph
and root (`scan_explained_with_options`), loaded from a schema database
written with `protoback --schema-db-out DB.desc --emit-extension-ranges`
(0448 S4): its descriptor set `DB.desc`, and its graph
`DB/hopcroft.rkyv`.

**The schema is a repository setting,** `git config seriate.fdpSchema
DB.desc` (relative to the work tree, or the git directory of a bare
repository) and `seriate.fdpRoot` (`google.protobuf.FileDescriptorProto`
by default), read by every command, not a flag of `extract`: `settle`
renders canonical forms with the renderer of S3, and a schema other than
the one items were extracted with would render every one of them anew.

- **Items:** each record found, at the path its `name` gives with `.proto`
  replaced by `.pb`; bytes the record. A record is found by its `name`,
  field 1 (0448): a writer that put another field before it would lose
  that field to the scan; none measured does. The scanner's plausibility rules
  (0448 S5) run first; the store's path rules (0452 S3) then, on what they
  let through.
- **Refusals,** in the block's `rejected`, with the scanner's reasons as
  codes: `implausible-name`, `name-only`, `defect`, `dangling-dependency`,
  `implausible-edition` (0456);
  then the store's (`dotgit`, `under-file`, …).
- **Order cuts** (with `--increasing-field-numbers`, 0449 S7): in the
  block's `record`, one entry per record the option shortened,
  `order-cut <path>: <start>..<end>, out of order at <at>`. The record is
  kept; the entry says what it lost.

### S3 — The renderer

- **render:** `prototext_core::render_as_text` with annotations, against
  `FileDescriptorProto` of the extractor's schema, so that a caller's
  custom options read by name.
- **parse:** `prototext_core::render_as_bytes`, which needs no schema: the
  annotations carry every field's number and type.
- **Checked:** every item's rendering is parsed back on write and compared
  with the bytes it came from. A difference refuses the item, with the
  reason `render` (0452 S2's one rule for render failures): a stored item
  is always one that reads back exactly.

The stored files keep the `.pb` extension, holding text: the existing
repositories' convention, and the item pattern `*.pb` (S5).

### S4 — The canonicalizer

The rule set `fdp`, version 1, applied to an FDP's bytes:

- **R1.** A field's `json_name` is dropped where it equals what protoc
  derives from the field's name (`ToJsonName`: underscores removed, the
  letter after each one upper-cased). protoc derives it again, so nothing
  is lost. One function computes it for this rule and for protoback's
  rendering, which today has its own (`camel_case` in
  `protoback/src/render/message.rs`): moved to `prototext-core`, and
  checked against protoc's on every field of googleapis.
- **R2.** Fields re-encoded in field-number order, recursively in the
  fields `descriptor.proto` declares as messages, the occurrences of one
  number keeping their order (a repeated field's order is its meaning).
  protoc's C++ writer emits this order already (0449 Background), so its
  descriptors are unchanged; other writers' converge to it, as far as R2
  reaches: a difference inside a field `descriptor.proto` doesn't declare
  (a custom option's payload, say) is left as it is, so a flavor carrying
  one survives. Whether that matters is measured, not assumed. A field the
  schema doesn't declare is ordered by its number and not entered.

R1 then R2. Both are idempotent, and each only removes or reorders, so
`canon(canon(x)) == canon(x)`. A later version adds rules and never
undoes one, which is what 0450 S2's equation asks of it.

`git_seriate::plugins::Composed`, a list of canonicalizers applied in
order, their rule sets concatenated: a caller composes `fdp` with its
own (N4).

### S5 — The binary

```
git seriate-fdp extract [--id ID] [--build-time T] [--order-hint N] [--group G]
                        [--meta KEY=VALUE]… [--increasing-field-numbers] FILE…
git seriate-fdp add | list | drop | import | settle | compact   (as git seriate)
```

`extract` makes one snapshot per `FILE`. Its identity, by default, is the
file's path relative to the current directory, made a valid branch name
(a root, `.` and `..` dropped; in each component a leading `.`, a `.lock`
ending, `..`, `@{`, spaces, control characters and `~^:?*[\` replaced by
`_`); `--id` names it when one file
is given. The same file extracted from another directory then gets
another identity, and is another snapshot: extract from one place, or
name it. Its build time defaults to the file's modification time, which
files from content-addressed stores, archives and copies often share or
lack: `extract` warns when it uses a time of 1 or less, or one every file
of the run shares, and says to give `--build-time`. A file
with no descriptor and no refusal adds nothing; one with refusals only
adds a record-only branch, so that what was refused is kept (0452 S3).

`settle` and `compact` use S3's renderer, S4's canonicalizer, the text
comparator, and `*.pb` as the item pattern when the repository sets none.

## Alternatives considered

**The name `protosets`,** as first drafted. It is the name of the
downstream tool that will build on this crate: the two binaries would
collide on a path, the tool would depend on a crate of its own name, and
every spec reference would be ambiguous. `git-seriate-fdp` says what it
is, and git runs it as `git seriate-fdp`.

**Storing FDPs as binary.** Forges show binary diffs as "binary files
differ"; the readable history is what people review (0450 N5).

**Canonicalizing in the renderer** (storing every item canonical, raw
nowhere). The snapshot branches would lose the raw items 0450 S2 keeps
them for, and a rule change could not be re-applied from them.

**Field order by declaration in `descriptor.proto`** rather than by
number. Declaration order is a property of one version of the schema;
numbers are what protoc orders by, and what every version agrees on.

**A copy of `camel_case` in git-seriate-fdp.** Two functions computing protoc's
`ToJsonName` would drift; one, tested once against protoc, serves both.

## Test plan

1. The extractor, on `fdp-scan`'s corpus layouts as executables: every
   true record an item, at its path; the decoys refused with their codes;
   an order cut recorded.
2. The renderer: every descriptor of googleapis rendered and parsed back
   to its bytes; an item that fails the check refused as `render`.
3. R1: a default `json_name` dropped, another kept, against protoc's own
   (`protoc --descriptor_set_out` with and without `json_name` set).
   The shared function on every googleapis field.
4. R2: a descriptor with fields out of order (Go's shape) and the same in
   order give one canonical form; a repeated field's order kept; protoc's
   output unchanged.
5. Idempotency and composition: `canon(canon(x)) == canon(x)` on
   googleapis; `Composed` with a test rule set records both versions.
6. End to end: executables carrying the same file in two flavors (a
   protoc-built one and one with `json_name` added and fields reordered)
   extracted, settled: one version in the history, not two; the history
   the same whichever is extracted first; protoback reads the history
   (`--schema-db-out`) without a warning. And with `Composed`: a third
   flavor that only a test rule set (removing one field) collapses, one
   version with the composition, two without it. (Reading the history
   with protoback is left to the downstream tool's acceptance, which
   builds its schema database from such a history.)
7. `extract`'s warnings: a modification time of 1, and one shared by every
   file.

## Measured outcome

- **Three flavors of one file,** protoc's C++ form, another writer's
  (fields out of order, every `json_name` set) and a Python build's (one
  field more), extracted in either order: one version in the history with
  `fdp` composed with a test rule set for the third, two with `fdp` alone.
- **The renderer** reads back every file of the stock descriptor set; R1
  keeps exactly the `json_name` protoc's own output sets explicitly.
- **protoback,** on the shared `json_name`: its self-check (1 669 cases,
  1 655 graph dumps) finds no difference.
