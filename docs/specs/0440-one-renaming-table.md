<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0440 — one renaming table for the variant's rewrites

Status: implemented
Implemented in: 2026-11-17
App: reproto, protoback
Refs: docs/specs/0429-protoback-rendering.md (S17: the rewriting left as
        reproto's until the port was done),
      docs/specs/0434-type-names-as-protoc-resolves-them.md (N2: rewritten
        names written whole; S4: the naming convention this spec extends to
        them),
      docs/specs/0437-rewrite-collisions.md (file paths: collisions
        detected before rendering; this spec does the same for symbols),
      docs/specs/0439-option-names-and-feature-paths.md (option names,
        resolved from the element's scope)

## Background

A variant's `import_rewrites` and `namespace_rewrites` rename files,
packages and symbols in the output (`.priv.` → `.google.protobuf.`). Both
tools apply the rules where each name is written: about twenty call sites
in reproto (`re_file`, `re_field`, `re_method`, `syntax`, `utils`,
`phases`), fifteen in protoback (`render/binary`, `render/file`,
`render/mod`, `render/names`, `schema_db`), each with its own
`--keep-descriptor-path` guard. Two defects follow, reproduced
2026-11-17 in both tools with a variant rewriting `.priv.` to
`.google.protobuf.` and `priv/` to `google/protobuf/`, on an input whose
`app/use.proto` uses `priv.Note`, `priv.Empty` and `google.protobuf.Empty`:

1. **Rewritten names are written whole.** `optional .google.protobuf.Note
   note = 1;` beside `optional google.protobuf.Empty theirs = 3;`: spec
   0434's convention (no leading dot unless the name would be captured;
   short in its own package) is applied to input names only, before the
   rewrite (0434 N2).
2. **Option names are not rewritten at all:** `[(priv.tag) = "t"]` stays
   as it is, naming an extension the output declares as
   `google.protobuf.tag`.
3. **Two symbols rewritten onto one name pass silently.** `priv.Empty`
   becomes `google.protobuf.Empty`, as protobuf's own is. The output does
   not compile: `"google.protobuf.Empty" is already defined in file
   "google/protobuf/extra.proto"`. Spec 0437 catches two *files* rewritten
   onto one path; nothing catches two symbols in different files.

## Goals

- **G1.** One table, input name → output name, for every file, package
  and symbol, built once before rendering; rendering, the descriptors,
  the schema database and the scoring graphs look names up in it.
- **G2.** Names written as spec 0434 and 0439 write them, in the output
  namespace: a rewritten name shortened like any other.
- **G3.** A collision in the output namespace is never silent.

## Non-goals

- **N1.** Changing the rule language (`match`, `action`, `to`) or the
  rules' meaning: the table holds what the rules give today.
- **N2.** Merging two colliding symbols: as in 0437 N1, which one to keep
  is the user's to say, by pruning (`-p`).

## Specification

- **S1. The renaming.** Three functions of the input name, the only
  appliers of the rules: `output_symbol` (a symbol's full name, its
  leading dot kept as given), `output_package` (a package's, or a scope's:
  a package or a message's full name, matched as `.scope.`) and
  `canonize_dependency` (a file's path), in `mappings.py` and protoback's
  `variant.rs`. Under `--keep-descriptor-path`, symbols and packages keep
  their input names, and files all but the variant's descriptor.proto are
  rewritten, as before. The rules alone, unguarded, remain only for a
  predicate on what they give (`descriptor_options_fqns`). Functions
  rather than a stored table: they are pure, and a table would hold every
  symbol of the input for the few the output writes.
- **S2. Symbol collisions, before rendering**, after 0437's file check:
  among the summoned, present nodes of the graph (messages, enums,
  services, methods, fields and extensions), two of different input names
  with one output name stop the run, exit status 1, one line per name in
  node order, the outermost only (two colliding messages, not each of
  their fields), the files in input order: "'A' and 'B' both define
  'NAME' once rewritten; prune one (-p)". A node of a file 0437 dropped
  is not counted. A package is not a symbol: two files may share one. An
  enum value has no node, and is not checked: its enum's collision is.
- **S3. Names resolved in the output namespace.** Spec 0434's symbol
  table holds the output names of symbols and packages (files keep their
  input names, which are only identities there), and `type_ref` and
  `option_name` look up the output names of their target and scope, so a type reference, an extendee, a method's types
  and an option name are shortened, and dotted only when captured, as if
  the input had been written with the output names.
- **S4. The descriptors** (`-b`, the schema database): their names from
  the table, always full and dotted, as protoc writes them.
- **S5. Both tools alike**, the same text and the same stderr.

## Downstream-visible

Under a variant with `namespace_rewrites`, option names are rewritten
like type names (they were left as the input's), and both lose their
leading dot and are shortened (`Note` within
`google.protobuf`, `google.protobuf.Note` from elsewhere). Descriptors are
unchanged. An input with a symbol collision now stops (S2), where the
output used to be uncompilable.

## Tests

`reproto/src/reproto/tests/test_renaming.py`, on both tools:

1. The reproduction above: S2's message, the outermost name only, exit
   status 1, nothing written.
2. With `google/protobuf/empty.proto` pruned: a rewritten type and option
   name from another package (`google.protobuf.Note`,
   `(google.protobuf.tag)`), from a scope capturing `google`
   (`.google.protobuf.Note`), and from its own output package (`Empty`);
   the output compiles.
3. `--keep-descriptor-path`: the input's names, and no collision.
4. The harness, both modes, every corpus: identical between the tools.
