<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0442 — handles without a shared reference count

Status: draft (S1 implemented; S2 to S4 decided)
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
- **S2. Sealed and leaked pools** (opt-in; the handle types unchanged).
  - `Pool::seal(&self)`: the pool is complete. Its state is copied into
    a write-once cell (`OnceLock`), which lookups, decoding and encoding
    read without the lock: a read lock writes the lock's own counter,
    contended as the reference count is. Adding a file to a sealed pool
    panics, naming the file: a broken contract, not a runtime condition.
    The copy, not a move, so that a reader holding the lock across the
    seal still sees the whole state.
  - `Pool::leak(self) -> Pool`: seals, keeps one reference to the pool
    for ever, and returns a `Pool` whose handles do not count: cloning
    one copies a pointer, dropping one does nothing. The mark is the
    handle's own (the pool pointer's low bit), inherited by every handle
    derived from it, so a handle increments and decrements, or does
    neither: a pool-wide flag read at clone and at drop could see two
    values and decrement without having incremented. Handles made before
    the leak keep counting, balanced, and the kept reference stops the
    count reaching zero.
  - `Pool::vendored()`, a process-wide static never freed, is leaked when
    built: every user's descriptors decoded with it count nothing.
  - protoback leaks its pools once filled, before rendering (the pool
    loads and the options pool): a long-running program does not have
    to, and frees its pools as today.
  - Dismissed: borrowed handles (`Message<'p>`) everywhere, the lifetime
    reaching every type holding one, the Python bindings' included;
    owned or borrowed handles, a lifetime in every signature of two
    published crates; borrowed views beside owned handles (`MessageRef`
    …), a second API to keep, and the lock still taken.
- **S3. Each node's definition found once.** `Shared::def(i)` walked node
  `i`'s path (file, then field number and index, for example message 3,
  nested 1, field 2) down from the file's root on every call, a
  `get_by_number` and a `List::get` a step, many times a node. The
  definitions are found once, after phase 6, into a table `def(i)`
  indexes.
- **S4. Less hashing.** SipHash, Rust's default, about 3.9 % of the
  samples, nearly all on the main thread: `SymbolTable::build` (built,
  then every key renamed into a second map: the renaming now as each
  symbol is added), the 0440 symbol check (a `String` key a node, the
  map grown as it goes: sized up front), `graph::run`. protoback's maps
  use one hasher alias chosen at compile time: `rustc-hash`'s FxHash,
  unless the feature `hash-fold` (`foldhash`) or `hash-sip` (the
  standard library's) is enabled, `hash-sip` winning. FxHash is no
  feature of its own: the workspace's Nix builds pass
  `--no-default-features`, which would leave a default-feature choice
  with none. No output depends on a map's iteration
  order (the standard hasher is already seeded at random per run).
- **S5. Measured** before and after each of S1 to S4: the googleapis case at
  1, 2, 4 and 8 threads, phase by phase, task-clock, and the profile.

## Non-goals

- **N1.** Skipping the frees at exit: separate, once the 1.1 s is
  confirmed to be them.
- **N2.** Parallelizing phases 1 to 6, the scoring data or the schema
  database.
