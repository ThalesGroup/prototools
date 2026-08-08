<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0261 — an export waits for the lines it names

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
        (N5, which recorded this and deferred it),
        docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the bake, `auto_folded`, `expand_auto_fold`),
        docs/specs/0249-a-large-document-answers-the-user-first.md (S8,
        the precedent: the bake is reordered around what the user is
        waiting for),
        docs/specs/0156-protolens-export-rename-and-export-format-chord.md (G6/G7,
        the `--descriptor-*` formats),
        docs/specs/0242-the-selection-is-a-span-of-characters.md (the fold-blind
        clipboard copy this spec leaves alone)

## Background

`:export` writes a truncated file, silently, and it has done so since
spec 0255. Spec 0257 made it reachable in the first ten seconds of every
session and recorded it as N5 rather than fixing it, because what an
export of a half-baked document should *mean* deserved deciding on its
own.

The mechanism, for the default `--prototext` format:
`push_subtree_lines` (`decode.rs:991`) returns early on a vacant slot,
which is the right thing for a scalar. But a *bracketed* node writes its
header, recurses into children, and then writes `derived_close`. A stop's
children are all vacant, so the recursion contributes nothing and the
node exports as

```
some_field {
}
```

— a message that reads as empty rather than as unread. Everything below
it is gone from the file with nothing to say so.

Two facts about the blast radius are worth having before choosing a fix,
because both narrow it and one widens it:

- It is the **interactive** `:export` only. The headless `export`
  subcommand is already safe: `main::startup_row_budget` returns `None`
  for it, so nothing is ever folded (spec 0257).
- **`--binary` cannot truncate.** `extract_bytes` slices the blob by
  `node.span.raw_range` (`extract.rs:179`) and never looks at a rendered
  line.
- **The clipboard cannot truncate either**, notwithstanding
  `decode.rs:956`'s doc comment, which says "export and clipboard".
  `copy_selection_to_clipboard` reads drawn rows, not `subtree_lines`.
  The comment is wrong and this spec fixes it.
- **`--descriptor-*` truncates too, by a different route N5 does not
  name.** `resolve_export_fields` (`tui/override_export.rs:38`) groups
  `self.first_child(idx)`, and `child_slots` (`tui/structure.rs:76`)
  reports `0..0` when the first child is not rendered. A stop therefore
  exports as a `FileDescriptorSet` describing a message with **no fields
  at all**. A fix that guards only `push_subtree_lines` leaves this one
  standing.

### What waiting costs

Measured on `googleapis.desc` (25.6 MB, 7 771 files), release,
`taskset -c 4-11`, opened with spec 0257's startup budget — 7 770 stops,
one per top-level `FileDescriptorProto`. Each of the 7 770 was drained on
its own and timed; that adds up to the whole document.

| expand budget | median child | p90 | p99 | worst child | whole document |
|---|---|---|---|---|---|
| unbounded | **168 µs** | 780 µs | 3.9 ms | **226 ms** | **3.56 s** / 7 770 expansions |
| `BAKE_ROW_BUDGET` (5 000) | 205 µs | 840 µs | 5.6 ms | 1.58 s | 6.65 s / 70 856 expansions |

The median export target on the most adversarial corpus available costs
under a fifth of a millisecond to make whole, and the worst single file
in it costs a quarter of a second. Only exporting the entire document is
expensive, and that writes 250 MB.

## Goals

- **G1.** An export either writes the node it names, whole, or writes
  nothing and says why. It never writes a document that looks complete
  and is not.
- **G2.** No perceptible wait for a target of ordinary size.

## Non-goals

- **N1. A deferred or background export.** "Queued; will write when the
  bake reaches it" is a new lifecycle for the user to track, and G2's
  measurement says there is nothing to track: the wait is invisible.
- **N2. The clipboard.** A copy takes what is drawn, and a folded stop is
  drawn as `{ ... }` — in spec 0260's violet, which says exactly that
  the region is unread. Spec 0242's copy is deliberately fold-blind in
  the other direction (it un-splices the summary and takes the row's own
  text); either way the user is copying rows they can see, and the screen
  is not lying to them.
- **N3. Search over unbaked regions.** A stop's body is not in
  `node_text`, so `/` cannot match inside it. That is real, it is a
  different problem, and unlike an exported file it corrects itself
  within seconds and leaves nothing behind.
- **N4. `--binary`.** It slices the blob and never reads a rendered
  line, so it has nothing to wait for. Making it wait anyway would turn
  a free root export into a 3.6-second one to no end.
- **N5. Refusing.** See "Alternatives considered".

## Specification

- **S1. `App::bake_subtree(&mut self, idx: usize) -> bool`**, in
  `tui/bake.rs` beside the rest of the bake. It descends `idx`'s subtree,
  expanding every node it meets that is in `auto_folded`, and returns
  whether the subtree is clear of stops when it is finished.

  A descent, not a single `expand_auto_fold`: an expansion runs
  `render_overrides` over what it revealed, and a nested `Any` under
  there registers stops of its own through `confirm_row_budget` (spec
  0255 S2). Those are below `idx` and the descent picks them up on its
  way down. The children to visit are therefore read *after* the
  expansion, not before it.

- **S2. Each node is expanded at most once.** A splice that refuses
  leaves the node in `auto_folded` on purpose (spec 0255), so a loop that
  retried until the set was empty would not terminate. One attempt per
  node; the return value carries the news.

- **S3. The budget is unbounded.** `BAKE_ROW_BUDGET` exists to keep a
  keystroke responsive between idle steps; an export has already
  committed to blocking, and paying the bounded render's re-emitted
  frontier buys nothing there. Measured above: 3.56 s against 6.65 s over
  the whole corpus, 226 ms against 1.58 s for its worst single file.

- **S4. `run_export` drains before it reads**, for `--prototext` and for
  both `--descriptor-*` formats. Not for `--binary` (N4).

- **S5. A subtree that will not clear refuses.** No file is written; the
  message says the export was refused and why. `expand_auto_fold`
  already writes the underlying splice failure to `self.message`, so the
  refusal appends to it rather than replacing it.

- **S6. `decode::subtree_lines`' doc comment stops claiming the
  clipboard as a caller.** It has two callers, both exports, and one of
  them is headless.

## Alternatives considered

### Refuse while anything is unbaked

The cheapest fix, and it makes a working feature intermittently
unavailable for a reason the user did not cause and cannot see the end
of — they would retry until it happened to succeed. The measurement kills
it: at a median of 168 µs there is nothing to refuse *for*.

### Guard inside `push_subtree_lines`

Turning the walker into something that returns an error would push the
decision out to every caller, which is the right shape if the answer were
"refuse". It is not, and it would still leave the `--descriptor-*` route
untouched, since that one never calls the walker.

### Drain at `BAKE_ROW_BUDGET`, reusing `bake_step`

Would have let the export share the queue instead of walking the
subtree. It drains the whole document rather than the named subtree,
which is the wrong unit — and the queue is document order, so an export
of the last node would wait for every node before it.

### Export what the wire says, ignoring the render

`--binary` already does exactly this, which is why it needs no fix.
Extending it to `--prototext` means rendering, which is the work the
drain does; there is no cheaper path to the same bytes.

## Test plan

1. `an_export_of_a_stop_is_whole` — export a bounded fixture's stop and
   compare against the same export from an unbounded fixture. Fails
   today with a `{`/`}` pair where the body should be.
2. `an_export_of_the_root_is_whole` — the same over the whole document,
   which is the case with the most stops under it.
3. `a_binary_export_does_not_wait` — S4/N4: `--binary` on a stop leaves
   `auto_folded` unchanged and still writes the right bytes.
4. `a_descriptor_export_of_a_stop_has_its_fields` — the second
   truncation, which N5 does not name; fails today with an empty message.
5. `a_refused_expansion_refuses_the_export` — S5: no file is written and
   the message says so.
6. `bake_subtree_expands_each_node_once` — S2, by construction: a node
   still in `auto_folded` after its attempt must not be retried.

## Measured outcome

The drain landed as a 20-line descent in `tui/bake.rs` and a five-line
guard at each of `run_export`'s two branches. Nothing else moved: 818
protolens tests and the full workspace green.

**Correctness, on the corpus.** `googleapis.desc` opened twice in one
process — once unbounded, once under spec 0257's startup budget (7 770
stops) — and 20 of the root's 7 771 children sampled across the file.
Every one of them, after `bake_subtree`, produced a `subtree_lines`
byte-identical to the unbounded render, from 2 558 B to 69 223 B. Their
drains cost **10 µs to 1.8 ms**, which is the Background's per-target
table confirmed on the path the fix actually uses.

**The five mutations, each killed:**

| mutation | killed by |
|---|---|
| the text branch does not drain | `an_export_of_a_stop_is_whole`, `an_export_of_the_root_is_whole`, `a_refused_expansion_refuses_the_export` |
| the descriptor branch does not drain | `a_descriptor_export_of_a_stop_has_its_fields` |
| `--binary` drains too | `a_binary_export_does_not_wait` |
| a refused expansion is reported as clear | `a_refused_expansion_refuses_the_export`, `bake_subtree_attempts_each_node_once` |
| the descent expands `idx` and stops | `an_export_of_the_root_is_whole`, `bake_subtree_attempts_each_node_once` |

S3's budget is deliberately **not** covered by a test. Bounded and
unbounded produce the same bytes — that is spec 0255's whole premise —
so the choice is a performance one and its evidence is the table in the
Background, kept next to the constant in `bake_subtree`'s doc comment.
