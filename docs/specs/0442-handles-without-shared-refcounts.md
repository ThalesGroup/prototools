<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0442 — handles without a shared reference count

Status: draft (S1 implemented; S2's design to decide)
App: prototext-upb, protoback
Refs: docs/specs/0429-protoback-rendering.md (S4: files rendered on a
        rayon pool)

## Background

Measured 2026-11-17 on googleapis (googleapisDb's flags, a release build,
vCPUs 0-7, host idle; perf with software events, the VM exposing no
hardware counters): protoback takes ~11.0 s on one thread and ~9.0 s on
2, 4 or 8. The render phase, the only parallel one, is ~6.6 s on one
thread and ~5.3 s on two or more: it stops scaling at two. On eight
threads the run uses 23.7 s of CPU against 10.9 s on one.

The profile on eight threads: `check_field` 15.5 %, `get_by_number`
11.6 %, `Field::kind` 8.3 %, with `is_map`, `push_by_number`, `value` and
`List::get` about 48 % together, nearly all of it the `clone` and `drop`
of an `Arc<PoolInner>`; on one thread, the same functions are about 7 %.
Every handle (`File`, `Message`, `Field`, `Enum`, …) owns an
`Arc<PoolInner>`, and deriving one handle from another clones it: all the
threads increment and decrement one atomic counter, whose cache line
moves from core to core on every access.

What stays serial, whatever is done here: phases 1 to 6 (~1.3 s), the
scoring data (~1.1 s), the schema database (~1 s), and ~1.1 s after the
last line (presumably the frees before exit, to confirm).

## Goals

- **G1.** Reading a definition or a message's field takes no reference
  to the pool.
- **G2.** The render phase scales with the threads, to measure.

## Specification

- **S1. `check_field` compares definitions** (implemented 2026-11-17).
  It asserted `field.containing_message() == *def`, building a `Message`
  handle on every field access to compare and drop it; now
  `Field::belongs_to(&Message)` compares upb's pointers, no handle made.
- **S2. Handles derived from a handle borrow its pool.** Design to
  decide among:
  - **(a) Borrowed handles only:** `Message<'p>` holds `&'p PoolInner`.
    No count anywhere; but a handle stored beside its pool (a
    `DynamicMessage`, protoback's `Database`, a Python object of
    `prototext-pyo3`, which must be `'static`) needs the pool kept by
    something else, and the lifetime spreads into every type holding
    one.
  - **(b) Owned or borrowed:** `Message<'p>` holds either an
    `Arc<PoolInner>` or a `&'p PoolInner`. A handle made from another
    borrows (`'p` its source's), so the hot paths take no count; a
    handle kept is made owned (`into_owned()`, `Message<'static>`), as
    the Python bindings and stored descriptors need. One API, a lifetime
    in its signatures; a borrowed handle is two words, as today.
  - **(c) No API change:** the hot accessors (`get_by_number`, `kind`,
    `is_map`, `value`, `push_by_number`, `List::get`) use upb's pointers
    internally, and only the handles they return are counted. Least
    churn; does not remove the count of a handle the caller asks for
    (`Field::kind` returning `Kind::Message(Message)`), so may not reach
    G2.
- **S3. Measured** before and after S1, after S2: the googleapis case at
  1, 2, 4 and 8 threads, phase by phase, task-clock, and the profile.

## Non-goals

- **N1.** Skipping the frees at exit: separate, once the 1.1 s is
  confirmed to be them.
- **N2.** Parallelizing phases 1 to 6, the scoring data or the schema
  database.
