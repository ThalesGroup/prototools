<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0202 — an override is refused rather than fatal

Status: implemented
Implemented in: 2026-07-28
App: protolens
Refs: docs/specs/0118-protolens-recursive-override-rendering.md
        (§2.1/§4, the splice and `rendered_as`),
      docs/specs/0160-protolens-render-overrides-batch-scaling.md
        (G1/G2, the batch),
      docs/specs/0183-prune-the-override-walk.md (S2, the descent
        marks),
      docs/specs/0203-the-override-arena-is-compacted.md (the fix this
        guard stands in for)

## Background

Reported as a reproducible crash:

```
target/release/protolens \
  --descriptor-set .../googleapis.desc .../googleapis.desc
```

then `Down`, then three times `t`, `Enter`, `o`, `d`, `Esc`. The third
cycle is killed:

```
Killed
real 3m8.693s
```

It is an OOM kill. Driven through a pty with per-keystroke RSS
sampling, and instrumented with per-splice counters under
`PROTOLENS_TRACE`:

| after | RSS | `tree.len()` | reachable from root |
|---|---|---|---|
| startup | 2045 MiB | 4 501 014 | 4 501 014 |
| `Enter` (apply) | 3889 MiB | 9 000 349 | 4 499 336 |
| `d` (remove) | 5256 MiB | 13 499 684 | 4 499 336 |
| `Esc`, `t`, `Enter` | > 6 GiB | — | — |

`lines.len()` stays flat at ~5.28 M throughout: the text buffer is
patched in place and is not the problem.

### It is not over-materialization

The obvious suspicion — that the walk descends too far — is wrong, and
was measured to be wrong. One batch:

```
visits=7772  splices=7771  nodes=4499335
```

against 7772 descent marks. Spec 0183's pruning gate is doing exactly
its job: 7 772 visits into a 4.5 M-node arena.

The override's origin is `path:field` at the *root* — field 1 (`file`)
of `FileDescriptorSet` — so it targets all 7 771 top-level
`FileDescriptorProto` records. A splice re-decodes its target's whole
subtree, which it must: a retype reinterprets the entire payload. 7 771
subtrees is the whole document. The largest single splices are `/5767`
at 260 944 nodes and `/5766` at 234 781.

So the work per batch is correct and proportional to the override's
reach. The live set never grows.

### The defect is that nothing is ever freed

`App::tree` is an append-only arena; `override_apply.rs`'s own doc
comments say so ("the arena also still holds nodes superseded by
earlier splices"). Each batch appends a full document's worth of nodes
at ~305 B/node and drops the previous copy on the floor — after the
second batch, 67% of the arena is unreachable. `Vec` doubling
amplifies it: `capacity()` reached 18 004 056 for 13 499 684 entries,
~5.5 GiB reserved.

It is a real retain, not allocator fragmentation: re-running with
`MALLOC_ARENA_MAX=2`, `MALLOC_TRIM_THRESHOLD_=131072` and
`MALLOC_MMAP_THRESHOLD_=131072` reproduced the identical curve to
within 20 MiB.

The fix is reclamation, and that is spec 0203. This spec is the
stop-gap that ships first, because the crash is reaching users now and
0203 is a much larger change.

## Goals

- **G1.** An override that cannot safely run is refused, with an
  explanation, instead of taking the process down.
- **G2.** A refused batch leaves the document byte-identical and the
  arena unchanged.
- **G3.** The guard cannot misfire on a freshly opened document.

## Non-goals

- **N1.** Reclaiming any memory. Nothing here makes an override that
  used to fail succeed; the arena still only grows. Spec 0203.
- **N2.** Predicting a batch's cost. See S2 for why the attempt was
  made and abandoned.
- **N3.** Reducing the per-node footprint (`Option<usize>` links,
  `Option<String>` type names). A separate, later change, and one that
  must not land before 0203 — narrowing an index to `u32` is only
  sound once the arena is bounded.
- **N4.** The ~4.8 s stall a large batch causes. Separate work.
- **N5.** Making the refusal recoverable in-session. The advice is to
  restart, because until 0203 there is nothing else to give.

## Specification

### S1. The budget

`available_memory_bytes` reads `MemAvailable` from `/proc/meminfo`.
`None` — unreadable, or `PROTOLENS_NO_MEMORY_GUARD` set — disables the
guard entirely. That is the right failure mode: an inability to measure
must never block a user from an override.

The budget is half of `MemAvailable`, not all of it, because the arena
is not the only thing a batch allocates (the line buffer, the patches,
and the decode's own working set grow too), and because leaving the
machine with no headroom invites the OOM killer to take some *other*
process instead.

### S2. The rule uses only exact quantities

`override_batch_refusal(available)` refuses when

```
tree.len() * (size_of::<TreeNode>() + 64) > available / 2
```

The `+ 64` covers the `Option<String>` type name each node carries;
the constant is calibrated against the measured ~305 B/node for a
~250 B struct.

What this checks is headroom for one more batch: the worst case is a
document-wide override, which appends about as many nodes as the arena
already holds.

It deliberately does **not** predict the batch. An estimate built from
spec 0183's descent marks was written first and discarded, and the
reason is worth recording. `descend` marks every node whose rendering
*could* change plus every ancestor of one, so a marked node with no
marked child is a target and summing those targets' extents looks like
the batch's cost. At startup the marks are the root alone — so the
estimate charged the batch the entire document (5 278 324 lines) for a
pass that in fact splices nothing. Predicting accurately means running
`resettle_node`'s own comparison for every marked node, which is half
the batch; over-predicting means refusing harmless work, at startup
first of all. G3 is not satisfiable by a predictor built this way.

The cost of not predicting is that once the arena is large, *every*
override is refused, including a one-node one that would have been
harmless. That is the trade: a guard that cannot misfire on a fresh
document, in exchange for one that is blunt on an exhausted one. It
disappears with 0203.

### S3. Refusing is safe by placement

The check runs in `render_overrides` at batch depth 0, before
`override_batch_depth` is incremented and before
`render_overrides_inner` is called, so a refusal cannot leave a
half-applied batch. It sets `self.message` and returns.

`compute_descend_marks` still runs first. It only extends `descend`,
and over-marking is safe by construction (spec 0183) — a refused batch
therefore leaves marks that the next batch will honor.

The override entry stays active and visible in the management pane
while the document keeps its previous rendering. That inconsistency is
deliberate: it is visible, it is recoverable by deactivating the entry,
and it is strictly better than losing the session.

## Test plan

1. `the_memory_guard_refuses_a_batch_with_no_headroom_left` — an arena
   occupying exactly half of what is free is allowed; one byte past it
   is refused; the message names both the problem and the remedy.
2. `a_refused_batch_leaves_the_document_untouched` — `lines` and
   `tree.len()` are unchanged across a refused `render_overrides`,
   `message` explains why, and the same batch then succeeds once
   headroom is restored (so the guard refuses rather than corrupts).
3. `App::memory_available` (a `#[cfg(test)]` field, following the
   `unpruned_walk`/`verify_repair` precedent) stands in for
   `MemAvailable`, so the suite does not depend on how much memory the
   machine running it happens to have free.

## Measured outcome

The reported reproduction, run to completion through the same pty
driver — `Down`, then three times `t`, `Enter`, `o`, `d`, `Esc`:

| after | RSS before | RSS after |
|---|---|---|
| startup | 2045 MiB | 2045 MiB |
| cycle 1 `Enter` (apply) | 3889 MiB | 3889 MiB |
| cycle 1 `d` (remove) | 5256 MiB | 3889 MiB, refused |
| cycles 2 and 3 | > 6 GiB, killed | 3889 MiB, refused |

The process survives all three cycles and exits normally; peak RSS is
3989 MiB. The first override still applies — the guard only starts
refusing once the arena is past half of what is free, which on this
machine (`MemAvailable` 4.4 GiB at that point) happened at the second
batch, with the arena at 2.7 GiB.

The refusal reaches the status line intact:

```
override skipped: protolens is already holding 2.7 GiB of decoded
nodes and another override could need that much again, but only
4.4 GiB is free — restart protolens to reclaim what earlier
overrides are still holding
```

The document keeps its previous rendering across every refused batch,
and `tree.len()` stays at 9 000 349 instead of climbing to 13 499 684.

The bluntness predicted in S2 is confirmed: after the first batch on
this document, every subsequent override is refused, including ones
that would have been cheap. That is the cost this spec accepts, and
0203 removes it.
