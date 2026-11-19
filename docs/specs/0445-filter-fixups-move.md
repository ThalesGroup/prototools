<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0445 — filter fixups: `move:`

Status: implemented
Implemented in: 2026-11-18
App: protoback (not reproto: N3)
Refs: docs/specs/0149-reproto-filter-config-file.md (the `-C` file this
        extends, and its strict loader); docs/specs/0438-warning-codes-and-fail-on.md
        (the codes added here); docs/specs/0431-protoback-plugins.md (the
        `--fdp-plugin` hook this replaces for moves)

## Background

A corpus collected from binaries of different ages holds several
generations of the same files: moved, renamed, split. A filter (`-C`)
can keep only one of two versions, but the other's importers, mostly from
older binaries, then lose their import (W23), the types they take from it
no longer resolve (W4), and the pool refuses them (W19, W6).

A downstream user works around it with a generated `--fdp-plugin` that
rewrites each importer's `dependency` (keeping `public_dependency` and
`weak_dependency` consistent): on its corpus, 23 re-pointings remove every
W23 and their knock-on W4, W19 and W6. It works because the plugin runs
before import discovery (`patched_imports`, `protoback/src/load.rs`). But
a plugin is an adapter process, opaque to review, and checks nothing.

**The principle:** repair what is obvious, do not bend over backwards to
repair what is complex, and push the rest to the user, stated precisely
enough to act on. A move rewrites imports, which is obvious; rebuilding a
file's contents to reconcile two versions is not (N1). What a move leaves
inconsistent is said (W24), so that the input gets fixed where it comes
from.

**And prefer moving forward**, onto the newer file. One corpus cannot tell
a file split in two, one half never collected, from a file whose other
definitions were deleted: an old file of 79 definitions and a newer one
holding 5 of them fit both histories. Moving forward is right for the
second, and for the first leaves W24s naming exactly the importers to
collect again: either way the problems converge on newer inputs. Moving
backward, onto the older file, is the obvious repair only when it loses
nothing: the older file defines everything the newer one does, and those
shared definitions are identical. protoback sees no build dates and
compares no versions: the caller chooses the direction (the downstream
user's generator applies exactly that rule), and protoback rewrites in
whichever direction it is told.

## Goals

- **G1.** Declare in the `-C` file that a file has moved, to one file or
  several; its importers re-pointed to the target(s) before import
  discovery, the file itself pruned.
- **G2.** A move that does nothing, or something wrong, warned under its
  own code, so that `--fail-on` catches a stale or wrong fixup.
- **G3.** Whatever a move leaves inconsistent stated precisely enough to
  act on: which importer lacks which definition, because of which move.
- **G4.** One `-C` file serving runs with and without a renaming variant.
- **G5.** The same directive on the command line, merged with the file.

## Non-goals

- **N1.** Rebuilding a file. A move rewrites import statements only: no
  symbol is removed from a file, no file is made. When the targets lack
  some of what the moved file defined, the input is left inconsistent and
  some definitions are lost; the run says which (S6), and fixing the input
  is the user's job, or a downstream tool's that knows where the inputs
  come from. Rebuilding the file a split most likely left (the old file
  less what moved, importing the new) was drafted and dropped: it assumes
  one of two histories the corpus cannot tell apart (Background), and
  takes symbol surgery and a resolution of references by name that a pool
  already does.
- **N2.** Reconciling two files that only become one under a variant's
  renaming (a file in a renamed namespace and its counterpart in the
  target namespace). Re-pointing fixes imports, not type names. That
  stays spec 0440's collision error.
- **N3.** reproto. A new feature, not a fix: reproto's strict `-C` loader
  keeps refusing the new keys, and the directive is tested on protoback
  alone, the harness having no reference for it.
- **N4.** Inferring moves, or advising on them. The tool applies what it
  is told and states the facts (S6, S7, spec 0446).
- **N5.** A `path:` form of `move:`, for two physical files under one
  import name: choosing between those is what `path:` prunes do.

## Specification

### The file

- **S1.** The `-C` file gains two keys besides `seed:` and `prune:`, and
  stays strict (unknown keys refused):

  ```yaml
  prune:
    - path:old/a~2.pb
  move:
    - old/score.proto: new/score.proto
    - common/data.proto: [common/power_options.proto, common/data_v2.proto]
  variants:
    proto2:              # a variant's `name:`
      prune:
        - path:google/protobuf/descriptor*.pb
  ```

  `move:` is a list of one-key mappings, `OLD: TARGET` or
  `OLD: [TARGET, …]`, file names as imports write them (`.proto`), not
  physical paths.
- **S2.** (Gone with variants, spec 0447 S8: `variants:` is now an
  unknown key, refused.) `variants: NAME:` held `seed:`, `prune:` and `move:`, applying
  only when the variant in use has that `name:`, appended to the
  top-level lists. A section for another variant is not warned: a file
  serving several variants (G4) has one on every run. The report lists
  it (spec 0446 S5). A typo in a variant's name goes unseen, the price.
- **S3.** `--move OLD=TARGET[,TARGET…]` on the command line, repeatable,
  before the file's, as `-s`/`-p` are merged today (which decides which
  of an `OLD` given twice applies, S7).

### The rewrite

- **S4.** Applied to every file read, at import discovery, where the
  plugin's patched imports are read today (so a move also decides what is
  loaded), and again to the FDP itself before phase 2 processes it:
  - each `dependency` entry `OLD` is replaced by the targets, in their
    order, an entry this duplicates removed;
  - `public_dependency` and `weak_dependency` renumbered: a public import
    of `OLD` gives public imports of the targets, a weak one weak ones;
  - `option_dependency` (an edition 2024 `import option`) rewritten
    alike;
  - a target is loaded only because something now imports it.
- **S5.** `OLD` is pruned, as `file:OLD` is, however it is reached
  (through an import, or by a directory walk).

### Checks and codes

- **S6.** W24, a definition lost to a move: an importer re-pointed by a
  move uses a definition that none of the files it can see defines. What
  it can see is what protoc lets it: itself, its imports, and what they
  pass on through `import public`, followed. Uses checked:
  - types: a field's `type_name`, an extension's `extendee`, a method's
    `input_type` and `output_type`;
  - option extensions: each field set in an options message of the
    importer (file, message, field, enum, value, service, method, oneof,
    extension range) whose number is no field of that options type in the
    variant's descriptor.proto, and which no visible file declares. The
    file that declared it is `OLD`, pruned, so the set of declarations
    cannot say; the options type's own fields can.

  W24 names the importer, the definition (a type's full name, or an
  option's options type and number), and the move. It does not name the
  file that used to define it: the move names `OLD`, and the user knows
  where to look.
- **S7.** Two codes for a move that does not apply as written, as they
  mean different things to whoever maintains the fixups:
  - **W25, idle:** a directive the input no longer needs, expected as a
    corpus moves on, and a cue to drop it: an `OLD` that nothing imports
    and no input holds. An `OLD` an input holds is pruned (S5): the
    directive did something.
  - **W26, broken:** a directive that is wrong, to be fixed, and what
    `--fail-on` is for. The directive is not applied:
    - a target that is itself moved (a chain, `A: B` with `B: C`) or in a
      cycle: neither resolved;
    - a target a `prune:` pattern matches;
    - an `OLD` given twice;
    - a target the input lacks: known only once phase 1 has loaded the
      input, after the imports were rewritten. Here the directive stays
      applied, W26 saying the target is missing (the importers then also
      get W1 and W5 for it): undoing a load is not worth it for a
      directive that is wrong anyway.

## Alternatives considered

- **Keep the plugin.** It already works; but it checks nothing (G2),
  states nothing (G3), and costs an adapter process per file.
- **Moves as prunes plus W23 accepted.** The importers' types then do not
  resolve: W4 and the pool's refusals, the problem itself.
- **A separate `split:` directive, rebuilding `OLD`** (N1). Dropped.
- **Chains resolved to their end.** No downstream generator writes one,
  so one is a mistake to report (W26), not to complete.

## Test plan

1. `move_repoints_importers` — `b` imports `old`; `move: old: new`: `b`
   imports `new`, `old` pruned, no W23, W4 or W19.
2. `move_to_several_targets` — `old: [n1, n2]`: `b` imports both.
3. `move_keeps_public_and_weak` — a public and a weak import of `old` give
   public and weak imports of the targets; duplicates removed, indices
   renumbered.
4. `move_prunes_walked_old` — `old` loaded by the directory walk, nothing
   importing it: pruned, no W25.
5. `lost_type_is_w24`, `lost_option_is_w24`,
   `type_through_a_public_import_is_not_w24`.
6. `idle_move_is_w25`; `broken_move_is_w26` for each case of S7.
7. `variant_section_applies`, `cli_move`.

## Measured outcome

`protoback/src/moves.rs`; tests in `protoback/tests/moves.rs` (12, one
per item of the test plan, `broken_move_is_w26` covering S7's cases but
the missing target, which `a_missing_target_is_w26` covers) and in the
module (6: the rewrite of public and weak imports, the static W26 checks,
`option_numbers` at each level, `--move`'s value). No change without a
move: harness against reproto, 1,668 cases and 1,654 graph dumps
identical; `--move` listed among protoback's own options
(`scripts/protoback-harness`, `HELP_OWN`).
