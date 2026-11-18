<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0445 — filter fixups: `move:` and `split:`

Status: draft
App: protoback (not reproto: N4)
Refs: docs/specs/0149-reproto-filter-config-file.md (the `-C` file this
        extends, and its strict loader); docs/specs/0438-warning-codes-and-fail-on.md
        (the codes added here); docs/specs/0431-protoback-plugins.md (the
        `--fdp-plugin` hook this replaces for moves)

## Background

A corpus collected from binaries of different ages holds several
generations of the same files: moved, renamed, split. A filter (`-C`)
can keep only the newer of two moved files, but the old file's importers,
mostly from older binaries, then lose their import (W23), the types they
take from it no longer resolve (W4), and the pool refuses them (W19, W6).

A downstream user works around it with a generated `--fdp-plugin` that
rewrites each importer's `dependency` (keeping `public_dependency` and
`weak_dependency` consistent): on its corpus, 23 re-pointings remove every
W23 and their knock-on W4, W19 and W6. It works because the plugin runs
before import discovery (`patched_imports`, `protoback/src/load.rs`). But
a plugin is an adapter process, opaque to review, and checks nothing.

A split cannot be worked around that way: part of a file moved to a new
file in a newer binary, and the rest is in no newer binary, so neither
version can replace the other. Only the tool, holding both files, can
rebuild what the source tree most likely has: the old file without the
moved symbols, importing the new one.

## Goals

- **G1.** Declare a move, old file → new file, in the `-C` file; importers
  of the old file re-pointed to the new one before import discovery.
- **G2.** Declare a split, old file → new files; the old file reduced to
  the symbols the new files lack, importers re-pointed to the files whose
  symbols they use.
- **G3.** A move or split that does nothing, or does something wrong, is
  warned under its own code, so `--fail-on` catches a stale or wrong fixup.
- **G4.** One `-C` file serves runs with and without a renaming variant.
- **G5.** The same directives on the command line, merged with the file.

## Non-goals

- **N1.** Reconciling two files that only become one under a variant's
  renaming (a file in a renamed namespace and its counterpart in the
  target namespace). Re-pointing fixes imports, not type names: importers'
  type references are resolved before the renaming applies. That stays
  spec 0440's collision error; merging such files when they define the
  same symbols once renamed is a separate spec.
- **N2.** Splits below the top level: only top-level messages, enums,
  extensions and services move. A nested type moving out of its parent is
  a change to the parent, not a split.
- **N3.** Inferring moves or splits. The tool applies what it is told and
  checks it (S7); proposing fixups is the downstream tool's job.
- **N4.** reproto. A new feature, not a fix: reproto's strict `-C` loader
  keeps refusing the new keys, and the directives are tested on protoback
  alone, the harness having no reference for them.
- **N5.** A `path:` form of `move:`, for two physical files under one
  import name: choosing between those is what `path:` prunes do. A move is
  about import names.

## Specification

### The file

- **S1.** The `-C` file gains three keys besides `seed:` and `prune:`, and
  stays strict (unknown keys refused):

  ```yaml
  prune:
    - path:old/a~2.pb
  move:
    - old/score.proto: new/score.proto
  split:
    - common/data.proto:
        - common/power_options.proto
  variants:
    proto2:              # a variant's `name:`
      prune:
        - path:google/protobuf/descriptor*.pb
  ```

  `move:` and `split:` are lists of one-key mappings, file names as
  imports write them (`.proto`), not physical paths.
- **S2.** `variants: NAME:` holds `seed:`, `prune:`, `move:` and `split:`
  applying only when the variant in use has that `name:`, appended to the
  top-level lists. A name no variant has is W26 (S8): most likely a typo.
- **S3.** `--move OLD=NEW` and `--split OLD=NEW[,NEW…]` on the command
  line, appended to the file's, as `-s`/`-p` are today.

### Moves

- **S4.** Applied to every file read, at import discovery, where the
  plugin's patched imports are read today (so a move also decides what is
  loaded): each `dependency` entry `OLD` becomes `NEW`; a duplicate entry
  this creates is removed, and `public_dependency` and `weak_dependency`
  renumbered (a public import of `OLD` stays public). `OLD` is then pruned
  as `file:OLD` is: a move replaces `OLD` entirely; when `OLD` has symbols
  `NEW` lacks, a split is the directive, and W24 says so of the importers
  using them. This is what a downstream `--fdp-plugin` rewriting
  `dependency` does today.
- **S5.** A move chain (`A: B`, `B: C`) resolves to its end; a cycle is a
  usage error before loading.

### Splits

- **S6.** `split: OLD: [NEW…]`, applied after loading, before phase 2:
  - the top-level symbols the `NEW` files define are removed from `OLD`;
    what remains is kept, and imports each `NEW` file the remainder
    references;
  - each importer of `OLD` imports, among `OLD` and the `NEW` files, those
    whose symbols it references (a fan-out), public imports staying
    public;
  - references are read by name, before the pool: every type reference
    (`type_name`, `extendee`, a method's `input_type` and `output_type`)
    in the remainder and in each importer, looked up against the `NEW`
    files' symbols. An FDP's references are fully qualified (protoc writes
    them so). One that is not (no leading `.`) stops the run with an
    error naming the file, the reference and the split, and saying how to
    go on: prune that file (`-p file:…`), or drop the split. Resolving it
    would take the pool's scope rules, which this does not follow; and
    such a name means input from something other than protoc, worth a
    look before going on;
  - a `NEW` file importing `OLD` (directly or not) is a loop: the split is
    not applied, and W26 says so (S8). Not an error: on a corpus of
    thousands of files, one wrong directive should not cost the whole run;
    `--fail-on W26` makes it fatal where wanted;
  - an empty remainder makes it a move of `OLD` onto the `NEW` files.

### Checks and codes

- **S7.** W24, a fixup that breaks a reference: after a move or split, a
  re-pointed importer references a symbol none of the files it now imports
  defines. Names the importer, the symbol and the directive.
- **S8.** Two codes for a fixup that does not apply, as they mean
  different things to whoever maintains the fixups:
  - **W25, idle:** a directive the input no longer needs, expected as a
    corpus moves on, and a cue to drop it: an `OLD` no file imports and no
    input holds; a split none of whose `NEW` symbols `OLD` defines. Like
    W11 for a prune pattern matching nothing.
  - **W26, broken:** a directive that is wrong, to be fixed, and what
    `--fail-on` is for: a `NEW` the input lacks; a split not applied for a
    loop (S6); a variant section matching no variant (S2).

## Alternatives considered

- **Keep the plugin.** It already works for moves; but it cannot split,
  checks nothing (G3), and costs an adapter process per file.
- **Moves as prunes plus W23 accepted.** The importers' types then do not
  resolve: W4 and the pool's refusals, the problem itself.
- **A reference not fully qualified skips its split, with a warning**
  (asked for downstream, so that one odd file does not cost a run over
  thousands): not the default, nor an opt-in flag. No such reference has
  been seen, and protoc writes none; the error names the file and the way
  on (S6), which is that outcome, chosen by whoever reads it. The agreed
  fallback, should such input ever be met in practice.

## Test plan

1. `move_repoints_importers` — `b` imports `old`; `move: old: new`: `b`
   imports `new`, `old` pruned, no W23, W4 or W19.
2. `move_keeps_public_and_weak` — a public and a weak import of `old`
   stay so after the move; a duplicate entry removed, indices renumbered.
3. `split_reduces_and_fans_out` — `old` defines `X` and `Y`, `new`
   defines `Y`; importers using `X`, `Y` and both import what they use.
4. `split_loop_is_refused` — `new` importing `old`: W26, `old` untouched.
5. `wrong_move_is_w24`, `idle_fixup_is_w25`, `broken_fixup_is_w26`,
   `variant_section_applies`.

## Measured outcome

(At implementation.)
