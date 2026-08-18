<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0320 — quitting is not a computation

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0152-… (the worker is joined before the terminal is
        restored), docs/specs/0222-… (each arena slot owns its lines —
        the allocations this is about), docs/specs/0256-… (`free` is
        unmeasurable on glibc, seen from the other side),
        docs/specs/0319-… (whose N4 guessed at this and guessed wrong)

## Background

Field report: `:q` on `googleapis.desc` takes a full second.

Measured over a pty at 50×200, keystroke to process exit:

| state | quit |
|---|---|
| 20 KB `boblog`, settled | 22 ms |
| googleapis, 4 s settle, idle | 166 ms |
| googleapis, 4 s settle, after a 4 s `PageDown` burst | 897 ms |
| googleapis, 45 s settle, idle | **1045 ms** |
| googleapis, 45 s settle, after a burst | 1134 ms |

The cost tracks how much has been **baked**, not what is running. The
45-second idle row is the one that matters: nothing is in flight, there
is no worker to cancel, and it is still a second.

Splitting the interval on `\x1b[?1049l`, the escape that leaves the
alternate screen:

```
keypress -> terminal given back     17 ms
         -> process gone          1075 ms   (1058 ms after handback)
```

`perf` over that final 1.1 s — 98.86% protolens, single-threaded:

```
34.95%  malloc_consolidate
21.33%  unlink_chunk
17.05%  _int_free
 9.52%  cfree
 4.48%  _int_free_create_chunk
 3.71%  _int_free_merge_chunk
```

91% in glibc's free, under `Arc::drop_slow`. The TUI is already gone.
The process then spends a second returning millions of small
allocations — spec 0222's per-slot line strings, chiefly — to an
allocator whose entire address space the kernel is about to reclaim in
one act. Nothing is computed and no result is kept.

`app` is a local of `main`; its drop at the end of the interactive arm
is the whole of it.

**This refutes spec 0319 N4**, which recorded the second as a worker
stuck in `override_pane::inferred_score` with no cancel flag. Spec 0152
joins every worker *before* `restore_terminal`, so a worker that refused
to stop would have to appear inside the handback. Handback is 17 ms even
when `:q` is sent in the middle of a four-second `PageDown` burst. The
missing cancel flag is real and is still worth fixing on its own terms;
it is not this.

## Goals

- **G1.** `:q` costs the same on a 25 MB document as on a 20 KB one.

## Non-goals

- **N1.** Allocating less, or allocating the baked text into an arena
  that frees in one call. That is a large change to spec 0222's layout
  to buy back time on a path that is about to end anyway. If the
  allocation profile is ever revisited it should be for the *interactive*
  cost, not this one.
- **N2.** `override_pane::inferred_score`'s missing cancel flag (spec
  0319 N4). Measured above at under 17 ms and so not the reported fault,
  but still a walk that cannot be interrupted. Left standing, and no
  longer described as the cause of a slow `:q`.
- **N3.** The other exit paths. `export` and the batch subcommands write
  a file and are not reported as slow; leaving them on the ordinary drop
  keeps this change to the one path that was measured.

## Specification

- **S1.** The interactive arm of `main` does not drop its `App`. After
  `tui::run` returns `Ok`, the value is released with
  `std::mem::forget`.

  Every destructor in that chain is either already run or has nothing to
  do at exit, and this list is the justification — it must be rechecked
  before anything with a side-effecting `Drop` is added to `App`:

  - `HeatWorkerHandle` — already `take()`n and shut down inside
    `tui::run`, which is what stops a worker writing to a restored
    terminal. Forgetting the `App` happens strictly after.
  - `Blob`'s `Mapping` — a `munmap` the kernel performs regardless.
  - `SegmentScan` — aborts and joins a scan thread so that a superseded
    sweep cannot outlive the data it borrows. There is no next
    `Arc::make_mut` after this point.
  - The two file-removing destructors in `protolens` (`blob.rs`,
    `decode.rs`) are both `#[cfg(test)]` and are never on this path. A
    leaked temp file would be a real cost and this was checked
    specifically.

- **S2.** Scoped to the successful interactive return. The error arm
  still drops normally: `main` prints to stderr and returns
  `ExitCode::FAILURE`, and a failure is not a path to optimize by
  skipping cleanup.

## Alternatives considered

**`std::process::exit(0)`.** Achieves the same and is the usual
prescription. It skips *every* destructor in the process rather than one
object's, including any future stdout buffering, and it removes `main`'s
single exit point — which is where the other subcommands' `ExitCode`s
converge. `mem::forget` buys the same second while leaving all of that
alone.

**`malloc_trim` or an allocator swap.** Both change the shape of every
allocation in the program to fix a path that runs once, at the end.

**Freeing on a detached thread while the process exits.** Strictly worse
than not freeing: the same work, plus a thread, plus a race with exit.

## Test plan

1. `the_interactive_arm_does_not_drop_the_app` — the shape of this is
   the problem: a `mem::forget` is invisible to a unit test, and `main`
   is not callable from one. What *is* testable is that the reachable
   destructors are the ones enumerated in S1. Pinning that by test would
   mean asserting over `App`'s field types, which is a restatement of
   the struct rather than a check on it.

   So: **no unit test.** The claim is a measurement, and it is verified
   as one, below. The enumeration in S1 is the durable artifact.

2. Measured, on the pty harness that produced the table above: `:q` on
   `googleapis.desc` after a 45-second settle, idle. Must fall to the
   handback time.

3. Regression: the full workspace suite, and a manual `:q` that leaves
   the terminal in a usable state — the one thing a skipped teardown
   could plausibly break.

## Measured outcome

Same pty harness, same machine, `googleapis.desc` after a 45-second
settle:

| | before | after |
|---|---|---|
| terminal given back | 17 ms | 18 ms |
| process gone | 1075 ms | **126 ms** |
| after a 4 s `PageDown` burst | 1093 ms | **124 ms** |
| 20 KB `boblog` | — | 19 ms |

G1 is **not** literally met and the gap is worth stating: 126 ms against
19 ms, so about 107 ms still scales with the document. It is not
protolens. `perf` over that residual window resolves to **100%
`python3`** — the harness's own garbage collector — with not a single
protolens sample in it. What remains is the kernel tearing down the
address space at `exit_mmap`, which is proportional to resident memory
and is not reachable from user code short of allocating less (N1).

So: protolens's own user-space work at exit is now unmeasurable, and the
second the user reported is gone. Do not chase the 107 ms with another
`mem::forget` — there is nothing left in it to forget.

The prediction in the investigation that preceded this spec was "~20 ms",
which was wrong by the width of the kernel's teardown. The 8.5× is real;
the number was not.
