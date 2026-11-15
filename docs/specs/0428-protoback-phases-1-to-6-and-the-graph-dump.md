<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0428 — protoback, phases 1 to 6, and the graph dump

Status: draft
App: protoback, reproto
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 4 of its
      work plan; S7, the crate's structure; S11, the harness; S14, the
      command line; S15, side by side; S17, shared state then per-file
      work); docs/specs/0427-text-input-parsed-with-the-inputs-schema.md
      (text input, which protoback reads with `input::read_input`)

## Background

Step 4 of spec 0424 brings up protoback's first half: loading, the pool,
and phases 1 to 6, the ones that decide what reproto will render, before
any rendering exists. What these phases decide is invisible in reproto's
output until rendering prints it, and a difference found in a `.proto`
file in step 5 may have its cause anywhere before. A dump of everything
phases 1 to 6 decided, written by both tools and compared byte for byte,
finds a difference where it is made.

reproto's node order is reproducible: on googleapis (246,171 nodes) and
on the harness's custom corpus (969), `ctx.nodes` after phase 6 holds the
same nodes, flags included, in the same order under two
`PYTHONHASHSEED`s (2026-11-15). The dump can compare that order, not a
sorted copy: rendering will walk nodes in it (spec 0424 S17, S18).

## Goals

- **G1.** A `--dump-graph FILE` option, hidden, in reproto and protoback,
  that writes the state phases 1 to 6 leave, as text, and stops.
- **G2.** The dump as detailed as the state is: a difference in any
  decision phases 1 to 6 make shows in it.
- **G3.** protoback's phases 1 to 6, for which the two dumps are
  identical on every corpus of the harness.
- **G4.** Those phases timed, against reproto's.

## Non-goals

- **N1.** Rendering, scoring graphs, the schema database, plugins,
  `instantiate`: steps 5 to 8. protoback refuses their options (S9).
- **N2.** Warnings as reproto prints them, wording, grouping and
  `--detailed-warnings`: step 9. Step 4 compares what is warned about
  (S6), not how it is shown.
- **N3.** Threads: phases 1 to 6 are shared state (spec 0424 S17), and
  run on one thread.
- **N4.** `seeder`, which phase 5 writes and nothing reads (a debug
  printout that followed it was removed in November 2025); its value
  depends on set iteration order. protoback does not have it, and the
  dump leaves it out.

## Specification

### The dump

- **S1. Its form.** UTF-8 text, one record per line, in sections, each
  opened by a `[name]` line, in the order below. Fields are separated by
  ` | `; a list is written `[a, b, …]`, sets sorted, sequences in their
  own order. A name is written as reproto holds it (an FQDN as
  `kind:name`, a file by its name). Written after phase 6, before any
  output; the run then stops with status 0, after the warnings' flush,
  as `--dump-resolved-features` does.
- **S2. `[files]`**, the topology's files, in `topo.files` order:
  - name; `ref` (named by an import, never loaded) or `file`;
  - for a loaded file: its root and root-relative path, `text` or
    `binary`, `seed` or not, `pruned` or not;
  - its imports as the topology has them (sorted);
  - what phase 2 stripped from it: imports (in order), public imports
    (in order), and fields and methods whose types did not resolve,
    each with its number, name, type and label, by message or service
    (sorted by FQDN, in their order within one).
- **S3. `[pool]`**, the files added to the pool, in the order phase 2
  added them: name, size and SHA-256 of the file's serialized
  `FileDescriptorProto` as added, after phase 2's mending and
  `go_package` patching. The bytes are compared, not a summary of them:
  two tools agreeing on every hash read and mended every file alike.
- **S4. `[context]`**: the pruned file names, the pruned FQDNs, the
  FQDNs seeded by path (each sorted); the edition defaults table, as
  `edition | feature | value` lines, sorted; and, with `--schema-db-out`,
  the nodes phase 6's third sub-pass adds for the schema database
  (`schema_db_extra_nodes`, in order).
- **S5. `[nodes]`**, every node, in `ctx.nodes` order: its kind (the
  node class), FQDN, `present` or `stub` (an unresolved reference), the
  flags pruned, reachable and summoned, its parent's FQDN or `-`, and its
  `contains` and `targets` (sorted).
- **S6. `[seeds]`**, the nodes phase 5 started from (sorted), then
  **`[warnings]`**, everything phases 1 to 6 warned about, in the order
  warned: the collector's calls (`w1` to `w7`, with their arguments), and
  each `cli_warning`, `cli_info` and `cli_attention` message, as
  formatted. One exception, by kind and subject only: a file the pool
  refused (`Could not add descriptor from 'F' to pool: …`) is written
  `pool-refused | F`, since the reason is Python's pool's text, which upb
  does not produce (S8).

### protoback

- **S7. The crate.** `protoback/`, in the workspace, one binary
  `protoback`, structured as reproto (spec 0424 S7): `load` (phase 1,
  the input's files and their splitting), `pool` (phase 2), `graph` (the
  node arena, phase 3), `prune`, `reach`, `summon` (phases 4 to 6),
  `dump` (S1–S6), `variant` (the variant YAML and its resources), `cli`.
  - Text input through `prototext_upb::input::read_input` (spec 0427):
    no tree-sitter.
  - A directory is walked in sorted path order, component by component,
    as reproto walks it since 2026-11-15: before, reproto took the
    filesystem's listing order (`Path.rglob`), on which W7's "first
    occurrence" and the topology's order depended
    (`test_a_directory_is_read_in_sorted_order`).
  - The built-in variant's resources (its `.pb` fallbacks) are reproto's,
    `reproto/src/reproto/variants/`, embedded at build time: protoback is
    not published yet (spec 0424 S15), and the two tools must load the
    same fallbacks.
- **S8. The pool, upb's.** Files are added to a upb pool as phase 2 adds
  them to Python's, imports first, with phase 2's mending before each.
  Where upb and Python's pool judge a file differently (one accepts,
  the other refuses), the dump shows it in `[pool]` and `[warnings]`;
  each such case is resolved toward reproto's outcome and listed under
  "Measured outcome", with its cause.
- **S9. The command line**, clap, under reproto's spellings, deprecated
  ones included. Every option phases 1 to 6 read: the inputs,
  `-I`/`--desc-root` (and `--pb-path`); `-s`/`--seed` and `-p`/`--prune`,
  FQDNs and path patterns; `-C`/`--filter-config`; `--proto-variant`,
  `--use-variant` and `REPROTO_VARIANT`; `--keep-descriptor-path`;
  `--keep-duplicates`; `--go-root` (phase 2's `go_package` patching);
  `--schema-db-out` (phase 6's third sub-pass); `--quiet`, `--debug`,
  `--debug-fqdn` (phase 5's report); and the hidden `--dump-graph`. Every
  other option of reproto's is parsed, with its arguments: under
  `--dump-graph`, it acts after phase 6 and is ignored, so that the
  harness's flag sets run unchanged; without it, protoback refuses it
  before phase 1, naming the step that brings it.
- **S10. The harness**, `scripts/protoback-harness run --dump-graph`:
  each case run with `--dump-graph graph.txt` added, the dumps compared
  as any output is.

## Alternatives considered

### A sorted dump

Insensitive to the order nodes are made in, which rendering depends on;
reproto's order is reproducible (Background), so there is nothing to
gain by losing it.

### Comparing in-memory state through a Python binding of protoback

Ties the comparison to one process and to Python's object model; a text
file per run compares with `diff`, and its first differing line names the
first differing decision.

### `seeder` in the dump

Written, never read, and dependent on set iteration order (N4).

## Test plan

1. reproto: `--dump-graph` on the fixtures of its suite: every section
   present; two runs under different `PYTHONHASHSEED`s identical; the
   run stops after phase 6 (no `--proto-out` written).
2. protoback against reproto, `scripts/protoback-harness run
   --dump-graph` on every corpus: identical dumps.
3. The same on reproto's CLI test inputs that reach phases 1 to 6 with
   the options of S9: pruning, seeding by FQDN and by path, filter
   configs, duplicates, missing files, cycles, a self-dependency, text
   input.
4. Phases 1 to 6 timed on googleapis, protoback against reproto
   (`scripts/reproto-phase-times` for reproto's).

## Measured outcome

Filled in at implementation.
